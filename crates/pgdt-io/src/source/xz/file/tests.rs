use std::num::NonZeroUsize;
use std::sync::Arc;

use crate::parallelism::worker_count;
use crate::source::pool::POOL_DEPTH;
use crate::test_support::*;
use crate::{PartitionBoundaries, RetainedUnit};

use super::*;

/// The multi-block shape: `size()` is the exact uncompressed length from
/// the seek table, with no read yet.
#[tokio::test]
async fn xz_source_size_is_the_uncompressed_length() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let source = XzSource::open(compressed.path()).unwrap();
    assert_eq!(source.size().await.unwrap(), payload.len() as u64);
    assert!(source.size_is_exact());
}

/// `stored_size()` is a `stat` on the compressed file itself, not the
/// uncompressed length `size()` answers — the two must disagree for a
/// payload that actually compresses
/// (`docs/design/decisions.md`, "D21").
#[tokio::test]
async fn xz_source_stored_size_is_the_compressed_files_on_disk_length() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let on_disk = std::fs::metadata(compressed.path()).unwrap().len();
    let source = XzSource::open(compressed.path()).unwrap();
    assert_eq!(source.stored_size().await.unwrap(), on_disk);
    assert_ne!(source.stored_size().await.unwrap(), source.size().await.unwrap());
}

/// `seek_table()` is what the cache's save wraps into
/// `CompressionIndex::Xz` — a multi-block file must actually report more
/// than one block, or the "seekable" half of this test proves nothing.
#[tokio::test]
async fn xz_source_seek_table_reports_every_block() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let source = XzSource::open(compressed.path()).unwrap();
    let table = source.seek_table().expect("an XzSource always has a table");
    assert!(table.is_seekable(), "block_count = {}", table.block_count());
    assert_eq!(table.uncompressed_size(), payload.len() as u64);

    // What an embedder reads of it without naming `xz_seek`: blocks that
    // tile the plaintext in order, and the length of the file it describes.
    let ranges: Vec<_> = table.block_ranges().collect();
    assert_eq!(ranges.len(), table.block_count());
    assert_eq!(ranges.first().map(|r| r.start), Some(0));
    assert!(ranges.windows(2).all(|w| w[0].end == w[1].start), "{ranges:?}");
    assert_eq!(ranges.last().map(|r| r.end), Some(table.uncompressed_size()));
    let widest = ranges.iter().map(|r| r.end - r.start).max();
    assert_eq!(widest, Some(table.max_block_uncompressed()));
    assert_eq!(table.compressed_size(), std::fs::metadata(compressed.path()).unwrap().len());
}

/// **A compressed source recommends the cores it was given, capped at its
/// own block count** (`docs/design/decisions.md`, "D2"):
/// `crate::partition::cut` cuts at block boundaries, so a file with fewer
/// blocks than the machine has cores offers no seam for the rest.
///
/// The core count is asserted against `available_parallelism()` rather
/// than a literal, the property pinned being that this source asks `std`
/// rather than carrying a constant of its own — which is what makes a
/// container's CPU quota reach the default
/// (`docs/design/runtime-invariants.md`, `RT7`).
#[test]
fn an_xz_source_recommends_the_cores_it_was_given_capped_at_its_block_count() {
    let cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);

    // More blocks than any machine has cores, so `std`'s answer is what
    // binds and the cap is inert.
    let wide: Vec<u8> = (0..4096u32 * 512).map(|i| (i % 251) as u8).collect();
    let compressed = xz_compress(&wide, &["--block-size=4096"]);
    let source = XzSource::open(compressed.path()).unwrap();
    assert!(source.table.block_count() > cores, "{} blocks", source.table.block_count());
    assert_eq!(source.default_workers(), cores);

    // A handful of blocks: the *file* binds on any machine wider than it,
    // which is the half that was missing.
    let narrow = xz_compress(&xz_test_payload(), &["--block-size=4096"]);
    let source = XzSource::open(narrow.path()).unwrap();
    let blocks = source.table.block_count();
    assert!((2..64).contains(&blocks), "{blocks} blocks");
    assert_eq!(source.default_workers(), cores.min(blocks));
    if cores > blocks {
        assert_eq!(source.default_workers(), blocks, "the file is what binds here");
    }
}

