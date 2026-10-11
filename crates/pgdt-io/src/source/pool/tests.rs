use std::sync::Arc;

use crate::source::local::PLAIN_PARTITION_CHUNKS;

use super::*;

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
