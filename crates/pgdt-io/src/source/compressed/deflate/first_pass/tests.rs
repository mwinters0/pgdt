use std::sync::Arc;

use crate::source::compressed::index_refused;
use crate::test_support::*;
use crate::{
    ByteRangeSource, CompressionIndex, Extent, KnownCompression, LocalFileSource, Origin,
    Recognized,
};

use super::*;

/// The spacing the tests place checkpoints at: a few dozen units over the
/// payload, each past several deflate blocks.
const SPACING: NonZeroU64 = NonZeroU64::new(32 * 1024).unwrap();

/// The two feeds a pass reads through: the file pulled from, and the same
/// file fetched through a transport.
const FEEDS: [&str; 2] = ["pulled", "fetched"];

/// A feed over `path` of the kind `name` names.
fn feed(name: &str, path: &std::path::Path) -> Feed {
    match name {
        "pulled" => Feed::file(path).unwrap(),
        _ => Feed::Fetched(Arc::new(LocalFileSource::open(path).unwrap())),
    }
}

/// `source` read forward from `from` to its end in `chunk`-byte reads, as a
/// scan to an open end reads.
async fn scanned(source: &dyn ByteRangeSource, from: u64, chunk: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let read = source.read_at_most(from + out.len() as u64, chunk).await.unwrap();
        out.extend_from_slice(&read);
        if read.len() < chunk {
            return out;
        }
    }
}

/// `source`'s gzip index.
fn gzip_index_of(source: &dyn ByteRangeSource) -> GzipIndex {
    match source.compression_index() {
        Some(CompressionIndex::Gzip(index)) => index,
        other => panic!("a gzip first pass answers a gzip index, not {other:?}"),
    }
}

/// Every window `index` holds, read through its provider.
fn windows_of(index: &GzipIndex) -> Vec<Vec<u8>> {
    (0..index.checkpoint_count())
        .map(|c| {
            let mut window = vec![0u8; index.window_len(c).unwrap()];
            index.read_window(c, &mut window).unwrap();
            window
        })
        .collect()
}

/// **A first pass reads the file and builds the index a forward pass over it
/// builds**, on both feeds and at a chunk that ends nowhere in particular:
/// the plaintext whole, the extent open until the read that meets the end and
/// exact from it, and the complete index, every checkpoint's window read
/// through the provider beside it, equal to `gz_seek`'s own pass's.
#[tokio::test]
async fn a_first_pass_reads_the_file_and_builds_its_index() {
    let payload = gzip_test_payload();
    let (compressed, expected) = gzip_indexed(&payload, SPACING.get());
    let stored = std::fs::metadata(compressed.path()).unwrap().len();
    for feed_name in FEEDS {
        let pass = FirstPass::spaced(feed(feed_name, compressed.path()), stored, SPACING).unwrap();
        assert_eq!(pass.extent().await.unwrap(), Extent::Open { at_least: 0 }, "{feed_name}");
        assert_eq!(pass.default_workers(), 1, "{feed_name}");
        let half = pass.read_range(0, payload.len() / 2).await.unwrap();
        assert_eq!(&half[..], &payload[..payload.len() / 2], "{feed_name}");
        let Extent::Open { at_least } = pass.extent().await.unwrap() else {
            panic!("{feed_name}: the end is not known halfway");
        };
        assert_eq!(at_least, payload.len() as u64 / 2, "{feed_name}");
        let partial = gzip_index_of(&pass);
        assert!(!partial.is_complete() && partial.unit_count() >= 1, "{feed_name}");

        let rest = scanned(&pass, half.len() as u64, 7919).await;
        assert_eq!(&rest[..], &payload[half.len()..], "{feed_name}");
        assert_eq!(pass.extent().await.unwrap(), Extent::Exact(payload.len() as u64));
        let built = gzip_index_of(&pass);
        assert_eq!(built, expected, "{feed_name}");
        assert_eq!(windows_of(&built), windows_of(&expected), "{feed_name}");
    }
}

