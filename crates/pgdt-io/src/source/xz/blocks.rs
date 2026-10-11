//! The decoded-block cache and the decode budget every `.xz` source reads
//! through: a block decoded once into a pooled slot, sliced zero-copy, what a
//! decoder may hold beside it, and how a range may be cut given which of the
//! two a read takes.

use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::memory::DEFAULT_MEMORY_BUDGET;
use crate::parallelism::{Parallelism, WorkerMemory};
use crate::partition::{PartitionRead, Partitioning, RetainedUnit};
use crate::source::SCAN_CHUNK_DEFAULT_SIZE_BYTES;
use crate::source::pool::{BufferPool, POOL_DEPTH, PooledBuffer, budget_bytes};

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
    pub(super) fn bytes(&self) -> &[u8] {
        &self.slot.as_ref()[..self.len]
    }
}

/// The owner behind a [`Bytes`](bytes::Bytes) sliced out of a decoded block: a share of the
/// block, so the slot outlives every view into it.
pub(super) struct BlockView(pub(super) Arc<DecodedBlock>);

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
/// *Rejected: retaining one block on the serial path.* A cap of one makes
/// [`BlockCache::slot`] keep zero and drain before every decode, so any
/// outstanding view forces a fresh allocation of the block unit.
pub(crate) struct BlockCache {
    /// Slot size: the file's largest block. Every slot fits every block, which
    /// is what `BlockTask::decode_into` is documented to allow, and it is what
    /// the pool is hinted with so a slot survives [`POOL_MAX_BYTES`](crate::source::pool::POOL_MAX_BYTES).
    pub(super) unit: usize,
    pub(super) pool: Arc<BufferPool>,
    /// Retained blocks by index, least-recently-used first.
    pub(super) retained: Mutex<Vec<(usize, Arc<DecodedBlock>)>>,
}

impl BlockCache {
    /// The cache for `table`, or `None` where there is no block unit to build
    /// one around: a file with no blocks at all, or a block longer than a
    /// length on this target.
    ///
    /// **Whether the unit is *affordable* is a separate question and a later
    /// one** ([`BlockCache::affordable`]): the answer depends on a budget the
    /// caller has not stated yet at construction.
    pub(super) fn for_table(table: &xz_seek::SeekTable) -> Option<BlockCache> {
        let unit = table.max_block_uncompressed();
        if unit == 0 {
            return None;
        }
        let unit = usize::try_from(unit).ok()?;
        let pool = Arc::new(BufferPool::default());
        pool.hint(unit);
        Some(BlockCache { unit, pool, retained: Mutex::new(Vec::new()) })
    }

    /// What **one concurrent reader** of this file holds while it decodes
    /// whole blocks, in bytes: one block unit, the chunk buffer a straddling
    /// read is assembled into, and the decoder's own retention
    /// (`docs/design/decisions.md`, "D15").
    ///
    /// **One unit, because the block a reader decodes *becomes* the block it
    /// retains**: [`BlockCache::slot`] drains the retention list to
    /// `slots() - 1` before taking a slot, and [`BlockCache::retain`] pushes
    /// that same buffer's block onto the list it just drained. The rest of the
    /// list is `slots() - 1` units shared by every reader, and is
    /// [`WorkerMemory`]'s second term.
    ///
    /// It is one number with three consumers — [`BlockCache::affordable`],
    /// [`xz_partition_advice`] and [`BlockCache::worker_memory`]. *Rejected:*
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
    /// `slots - 1` before obtaining the buffer [`BlockCache::retain`] then
    /// pushes onto that same list, so the pool holds `slots - 1 + workers`
    /// units while each worker holds one unit at a time. No test asserts that
    /// bound, and the case it has to survive is [`BlockCache::retain`]
    /// replacing an entry for the same block while the reader that decoded it
    /// still views it (`KD20`'s double decode). *Rejected:* charging
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
    /// (`pgdt_query::stream::compressed_block_path_declined`), and a caller who
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

