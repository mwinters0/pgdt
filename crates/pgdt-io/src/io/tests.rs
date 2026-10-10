use super::*;
use std::io::Write;

fn source_of(bytes: &[u8]) -> (tempfile::NamedTempFile, LocalFileSource) {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(bytes).unwrap();
    file.flush().unwrap();
    let source = LocalFileSource::open(file.path()).unwrap();
    (file, source)
}

/// `LocalFileSource` never overrides [`ByteRangeSource::stored_size`], so
/// its default — mirroring [`ByteRangeSource::size`] — is what a
/// non-decompressing source is meant to answer.
#[tokio::test]
async fn stored_size_defaults_to_size() {
    let (_file, source) = source_of(b"0123456789abcdef");
    assert_eq!(source.stored_size().await.unwrap(), source.size().await.unwrap());
}

/// Every source answers its addressable length exactly; nothing
/// overrides the default.
#[test]
fn size_is_exact_defaults_true() {
    let (_file, source) = source_of(b"0123456789abcdef");
    assert!(source.size_is_exact());
}

/// The pool's whole risk: a reused buffer still holding the previous
/// read's bytes past the length of the current one. A shorter range read
/// after a longer one must see the file's bytes and nothing else.
#[tokio::test]
async fn short_read_after_long_read_sees_only_its_own_bytes() {
    let (_file, source) = source_of(b"0123456789abcdef");
    let long = source.read_range(0, 16).await.unwrap();
    assert_eq!(&long[..], b"0123456789abcdef");
    drop(long);
    let short = source.read_range(0, 4).await.unwrap();
    assert_eq!(&short[..], b"0123");
    let elsewhere = source.read_range(10, 3).await.unwrap();
    assert_eq!(&elsewhere[..], b"abc");
}

/// A retained chunk must not be handed back out while it is still
/// referenced — the query path holds chunks past the read that produced
/// them, and a zero-copy view into a recycled buffer would silently
/// return another chunk's bytes.
#[tokio::test]
async fn a_retained_chunk_is_not_recycled_under_it() {
    let (_file, source) = source_of(b"0123456789abcdef");
    let held = source.read_range(0, 8).await.unwrap();
    let next = source.read_range(8, 8).await.unwrap();
    assert_eq!(&held[..], b"01234567");
    assert_eq!(&next[..], b"89abcdef");
}

/// A buffer above the ceiling that no caller announced as its read length
/// is dropped rather than pooled, so one oversized `attach_text` read
/// cannot hold megabytes for the life of the process.
#[test]
fn the_pool_refuses_a_buffer_above_the_ceiling() {
    let pool = BufferPool::default();
    pool.give(vec![0u8; POOL_MAX_BYTES + 1]);
    assert_eq!(pool.free_len(), 0);
    pool.give(vec![0u8; 64]);
    assert_eq!(pool.free_len(), 1);
}

/// The announced read length is what raises the ceiling: a scan configured
/// with a large chunk keeps its pooling, and a span read longer than that
/// chunk is still dropped.
#[test]
fn the_pool_keeps_a_buffer_of_the_announced_read_length() {
    let chunk = 16 << 20;
    let pool = Arc::new(BufferPool::default());
    pool.give(vec![0u8; chunk]);
    assert_eq!(pool.free_len(), 0);

    pool.hint(chunk);
    pool.give(vec![0u8; chunk]);
    assert_eq!(pool.free_len(), 1);
    // The same read asked for again gets that buffer back rather than a
    // fresh `calloc`, which is the whole point.
    let taken = pool.obtain(chunk);
    assert_eq!(taken.as_ref().len(), chunk);
    assert_eq!(pool.free_len(), 0);
    drop(taken);

    // A buffer that fits a slot is kept and occupies one; one larger than
    // a slot is dropped, which is what makes `held_bytes` a bound.
    pool.give(vec![0u8; chunk - 1]);
    assert_eq!(pool.free_len(), 2);
    pool.give(vec![0u8; chunk + 1]);
    assert_eq!(pool.free_len(), 2);
}

/// **The free list holds no more bytes than the pool reports.** Every
/// buffer it keeps fits a slot and every slot is counted, so
/// `held_bytes()` is an upper bound — the number that divides one stated
/// budget between a source's two pools (`XzBudget::apportion`).
///
/// The buffer it refuses is a whole plain partition against a pool whose
/// slot is a chunk: a second read unit, which the plain path does not
/// allocate, reading its piece [`PartitionRead::Chunked`] instead.
#[test]
fn the_free_list_holds_no_more_than_it_reports() {
    let chunk = 1 << 20;
    let pool = Arc::new(BufferPool::default());
    pool.hint(chunk);
    for _ in 0..8 {
        pool.give(vec![0u8; chunk * PLAIN_PARTITION_CHUNKS]);
    }
    assert_eq!(pool.free_len(), 0, "a partition read is larger than this pool's slot");
    for _ in 0..8 {
        pool.give(vec![0u8; chunk]);
    }
    assert_eq!(pool.free_len(), POOL_DEPTH);
    assert_eq!(pool.free_len() * chunk, pool.held_bytes());
}

/// **A reservation comes out of the free list's share, not beside it.**
/// The one holder that takes slots without leaving them on the free list
/// is `BlockCache`'s retained list (`docs/design/decisions.md`, "D17").
#[test]
fn a_reservation_takes_the_free_list_share() {
    let chunk = 1 << 20;
    let pool = Arc::new(BufferPool::default());
    pool.hint(chunk);
    assert_eq!(pool.slots(), POOL_DEPTH);

    pool.reserve(POOL_DEPTH - 1);
    for _ in 0..POOL_DEPTH {
        pool.give(vec![0u8; chunk]);
    }
    assert_eq!(pool.free_len(), 1, "free plus reserved is one slot count, not two");

    pool.reserve(0);
    pool.give(vec![0u8; chunk]);
    assert_eq!(pool.free_len(), 2, "a released reservation is the free list's again");
}

/// The hint reaches the pool through the trait method, which is the only
/// way a read loop has of announcing anything.
#[test]
fn a_source_passes_the_hint_to_its_pool() {
    let (_file, source) = source_of(b"0123456789abcdef");
    let chunk = POOL_MAX_BYTES + 1;
    source.hint_read_size(chunk);
    source.pool.give(vec![0u8; chunk]);
    assert_eq!(source.pool.free_len(), 1);
}

/// One worker **is** the serial path, so it is spelled that way rather
/// than as a `Workers` of one nobody can tell from it; zero is a caller's
/// own arithmetic and reads as one, exactly as `xz_seek::Bulk::new` reads
/// it.
///
/// **The collapse takes the worker count down and not the budget**: a
/// caller asking for one worker inside a stated budget has still stated
/// it, and every pool sized from `memory_bytes` sees it
/// (`docs/design/decisions.md`, "D1").
#[test]
fn one_worker_and_none_are_both_the_serial_state() {
    let serial_in_budget = Parallelism::Serial { memory_bytes: Some(512 << 20) };
    assert_eq!(Parallelism::workers(0, 512 << 20), serial_in_budget);
    assert_eq!(Parallelism::workers(1, 512 << 20), serial_in_budget);
    assert!(serial_in_budget.is_serial());
    assert_eq!(serial_in_budget.jobs(), 1);
    assert_eq!(serial_in_budget.memory_bytes(), Some(512 << 20));
    let eight = Parallelism::workers(8, 512 << 20);
    assert!(!eight.is_serial());
    assert_eq!(
        eight,
        Parallelism::Workers { jobs: NonZeroUsize::new(8).unwrap(), memory_bytes: 512 << 20 }
    );
}

/// **A one-worker budget reaches the pool it is a budget for**, driven
/// through the announcement a serial read loop makes rather than the value
/// alone: the stated budget sizes the pool, where the same source told
/// nothing keeps `DEFAULT_MEMORY_BUDGET`'s depth.
#[test]
fn a_serial_budget_sizes_the_source_it_is_announced_to() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.sql");
    std::fs::write(&path, b"x").unwrap();
    let source = LocalFileSource::open(&path).unwrap();
    source.hint_read_size(4 << 10);

    source.hint_parallelism(Parallelism::workers(1, 12 << 10));
    assert_eq!(source.pool.slots(), 3, "the serial path's stated budget sizes the pool");

    source.hint_parallelism(Parallelism::default());
    assert_eq!(source.pool.slots(), POOL_DEPTH, "stating nothing keeps the default depth");
}

/// The pool holds `POOL_DEPTH` buffers and no more, and a take picks the
/// smallest that fits rather than the first.
#[test]
fn the_pool_is_bounded_and_takes_the_smallest_that_fits() {
    let pool = Arc::new(BufferPool::default());
    for len in [8usize, 64, 32, 16, 128] {
        pool.give(vec![0u8; len]);
    }
    assert_eq!(pool.free_len(), POOL_DEPTH);
    let sixteen = pool.obtain(16);
    assert_eq!(sixteen.as_ref().len(), 16);
    let thirty_two = pool.obtain(16);
    assert_eq!(thirty_two.as_ref().len(), 32);
    // Nothing left that fits: a fresh allocation, exactly as long as asked.
    assert_eq!(pool.obtain(1024).as_ref().len(), 1024);
}

/// The slot count is a consequence of the announced unit, which is what
/// makes a decoded xz block a slot the pool can state a bound for:
/// [`POOL_DEPTH`] slots of a large block would be the whole of a small
/// cgroup, so the count falls rather than the budget rising.
#[test]
fn the_slot_count_falls_out_of_the_announced_unit() {
    let pool = BufferPool::default();
    // Nothing announced: the ceiling stands in for the unit.
    assert_eq!(pool.slot_bytes(), POOL_MAX_BYTES);
    assert_eq!(pool.slots(), POOL_DEPTH);

    // A block larger than the whole budget gets one slot rather than
    // none: a pool that refused to keep a block at all would hand back the
    // fresh `calloc` it exists to remove.
    for (unit, slots) in
        [(1 << 20, POOL_DEPTH), (16 << 20, POOL_DEPTH), (24 << 20, 2), (128 << 20, 1)]
    {
        pool.hint(unit);
        assert_eq!(pool.slot_bytes(), unit);
        assert_eq!(pool.slots(), slots, "unit {unit}");
    }
}

