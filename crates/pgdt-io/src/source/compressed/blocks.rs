//! The decoded-block cache and the decode budget every compressed source
//! reads through: a block decoded once into a pooled slot, sliced zero-copy or
//! copied into a read straddling it, what a decoder may hold beside it, and
//! how a range may be cut given which of the two a read takes.

use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use bytes::Bytes;
use tokio::sync::Notify;

use pgdt_core::Result;

use crate::memory::DEFAULT_MEMORY_BUDGET;
use crate::parallelism::{Parallelism, WorkerMemory};
use crate::partition::{PartitionRead, Partitioning, RetainedUnit};
use crate::source::SCAN_CHUNK_DEFAULT_SIZE_BYTES;
use crate::source::pool::{BufferPool, POOL_DEPTH, PooledBuffer, budget_bytes};

use super::{Codec, piece_of, short_read};

/// One block's plaintext, decoded whole into a pooled slot.
///
/// The slot returns to its pool when the last [`Bytes`](bytes::Bytes) viewing it drops
/// together with the retention list's own reference, which is what lets a
/// read inside this block be a zero-copy slice rather than a copy
/// (`docs/design/decisions.md`, "D15").
pub(super) struct DecodedBlock {
    /// A slot sized to the file's *largest* block, so every block fits every
    /// slot; `len` is what this block actually holds.
    pub(super) slot: PooledBuffer,
    pub(super) len: usize,
}

impl DecodedBlock {
    /// This block's plaintext — the slot cut down to what the block holds.
    fn bytes(&self) -> &[u8] {
        &self.slot.as_ref()[..self.len]
    }

    /// The `len` bytes at uncompressed `offset`, a read lying inside this
    /// block, whose first byte is at `base`: a **slice of the block**, so the
    /// common case copies nothing.
    ///
    /// The read's covering block is the one at or below `offset`, so the
    /// checked subtraction is what keeps an index that says otherwise a refusal
    /// rather than a panic.
    pub(super) fn slice(self: Arc<Self>, base: u64, offset: u64, len: usize) -> Result<Bytes> {
        let start = offset
            .checked_sub(base)
            .and_then(|at| usize::try_from(at).ok())
            .ok_or_else(short_read)?;
        if start + len > self.len {
            return Err(short_read());
        }
        Ok(Bytes::from_owner(BlockView(self)).slice(start..start + len))
    }

    /// Copy what this block, whose first byte is at `base`, answers of the
    /// read `offset..end` into `out`, the read's own buffer, and say how many
    /// bytes that was — none where the two do not overlap. A read straddling
    /// a boundary is assembled from one call a covering block.
    pub(super) fn copy_piece(&self, base: u64, offset: u64, end: u64, out: &mut [u8]) -> usize {
        let Some((at, from, n)) = piece_of(base..base + self.len as u64, offset, end) else {
            return 0;
        };
        let within = (from - base) as usize;
        out[at..at + n].copy_from_slice(&self.bytes()[within..within + n]);
        n
    }
}

/// The owner behind a [`Bytes`] sliced out of a decoded block: a share of the
/// block, so the slot outlives every view into it.
struct BlockView(Arc<DecodedBlock>);

impl AsRef<[u8]> for BlockView {
    fn as_ref(&self) -> &[u8] {
        self.0.bytes()
    }
}