    /// The retained block at `index`, promoted to most-recently-used.
    pub(super) fn lookup(&self, index: usize) -> Option<Arc<DecodedBlock>> {
        let mut retained = self.retained.lock().unwrap_or_else(|e| e.into_inner());
        let at = retained.iter().position(|(i, _)| *i == index)?;
        let entry = retained.remove(at);
        let block = Arc::clone(&entry.1);
        retained.push(entry);
        Some(block)
    }

    /// A slot to decode into, after making room for it: least-recently-used
    /// blocks are dropped down to one below the slot count *first*, so the
    /// buffer this take reuses is usually the one that eviction just released.
    ///
    /// **Eviction before acquisition is a reuse rule, not a progress
    /// guarantee**: a retained block is normally also the block some reader is
    /// holding a view into, so the drain may free no slot at all. That is why
    /// the block pool is never granted a wait
    /// (`XzSource::hint_wait_policy`, and `docs/design/decisions.md`, "D5").
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
            let mut retained = self.retained.lock().unwrap_or_else(|e| e.into_inner());
            let over = retained.len().saturating_sub(keep);
            let evicted: Vec<_> = retained.drain(..over).collect();
            self.pool.reserve(retained.len());
            evicted
        };
        drop(evicted);
        self.pool.obtain(self.unit)
    }

    /// Retain `block` as most-recently-used. [`BlockCache::slot`] has already
    /// made room, so nothing is evicted here.
    pub(super) fn retain(&self, index: usize, block: Arc<DecodedBlock>) {
        let mut retained = self.retained.lock().unwrap_or_else(|e| e.into_inner());
        retained.retain(|(i, _)| *i != index);
        retained.push((index, block));
        self.pool.reserve(retained.len());
    }
}

/// **The budget policy both `.xz` sources hold**: what the caller announced,
/// the two pools that one number is divided between, and the answers derived
/// from it — which arm a read takes ([`XzBudget::block_path`]), what a reader
/// of this file costs ([`XzBudget::block_worker_memory`]), and how a range may
/// be cut ([`XzBudget::partitions`]).
///
/// **One value both sources hold, never a branch on the provider.** Nothing
/// this policy *answers* differs by transport, which reaches it as the
/// `decode_bytes` its constructor is handed and as the chunk pool's wait
/// policy, which each source announces for itself; two copies of one charge
/// policy stay right exactly until one of them is edited
/// (`docs/design/decisions.md`, "D15").
pub(crate) struct XzBudget {
    /// The chunk unit: what a read spanning more than one block is assembled
    /// into, and what a piecewise read fills.
    pub(super) pool: Arc<BufferPool>,
    /// The block unit, or `None` where this file has no blocks to decode.
    /// Present does not mean *taken*: [`XzBudget::block_path`] decides per
    /// read, a block the caller's budget cannot hold being streamed.
    pub(super) blocks: Option<Arc<BlockCache>>,
    /// What one decode of this file retains beyond the slot it writes into:
    /// the backend's own state, and whatever the *provider* holds while it
    /// decodes.
    ///
    /// **It is the one term the transport moves**, so it is stated by whoever
    /// builds this policy rather than derived here — [`XzSource::over`](crate::XzSource::over) pays
    /// for a compressed input chunk its source does not lend,
    /// [`FetchedXzSource::assembled`](crate::FetchedXzSource::assembled) for the window its source lends out of.
    ///
    /// **Read once, at construction**: it is a property of the file and of the
    /// backend, not of a range.
    pub(super) decode_bytes: u64,
    /// What the caller stated it may hold, in bytes, across **both** pools —
    /// [`DEFAULT_MEMORY_BUDGET`] until one is announced.
    ///
    /// Held here rather than pushed straight into the pools: the split is
    /// derived from it and from the announced chunk size, which arrive in
    /// either order, and [`XzBudget::apportion`] recomputes from this on both
    /// announcements.
    budget: AtomicUsize,
    /// The worker ceiling the caller stated — 1 until one is announced. It is
    /// the **block** pool's depth: one retained block per concurrent reader,
    /// where a chunk buffer is taken by one read and returned when its bytes
    /// are dropped, and wants the replay path's depth instead
    /// (`LocalFileSource::hint_parallelism`).
    jobs: AtomicUsize,
}

