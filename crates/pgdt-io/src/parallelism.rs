//! A caller's stated parallelism, what a source holds resident per worker,
//! and how many sub-streams a budget affords — the rule the scan and the
//! leader share.

use std::num::NonZeroUsize;
use std::path::Path;

use crate::memory::{
    DEFAULT_MEMORY_BUDGET, MEMORY_RESERVE, available_memory_in, discover_memory_limit_in,
    margin_allowance,
};

/// What a source holds resident while some number of concurrent workers read
/// it — **a shape rather than a scalar**, because the cost is not linear in
/// the count. See `docs/design/decisions.md`, "D4".
///
/// Two terms:
///
/// - **per worker**, paid once for each concurrent reader, which is what
///   [`Partitioning::partition_bytes`](crate::Partitioning::partition_bytes) states; and
/// - a **shared pool**, `(pool_depth.max(workers) − 1) × pool_unit` — the
///   buffers a pool retains beside the one each worker is filling, which is
///   the block pool's retention list and nothing every other source has.
///
/// **The pool term is arithmetic from [`BufferPool`](crate::source::pool::BufferPool), not a safety margin.**
/// [`BufferPool::slots`](crate::source::pool::BufferPool::slots) clamps a pool at `POOL_DEPTH.max(jobs)` and
/// [`BlockCache::slot`](crate::source::xz::blocks::BlockCache::slot) drains the retention list to `slots − 1` *before*
/// obtaining the buffer [`BlockCache::retain`](crate::source::xz::blocks::BlockCache::retain) then pushes back onto it, so
/// the pool holds `slots − 1` retained units beside the one unit each worker
/// has in flight — unbounded in the block size, which is why
/// [`MEMORY_RESERVE`] cannot absorb it.
///
/// deficiency: KD111 — from two block-decoding readers up, the process holds
/// one unit more than this bills: Rust's live high-water, counted in front of
/// the allocator, sits one block and a few MiB above the charge's Rust terms at
/// every count read from two readers up, on 24 MiB blocks and on 128 MiB, and
/// at one reader it does not (`runs/measure-20261005T193210/`, `reserve`'s
/// instrument tables). Nothing here names the unit. The candidate is `KD20`'s
/// duplicate decode — a worker still holding its own block while its tail read
/// decodes its successor's into a second slot — which only a second reader can
/// cause; no reading confirms it. Being the program's, it is on every
/// allocator, and so far [`MEMORY_UNPOOLED_BOUND`](crate::MEMORY_UNPOOLED_BOUND) has absorbed it. **(b) owned
/// by P23**: closing it means billing the unit here, or, if it is `KD20`'s,
/// not holding it.
///
/// **A budget is solved against this, never divided by it**
/// ([`WorkerMemory::affords`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkerMemory {
    per_worker: u64,
    pool_unit: u64,
    pool_depth: usize,
}

impl WorkerMemory {
    /// A cost that is `per_worker` bytes a worker and nothing else — every
    /// source's shape but the block-decoding one's.
    pub const fn per_worker(per_worker: u64) -> Self {
        Self { per_worker, pool_unit: 0, pool_depth: 0 }
    }

    /// State that a pool retains `unit` bytes for every slot but the one the
    /// worker in flight is filling, over a pool `depth` slots deep or one slot
    /// per worker, whichever is more — on top of the per-worker term.
    pub const fn pooling(mut self, unit: u64, depth: usize) -> Self {
        self.pool_unit = unit;
        self.pool_depth = depth;
        self
    }

    /// The per-worker term alone — **what one concurrent reader holds on its
    /// own**, and what a *cut* is sized by ([`Partitioning::window_end`](crate::Partitioning::window_end)).
    ///
    /// It is deliberately not "what one more reader adds", which is not one
    /// number: the shared pool grows by a unit for every reader past
    /// `pool_depth` and by nothing below it. What stays true at every count is
    /// that a reader holds this much of its own — [`WorkerMemory::at`] is what
    /// a caller sizing an allowance asks.
    pub const fn bytes_per_worker(self) -> u64 {
        self.per_worker
    }

    /// The shared pool at `workers` readers: `pool_depth.max(workers) − 1`
    /// units, which is [`BufferPool::slots`](crate::source::pool::BufferPool::slots) less the slot a decode is writing
    /// into — flat up to the depth and rising with the count above it, and
    /// zero for a shape with no pool unit ([`WorkerMemory::per_worker`]) or a
    /// depth of at most one at one reader.
    pub fn pool_bytes(self, workers: usize) -> u64 {
        (self.pool_depth.max(workers).saturating_sub(1) as u64).saturating_mul(self.pool_unit)
    }