/// The block unit's pool, and the decoded blocks it is currently holding.
///
/// **This is the second pool a source with two read units takes**, not a
/// second unit announced into the chunk pool: [`BufferPool::hinted`] is one
/// value driving both [`BufferPool::keeps`] and [`BufferPool::slot_bytes`], so
/// one pool serving both units is wrong for one of them either way
/// (`docs/design/decisions.md`, "D17").
///
/// **Retention is what makes per-call block decode affordable**: a block
/// decoded afresh per call would repeat the whole block's decode once per
/// chunk inside it, where a retained block makes the next read a slice. The
/// retained set is drained to [`BufferPool::slots`] less one before each
/// decode takes its slot.
///
/// **A retained block occupies a slot rather than adding to the free list**,
/// eviction happening *before* a slot is taken ([`BlockCache::slot`]) so the
/// evicted buffer is what the next decode reuses. **The retained list and the
/// free list share one count** — this list reserves what it holds
/// ([`BufferPool::reserve`]) — so one reader's ceiling is `slots * unit`, and
/// `workers` readers decoding at once hold up to `slots − 1 + workers` units
/// (`docs/design/decisions.md`, "D17").
///
/// **A block is decoded once however many readers miss on it together**
/// ([`BlockCache::claim`]): a fused worker's tail read lands in the block its
/// successor owns, so on the parallel path two misses on one block are the
/// common case rather than a race.
///
/// *Rejected: retaining one block on the serial path.* A cap of one makes
/// [`BlockCache::slot`] keep zero and drain before every decode, so any
/// outstanding view forces a fresh allocation of the block unit.
pub(crate) struct BlockCache {
    /// Slot size: the file's largest unit. Every slot fits every unit, a
    /// decode being handed its unit's own length of it, and it is what
    /// the pool is hinted with so a slot survives [`POOL_MAX_BYTES`](crate::source::pool::POOL_MAX_BYTES).
    pub(super) unit: usize,
    pub(super) pool: Arc<BufferPool>,
    /// The retained blocks and the decodes in flight, under **one lock**.
    pub(super) retention: Mutex<Retention>,
}

/// What [`BlockCache`] holds behind its lock.
///
/// **The decodes in flight sit under the retention list's own mutex**, so
/// the one acquisition a lookup always took answers all three cases of
/// [`BlockCache::claim`]: a read colliding with no other reader pays nothing
/// for the case where one does. *Rejected:* a map of its own, which puts a
/// second lock in front of every miss.
#[derive(Default)]
pub(super) struct Retention {
    /// Retained blocks by index, least-recently-used first.
    pub(super) blocks: Vec<(usize, Arc<DecodedBlock>)>,
    /// The blocks a reader is decoding now, one flight each. Never an index
    /// [`Retention::blocks`] also holds: a claim decodes only what neither
    /// list has, and its landing moves it from this list to that one.
    decoding: Vec<(usize, Arc<Flight>)>,
}

/// What [`BlockCache::claim`] answers for one block.
pub(super) enum Claim<'a> {
    /// Retained, and promoted to most-recently-used.
    Hit(Arc<DecodedBlock>),
    /// Another reader is decoding it. Wait on the flight rather than decode
    /// it a second time, and claim again if it comes down empty.
    Wait(Arc<Flight>),
    /// Neither retained nor in flight: the caller decodes it, into
    /// [`BlockCache::slot`], and lands it.
    Decode(Decoding<'a>),
}

/// One block a reader is decoding, which every other miss on it waits for.
///
/// It comes down holding the block, or **empty** where its decoder failed or
/// was dropped — a fetched read's future cancelled mid-fetch included — and
/// a waiter that sees it empty claims again, so one of them decodes. **An
/// error is not handed to the waiters**: the failure is the decoder's, and a
/// waiter that fails the same way does so on a decode of its own, which keeps
/// the error unshared and the block never half-landed.
///
/// **Two ways to wait, because the two sources wait differently**: the local
/// one blocks the `spawn_blocking` thread its read already runs on
/// ([`Flight::wait`]), the fetched one awaits ([`Flight::landed`]). A landing
/// wakes both.
pub(super) struct Flight {
    state: Mutex<Landing>,
    blocking: Condvar,
    awaiting: Notify,
}

enum Landing {
    Decoding,
    Landed(Arc<DecodedBlock>),
    Abandoned,
}

impl Flight {
    fn new() -> Self {
        Self {
            state: Mutex::new(Landing::Decoding),
            blocking: Condvar::new(),
            awaiting: Notify::new(),
        }
    }

