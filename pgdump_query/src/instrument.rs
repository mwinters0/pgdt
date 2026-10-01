//! What the library can tell an instrumented binary about its own work: which
//! allocations are statistics, how often a read began decoding a block, and
//! where a dynamic filter's row evaluation spends its time.
//!
//! **Off by default and compiled out of every build that does not ask.** The
//! `introspect` feature turns it on, and only a binary's own `introspect`
//! feature does — `pgdt`'s and `datafusion-cli-pgdump`'s
//! (`docs/design/decisions.md`, "D13"): without it, `StatisticsScope` is an
//! empty guard, [`timed!`] is what it wraps beside its part evaluated and
//! dropped, [`row_evaluated!`] what it wraps and nothing more, and every function recording or reading a figure does
//! nothing.
//! The library still installs no allocator; the binary's counting allocator
//! calls `allocated` and `freed`, and this module decides what counts.
//!
//! **An allocation is a statistic when the thread making it is inside a
//! `StatisticsScope`**, which the gatherer enters for everything it does —
//! building, observing, closing, folding, finishing and dropping an observer —
//! and which a block's statistics are decoded and replaced under, and sealed
//! into their `Arc` under, by a mapping pass and a re-read alike. A free is
//! attributed the same way, so a statistic freed outside every scope reads as
//! still live: the instrument errs towards finding the account short.
//!
//! What it answers is the reconciliation of `crate::statistics`'
//! account against the heap (`docs/design/decisions.md`, "D81"): at every
//! update the live statistics bytes this module counts are compared with the
//! account's total, and with that total less what it carries for allocations
//! charged ahead — made or not yet, on any thread — and the worst shortfall of
//! the first and excess of the second are kept, raw and past the allowance the
//! account gives its open observers' uncharged growth then.

#[cfg(feature = "introspect")]
pub use enabled::{StatisticsReading, allocated, freed, statistics_reading};
#[cfg(feature = "introspect")]
pub use evaluation::{EvaluationReading, evaluation_reading};

/// The parts of a dynamic filter's row evaluation the instrument times apart,
/// in the order its report prints them.
///
/// **What it answers is where a row's evaluation spends its time**, which a
/// timing of the whole run cannot say: the fixed cost of evaluating a row at
/// all against what each leaf pays, and of a leaf's cost how much is finding,
/// unescaping and keying its field — paid again by every leaf reading the same
/// one — against its comparison or its membership's lookup. The tree walk
/// has no part of its own: it is [`Self::Row`] less every leaf part inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationPart {
    /// One row's evaluation against a dynamic filter's state, whole
    /// (`stream::DynamicRead::rejects`): every leaf part below and the tree
    /// walk between them. **The leaf parts are timed only inside one**, so a
    /// static filter, a sorted stop and a row group's statistics, which
    /// evaluate the same leaves, add nothing to them.
    Row,
    /// A leaf finding its field in the row (`copy::RowSplit::field`), which
    /// extends the row's one shared split as far as that field.
    Locate,
    /// A leaf unescaping its field (`copy::RawRow::decode`).
    Unescape,
    /// A leaf reading the unescaped text as its column's comparison key
    /// (`predicate::order_key` or its nested counterpart); a comparison of
    /// text has none.
    Key,
    /// A term comparing that key or text with its bound.
    Compare,
    /// A membership probing its lookup for that key or text.
    Lookup,
    /// A replay's read of the filter's state as it takes a chunk
    /// (`stream::DynamicRead::at_chunk`), outside every row.
    Chunk,
}

impl EvaluationPart {
    /// Every part, in report order.
    pub const ALL: [Self; 7] = [
        Self::Row,
        Self::Locate,
        Self::Unescape,
        Self::Key,
        Self::Compare,
        Self::Lookup,
        Self::Chunk,
    ];

    /// The part's name in the report's keys.
    pub fn name(self) -> &'static str {
        match self {
            Self::Row => "row",
            Self::Locate => "locate",
            Self::Unescape => "unescape",
            Self::Key => "key",
            Self::Compare => "compare",
            Self::Lookup => "lookup",
            Self::Chunk => "chunk",
        }
    }

    /// Whether the part is timed only inside a [`Self::Row`].
    #[cfg_attr(not(feature = "introspect"), allow(dead_code))]
    fn is_leaf(self) -> bool {
        !matches!(self, Self::Row | Self::Chunk)
    }
}