/// The budget and the depth are both the caller's to state, and each binds
/// on its own: whichever is smaller decides the count, and the floor of
/// one survives both.
#[test]
fn the_stated_budget_and_depth_each_bound_the_slot_count() {
    let pool = BufferPool::default();
    pool.hint(24 << 20);
    assert_eq!(pool.slots(), 2, "the default budget affords two 24 MiB slots");

    // A larger budget with the same depth: the depth binds.
    pool.set_limits(512 << 20, POOL_DEPTH);
    assert_eq!(pool.slots(), POOL_DEPTH);
    // A larger budget and a larger depth: the budget binds.
    pool.set_limits(512 << 20, 32);
    assert_eq!(pool.slots(), 21);
    // A budget below one slot still keeps one, since a pool that holds
    // nothing hands every reader back the `calloc` it exists to remove.
    pool.set_limits(1 << 20, 8);
    assert_eq!(pool.slots(), 1);
    assert_eq!(pool.held_bytes(), 24 << 20);
}

/// A plain file has one read unit, so the stated budget sizes its free
/// list and the worker count does not — a chunk buffer is held only until
/// one read's bytes are dropped, where a retained block is kept.
#[test]
fn a_plain_source_takes_the_budget_and_not_the_worker_count() {
    let (_file, source) = source_of(b"0123456789abcdef");
    source.hint_read_size(1 << 20);
    assert_eq!(source.pool.slots(), POOL_DEPTH);

    source.hint_parallelism(Parallelism::workers(16, 2 << 20));
    assert_eq!(source.pool.slots(), 2, "the stated budget binds");
    source.hint_parallelism(Parallelism::default());
    assert_eq!(source.pool.slots(), POOL_DEPTH, "the default states no number");
}

/// **Backpressure**: a loop that permits a wait gets one instead of
/// allocating past the stated budget, which is what turns the slot count
/// into a bound on what is *outstanding* rather than on what is idle.
///
/// Two threads, because a wait has no behaviour except its interaction
/// with another holder. The one loop that grants this permission is the
/// leader's fused worker, exercised against a real source by
/// `pgdt_dump::leader`'s scheduler tests.
#[test]
fn a_permitted_wait_takes_a_slot_rather_than_allocating() {
    let unit = 1 << 20;
    let pool = Arc::new(BufferPool::default());
    pool.hint(unit);
    pool.set_limits(unit, POOL_DEPTH);
    pool.set_policy(WaitPolicy::MayWait);
    assert_eq!(pool.slots(), 1, "one slot, so the second holder must wait for the first");

    let held = pool.obtain(unit);
    assert_eq!(pool.charged(), 1);

    let (tx, rx) = std::sync::mpsc::channel();
    let waiting = Arc::clone(&pool);
    let second = std::thread::spawn(move || {
        let slot = waiting.obtain(unit);
        tx.send(()).unwrap();
        drop(slot);
    });
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(150)).is_err(),
        "the only slot is out, so the second acquisition must not complete"
    );

    drop(held);
    rx.recv_timeout(std::time::Duration::from_secs(10))
        .expect("the released slot lets the waiting holder through");
    second.join().unwrap();
    assert_eq!(pool.charged(), 0, "every charge is discharged when its buffer returns");
}

/// The exemption, and it is stated by permission rather than by any
/// setting: a loop that grants no wait allocates past the budget instead.
/// Three buffers at once against a one-slot pool on **one** thread — a
/// wait here would be the deadlock the exemption exists to prevent, so
/// this test hangs rather than fails if the policy is ignored.
#[test]
fn an_unpermitted_wait_allocates_past_the_budget_instead() {
    let unit = 1 << 20;
    let pool = Arc::new(BufferPool::default());
    pool.hint(unit);
    pool.set_limits(unit, POOL_DEPTH);
    assert_eq!(pool.slots(), 1);

    // The default, and what every shipped read loop but the leader's
    // fused worker states.
    assert_eq!(pool.policy(), WaitPolicy::NeverWait);
    let held: Vec<_> = (0..3).map(|_| pool.obtain(unit)).collect();
    assert_eq!(held.len(), 3);
    assert_eq!(pool.charged(), 0, "an exempt read charges no slot");

    drop(held);
    assert_eq!(pool.free_len(), 1, "the slot count still bounds what is kept");
}

/// The charge belongs to the buffer, not to the pool's current policy, so
/// a source may be handed different permissions by successive loops while
/// an earlier one's buffers are still out. And it is discharged whether or
/// not the buffer is kept.
#[test]
fn a_charge_outlives_the_policy_that_took_it_and_survives_a_refused_release() {
    let pool = Arc::new(BufferPool::default());
    pool.set_policy(WaitPolicy::MayWait);
    let held = pool.obtain(POOL_MAX_BYTES + 1);
    assert_eq!(pool.charged(), 1);

    pool.set_policy(WaitPolicy::NeverWait);
    drop(held);
    assert_eq!(pool.charged(), 0);
    assert_eq!(pool.free_len(), 0, "a buffer above the ceiling is still dropped");
}

/// The policy reaches the pool through the trait method, which is the only
/// way a read loop has of granting one — and the default is to permit no
/// wait, so a source nobody announces to allocates rather than blocking.
#[test]
fn a_source_states_the_wait_policy_to_its_pool() {
    let (_file, source) = source_of(b"0123456789abcdef");
    assert_eq!(source.pool.policy(), WaitPolicy::NeverWait);
    source.hint_wait_policy(WaitPolicy::MayWait);
    assert_eq!(source.pool.policy(), WaitPolicy::MayWait);
    source.hint_wait_policy(WaitPolicy::NeverWait);
    assert_eq!(source.pool.policy(), WaitPolicy::NeverWait);
}

/// A source implementing nothing but the three required methods, to pin
/// what the *defaults* answer apart from any shipped source, `XzSource`
/// overriding them and `LocalFileSource` free to.
struct BareSource;

impl ByteRangeSource for BareSource {
    fn read_range(
        &self,
        _offset: u64,
        _len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        Box::pin(async { Ok(Bytes::new()) })
    }
    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        Box::pin(async { Ok(0) })
    }
    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        Box::pin(async { Ok(None) })
    }
}

/// A source that has not thought about being read concurrently advises
/// one partition, so a scheduler cannot split it on the assumption that
/// silence means consent.
#[test]
fn a_source_that_does_not_advise_gets_one_partition() {
    let advice = BareSource.partitions(0..1 << 30);
    assert_eq!(advice.max_partitions(), Some(1));
    assert_eq!(advice.boundaries(), &PartitionBoundaries::At(Vec::new()));
    assert_eq!(advice.partition_bytes(), 0);
}

/// **Silence recommends the serial path**, the same convention
/// `partitions` above answers with — and [`LocalFileSource`] inherits it
/// rather than overriding (`docs/design/decisions.md`, "D2").
#[test]
fn a_source_that_does_not_advise_recommends_the_serial_path() {
    assert_eq!(BareSource.default_workers(), 1);
    let (_file, source) = source_of(b"0123456789abcdef");
    assert_eq!(source.default_workers(), 1);
}

/// A plain file prefers no split point to another, and a partition costs
/// [`PLAIN_PARTITION_CHUNKS`] read chunks — the caller's own announced
/// number where it announced one, so the size scales with the unit the
/// pool sizes a slot by, and never past the ceiling above which a released
/// buffer stops being kept.
///
/// **What a partition is a multiple of, and where the multiple stops, are
/// two rules and both are asserted**: a hint under the ceiling multiplies,
/// one that would carry the product past it is capped, and one already at
/// or past it is a partition on its own.
#[test]
fn a_plain_file_advises_anywhere_at_several_chunks_each() {
    let (_file, source) = source_of(b"0123456789abcdef");
    let advice = source.partitions(0..16);
    assert_eq!(advice.boundaries(), &PartitionBoundaries::Anywhere);
    assert_eq!(advice.max_partitions(), None);
    assert_eq!(
        advice.retained_unit(),
        RetainedUnit::ReadChunk,
        "a batch pins the chunks it was read into, which partition_bytes has not charged"
    );
    // Nothing announced: the slot size is the ceiling, and the multiple is
    // capped back to it.
    assert_eq!(advice.partition_bytes(), POOL_MAX_BYTES as u64);

    source.hint_read_size(64 << 10);
    assert_eq!(
        source.partitions(0..16).partition_bytes(),
        (PLAIN_PARTITION_CHUNKS as u64) * (64 << 10)
    );

    source.hint_read_size(4 << 20);
    assert_eq!(source.partitions(0..16).partition_bytes(), POOL_MAX_BYTES as u64);

    source.hint_read_size(16 << 20);
    assert_eq!(source.partitions(0..16).partition_bytes(), 16 << 20);
}