    /// Bring it down, holding `block` or empty, and wake every waiter.
    fn land(&self, block: Option<Arc<DecodedBlock>>) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = match block {
            Some(block) => Landing::Landed(block),
            None => Landing::Abandoned,
        };
        self.blocking.notify_all();
        self.awaiting.notify_waiters();
    }

    /// `None` while the decode is running; once down, the block or `None`.
    fn outcome(&self) -> Option<Option<Arc<DecodedBlock>>> {
        match &*self.state.lock().unwrap_or_else(|e| e.into_inner()) {
            Landing::Decoding => None,
            Landing::Landed(block) => Some(Some(Arc::clone(block))),
            Landing::Abandoned => Some(None),
        }
    }

    /// The block once it is down, blocking this thread until then; `None`
    /// where it came down empty and the caller claims again.
    pub(super) fn wait(&self) -> Option<Arc<DecodedBlock>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match &*state {
                Landing::Decoding => {
                    state = self.blocking.wait(state).unwrap_or_else(|e| e.into_inner());
                }
                Landing::Landed(block) => return Some(Arc::clone(block)),
                Landing::Abandoned => return None,
            }
        }
    }

    /// [`Flight::wait`], awaited. The notification is enabled *before* the
    /// state is read, so a landing between the two is not missed.
    pub(super) async fn landed(&self) -> Option<Arc<DecodedBlock>> {
        loop {
            let notified = self.awaiting.notified();
            let mut notified = std::pin::pin!(notified);
            notified.as_mut().enable();
            if let Some(outcome) = self.outcome() {
                return outcome;
            }
            notified.await;
        }
    }
}

/// The claim to decode one block ([`Claim::Decode`]). It is landed with the
/// block ([`Decoding::land`]); dropped without landing — a failed decode, a
/// cancelled future — it brings its flight down empty, so no waiter is left
/// on a decode nobody is running.
pub(super) struct Decoding<'a> {
    cache: &'a BlockCache,
    index: usize,
    flight: Option<Arc<Flight>>,
}

impl Decoding<'_> {
    /// Retain `block` as most-recently-used and hand it to every reader
    /// waiting on it. [`BlockCache::slot`] has already made room, so nothing
    /// is evicted here.
    pub(super) fn land(mut self, block: DecodedBlock) -> Arc<DecodedBlock> {
        let block = Arc::new(block);
        self.settle(Some(Arc::clone(&block)));
        block
    }

    fn settle(&mut self, block: Option<Arc<DecodedBlock>>) {
        let Some(flight) = self.flight.take() else {
            return;
        };
        {
            let mut retention = self.cache.retention.lock().unwrap_or_else(|e| e.into_inner());
            retention.decoding.retain(|(i, _)| *i != self.index);
            if let Some(block) = &block {
                retention.blocks.push((self.index, Arc::clone(block)));
                self.cache.pool.reserve(retention.blocks.len());
            }
        }
        flight.land(block);
    }
}

impl Drop for Decoding<'_> {
    fn drop(&mut self) {
        self.settle(None);
    }
}

impl BlockCache {
    /// The cache for a file whose largest unit is `unit` plaintext bytes, or
    /// `None` where there is no block unit to build
    /// one around: a file with no blocks at all, or a block longer than a
    /// length on this target.
    ///
    /// **Whether the unit is *affordable* is a separate question and a later
    /// one** ([`BlockCache::affordable`]): the answer depends on a budget the
    /// caller has not stated yet at construction.
    pub(super) fn for_unit(unit: u64) -> Option<BlockCache> {
        if unit == 0 {
            return None;
        }
        let unit = usize::try_from(unit).ok()?;
        let pool = Arc::new(BufferPool::default());
        pool.hint(unit);
        Some(BlockCache { unit, pool, retention: Mutex::new(Retention::default()) })
    }

    /// What **one concurrent reader** of this file holds while it decodes
    /// whole blocks, in bytes: one block unit, the chunk buffer a straddling
    /// read is assembled into, and the decoder's own retention
    /// (`docs/design/decisions.md`, "D15").
    ///
    /// **One unit, because the block a reader decodes *becomes* the block it
    /// retains**: [`BlockCache::slot`] drains the retention list to
    /// `slots() - 1` before taking a slot, and [`Decoding::land`] pushes
    /// that same buffer's block onto the list it just drained. The rest of the
    /// list is `slots() - 1` units shared by every reader, and is
    /// [`WorkerMemory`]'s second term.
    ///
    /// It is one number with three consumers — [`BlockCache::affordable`],
    /// [`partition_advice`] and [`BlockCache::worker_memory`]. *Rejected:*
    /// two statements of the one cost, which is how a divisor comes to charge
    /// nothing for the decoder.
    pub(super) fn reader_bytes(&self, chunk_bytes: u64, decode_bytes: u64) -> u64 {
        (self.unit as u64).saturating_add(chunk_bytes).saturating_add(decode_bytes)
    }

