use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use bytes::Bytes;

use pgdt_core::{Error, Result};

use crate::source::compressed::walk_seek_table;
use crate::source::origin::ORIGIN_LEADING_BYTES;
use crate::test_support::*;
use crate::*;

use super::*;

/// **One mechanism, three ways of reaching it, one answer.** A forward scan
/// over the same file read through the local whole-block arm, the local
/// piecewise arm and the fetched window-fed arm must agree byte for byte:
/// the source a block handle pulls from is the only difference between them
/// (`docs/design/decisions.md`, "D15"), so a divergence
/// surfaces here as a disagreement between two read paths over one byte
/// stream rather than between two files.
#[tokio::test]
async fn the_two_piecewise_arms_and_the_block_arm_read_one_file_alike() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);

    let whole = CompressedFileSource::open(compressed.path()).unwrap();
    let pieces = CompressedFileSource::open(compressed.path()).unwrap();
    pieces.hint_parallelism(Parallelism::Serial { memory_bytes: Some(1) });
    assert!(whole.budget.block_path().is_some() && pieces.budget.block_path().is_none());
    let (fetched, _) = fetched_xz(compressed.path()).await;
    fetched.hint_parallelism(Parallelism::Serial { memory_bytes: Some(1) });
    assert!(fetched.budget.block_path().is_none(), "the window-fed twin of `pieces`");

    // Chunks that land inside one block, straddle a boundary and span
    // several, read forward as every real read loop here reads.
    let mut offset = 0u64;
    for len in [100usize, 4096, 3000, 5000] {
        let want = &payload[offset as usize..offset as usize + len];
        assert_eq!(&whole.read_range(offset, len).await.unwrap()[..], want, "whole {offset}");
        assert_eq!(&pieces.read_range(offset, len).await.unwrap()[..], want, "pieces {offset}");
        assert_eq!(&fetched.read_range(offset, len).await.unwrap()[..], want, "fetched {offset}");
        offset += len as u64;
    }
}

/// Flip the last byte of block `index`'s stored check, leaving its payload
/// — and so every byte it decodes to — as it was. The check is the last
/// field of a block's padded extent, so that byte is always the check's.
fn corrupt_check(path: &Path, index: usize) {
    let table = seek_table_of(&CompressedFileSource::open(path).unwrap());
    let block = &table.0.blocks[index];
    let at = usize::try_from(block.compressed_offset + block.total_size() - 1).unwrap();
    let mut bytes = std::fs::read(path).unwrap();
    bytes[at] ^= 0xFF;
    std::fs::write(path, bytes).unwrap();
}

/// Whether `result` is a block failing its own check.
fn check_failed<T: std::fmt::Debug>(result: Result<T>) -> bool {
    matches!(&result, Err(error) if matches!(error.codec_cause(), Some(xz_seek::Error::BlockCheckFailed { .. })))
}

/// **A block a read stopped inside owes its check until the run ends.**
/// The bytes out of it go out before the check can be compared, since the
/// check covers the whole block, and `complete_reads` is what compares it —
/// on both piecewise arms, and on none where nothing was left inside a
/// block.
#[tokio::test]
async fn a_block_a_read_stopped_inside_is_compared_when_reads_complete() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    corrupt_check(compressed.path(), 1);

    let local = CompressedFileSource::open(compressed.path()).unwrap();
    local.hint_parallelism(Parallelism::Serial { memory_bytes: Some(1) });
    let (fetched, _) = fetched_xz(compressed.path()).await;
    fetched.hint_parallelism(Parallelism::Serial { memory_bytes: Some(1) });
    let sources: [&dyn ByteRangeSource; 2] = [&local, &fetched];
    for source in sources {
        // Inside the corrupted block and short of its end: the bytes are
        // right, the check not yet compared.
        let got = source.read_range(4096, 100).await.unwrap();
        assert_eq!(&got[..], &payload[4096..4196]);
        assert!(check_failed(source.complete_reads().await));
        // Nothing is left owing, and a read after it begins the block again.
        source.complete_reads().await.unwrap();
        assert_eq!(&source.read_range(4196, 100).await.unwrap()[..], &payload[4196..4296]);
        assert!(check_failed(source.complete_reads().await));

        // Read to its end, the block compares on the read itself, which is
        // `xz_seek`'s own rule and leaves nothing owing.
        assert!(check_failed(source.read_range(4096, 4096).await));
        source.complete_reads().await.unwrap();
        // An intact block completes as cleanly.
        source.read_range(100, 100).await.unwrap();
        source.complete_reads().await.unwrap();
    }
}