/// **The shipped configuration must be whole chunks, not capped ones.**
/// `SCAN_CHUNK_DEFAULT_SIZE_BYTES * PLAIN_PARTITION_CHUNKS` is exactly
/// [`POOL_MAX_BYTES`], on the cap, and the three constants are justified
/// independently, so this assertion is what stops a later change to any
/// one of them from silently converting the default from whole chunks to
/// capped.
#[test]
fn a_shipped_plain_partition_is_eight_whole_chunks() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let source = LocalFileSource::open(file.path()).unwrap();
    source.hint_read_size(crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES);
    assert_eq!(
        source.partitions(0..16).partition_bytes(),
        (PLAIN_PARTITION_CHUNKS as u64) * (crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES as u64),
        "the shipped chunk size must yield a whole multiple, uncapped"
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
    let starts: Vec<u64> = table.blocks.iter().map(|b| b.uncompressed_offset).collect();
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

/// **A streaming-fallback source advises one partition even though its
/// table has boundaries** (`docs/design/decisions.md`, "D15"). Asserted
/// against a synthetic multi-block table, `xz_partition_advice` being a free
/// function precisely so that the fallback arm can be handed a table that
/// *does* have boundaries.
#[test]
fn a_streaming_fallback_source_advises_one_partition() {
    let table = four_block_table();
    assert!(table.is_seekable(), "the table has boundaries to advise");

    let cache = BlockCache::for_table(&table).expect("4 KiB blocks decode whole");
    let decoding = xz_partition_advice(&table, Some(&cache), 1 << 20, 9 << 20, 0..4 * 4096);
    assert_eq!(decoding.max_partitions(), Some(4));
    assert_eq!(decoding.retained_unit(), RetainedUnit::Partition);
    assert_eq!(
        decoding.partition_bytes(),
        4096 + (1 << 20) + (9 << 20),
        "one block slot, the chunk buffer, and the decoder's own retention"
    );

    let streaming = xz_partition_advice(&table, None, 1 << 20, 9 << 20, 0..4 * 4096);
    assert_eq!(streaming.max_partitions(), Some(1));
    assert_eq!(
        streaming.partition_bytes(),
        1 << 20,
        "one chunk buffer a reader; the fallback's single decoder is a cost of the source"
    );
    assert_eq!(
        streaming.retained_unit(),
        RetainedUnit::ReadChunk,
        "a piecewise read is assembled into a chunk buffer, so chunks are what a batch pins"
    );
    // **The read shape is stated apart from both of those**: a
    // block-decoding piece is read whole and copies nothing, while a
    // piecewise read is assembled into a chunk buffer whatever length is
    // asked for. The unit the block arm states is the table's own largest
    // block rather than the piece.
    assert_eq!(decoding.partition_read(), PartitionRead::Whole { unit: 4096 });
    assert_eq!(
        table.max_block_uncompressed(),
        4096,
        "the stated unit is the table's largest block, not the piece"
    );
    assert_eq!(streaming.partition_read(), PartitionRead::Chunked);
}

/// A four-block seek table over 4 KiB blocks, built by hand rather than by
/// `xz`: the block *cache* only ever reads a table's block lengths, so a
/// synthetic one lets a slot be small enough for a budget to be stated in
/// whole slots.
fn four_block_table() -> xz_seek::SeekTable {
    block_table(4)
}

/// The same table at `count` blocks, for the one test that needs more
/// boundaries than workers: a bound on how many units a piece spans is
/// vacuous on a file whose windows run out of boundaries first.
fn block_table(count: u64) -> xz_seek::SeekTable {
    let block = |i: u64| xz_seek::BlockEntry {
        compressed_offset: 12 + i * 128,
        uncompressed_offset: i * 4096,
        unpadded_size: 64,
        uncompressed_size: 4096,
    };
    xz_seek::SeekTable {
        compressed_file_size: 1 << 20,
        streams: vec![xz_seek::StreamEntry {
            compressed_offset: 0,
            uncompressed_offset: 0,
            compressed_size: 1 << 20,
            uncompressed_size: count * 4096,
            check: xz_seek::Check::Crc64,
            first_block_dict_size: Some(8 << 20),
            padding: 0,
            first_block: 0,
            block_count: count as usize,
        }],
        blocks: (0..count).map(block).collect(),
    }
}

/// **Every partition the leader hands a worker covers at most
/// [`BOUNDARIED_PARTITION_UNITS`] of this source's units**, whatever the
/// charge says and whatever the worker count is — which is what makes the
/// cut width a number somebody chose rather than a consequence of the
/// memory charge.
///
/// The failure mode it guards is `partition_bytes` serving as both the
/// memory charge and `run_region`'s cut size
/// (`docs/design/decisions.md`, "D8").
///
/// It walks the window loop rather than one window, a *mid-block* start
/// being the case a single aligned window would never show.
#[test]
fn a_block_decoding_partition_spans_at_most_the_cut_width() {
    // Enough blocks that a window of `workers × BOUNDARIED_PARTITION_UNITS`
    // is satisfied out of the boundary list rather than running off the end
    // of the file, which is what makes the bound below bind.
    let blocks = 8 * BOUNDARIED_PARTITION_UNITS as u64 + 4;
    let table = block_table(blocks);
    let size = blocks * 4096;
    let cache = BlockCache::for_table(&table).expect("4 KiB blocks decode whole");
    let advice = xz_partition_advice(&table, Some(&cache), 1 << 20, 9 << 20, 0..size);
    // The charge is a block, a chunk and the decoder — orders of
    // magnitude above the 4 KiB unit here, so a window sized by it would
    // swallow the whole file.
    assert!(advice.partition_bytes() > size, "the charge is not the cut size");

    for workers in [2usize, 3, 4, 8] {
        for start in [0u64, 1, 4095, 4096, 5000, 8192, 12287] {
            assert!(start < size, "a frontier inside the file");
            let end = advice.window_end(start, workers, size);
            assert!(end > start, "a window must advance: {workers} workers from {start}");
            let ranges = cut(start..end, &advice, workers);
            assert!(
                ranges.len() <= workers,
                "{workers} workers were handed {} pieces from {start}",
                ranges.len()
            );
            for piece in ranges {
                let covering = table.blocks_in(piece.clone());
                assert!(
                    covering.len() <= BOUNDARIED_PARTITION_UNITS,
                    "{piece:?} spans {} blocks at {workers} workers from {start}, \
                         against a cut width of {BOUNDARIED_PARTITION_UNITS}",
                    covering.len()
                );
            }
        }
    }
}

/// **A plain file's window is unchanged by the split above**: with no
/// boundaries to respect the charge *is* the cut size, so a window is
/// `workers` charges wide exactly as it always was.
#[test]
fn a_plain_window_is_still_the_charge_times_the_worker_count() {
    let (_file, source) = source_of(&[0u8; 64]);
    source.hint_read_size(8);
    let advice = source.partitions(0..64);
    assert_eq!(
        advice.partition_read(),
        PartitionRead::Chunked,
        "a plain partition is read a chunk at a time, so every buffer is a pooled one"
    );
    assert_eq!(
        BareSource.partitions(0..1 << 30).partition_read(),
        PartitionRead::Chunked,
        "and a source that has said nothing is read the conservative way"
    );
    let charge = advice.partition_bytes();
    assert_eq!(advice.window_end(0, 3, u64::MAX), 3 * charge);
    assert_eq!(advice.window_end(charge, 3, u64::MAX), 4 * charge);
    assert_eq!(advice.window_end(0, 3, 17), 17, "the limit binds");
}

/// **Whole-block decode is declined unless the budget affords the one
/// block a reader holds *and* the retention list beside it**
/// (`docs/design/decisions.md`, "D15"). A reader's decode buffer and the
/// block it then retains are the same buffer, and a single reader leaves
/// `(POOL_DEPTH - 1)` units of list nothing else is there to fill, so
/// [`POOL_DEPTH`] units is the line.
#[test]
fn the_block_path_wants_room_for_one_block_and_the_retention_list() {
    let table = four_block_table();
    let cache = BlockCache::for_table(&table).expect("4 KiB blocks have a unit");
    // The two terms a decline does not save, held at zero so that this
    // test is about the block slots alone; its last assertion and
    // `a_block_too_large_to_hold_refuses_the_block_path` are where they
    // bind.
    let unit = cache.unit as u64;
    let retained = POOL_DEPTH as u64 - 1;

    assert!(
        cache.affordable(0, 0, unit * (1 + retained)),
        "one unit for the reader, and the list the pool retains beside it"
    );
    cache.pool.set_limits(cache.unit * 2, POOL_DEPTH);
    assert_eq!(cache.pool.slots(), 2);

    assert!(
        !cache.affordable(0, 0, unit * (1 + retained) - 1),
        "a byte short of the list is a pool the budget never granted, so it is declined"
    );
    // One unit alone is the per-reader term, and the retained list is what
    // it does not carry.
    assert!(!cache.affordable(0, 0, unit));
    // And the reader's other two terms are inside the same number: the
    // same budget that afforded the blocks declines them once a chunk
    // buffer and a decoder are charged beside it.
    assert!(!cache.affordable(1, 1, unit * (1 + retained)));
}

/// **One slot count covers the retained blocks and the free ones
/// together**, so a block pool's ceiling is its stated budget rather than
/// twice it. Driven through the two calls the decode path makes — a slot,
/// then a retention — with the most recent block held live.
#[test]
fn a_block_pool_holds_one_slot_count_across_both_lists() {
    let table = four_block_table();
    let cache = BlockCache::for_table(&table).expect("4 KiB blocks have a unit");
    cache.pool.set_limits(cache.unit * 3, POOL_DEPTH);
    assert_eq!(cache.pool.slots(), 3);

    let mut live = None;
    for index in 0..12 {
        let slot = cache.slot();
        let block = Arc::new(DecodedBlock { slot, len: cache.unit });
        cache.retain(index, Arc::clone(&block));
        live = Some(block);
        let retained = cache.retained.lock().unwrap().len();
        let free = cache.pool.free_len();
        assert!(
            retained + free <= cache.pool.slots(),
            "read {index}: {retained} retained + {free} free exceeds {} slots",
            cache.pool.slots(),
        );
    }
    drop(live);
}

/// `xz` is not `mise`-pinned (`docs/design/decisions.md`, "D73"), so a
/// missing binary fails with the remedy rather than skipping silently
/// (`docs/design/roadmap.md`, "A test may assume the tools `mise`
/// pins").
fn require_xz() {
    let ok = std::process::Command::new("xz")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    assert!(ok, "`xz` is not runnable, so this test cannot build its fixture; install it.");
}

/// Compress `bytes` with `xz`, passing `extra_args` before the input path
/// (e.g. `&["--block-size=4096"]` for a seekable multi-block file, or
/// `&[]` for a single-stream, single-block one), and return the temp
/// file holding the compressed bytes.
fn xz_compress(bytes: &[u8], extra_args: &[&str]) -> tempfile::NamedTempFile {
    require_xz();
    let mut input = tempfile::NamedTempFile::new().unwrap();
    input.write_all(bytes).unwrap();
    input.flush().unwrap();
    let out = std::process::Command::new("xz")
        .args(extra_args)
        .arg("-c")
        .arg(input.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "xz {extra_args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut compressed = tempfile::NamedTempFile::new().unwrap();
    compressed.write_all(&out.stdout).unwrap();
    compressed.flush().unwrap();
    compressed
}

/// A payload long enough that a small `--block-size` reliably splits it
/// into several blocks — a throwaway pattern for pinning `XzSource`'s own
/// wiring, not a fixture.
fn xz_test_payload() -> Vec<u8> {
    (0..20_000u32).map(|i| (i % 251) as u8).collect()
}

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

/// `seek_table()` is what `cache::save` wraps into
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
}

/// **A compressed source recommends the cores it was given, capped at its
/// own block count** (`docs/design/decisions.md`, "D2"):
/// `crate::io::cut` cuts at block boundaries, so a file with fewer
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
    assert!(cache.retained.lock().unwrap().is_empty());

    let first = source.read_range(100, 200).await.unwrap();
    assert_eq!(&first[..], &payload[100..300]);
    let retained = cache.retained.lock().unwrap().clone();
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
    assert_eq!(cache.retained.lock().unwrap().len(), 1);
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
        assert!(cache.retained.lock().unwrap().len() <= slots);
        offset += 4096;
    }
    assert!(cache.retained.lock().unwrap().len() <= slots);
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