impl XzBudget {
    /// The policy for a file `table` describes, over the `decode_bytes` its
    /// provider states.
    ///
    /// The block unit is sized from that table or refused
    /// ([`BlockCache::for_table`]), and the budget is divided before this
    /// returns: a source is readable before anything is announced to it, so
    /// the two pools are split at construction rather than at the first hint.
    pub(super) fn over(table: &xz_seek::SeekTable, decode_bytes: u64) -> Self {
        let budget = Self {
            pool: Arc::new(BufferPool::default()),
            blocks: BlockCache::for_table(table).map(Arc::new),
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
    /// composed incrementally, so [`XzBudget::hint_read_size`] and
    /// [`XzBudget::hint_parallelism`] may arrive in either order and a source
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
    /// source's own chunk size ([`XzBudget::charged_chunk_bytes`]) and decoder
    /// charge — the per-reader charge and the retention list shared beside
    /// it, as one shape.
    ///
    /// It composes the recommendation's shape
    /// ([`ByteRangeSource::default_worker_memory`](crate::ByteRangeSource::default_worker_memory)); the gate
    /// ([`BlockCache::affordable`]) reads [`BlockCache::worker_memory`] itself
    /// and the advice ([`xz_partition_advice`]) builds the same shape from
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

    /// [`XzBudget::block_worker_memory`] evaluated at one reader: one
    /// statement of what the block path costs
    /// ([`ByteRangeSource::block_decode_bytes`](crate::ByteRangeSource::block_decode_bytes)).
    pub(super) fn block_decode_bytes(&self) -> Option<u64> {
        Some(self.block_worker_memory()?.at(1))
    }

    /// How a range over `table` may be cut, read off the arm a read would
    /// actually take ([`xz_partition_advice`]).
    ///
    /// The table is passed in rather than held: the cut points are a property
    /// of the file, which each source already holds its own alias of, and what
    /// this policy decides is which arm they are priced for.
    pub(super) fn partitions(&self, table: &xz_seek::SeekTable, range: Range<u64>) -> Partitioning {
        xz_partition_advice(
            table,
            self.block_path().map(|cache| &**cache),
            self.charged_chunk_bytes(),
            self.decode_bytes,
            range,
        )
    }

    /// The chunk unit is announced to the chunk pool, and the split between
    /// the two pools is re-derived: the chunk pool's share is its own ceiling,
    /// which this number is what sizes ([`XzBudget::apportion`]).
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

/// An `.xz` source's partitioning advice, read off the **read path it took**
/// rather than off the seek table.
///
/// Two answers, and which one applies is `blocks`:
///
/// **Block-decoding** — the boundaries are the block starts inside
/// `range`, so a partition is a whole number of blocks. A partition costs
/// what one concurrent reader holds — [`BlockCache::reader_bytes`], the
/// per-reader term of the [`BlockCache::worker_memory`] whose `at(1)`
/// [`BlockCache::affordable`] compares a budget against.
///
/// **Piecewise** — one partition, whatever the table says: one live
/// `xz_seek::BlockRead` goes forward only, so two workers on different
/// partitions would each force the other's block to be left and begun again.
/// That is true of both providers, each holding one handle behind one mutex
/// (`xz`'s `LiveBlock`, and `docs/design/decisions.md`, "D15").
///
/// Taken as a free function over the two pieces of state it reads, so the
/// piecewise arm is assertable against a table that *does* have blocks.
fn xz_partition_advice(
    table: &xz_seek::SeekTable,
    blocks: Option<&BlockCache>,
    chunk_bytes: u64,
    decode_bytes: u64,
    range: Range<u64>,
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
        // closing it means a handle per reader — which `xz_seek::BlockRead`
        // being `Send` and borrowing nothing already allows — and a figure
        // over the count, and the figure is the half this cannot skip.
        return Partitioning::single(chunk_bytes);
    };
    let covering = table.blocks_in(range.clone());
    let at = covering
        .filter_map(|i| {
            let start = table.blocks[i].uncompressed_offset;
            (start > range.start && start < range.end).then_some(start)
        })
        .collect();
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
    //   `BlockCache::for_table`'s own guard;
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
