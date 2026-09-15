//! What the library can tell an instrumented binary about its own heap: which
//! allocations are statistics.
//!
//! **Off by default and compiled out of every build that does not ask.** The
//! `introspect` feature turns it on, and only `pgdump_query-cli`'s own
//! `introspect` feature does (`docs/design/decisions.md`, "D13"): without it,
//! `StatisticsScope` is an empty guard and every function here does nothing.
//! The library still installs no allocator; the binary's counting allocator
//! calls `allocated` and `freed`, and this module decides what counts.
//!
//! **An allocation is a statistic when the thread making it is inside a
//! `StatisticsScope`**, which the gatherer enters for everything it does —
//! building, observing, closing, folding, finishing and dropping an observer —
//! and which a block's statistics are decoded, sealed into their `Arc` and
//! replaced under. A free is attributed the same way, so a statistic freed
//! outside every scope reads as still live: the instrument errs towards
//! finding the account short, never towards hiding it.
//!
//! What it answers is the reconciliation of `crate::statistics`'
//! account against the heap (`docs/design/decisions.md`, "D81"): at every
//! update the account's total is compared with the live statistics bytes this
//! module counts, and the worst shortfall is kept.

#[cfg(feature = "introspect")]
pub use enabled::{
    STATISTICS_SLACK_PER_MILLE, StatisticsReading, allocated, freed, statistics_reading,
};

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

/// Compare the account's new `total` with the live statistics bytes, keeping
/// the worst shortfall. Nothing without the instrument.
#[inline]
pub(crate) fn statistics_account_updated(total: u64) {
    #[cfg(feature = "introspect")]
    enabled::account_updated(total);
    #[cfg(not(feature = "introspect"))]
    let _ = total;
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
    /// The largest `live − total` any update read, and the live bytes then.
    static SHORTFALL: AtomicU64 = AtomicU64::new(0);
    static SHORTFALL_LIVE: AtomicU64 = AtomicU64::new(0);
    /// The largest `live − total − live × SLACK` any update read.
    static SHORTFALL_PAST_SLACK: AtomicU64 = AtomicU64::new(0);

    /// The proportional half of the shortfall the account is allowed at an
    /// update, per mille of the live statistics bytes then: what
    /// [`StatisticsReading::shortfall_past_slack`] is measured past.
    pub const STATISTICS_SLACK_PER_MILLE: u64 = 20;

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

    pub(super) fn account_updated(total: u64) {
        CHECKS.fetch_add(1, Ordering::Relaxed);
        let live = LIVE.load(Ordering::Relaxed);
        let short = live.saturating_sub(total);
        if short > SHORTFALL.fetch_max(short, Ordering::Relaxed) {
            SHORTFALL_LIVE.store(live, Ordering::Relaxed);
        }
        let past = short.saturating_sub(live * STATISTICS_SLACK_PER_MILLE / 1000);
        SHORTFALL_PAST_SLACK.fetch_max(past, Ordering::Relaxed);
    }

    /// What the instrument has counted so far.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct StatisticsReading {
        /// Statistics bytes live now.
        pub live: u64,
        /// The most statistics bytes ever live at once.
        pub live_peak: u64,
        /// Account updates compared.
        pub checks: u64,
        /// The largest amount any update found the account short of live.
        pub shortfall: u64,
        /// The live statistics bytes at that update.
        pub shortfall_live: u64,
        /// The largest amount any update found the account short past
        /// [`STATISTICS_SLACK_PER_MILLE`] of live.
        pub shortfall_past_slack: u64,
    }

    /// Read the counters. Racy against a thread still allocating, which the
    /// caller avoids by reading once the pass has returned.
    pub fn statistics_reading() -> StatisticsReading {
        StatisticsReading {
            live: LIVE.load(Ordering::Relaxed),
            live_peak: LIVE_PEAK.load(Ordering::Relaxed),
            checks: CHECKS.load(Ordering::Relaxed),
            shortfall: SHORTFALL.load(Ordering::Relaxed),
            shortfall_live: SHORTFALL_LIVE.load(Ordering::Relaxed),
            shortfall_past_slack: SHORTFALL_PAST_SLACK.load(Ordering::Relaxed),
        }
    }
}