/// **A block the stated budget cannot hold is not decoded whole**, and the
/// line is the budget rather than a constant beside it
/// (`docs/design/decisions.md`, "D15"). Asserted against a synthetic
/// table, that being the only way to have a block this size without
/// writing one.
#[test]
fn a_block_too_large_to_hold_refuses_the_block_path() {
    let block = |uncompressed_size| xz_seek::BlockEntry {
        compressed_offset: 12,
        uncompressed_offset: 0,
        unpadded_size: 64,
        uncompressed_size,
    };
    let table = |uncompressed_size| xz_seek::SeekTable {
        compressed_file_size: 1 << 20,
        streams: vec![xz_seek::StreamEntry {
            compressed_offset: 0,
            uncompressed_offset: 0,
            compressed_size: 1 << 20,
            uncompressed_size,
            check: xz_seek::Check::Crc64,
            first_block_dict_size: Some(8 << 20),
            padding: 0,
            first_block: 0,
            block_count: 1,
        }],
        blocks: vec![block(uncompressed_size)],
    };
    // A cache exists for any file with blocks; whether the block path is
    // *taken* is the budget's answer, asked again on every read. The
    // decoder's charge is stated here rather than read off a reader — a
    // synthetic table has none — and stands for what
    // `xz_seek::Layout::decode_footprint` answers on an 8 MiB-dictionary
    // file.
    const DECODE: u64 = 9_471_776;
    let affordable = |unit: u64, budget: u64| {
        let cache = BlockCache::for_table(&table(unit)).expect("a block to build a unit from");
        cache.affordable(crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES as u64, DECODE, budget)
    };
    // **`POOL_DEPTH` units, not one.** A single reader is charged the
    // block it holds *and* the retention list beside it
    // ([`BlockCache::worker_memory`]), so the default budget declines an
    // ordinary compressed dump (`docs/design/decisions.md`, "D15").
    assert!(!affordable(24 << 20, DEFAULT_MEMORY_BUDGET));
    assert!(affordable(24 << 20, 107 << 20));
    // Larger blocks — `xz --block-size=128MiB`, and `xz -9 -T0` — are
    // declined at the default and taken once the caller allows the room.
    assert!(!affordable(128 << 20, DEFAULT_MEMORY_BUDGET));
    assert!(!affordable(192 << 20, DEFAULT_MEMORY_BUDGET));
    assert!(!affordable(192 << 20, 768 << 20), "four units of 192 MiB is 768");
    assert!(affordable(192 << 20, 779 << 20));
    // **The unavoidable terms come off the top**: the blocks alone fit
    // this budget and the file is declined all the same, a reader of it
    // also holding the chunk buffer and the decoder — both of which the
    // piecewise arm holds too.
    assert!(!affordable(24 << 20, 96 << 20));
    // A file with no blocks at all — `xz -c /dev/null` writes one — has no
    // unit to size a slot with, so there is no cache to ask.
    let empty =
        xz_seek::SeekTable { compressed_file_size: 32, streams: Vec::new(), blocks: Vec::new() };
    assert!(BlockCache::for_table(&empty).is_none());
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
    assert_eq!(cache.retained.lock().unwrap().len(), 2);

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

    let whole = XzSource::open(compressed.path()).unwrap();
    let pieces = XzSource::open(compressed.path()).unwrap();
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

/// Flip the last byte of block `index`'s stored check, leaving its payload
/// — and so every byte it decodes to — as it was. The check is the last
/// field of a block's padded extent, so that byte is always the check's.
fn corrupt_check(path: &Path, index: usize) {
    let table = XzSource::open(path).unwrap().seek_table().unwrap();
    let block = &table.blocks[index];
    let at = usize::try_from(block.compressed_offset + block.total_size() - 1).unwrap();
    let mut bytes = std::fs::read(path).unwrap();
    bytes[at] ^= 0xFF;
    std::fs::write(path, bytes).unwrap();
}

/// Whether `result` is a block failing its own check.
fn check_failed<T: std::fmt::Debug>(result: Result<T>) -> bool {
    matches!(&result, Err(error) if matches!(error.xz_cause(), Some(xz_seek::Error::BlockCheckFailed { .. })))
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

    let local = XzSource::open(compressed.path()).unwrap();
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

    let local = XzSource::open(compressed.path()).unwrap();
    local.hint_parallelism(Parallelism::Serial { memory_bytes: Some(1) });
    local.read_range(4096, 100).await.unwrap();
    local.hint_parallelism(Parallelism::default());
    assert_eq!(&local.read_range(0, 100).await.unwrap()[..], &payload[..100]);
    assert!(check_failed(local.complete_reads().await));
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

/// The three answers an origin exists to give, from one probe of a local
/// file: the bytes as stored, the weak identity, and the leading magic.
#[tokio::test]
async fn an_origin_probe_answers_size_identity_and_the_leading_bytes() {
    let (file, _source) = source_of(b"\xfd7zXZ\x00 and then some more bytes");
    let origin = Origin::local(file.path());
    let probed = origin.probe().await.unwrap();
    assert_eq!(probed.stored_size(), std::fs::metadata(file.path()).unwrap().len());
    assert_eq!(
        probed.modified(),
        Some(std::fs::metadata(file.path()).unwrap().modified().unwrap())
    );
    assert_eq!(probed.leading(), &XZ_MAGIC[..]);
}

/// A source shorter than the magic brings back what it has, which is not
/// an error and matches nothing.
#[tokio::test]
async fn an_origin_probe_of_a_short_source_brings_back_what_there_is() {
    let (file, _source) = source_of(b"ab");
    let origin = Origin::local(file.path());
    let probed = origin.probe().await.unwrap();
    assert_eq!(probed.leading(), b"ab");
    assert_eq!(probed.stored_size(), 2);
}

/// **The probe runs once.** Rewriting the file underneath a probed origin
/// changes nothing it answers — which is the property a source whose
/// probe is a round trip depends on, and the reason the claim and
/// recognition see one consistent set of answers.
#[tokio::test]
async fn an_origin_probes_at_most_once() {
    let (mut file, _source) = source_of(b"plain");
    let origin = Origin::local(file.path());
    let first = origin.probe().await.unwrap().clone();
    file.write_all(b" and rather more content than there was before").unwrap();
    file.flush().unwrap();
    assert_eq!(origin.probe().await.unwrap(), &first);
}

/// **A failed probe is not cached**, so the open that follows a claim
/// still reaches the failure — and an origin is not poisoned by having
/// been asked too early.
#[tokio::test]
async fn a_failed_origin_probe_is_not_remembered() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not-yet.sql");
    let origin = Origin::local(&path);
    assert!(origin.probe().await.is_err(), "nothing is there yet");
    std::fs::write(&path, b"now it is").unwrap();
    assert_eq!(origin.probe().await.unwrap().stored_size(), 9);
}

/// An origin names itself the way the message that refuses it does.
#[tokio::test]
async fn an_origin_displays_as_its_path() {
    let (file, _source) = source_of(b"x");
    assert_eq!(Origin::local(file.path()).to_string(), file.path().display().to_string());
}

/// Recognition with nothing claimed, unwrapped: `KnownCompression::Unknown`
/// claims nothing recognition could contradict.
async fn recognize(path: &Path) -> Arc<dyn ByteRangeSource> {
    match open_local(&Origin::local(path), KnownCompression::Unknown).await.unwrap() {
        Recognized::Source(source) => source,
        Recognized::Mismatch => panic!("`Unknown` claims nothing to contradict"),
    }
}

/// A genuinely `.xz`-compressed file is recognised whatever it is named
/// (`docs/design/decisions.md`, "D14"): the temp file `xz_compress`
/// returns carries no `.xz` suffix, and `open_local` still hands back a
/// source whose `seek_table()` answers `Some`, which of what `open_local`
/// returns only `XzSource` does.
#[tokio::test]
async fn open_local_recognizes_xz_content_with_no_xz_name() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    assert_ne!(compressed.path().extension(), Some(std::ffi::OsStr::new("xz")));

    let source = recognize(compressed.path()).await;
    assert!(source.seek_table().is_some(), "content-sniffed as .xz");
    assert_eq!(source.size().await.unwrap(), payload.len() as u64);
    let got = source.read_range(0, payload.len()).await.unwrap();
    assert_eq!(&got[..], &payload[..]);
}

/// The other direction: a file *named* `.xz` whose bytes are not, which
/// content sniffing opens plain (`docs/design/decisions.md`, "D14").
#[tokio::test]
async fn open_local_opens_a_dot_xz_named_file_with_plain_content_as_plain() {
    let mut file = tempfile::Builder::new().suffix(".xz").tempfile().unwrap();
    file.write_all(b"not actually compressed").unwrap();
    file.flush().unwrap();

    let source = recognize(file.path()).await;
    assert!(source.seek_table().is_none(), "no compression layer — read as plain");
    let got = source.read_range(0, 23).await.unwrap();
    assert_eq!(&got[..], b"not actually compressed");
}

/// A file too short to hold the six-byte magic must not be mistaken for
/// `.xz`, and must not error just for being short.
#[tokio::test]
async fn open_local_treats_a_file_shorter_than_the_magic_as_plain() {
    let (_file, plain) = source_of(b"ab");
    let path = plain.path().to_path_buf();
    let source = recognize(&path).await;
    assert!(source.seek_table().is_none());
    assert_eq!(source.size().await.unwrap(), 2);
}

/// A table a previous walk produced is handed back and used: the source
/// reads correctly and answers the same table it was given.
#[tokio::test]
async fn open_local_builds_an_xz_source_from_a_handed_back_table() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let table = XzSource::open(compressed.path()).unwrap().seek_table().unwrap();

    let source =
        match open_local(&Origin::local(compressed.path()), KnownCompression::Xz(table.clone()))
            .await
            .unwrap()
        {
            Recognized::Source(source) => source,
            Recognized::Mismatch => panic!("the file's own table must describe it"),
        };
    assert_eq!(source.seek_table().as_ref(), Some(&table));
    assert_eq!(source.size().await.unwrap(), payload.len() as u64);
    let got = source.read_range(9_000, 4000).await.unwrap();
    assert_eq!(&got[..], &payload[9_000..13_000]);
}

// -----------------------------------------------------------------------
// The fetched `.xz` source: one block at a time, out of windows the caller
// fetched (`docs/design/decisions.md`, "D14", "D15" and "D18").
//
// The transport here is a local file, which is exactly the point: the
// window-fed path and its local twin, `XzSource`, then read one file, so
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
async fn fetched_xz(path: &Path) -> (FetchedXzSource, usize) {
    let transport: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(path).unwrap());
    let stored_size = transport.size().await.unwrap();
    let leading = transport.read_range(0, ORIGIN_LEADING_BYTES).await.unwrap();
    let (table, fetches) = walk_seek_table(&*transport, stored_size, &leading).await.unwrap();
    (FetchedXzSource::with_table(transport, stored_size, table).unwrap(), fetches)
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
    assert!(source.seek_table().unwrap().block_count() > 1, "the fixture must be split");
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
    let local = XzSource::open(compressed.path()).unwrap().seek_table().unwrap();
    assert_eq!(source.seek_table().as_ref(), Some(&local));
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
/// same bytes out of the same windows, through `xz_seek::BlockRead` rather
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
    let table = source.seek_table().unwrap();
    let first = table.blocks[0].uncompressed_size as usize;
    let step = first / 8;
    assert!(step > 0, "the fixture's blocks hold several reads apiece: {first} byte(s)");

    for read in 0..4usize {
        let at = read * step;
        assert_eq!(&source.read_range(at as u64, step).await.unwrap()[..], &payload[at..at + step]);
    }
    assert_eq!(source.block_decodes_begun(), 1, "four forward reads, one decode");

    // Leaving the block and coming back is what costs one apiece, which
    // is the local arm's shape exactly.
    let second = table.blocks[1].uncompressed_offset as usize;
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
    let table = XzSource::open(other.path()).unwrap().seek_table().unwrap();

    let transport: Arc<dyn ByteRangeSource> =
        Arc::new(LocalFileSource::open(compressed.path()).unwrap());
    let stored_size = transport.size().await.unwrap();
    let err = FetchedXzSource::with_table(transport, stored_size, table).unwrap_err();
    assert!(matches!(err.xz_cause(), Some(xz_seek::Error::InvalidTable { .. })), "{err:?}");
}