/// `$body`, timed as `$part` where the instrument is built in — a leaf part
/// only inside [`row_evaluated!`]. **Without the instrument it expands to
/// `$body`, `$part` evaluated and dropped beside it**, so a shipped build compiles what it wraps
/// exactly as written; an inlined closure is not enough to promise that.
/// With it, `$body` runs in a closure passed to [`timed_span`], which the
/// calling module imports under the feature — in a `use`, where
/// `tests/layering.rs` sees the edge.
macro_rules! timed {
    ($part:expr, $body:expr) => {{
        #[cfg(feature = "introspect")]
        {
            timed_span($part, || $body)
        }
        #[cfg(not(feature = "introspect"))]
        {
            let _ = $part;
            $body
        }
    }};
}
pub(crate) use timed;

/// `$body`, one row's evaluation against a dynamic filter's state, timed as
/// [`EvaluationPart::Row`] with every leaf part it reaches, where the
/// instrument is built in, through [`evaluated_row`]; without it, `$body`
/// alone, as [`timed!`] is.
macro_rules! row_evaluated {
    ($body:expr) => {{
        #[cfg(feature = "introspect")]
        {
            evaluated_row(|| $body)
        }
        #[cfg(not(feature = "introspect"))]
        {
            $body
        }
    }};
}
pub(crate) use row_evaluated;

/// What [`timed!`] runs its body through with the instrument built in.
#[cfg(feature = "introspect")]
#[inline(always)]
pub(crate) fn timed_span<T>(part: EvaluationPart, f: impl FnOnce() -> T) -> T {
    evaluation::timed(part, f)
}

/// What [`row_evaluated!`] runs its body through with the instrument built in.
#[cfg(feature = "introspect")]
#[inline(always)]
pub(crate) fn evaluated_row<T>(f: impl FnOnce() -> T) -> T {
    evaluation::row(f)
}

/// How many block decodes a piecewise `.xz` read path has begun rather than
/// resumed.
///
/// **Nothing without the instrument**: the field is absent, `begun` compiles
/// away and no build that ships carries the atomic. What it exists for is a
/// property a shipped build cannot state — that a block a forward scan sits
/// inside is decoded once rather than once per read — which a test asserts
/// rather than argues (`docs/design/decisions.md`, "D15").
#[derive(Debug, Default)]
pub(crate) struct DecodeCounter {
    #[cfg(feature = "introspect")]
    begun: std::sync::atomic::AtomicU64,
}