/// **The pass hands over to the indexed source at the end**: every read,
/// the partitioning and the worker recommendation are then the indexed
/// source's, so a read anywhere decodes one unit and a range is cut at unit
/// starts; a read past the end is short where it may be and refused where it
/// may not.
#[tokio::test]
async fn a_finished_pass_reads_as_the_indexed_source() {
    let payload = gzip_test_payload();
    let (compressed, expected) = gzip_indexed(&payload, SPACING.get());
    let stored = std::fs::metadata(compressed.path()).unwrap().len();
    for feed_name in FEEDS {
        let pass = FirstPass::spaced(feed(feed_name, compressed.path()), stored, SPACING).unwrap();
        pass.hint_parallelism(crate::Parallelism::workers(4, 1 << 30));
        let advice = pass.partitions(0..payload.len() as u64);
        assert_eq!(advice.max_partitions(), Some(1), "{feed_name}: {advice:?}");
        scanned(&pass, 0, 1 << 20).await;
        let len = payload.len() as u64;
        let advice = pass.partitions(0..len);
        let crate::PartitionBoundaries::At(cuts) = advice.boundaries() else {
            panic!("{feed_name}: the indexed source cuts at its units: {advice:?}");
        };
        let starts: Vec<u64> = expected.unit_ranges().map(|unit| unit.start).collect();
        assert!(!cuts.is_empty(), "{feed_name}: the indexed source cuts at unit starts");
        assert!(cuts.iter().all(|cut| starts.contains(cut)), "{feed_name}: {cuts:?}");
        let mid = len / 3;
        assert_eq!(&pass.read_range(mid, 100).await.unwrap()[..], &payload[mid as usize..][..100]);
        assert_eq!(pass.read_at_most(len - 10, 100).await.unwrap().len(), 10, "{feed_name}");
        assert!(pass.read_range(len - 10, 100).await.is_err(), "{feed_name}");
    }
}

/// **A read behind the frontier decodes from the last checkpoint at or
/// before it** — inside a unit the pass has closed and inside the one it is
/// in — and leaves the frontier where it stood, so the pass then builds the
/// same index; a scan behind it continues one decode rather than restarting.
#[tokio::test]
async fn a_read_behind_the_frontier_reads_from_its_checkpoint() {
    let payload = gzip_test_payload();
    let (compressed, expected) = gzip_indexed(&payload, SPACING.get());
    let stored = std::fs::metadata(compressed.path()).unwrap().len();
    for feed_name in FEEDS {
        let pass = FirstPass::spaced(feed(feed_name, compressed.path()), stored, SPACING).unwrap();
        let ahead = payload.len() * 2 / 3;
        pass.read_range(0, ahead).await.unwrap();
        let open_unit = gzip_index_of(&pass).unit_ranges().last().unwrap().end;
        for (from, len) in [(10, 5000), (5010, 3000), (open_unit as usize + 7, 900), (0, 64)] {
            let read = pass.read_range(from as u64, len).await.unwrap();
            assert_eq!(&read[..], &payload[from..from + len], "{feed_name}: {from}+{len}");
        }
        assert_eq!(pass.extent().await.unwrap(), Extent::Open { at_least: ahead as u64 });
        // Straddling the frontier, from behind it.
        let read = pass.read_range(ahead as u64 - 100, 300).await.unwrap();
        assert_eq!(&read[..], &payload[ahead - 100..ahead + 200], "{feed_name}");
        let rest = scanned(&pass, ahead as u64, 4096).await;
        assert_eq!(&rest[..], &payload[ahead..], "{feed_name}");
        assert_eq!(gzip_index_of(&pass), expected, "{feed_name}");
    }
}

