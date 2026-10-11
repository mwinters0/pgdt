//! The buffer pool every source reads into — a source's chunk pool, and the
//! block pool a block-decoding source decodes into — and the byte budget a
//! caller's parallelism hands it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::memory::DEFAULT_MEMORY_BUDGET;
use crate::parallelism::Parallelism;

use super::WaitPolicy;

/// How deep a [`BufferPool`] would like to be, in slots.
///
/// A scan holds one chunk at a time, so one slot would serve it; the query
/// replay path retains chunks past the read that produced them
/// (`pgdt_query::batch::RetainedChunks`), so the buffer of chunk *N* can still be
/// alive when chunk *N+1* is read — and every chunk a batch's views point into
/// is alive until that batch flushes, which this depth does not cover (`KD54`).
///
/// **It is a wish, not the bound** — the stated budget is the bound, and the
/// two together are what make a slot able to hold a decoded xz block rather
/// than only a read chunk (`docs/design/decisions.md`, "D9").
///
/// **It is also the floor under a stated worker count**, not a second number
/// beside it: a block pool of one makes eviction drain before every decode and
/// stops pooling exactly when a caller is holding a block. Neither depth is
/// measured against what block retention buys a seeking query.
// Deficiency register: `deficiency: KD54` — a replay batch pins every chunk
// its rows came from, so a batch of rows wider than a chunk over `POOL_DEPTH`
// pins more chunks than the free list keeps: at its flush the rest are
// dropped ([`BufferPool::release`]), and the reads after it allocate
// `vec![0u8; len]` afresh ([`BufferPool::pick`]), the zeroing a `memset`
// under mimalloc. The costing input's profiles find a third of the off leg's
// user cycles there, in a provider scan
// (`docs/design/measurements.md`, "What DataFusion's dynamic filters buy a
// query"). **(c) unowned**; promoted by a figure pricing the zeroing or a
// phase taking up query-path memory, the fix being a depth that follows what
// a batch retains, which the budget then has to bill (`KD24`).
pub(crate) const POOL_DEPTH: usize = 4;

/// The largest buffer worth keeping, in bytes, for a length nobody has
/// announced as a read size.
///
/// The read path's steady state is chunk-sized — [`crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES`],
/// tunable through `ScanOptions::chunk_size_bytes`. What can be far larger is
/// `pgdt_dump::attach_text`'s coalesced span read, which happens once per map
/// and never again; holding one of those for the rest of a process would trade
/// what a scan holds resident (`peak-rss`) for an allocation nothing asks for
/// twice.
///
/// **Size stands in for one-off-ness, and the caller is what overrides it**:
/// [`ByteRangeSource::hint_read_size`](crate::ByteRangeSource::hint_read_size) names the length a read loop is about
/// to repeat, and *becomes* the pool's slot size
/// ([`BufferPool::slot_bytes`]), so a buffer of that length is kept however
/// large it is and this constant governs only a pool nobody has announced to
/// (`docs/design/decisions.md`, "D9").
pub(crate) const POOL_MAX_BYTES: usize = 8 << 20;