impl DecodeCounter {
    /// A block is about to be begun rather than resumed. Counted where that
    /// is decided, so a `begin` that then errors is counted.
    #[inline]
    pub(crate) fn begun(&self) {
        #[cfg(feature = "introspect")]
        self.begun.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// How many have. Racy against a read still running, which the caller
    /// avoids by reading once the read has returned.
    #[cfg(feature = "introspect")]
    pub(crate) fn count(&self) -> u64 {
        self.begun.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Attributes every allocation and free the current thread makes to
/// statistics while it is alive. Scopes nest.
pub(crate) struct StatisticsScope {
    #[cfg(feature = "introspect")]
    outer: bool,
}

impl StatisticsScope {
    #[inline]
    pub(crate) fn enter() -> Self {
        #[cfg(feature = "introspect")]
        {
            Self { outer: enabled::IN_STATISTICS.replace(true) }
        }
        #[cfg(not(feature = "introspect"))]
        {
            Self {}
        }
    }
}

#[cfg(feature = "introspect")]
impl Drop for StatisticsScope {
    fn drop(&mut self) {
        enabled::IN_STATISTICS.set(self.outer);
    }
}

/// Record that a cache load handed a pass `bytes` of block statistics — the
/// walk [`crate::DumpIndex::statistics_heap_bytes`] does. Nothing without the
/// instrument, and the call sites are `cfg`'d, so a shipped build does not walk
/// the index for it.
///
/// **It is the only statistics term a `query` has.** A query loads a cache and
/// keeps no [`crate::statistics::StatisticsAccount`], so nothing else says what
/// the cache handed it.
#[cfg(feature = "introspect")]
pub(crate) fn statistics_loaded(bytes: u64) {
    enabled::statistics_loaded(bytes);
}

/// Compare the account's new `total`, `announced` of it charged ahead of
/// allocations, with the live statistics bytes, keeping the worst difference
/// either way past `allowance`, the uncharged growth its open observers may
/// hold. Nothing without the instrument.
#[inline]
pub(crate) fn statistics_account_updated(total: u64, announced: u64, allowance: u64) {
    #[cfg(feature = "introspect")]
    enabled::account_updated(total, announced, allowance);
    #[cfg(not(feature = "introspect"))]
    let _ = (total, announced, allowance);
}

#[cfg(feature = "introspect")]
mod enabled {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};

    thread_local! {
        /// Whether this thread is inside a [`super::StatisticsScope`]. `const`
        /// and without a destructor, so reading it from inside an allocator
        /// allocates nothing and is sound on a thread being torn down.
        pub(super) static IN_STATISTICS: Cell<bool> = const { Cell::new(false) };
    }

    /// Statistics bytes allocated and not yet freed.
    static LIVE: AtomicU64 = AtomicU64::new(0);
    /// The largest [`LIVE`] reached.
    static LIVE_PEAK: AtomicU64 = AtomicU64::new(0);
    /// How many times the account was updated, and so compared.
    static CHECKS: AtomicU64 = AtomicU64::new(0);
    /// The largest allowance any update was given.
    static ALLOWANCE_PEAK: AtomicU64 = AtomicU64::new(0);
    /// The largest `live − total` and `total − announced − live` any update
    /// read, and each past the allowance then — the second being the total
    /// less what it carries for allocations charged ahead.
    static SHORT: AtomicU64 = AtomicU64::new(0);
    static SHORT_PAST_ALLOWANCE: AtomicU64 = AtomicU64::new(0);
    static OVER: AtomicU64 = AtomicU64::new(0);
    static OVER_PAST_ALLOWANCE: AtomicU64 = AtomicU64::new(0);
    /// The most statistics heap any cache load handed a pass.
    static LOADED: AtomicU64 = AtomicU64::new(0);

    /// The binary's allocator reports `bytes` allocated on this thread.
    #[inline]
    pub fn allocated(bytes: usize) {
        if IN_STATISTICS.get() {
            let live = LIVE.fetch_add(bytes as u64, Ordering::Relaxed) + bytes as u64;
            LIVE_PEAK.fetch_max(live, Ordering::Relaxed);
        }
    }

    /// The binary's allocator reports `bytes` freed on this thread.
    #[inline]
    pub fn freed(bytes: usize) {
        if IN_STATISTICS.get() {
            LIVE.fetch_sub(bytes as u64, Ordering::Relaxed);
        }
    }

    pub(super) fn account_updated(total: u64, announced: u64, allowance: u64) {
        CHECKS.fetch_add(1, Ordering::Relaxed);
        ALLOWANCE_PEAK.fetch_max(allowance, Ordering::Relaxed);
        let live = LIVE.load(Ordering::Relaxed);
        // An allocation charged ahead may or may not have been made yet, on
        // this thread or another: short of the total, over what it leaves.
        let short = live.saturating_sub(total);
        SHORT.fetch_max(short, Ordering::Relaxed);
        SHORT_PAST_ALLOWANCE.fetch_max(short.saturating_sub(allowance), Ordering::Relaxed);
        let over = total.saturating_sub(announced).saturating_sub(live);
        OVER.fetch_max(over, Ordering::Relaxed);
        OVER_PAST_ALLOWANCE.fetch_max(over.saturating_sub(allowance), Ordering::Relaxed);
    }

    /// A cache load handed a pass `bytes` of block statistics.
    pub(super) fn statistics_loaded(bytes: u64) {
        LOADED.fetch_max(bytes, Ordering::Relaxed);
    }

    /// What the instrument has counted so far.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct StatisticsReading {
        /// The most block-statistics heap a cache load handed a pass
        /// ([`super::statistics_loaded`]).
        pub loaded: u64,
        /// Statistics bytes live now.
        pub live: u64,
        /// The most statistics bytes ever live at once.
        pub live_peak: u64,
        /// Account updates compared.
        pub checks: u64,
        /// The largest allowance any update was given.
        pub allowance_peak: u64,
        /// The largest amount any update found the account short of live.
        pub short: u64,
        /// The largest amount any update found it short past its allowance.
        pub short_past_allowance: u64,
        /// The largest amount any update found the account, less what it
        /// announced, over live.
        pub over: u64,
        /// The largest amount any update found it over past its allowance.
        pub over_past_allowance: u64,
    }

    /// Read the counters. Racy against a thread still allocating, which the
    /// caller avoids by reading once the pass has returned.
    pub fn statistics_reading() -> StatisticsReading {
        StatisticsReading {
            loaded: LOADED.load(Ordering::Relaxed),
            live: LIVE.load(Ordering::Relaxed),
            live_peak: LIVE_PEAK.load(Ordering::Relaxed),
            checks: CHECKS.load(Ordering::Relaxed),
            allowance_peak: ALLOWANCE_PEAK.load(Ordering::Relaxed),
            short: SHORT.load(Ordering::Relaxed),
            short_past_allowance: SHORT_PAST_ALLOWANCE.load(Ordering::Relaxed),
            over: OVER.load(Ordering::Relaxed),
            over_past_allowance: OVER_PAST_ALLOWANCE.load(Ordering::Relaxed),
        }
    }
}

#[cfg(feature = "introspect")]
mod evaluation {
    use std::cell::Cell;
    use std::fmt::Write as _;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    use super::EvaluationPart;