/// The single-block shape reads correctly too — there is no seek structure
/// to cut at, so every read decodes the one block from its start, and the
/// piecewise arm is what keeps that from allocating the whole plaintext.
#[tokio::test]
async fn a_single_block_file_is_read_by_the_same_two_arms() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &[]);
    let (source, _) = fetched_xz(compressed.path()).await;
    assert!(!source.seek_table().unwrap().is_seekable());
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

    let local = XzSource::open(compressed.path()).unwrap();
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
    let table = source.seek_table().unwrap();
    let widest = table.blocks.iter().map(|b| b.total_size()).max().unwrap();
    assert!(widest > 0);

    let local = XzSource::open(compressed.path()).unwrap();
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

/// **The walk is actually skipped**, tested behaviourally rather than by
/// instrumentation (`docs/design/decisions.md`, "D18"): the table handed
/// in names a check algorithm this file's streams do not use, which
/// `validate` does not police. The check's size moves the payload's end,
/// so a decode from the handed table fails where a silent re-walk would
/// have read the payload back happily.
#[tokio::test]
async fn a_handed_back_table_is_used_rather_than_re_walked() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let mut table = XzSource::open(compressed.path()).unwrap().seek_table().unwrap();
    let real = table.streams[0].check;
    let wrong =
        if real == xz_seek::Check::Crc32 { xz_seek::Check::Crc64 } else { xz_seek::Check::Crc32 };
    for stream in &mut table.streams {
        stream.check = wrong;
    }

    let source = match open_local(&Origin::local(compressed.path()), KnownCompression::Xz(table))
        .await
        .unwrap()
    {
        Recognized::Source(source) => source,
        Recognized::Mismatch => panic!("`validate` does not police the check algorithm"),
    };
    assert!(
        source.read_range(0, 4000).await.is_err(),
        "a decode under the wrong check must fail — a silent re-walk would have succeeded"
    );
}

/// A table that does not describe this file is refused, and the refusal
/// is an outcome rather than an error: nothing was read and the caller
/// still chooses whether to pay the walk.
#[tokio::test]
async fn open_local_reports_a_table_that_does_not_describe_the_file() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    // A second, shorter file's table: `validate` compares the table's
    // recorded compressed length against this source's.
    let other = xz_compress(&payload[..5_000], &["--block-size=4096"]);
    let other_table = XzSource::open(other.path()).unwrap().seek_table().unwrap();

    assert!(matches!(
        open_local(&Origin::local(compressed.path()), KnownCompression::Xz(other_table))
            .await
            .unwrap(),
        Recognized::Mismatch
    ));
}

/// Both directions of "the cache describes a different file": a
/// compression index for a file whose bytes are plain, and a plain cache
/// for a file whose bytes are `.xz`.
#[tokio::test]
async fn open_local_reports_a_compression_claim_the_file_contradicts() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let table = XzSource::open(compressed.path()).unwrap().seek_table().unwrap();
    let (plain_file, _plain) = source_of(&payload);

    assert!(matches!(
        open_local(&Origin::local(plain_file.path()), KnownCompression::Xz(table)).await.unwrap(),
        Recognized::Mismatch
    ));
    assert!(matches!(
        open_local(&Origin::local(compressed.path()), KnownCompression::Plain).await.unwrap(),
        Recognized::Mismatch
    ));
}

/// A plain claim about a plain file is simply right, and costs nothing.
#[tokio::test]
async fn open_local_accepts_a_plain_claim_about_a_plain_file() {
    let (file, _plain) = source_of(b"0123456789abcdef");
    let source =
        match open_local(&Origin::local(file.path()), KnownCompression::Plain).await.unwrap() {
            Recognized::Source(source) => source,
            Recognized::Mismatch => panic!("a plain file is what the claim said"),
        };
    assert!(source.seek_table().is_none());
    assert_eq!(source.size().await.unwrap(), 16);
}

/// A filesystem root shaped like a Linux one, for driving discovery
/// against something other than the machine the test is running on.
///
/// **Everything the reader consults is under the root** — `proc/self/cgroup`,
/// `proc/meminfo`, and the cgroup mount — which is what makes the v1 arm
/// executable at all: the memory controller lives in exactly one hierarchy
/// (`RT6`), so no v1 shape can exist beside this machine's v2 one.
struct FakeRoot(tempfile::TempDir);

impl FakeRoot {
    fn new() -> FakeRoot {
        FakeRoot(tempfile::tempdir().unwrap())
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    /// Write `body` at `rel`, creating the directories above it.
    fn write(&self, rel: &str, body: &str) -> &FakeRoot {
        let at = self.0.path().join(rel);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, body).unwrap();
        self
    }

    /// The `0::` line for a v2 process at `cgroup`.
    fn v2(&self, cgroup: &str) -> &FakeRoot {
        self.write("proc/self/cgroup", &format!("0::{cgroup}\n"))
    }

    /// A v2 cgroup directory's two limit files, each `None` for absent.
    fn v2_limits(&self, cgroup: &str, max: Option<&str>, high: Option<&str>) -> &FakeRoot {
        let dir = format!("sys/fs/cgroup{cgroup}");
        let dir = dir.trim_end_matches('/');
        if let Some(max) = max {
            self.write(&format!("{dir}/memory.max"), &format!("{max}\n"));
        }
        if let Some(high) = high {
            self.write(&format!("{dir}/memory.high"), &format!("{high}\n"));
        }
        self
    }
}

/// **The `0::` line is selected by its shape, and a dead cgroup's path is
/// not a directory name** (`RT1`). The hierarchy id is whatever
/// `idr_alloc_cyclic` handed out and the line order is unspecified, so
/// neither may be read; the ` (deleted)` suffix would otherwise be joined
/// onto the mount point.
#[test]
fn the_unified_line_is_found_by_shape_and_stripped_of_a_deleted_suffix() {
    let root = FakeRoot::new();
    root.write("proc/self/cgroup", "7:name=probe:/somewhere\n0::/leaf (deleted)\n").v2_limits(
        "/leaf",
        Some("536870912"),
        None,
    );
    assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(536870912));
}

/// **The controller field is a comma-separated list matched by
/// membership** (`RT6`): a v1 hierarchy carrying only a *name* reads
/// `name=memory` and holds no memory controller at all.
#[test]
fn a_hierarchy_merely_named_memory_is_not_the_memory_controller() {
    let root = FakeRoot::new();
    root.write("proc/self/cgroup", "3:name=memory:/decoy\n0::/leaf\n").v2_limits(
        "/leaf",
        Some("268435456"),
        None,
    );
    assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(268435456));
}

/// **`max` is a sentinel for one level, not a statement that the process is
/// unlimited** (`RT2`), and an absent file is "no limit here" rather than an
/// error (`RT5`) — the hierarchy root carries neither file at all. So the
/// walk climbs past both and finds the ancestor that binds.
#[test]
fn an_ancestors_limit_binds_where_the_leaf_states_max() {
    let root = FakeRoot::new();
    root.v2("/pgdt-probe/child")
        .v2_limits("/pgdt-probe/child", Some("max"), Some("max"))
        .v2_limits("/pgdt-probe", Some("268435456"), None);
    assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(268435456));
}

/// **`memory.high` and `memory.max` are minimised together, across levels
/// and across the two files** (`RT3`, `RT5`): nothing orders them, and the
/// throttle two levels up may be the smallest number in the walk.
#[test]
fn the_smallest_of_every_limit_at_every_level_is_what_binds() {
    let root = FakeRoot::new();
    root.v2("/a/b/c")
        .v2_limits("/a/b/c", Some("2147483648"), None)
        .v2_limits("/a/b", None, Some("134217728"))
        .v2_limits("/a", Some("1073741824"), None);
    assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(134217728));
}

/// **The file that stated the smallest limit comes back with it**:
/// `memory.high` throttles where `memory.max` kills (`RT3`) and either may
/// be an ancestor's (`RT5`), so a status line is only actionable beside the
/// file that cut the budget (`docs/design/decisions.md`, "D64").
#[test]
fn a_discovered_limit_names_the_file_that_stated_it() {
    let root = FakeRoot::new();
    root.v2("/a/b").v2_limits("/a/b", Some("2147483648"), None).v2_limits(
        "/a",
        None,
        Some("134217728"),
    );
    let limit = discover_memory_limit_in(root.path()).expect("a limit binds here");
    assert_eq!(limit.bytes, 134217728);
    assert_eq!(limit.read_from, root.path().join("sys/fs/cgroup/a/memory.high"));
}

/// **Nothing anywhere is `None`, and that is a complete statement**: no
/// limit found means no limit is being enforced. A tree with no cgroup
/// file at all reads the same way, which is the non-Linux case.
#[test]
fn an_unlimited_hierarchy_and_a_missing_one_both_read_as_no_limit() {
    let root = FakeRoot::new();
    root.v2("/leaf").v2_limits("/leaf", Some("max"), Some("max"));
    assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), None);
    assert_eq!(discover_memory_limit_in(FakeRoot::new().path()).map(|l| l.bytes), None);
}