    /// What `workers` concurrent readers cost, in bytes.
    pub fn at(self, workers: usize) -> u64 {
        self.per_worker.saturating_mul(workers as u64).saturating_add(self.pool_bytes(workers))
    }

    /// Whether this source states no cost at all, which is the declining
    /// default: such a source is bounded by the caller's own count and by
    /// nothing here.
    pub const fn is_zero(self) -> bool {
        self.per_worker == 0 && self.pool_unit == 0
    }

    /// The same shape with `extra` bytes added to the per-worker term — what a
    /// caller pinning a batch of its own on top of the source's charge pays
    /// (`pgdt_query::stream::plan_partitions`).
    pub const fn plus_per_worker(mut self, extra: u64) -> Self {
        self.per_worker = self.per_worker.saturating_add(extra);
        self
    }

    /// The largest count in `1..=most` that `cap` affords, **never zero**: a
    /// cap too small for even one worker is a real arrangement, and one worker
    /// is what the floors inside the mechanism deliver there.
    ///
    /// **Two regimes, because the cost is piecewise linear.** At or above
    /// `pool_depth` every reader adds its own term *and* a pool slot, so the
    /// cost is `n × (per_worker + pool_unit) − pool_unit` and the largest
    /// affordable count there is a single division. Below the depth the shared
    /// pool is a constant `(pool_depth − 1)` units, so the cost is linear again
    /// with a different intercept; those candidates are enumerated rather than
    /// divided for, and there are at most `pool_depth` of them — which every
    /// shipped shape sets to [`POOL_DEPTH`](crate::source::pool::POOL_DEPTH), though the type accepts any.
    ///
    /// The cost is non-decreasing in the count on both pieces, so the first
    /// regime's answer is final whenever it reaches the depth at all.
    pub fn affords(self, cap: u64, most: usize) -> usize {
        let most = most.max(1);
        let above = match self.per_worker.saturating_add(self.pool_unit) {
            0 => most,
            slope => usize::try_from(cap.saturating_add(self.pool_unit) / slope)
                .unwrap_or(usize::MAX)
                .min(most),
        };
        if above >= self.pool_depth {
            return above.max(1);
        }
        (1..=most.min(self.pool_depth)).rev().find(|workers| self.at(*workers) <= cap).unwrap_or(1)
    }
}

/// How much concurrency a caller allows a scan or a query, and how much
/// memory that concurrency may hold.
///
/// **The library defaults to a [`Parallelism::Serial`] stating no budget**,
/// which is the serial code path and not a pool of one. See
/// `docs/design/decisions.md`, "D1".
///
/// **Two numbers, and whichever binds first wins**, mirroring
/// `xz_seek::Bulk::new(workers, budget_bytes)`. Neither is defaulted inside
/// [`Parallelism::Workers`].
///
/// **The bytes here are a resolved pool budget, not the allowance a person
/// states.** A caller that means "this is all the memory the process may have"
/// says so through [`Parallelism::within`], which carves the budget out of it;
/// what survives into this type is the carved number, because that is the one
/// every pool and every affordability test reads
/// (`docs/design/decisions.md`, "D83").
///
/// **`Serial` is a state, not the number one.** One worker and the serial path
/// are the same execution, so [`Parallelism::workers`] answers `Serial` for a
/// count of one rather than a degenerate `Workers`, which keeps "is this
/// parallel" a match on the value. The two numbers are independent, so the
/// collapse takes only the count down: `Serial` carries the stated budget as
/// an `Option`, and `None` is the distinct fact that nobody stated a budget at
/// all ([`Parallelism::default`], answered by [`DEFAULT_MEMORY_BUDGET`]).
///
/// **Three mechanisms read it, and only the third spawns.** `memory_bytes`
/// sizes both pools in a source and decides whether a compressed source can
/// afford to decode a whole block (`docs/design/decisions.md`, "D15"); `jobs`
/// is the block pool's depth and — capped by what the bytes afford — how many
/// sub-streams a partitioned replay is cut into
/// (`pgdt_query::table_stream_partitions`), a ceiling rather than a request. The
/// third is `pgdt_dump::leader::scan_region`
/// (`docs/design/decisions.md`, "D48").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parallelism {
    /// The serial code path: one thread, reading in file order — inside
    /// `memory_bytes` where the caller stated one.
    Serial {
        /// What that one thread's source may hold in pooled buffers, in
        /// bytes, or `None` where the caller stated no allowance at all. The
        /// worker count collapsing to the serial path says nothing about the
        /// budget beside it, so the budget survives the collapse.
        memory_bytes: Option<u64>,
    },
    /// At most `jobs` concurrent workers, holding at most `memory_bytes`
    /// between them.
    Workers {
        /// The ceiling on concurrent workers — a ceiling rather than a
        /// request, since some input shapes admit no parallelism at all.
        jobs: NonZeroUsize,
        /// What those workers may hold in their source's **pools**, in bytes —
        /// a resolved budget and not the resident allowance it was carved from
        /// ([`Parallelism::within`]), which is the number a memory cgroup is
        /// denominated in. A worker count cannot be promised to either: what a
        /// given count costs scales with the source's block size
        /// ([`WorkerMemory::at`]).
        memory_bytes: u64,
    },
}