    /// One part's spans: how many, and the ticks they spent.
    struct Slot {
        count: AtomicU64,
        ticks: AtomicU64,
    }

    impl Slot {
        const fn new() -> Self {
            Self { count: AtomicU64::new(0), ticks: AtomicU64::new(0) }
        }
    }

    /// Shared by every thread: at one partition a relaxed atomic is
    /// uncontended, so per-thread accumulators buy nothing.
    static SLOTS: [Slot; EvaluationPart::ALL.len()] =
        [const { Slot::new() }; EvaluationPart::ALL.len()];

    thread_local! {
        /// Whether this thread is inside a [`row`]. `const` and without a
        /// destructor, so reading it costs one load.
        static IN_ROW: Cell<bool> = const { Cell::new(false) };
    }

    /// An `Instant` and a tick count read together at the first span closed,
    /// an earlier reading's calibration included, which a reading's own pair
    /// converts ticks to nanoseconds against.
    static ORIGIN: OnceLock<(Instant, u64)> = OnceLock::new();

    /// What [`ticks`] reads, for the report.
    #[cfg(target_arch = "x86_64")]
    const TIMER: &str = "rdtsc";
    #[cfg(not(target_arch = "x86_64"))]
    const TIMER: &str = "instant";

    /// The time now, in ticks: the time-stamp counter where there is one, a
    /// read unordered against the work around it, so a span shorter than the
    /// pipeline is smeared across its neighbours and only a sum over many is
    /// read — nanoseconds since the first read elsewhere. `Instant` is a vDSO
    /// call around the same counter; `rdtscp` or a fence would price a span
    /// as if the pipeline held nothing else.
    #[inline(always)]
    #[allow(unused_unsafe)]
    fn ticks() -> u64 {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: `rdtsc` reads a counter every x86_64 processor has.
        unsafe {
            std::arch::x86_64::_rdtsc()
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            static EPOCH: OnceLock<Instant> = OnceLock::new();
            EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
        }
    }

    /// Close a span begun at `start` into `slot`.
    #[inline(always)]
    fn record(slot: &Slot, start: u64) {
        let spent = ticks().wrapping_sub(start);
        slot.count.fetch_add(1, Ordering::Relaxed);
        slot.ticks.fetch_add(spent, Ordering::Relaxed);
        ORIGIN.get_or_init(|| (Instant::now(), ticks()));
    }

    /// `f` timed into `slot`, a `leaf` span only inside a row — the one path
    /// both a real span and [`calibrate`]'s take, so what calibration reads
    /// is what a span costs.
    #[inline(always)]
    fn timed_into<T>(slot: &Slot, leaf: bool, f: impl FnOnce() -> T) -> T {
        if leaf && !IN_ROW.get() {
            return f();
        }
        let start = ticks();
        let out = f();
        record(slot, start);
        out
    }

    #[inline(always)]
    pub(super) fn timed<T>(part: EvaluationPart, f: impl FnOnce() -> T) -> T {
        timed_into(&SLOTS[part as usize], part.is_leaf(), f)
    }

    #[inline(always)]
    pub(super) fn row<T>(f: impl FnOnce() -> T) -> T {
        let outer = IN_ROW.replace(true);
        let out = timed_into(&SLOTS[EvaluationPart::Row as usize], false, f);
        IN_ROW.set(outer);
        out
    }

    /// What an empty span records of itself, and what one costs a span
    /// enclosing it, in ticks: each the least mean of several rounds, the
    /// least being the one the machine disturbed least.
    fn calibrate() -> (f64, f64) {
        const SPANS: u64 = 200_000;
        const ROUNDS: usize = 7;
        let outer = IN_ROW.replace(true);
        let (mut empty, mut nested) = (f64::INFINITY, f64::INFINITY);
        for _ in 0..ROUNDS {
            let (inner, enclosing) = (Slot::new(), Slot::new());
            timed_into(&enclosing, false, || {
                for _ in 0..SPANS {
                    timed_into(&inner, true, || std::hint::black_box(()));
                }
            });
            let mean = |slot: &Slot| slot.ticks.load(Ordering::Relaxed) as f64 / SPANS as f64;
            empty = empty.min(mean(&inner));
            nested = nested.min(mean(&enclosing));
        }
        IN_ROW.set(outer);
        (empty, nested)
    }