/// **A v1 hierarchy is read from its own mount, located through
/// `mountinfo`** (`RT4`, `RT6`), and "unlimited" there is a *threshold*
/// near `LONG_MAX` rather than a sentinel string, the value being a
/// function of the page size and the word width.
///
/// This establishes that the reader handles the shape `RT4` describes. It
/// observes no kernel (`docs/design/decisions.md`, "D11").
#[test]
fn the_v1_arm_reads_its_own_mount_and_treats_a_huge_value_as_no_limit() {
    let root = FakeRoot::new();
    root.write("proc/self/cgroup", "5:memory:/svc\n0::/leaf\n")
        .write(
            "proc/self/mountinfo",
            "31 24 0:27 / /sys/fs/cgroup/memory rw,nosuid - cgroup cgroup rw,memory\n",
        )
        .write("sys/fs/cgroup/memory/svc/memory.limit_in_bytes", "536870912\n")
        // The v2 files are there and must not be consulted: a controller
        // lives in exactly one hierarchy, and this process's memory
        // controller is the v1 one.
        .v2_limits("/leaf", Some("104857600"), None);
    assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(536870912));

    let unset = FakeRoot::new();
    unset
        .write("proc/self/cgroup", "5:memory:/\n")
        .write(
            "proc/self/mountinfo",
            "31 24 0:27 / /sys/fs/cgroup/memory rw - cgroup cgroup rw,memory\n",
        )
        // 32-bit `PAGE_COUNTER_MAX × PAGE_SIZE`, the smallest shape
        // "unlimited" takes and so the one a threshold has to clear.
        .write("sys/fs/cgroup/memory/memory.limit_in_bytes", "8796093018112\n");
    assert_eq!(discover_memory_limit_in(unset.path()).map(|l| l.bytes), None);
}

/// **The conventional mount is the fallback, not the authority** — a tree
/// that lists no cgroup mount still reads `/sys/fs/cgroup/memory`, which is
/// where `file-hierarchy(7)` puts it.
#[test]
fn the_v1_arm_falls_back_to_the_conventional_mount() {
    let root = FakeRoot::new();
    root.write("proc/self/cgroup", "5:cpu,memory,cpuacct:/svc\n")
        .write("sys/fs/cgroup/memory/svc/memory.limit_in_bytes", "268435456\n");
    assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(268435456));
}

/// **`MemAvailable`, in kB, and nothing else** (`RT8`). `MemFree` is on
/// the same file and is not read: it excludes reclaimable page cache.
#[test]
fn available_memory_reads_memavailable_and_converts_from_kb() {
    let root = FakeRoot::new();
    root.write(
        "proc/meminfo",
        "MemTotal:       32774304 kB\nMemFree:         1489828 kB\n\
             MemAvailable:   20002184 kB\n",
    );
    assert_eq!(available_memory_in(root.path()), Some(20002184 * 1024));
    assert_eq!(available_memory_in(FakeRoot::new().path()), None);
}

/// **A discovered limit caps what a source asked for; it does not become
/// it.** The reserve comes off the top and the source's recommendation is
/// taken no higher, while a recommendation that already fits is left
/// alone. The count is held to [`MEMORY_MARGIN_PERCENT`] besides, the
/// tighter of the two conditions at this limit; the budget is still what
/// the resolved count spends.
#[test]
fn a_discovered_limit_caps_the_recommendation_at_the_limit_less_the_reserve() {
    let root = FakeRoot::new();
    root.v2("/leaf").v2_limits("/leaf", Some("1073741824"), None);
    let cap = (1024 << 20) - MEMORY_RESERVE;

    // Far more than the cap affords, so the count comes down with the
    // budget rather than being printed beside one it cannot spend. The
    // budget is still `cap`-bounded and not margin-bounded, so the floor
    // arrangement reports what one reader holds.
    let squeezed =
        Parallelism::discover_in(root.path(), 8, Some(WorkerMemory::per_worker(512 << 20)));
    assert_eq!(squeezed.memory_bytes(), Some(512 << 20));
    assert_eq!(squeezed.jobs(), 1);

    // Inside the margin allowance as well as inside the cap, so both
    // numbers stand.
    let roomy = Parallelism::discover_in(root.path(), 8, Some(WorkerMemory::per_worker(16 << 20)));
    assert_eq!(roomy.memory_bytes(), Some(128 << 20));
    assert_eq!(roomy.jobs(), 8);

    // And in between, the count is what the margin affords and the budget
    // is exactly what that many readers spend — never the cap itself,
    // which is the over-ask this pairing exists to remove. One more reader
    // would fit the cap and is refused.
    let fitted =
        Parallelism::discover_in(root.path(), 8, Some(WorkerMemory::per_worker(100 << 20)));
    assert_eq!(fitted.jobs(), 5);
    assert_eq!(fitted.memory_bytes(), Some(500 << 20));
    assert!(fitted.memory_bytes() < Some(cap), "the cap itself would be the over-ask");
}

/// **The resolved count answers to the criterion rather than to the host's
/// width**: a recommendation above what the margin allows resolves the
/// same arrangement whatever the recommendation is
/// (`docs/design/decisions.md`, "D3").
#[test]
fn the_count_answers_to_the_margin_and_not_to_the_recommendation() {
    let root = FakeRoot::new();
    root.v2("/leaf").v2_limits("/leaf", Some(&(2u64 << 30).to_string()), None);
    // What one reader of a large-block `.xz` holds, and the retention list
    // the pool keeps beside it — a block size at which this limit resolves
    // well inside either host's core count, so what binds is unambiguous.
    let reader = 138 << 20;
    let memory = WorkerMemory::per_worker(reader).pooling(128 << 20, POOL_DEPTH);

    let narrow = Parallelism::discover_in(root.path(), 24, Some(memory));
    let wide = Parallelism::discover_in(root.path(), 64, Some(memory));
    assert_eq!(narrow.jobs(), wide.jobs(), "the host's core count is not the answer");
    assert_eq!(narrow.memory_bytes(), wide.memory_bytes());

    // And what it resolves to leaves the margin, where the cap alone would
    // have admitted another reader.
    let limit = 2u64 << 30;
    let charge = wide.memory_bytes().unwrap();
    assert!(
        charge + MEMORY_UNPOOLED_BOUND <= limit / 100 * (100 - MEMORY_MARGIN_PERCENT),
        "predicted resident {} breaches the margin",
        charge + MEMORY_UNPOOLED_BOUND
    );
    assert!(
        memory.at(wide.jobs() + 1) + MEMORY_UNPOOLED_BOUND
            > limit / 100 * (100 - MEMORY_MARGIN_PERCENT),
        "one more reader would still have fitted, so the margin is not what bound it"
    );
    assert!(
        memory.at(wide.jobs() + 1) <= limit - MEMORY_RESERVE,
        "the cap alone would have afforded more, which is what the margin is for"
    );
}

/// **A margin the smallest arrangement cannot meet costs nothing.** The
/// floor is one worker at whatever the cap is, so a limit whose margin
/// allowance is under one reader's charge still resolves that reader and
/// still reports the budget it spends — which is what
/// `BlockCache::affordable` reads, so the compressed block path is not
/// declined by the margin at any limit.
///
/// The arrangement needs a limit whose margin allowance falls under one
/// reader's charge while the cap stays above it, which a large-block file
/// reaches and a small-block one does not.
#[test]
fn the_margin_never_takes_the_last_reader_or_the_budget_it_spends() {
    let root = FakeRoot::new();
    // A limit whose margin allowance is under what one block-decoding
    // reader of this file is charged, while its cap is above it.
    root.v2("/leaf").v2_limits("/leaf", Some(&(960u64 << 20).to_string()), None);
    let memory = WorkerMemory::per_worker(138 << 20).pooling(128 << 20, POOL_DEPTH);
    let tight = Parallelism::discover_in(root.path(), 24, Some(memory));
    assert_eq!(tight.jobs(), 1);
    assert_eq!(tight.memory_bytes(), Some(memory.at(1)));
    assert!(memory.at(1) > margin_allowance(960 << 20), "the margin cannot afford it");
}

/// **The margin binds at a large limit and is inert at a small one**, which
/// is the shape it exists for: a constant reserve leaves a shrinking
/// *share* as the limit grows, so the fraction is needed at the top end and
/// the subtraction already covers the bottom. The crossover is arithmetic
/// rather than a reading — `limit − MEMORY_RESERVE` and
/// `(100 − margin)% × limit − MEMORY_UNPOOLED_BOUND` are equal at
/// `5 × (MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND)` — and it is pinned here
/// because nothing else would notice either constant moving past the other.
#[test]
fn the_margin_is_the_tighter_condition_only_above_the_crossover() {
    let crossover = 5 * (MEMORY_RESERVE - MEMORY_UNPOOLED_BOUND);
    assert_eq!(crossover, 640 << 20);
    // Equal there but for the rounding: `limit / 100` discards under a
    // hundred bytes, so the allowance errs small by that much and never
    // the other way.
    assert!(crossover - MEMORY_RESERVE - margin_allowance(crossover) < 100);
    // Below it the cap is what a count is solved against, so the margin
    // can lower nothing; above it the margin is, at every limit.
    assert!(margin_allowance(crossover - (64 << 20)) > (crossover - (64 << 20)) - MEMORY_RESERVE);
    assert!(margin_allowance(crossover + (64 << 20)) < (crossover + (64 << 20)) - MEMORY_RESERVE);
    assert!(margin_allowance(4 << 30) < (4 << 30) - MEMORY_RESERVE);
}

/// **What a mapping pass's statistics are left has two bands, and its
/// floor is not the margin's** (`docs/design/decisions.md`, "D85"). Both
/// lines are arithmetic between the three constants rather than readings,
/// and nothing else would notice one of them moving past another.
///
/// **Its own zero line is `MEMORY_UNPOOLED_BOUND × 100 / (100 −
/// MEMORY_MARGIN_PERCENT)`** — at and below it the ceiling this subtracts
/// a budget from is zero on its own, so no allowance there gathers
/// anything at any budget, and the line is reached rather than merely
/// approached.
///
/// **The swap is `the_margin_is_the_tighter_condition_only_above_the_crossover`'s
/// crossover read the other way round.** Below it a count is solved
/// against the cap, which stands under the ceiling, so `ceiling − cap`
/// survives whatever the source charges; above it the count is solved
/// against the ceiling itself and a source whose step divides it takes the
/// statistics to nothing — a wide allowance, not a narrow one.
#[test]
fn statistics_are_starved_below_their_own_line_and_unfloored_above_the_crossover() {
    let zero_line = MEMORY_UNPOOLED_BOUND * 100 / (100 - MEMORY_MARGIN_PERCENT);
    assert_eq!(zero_line, 320 << 20);
    assert_eq!(statistics_allowance(zero_line, 0), 0, "at the line, not only under it");
    assert_eq!(statistics_allowance(zero_line - (1 << 20), 0), 0);
    assert!(statistics_allowance(zero_line + (1 << 20), 0) > 0, "and positive above it");

    let crossover = 5 * (MEMORY_RESERVE - MEMORY_UNPOOLED_BOUND);
    let below = crossover - (64 << 20);
    let above = crossover + (64 << 20);

    // Below the crossover no budget a fit reports can reach the ceiling,
    // the cap being the tighter of the two: `ceiling − cap` is left at
    // every per-worker charge, including one too large for a single worker.
    let floor = margin_allowance(below) - (below - MEMORY_RESERVE);
    assert!(floor > 0, "an allowance in this band cannot starve statistics");
    for per_worker in [1u64 << 20, 17 << 20, 64 << 20, 96 << 20, 1 << 30] {
        let fitted = Parallelism::within(24, Some(WorkerMemory::per_worker(per_worker)), below);
        let left = statistics_allowance(below, fitted.memory_bytes().unwrap());
        assert!(left >= floor, "{per_worker} left {left}, under the cap's own floor {floor}");
    }

    // Above it the ceiling binds instead, so the count is solved right up
    // against the number this subtracts from and there is no floor left.
    let ceiling = margin_allowance(above);
    assert!(ceiling < above - MEMORY_RESERVE, "the ceiling binds here, not the cap");
    let fitted = Parallelism::within(24, Some(WorkerMemory::per_worker(ceiling / 4)), above);
    assert_eq!(fitted.jobs(), 4, "four workers is what the ceiling exactly affords");
    assert_eq!(statistics_allowance(above, fitted.memory_bytes().unwrap()), 0);
}

