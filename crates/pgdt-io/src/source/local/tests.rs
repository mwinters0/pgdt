use crate::test_support::*;
use crate::*;

use super::*;

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