    /// What the instrument has timed so far, and what it takes to read it.
    #[derive(Debug, Clone)]
    pub struct EvaluationReading {
        /// Each part's spans and the raw ticks they spent, in
        /// [`EvaluationPart::ALL`]'s order.
        counts: [(u64, u64); EvaluationPart::ALL.len()],
        /// Nanoseconds a tick, from the first span closed — an earlier
        /// reading's calibration included — to this reading; `NaN` where no
        /// span closed before it.
        pub nanos_per_tick: f64,
        /// What an empty span records of itself, in ticks.
        pub empty_span_ticks: f64,
        /// What an empty span costs a span enclosing it, in ticks.
        pub nested_span_ticks: f64,
        /// The counter a tick is: `rdtsc`, or `instant`'s nanoseconds.
        pub timer: &'static str,
    }

    impl EvaluationReading {
        /// How many spans of `part` were timed.
        pub fn count(&self, part: EvaluationPart) -> u64 {
            self.counts[part as usize].0
        }

        /// The ticks `part`'s spans recorded, as read.
        pub fn raw_ticks(&self, part: EvaluationPart) -> u64 {
            self.counts[part as usize].1
        }

        /// The nanoseconds `part` spent, less its own spans' overhead and, for
        /// a row, what each leaf span inside it cost it. Not clamped: a
        /// negative is a calibration that overstates the overhead, and is
        /// read as such.
        pub fn nanos(&self, part: EvaluationPart) -> f64 {
            let (count, ticks) = self.counts[part as usize];
            let mut spent = ticks as f64 - count as f64 * self.empty_span_ticks;
            if part == EvaluationPart::Row {
                let leaves: u64 = EvaluationPart::ALL
                    .into_iter()
                    .filter(|p| p.is_leaf())
                    .map(|p| self.count(p))
                    .sum();
                spent -= leaves as f64 * self.nested_span_ticks;
            }
            spent * self.nanos_per_tick
        }

        /// A row's time outside every leaf part: the walk of the tree and the
        /// dispatch of each leaf.
        pub fn tree_walk_nanos(&self) -> f64 {
            let leaves: f64 = EvaluationPart::ALL
                .into_iter()
                .filter(|p| p.is_leaf())
                .map(|p| self.nanos(p))
                .sum();
            self.nanos(EvaluationPart::Row) - leaves
        }

        /// The reading as `key=value` lines, the shape `pgdt`'s report takes.
        pub fn lines(&self) -> String {
            let mut out = String::new();
            let _ = writeln!(out, "evaluation_timer={}", self.timer);
            let _ = writeln!(out, "evaluation_nanos_per_tick={:.6}", self.nanos_per_tick);
            let _ = writeln!(out, "evaluation_empty_span_ticks={:.2}", self.empty_span_ticks);
            let _ = writeln!(out, "evaluation_nested_span_ticks={:.2}", self.nested_span_ticks);
            for part in EvaluationPart::ALL {
                let name = part.name();
                let _ = writeln!(out, "evaluation_{name}_count={}", self.count(part));
                let _ = writeln!(out, "evaluation_{name}_raw_ticks={}", self.raw_ticks(part));
                let _ = writeln!(out, "evaluation_{name}_nanos={:.0}", self.nanos(part));
            }
            let _ = writeln!(out, "evaluation_tree_walk_nanos={:.0}", self.tree_walk_nanos());
            out
        }
    }

    /// Read what has been timed, calibrating the spans' own cost first. Racy
    /// against a thread still evaluating, which the caller avoids by reading
    /// once the query has returned.
    pub fn evaluation_reading() -> EvaluationReading {
        // Taken before calibrating, whose own spans would otherwise set it.
        let origin = ORIGIN.get().copied();
        let (empty_span_ticks, nested_span_ticks) = calibrate();
        let counts = std::array::from_fn(|i| {
            (SLOTS[i].count.load(Ordering::Relaxed), SLOTS[i].ticks.load(Ordering::Relaxed))
        });
        let nanos_per_tick = match origin {
            Some((at, first)) => {
                let (elapsed, now) = (at.elapsed(), ticks());
                elapsed.as_nanos() as f64 / now.wrapping_sub(first) as f64
            }
            None => f64::NAN,
        };
        EvaluationReading {
            counts,
            nanos_per_tick,
            empty_span_ticks,
            nested_span_ticks,
            timer: TIMER,
        }
    }
}