/// The library's own default: the serial path, stating no budget
/// (`docs/design/decisions.md`, "D1").
impl Default for Parallelism {
    fn default() -> Self {
        Self::Serial { memory_bytes: None }
    }
}

impl Parallelism {
    /// `jobs` workers inside `memory_bytes`, or the serial path inside that
    /// same budget where `jobs` is one or zero.
    ///
    /// **The collapse is on the worker count alone**: the bytes beside a count
    /// of one were still stated, so they ride through into
    /// [`Parallelism::Serial`]'s own field.
    ///
    /// Zero reads as one rather than as an error, exactly as
    /// `xz_seek::Bulk::new` reads it: it is a shape a caller's own arithmetic
    /// produces.
    pub fn workers(jobs: usize, memory_bytes: u64) -> Self {
        match NonZeroUsize::new(jobs) {
            Some(jobs) if jobs.get() > 1 => Self::Workers { jobs, memory_bytes },
            _ => Self::Serial { memory_bytes: Some(memory_bytes) },
        }
    }

    /// Whether this is the serial path — a match on the variant, so a stated
    /// budget does not change the answer.
    pub fn is_serial(&self) -> bool {
        matches!(self, Self::Serial { .. })
    }

    /// The bytes this caller allows, or `None` where it stated none — which is
    /// [`Parallelism::default`] and nothing else.
    ///
    /// `None` rather than [`DEFAULT_MEMORY_BUDGET`]: a source that already
    /// holds a budget from an earlier announcement must be able to tell "the
    /// caller said nothing" from "the caller stated that same number".
    pub fn memory_bytes(&self) -> Option<u64> {
        match self {
            Self::Serial { memory_bytes } => *memory_bytes,
            Self::Workers { memory_bytes, .. } => Some(*memory_bytes),
        }
    }

    /// The ceiling on concurrent workers: one for [`Parallelism::Serial`],
    /// which *is* one worker rather than none.
    pub fn jobs(&self) -> usize {
        match self {
            Self::Serial { .. } => 1,
            Self::Workers { jobs, .. } => jobs.get(),
        }
    }

    /// The arrangement to run when the caller has stated neither number and
    /// has nothing open — the environment's own answer, composed from
    /// [`discover_memory_limit`](crate::discover_memory_limit) and `std::thread::available_parallelism`
    /// through [`MEMORY_RESERVE`].
    ///
    /// **It is a convenience over two primitives, and taking it is opting
    /// in**: [`Parallelism::default`] is still the serial path stating no
    /// budget. See `docs/design/decisions.md`, "D1" and "D11".
    ///
    /// **An unlimited environment falls back to the shipped constant**: with
    /// no limit found and no source to recommend otherwise, this is
    /// [`DEFAULT_MEMORY_BUDGET`] — or, at a serial count,
    /// [`Parallelism::default`] itself.
    pub fn discover() -> Self {
        Self::discover_for(std::thread::available_parallelism().map_or(1, NonZeroUsize::get), None)
    }