/// Read buffers, reused rather than allocated per chunk.
///
/// **What this is for is not allocator throughput.** A fresh `vec![0u8; len]`
/// per chunk is `calloc`, so the kernel or the allocator zeroes a chunk that
/// `read_exact_at` immediately overwrites (`chunk-size`), and hands the region
/// back once per chunk, driving `madvise` traffic (`allocator`). See
/// `docs/design/decisions.md`, "D10".
///
/// **A pooled buffer is fully initialized and stays at its own length.** It is
/// created once as `vec![0u8; len]` and thereafter only ever read into, so no
/// reuse memsets anything. A read shorter than the buffer takes the buffer
/// whole and the `Bytes` handed back is sliced down to what was read, so a
/// short final chunk does not shrink a pooled buffer.
///
/// **Whether an acquisition blocks is the caller's permission, not the pool's
/// policy.** [`BufferPool::obtain`] waits for a free slot where the read loop
/// granted [`WaitPolicy::MayWait`] and allocates past the budget where it
/// granted [`WaitPolicy::NeverWait`] (`docs/design/decisions.md`, "D5").
/// [`BufferPool::pick`] is the unwaiting primitive underneath both.
#[derive(Debug)]
pub(crate) struct BufferPool {
    /// The free list and the count of charged buffers outstanding, under
    /// one lock because [`BufferPool::returned`] is what a waiting acquisition
    /// blocks on and a condvar needs the state it waits on beside it.
    ///
    /// A poisoned lock is not a corruption hazard here — the only thing under
    /// it is a list of scratch buffers and a counter — so every caller
    /// recovers the guard rather than propagating a panic from an unrelated
    /// task.
    state: Mutex<PoolState>,
    /// Signalled whenever a charged buffer comes back, which is the only event
    /// that can let a waiting acquisition through.
    returned: Condvar,
    /// What this pool may hold in free buffers at once, in bytes: for a chunk
    /// pool [`DEFAULT_MEMORY_BUDGET`] until a caller states one through
    /// [`ByteRangeSource::hint_parallelism`](crate::ByteRangeSource::hint_parallelism), for a block pool what its
    /// source's chunk pool leaves of that (`XzBudget::apportion`).
    ///
    /// **The pool is bounded in bytes because a block pool cannot be bounded
    /// in slots** ([`POOL_DEPTH`] slots of a decoded xz block can be the whole
    /// of a small cgroup). The byte budget makes the count the consequence
    /// ([`BufferPool::slots`]) and the memory the number a caller states.
    ///
    /// **It bounds the free list at or below its own slot size, and not above
    /// it.** [`BufferPool::slots`] clamps to at least one, so a unit larger
    /// than the budget still gets a slot at that unit's size; what keeps that
    /// floor from making the budget a fiction is that a decoded xz block is
    /// refused *before* a slot is asked for ([`BlockCache::affordable`](crate::source::xz::blocks::BlockCache::affordable)).
    ///
    /// `Relaxed` for the same reason [`BufferPool::hinted`] is.
    budget: AtomicUsize,
    /// The ceiling on the free list in slots, whatever the budget affords:
    /// [`POOL_DEPTH`], which every chunk pool keeps whatever count is
    /// announced, and which [`XzBudget::apportion`](crate::source::xz::blocks::XzBudget::apportion) raises on the block pool
    /// alone to one slot per announced reader above it.
    depth: AtomicUsize,
    /// How many of [`BufferPool::slots`] are held by a holder outside the free
    /// list — `0` for every pool but [`BlockCache`](crate::source::xz::blocks::BlockCache)'s, whose retained blocks
    /// own slots of this pool's own unit ([`BufferPool::reserve`]).
    reserved: AtomicUsize,
    /// The read length a caller announced through
    /// [`ByteRangeSource::hint_read_size`](crate::ByteRangeSource::hint_read_size), or `0` for none — the pool's slot
    /// size, and a buffer of exactly this length is kept past
    /// [`POOL_MAX_BYTES`].
    ///
    /// It is an atomic rather than part of the `Mutex` because
    /// `hint_read_size` takes `&self`: a source is shared, and the
    /// announcement must not have to wait behind a `take` on another task.
    /// `Relaxed` is enough — nothing is published through it, and a hint that
    /// arrives a buffer late costs one allocation.
    ///
    /// **One value, so one pool describes one read unit.** It drives both
    /// [`BufferPool::keeps`] and [`BufferPool::slot_bytes`], and a pool asked
    /// to serve two units is wrong for one of them either way, so a source
    /// with a second read unit takes a second pool rather than a second hint
    /// (`docs/design/decisions.md`, "D17").
    hinted: AtomicUsize,
    /// What the read loop currently reading through this pool permits, as
    /// [`WaitPolicy`] encodes it — [`WaitPolicy::NeverWait`] until one is
    /// granted, so a pool nobody has spoken to never waits.
    ///
    /// `Relaxed` for the same reason [`BufferPool::hinted`] is. It is read
    /// *inside* the state lock by [`BufferPool::obtain`], so a policy that
    /// changes between the read and the wait cannot leave a charge unmatched —
    /// the charge is carried by the buffer, not re-derived at release.
    policy: AtomicUsize,
}