/// **The seek table is held once, at a stated cost per entry** — the check
/// for what is left of `KD26` (`docs/design/decisions.md`, "D4").
///
/// Two properties, neither checked anywhere else. The **share**:
/// `XzSource::over` takes the layout's own `Arc` through
/// `xz_seek::Layout::index_shared`, so the source's table and the layout's
/// are one allocation, and both constructors go through `over`. The
/// **per-entry cost** is a property of a vendored struct, so a re-sync
/// that grew either entry fails here rather than silently moving a
/// published number.
#[test]
fn the_seek_table_is_held_once_at_a_stated_cost_per_entry() {
    assert_eq!(std::mem::size_of::<xz_seek::StreamEntry>(), 80);
    assert_eq!(std::mem::size_of::<xz_seek::BlockEntry>(), 32);

    let compressed = xz_compress(&xz_test_payload(), &["--block-size=4096"]);

    // The walking constructor: the walk built the table and the layout
    // hands its own handle out.
    let source = XzSource::open(compressed.path()).unwrap();
    let persisted = source.seek_table().unwrap();
    assert!(
        std::ptr::eq(&*source.table, source.layout.index()),
        "the source's table must be the layout's own, not a clone of it",
    );

    // The cached one: a table handed in is the layout's table too, so the
    // caller's copy is dropped rather than becoming a third.
    let cached = XzSource::with_table(compressed.path(), persisted).unwrap();
    assert!(
        std::ptr::eq(&*cached.table, cached.layout.index()),
        "a cached table must be shared with the layout it was handed to",
    );
}

/// Reading forward in chunks that each land inside one block, straddle a
/// block boundary, or span several — the shape every real read loop in
/// this crate produces — must reproduce the plaintext exactly.
#[tokio::test]
async fn xz_source_reads_forward_matching_plain_bytes() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let source = XzSource::open(compressed.path()).unwrap();

    let mut offset = 0u64;
    for len in [100usize, 4096, 3000, 5000] {
        let got = source.read_range(offset, len).await.unwrap();
        assert_eq!(&got[..], &payload[offset as usize..offset as usize + len]);
        offset += len as u64;
    }
}

/// A read at an offset behind an earlier one must serve the covering
/// block rather than return the wrong bytes — the one path
/// `LocalFileSource` never has. The default budget affords these blocks,
/// so this is the block path; the piecewise arm's backward read is
/// `a_budget_below_the_block_unit_reads_in_pieces_and_still_reads_the_same_bytes`'s.
#[tokio::test]
async fn xz_source_seeks_backward_across_a_block_boundary() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let source = XzSource::open(compressed.path()).unwrap();

    // Walk forward past several block boundaries first, exactly like the
    // multi-chunk test above.
    let far = source.read_range(15_000, 2000).await.unwrap();
    assert_eq!(&far[..], &payload[15_000..17_000]);

    // Then jump backward into an earlier block.
    let near = source.read_range(500, 1000).await.unwrap();
    assert_eq!(&near[..], &payload[500..1500]);

    // And forward again, past where the first read left off, into a
    // block neither earlier read touched.
    let later = source.read_range(18_000, 2000).await.unwrap();
    assert_eq!(&later[..], &payload[18_000..20_000]);
}

/// The block a read landed in is retained, so the next read inside it
/// costs a slice rather than a second decode.
#[tokio::test]
async fn a_read_retains_the_block_it_landed_in() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let source = XzSource::open(compressed.path()).unwrap();
    let cache = source.budget.blocks.clone().expect("4 KiB blocks are decoded whole");
    assert!(cache.retention.lock().unwrap().blocks.is_empty());

    let first = source.read_range(100, 200).await.unwrap();
    assert_eq!(&first[..], &payload[100..300]);
    let retained = cache.retention.lock().unwrap().blocks.clone();
    assert_eq!(retained.len(), 1, "one read, one block");
    let (index, block) = retained.into_iter().next().unwrap();
    assert_eq!(index, 0);

    // A second read inside the same block is served from that same
    // decoded block: the view and the retention list are the only two
    // holders after the first read's `Bytes` are dropped.
    drop(first);
    assert_eq!(Arc::strong_count(&block), 2, "the retention list and this handle");
    let second = source.read_range(1000, 500).await.unwrap();
    assert_eq!(&second[..], &payload[1000..1500]);
    assert_eq!(cache.retention.lock().unwrap().blocks.len(), 1);
    assert_eq!(Arc::strong_count(&block), 3, "plus the view the second read handed back");
}