    /// What **the block path costs at `workers` concurrent readers**: the
    /// per-reader charge above, plus the retention list those readers share
    /// ([`WorkerMemory`]).
    ///
    /// **The shared term is `(POOL_DEPTH.max(workers) − 1) × unit`, and it is
    /// arithmetic from this pool rather than a measured term**: the free list
    /// and the retention list share the pool's slots
    /// ([`BufferPool::reserve`]), and [`BlockCache::slot`] drains to
    /// `slots - 1` before obtaining the buffer [`Decoding::land`] then
    /// pushes onto that same list, so the pool holds `slots - 1 + workers`
    /// units while each worker holds one unit at a time — a reader waiting on
    /// another's decode holding none ([`BlockCache::claim`]). No test asserts
    /// that bound. *Rejected:* charging
    /// `2 × workers` plus a decaying floor
    /// (`docs/design/decisions.md`, "D4"; `parallel-peak-rss`).
    fn worker_memory(&self, chunk_bytes: u64, decode_bytes: u64) -> WorkerMemory {
        WorkerMemory::per_worker(self.reader_bytes(chunk_bytes, decode_bytes))
            .pooling(self.unit as u64, POOL_DEPTH)
    }

    /// Whether `budget` admits one such reader — the line that decides between
    /// block decode and the piecewise arm
    /// (`docs/design/decisions.md`, "D15").
    ///
    /// **It is asked of *one* reader and charges that reader's share of the
    /// retention list with it**: a pool serving a single reader still holds
    /// [`POOL_DEPTH`] slots, so admitting the path on the per-reader charge
    /// alone would allow `(POOL_DEPTH − 1)` units the caller never granted.
    /// The chunk and the decoder come off the top, both being paid on the
    /// streaming path too, so what a decline saves is the block slots.
    ///
    /// **It does not hold on the query path, and `KD23` is that defect.** A
    /// query partition is cut over a whole `CopyBlock` rather than through
    /// [`Partitioning::window_end`]'s window, so a held batch can pin several
    /// units where this charges one. The decline it draws on a large-block
    /// file is accepted rather than worked around: it is reported
    /// (`pgdt_query`'s `compressed_block_path_declined`), and a caller who
    /// wants the path back states a budget.
    ///
    /// *Rejected: a fixed refusal line beside a fixed budget*
    /// (`docs/design/decisions.md`, "D15"). Reading the line off the budget
    /// makes the stated number true: a unit the caller did not allow room for
    /// is never decoded whole.
    ///
    /// **What it declines is memory, not seekability**: a file written with
    /// large blocks (`xz -9 -T0`, or an explicit `--block-size`) is declined
    /// under the default budget and does have parallelism to lose, which
    /// `--memory` or `Parallelism::Workers` reverses.
    pub(super) fn affordable(&self, chunk_bytes: u64, decode_bytes: u64, budget: u64) -> bool {
        self.worker_memory(chunk_bytes, decode_bytes).at(1) <= budget
    }