/// What a [`BufferPool`] holds under its lock.
#[derive(Debug, Default)]
struct PoolState {
    /// Buffers nobody is using. A release adds one only while fewer than
    /// [`BufferPool::free_slots`] are here, and nothing trims them, so a
    /// lowered limit leaves the list over it until acquisitions drain it.
    free: Vec<Vec<u8>>,
    /// Buffers handed to a [`WaitPolicy::MayWait`] loop and not yet
    /// returned — the term a waiting acquisition is bounded by, and the only
    /// one of the two the library sets. What in-flight batches pin is the
    /// caller's and is deliberately not counted here
    /// (`docs/design/decisions.md`, "D5").
    charged: usize,
}

impl Default for BufferPool {
    fn default() -> Self {
        Self {
            state: Mutex::new(PoolState::default()),
            returned: Condvar::new(),
            budget: AtomicUsize::new(DEFAULT_MEMORY_BUDGET as usize),
            depth: AtomicUsize::new(POOL_DEPTH),
            reserved: AtomicUsize::new(0),
            hinted: AtomicUsize::new(0),
            policy: AtomicUsize::new(WaitPolicy::NeverWait as usize),
        }
    }
}

impl BufferPool {
    /// The smallest free buffer that fits `len`, or a fresh allocation —
    /// taken under the guard [`BufferPool::obtain`] is already holding, so
    /// that a waiting acquisition's wait and its take are one critical
    /// section.
    fn pick(state: &mut PoolState, len: usize) -> Vec<u8> {
        let pick = state
            .free
            .iter()
            .enumerate()
            .filter(|(_, buf)| buf.len() >= len)
            .min_by_key(|(_, buf)| buf.len())
            .map(|(i, _)| i);
        match pick {
            Some(i) => state.free.swap_remove(i),
            None => vec![0u8; len],
        }
    }