/// The retention list holds no more than the pool's slot count, so a walk
/// across many blocks does not accumulate decoded plaintext.
#[tokio::test]
async fn retention_is_bounded_by_the_pools_slot_count() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let source = XzSource::open(compressed.path()).unwrap();
    let cache = source.budget.blocks.clone().unwrap();
    let slots = cache.pool.slots();

    let mut offset = 0u64;
    while offset < payload.len() as u64 {
        let len = 100.min(payload.len() as u64 - offset) as usize;
        source.read_range(offset, len).await.unwrap();
        assert!(cache.retention.lock().unwrap().blocks.len() <= slots);
        offset += 4096;
    }
    assert!(cache.retention.lock().unwrap().blocks.len() <= slots);
}

/// **The block arm takes none of the piecewise arm's locks**: with the live
/// piecewise handle's mutex held by another thread outright, a block-path
/// read — a cache miss as well as a hit — still answers. Naming a
/// `BlockTask` is arithmetic over the layout's table, so what this arm locks
/// is its own retention list and never the live handle.
#[tokio::test]
async fn a_block_path_read_is_served_while_the_live_handle_is_held() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let source = XzSource::open(compressed.path()).unwrap();
    assert!(source.budget.block_path().is_some(), "4 KiB blocks fit the default budget");

    // Warm the block one of the reads below lands in: 9_000..10_000 is
    // inside the 8_192..12_288 block.
    source.read_range(9_000, 100).await.unwrap();

    // Held on a thread of its own rather than across the await, so the
    // guard's lifetime is not entangled with the future being tested.
    let (locked, is_locked) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let live = Arc::clone(&source.live);
    let holder = std::thread::spawn(move || {
        let guard = live.lock().unwrap_or_else(|e| e.into_inner());
        locked.send(()).unwrap();
        released.recv().unwrap();
        drop(guard);
    });
    is_locked.recv().unwrap();

    let hit = source.read_range(9_000, 1_000).await.unwrap();
    assert_eq!(&hit[..], &payload[9_000..10_000], "a retained block");
    let miss = source.read_range(100, 200).await.unwrap();
    assert_eq!(&miss[..], &payload[100..300], "a block decoded for the first time");

    release.send(()).unwrap();
    holder.join().unwrap();
}

/// Concurrent reads over different blocks all answer their own bytes, on
/// **both** arms: the block path runs them at once, and the piecewise arm
/// serializes them behind one live handle whose position each read has to
/// leave usable for the next.
///
/// The piecewise leg is what a single live decode makes easy to get wrong —
/// a read served out of another read's position answers the wrong bytes
/// rather than failing — so it is asserted at the same offsets, in the same
/// order, under a budget that holds no block.
#[tokio::test]
async fn concurrent_reads_each_answer_their_own_bytes() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    for budget in [None, Some(1)] {
        let source = Arc::new(XzSource::open(compressed.path()).unwrap());
        source.hint_parallelism(Parallelism::Serial { memory_bytes: budget });
        assert_eq!(source.budget.block_path().is_none(), budget.is_some(), "{budget:?}");

        let mut tasks = Vec::new();
        for offset in (0..16_000u64).step_by(997) {
            let source = Arc::clone(&source);
            tasks.push(tokio::spawn(async move {
                (offset, source.read_range(offset, 900).await.unwrap())
            }));
        }
        for task in tasks {
            let (offset, got) = task.await.unwrap();
            let at = offset as usize;
            assert_eq!(&got[..], &payload[at..at + 900], "at {offset}, budget {budget:?}");
        }
    }
}

/// The stated budget bounds the **source**, not each of its pools: the
/// chunk pool takes its own ceiling and the block pool takes what is left,
/// and the two hints may arrive in either order because the split is
/// re-derived from the stated number rather than composed incrementally.
#[tokio::test]
async fn a_stated_budget_is_divided_between_the_two_pools() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let held = |source: &XzSource| {
        let blocks = source.budget.blocks.as_ref().unwrap();
        (source.budget.pool.held_bytes(), blocks.pool.held_bytes(), blocks.pool.slots())
    };

    // Budget then chunk size, and chunk size then budget: same split.
    let one = XzSource::open(compressed.path()).unwrap();
    one.hint_parallelism(Parallelism::workers(8, 32 << 20));
    one.hint_read_size(1 << 20);
    let two = XzSource::open(compressed.path()).unwrap();
    two.hint_read_size(1 << 20);
    two.hint_parallelism(Parallelism::workers(8, 32 << 20));
    assert_eq!(held(&one), held(&two));

    let (chunks, blocks, slots) = held(&one);
    assert_eq!(chunks, POOL_DEPTH * (1 << 20), "four slots of the announced chunk");
    assert_eq!(blocks, slots * 4096);
    assert!(chunks + blocks <= 32 << 20, "the two pools sum inside the stated budget");
    // Eight workers, so eight retained blocks where the budget affords
    // them — the depth is the caller's number and the floor is the pool's.
    assert_eq!(slots, 8);
    one.hint_parallelism(Parallelism::default());
    assert_eq!(held(&one).2, POOL_DEPTH, "stating nothing keeps the pool's own depth");
}