    /// Block `index` retained, in flight, or this caller's to decode — one
    /// acquisition of the lock deciding which ([`Retention`]).
    pub(super) fn claim(&self, index: usize) -> Claim<'_> {
        let mut retention = self.retention.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(at) = retention.blocks.iter().position(|(i, _)| *i == index) {
            let entry = retention.blocks.remove(at);
            let block = Arc::clone(&entry.1);
            retention.blocks.push(entry);
            return Claim::Hit(block);
        }
        if let Some((_, flight)) = retention.decoding.iter().find(|(i, _)| *i == index) {
            return Claim::Wait(Arc::clone(flight));
        }
        let flight = Arc::new(Flight::new());
        retention.decoding.push((index, Arc::clone(&flight)));
        Claim::Decode(Decoding { cache: self, index, flight: Some(flight) })
    }

    /// A slot to decode into, after making room for it: least-recently-used
    /// blocks are dropped down to one below the slot count *first*, so the
    /// buffer this take reuses is usually the one that eviction just released.
    ///
    /// **Eviction before acquisition is a reuse rule, not a progress
    /// guarantee**: a retained block is normally also the block some reader is
    /// holding a view into, so the drain may free no slot at all. That is why
    /// the block pool is never granted a wait
    /// (`CompressedFileSource::hint_wait_policy`, and `docs/design/decisions.md`, "D5").
    /// A block evicted while a [`Bytes`](bytes::Bytes) still views it stays alive until that
    /// view drops.
    ///
    /// **The reservation is lowered before the evicted blocks drop**, and the
    /// order is load-bearing: a release seeing the pre-eviction reservation
    /// would find no room and discard the very buffer this take is about to
    /// reuse.
    pub(super) fn slot(&self) -> PooledBuffer {
        let keep = self.pool.slots().saturating_sub(1);
        let evicted = {
            let mut retention = self.retention.lock().unwrap_or_else(|e| e.into_inner());
            let over = retention.blocks.len().saturating_sub(keep);
            let evicted: Vec<_> = retention.blocks.drain(..over).collect();
            self.pool.reserve(retention.blocks.len());
            evicted
        };
        drop(evicted);
        self.pool.obtain(self.unit)
    }
}

/// **The budget policy both compressed arms hold**: what the caller announced,
/// the two pools that one number is divided between, and the answers derived
/// from it — which arm a read takes ([`DecodeBudget::block_path`]), what a reader
/// of this file costs ([`DecodeBudget::block_worker_memory`]), and how a range may
/// be cut ([`DecodeBudget::partitions`]).
///
/// **One value both sources hold, never a branch on the provider.** Nothing
/// this policy *answers* differs by transport, which reaches it as the
/// `decode_bytes` its constructor is handed and as the chunk pool's wait
/// policy, which each source announces for itself; two copies of one charge
/// policy stay right exactly until one of them is edited
/// (`docs/design/decisions.md`, "D15").
pub(crate) struct DecodeBudget {
    /// The chunk unit: what a read spanning more than one block is assembled
    /// into, and what a piecewise read fills.
    pub(super) pool: Arc<BufferPool>,
    /// The block unit, or `None` where this file has no blocks to decode.
    /// Present does not mean *taken*: [`DecodeBudget::block_path`] decides per
    /// read, a block the caller's budget cannot hold being streamed.
    pub(super) blocks: Option<Arc<BlockCache>>,
    /// What one decode of this file retains beyond the slot it writes into:
    /// the backend's own state, and whatever the *provider* holds while it
    /// decodes.
    ///
    /// **It is the one term the transport moves**, so it is stated by whoever
    /// builds this policy rather than derived here — [`CompressedFileSource::over`](crate::CompressedFileSource::over) pays
    /// for a compressed input chunk its source does not lend,
    /// [`FetchedCompressedSource::assembled`](crate::FetchedCompressedSource::assembled) for the window its source lends out of.
    ///
    /// **Read once, at construction**: it is a property of the file and of the
    /// backend, not of a range.
    pub(super) decode_bytes: u64,
    /// What the caller stated it may hold, in bytes, across **both** pools —
    /// [`DEFAULT_MEMORY_BUDGET`] until one is announced.
    ///
    /// Held here rather than pushed straight into the pools: the split is
    /// derived from it and from the announced chunk size, which arrive in
    /// either order, and [`DecodeBudget::apportion`] recomputes from this on both
    /// announcements.
    budget: AtomicUsize,
    /// The worker ceiling the caller stated — 1 until one is announced. It is
    /// the **block** pool's depth: one retained block per concurrent reader,
    /// where a chunk buffer is taken by one read and returned when its bytes
    /// are dropped, and wants the replay path's depth instead
    /// (`LocalFileSource::hint_parallelism`).
    jobs: AtomicUsize,
}

