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
//! update the live statistics bytes this module counts are compared with the
//! account's total, and with that total less what it carries for allocations
//! charged ahead — made or not yet, on any thread — and the worst shortfall of
//! the first and excess of the second are kept, raw and past the allowance the
//! account gives its open observers' uncharged growth then.

#[cfg(feature = "introspect")]
pub use enabled::{StatisticsReading, allocated, freed, statistics_reading};

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
    /// The largest `live − account` and `account − live` any update read, and
    /// each past the allowance then.
    static SHORT: AtomicU64 = AtomicU64::new(0);
    static SHORT_PAST_ALLOWANCE: AtomicU64 = AtomicU64::new(0);
    static OVER: AtomicU64 = AtomicU64::new(0);
    static OVER_PAST_ALLOWANCE: AtomicU64 = AtomicU64::new(0);

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

    /// What the instrument has counted so far.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct StatisticsReading {
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
        /// The largest amount any update found the account over live.
        pub over: u64,
        /// The largest amount any update found it over past its allowance.
        pub over_past_allowance: u64,
    }

    /// Read the counters. Racy against a thread still allocating, which the
    /// caller avoids by reading once the pass has returned.
    pub fn statistics_reading() -> StatisticsReading {
        StatisticsReading {
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
