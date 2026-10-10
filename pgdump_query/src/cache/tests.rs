use std::process::Command;

use super::*;
use crate::model::CACHE_FORMAT_VERSION;
use crate::{LocalFileSource, ScanOptions, StatisticsRequest, XzSource, map_file};

/// Real `pg_dump` output whose columns gather bounds and dictionaries.
fn statistics_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/16/statistics/default.sql")
}

/// Every name in `dir`, sorted.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// Streaming changed where the bytes go, not what they are: a save writes
/// exactly what the owned [`CacheFile`] encodes to in one buffer, for a
/// plain source and for a compressed one carrying its seek table, with
/// statistics gathered — and reads back through the reader as the index it
/// wrote. The same file cut short reads as [`Unusable::Unreadable`] —
/// ours, so overwritable — never as an I/O error, and cut inside its magic
/// as not a pgdt cache.
#[tokio::test]
async fn a_save_writes_the_bytes_the_whole_file_encodes_to() {
    let dir = tempfile::tempdir().unwrap();
    let compressed = dir.path().join("default.sql.xz");
    let out = Command::new("xz")
        .args(["--block-size=65536", "-c"])
        .arg(statistics_fixture())
        .output()
        .expect("`xz` is not runnable, so this test cannot build its fixture; install it.");
    assert!(out.status.success(), "xz failed: {}", String::from_utf8_lossy(&out.stderr));
    std::fs::write(&compressed, &out.stdout).unwrap();

    let plain = LocalFileSource::open(statistics_fixture()).unwrap();
    let xz = XzSource::open(&compressed).unwrap();
    let sources: [(&str, &dyn ByteRangeSource); 2] = [("plain", &plain), ("xz", &xz)];
    for (name, source) in sources {
        let run = map_file(
            source,
            &ScanOptions::default(),
            &CacheMode::DISABLED,
            &StatisticsRequest::default(),
        )
        .await
        .unwrap();
        assert!(
            run.index.spans.iter().any(|span| matches!(
                &span.body,
                SpanBody::Data(DataBlock::Copy(block)) if block.statistics.as_ref().is_some_and(
                    |statistics| statistics.columns.iter().flatten().any(|column| {
                        column.bounds.is_some() && column.dictionary.is_some()
                    })
                )
            )),
            "{name}: the encoding under test must carry statistics"
        );
        assert_eq!(source.seek_table().is_some(), name == "xz");

        let path = dir.path().join(format!("{name}.dtcache"));
        save(&path, source, &run.index).await.unwrap();
        let whole = CacheFile {
            container_kind: ContainerKind::Plain,
            compression: source.seek_table().map(CompressionIndex::Xz),
            identity: SourceIdentity::observe(source).await.unwrap(),
            total_size: source.size().await.unwrap(),
            calendar_end: calendar_end(),
            index: run.index.clone(),
        };
        let bytes = std::fs::read(&path).unwrap();
        let header = [&CACHE_MAGIC[..], &CACHE_FORMAT_VERSION.to_le_bytes()].concat();
        assert_eq!(
            bytes,
            [
                header.clone(),
                bincode::serde::encode_to_vec(&whole, bincode::config::standard()).unwrap()
            ]
            .concat(),
            "{name}: the streamed save's bytes"
        );
        let Ok(Ok(read)) = read_cache_file(&path) else {
            panic!("{name}: the streamed save must read back");
        };
        // Diagnostics are not persisted, so the index is compared by its
        // encoding: what reads back encodes to what was written.
        assert_eq!(
            [header, bincode::serde::encode_to_vec(&read, bincode::config::standard()).unwrap()]
                .concat(),
            bytes,
            "{name}: the file read back"
        );

        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(
            matches!(read_cache_file(&path), Ok(Err(CacheStatus::Unusable(Unusable::Unreadable)))),
            "{name}: a cache cut short is unreadable, not an I/O failure"
        );
        std::fs::write(&path, &bytes[..CACHE_MAGIC.len() - 1]).unwrap();
        assert!(
            matches!(read_cache_file(&path), Ok(Err(CacheStatus::Unusable(Unusable::NotACache)))),
            "{name}: a file cut inside the magic never said it was a pgdt cache"
        );
    }
}