impl DecodeBudget {
    /// The policy for a file whose largest unit is `largest_unit` plaintext
    /// bytes, over the `decode_bytes` its
    /// provider states.
    ///
    /// The block unit is sized from that unit or refused
    /// ([`BlockCache::for_unit`]), and the budget is divided before this
    /// returns: a source is readable before anything is announced to it, so
    /// the two pools are split at construction rather than at the first hint.
    pub(super) fn over(largest_unit: u64, decode_bytes: u64) -> Self {
        let budget = Self {
            pool: Arc::new(BufferPool::default()),
            blocks: BlockCache::for_unit(largest_unit).map(Arc::new),
            decode_bytes,
            budget: AtomicUsize::new(DEFAULT_MEMORY_BUDGET as usize),
            jobs: AtomicUsize::new(1),
        };
        budget.apportion();
        budget
    }

    /// Divide the stated budget between the two pools, **chunks first**.
    ///
    /// One stated number bounds the source, not each of its pools, so the
    /// block pool is given what is left after the chunk pool's own ceiling
    /// (`BufferPool::held_bytes`), its slot count at its slot size — up to
    /// [`POOL_DEPTH`] slots, fewer where the budget affords fewer. Chunks
    /// come first because that pool is the one every read path uses and the
    /// one the pool-miss cost was measured through (`chunk-size`)
    /// (`docs/design/decisions.md`, "D17").
    ///
    /// Recomputed from `self.budget` on **both** announcements rather than
    /// composed incrementally, so [`DecodeBudget::hint_read_size`] and
    /// [`DecodeBudget::hint_parallelism`] may arrive in either order and a source
    /// that is never told either still holds a coherent split.
    fn apportion(&self) {
        let budget = self.budget.load(Ordering::Relaxed);
        let jobs = self.jobs.load(Ordering::Relaxed).max(1);
        self.pool.set_limits(budget, POOL_DEPTH);
        if let Some(blocks) = &self.blocks {
            // One retained block per concurrent reader, and never fewer than
            // the pool's own depth: a block pool of one drains before every
            // decode, so it stops pooling exactly when a caller is holding a
            // block. That argues for a shallower floor; this is
            // `POOL_DEPTH`'s, inherited.
            //
            // **`jobs` here is the count the caller *announced*, not the count
            // `worker_count` then delivers.** They agree under
            // discovery and diverge where a stated `--jobs` outruns a stated
            // budget (`docs/design/decisions.md`, "D17").
            //
            // Deficiency register: `deficiency: KD21` — the slot count is then
            // bounded by the pool's byte budget rather than by the delivered
            // count, and the lists fill to it while each delivered reader
            // decodes into a buffer besides, so the source holds more than the
            // stated budget. **(c) unowned**; closing it means telling the
            // pool the *delivered* count, or [`BufferPool::slots`] reserving
            // for the decodes in flight — either reworks a pool's sizing rule.
            blocks
                .pool
                .set_limits(budget.saturating_sub(self.pool.held_bytes()), POOL_DEPTH.max(jobs));
        }
    }

    /// The chunk slot every one of a source's charges is stated against: the
    /// length a read loop announced ([`ByteRangeSource::hint_read_size`](crate::ByteRangeSource::hint_read_size)), or
    /// [`crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES`] where none has yet.
    ///
    /// **The fallback is the chunk a scan settles at, not
    /// [`POOL_MAX_BYTES`](crate::source::pool::POOL_MAX_BYTES)**: an allowance is solved against this number by
    /// [`Parallelism::fit`] before the file is open, while the gate the count
    /// then meets ([`BlockCache::affordable`]) is compared at the steady-state
    /// slot (`docs/design/decisions.md`, "D4"). *Rejected:* moving the gate to
    /// the recommendation instead, which would decline the block path on files
    /// that fit.
    ///
    /// **It is charged once a reader, and the chunk pool's free list beside it
    /// is charged nowhere** (`docs/design/decisions.md`, "D4").
    // Deficiency register: `deficiency: KD24` — that free list is a different
    // population from the per-reader buffer this bills, so a stated budget is
    // short by `⌊budget/chunk⌋.clamp(1, POOL_DEPTH)` chunks: flat in the
    // count, since the depth is a constant, and linear in whatever chunk the
    // caller announced. It is never *above* the stated budget unless one
    // chunk is. **(c)
    // unowned**; closing it means a count-independent term, which
    // [`WorkerMemory`] does not have.
    fn charged_chunk_bytes(&self) -> u64 {
        match self.pool.announced_bytes() {
            Some(len) => len as u64,
            None => SCAN_CHUNK_DEFAULT_SIZE_BYTES as u64,
        }
    }