    /// The buffer for one read, waiting or allocating according to what the
    /// read loop permitted.
    ///
    /// [`WaitPolicy::MayWait`] blocks while [`BufferPool::slots`] buffers
    /// taken under that permission are already out, so the pool's slot count
    /// bounds what is outstanding; the buffer carries the charge, so the
    /// matching release cannot be lost to a policy granted in between.
    /// [`WaitPolicy::NeverWait`] takes the same buffer without blocking and
    /// without charging.
    ///
    /// **The wait is safe only because a waiting loop holds one buffer** —
    /// the promise [`ByteRangeSource::hint_wait_policy`](crate::ByteRangeSource::hint_wait_policy) documents.
    pub(super) fn obtain(self: &Arc<Self>, len: usize) -> PooledBuffer {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let charged = self.policy() == WaitPolicy::MayWait;
        if charged {
            // Re-read the ceiling every pass: `set_limits` may raise it while
            // this holder waits, and a wait against a stale number would
            // outlast the condition that caused it.
            while state.charged >= self.slots() {
                state = self.returned.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            state.charged += 1;
        }
        let buf = Self::pick(&mut state, len);
        drop(state);
        PooledBuffer { buf: Some(buf), pool: Arc::clone(self), charged }
    }

    /// Announce the read length a loop is about to repeat, so a buffer of
    /// that length survives [`POOL_MAX_BYTES`] and the pool's slot size is
    /// the unit that loop retains.
    pub(super) fn hint(&self, len: usize) {
        self.hinted.store(len, Ordering::Relaxed);
    }

    /// The pool's slot size: the unit a read loop announced, or
    /// [`POOL_MAX_BYTES`] where nothing has been announced — which is the
    /// largest buffer [`BufferPool::keeps`] will take in that case, so the
    /// two rules bound the same thing.
    pub(super) fn slot_bytes(&self) -> usize {
        match self.hinted.load(Ordering::Relaxed) {
            0 => POOL_MAX_BYTES,
            len => len,
        }
    }

    /// The unit a read loop announced, or `None` where none has — the same
    /// state [`BufferPool::slot_bytes`] reads, with the absence still an
    /// absence.
    ///
    /// A caller pricing a *reader* and a pool sizing a *slot* want different
    /// answers to "nobody has said": the pool's own is [`POOL_MAX_BYTES`], and
    /// a charge divided into a memory allowance wants the chunk the scan is
    /// about to announce ([`XzBudget::charged_chunk_bytes`](crate::source::xz::blocks::XzBudget::charged_chunk_bytes)).
    pub(super) fn announced_bytes(&self) -> Option<usize> {
        match self.hinted.load(Ordering::Relaxed) {
            0 => None,
            len => Some(len),
        }
    }

    /// The budget this pool is sizing itself against, in bytes.
    fn budget(&self) -> usize {
        self.budget.load(Ordering::Relaxed)
    }

    /// What the loop currently reading through this pool permits.
    pub(super) fn policy(&self) -> WaitPolicy {
        match self.policy.load(Ordering::Relaxed) {
            n if n == WaitPolicy::MayWait as usize => WaitPolicy::MayWait,
            _ => WaitPolicy::NeverWait,
        }
    }

    /// State what the read loop about to start permits.
    ///
    /// It governs acquisitions made *after* it: a buffer already out keeps the
    /// charge it was taken with, which is what makes the two policies safe to
    /// alternate across a query's mapping and replay passes.
    pub(super) fn set_policy(&self, policy: WaitPolicy) {
        self.policy.store(policy as usize, Ordering::Relaxed);
    }

    /// State the budget and the slot ceiling this pool is to work inside.
    ///
    /// Both at once, because they are two halves of one sizing decision and a
    /// pool caught between an old depth and a new budget would report a count
    /// neither caller asked for.
    /// A raised ceiling wakes every waiter, since one of them may now fit
    /// under it — the only other thing that can release a waiting acquisition
    /// is a charged buffer coming back.
    pub(super) fn set_limits(&self, budget: usize, depth: usize) {
        self.budget.store(budget, Ordering::Relaxed);
        self.depth.store(depth.max(1), Ordering::Relaxed);
        self.returned.notify_all();
    }

    /// How many free buffers this pool holds: its depth where the budget
    /// affords it, fewer where a slot is large, never zero.
    ///
    /// **This is what makes the pool block-capable.** At the default chunk it
    /// is [`POOL_DEPTH`]; at a decoded xz block large enough that the depth
    /// would be the whole allowance, it is one.
    pub(super) fn slots(&self) -> usize {
        let depth = self.depth.load(Ordering::Relaxed).max(1);
        (self.budget() / self.slot_bytes()).clamp(1, depth)
    }

    /// How many of those slots the free list may take: the rest are held by a
    /// reserving holder that accounts for them itself
    /// ([`BufferPool::reserve`]).
    fn free_slots(&self) -> usize {
        self.slots().saturating_sub(self.reserved.load(Ordering::Relaxed))
    }

    /// The most this pool can hold at once, in bytes: its slot count at its
    /// own slot size, counting the free list and whatever a reserving holder
    /// retains **together**.
    ///
    /// **It is a true bound only because [`BufferPool::keeps`] refuses
    /// anything above a slot and [`BufferPool::reserve`] takes the retained
    /// blocks out of the free list's share — and only for one reader**: with
    /// several decoding at once the retention list outgrows its share by up to
    /// their number ([`BlockCache::worker_memory`](crate::source::xz::blocks::BlockCache::worker_memory)). This is what a source with
    /// **two** read units subtracts before handing the remainder to the second
    /// pool (`docs/design/decisions.md`, "D17").
    ///
    /// **It bounds the two lists and not a buffer a caller is still holding.**
    /// A [`PooledBuffer`] taken by [`BufferPool::obtain`] is on neither list
    /// until it is released, and a [`BlockCache`](crate::source::xz::blocks::BlockCache) block evicted while a
    /// [`Bytes`](bytes::Bytes) still views it is on neither list at all — so what this
    /// answers is the pool's own retention, the quantity
    /// [`XzBudget::apportion`](crate::source::xz::blocks::XzBudget::apportion) divides (`docs/design/decisions.md`, "D4").
    pub(super) fn held_bytes(&self) -> usize {
        self.slots().saturating_mul(self.slot_bytes())
    }

    /// Whether a released buffer is worth keeping: **anything that fits a
    /// slot**, which is the announced read length or [`POOL_MAX_BYTES`] where
    /// nothing has been announced ([`BufferPool::slot_bytes`]).
    ///
    /// **One rule, so that [`BufferPool::held_bytes`] is true.** A pool
    /// describes one read unit, and a buffer larger than that unit occupying a
    /// slot counted at the unit would make the free list hold
    /// `slots × POOL_MAX_BYTES` while `held_bytes` reported
    /// `slots × slot_bytes`.
    ///
    /// **What it costs is a second *pooled* read unit**, which the plain path
    /// does not have: it reads [`PartitionRead::Chunked`](crate::PartitionRead::Chunked), so every buffer its
    /// read loop takes is the announced length. It does take one other length
    /// — `pgdt_dump::attach_text`'s run read, which is bounded by the spans
    /// it is filling rather than by the hint — and this rule is exactly what
    /// drops that buffer instead of pooling it. The two-unit sources are the
    /// `.xz` ones, [`XzSource`](crate::XzSource) and `FetchedXzSource`, whose `XzBudget` takes
    /// a second pool rather than a second hint
    /// (`docs/design/decisions.md`, "D17").
    fn keeps(&self, len: usize) -> bool {
        len <= self.slot_bytes()
    }

    /// State how many slots a holder outside the free list is accounting for.
    ///
    /// The only one is [`BlockCache`](crate::source::xz::blocks::BlockCache)'s retained list, whose blocks hold
    /// [`PooledBuffer`]s of this pool's own slot size
    /// (`docs/design/decisions.md`, "D17").
    ///
    /// `Relaxed` for the same reason [`BufferPool::hinted`] is: nothing is
    /// published through it, and a value that arrives a buffer late costs one
    /// keep or one drop.
    pub(super) fn reserve(&self, slots: usize) {
        self.reserved.store(slots, Ordering::Relaxed);
    }

    /// Seed the free list with a buffer nobody took, which only a test has
    /// reason to do: every shipped path releases through [`PooledBuffer`],
    /// whose charge this shape cannot carry.
    #[cfg(test)]
    pub(super) fn give(&self, buf: Vec<u8>) {
        self.release(buf, false);
    }

    /// How many buffers are on the free list.
    #[cfg(test)]
    pub(super) fn free_len(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).free.len()
    }