/// **Below the reserve the budget goes to zero rather than to a floor**
/// (`docs/design/decisions.md`, "D3"). What zero produces is one reader's
/// worth on the streaming path, through the floors already in the
/// mechanism.
#[test]
fn a_limit_at_or_under_the_reserve_leaves_no_budget_at_all() {
    let root = FakeRoot::new();
    root.v2("/leaf").v2_limits("/leaf", Some(&MEMORY_RESERVE.to_string()), None);
    let starved = Parallelism::discover_in(root.path(), 4, Some(WorkerMemory::per_worker(1 << 30)));
    assert_eq!(starved.memory_bytes(), Some(0));
    // One worker at whatever the cap is, not one worker's worth of bytes:
    // the floor is on the count.
    assert_eq!(starved.jobs(), 1);

    let tighter = FakeRoot::new();
    tighter.v2("/leaf").v2_limits("/leaf", Some("134217728"), None);
    assert_eq!(Parallelism::discover_in(tighter.path(), 4, None).memory_bytes(), Some(0));
}

/// **A source with no recommendation is still capped by a discovered
/// limit**, at [`DEFAULT_MEMORY_BUDGET`] where the limit leaves room — the
/// plain path, which asks for nothing and is left where it was.
#[test]
fn a_source_that_recommends_nothing_is_capped_at_the_existing_default() {
    let root = FakeRoot::new();
    root.v2("/leaf").v2_limits("/leaf", Some("3221225472"), None);
    assert_eq!(
        Parallelism::discover_in(root.path(), 1, None).memory_bytes(),
        Some(DEFAULT_MEMORY_BUDGET)
    );
}

/// **The composition is not a `min`.** With no limit found there is no cap
/// from the environment at all, so a source's recommendation stands, where
/// a `min` against [`DEFAULT_MEMORY_BUDGET`] would have collapsed a
/// compressed scan to serial (`docs/design/decisions.md`, "D3").
#[test]
fn no_limit_found_leaves_the_recommendation_uncapped_but_for_memavailable() {
    let root = FakeRoot::new();
    root.v2("/leaf")
        .v2_limits("/leaf", Some("max"), None)
        .write("proc/meminfo", "MemAvailable:   20002184 kB\n");
    // Readers of an ordinary compressed dump, well under half of the
    // `MemAvailable` stated above.
    let per_worker = 58 << 20;
    let roomy =
        Parallelism::discover_in(root.path(), 24, Some(WorkerMemory::per_worker(per_worker)));
    assert_eq!(roomy.memory_bytes(), Some(per_worker * 24));
    assert_eq!(roomy.jobs(), 24);

    // Half of `MemAvailable`, because it is an estimate two processes
    // reading at once each see the whole of — and it binds wherever the
    // recommended count costs more than half of what is free, the count
    // then reduced to what half of it buys rather than left standing.
    let small = FakeRoot::new();
    small.v2("/leaf").write("proc/meminfo", "MemAvailable:     262144 kB\n");
    let squeezed =
        Parallelism::discover_in(small.path(), 24, Some(WorkerMemory::per_worker(per_worker)));
    assert_eq!(squeezed.jobs(), 2);
    assert_eq!(squeezed.memory_bytes(), Some(per_worker * 2));

    // `MemAvailable` unreadable is the one shape with a recommendation and
    // no ceiling over it: the whole of what the count asks for.
    let blind = FakeRoot::new();
    blind.v2("/leaf");
    assert_eq!(
        Parallelism::discover_in(blind.path(), 24, Some(WorkerMemory::per_worker(per_worker)))
            .memory_bytes(),
        Some(per_worker * 24)
    );
}

/// **A stated allowance is the discovered limit's own carving**, which is
/// the whole of what `--memory` promises: the same number reached either
/// way resolves to the same pair (`docs/design/decisions.md`, "D83"). The
/// allowances span the margin's crossover, so the check covers the band
/// where the cap binds and the band where the ceiling does.
#[test]
fn a_stated_allowance_resolves_as_the_same_number_discovered_would() {
    let memory = Some(WorkerMemory::per_worker(58 << 20).pooling(24 << 20, POOL_DEPTH));
    for allowance in [512u64 << 20, 640 << 20, 1024 << 20, 4096 << 20] {
        let root = FakeRoot::new();
        root.v2("/leaf").v2_limits("/leaf", Some(&allowance.to_string()), None);
        for jobs in [1, 4, 24] {
            assert_eq!(
                Parallelism::within(jobs, memory, allowance),
                Parallelism::discover_in(root.path(), jobs, memory),
                "{allowance} bytes at {jobs} jobs",
            );
        }
    }
}

/// **The typed number bounds the process, and what comes back bounds the
/// pools**: the reserve is off the top before a reader is counted, and an
/// allowance the reserve swallows resolves to a budget of zero rather than
/// to a floor — which the floors inside the mechanism turn into one reader
/// on the streaming path.
#[test]
fn a_stated_allowance_pays_the_reserve_before_anything_else() {
    let per_worker = 58 << 20;
    let memory = Some(WorkerMemory::per_worker(per_worker));

    // Six readers fit inside the 400 MiB *left* by the allowance; the
    // seventh does not. The budget is what those six spend, never the
    // number typed.
    let cut = Parallelism::within(24, memory, MEMORY_RESERVE + (400 << 20));
    assert_eq!(cut.jobs(), 6);
    assert_eq!(cut.memory_bytes(), Some(6 * per_worker), "the carved spend, not the allowance");

    // Room for the whole recommendation leaves it standing.
    let roomy = Parallelism::within(24, memory, 64 << 30);
    assert_eq!(roomy.jobs(), 24);
    assert_eq!(roomy.memory_bytes(), Some(24 * per_worker));

    // At and below the reserve there is nothing left to spend.
    let starved = Parallelism::within(24, memory, MEMORY_RESERVE);
    assert_eq!(starved, Parallelism::Serial { memory_bytes: Some(0) });

    // Nothing to divide by leaves the count where it is, on the constant.
    let blind = Parallelism::within(8, None, 64 << 30);
    assert_eq!(blind.jobs(), 8);
    assert_eq!(blind.memory_bytes(), Some(DEFAULT_MEMORY_BUDGET));
}

/// **A share of an allowance others are drawing on spends only what they
/// leave.** Inside 10 GiB the margin affords seven 1 GiB readers: the first
/// arrangement takes the four it asks for, the second the three that are
/// left, and one arriving after the whole has been drawn gets one reader on
/// nothing.
#[test]
fn a_shared_allowance_leaves_each_arrangement_what_the_others_have_not_drawn() {
    let per_worker = 1 << 30;
    let memory = Some(WorkerMemory::per_worker(per_worker));
    let allowance = 10 << 30;
    assert_eq!(
        Parallelism::within_shared(24, memory, allowance, 0, 0),
        Parallelism::within(24, memory, allowance),
        "nothing drawn is `within` itself"
    );
    assert_eq!(Parallelism::within(24, memory, allowance).jobs(), 7);

    let first = Parallelism::within_shared(4, memory, allowance, 0, 0);
    assert_eq!(first.jobs(), 4);
    let drawn = first.memory_bytes().unwrap();
    let second = Parallelism::within_shared(4, memory, allowance, drawn, 0);
    assert_eq!(second.jobs(), 3);
    assert_eq!(second.memory_bytes(), Some(3 * per_worker));

    let exhausted = Parallelism::within_shared(4, memory, allowance, allowance, 0);
    assert_eq!(exhausted, Parallelism::Serial { memory_bytes: Some(0) });
}

/// **What the process holds besides its arrangements comes off the
/// margin's ceiling alone.** Where the ceiling binds, a holding lowers
/// the count exactly as a draw of the same size does; where the cap binds
/// and the holding fits under the gap between them, it changes nothing a
/// draw would. **Where the count cannot fall, what the ceiling's room
/// cannot absorb comes off the budget**: one worker, or a source
/// recommending nothing, which keeps its count, spends only what the
/// holding leaves under the ceiling, and nothing once it fills it.
#[test]
fn a_holding_comes_off_the_ceiling_and_never_the_cap() {
    let per_worker = 1 << 30;
    let memory = Some(WorkerMemory::per_worker(per_worker));
    let allowance = 10 << 30;
    let held = 2 << 30;
    let holding = Parallelism::within_shared(24, memory, allowance, 0, held);
    assert_eq!(holding, Parallelism::within_shared(24, memory, allowance, held, 0));
    assert_eq!(holding.jobs(), 5);

    // Under `5 × (MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND)` the cap binds,
    // and a holding no larger than the ceiling's lead over it is free.
    let small = 600 << 20;
    let cap = small - MEMORY_RESERVE;
    let unit = Some(WorkerMemory::per_worker(cap / 4));
    let lead = margin_allowance(small) - cap;
    assert!(lead > 0);
    assert_eq!(Parallelism::within(64, unit, small).jobs(), 4);
    assert_eq!(
        Parallelism::within_shared(64, unit, small, 0, lead),
        Parallelism::within(64, unit, small),
    );
    assert_eq!(Parallelism::within_shared(64, unit, small, lead, 0).jobs(), 3);

    // One worker, the floor: the ceiling's room at one reader absorbs
    // what it can, and the rest comes off that reader's budget.
    let ceiling = margin_allowance(allowance);
    let short = 256 << 20;
    let floor = Parallelism::within_shared(4, memory, allowance, 0, ceiling - per_worker + short);
    assert_eq!(floor, Parallelism::Serial { memory_bytes: Some(per_worker - short) });
    let filled = Parallelism::within_shared(4, memory, allowance, 0, allowance);
    assert_eq!(filled, Parallelism::Serial { memory_bytes: Some(0) });

    // A source recommending nothing keeps its count and its constant
    // until the holding leaves less than the constant under the ceiling.
    let blind = Parallelism::within(8, None, allowance);
    assert_eq!(blind.memory_bytes(), Some(DEFAULT_MEMORY_BUDGET));
    let room = ceiling - DEFAULT_MEMORY_BUDGET;
    assert_eq!(Parallelism::within_shared(8, None, allowance, 0, room), blind);
    let left = 10 << 20;
    assert_eq!(
        Parallelism::within_shared(8, None, allowance, 0, ceiling - left),
        Parallelism::workers(8, left),
    );
    assert_eq!(
        Parallelism::within_shared(8, None, allowance, 0, allowance),
        Parallelism::workers(8, 0),
    );

    // Nothing held is `fit`'s budget, even past the ceiling: one worker
    // costing more than the cap is budgeted the cap.
    let narrow = 1 << 30;
    let alone = Parallelism::within_shared(1, memory, narrow, 0, 0);
    assert_eq!(alone.memory_bytes(), Some(narrow - MEMORY_RESERVE));
    assert!(narrow - MEMORY_RESERVE > margin_allowance(narrow));
}