    /// What concurrent block-decoding readers of this file would cost, whether
    /// or not the budget admits one: [`BlockCache::worker_memory`] over this
    /// source's own chunk size ([`DecodeBudget::charged_chunk_bytes`]) and decoder
    /// charge — the per-reader charge and the retention list shared beside
    /// it, as one shape.
    ///
    /// It composes the recommendation's shape
    /// ([`ByteRangeSource::default_worker_memory`](crate::ByteRangeSource::default_worker_memory)); the gate
    /// ([`BlockCache::affordable`]) reads [`BlockCache::worker_memory`] itself
    /// and the advice ([`partition_advice`]) builds the same shape from
    /// [`BlockCache::reader_bytes`], which
    /// `the_recommendation_an_allowance_is_solved_against_is_the_scans_own_charge`
    /// holds alike.
    pub(super) fn block_worker_memory(&self) -> Option<WorkerMemory> {
        let cache = self.blocks.as_ref()?;
        Some(cache.worker_memory(self.charged_chunk_bytes(), self.decode_bytes))
    }

    /// The block-decode path, or `None` where a read goes through the
    /// piecewise arm: no blocks at all, or a reader the caller's budget cannot
    /// afford ([`BlockCache::affordable`]).
    ///
    /// **The piecewise arm is not free of the budget either** — it holds the
    /// decoder, whatever its provider decodes out of, and the caller's chunk,
    /// which is [`BlockCache::reader_bytes`] less the block unit. So what
    /// declining saves is the plaintext, and a budget below even that
    /// remainder is exceeded rather than honoured.
    pub(super) fn block_path(&self) -> Option<&Arc<BlockCache>> {
        let budget = self.budget.load(Ordering::Relaxed) as u64;
        let chunk_bytes = self.charged_chunk_bytes();
        self.blocks
            .as_ref()
            .filter(|cache| cache.affordable(chunk_bytes, self.decode_bytes, budget))
    }

    /// [`DecodeBudget::block_worker_memory`] evaluated at one reader: one
    /// statement of what the block path costs
    /// ([`ByteRangeSource::block_decode_bytes`](crate::ByteRangeSource::block_decode_bytes)).
    pub(super) fn block_decode_bytes(&self) -> Option<u64> {
        Some(self.block_worker_memory()?.at(1))
    }

    /// How a range over `codec`'s file may be cut, read off the arm a read
    /// would actually take ([`partition_advice`]).
    ///
    /// The codec is passed in rather than held: the cut points are a property
    /// of the file, which each source already holds, and what this policy
    /// decides is which arm they are priced for.
    pub(super) fn partitions(&self, codec: &dyn Codec, range: Range<u64>) -> Partitioning {
        partition_advice(
            || unit_starts_inside(codec, range),
            self.block_path().map(|cache| &**cache),
            self.charged_chunk_bytes(),
            self.decode_bytes,
        )
    }

    /// The chunk unit is announced to the chunk pool, and the split between
    /// the two pools is re-derived: the chunk pool's share is its own ceiling,
    /// which this number is what sizes ([`DecodeBudget::apportion`]).
    pub(super) fn hint_read_size(&self, len: usize) {
        self.pool.hint(len);
        self.apportion();
    }

    /// **One stated budget, divided between two read units.** The chunk pool
    /// takes its own ceiling and the block pool takes the rest, so the number
    /// a caller states bounds the source rather than each of its pools; the
    /// worker count is the block pool's depth, one retained block per
    /// concurrent reader (`docs/design/decisions.md`, "D17").
    ///
    /// A budget too small for a whole block sends every read through the
    /// piecewise arm ([`BlockCache::affordable`]) — the budget is honoured
    /// rather than exceeded by a slot the pool floors at one.
    pub(super) fn hint_parallelism(&self, parallelism: Parallelism) {
        self.budget.store(budget_bytes(parallelism), Ordering::Relaxed);
        self.jobs.store(parallelism.jobs(), Ordering::Relaxed);
        self.apportion();
    }
}