/// **A raised budget does not leave the piecewise block unverified.** The
/// fetched arm cannot keep a window beside the block arm's slots, so it
/// completes the block there; the local arm leaves its handle seated, for
/// the run's end to complete.
#[tokio::test]
async fn a_block_left_behind_by_a_raised_budget_is_still_compared() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    corrupt_check(compressed.path(), 1);

    let (fetched, _) = fetched_xz(compressed.path()).await;
    fetched.hint_parallelism(Parallelism::Serial { memory_bytes: Some(1) });
    fetched.read_range(4096, 100).await.unwrap();
    fetched.hint_parallelism(Parallelism::default());
    assert!(fetched.budget.block_path().is_some());
    assert!(check_failed(fetched.read_range(0, 100).await));

    let local = CompressedFileSource::open(compressed.path()).unwrap();
    local.hint_parallelism(Parallelism::Serial { memory_bytes: Some(1) });
    local.read_range(4096, 100).await.unwrap();
    local.hint_parallelism(Parallelism::default());
    assert_eq!(&local.read_range(0, 100).await.unwrap()[..], &payload[..100]);
    assert!(check_failed(local.complete_reads().await));
}

// -----------------------------------------------------------------------
// The fetched `.xz` source: one block at a time, out of windows the caller
// fetched (`docs/design/decisions.md`, "D14", "D15" and "D18").
//
// The transport here is a local file, which is exactly the point: the
// window-fed path and its local twin, `CompressedFileSource`, then read one file, so
// putting a file through it is what makes a divergence surface as a disagreement between
// two read paths over one byte stream rather than between two sources.
// -----------------------------------------------------------------------

/// A `ByteRangeSource` that counts the reads made through it, so an
/// assertion about the *request stream* — how many fetches a walk spends —
/// is possible without a server.
struct Counting {
    inner: Arc<dyn ByteRangeSource>,
    reads: Arc<AtomicUsize>,
}

impl ByteRangeSource for Counting {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.inner.read_range(offset, len)
    }
    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        self.inner.size()
    }
    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        self.inner.modified()
    }
}

/// A fetched source over the compressed file at `path`, walked the way
/// `open_remote` walks one: the probe's leading bytes first, then whatever
/// the machine asks for.
async fn fetched_xz(path: &Path) -> (FetchedCompressedSource, usize) {
    let transport: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(path).unwrap());
    let stored_size = transport.size().await.unwrap();
    let leading = transport.read_range(0, ORIGIN_LEADING_BYTES).await.unwrap();
    let (table, fetches) = walk_seek_table(&*transport, stored_size, &leading).await.unwrap();
    (
        FetchedCompressedSource::with_index(transport, stored_size, CompressionIndex::Xz(table))
            .unwrap(),
        fetches,
    )
}

/// The composition, end to end: a table built by driving the walk over a
/// byte-range source, and every byte of the file read back out of fetched
/// windows.
#[tokio::test]
async fn a_fetched_xz_source_reads_what_the_file_holds() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let (source, _) = fetched_xz(compressed.path()).await;

    assert_eq!(source.size().await.unwrap(), payload.len() as u64);
    assert_eq!(
        source.stored_size().await.unwrap(),
        std::fs::metadata(compressed.path()).unwrap().len()
    );
    assert!(seek_table_of(&source).block_count() > 1, "the fixture must be split");
    // Inside one block, straddling two, and the whole stream.
    assert_eq!(&source.read_range(100, 200).await.unwrap()[..], &payload[100..300]);
    assert_eq!(&source.read_range(4_000, 4_000).await.unwrap()[..], &payload[4_000..8_000]);
    assert_eq!(&source.read_range(0, payload.len()).await.unwrap()[..], &payload[..]);
}

