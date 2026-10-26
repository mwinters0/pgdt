use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bytes::Bytes;

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

/// What a pass over `path` names it by.
fn named(path: &std::path::Path) -> String {
    Origin::local(path).to_string()
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
        let pass = FirstPass::spaced(
            named(compressed.path()),
            feed(feed_name, compressed.path()),
            stored,
            SPACING,
            &std::env::temp_dir(),
        )
        .unwrap();
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

/// **The pass holds its windows in a file with no name, made where it is
/// told**: a pass spilling into a directory of its own leaves nothing there,
/// halfway or at its end, while every window reads back as `gz_seek`'s own
/// pass placed it. One told a directory that does not exist reads a file under
/// the spacing whole, placing no window holding bytes, and refuses the read
/// that would place one, naming the directory, rather than holding the window
/// in memory.
#[tokio::test]
async fn a_first_pass_spills_its_windows_where_it_is_told() {
    let payload = gzip_test_payload();
    let (compressed, expected) = gzip_indexed(&payload, SPACING.get());
    let stored = std::fs::metadata(compressed.path()).unwrap().len();
    let dir = tempfile::tempdir().unwrap();
    let left_in = |dir: &std::path::Path| std::fs::read_dir(dir).unwrap().count();
    for feed_name in FEEDS {
        let pass = FirstPass::spaced(
            named(compressed.path()),
            feed(feed_name, compressed.path()),
            stored,
            SPACING,
            dir.path(),
        )
        .unwrap();
        let half = pass.read_range(0, payload.len() / 2).await.unwrap();
        let partial = gzip_index_of(&pass);
        assert!(partial.checkpoint_count() > 1, "{feed_name}: windows were placed");
        assert_eq!(windows_of(&partial), windows_of(&expected)[..partial.checkpoint_count()]);
        assert_eq!(left_in(dir.path()), 0, "{feed_name}: the spill file has no name");
        scanned(&pass, half.len() as u64, 1 << 20).await;
        assert_eq!(windows_of(&gzip_index_of(&pass)), windows_of(&expected), "{feed_name}");
        assert_eq!(left_in(dir.path()), 0, "{feed_name}: the spill file has no name");
    }

    let missing = dir.path().join("absent");
    let small = &payload[..SPACING.get() as usize / 2];
    let (small_compressed, _) = gzip_indexed(small, SPACING.get());
    let small_stored = std::fs::metadata(small_compressed.path()).unwrap().len();
    for feed_name in FEEDS {
        let pass = FirstPass::spaced(
            named(small_compressed.path()),
            feed(feed_name, small_compressed.path()),
            small_stored,
            SPACING,
            &missing,
        )
        .unwrap();
        assert_eq!(scanned(&pass, 0, 4096).await, small, "{feed_name}");

        let pass = FirstPass::spaced(
            named(compressed.path()),
            feed(feed_name, compressed.path()),
            stored,
            SPACING,
            &missing,
        )
        .unwrap();
        let refused = pass.read_range(0, payload.len() / 2).await.unwrap_err().to_string();
        assert!(
            refused.contains(&missing.display().to_string()) && refused.contains("windows"),
            "{feed_name}: {refused}"
        );
    }
    assert!(!missing.exists());
}

/// **The pass hands over to the indexed source at the end**: its reads and
/// partitioning are then the indexed source's, so a range is cut at unit
/// starts, a read anywhere answers the payload's bytes, and a read past the
/// end is short where it may be and refused where it may not.
#[tokio::test]
async fn a_finished_pass_reads_as_the_indexed_source() {
    let payload = gzip_test_payload();
    let (compressed, expected) = gzip_indexed(&payload, SPACING.get());
    let stored = std::fs::metadata(compressed.path()).unwrap().len();
    for feed_name in FEEDS {
        let pass = FirstPass::spaced(
            named(compressed.path()),
            feed(feed_name, compressed.path()),
            stored,
            SPACING,
            &std::env::temp_dir(),
        )
        .unwrap();
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
/// same index. Its reads answer the payload's bytes, which a continued decode
/// and a restarted one both do, across a closed unit's end and into the open
/// unit, and every closed unit it leaves, or stops inside as the reads
/// complete, compares its check and passes.
#[tokio::test]
async fn a_read_behind_the_frontier_reads_from_its_checkpoint() {
    let payload = gzip_test_payload();
    let (compressed, expected) = gzip_indexed(&payload, SPACING.get());
    let stored = std::fs::metadata(compressed.path()).unwrap().len();
    for feed_name in FEEDS {
        let pass = FirstPass::spaced(
            named(compressed.path()),
            feed(feed_name, compressed.path()),
            stored,
            SPACING,
            &std::env::temp_dir(),
        )
        .unwrap();
        let ahead = payload.len() * 2 / 3;
        pass.read_range(0, ahead).await.unwrap();
        let open_unit = gzip_index_of(&pass).unit_ranges().last().unwrap().end;
        let second = gzip_index_of(&pass).unit_ranges().nth(1).unwrap();
        let across = second.end as usize - 50;
        for (from, len) in [
            (10, 5000),
            (5010, 3000),
            (open_unit as usize + 7, 900),
            (0, 64),
            (across, 100),
            (open_unit as usize - 20, 40),
            (second.start as usize + 3, 10),
        ] {
            let read = pass.read_range(from as u64, len).await.unwrap();
            assert_eq!(&read[..], &payload[from..from + len], "{feed_name}: {from}+{len}");
        }
        pass.complete_reads().await.unwrap();
        assert_eq!(pass.extent().await.unwrap(), Extent::Open { at_least: ahead as u64 });
        // Straddling the frontier, from behind it.
        let read = pass.read_range(ahead as u64 - 100, 300).await.unwrap();
        assert_eq!(&read[..], &payload[ahead - 100..ahead + 200], "{feed_name}");
        let rest = scanned(&pass, ahead as u64, 4096).await;
        assert_eq!(&rest[..], &payload[ahead..], "{feed_name}");
        assert_eq!(gzip_index_of(&pass), expected, "{feed_name}");
    }
}

/// A unit decode's refusal of a unit whose plaintext does not match the
/// CRC-32 its index recorded.
fn is_unit_check(error: &Error) -> bool {
    matches!(error.codec_cause::<gz_seek::Error>(), Some(gz_seek::Error::UnitCheck { .. }))
}

/// **A read behind the frontier inside a closed unit is checked against the
/// CRC-32 that unit recorded**, whatever provider its window was read through:
/// over a resumed index whose provider hands out a damaged window for a unit
/// the earlier pass closed, a read leaving that unit is refused, one stopped
/// inside it is refused as the reads complete — once, the unit being left —
/// and so is the read whose decode leaves it for another; while a read of an
/// undamaged closed unit, and one inside the open unit, the frontier's own,
/// answer the payload's bytes. On both feeds.
#[tokio::test]
async fn a_read_behind_the_frontier_is_checked_against_its_units_crc() {
    let payload = gzip_test_payload();
    let (compressed, _) = gzip_indexed(&payload, SPACING.get());
    let stored = std::fs::metadata(compressed.path()).unwrap().len();
    let interrupted = FirstPass::spaced(
        named(compressed.path()),
        Feed::file(compressed.path()).unwrap(),
        stored,
        SPACING,
        &std::env::temp_dir(),
    )
    .unwrap();
    interrupted.read_range(0, payload.len() - 1000).await.unwrap();
    let saved = gzip_index_of(&interrupted);
    let units: Vec<_> = saved.unit_ranges().collect();
    assert!(units.len() >= 3, "{units:?}");
    let damaged = 1;
    let mut windows = windows_of(&saved);
    assert!(!windows[damaged].is_empty(), "the damaged unit begins inside the member");
    windows[damaged].iter_mut().for_each(|byte| *byte ^= 0x55);
    let held = Arc::new(super::super::held::HeldWindows(windows));
    let saved = saved.with_windows(held);
    let unit = units[damaged].clone();
    let open = units.last().unwrap().end as usize;
    for feed_name in FEEDS {
        let resumed = || {
            FirstPass::resumed(
                named(compressed.path()),
                feed(feed_name, compressed.path()),
                stored,
                saved.clone(),
                &std::env::temp_dir(),
            )
            .unwrap()
        };

        let pass = resumed();
        let error = pass.read_range(unit.start + 5, (unit.end - unit.start) as usize).await;
        assert!(is_unit_check(&error.unwrap_err()), "{feed_name}: leaving the unit");
        for (from, len) in [(10, 3000), (units[2].start as usize + 9, 500), (open + 11, 700)] {
            let read = pass.read_range(from as u64, len).await.unwrap();
            assert_eq!(&read[..], &payload[from..from + len], "{feed_name}: {from}+{len}");
        }
        pass.complete_reads().await.unwrap();

        let pass = resumed();
        pass.read_range(unit.start + 5, 10).await.unwrap();
        let error = pass.complete_reads().await.unwrap_err();
        assert!(is_unit_check(&error), "{feed_name}: as the reads complete: {error}");
        pass.complete_reads().await.unwrap();

        let pass = resumed();
        pass.read_range(unit.start + 5, 10).await.unwrap();
        let error = pass.read_range(0, 10).await.unwrap_err();
        assert!(is_unit_check(&error), "{feed_name}: leaving it for another: {error}");
        let read = pass.read_range(0, 10).await.unwrap();
        assert_eq!(&read[..], &payload[..10], "{feed_name}");
        let rest = scanned(&pass, open as u64, 4096).await;
        assert_eq!(&rest[..], &payload[open..], "{feed_name}");
    }
}

/// **A pass resumed from the index an interrupted one saved builds the index
/// an uninterrupted pass builds**: from a checkpoint inside the member and
/// from the member's start, on both feeds, reading on from before its last
/// checkpoint, or from it where it is the member's start, and from past it; and the resumed pass answers the saved index
/// until it places a checkpoint of its own, so a cache saved from it is
/// judged against what it holds.
#[tokio::test]
async fn a_resumed_pass_builds_the_uninterrupted_index() {
    let payload = gzip_test_payload();
    let (compressed, expected) = gzip_indexed(&payload, SPACING.get());
    let stored = std::fs::metadata(compressed.path()).unwrap().len();
    for stop in [payload.len() / 2, 100] {
        let interrupted = FirstPass::spaced(
            named(compressed.path()),
            Feed::file(compressed.path()).unwrap(),
            stored,
            SPACING,
            &std::env::temp_dir(),
        )
        .unwrap();
        interrupted.read_range(0, stop).await.unwrap();
        let saved = gzip_index_of(&interrupted);
        let last = saved.unit_ranges().last().map_or(0, |unit| unit.end);
        for feed_name in FEEDS {
            for from in [last.saturating_sub(50), last + 3] {
                let resumed = FirstPass::resumed(
                    named(compressed.path()),
                    feed(feed_name, compressed.path()),
                    stored,
                    saved.clone(),
                    &std::env::temp_dir(),
                )
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
    let pass = FirstPass::spaced(
        named(compressed.path()),
        Feed::file(compressed.path()).unwrap(),
        stored,
        SPACING,
        &std::env::temp_dir(),
    )
    .unwrap();
    pass.read_range(0, payload.len() - 1).await.unwrap();
    let saved = gzip_index_of(&pass);

    let (shorter, _) = gzip_indexed(&payload[..payload.len() / 4], SPACING.get());
    let short_size = std::fs::metadata(shorter.path()).unwrap().len();
    let refused = FirstPass::resumed(
        named(shorter.path()),
        Feed::file(shorter.path()).unwrap(),
        short_size,
        saved.clone(),
        &std::env::temp_dir(),
    )
    .err()
    .expect("refused");
    assert!(index_refused(&refused), "{refused}");
    let known = KnownCompression::Compressed(CompressionIndex::Gzip(saved.clone()));
    let origin = Origin::local(shorter.path());
    assert!(matches!(
        crate::open(&origin, known, &std::env::temp_dir()).await.unwrap(),
        Recognized::Mismatch
    ));

    let known = KnownCompression::Compressed(CompressionIndex::Gzip(saved));
    let Recognized::Source(resumed) =
        crate::open(&Origin::local(compressed.path()), known, &std::env::temp_dir()).await.unwrap()
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
        let pass = FirstPass::spaced(
            named(file.path()),
            Feed::file(file.path()).unwrap(),
            stored,
            SPACING,
            &std::env::temp_dir(),
        )
        .unwrap();
        let read = pass.read_at_most(0, payload.len() + 1).await;
        assert!(read.is_err(), "{what}: {read:?}");
        assert!(pass.read_at_most(0, 10).await.is_err(), "{what}: the pass stopped there");
    }
}

/// Chunks small enough that the test payload is a dozen of them, the shipped
/// lead's. Its member as [`gzip_compress`] writes it ends a deflate block
/// only every few chunks; [`flushed_indexed`]'s ends several in each.
const CHUNKING: Chunking = Chunking {
    bytes: NonZeroU64::new(8 * 1024).unwrap(),
    limit: 256 * 1024,
    lead: Chunking::SHIPPED.lead,
};

/// A parallelism of `jobs` workers whose budget affords every one of them.
fn lifted(jobs: usize) -> Parallelism {
    Parallelism::workers(jobs, 64 * CHUNKING.task_bytes())
}

/// How many chunks' decodes `pass`'s frontier has taken from a task so far.
fn taken(pass: &FirstPass) -> u64 {
    pass.lock().frontier.as_ref().map_or(0, |frontier| frontier.decode.taken())
}

/// **A pass inflates ahead on the workers its parallelism affords and builds
/// the serial pass's index, on either feed**: under each count the plaintext
/// whole and the index and windows `gz_seek`'s forward pass builds. A pulled
/// pass recommends what it can use, a fetched one one worker, its tasks
/// running only under a count stated for it.
#[tokio::test]
async fn a_pass_inflates_ahead_on_either_feed_and_builds_the_serial_index() {
    let payload = gzip_test_payload();
    let (compressed, expected) = gzip_indexed(&payload, SPACING.get());
    let stored = std::fs::metadata(compressed.path()).unwrap().len();
    assert!(CHUNKING.chunks(stored) > 8, "{stored} bytes");
    let cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    for feed_name in FEEDS {
        for jobs in [1, 2, 4, 8] {
            let what = format!("{feed_name} at {jobs}");
            let pass = FirstPass::chunked(
                named(compressed.path()),
                feed(feed_name, compressed.path()),
                stored,
                SPACING,
                CHUNKING,
                &std::env::temp_dir(),
            )
            .unwrap();
            let recommended = match feed_name {
                "pulled" => cores.min(CHUNKING.chunks(stored) as usize),
                _ => 1,
            };
            assert_eq!(pass.default_workers(), recommended, "{what}");
            pass.hint_parallelism(lifted(jobs));
            assert_eq!(pass.ahead_tasks(), jobs - 1, "{what}");

            let most = scanned(&pass, 0, 7919).await;
            assert_eq!(most, payload, "{what}");
            let built = gzip_index_of(&pass);
            assert_eq!(built, expected, "{what}");
            assert_eq!(windows_of(&built), windows_of(&expected), "{what}");
        }
    }
}

/// `path` read through a transport recording every range asked of it.
struct Recording {
    inner: LocalFileSource,
    asked: Mutex<Vec<(u64, usize)>>,
}

impl Recording {
    fn over(path: &std::path::Path) -> Arc<Recording> {
        let inner = LocalFileSource::open(path).unwrap();
        Arc::new(Recording { inner, asked: Mutex::new(Vec::new()) })
    }

    /// Every range asked for, in file order.
    fn asked(&self) -> Vec<(u64, usize)> {
        let mut asked = self.asked.lock().unwrap().clone();
        asked.sort_unstable();
        asked
    }
}

impl ByteRangeSource for Recording {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        self.asked.lock().unwrap().push((offset, len));
        self.inner.read_range(offset, len)
    }

    fn extent(&self) -> Pin<Box<dyn Future<Output = Result<Extent>> + Send + '_>> {
        self.inner.extent()
    }

    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        self.inner.modified()
    }
}

/// **A fetched pass downloads the object once, whatever runs ahead**: at
/// each count every compressed byte is asked of the transport exactly once,
/// the ranges tiling the object.
#[tokio::test]
async fn a_fetched_pass_downloads_the_object_once_with_tasks_ahead() {
    let payload = gzip_test_payload();
    let (compressed, expected) = gzip_indexed(&payload, SPACING.get());
    let stored = std::fs::metadata(compressed.path()).unwrap().len();
    for jobs in [1, 2, 4, 8] {
        let raw = Recording::over(compressed.path());
        let pass = FirstPass::chunked(
            named(compressed.path()),
            Feed::Fetched(Arc::clone(&raw) as Arc<dyn ByteRangeSource>),
            stored,
            SPACING,
            CHUNKING,
            &std::env::temp_dir(),
        )
        .unwrap();
        pass.hint_parallelism(lifted(jobs));
        let most = payload.len() * 3 / 4;
        let mut got = pass.read_range(0, most).await.unwrap().to_vec();
        got.extend_from_slice(&scanned(&pass, most as u64, 7919).await);
        assert_eq!(got, payload, "at {jobs}");
        assert_eq!(gzip_index_of(&pass), expected, "at {jobs}");

        let mut next = 0;
        for (offset, len) in raw.asked() {
            assert_eq!(offset, next, "at {jobs}: {:?}", raw.asked());
            next += len as u64;
        }
        assert_eq!(next, stored, "at {jobs}: the object, once");
    }
}

/// The test payload as one gzip member flushed every 8 KiB of plaintext, so
/// its deflate blocks end several times a [`CHUNKING`] chunk whatever backend
/// `flate2` is built over, and its complete index at [`SPACING`].
fn flushed_indexed(payload: &[u8]) -> (tempfile::NamedTempFile, GzipIndex) {
    use std::io::Write as _;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    for piece in payload.chunks(8 * 1024) {
        encoder.write_all(piece).unwrap();
        encoder.flush().unwrap();
    }
    let bytes = encoder.finish().unwrap();
    let (file, _) = source_of(&bytes);
    let index = crate::source::gzip_indexed_in_memory(&bytes, SPACING.get());
    (file, index)
}

/// How long a test waits for a task ahead to land before failing: far past
/// what decoding one chunk of the test payload takes on a loaded machine.
const LANDING_BOUND: std::time::Duration = std::time::Duration::from_secs(60);

/// A read short of any chunk's plaintext, so no read carries the frontier
/// across a whole chunk.
const STEP: usize = 1024;

/// Read `pass` on from `got.len()` a [`STEP`] at a time, letting every task
/// it hands out land between reads, until it holds a decode ahead of its
/// frontier: whether it does before the end. A wait for a landing is bounded
/// by [`LANDING_BOUND`].
async fn read_until_a_decode_is_ready(pass: &FirstPass, got: &mut Vec<u8>, end: usize) -> bool {
    while got.len() < end {
        let len = STEP.min(end - got.len());
        got.extend_from_slice(&pass.read_range(got.len() as u64, len).await.unwrap());
        loop {
            let (ready, running) =
                pass.lock().frontier.as_mut().map_or((0, 0), |frontier| frontier.decode.settle());
            if ready > 0 {
                return true;
            }
            if running == 0 {
                break;
            }
            tokio::time::timeout(LANDING_BOUND, pass.landed.notified())
                .await
                .expect("a task ahead lands within the bound");
        }
    }
    false
}

/// **The frontier takes the chunks its tasks decoded**, on either feed at
/// every count above one, over a member ending several blocks a chunk: read a
/// step at a time, every task let land between steps, until a decode is held
/// ahead of the frontier, then read on — at a read size that ends nowhere in
/// particular and at one wider than a chunk's plaintext — and that chunk is
/// taken. Sequenced rather than raced: whether a decode lands before the
/// frontier reaches it is otherwise the scheduler's
/// (`docs/design/decisions.md`, "D73").
#[tokio::test]
async fn a_lifted_frontier_takes_the_chunks_its_tasks_decoded() {
    let payload = gzip_test_payload();
    let (compressed, expected) = flushed_indexed(&payload);
    let stored = std::fs::metadata(compressed.path()).unwrap().len();
    for feed_name in FEEDS {
        for jobs in [2, 4, 8] {
            for read in [4099, 1 << 20] {
                let what = format!("{feed_name} at {jobs}, reads of {read}");
                let pass = FirstPass::chunked(
                    named(compressed.path()),
                    feed(feed_name, compressed.path()),
                    stored,
                    SPACING,
                    CHUNKING,
                    &std::env::temp_dir(),
                )
                .unwrap();
                pass.hint_parallelism(lifted(jobs));
                let (mut got, most) = (Vec::new(), payload.len() * 3 / 4);
                let ready = read_until_a_decode_is_ready(&pass, &mut got, most).await;
                assert!(ready, "{what}: no decode landed ahead of the frontier");
                while got.len() < most {
                    let len = read.min(most - got.len());
                    got.extend_from_slice(&pass.read_range(got.len() as u64, len).await.unwrap());
                }
                // Before the end, where the pass hands over and its frontier goes.
                assert!(taken(&pass) > 0, "{what}: every chunk decoded by the frontier");
                got.extend_from_slice(&scanned(&pass, most as u64, read).await);
                assert_eq!(got, payload, "{what}");
                assert_eq!(gzip_index_of(&pass), expected, "{what}");
            }
        }
    }
}

/// **A pass resumed with tasks ahead builds the uninterrupted index**, from
/// the index a pass interrupted with tasks ahead saved, inside the member and
/// at its start, on either feed.
#[tokio::test]
async fn a_lifted_pass_resumes_into_the_uninterrupted_index() {
    let payload = gzip_test_payload();
    let (compressed, expected) = gzip_indexed(&payload, SPACING.get());
    let stored = std::fs::metadata(compressed.path()).unwrap().len();
    for feed_name in FEEDS {
        for stop in [payload.len() / 2, 100] {
            let what = format!("{feed_name}, stopped at {stop}");
            let interrupted = FirstPass::chunked(
                named(compressed.path()),
                feed(feed_name, compressed.path()),
                stored,
                SPACING,
                CHUNKING,
                &std::env::temp_dir(),
            )
            .unwrap();
            interrupted.hint_parallelism(lifted(4));
            interrupted.read_range(0, stop).await.unwrap();
            let saved = gzip_index_of(&interrupted);
            let resumed = FirstPass::resumed_chunked(
                named(compressed.path()),
                feed(feed_name, compressed.path()),
                stored,
                saved,
                CHUNKING,
                &std::env::temp_dir(),
            )
            .unwrap();
            resumed.hint_parallelism(lifted(4));
            let rest = scanned(&resumed, 0, 6007).await;
            assert_eq!(rest, payload, "{what}");
            let built = gzip_index_of(&resumed);
            assert_eq!(built, expected, "{what}");
            assert_eq!(windows_of(&built), windows_of(&expected), "{what}");
        }
    }
}

/// **Damage is refused with tasks ahead as without**: a file cut short
/// inside its member fails the read that meets the cut, whichever decoded
/// the chunks before it, on either feed.
#[tokio::test]
async fn a_damaged_file_is_refused_with_tasks_ahead() {
    let payload = gzip_test_payload();
    let (compressed, _) = gzip_indexed(&payload, SPACING.get());
    let bytes = std::fs::read(compressed.path()).unwrap();
    let damaged = bytes[..bytes.len() - 20].to_vec();
    let (file, _source) = source_of(&damaged);
    for feed_name in FEEDS {
        let pass = FirstPass::chunked(
            named(file.path()),
            feed(feed_name, file.path()),
            damaged.len() as u64,
            SPACING,
            CHUNKING,
            &std::env::temp_dir(),
        )
        .unwrap();
        pass.hint_parallelism(lifted(4));
        assert!(pass.read_at_most(0, payload.len() + 1).await.is_err(), "{feed_name}");
    }
}

/// **A wait never rests on a thread the runtime has yet to give**: on a
/// runtime of one blocking thread, which the frontier's step holds, every
/// task handed out during a read queues behind it. So one read, a step into
/// the first chunk, is let land its speculations, nothing anchored behind
/// them; the next, wider than the chunks they guessed, anchors at the first
/// guess and reaches the decodes it then hands out with none started, a
/// pulled frontier running each it waits for itself. Both feeds reach the
/// end with the serial pass's index. **A fetched frontier's wait is not
/// reached here, and no test reaches it**: it steps a piece at a time, going
/// back to the runtime for each, where the decode's own pieces run first, so
/// only a race between threads makes it stand at a decode still running.
#[test]
fn a_frontier_waits_for_a_task_that_nothing_has_started_on_either_feed() {
    // No lead: the guesses the first read lets land are of the first chunks,
    // whose few block starts are the chain the wide read anchors.
    let chunking = Chunking { lead: 0, ..CHUNKING };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_time()
        .build()
        .unwrap();
    let payload = gzip_test_payload();
    let (compressed, expected) = gzip_indexed(&payload, SPACING.get());
    let stored = std::fs::metadata(compressed.path()).unwrap().len();
    for feed_name in FEEDS {
        runtime.block_on(async {
            let pass = FirstPass::chunked(
                named(compressed.path()),
                feed(feed_name, compressed.path()),
                stored,
                SPACING,
                chunking,
                &std::env::temp_dir(),
            )
            .unwrap();
            pass.hint_parallelism(lifted(4));
            let mut got = pass.read_range(0, STEP).await.unwrap().to_vec();
            loop {
                let (ready, running) = pass
                    .lock()
                    .frontier
                    .as_mut()
                    .map_or((0, 0), |frontier| frontier.decode.settle());
                assert_eq!(ready, 0, "{feed_name}: a decode before the frontier anchored");
                if running == 0 {
                    break;
                }
                tokio::time::timeout(LANDING_BOUND, pass.landed.notified())
                    .await
                    .expect("a task ahead lands within the bound");
            }
            let wide = payload.len() / 2;
            got.extend_from_slice(&pass.read_range(STEP as u64, wide).await.unwrap());
            let (waits, ran_here) = pass.lock().frontier.as_ref().unwrap().decode.waits();
            if feed_name == "pulled" {
                assert!(waits > 0, "{feed_name}: the frontier waited for no decode");
                assert_eq!(ran_here, waits, "{feed_name}: a wait for a decode started");
            }
            got.extend_from_slice(&scanned(&pass, got.len() as u64, 1 << 20).await);
            assert!(got == payload, "{feed_name}");
            assert_eq!(gzip_index_of(&pass), expected, "{feed_name}");
        });
    }
}

/// Chunks a few BGZF members long, as the shipped chunk is many: the frontier
/// then stands at a member start within each chunk, passing the guesses it
/// holds as it goes, where in a chunk shorter than a member it holds them for
/// chunks at a time.
const BGZF_CHUNKING: Chunking = Chunking {
    bytes: NonZeroU64::new(64 * 1024).unwrap(),
    limit: 1024 * 1024,
    lead: Chunking::SHIPPED.lead,
};

/// **A fetched BGZF file inflates ahead over one download**, each chunk ahead
/// of the frontier read off its members' headers (`gz_seek`'s
/// `a_bgzf_chunk_is_read_off_its_members_headers`): at every count above one the frontier takes a chunk its tasks decoded —
/// sequenced as [`a_lifted_frontier_takes_the_chunks_its_tasks_decoded`] is —
/// the pass builds the index a forward pass builds, which is the walk's
/// (`docs/design/decisions.md`, "D116"), and every compressed byte is asked of
/// the transport exactly once.
#[tokio::test]
async fn a_fetched_bgzf_file_inflates_ahead_over_one_download() {
    let payload = gzip_test_payload().repeat(12);
    let file = bgzf_compress(&payload);
    let bytes = std::fs::read(file.path()).unwrap();
    let expected = crate::source::gzip_indexed_in_memory(&bytes, SPACING.get());
    let stored = bytes.len() as u64;
    assert!(BGZF_CHUNKING.chunks(stored) > 8, "{stored} bytes");
    for jobs in [2, 4, 8] {
        let raw = Recording::over(file.path());
        let pass = FirstPass::chunked(
            named(file.path()),
            Feed::Fetched(Arc::clone(&raw) as Arc<dyn ByteRangeSource>),
            stored,
            SPACING,
            BGZF_CHUNKING,
            &std::env::temp_dir(),
        )
        .unwrap();
        pass.hint_parallelism(Parallelism::workers(jobs, 64 * BGZF_CHUNKING.task_bytes()));
        let (mut got, most) = (Vec::new(), payload.len() * 3 / 4);
        let ready = read_until_a_decode_is_ready(&pass, &mut got, most).await;
        assert!(ready, "at {jobs}: no decode landed ahead of the frontier");
        while got.len() < most {
            let len = 4099.min(most - got.len());
            got.extend_from_slice(&pass.read_range(got.len() as u64, len).await.unwrap());
        }
        assert!(taken(&pass) > 0, "at {jobs}: every chunk decoded by the frontier");
        got.extend_from_slice(&scanned(&pass, most as u64, 4099).await);
        assert!(got == payload, "at {jobs}");
        assert_eq!(gzip_index_of(&pass), expected, "at {jobs}");

        let mut next = 0;
        for (offset, len) in raw.asked() {
            assert_eq!(offset, next, "at {jobs}: {:?}", raw.asked());
            next += len as u64;
        }
        assert_eq!(next, stored, "at {jobs}: the object, once");
    }
}