/// The plaintext offsets at which a unit of `codec`'s file starts strictly
/// inside `range`: where a block-decoding partition may be cut.
fn unit_starts_inside(codec: &dyn Codec, range: Range<u64>) -> Vec<u64> {
    codec
        .units_in(range.clone())
        .filter_map(|unit| codec.plaintext(unit))
        .map(|unit| unit.start)
        .filter(|start| *start > range.start && *start < range.end)
        .collect()
}

/// A compressed source's partitioning advice, read off the **read path it
/// took** rather than off its index.
///
/// Two answers, and which one applies is `blocks`:
///
/// **Block-decoding** — the boundaries are the unit starts `cuts` answers,
/// so a partition is a whole number of units. A partition costs
/// what one concurrent reader holds — [`BlockCache::reader_bytes`], the
/// per-reader term of the [`BlockCache::worker_memory`] whose `at(1)`
/// [`BlockCache::affordable`] compares a budget against.
///
/// **Piecewise** — one partition, whatever the index says: one live
/// [`UnitRead`](super::UnitRead) goes forward only, so two workers on
/// different partitions would each force the other's unit to be left and
/// begun again. That is true of both providers, each holding one handle
/// behind one mutex ([`LiveBlock`](super::LiveBlock), and
/// `docs/design/decisions.md`, "D15").
///
/// Taken as a free function over the pieces of state it reads, the cut
/// points asked for only on the arm that cuts, so the piecewise arm is
/// assertable against a file that *does* have units.
fn partition_advice(
    cuts: impl FnOnce() -> Vec<u64>,
    blocks: Option<&BlockCache>,
    chunk_bytes: u64,
    decode_bytes: u64,
) -> Partitioning {
    let Some(cache) = blocks else {
        // **The piecewise arm charges the chunk buffer and not the
        // decoder**: on both providers the decoder is a fixed cost of the
        // source rather than of a concurrent reader
        // ([`Partitioning::partition_bytes`]).
        //
        // Deficiency register: `deficiency: KD35` — one partition is not
        // what the budget affords: a decoder retains far less than a decoded
        // block, so several would fit where the blocks they decode do not,
        // and the budget that sends a file down this arm is exactly the one
        // that would benefit. **What pins the count at one is not the
        // budget**: [`LiveBlock`] is forward-only behind one mutex on both
        // providers, so two readers would force each other's restarts. The
        // fetched arm's statelessness would have let it serve N partitions
        // for want of cut points alone, and was spent to stop it re-decoding
        // a block a scan sits inside ([`HeldBlock`]) — this is where that
        // option went. **(c) unowned**;
        // closing it means a handle per reader — which a `UnitRead` being
        // `Send` and borrowing nothing already allows — and a figure
        // over the count, and the figure is the half this cannot skip.
        return Partitioning::single(chunk_bytes);
    };
    let at = cuts();
    // Three statements the piecewise arm above does not make, each
    // because a read inside a decoded block is a zero-copy slice of it:
    //
    // - **the retained unit is the partition, not the chunk**, so a caller
    //   adding its span allowance on top would count the partition's
    //   already-charged bytes twice ([`RetainedUnit`]);
    // - **a worker reads one of this source's units in one call**, which
    //   at the shipped cut width is the whole piece
    //   ([`PartitionRead::Whole`]). The unit is `cache.unit`, read off the
    //   cache rather than recomputed and non-zero by
    //   `BlockCache::for_unit`'s own guard;
    // - **the block pool's retention list is stated beside the per-reader
    //   charge**, which is what lets `worker_count` solve
    //   for a count rather than divide by one
    //   ([`BlockCache::worker_memory`]).
    Partitioning::at(at, cache.reader_bytes(chunk_bytes, decode_bytes))
        .pooling(cache.unit as u64, POOL_DEPTH)
        .retaining(RetainedUnit::Partition)
        .reading(PartitionRead::Whole { unit: cache.unit as u64 })
}

#[cfg(test)]
mod tests;