/// **A retained block is storage, not a waiting holder, so no read loop's
/// permission reaches the block pool** (`docs/design/decisions.md`,
/// "D5"). Every retained block is also one some reader is holding a view
/// into, so `BlockCache::slot`'s drain can free nothing the waiting reader
/// is not itself holding.
///
/// The third read below is the one that would block: it runs on a detached
/// thread against a bounded receive, so a pool that charges block slots
/// fails this test rather than hanging the suite.
#[test]
fn a_permitted_wait_never_reaches_the_block_pool() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let source = XzSource::open(compressed.path()).unwrap();
    // 12 KiB: four chunk slots of the announced 1 KiB, and the 8 KiB left
    // over is two 4 KiB block slots.
    source.hint_read_size(1 << 10);
    source.hint_parallelism(Parallelism::workers(2, 12 << 10));
    source.hint_wait_policy(WaitPolicy::MayWait);
    let cache = Arc::clone(source.budget.blocks.as_ref().expect("4 KiB blocks are decoded whole"));
    assert_eq!(cache.pool.slots(), 2, "two block slots inside the stated budget");

    let read = |offset: u64| {
        XzSource::read_by_blocks(
            offset,
            100,
            &source.table,
            &cache,
            &source.layout,
            &source.file,
            &source.budget.pool,
        )
    };
    // One live view per slot, and both blocks are retained as well.
    let held = vec![read(0).unwrap(), read(4096).unwrap()];
    assert_eq!(cache.retention.lock().unwrap().blocks.len(), 2);

    let (tx, rx) = std::sync::mpsc::channel();
    let (table, blocks, layout, file, chunks) = (
        Arc::clone(&source.table),
        Arc::clone(&cache),
        source.layout.clone(),
        Arc::clone(&source.file),
        Arc::clone(&source.budget.pool),
    );
    std::thread::spawn(move || {
        let got = XzSource::read_by_blocks(8192, 100, &table, &blocks, &layout, &file, &chunks);
        let _ = tx.send(got.map(|bytes| bytes.to_vec()));
    });
    let third = rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("a third block decoded while two are held")
        .unwrap();
    assert_eq!(&third[..], &payload[8192..8292]);
    drop(held);
}

/// Two read units, one permission, and it reaches one of them: the chunk
/// pool takes what the loop granted and the block pool stays at
/// `NeverWait`, its holder being the retention list rather than the loop.
#[tokio::test]
async fn a_compressed_source_states_the_policy_to_its_chunk_pool_only() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let source = XzSource::open(compressed.path()).unwrap();
    let policies = |source: &XzSource| {
        (source.budget.pool.policy(), source.budget.blocks.as_ref().unwrap().pool.policy())
    };
    assert_eq!(policies(&source), (WaitPolicy::NeverWait, WaitPolicy::NeverWait));
    source.hint_wait_policy(WaitPolicy::MayWait);
    assert_eq!(policies(&source), (WaitPolicy::MayWait, WaitPolicy::NeverWait));
    source.hint_wait_policy(WaitPolicy::NeverWait);
    assert_eq!(policies(&source), (WaitPolicy::NeverWait, WaitPolicy::NeverWait));

    // And the block path still reads correctly while a wait is permitted.
    source.hint_wait_policy(WaitPolicy::MayWait);
    source.hint_read_size(1024);
    let got = source.read_range(0, 4096).await.unwrap();
    assert_eq!(&got[..], &payload[..4096]);
}