    /// [`Parallelism::discover`]'s rule against recommendations a caller has
    /// already obtained — a source's own worker count
    /// ([`ByteRangeSource::default_workers`](crate::ByteRangeSource::default_workers)) and what one of those workers
    /// holds ([`ByteRangeSource::default_worker_memory`](crate::ByteRangeSource::default_worker_memory)).
    ///
    /// **It answers with both numbers, because they are a pair**: where the
    /// allowance affords fewer than `jobs`, the answer is the smaller count
    /// *and* the budget that count spends (`docs/design/roadmap.md`, "A
    /// default runs as fast as the allocation permits"). A caller holding a
    /// count somebody **stated** keeps that count and takes only the budget
    /// from here (`Discovered::resolve`).
    ///
    /// **The composition is not a `min`**, which would collapse a compressed
    /// scan to serial on an unlimited host (`docs/design/decisions.md`, "D3").
    /// What distinguishes *no limit found* from *a small limit* is that the
    /// first takes no reserve and no margin:
    ///
    /// - a discovered limit is carved by [`Parallelism::within`], the same
    ///   arithmetic a stated allowance gets: it caps at `limit −
    ///   MEMORY_RESERVE`, and [`WorkerMemory::at`] evaluated at the count the
    ///   budget *affords* — or
    ///   [`DEFAULT_MEMORY_BUDGET`] where the caller has no recommendation — is
    ///   taken no higher, and may fall **below**
    ///   [`DEFAULT_MEMORY_BUDGET`]. It is **not** `jobs × per_worker`: the
    ///   count is the afforded one rather than the asked-for one, and `at`
    ///   carries the pool's retention term beside the per-reader charge
    ///   (`docs/design/decisions.md`, "D4"). The **count** answers to a second
    ///   condition: its predicted resident must leave
    ///   [`MEMORY_MARGIN_PERCENT`](crate::MEMORY_MARGIN_PERCENT) of the limit unused ([`margin_allowance`]);
    /// - no limit found caps at half of [`available_memory`](crate::available_memory), an estimate two
    ///   processes reading at once each see the whole of, and where that
    ///   cannot be read, not at all.
    ///
    /// **A caller with no recommendation and no limit states nothing**, which
    /// is [`Parallelism::default`] at a serial count and
    /// [`DEFAULT_MEMORY_BUDGET`] above one.
    pub fn discover_for(jobs: usize, memory: Option<WorkerMemory>) -> Self {
        Self::discover_in(Path::new("/"), jobs, memory)
    }

    /// [`Parallelism::discover_for`] against an arbitrary filesystem root, for
    /// the reason [`discover_memory_limit_in`] takes one — and public for the
    /// same reason it is: a caller's own resolution is pinned against
    /// environments this machine cannot be put into
    /// (`crates/pgdt/tests/data/runtime/`).
    pub fn discover_in(root: &Path, jobs: usize, memory: Option<WorkerMemory>) -> Self {
        Self::discover_holding_in(root, jobs, memory, 0)
    }

    /// [`Parallelism::discover_in`] for a caller holding `held` bytes resident
    /// besides the arrangement — the statistics a cache hands its scan — which
    /// a discovered limit carves as [`Parallelism::within_shared`] does.
    ///
    /// **With no limit found, `held` changes nothing**: that arrangement is
    /// held under no margin, the halving of what the machine reports free
    /// being the margin there.
    pub fn discover_holding_in(
        root: &Path,
        jobs: usize,
        memory: Option<WorkerMemory>,
        held: u64,
    ) -> Self {
        let (cap, ceiling) = match discover_memory_limit_in(root) {
            // The one carving, shared with a stated allowance
            // ([`Parallelism::within`]).
            Some(limit) => return Self::within_shared(jobs, memory, limit.bytes, 0, held),
            // Nothing discovered and nothing recommended: no cap to state, and
            // no recommendation to cap, which is the library's own default.
            None if memory.is_none() => (None, None),
            // `MemAvailable` unreadable is the one shape with a recommendation
            // and no ceiling to hold it under — and no limit to take a margin
            // of either, the halving being the margin there.
            // deficiency: KD113 — so the recommendation stands whole, at the
            // count asked for, and `datafusion-pgdump`'s `ScanBudget`, holding
            // no allowance, lets each scan discover alone, sharing nothing.
            // The standing rule is that unstated is not unlimited
            // (`docs/design/roadmap.md`, "A default runs as fast as the
            // allocation permits"); the fix is a choice between
            // `DEFAULT_MEMORY_BUDGET` here and refusing a recommendation.
            None => {
                (Some(available_memory_in(root).map_or(u64::MAX, |available| available / 2)), None)
            }
        };
        match cap {
            Some(cap) => {
                let (jobs, budget) = Self::fit(jobs, memory, cap, ceiling);
                Self::workers(jobs, budget)
            }
            None if jobs > 1 => Self::workers(jobs, DEFAULT_MEMORY_BUDGET),
            None => Self::default(),
        }
    }