/// The walked table is the same table the local source's own walk
/// produces over the same file — the two drivers differ in who fetches
/// and in nothing else.
#[tokio::test]
async fn a_walked_table_is_the_one_the_local_walk_produces() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let (source, _) = fetched_xz(compressed.path()).await;
    let local = seek_table_of(&CompressedFileSource::open(compressed.path()).unwrap());
    assert_eq!(seek_table_of(&source), local);
}

/// **The probe's bytes are the walk's first request**, so a caller holding
/// them spends no round trip on it: the same walk costs one more fetch
/// when nothing is handed in.
#[tokio::test]
async fn the_walk_spends_no_fetch_on_leading_bytes_the_caller_holds() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let file: Arc<dyn ByteRangeSource> =
        Arc::new(LocalFileSource::open(compressed.path()).unwrap());
    let stored_size = file.size().await.unwrap();
    let leading = file.read_range(0, ORIGIN_LEADING_BYTES).await.unwrap();

    let reads = Arc::new(AtomicUsize::new(0));
    let counting = Counting { inner: Arc::clone(&file), reads: Arc::clone(&reads) };
    let (_, with_probe) = walk_seek_table(&counting, stored_size, &leading).await.unwrap();
    assert_eq!(with_probe, reads.load(Ordering::Relaxed));

    let reads = Arc::new(AtomicUsize::new(0));
    let counting = Counting { inner: file, reads: Arc::clone(&reads) };
    let (_, without) = walk_seek_table(&counting, stored_size, &[]).await.unwrap();
    assert_eq!(without, with_probe + 1);
}

/// The piecewise arm: a budget too small to hold a decoded block reads the
/// same bytes out of the same windows, through a live handle rather
/// than into a pooled slot (D15), each read asserted against the plaintext
/// the file was compressed from.
#[tokio::test]
async fn a_budget_too_small_for_a_block_reads_the_same_bytes_in_pieces() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let (source, _) = fetched_xz(compressed.path()).await;
    assert!(source.budget.block_path().is_some(), "the default budget holds a 4 KiB block");

    source.hint_parallelism(Parallelism::Serial { memory_bytes: Some(1) });
    assert!(source.budget.block_path().is_none(), "a one-byte budget holds no block");

    assert_eq!(&source.read_range(100, 200).await.unwrap()[..], &payload[100..300]);
    assert_eq!(&source.read_range(4_000, 4_000).await.unwrap()[..], &payload[4_000..8_000]);
    assert_eq!(&source.read_range(0, payload.len()).await.unwrap()[..], &payload[..]);
}

/// **A block a forward scan sits inside is decoded once**, not once per
/// read: the piecewise arm keeps its handle across reads as the local one
/// does, so what costs a decode is leaving a block and coming back to it
/// (D15). Counted rather than argued, which is what the `introspect`
/// build is for — and it is never a timed build.
#[cfg(feature = "introspect")]
#[tokio::test]
async fn a_fetched_piecewise_scan_inside_one_block_decodes_it_once() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let (source, _) = fetched_xz(compressed.path()).await;
    source.hint_parallelism(Parallelism::Serial { memory_bytes: Some(1) });
    assert!(source.budget.block_path().is_none(), "the piecewise arm, not the block cache");
    let table = seek_table_of(&source);
    let first = table.0.blocks[0].uncompressed_size as usize;
    let step = first / 8;
    assert!(step > 0, "the fixture's blocks hold several reads apiece: {first} byte(s)");

    for read in 0..4usize {
        let at = read * step;
        assert_eq!(&source.read_range(at as u64, step).await.unwrap()[..], &payload[at..at + step]);
    }
    assert_eq!(source.block_decodes_begun(), 1, "four forward reads, one decode");

    // Leaving the block and coming back is what costs one apiece, which
    // is the local arm's shape exactly.
    let second = table.0.blocks[1].uncompressed_offset as usize;
    assert_eq!(
        &source.read_range(second as u64, step).await.unwrap()[..],
        &payload[second..second + step]
    );
    assert_eq!(&source.read_range(0, step).await.unwrap()[..], &payload[..step]);
    assert_eq!(source.block_decodes_begun(), 3, "one per block entered");
}