/// A budget too small for a whole block sends every read through the
/// piecewise arm, and the bytes are the same either way — the budget
/// changes the path, never the answer.
#[tokio::test]
async fn a_budget_below_the_block_unit_reads_in_pieces_and_still_reads_the_same_bytes() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let source = XzSource::open(compressed.path()).unwrap();
    assert!(source.budget.block_path().is_some(), "4 KiB blocks fit the default budget");

    // Under a budget of one chunk buffer there is nothing left for a
    // block, so the source falls back — and says so through its own
    // partitioning advice as well.
    source.hint_read_size(1 << 20);
    source.hint_parallelism(Parallelism::workers(4, 1 << 20));
    assert!(source.budget.block_path().is_none());
    assert_eq!(source.partitions(0..payload.len() as u64).max_partitions(), Some(1));

    let got = source.read_range(4000, 5000).await.unwrap();
    assert_eq!(&got[..], &payload[4000..9000]);
    // Backward inside a block the live handle has already passed, which is
    // what makes it begin a second handle over the same block.
    let back = source.read_range(4100, 100).await.unwrap();
    assert_eq!(&back[..], &payload[4100..4200]);

    source.hint_parallelism(Parallelism::default());
    assert!(source.budget.block_path().is_some(), "the default budget takes it back");
    let again = source.read_range(4000, 5000).await.unwrap();
    assert_eq!(&again[..], &payload[4000..9000]);
}

/// **The live handle is what keeps a forward scan linear.** A scan that
/// reads a block in pieces continues the decode it already has, so the
/// handle stays on one block across the reads inside it and moves on once —
/// where a handle begun per read would skip from that block's start every
/// time.
#[tokio::test]
async fn a_forward_piecewise_scan_continues_the_decode_it_has() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let source = XzSource::open(compressed.path()).unwrap();
    source.hint_parallelism(Parallelism::Serial { memory_bytes: Some(1) });
    assert!(source.budget.block_path().is_none());

    // Four reads inside the first 4 KiB block: the handle advances with
    // them and never leaves the block.
    for step in 0..4u64 {
        let at = step * 1000;
        let got = source.read_range(at, 1000).await.unwrap();
        assert_eq!(&got[..], &payload[at as usize..at as usize + 1000]);
        let live = source.live.lock().unwrap();
        let live = live.as_ref().expect("the piecewise arm seats a handle");
        assert_eq!(live.index, 0, "still the first block");
        assert_eq!(live.read.position(), at + 1000, "the handle is where the read left it");
    }

    // A read in the next block leaves the first one — which is where its
    // check is compared — and seats a handle there.
    source.read_range(4096, 100).await.unwrap();
    let live = source.live.lock().unwrap();
    assert_eq!(live.as_ref().expect("a handle").index, 1);
}

/// The non-seekable shape: one stream, one block, produced by a bare `xz`
/// invocation with no `-T`/`--block-size`. Every read still decodes to the
/// right bytes, backward ones included.
#[tokio::test]
async fn xz_source_reads_a_single_block_non_seekable_file() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &[]);
    let source = XzSource::open(compressed.path()).unwrap();

    let table = source.seek_table().unwrap();
    assert_eq!(table.block_count(), 1);
    assert!(!table.is_seekable());

    let forward = source.read_range(10_000, 5000).await.unwrap();
    assert_eq!(&forward[..], &payload[10_000..15_000]);

    // Backward, into the sole block the forward read already retained.
    let backward = source.read_range(0, 3000).await.unwrap();
    assert_eq!(&backward[..], &payload[0..3000]);
}

/// **A source recommends what its workers hold**, so that "no limit found"
/// cannot mean "serial" (`docs/design/decisions.md`, "D3"). The plain
/// source inherits the silence it inherits for the worker count.
///
/// **A shape rather than a scalar**, because the block pool's retention
/// list is not per worker: `block_decode_bytes` is that same shape
/// evaluated at one reader — the gate's own number — and is strictly above
/// the per-worker term, while past [`POOL_DEPTH`] readers every further
/// reader adds a unit of list as well as its own term.
#[test]
fn a_compressed_source_recommends_what_its_workers_hold() {
    assert_eq!(BareSource.default_worker_memory(), None);
    let (_file, plain) = source_of(b"0123456789abcdef");
    assert_eq!(plain.default_worker_memory(), None);

    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let source = XzSource::open(compressed.path()).unwrap();
    let memory = source.default_worker_memory().expect("a block path to price");
    let unit = source.budget.blocks.as_ref().expect("a block cache").unit as u64;
    assert_eq!(source.block_decode_bytes(), Some(memory.at(1)));
    assert_eq!(
        memory.at(1),
        memory.bytes_per_worker() + (POOL_DEPTH as u64 - 1) * unit,
        "one reader leaves a retention list nobody else fills"
    );
    assert_eq!(
        memory.at(POOL_DEPTH + 2),
        memory.bytes_per_worker() * (POOL_DEPTH as u64 + 2) + (POOL_DEPTH as u64 + 1) * unit,
        "past the pool's own depth each reader brings a slot of list with it"
    );
}

