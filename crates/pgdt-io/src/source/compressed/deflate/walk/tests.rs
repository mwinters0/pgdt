use std::io::Write;

use crate::source::compressed::CompressedFileSource;
use crate::test_support::*;
use crate::{
    ByteRangeSource, CompressionIndex, KnownCompression, LocalFileSource, Origin, Recognized,
};

use super::*;

/// The spacing the tests coalesce at: several members a unit over the
/// payload, and several units.
const SPACING: NonZeroU64 = NonZeroU64::new(128 * 1024).unwrap();

/// A payload of several hundred kilobytes, so BGZF writes it as a dozen or
/// more members.
fn payload() -> Vec<u8> {
    let mut payload = gzip_test_payload();
    payload.extend_from_slice(&gzip_test_payload());
    payload
}

/// `path` walked on the arm `arm` names, pulled or fetched.
async fn walked(arm: &str, path: &std::path::Path, spacing: NonZeroU64) -> Option<GzipIndex> {
    let stored = std::fs::metadata(path).unwrap().len();
    let named = path.display().to_string();
    match arm {
        "pulled" => {
            walk_file_spaced(&named, &std::fs::File::open(path).unwrap(), stored, spacing).unwrap()
        }
        _ => {
            let source = LocalFileSource::open(path).unwrap();
            walk_fetched_spaced(&named, &source, stored, spacing).await.unwrap()
        }
    }
}

/// **A BGZF file's index is read off its headers, alike on both arms**:
/// complete, every unit beginning at a member start with no window, units of
/// at least the spacing but the last, every member counted though only those
/// beginning a unit hold a checkpoint, and every unit read back as its
/// plaintext through the source the index opens.
#[tokio::test]
async fn a_bgzf_file_is_walked_into_its_index_on_both_arms() {
    let payload = payload();
    let file = bgzf_compress(&payload);
    let pulled = walked("pulled", file.path(), SPACING).await.unwrap();
    let fetched = walked("fetched", file.path(), SPACING).await.unwrap();
    assert_eq!(pulled, fetched);
    let every_member = walked("pulled", file.path(), NonZeroU64::MIN).await.unwrap();

    assert!(pulled.is_complete() && pulled.is_seekable());
    assert_eq!(pulled.uncompressed_size(), payload.len() as u64);
    assert!(pulled.unit_count() > 2, "{} units", pulled.unit_count());
    assert_eq!(pulled.member_count(), every_member.checkpoint_count());
    assert!(pulled.member_count() > pulled.unit_count());
    let units: Vec<_> = pulled.unit_ranges().collect();
    for (unit, range) in units.iter().enumerate() {
        assert_eq!(pulled.window_len(unit), Some(0), "unit {unit} begins at a member");
        if unit + 1 < units.len() {
            assert!(range.end - range.start >= SPACING.get(), "unit {unit}: {range:?}");
        }
    }

    let source =
        CompressedFileSource::with_index(file.path(), CompressionIndex::Gzip(pulled)).unwrap();
    for range in units {
        let got = source.read_range(range.start, (range.end - range.start) as usize).await.unwrap();
        assert!(got[..] == payload[range.start as usize..range.end as usize], "{range:?}");
    }
}

/// **The walk reads ahead**: one read a chunk, never one a member.
#[test]
fn a_walk_reads_a_chunk_at_a_time() {
    let mut payload = Vec::new();
    while payload.len() < 3 * WALK_CHUNK as usize {
        payload.extend_from_slice(&gzip_test_payload());
    }
    let file = std::fs::read(bgzf_compress(&payload).path()).unwrap();
    let mut walk = Walking::begin("bgzf", file.len() as u64, SPACING);
    let (mut reads, mut members) = (0, None);
    while members.is_none() {
        let (offset, len) = walk.next_read();
        assert!(len as u64 <= WALK_CHUNK.max(gz_seek::BGZF_HEADER_LEN as u64 + 4));
        reads += 1;
        let end = (offset as usize + len).min(file.len());
        if let ControlFlow::Break(walked) = walk.supply(offset, &file[offset as usize..end], None) {
            members = Some(walked.unwrap().member_count());
        }
    }
    let chunks = file.len().div_ceil(WALK_CHUNK as usize);
    assert!(reads <= chunks + 1, "{reads} reads over {chunks} chunks");
    assert!(members.unwrap() > 4 * reads, "{members:?} members in {reads} reads");
}

/// A BGZF file followed by `tail`, as one file.
fn bgzf_then(payload: &[u8], tail: &[u8]) -> tempfile::NamedTempFile {
    let mut bytes = std::fs::read(bgzf_compress(payload).path()).unwrap();
    bytes.extend_from_slice(tail);
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&bytes).unwrap();
    file.flush().unwrap();
    file
}

/// **A chain that breaks leaves the file to a first pass**, on both arms: a
/// BGZF file followed by a member that is not BGZF's — gzip of several
/// members, which `open` then reads whole by its first pass — and one cut
/// short inside a member.
#[tokio::test]
async fn a_broken_chain_is_left_to_a_first_pass() {
    let payload = payload();
    let single = std::fs::read(gzip_compress(b"and a member that is not BGZF's\n").path()).unwrap();
    let mixed = bgzf_then(&payload, &single);
    let whole = std::fs::read(bgzf_compress(&payload).path()).unwrap();
    let mut cut = tempfile::NamedTempFile::new().unwrap();
    cut.write_all(&whole[..whole.len() / 2]).unwrap();
    cut.flush().unwrap();
    for arm in ["pulled", "fetched"] {
        assert!(walked(arm, mixed.path(), SPACING).await.is_none(), "{arm}: a mixed file");
        assert!(walked(arm, cut.path(), SPACING).await.is_none(), "{arm}: a file cut short");
    }

    let Recognized::Source(source) =
        crate::open(&Origin::local(mixed.path()), KnownCompression::Unknown, &std::env::temp_dir())
            .await
            .unwrap()
    else {
        panic!("claiming nothing is never a mismatch");
    };
    assert!(source.extent().await.unwrap().exact().is_none(), "a first pass, its end open");
    let mut expected = payload.clone();
    expected.extend_from_slice(b"and a member that is not BGZF's\n");
    let got = source.read_at_most(0, expected.len() + 1).await.unwrap();
    assert!(got[..] == expected[..], "the first pass reads every member");
}

/// **`open` walks a BGZF file and builds no first pass**: the source it hands
/// back answers a complete index and an exact extent before any read, as
/// does `CompressedFileSource::open`, which refuses a single member by name.
#[tokio::test]
async fn open_walks_a_bgzf_file() {
    let payload = payload();
    let file = bgzf_compress(&payload);
    let Recognized::Source(source) =
        crate::open(&Origin::local(file.path()), KnownCompression::Unknown, &std::env::temp_dir())
            .await
            .unwrap()
    else {
        panic!("claiming nothing is never a mismatch");
    };
    let Some(CompressionIndex::Gzip(index)) = source.compression_index() else {
        panic!("a BGZF file is opened over its gzip index");
    };
    assert!(index.is_complete());
    assert_eq!(source.extent().await.unwrap().exact(), Some(payload.len() as u64));

    let direct = CompressedFileSource::open(file.path()).unwrap();
    assert_eq!(direct.compression_index(), Some(CompressionIndex::Gzip(index)));
    let single = gzip_compress(&payload);
    let refused = CompressedFileSource::open(single.path()).err().unwrap();
    assert!(refused.to_string().contains("not BGZF"), "{refused}");
}