/// **An unlimited environment falls back to the shipped constant**, and at
/// a serial count all the way back to [`Parallelism::default`], the state
/// that says nobody asked for a budget at all. A `Workers` arrangement has
/// nowhere to record that, so it carries the constant bare.
#[test]
fn nothing_discovered_and_nothing_recommended_is_todays_default() {
    let root = FakeRoot::new();
    root.v2("/leaf").write("proc/meminfo", "MemAvailable:   20002184 kB\n");
    assert_eq!(Parallelism::discover_in(root.path(), 1, None), Parallelism::default());
    assert_eq!(
        Parallelism::discover_in(root.path(), 8, None),
        Parallelism::workers(8, DEFAULT_MEMORY_BUDGET)
    );
}

/// **The closed form agrees with a search, over the whole grid.**
/// [`WorkerMemory::affords`] answers in one division above `pool_depth`
/// and by enumeration below it, which is two pieces of arithmetic where
/// the question is one — and the boundary between them is exactly where a
/// shape with a pool term stops being `n × (per_worker + pool_unit)`. So
/// it is checked against the definition it is an optimisation of: the
/// largest `n` in `1..=most` with `at(n) <= cap`, never zero.
#[test]
fn the_afforded_count_is_the_largest_one_the_cap_holds() {
    let shapes = [
        WorkerMemory::default(),
        WorkerMemory::per_worker(0).pooling(24 << 20, POOL_DEPTH),
        WorkerMemory::per_worker(34 << 20),
        WorkerMemory::per_worker(34 << 20).pooling(24 << 20, POOL_DEPTH),
        WorkerMemory::per_worker(138 << 20).pooling(128 << 20, POOL_DEPTH),
        WorkerMemory::per_worker(1).pooling(1, 1),
        WorkerMemory::per_worker(7).pooling(3, 9),
    ];
    for memory in shapes {
        for cap in [0u64, 1, 3, 7, 16, 100, 24 << 20, 128 << 20, 1 << 30, u64::MAX] {
            for most in [1usize, 2, 4, 5, 11, 64] {
                let want = (1..=most).rev().find(|n| memory.at(*n) <= cap).unwrap_or(1);
                assert_eq!(memory.affords(cap, most), want, "{memory:?} at cap {cap} most {most}");
            }
        }
    }
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
/// `crate::io::worker_count` solves the budget that produced against
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

/// A guard that records its own drop, so a test can assert that a future
/// given up on was released rather than merely left unpolled.
struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// The signal reports a cancellation that landed **before** anyone waited
/// on it: a reader that reaches its await after the ask must not wait for
/// a second one that will never come.
#[tokio::test]
async fn the_signal_resolves_at_once_for_a_cancellation_already_asked_for() {
    let cancel = Cancellation::new();
    assert!(!cancel.is_cancelled());
    cancel.cancel();
    assert!(cancel.is_cancelled());
    cancel.cancelled().await;
}

/// And it reports one that lands **while** a waiter is registered, which
/// is the ordinary case: the waiter is parked before `cancel` is called.
#[tokio::test]
async fn a_registered_waiter_is_woken_by_a_later_cancel() {
    let cancel = Cancellation::new();
    tokio::join!(cancel.cancelled(), async {
        assert!(!cancel.is_cancelled(), "the waiter parks before the ask");
        cancel.cancel();
    });
}

/// `cancel` is idempotent, and the polled bit is one-way.
#[tokio::test]
async fn cancelling_twice_says_the_same_thing() {
    let cancel = Cancellation::new();
    cancel.cancel();
    cancel.cancel();
    assert!(cancel.is_cancelled());
    cancel.cancelled().await;
}

/// Work that finishes before anyone cancels is handed back whole.
#[tokio::test]
async fn until_cancelled_yields_work_that_finished_first() {
    let cancel = Cancellation::new();
    assert_eq!(cancel.until_cancelled(async { 7u8 }).await, Some(7));
    assert!(!cancel.is_cancelled(), "asking nothing cancels nothing");
}

/// **The point of the awaitable form.** Work that never finishes is given
/// up on at the cancellation *and dropped there* — which is what lets a
/// reader release a request in flight instead of waiting out whatever
/// deadline sits under it.
#[tokio::test]
async fn until_cancelled_drops_the_work_it_gave_up_on() {
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = DropFlag(Arc::clone(&dropped));
    let forever = async move {
        let _guard = guard;
        std::future::pending::<u8>().await
    };

    let cancel = Cancellation::new();
    let (given_up, ()) = tokio::join!(cancel.until_cancelled(forever), async {
        assert!(!dropped.load(Ordering::SeqCst), "still in flight before the ask");
        cancel.cancel();
    });

    assert_eq!(given_up, None, "cancellation, not an answer");
    assert!(dropped.load(Ordering::SeqCst), "the request in flight was released");
}

/// The two properties every cut has to have, whatever the source advised:
/// the pieces **tile** the range exactly, in ascending order, and there
/// are never more of them than the caller asked for.
fn assert_tiles(range: Range<u64>, pieces: &[Range<u64>], want: usize) {
    assert!(!pieces.is_empty(), "a cut always yields at least the range itself");
    assert!(pieces.len() <= want.max(1), "{} pieces for want {want}", pieces.len());
    assert_eq!(pieces[0].start, range.start);
    assert_eq!(pieces[pieces.len() - 1].end, range.end);
    for pair in pieces.windows(2) {
        assert_eq!(pair[0].end, pair[1].start, "{pieces:?}");
    }
}

/// `Anywhere` is cut evenly, and never into pieces of zero bytes: a range
/// shorter than the worker count is cut into one piece per byte and no
/// finer, a piece with no bytes being able to own no rows.
#[test]
fn an_anywhere_source_is_cut_evenly_and_never_below_a_byte() {
    let advice = Partitioning::anywhere(1 << 20);
    for want in [1usize, 2, 3, 4, 7, 8, 64] {
        let pieces = cut(100..900, &advice, want);
        assert_tiles(100..900, &pieces, want);
        assert_eq!(pieces.len(), want.max(1));
    }
    let pieces = cut(10..13, &advice, 8);
    assert_eq!(pieces, vec![10..11, 11..12, 12..13]);
}

/// `At` is cut only where the source offered, thinned evenly when it
/// offers more boundaries than the caller can use, and taken whole when it
/// offers exactly as many as are wanted.
#[test]
fn an_at_source_is_cut_only_where_it_offered() {
    let advice = Partitioning::at(vec![20, 40, 60, 80], 1 << 20);
    assert_eq!(cut(0..100, &advice, 5), vec![0..20, 20..40, 40..60, 60..80, 80..100]);
    assert_eq!(cut(0..100, &advice, 9), vec![0..20, 20..40, 40..60, 60..80, 80..100]);
    assert_eq!(cut(0..100, &advice, 3), vec![0..40, 40..80, 80..100]);
    assert_eq!(cut(0..100, &advice, 2), vec![0..60, 60..100]);
    // Boundaries outside the range are not cuts, nor are its own ends.
    assert_eq!(cut(40..80, &advice, 4), vec![40..60, 60..80]);
    for want in [1usize, 2, 3, 4, 5, 9] {
        assert_tiles(0..100, &cut(0..100, &advice, want), want);
    }
}

/// A source that declines to be split is not split, however many workers
/// the caller has — the empty `At` is a policy, not an absence
/// (`docs/design/decisions.md`, "D7").
#[test]
fn a_source_that_declines_to_be_split_is_not() {
    assert_eq!(cut(0..1000, &Partitioning::single(0), 16), vec![0..1000]);
}

/// The bytes bind as well as the count, and they bind on the number of
/// **sub-streams**: eight workers against a budget that affords two
/// partitions is two.
#[test]
fn a_stated_budget_caps_the_worker_count_below_the_stated_jobs() {
    let eight = Parallelism::workers(8, 64 << 20);
    let per = WorkerMemory::per_worker;
    assert_eq!(worker_count(eight, per(32 << 20)), 2);
    assert_eq!(worker_count(eight, per(4 << 20)), 8);
    // A footprint larger than the whole budget still leaves one worker.
    assert_eq!(worker_count(eight, per(128 << 20)), 1);
    // A source that states no footprint is bounded by `jobs` alone.
    assert_eq!(worker_count(eight, per(0)), 8);
    // The serial state is one worker, whatever budget it carries.
    assert_eq!(worker_count(Parallelism::default(), per(32 << 20)), 1);
    assert_eq!(worker_count(Parallelism::workers(1, 1 << 30), per(32 << 20)), 1);
}

/// **A shared pool is billed, so the budget is solved rather than
/// divided.** The block pool retains a unit for every slot but the one a
/// reader is filling — `POOL_DEPTH - 1` of them below four readers and
/// `jobs - 1` above — which a per-worker charge does not carry.
#[test]
fn a_shared_pool_is_billed_before_the_count_is_handed_back() {
    let eight = Parallelism::workers(8, 64 << 20);
    let charge = WorkerMemory::per_worker(16 << 20).pooling(16 << 20, 4);
    // One reader costs 16 + 3 x 16 = 64 MiB, two cost 32 + 48 = 80 — so
    // this budget affords exactly one where the division said four.
    assert_eq!(worker_count(eight, charge), 1);
    // One byte short, and the serial floor is what is left: never zero.
    assert_eq!(worker_count(Parallelism::workers(8, (64 << 20) - 1), charge), 1);
    // Above the pool's own depth every reader takes a slot with it, so the
    // cost is `n x 32 - 16` MiB: 240 MiB affords eight, and 224 seven.
    assert_eq!(worker_count(Parallelism::workers(8, 240 << 20), charge), 8);
    assert_eq!(worker_count(Parallelism::workers(8, 224 << 20), charge), 7);
}