/// **The charge an allowance is solved against before the file is open is
/// the charge the resulting count then meets.** `Parallelism::fit` solves
/// a cap against `default_worker_memory` with nothing announced;
/// `crate::parallelism::worker_count` solves the budget that produced against
/// `partitions().worker_memory()` once the scan has announced its chunk.
/// Charged at the pool ceiling instead of the default chunk, the first
/// would sit above the second and resolve fewer readers than the scan
/// admits.
///
/// Asserted end to end rather than by comparing two accessors: what has to
/// agree is the *resolution* and the *scan*.
#[test]
fn the_recommendation_an_allowance_is_solved_against_is_the_scans_own_charge() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let source = XzSource::open(compressed.path()).unwrap();
    let memory = source.default_worker_memory().expect("a block path to price");

    // Four readers' worth of allowance, as `Parallelism::fit` would spend
    // it, then the scan's own announcement and its charge.
    let (jobs, budget) = Parallelism::fit(24, Some(memory), memory.at(4), None);
    assert_eq!(jobs, 4);
    let parallelism = Parallelism::workers(jobs, budget);
    source.hint_read_size(crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES);
    source.hint_parallelism(parallelism);
    let advice = source.partitions(0..payload.len() as u64);
    assert_eq!(advice.worker_memory(), memory, "the recommendation and the charge are one shape");
    assert_eq!(
        worker_count(parallelism, advice.worker_memory()),
        jobs,
        "the count the allowance bought is the count the scan runs"
    );
}

/// A block-decoding `.xz` advises its block boundaries, so a partition is
/// a whole number of blocks and no cut falls inside one.
/// The boundaries are ascending and strictly inside the range: the block
/// containing `range.start` begins at or before it, and is not a split.
#[tokio::test]
async fn an_xz_source_advises_its_block_boundaries() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let source = XzSource::open(compressed.path()).unwrap();
    let table = source.seek_table().unwrap();
    let starts: Vec<u64> = table.block_ranges().map(|r| r.start).collect();
    assert!(starts.len() > 2, "this test needs several blocks, got {}", starts.len());

    let whole = source.partitions(0..payload.len() as u64);
    assert_eq!(whole.boundaries(), &PartitionBoundaries::At(starts[1..].to_vec()));
    assert_eq!(whole.max_partitions(), Some(starts.len()));

    // A range opening inside a block: that block's own start is behind
    // `range.start` and is not offered as a split.
    let from = starts[1] + 10;
    let inner = source.partitions(from..payload.len() as u64);
    assert_eq!(inner.boundaries(), &PartitionBoundaries::At(starts[2..].to_vec()));

    // A range inside one block has nothing to split at.
    let one = source.partitions(from..from + 10);
    assert_eq!(one.max_partitions(), Some(1));

    // A partition costs what one concurrent reader holds: one block slot
    // — the one being decoded, which is the one it then retains — the
    // chunk buffer a straddling read is assembled into, and the decoder's
    // own retention. The chunk term is the one a scan settles at, no read
    // loop having announced one here
    // ([`XzBudget::charged_chunk_bytes`]).
    let unit = source.budget.blocks.as_ref().unwrap().unit as u64;
    assert_eq!(
        whole.partition_bytes(),
        unit + crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES as u64 + source.budget.decode_bytes
    );
    assert_eq!(whole.worker_memory().bytes_per_worker(), whole.partition_bytes());

    // **And the gate is that same charge at one reader**: the per-reader
    // term plus the `(POOL_DEPTH - 1)` units of retention list nobody else
    // is there to take ([`WorkerMemory`]).
    assert_eq!(
        source.block_decode_bytes(),
        Some(whole.partition_bytes() + (POOL_DEPTH as u64 - 1) * unit)
    );
    assert_eq!(source.block_decode_bytes(), Some(whole.worker_memory().at(1)));

    // And what a retained batch pins is that same block, which the line
    // above has already charged for — so a caller must not add its span
    // allowance on top ([`RetainedUnit`]).
    assert_eq!(whole.retained_unit(), RetainedUnit::Partition);
}