/// **A pass resumed from the index an interrupted one saved builds the index
/// an uninterrupted pass builds**: from a checkpoint inside the member and
/// from the member's start, on both feeds, reading on from before its last
/// checkpoint and from past it; and the resumed pass answers the saved index
/// until it places a checkpoint of its own, so a cache saved from it is
/// judged against what it holds.
#[tokio::test]
async fn a_resumed_pass_builds_the_uninterrupted_index() {
    let payload = gzip_test_payload();
    let (compressed, expected) = gzip_indexed(&payload, SPACING.get());
    let stored = std::fs::metadata(compressed.path()).unwrap().len();
    for stop in [payload.len() / 2, 100] {
        let interrupted =
            FirstPass::spaced(Feed::file(compressed.path()).unwrap(), stored, SPACING).unwrap();
        interrupted.read_range(0, stop).await.unwrap();
        let saved = gzip_index_of(&interrupted);
        let last = saved.unit_ranges().last().map_or(0, |unit| unit.end);
        for feed_name in FEEDS {
            for from in [last.saturating_sub(50), last + 3] {
                let resumed =
                    FirstPass::resumed(feed(feed_name, compressed.path()), stored, saved.clone())
                        .unwrap();
                assert_eq!(gzip_index_of(&resumed), saved, "{feed_name} from {from}");
                assert_eq!(windows_of(&gzip_index_of(&resumed)), windows_of(&saved));
                let rest = scanned(&resumed, from, 5003).await;
                assert_eq!(&rest[..], &payload[from as usize..], "{feed_name} from {from}");
                let built = gzip_index_of(&resumed);
                assert_eq!(built, expected, "{feed_name} stopped at {stop}, read from {from}");
                assert_eq!(windows_of(&built), windows_of(&expected), "{feed_name}");
            }
        }
    }
}

/// **A saved index that does not describe the file is refused as such**:
/// one whose last checkpoint lies past the file's end is the error
/// recognition turns into a mismatch, and through [`open`](crate::open) it is
/// one.
#[tokio::test]
async fn a_saved_index_past_the_files_end_is_a_mismatch() {
    let payload = gzip_test_payload();
    let (compressed, _) = gzip_indexed(&payload, SPACING.get());
    let stored = std::fs::metadata(compressed.path()).unwrap().len();
    let pass = FirstPass::spaced(Feed::file(compressed.path()).unwrap(), stored, SPACING).unwrap();
    pass.read_range(0, payload.len() - 1).await.unwrap();
    let saved = gzip_index_of(&pass);

    let (shorter, _) = gzip_indexed(&payload[..payload.len() / 4], SPACING.get());
    let short_size = std::fs::metadata(shorter.path()).unwrap().len();
    let refused =
        FirstPass::resumed(Feed::file(shorter.path()).unwrap(), short_size, saved.clone())
            .err()
            .expect("refused");
    assert!(index_refused(&refused), "{refused}");
    let known = KnownCompression::Compressed(CompressionIndex::Gzip(saved.clone()));
    let origin = Origin::local(shorter.path());
    assert!(matches!(crate::open(&origin, known).await.unwrap(), Recognized::Mismatch));

    let known = KnownCompression::Compressed(CompressionIndex::Gzip(saved));
    let Recognized::Source(resumed) =
        crate::open(&Origin::local(compressed.path()), known).await.unwrap()
    else {
        panic!("the file the index was saved from resumes over it");
    };
    let rest = scanned(&*resumed, 0, 1 << 20).await;
    assert_eq!(rest, payload);
}

/// **A file the pass cannot finish is refused where it fails**: one cut
/// short inside its member at the read that meets the cut, one carrying
/// bytes after its last member there too — and the pass does not go on past
/// a failure, behind it or ahead.
#[tokio::test]
async fn a_damaged_file_is_refused_where_the_pass_meets_it() {
    let payload = gzip_test_payload();
    let (compressed, _) = gzip_indexed(&payload, SPACING.get());
    let bytes = std::fs::read(compressed.path()).unwrap();
    let mut trailing = bytes.clone();
    trailing.extend_from_slice(b"not gzip");
    for (what, damaged) in [("cut short", bytes[..bytes.len() - 20].to_vec()), ("run on", trailing)]
    {
        let (file, _source) = source_of(&damaged);
        let stored = damaged.len() as u64;
        let pass = FirstPass::spaced(Feed::file(file.path()).unwrap(), stored, SPACING).unwrap();
        let read = pass.read_at_most(0, payload.len() + 1).await;
        assert!(read.is_err(), "{what}: {read:?}");
        assert!(pass.read_at_most(0, 10).await.is_err(), "{what}: the pass stopped there");
    }
}