    /// The arrangement `jobs` workers take inside a **resident allowance** the
    /// caller states — `pgdt --memory`, and an embedder that means by its
    /// number what a container means by one.
    ///
    /// **It is [`Parallelism::discover_in`]'s own carving, called with a
    /// stated number in place of the discovered limit**: [`MEMORY_RESERVE`]
    /// comes off the top for everything the pools do not bill, the count is
    /// held to [`margin_allowance`], and the budget it answers with is what
    /// that many workers spend. Neither number the caller gets back is the one
    /// it typed, and that is the point — the typed number bounds the
    /// **process**, where the budget bounds the pools
    /// (`docs/design/decisions.md`, "D83").
    ///
    /// **The count it returns is a recommendation lowered to fit**. A caller
    /// holding a count somebody *stated* keeps that count and takes only the
    /// budget from here, exactly as against a discovered limit
    /// (`docs/design/roadmap.md`, "A default runs as fast as the allocation
    /// permits").
    pub fn within(jobs: usize, memory: Option<WorkerMemory>, allowance: u64) -> Self {
        Self::within_shared(jobs, memory, allowance, 0, 0)
    }

    /// [`Parallelism::within`] for one of several arrangements drawing on one
    /// allowance at once — concurrent scans in one process — where `drawn` is
    /// the read-buffer budget the others already hold, and `held` what the
    /// process holds resident besides any arrangement: loaded statistics, or
    /// memory granted to something other than a scan.
    ///
    /// **The reserve comes off once, and what is drawn comes off both
    /// bounds**: [`MEMORY_RESERVE`] covers the process however many
    /// arrangements run inside it, while every byte another arrangement holds
    /// is a byte of the cap and of [`margin_allowance`]'s ceiling this one
    /// cannot spend. An allowance already drawn to nothing leaves a source
    /// recommending a per-worker cost one worker on a budget of zero, the
    /// floor [`Parallelism::within`] has too, and one recommending none its
    /// count on that zero.
    ///
    /// **What is `held` comes off the ceiling alone**, as
    /// [`statistics_allowance`](crate::statistics_allowance) bills statistics (`docs/design/decisions.md`,
    /// "D85"): the cap is the reserve's, which stands for the arrangements'
    /// own excess over their budgets, and a holding is none of that excess.
    /// So a holding lowers the count first.
    ///
    /// **Whatever of `held` the ceiling's room cannot absorb at the resolved
    /// count comes off the budget**, for every source: where the count cannot
    /// fall — a source recommending no per-worker memory, whose count no
    /// ceiling bounds, or one worker, the floor — the predicted resident still
    /// stays under the margin, or where one worker alone overruns it, no
    /// further over it than with nothing held, and a budget pushed below a
    /// unit declines it (`docs/design/decisions.md`, "D4"). The cap is
    /// untouched, and with nothing held the budget is [`Parallelism::fit`]'s.
    /// *Rejected: a plain-source exception*, what a plain budget is worth
    /// being `KD25`'s and `KD32`'s question rather than this rule's.
    pub fn within_shared(
        jobs: usize,
        memory: Option<WorkerMemory>,
        allowance: u64,
        drawn: u64,
        held: u64,
    ) -> Self {
        let ceiling = margin_allowance(allowance).saturating_sub(drawn);
        let (jobs, budget) = Self::fit(
            jobs,
            memory,
            allowance.saturating_sub(MEMORY_RESERVE).saturating_sub(drawn),
            Some(ceiling.saturating_sub(held)),
        );
        let unabsorbed = held.saturating_sub(ceiling.saturating_sub(budget));
        Self::workers(jobs, budget.saturating_sub(unabsorbed))
    }