/// A read past the end of the uncompressed stream is the same
/// `UnexpectedEof` every other source raises, on both arms.
#[tokio::test]
async fn a_read_past_the_end_is_refused_on_both_arms() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let (source, _) = fetched_xz(compressed.path()).await;
    let past = payload.len() as u64;
    for budget in [None, Some(1)] {
        source.hint_parallelism(Parallelism::Serial { memory_bytes: budget });
        let err = source.read_range(past - 10, 100).await.unwrap_err();
        assert!(matches!(&err, Error::Io(e) if e.kind() == std::io::ErrorKind::UnexpectedEof));
    }
}

/// A table that does not describe this object is refused by `xz_seek`'s
/// validation without a byte being read, which is what `open_remote`
/// reports as `Recognized::Mismatch` rather than paying a walk.
#[tokio::test]
async fn a_table_from_another_file_is_an_invalid_table_rather_than_a_read() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let other = xz_compress(&payload[..1000], &["--block-size=512"]);
    let table = seek_table_of(&CompressedFileSource::open(other.path()).unwrap());

    let transport: Arc<dyn ByteRangeSource> =
        Arc::new(LocalFileSource::open(compressed.path()).unwrap());
    let stored_size = transport.size().await.unwrap();
    let err =
        FetchedCompressedSource::with_index(transport, stored_size, CompressionIndex::Xz(table))
            .unwrap_err();
    assert!(matches!(err.codec_cause(), Some(xz_seek::Error::InvalidTable { .. })), "{err:?}");
}

/// The single-block shape reads correctly too — there is no seek structure
/// to cut at, so every read decodes the one block from its start, and the
/// piecewise arm is what keeps that from allocating the whole plaintext.
#[tokio::test]
async fn a_single_block_file_is_read_by_the_same_two_arms() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &[]);
    let (source, _) = fetched_xz(compressed.path()).await;
    assert!(!seek_table_of(&source).is_seekable());
    assert_eq!(&source.read_range(1_000, 500).await.unwrap()[..], &payload[1_000..1_500]);
    source.hint_parallelism(Parallelism::Serial { memory_bytes: Some(1) });
    assert_eq!(&source.read_range(1_000, 500).await.unwrap()[..], &payload[1_000..1_500]);
}

/// **A wait is never taken here, whatever a loop grants.** The chunk
/// buffer is obtained on the runtime's own task rather than inside a
/// `spawn_blocking` closure, so a `Condvar` wait would block the thread a
/// releasing sibling needs. The local source takes the same grant, which
/// is what makes this a difference between two transports rather than a
/// policy nobody implements.
#[tokio::test]
async fn a_fetched_source_never_takes_the_wait_a_loop_grants() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let (fetched, _) = fetched_xz(compressed.path()).await;
    fetched.hint_wait_policy(WaitPolicy::MayWait);
    assert_eq!(fetched.budget.pool.policy(), WaitPolicy::NeverWait);

    let local = CompressedFileSource::open(compressed.path()).unwrap();
    local.hint_wait_policy(WaitPolicy::MayWait);
    assert_eq!(local.budget.pool.policy(), WaitPolicy::MayWait);
}

/// The compressed window is charged rather than left unpooled: the
/// per-reader charge a caller solves a budget against carries the widest
/// window this file's blocks would make us fetch. It is not the local
/// charge *plus* that window — the local one carries a compressed input
/// chunk this source never builds — so what is asserted is that the window
/// cannot have gone unbilled (D15).
#[tokio::test]
async fn the_compressed_window_is_inside_the_charge() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let (source, _) = fetched_xz(compressed.path()).await;
    let table = seek_table_of(&source);
    let widest = table.0.blocks.iter().map(|b| b.total_size()).max().unwrap();
    assert!(widest > 0);

    let local = CompressedFileSource::open(compressed.path()).unwrap();
    let fetched_charge = source.block_decode_bytes().unwrap();
    let local_charge = local.block_decode_bytes().unwrap();
    // The local source's decoder charge carries an input chunk this one
    // never builds, so the two differ by the window less that chunk rather
    // than by the window alone.
    assert!(
        fetched_charge > local_charge.saturating_sub(widest),
        "the window must be billed: fetched {fetched_charge}, local {local_charge}, \
             widest window {widest}",
    );
}