/// **A cache counted under another calendar is refused, and one holding
/// no count is not**: counts past a calendar's end are the calendar's, so
/// a build whose calendar ends elsewhere reads none of them as current,
/// while a map at the metadata level counted nothing to be wrong about
/// (`docs/design/decisions.md`, "D96"). The refusal is ours, so a `parse`
/// told it may overwrite the cache replaces it.
#[tokio::test]
async fn a_cache_counted_under_another_calendar_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("dump.sql");
    std::fs::copy(statistics_fixture(), &dump).unwrap();
    let source = LocalFileSource::open(&dump).unwrap();
    let mode = CacheMode::enabled(dir.path().join("scratch.dtcache"));
    let options = ScanOptions::default();
    let data = map_file(&source, &options, &mode, &StatisticsRequest::DATA).await.unwrap();
    let metadata = map_file(&source, &options, &CacheMode::DISABLED, &StatisticsRequest::METADATA)
        .await
        .unwrap();

    let other = calendar_end() - 1;
    for (name, index, refused) in
        [("data", &data.index, true), ("metadata", &metadata.index, false)]
    {
        let path = dir.path().join(format!("{name}.dtcache"));
        let file = CacheFileRef {
            container_kind: ContainerKind::Plain,
            compression: None,
            identity: SourceIdentity::observe(&source).await.unwrap(),
            total_size: source.size().await.unwrap(),
            calendar_end: other,
            index,
        };
        write_beside(&path, |writer| {
            writer.write_all(&CACHE_MAGIC)?;
            writer.write_all(&CACHE_FORMAT_VERSION.to_le_bytes())?;
            bincode::serde::encode_into_std_write(&file, writer, bincode::config::standard())
                .map(drop)
                .map_err(Error::cache_encode)
        })
        .unwrap();
        let status = load(&path, &source).await.unwrap();
        if refused {
            let why = Unusable::CalendarChanged { counted_under: other, build: calendar_end() };
            assert_eq!(status, CacheStatus::Unusable(why.clone()), "{name}");
            assert!(why.overwritable());
            let message = Error::CacheUnusable { path: path.clone(), unusable: why }.to_string();
            assert!(
                message.contains("262142-12-30") && message.contains("262142-12-31"),
                "{message}"
            );
        } else {
            assert!(matches!(status, CacheStatus::Valid { .. }), "{name}: {status:?}");
        }
    }
}

/// A kill mid-save leaves the previous cache whole. No test races a
/// signal (`docs/design/decisions.md`, "D73"), so the save is stopped at
/// the moment a kill would land — partway through its encoding — and what
/// it leaves on disk is read there: the previous cache, untouched, and the
/// partial file beside it. A save that fails removes that file, and one
/// that finishes replaces the cache with nothing left beside it.
#[test]
fn a_save_leaves_the_previous_cache_whole_until_it_renames() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dump.sql.dtcache");
    std::fs::write(&path, b"the previous cache").unwrap();

    let failed = write_beside(&path, |writer| {
        writer.write_all(b"the first half of the next")?;
        writer.flush()?;
        assert_eq!(std::fs::read(&path).unwrap(), b"the previous cache");
        let names = listing(dir.path());
        assert_eq!(names.len(), 2, "the cache and the save's own file: {names:?}");
        let partial = names.iter().find(|name| *name != "dump.sql.dtcache").unwrap();
        assert!(
            partial.starts_with("dump.sql.dtcache.") && partial.ends_with(".tmp"),
            "the partial file is named for its cache: {partial}"
        );
        assert_eq!(std::fs::read(dir.path().join(partial)).unwrap(), b"the first half of the next");
        Err(Error::CacheModeMismatch("the save stops here"))
    });
    assert!(matches!(failed, Err(Error::CacheModeMismatch(_))));
    assert_eq!(std::fs::read(&path).unwrap(), b"the previous cache");
    assert_eq!(listing(dir.path()), ["dump.sql.dtcache"]);

    write_beside(&path, |writer| {
        writer.write_all(b"the next cache")?;
        assert_eq!(std::fs::read(&path).unwrap(), b"the previous cache");
        Ok(())
    })
    .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"the next cache");
    assert_eq!(listing(dir.path()), ["dump.sql.dtcache"]);
}