    /// The largest pair `(count, budget)` that fits inside `cap`: as many of
    /// `jobs` workers as `cap` affords at `per_worker` each, and what that
    /// many of them spend, never more than `cap` — so at the one-worker floor
    /// a worker costing more than `cap` is budgeted `cap`.
    ///
    /// **The floor is one worker at whatever `cap` is**, not one worker's
    /// worth of bytes: a cgroup at or under [`MEMORY_RESERVE`] resolves to a
    /// budget of zero, and the floors already in the mechanism turn that into
    /// one reader on the streaming path. A caller recommending nothing is
    /// capped at [`DEFAULT_MEMORY_BUDGET`] and keeps its count.
    ///
    /// **It solves rather than divides** ([`WorkerMemory::affords`]), and the
    /// budget it names is what that many workers actually spend, pool
    /// included, up to `cap` (`docs/design/decisions.md`, "D4").
    ///
    /// **`charge_ceiling` is the margin, and it bounds the count alone**: the
    /// most this arrangement may be *charged* if its predicted resident —
    /// `memory.at(n)` plus [`MEMORY_UNPOOLED_BOUND`](crate::MEMORY_UNPOOLED_BOUND) — is to leave
    /// [`MEMORY_MARGIN_PERCENT`](crate::MEMORY_MARGIN_PERCENT) of the allowance unused
    /// ([`margin_allowance`]), whether that allowance was discovered or typed
    /// (`docs/design/decisions.md`, "D83"). The budget stays `cap`-bounded
    /// rather than ceiling-bounded, so what a lowered count reports is still
    /// what it spends (`docs/design/decisions.md`, "D3").
    pub(super) fn fit(
        jobs: usize,
        memory: Option<WorkerMemory>,
        cap: u64,
        charge_ceiling: Option<u64>,
    ) -> (usize, u64) {
        let jobs = jobs.max(1);
        let Some(memory) = recommended(memory) else {
            return (jobs, DEFAULT_MEMORY_BUDGET.min(cap));
        };
        let affords = memory.affords(charge_ceiling.map_or(cap, |ceiling| cap.min(ceiling)), jobs);
        (affords, cap.min(memory.at(affords)))
    }

    /// Whether a larger allowance, carved as [`Parallelism::within`] carves
    /// one, would raise a budget of `budget` bytes for a source recommending
    /// `memory` ([`ByteRangeSource::default_worker_memory`](crate::ByteRangeSource::default_worker_memory)) — the question a
    /// plan note's [`pgdt_query::PlanLever::LargerAllowance`] answers.
    ///
    /// **It is the carving's cap read backwards**, so the rule stays in one
    /// place: a source recommending no cost is carved to
    /// [`DEFAULT_MEMORY_BUDGET`] at most whatever the allowance
    /// (`docs/design/decisions.md`, "D83", and `KD32`), so a budget already
    /// there is one no allowance raises, and one below it — an allowance
    /// under [`MEMORY_RESERVE`] plus that constant, or holdings the margin
    /// could not absorb —
    /// is one a larger allowance does. A source recommending a cost is
    /// answered yes, its count unread: its budget rises with the count wherever
    /// the cap or the margin, rather than `jobs`, bounded it.
    pub fn allowance_raises_budget(memory: Option<WorkerMemory>, budget: u64) -> bool {
        recommended(memory).is_some() || budget < DEFAULT_MEMORY_BUDGET
    }
}

/// A source's recommendation, where it states a cost at all: one stating
/// none is carved as one recommending nothing ([`Parallelism::fit`]).
fn recommended(memory: Option<WorkerMemory>) -> Option<WorkerMemory> {
    memory.filter(|memory| !memory.is_zero())
}

/// The byte budget actually governing reads under `p`, worded for a status
/// line: the stated byte count, or [`DEFAULT_MEMORY_BUDGET`] — what every pool
/// falls back to — marked `(default)`. Deliberately a free function rather
/// than a method on [`Parallelism`], whose own `memory_bytes` must keep "the
/// caller said nothing" apart from "the caller stated that number"
/// (`docs/design/decisions.md`, "D64").
pub fn memory_budget_display(p: Parallelism) -> String {
    match p.memory_bytes() {
        Some(bytes) => bytes.to_string(),
        None => format!("{DEFAULT_MEMORY_BUDGET} (default)"),
    }
}

/// How many sub-streams a caller's [`Parallelism`] and a source's per-partition
/// footprint allow between them.
///
/// **Both numbers bind, and the bytes bind on the *stream* count rather than
/// per block**, the sub-streams being what run at once. A source that states
/// no footprint (the declining default) is bounded by `jobs` alone, and the
/// budget is *solved* against the source's cost, never divided by it
/// ([`WorkerMemory::affords`], and
/// `docs/design/decisions.md`, "D4"). **Shared with the leader**, which asks
/// the same question of an open `COPY` block's interior
/// (`pgdt_dump::leader::scan_region`, and `docs/design/decisions.md`, "D48").
pub fn worker_count(parallelism: Parallelism, memory: WorkerMemory) -> usize {
    let jobs = parallelism.jobs();
    match parallelism.memory_bytes() {
        Some(budget) if !memory.is_zero() => memory.affords(budget, jobs),
        _ => jobs,
    }
}

#[cfg(test)]
mod tests;