    /// How many buffers taken under a granted wait are out — the term a
    /// waiting acquisition is bounded by.
    #[cfg(test)]
    pub(super) fn charged(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).charged
    }

    /// Return a buffer, discharging its slot where it was charged one.
    ///
    /// **The charge is discharged whether or not the buffer is kept**: the
    /// slot is free the moment its holder let go of it, and counting *kept*
    /// buffers would leak a slot per refused release.
    fn release(&self, buf: Vec<u8>, charged: bool) {
        let keeps = self.keeps(buf.len());
        let slots = self.free_slots();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if keeps && state.free.len() < slots {
            state.free.push(buf);
        }
        if charged {
            state.charged = state.charged.saturating_sub(1);
            drop(state);
            self.returned.notify_one();
        }
    }
}

/// The owner behind a [`BufferPool`]-backed [`Bytes`](bytes::Bytes): it returns the buffer
/// when the last reference to those bytes is dropped, which is what lets the
/// query path keep a chunk alive for as long as a zero-copy view points into
/// it and still get the buffer back afterwards.
pub(super) struct PooledBuffer {
    /// `None` only between [`Drop`] taking the buffer and the struct going
    /// away, which no `as_ref` can observe.
    buf: Option<Vec<u8>>,
    pool: Arc<BufferPool>,
    /// Whether this buffer counts against [`PoolState::charged`] — set by
    /// [`BufferPool::obtain`] from the [`WaitPolicy`] in force when it was
    /// taken, and carried here rather than re-derived at release so that a
    /// policy granted in between cannot leave the count wrong.
    charged: bool,
}

impl PooledBuffer {
    /// The bytes, for a caller that is about to fill them.
    pub(super) fn as_mut(&mut self) -> &mut [u8] {
        self.buf.as_deref_mut().unwrap_or(&mut [])
    }
}

impl AsRef<[u8]> for PooledBuffer {
    fn as_ref(&self) -> &[u8] {
        self.buf.as_deref().unwrap_or(&[])
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        if let Some(buf) = self.buf.take() {
            self.pool.release(buf, self.charged);
        }
    }
}

/// The byte budget a caller's [`Parallelism`] states, as a `usize`, falling
/// back to [`DEFAULT_MEMORY_BUDGET`] where it states none — which is
/// [`Parallelism::default`], and also a `u64` too large to be a length on this
/// target, where the default is the smaller and therefore the safe answer.
pub(super) fn budget_bytes(parallelism: Parallelism) -> usize {
    parallelism
        .memory_bytes()
        .and_then(|bytes| usize::try_from(bytes).ok())
        .unwrap_or(DEFAULT_MEMORY_BUDGET as usize)
}

#[cfg(test)]
mod tests;
