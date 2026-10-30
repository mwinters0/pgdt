use std::path::{Path, PathBuf};

use pgdt_io::{ByteRangeSource, CompressedFileSource, Extent};
use pgdt_model::{CACHE_FORMAT_VERSION, DumpIndex, calendar_end};

use super::super::{
    CACHE_MAGIC, CacheFile, CacheFileRef, CacheStatus, ContainerKind, read_cache_file,
    write_cache_file,
};
use super::*;
use crate::cache::identity::SourceIdentity;

/// The spacing the tests index at: several checkpoints over the payload,
/// most inside the member, so most carry a window.
const SPACING: u64 = 16 * 1024;

/// A payload of many lines, compressed as one gzip member by `flate2`, in a
/// directory of its own: the file, its plaintext, and its index with every
/// window held in memory.
fn gzip_file() -> (tempfile::TempDir, PathBuf, Vec<u8>, GzipIndex) {
    let plaintext: Vec<u8> = (0..20_000u32)
        .flat_map(|i| format!("{i}\tvalue {}\n", i.wrapping_mul(7919) % 10_007).into_bytes())
        .collect();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&plaintext).unwrap();
    let compressed = encoder.finish().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dump.sql.gz");
    std::fs::write(&path, &compressed).unwrap();
    let index = pgdt_io::internal::gzip_indexed_in_memory(&compressed, SPACING);
    (dir, path, plaintext, index)
}

/// Every checkpoint's window, read through whichever provider `index` carries.
fn windows_of(index: &GzipIndex) -> Vec<Vec<u8>> {
    (0..index.checkpoint_count())
        .map(|checkpoint| {
            let mut window = vec![0u8; index.window_len(checkpoint).unwrap()];
            index.read_window(checkpoint, &mut window).unwrap();
            window
        })
        .collect()
}

/// A cache of the gzip file at `dump` under `compression`, mapping nothing.
async fn cache_of(dump: &Path, compression: CompressionIndex) -> (CacheFile, Extent) {
    let source = CompressedFileSource::with_index(dump, compression.clone()).unwrap();
    let extent = source.extent().await.unwrap();
    let file = CacheFile {
        container_kind: ContainerKind::Plain,
        compression: Some(compression),
        identity: SourceIdentity::observe(&source).await.unwrap(),
        extent,
        calendar_end: calendar_end(),
        index: DumpIndex::default(),
    };
    (file, extent)
}

fn write(path: &Path, file: &CacheFile) {
    let file = CacheFileRef {
        container_kind: file.container_kind,
        compression: file.compression.clone(),
        identity: file.identity.clone(),
        extent: file.extent,
        calendar_end: file.calendar_end,
        index: &file.index,
    };
    write_cache_file(path, &file).unwrap();
}

/// **A gzip cache is the encoded file, then each checkpoint's window in
/// checkpoint order**, read through the provider its index carried; it reads
/// back as the same index carrying a provider of its own, which reads each
/// window back out of the section. A file cut inside the section, or run on
/// past it, is unreadable — ours, so overwritable — never an index whose
/// windows are someone else's bytes.
#[tokio::test]
async fn a_gzip_cache_holds_each_window_after_the_encoded_file() {
    let (dir, dump, _, index) = gzip_file();
    let windows = windows_of(&index);
    assert!(windows.len() > 2, "{} checkpoints", windows.len());
    assert!(windows[0].is_empty() && windows[1..].iter().all(|w| !w.is_empty()));
    let (file, _) = cache_of(&dump, CompressionIndex::Gzip(index.clone())).await;
    let path = dir.path().join("dump.sql.gz.pgdt");
    write(&path, &file);

    let bytes = std::fs::read(&path).unwrap();
    let header = [&CACHE_MAGIC[..], &CACHE_FORMAT_VERSION.to_le_bytes()].concat();
    let encoded = bincode::serde::encode_to_vec(&file, bincode::config::standard()).unwrap();
    assert_eq!(bytes, [header, encoded, windows.concat()].concat(), "the saved bytes");

    let Ok(Ok(read)) = read_cache_file(&path) else { panic!("the cache reads back") };
    let Some(CompressionIndex::Gzip(back)) = &read.compression else { panic!("a gzip index") };
    assert_eq!(back, &index);
    assert!(back.has_windows(), "a load attaches the section's provider");
    assert_eq!(windows_of(back), windows, "each window read back out of the section");
    drop(read);

    for (cut, damaged) in [
        ("cut inside the section", &bytes[..bytes.len() - 1]),
        ("run on past it", &[&bytes[..], b"x"].concat()[..]),
    ] {
        std::fs::write(&path, damaged).unwrap();
        assert!(
            matches!(read_cache_file(&path), Ok(Err(CacheStatus::Unusable(Unusable::Unreadable)))),
            "a gzip cache {cut} is unreadable"
        );
    }
}

/// **A save of an index loaded from the cache it replaces writes the same
/// windows**, read through the loaded file's handle, which the rename over
/// its name leaves on the bytes it was loaded from; and a source opened on
/// the loaded index reads the whole plaintext, every unit inside the member
/// begun from a window the section holds.
#[tokio::test]
async fn a_cache_saved_over_itself_keeps_its_windows_and_reads_its_file() {
    let (dir, dump, plaintext, index) = gzip_file();
    let (file, extent) = cache_of(&dump, CompressionIndex::Gzip(index)).await;
    let path = dir.path().join("dump.sql.gz.pgdt");
    write(&path, &file);
    let first = std::fs::read(&path).unwrap();

    for save in 0..2 {
        let Ok(Ok(loaded)) = read_cache_file(&path) else { panic!("the cache reads back") };
        write(&path, &loaded);
        assert_eq!(std::fs::read(&path).unwrap(), first, "save {save} over the cache");
    }

    let Ok(Ok(loaded)) = read_cache_file(&path) else { panic!("the cache reads back") };
    std::fs::remove_file(&path).unwrap();
    let source = CompressedFileSource::with_index(&dump, loaded.compression.unwrap()).unwrap();
    assert_eq!(extent, Extent::Exact(plaintext.len() as u64));
    let read = source.read_range(0, plaintext.len()).await.unwrap();
    assert_eq!(&read[..], &plaintext[..], "the plaintext, its windows read from a removed cache");
    source.complete_reads().await.unwrap();
}

/// **A window read failing names the cache**: a cache whose section is cut
/// away after the load attached it fails the first decode needing a window,
/// and the error the read carried out of the provider is the cache's, its path
/// and what was being done to it, never a bare `io error:`.
#[tokio::test]
async fn a_failing_window_read_names_the_cache() {
    let (dir, dump, plaintext, index) = gzip_file();
    let (file, _) = cache_of(&dump, CompressionIndex::Gzip(index)).await;
    let path = dir.path().join("dump.sql.gz.pgdt");
    write(&path, &file);
    let Ok(Ok(loaded)) = read_cache_file(&path) else { panic!("the cache reads back") };
    let header = CACHE_MAGIC.len() + CACHE_FORMAT_VERSION.to_le_bytes().len();
    let encoded = bincode::serde::encode_to_vec(&file, bincode::config::standard()).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len((header + encoded.len()) as u64)
        .unwrap();

    let source = CompressedFileSource::with_index(&dump, loaded.compression.unwrap()).unwrap();
    let error = source.read_range(0, plaintext.len()).await.expect_err("no window to read");
    let Error::File { doing, path: named, .. } = &error else { panic!("not named: {error:?}") };
    assert_eq!((*doing, named.as_path()), ("read the cache at", path.as_path()));
}
