//! The hooks the library calls to record its own work, and the macros
//! wrapping a span in one: not `instrument`'s API, which is what an
//! instrumented binary reads, but what a crate above calls, so `internal`
//! re-exports them.

#[cfg(feature = "introspect")]
use super::{EvaluationPart, enabled, evaluation};

/// `$body`, timed as `$part` where the instrument is built in — a leaf part
/// only inside [`row_evaluated!`]. **Without the instrument it expands to
/// `$body`, `$part` evaluated and dropped beside it**, so a shipped build compiles what it wraps
/// exactly as written; an inlined closure is not enough to promise that.
/// With it, `$body` runs in a closure passed to [`timed_span`], which the
/// calling module imports under the feature. The `cfg` is read in the crate
/// expanding it, so each such crate has an `introspect` turning on this one's.
#[macro_export]
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

/// `$body`, one row's evaluation against a dynamic filter's state, timed as
/// [`EvaluationPart::Row`](super::EvaluationPart::Row) with every leaf part it reaches, where the
/// instrument is built in, through [`evaluated_row`]; without it, `$body`
/// alone, as [`timed!`] is.
#[macro_export]
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

/// What [`timed!`] runs its body through with the instrument built in.
#[cfg(feature = "introspect")]
#[inline(always)]
pub fn timed_span<T>(part: EvaluationPart, f: impl FnOnce() -> T) -> T {
    evaluation::timed(part, f)
}

/// What [`row_evaluated!`] runs its body through with the instrument built in.
#[cfg(feature = "introspect")]
#[inline(always)]
pub fn evaluated_row<T>(f: impl FnOnce() -> T) -> T {
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
pub struct DecodeCounter {
    #[cfg(feature = "introspect")]
    begun: std::sync::atomic::AtomicU64,
}

impl DecodeCounter {
    /// A block is about to be begun rather than resumed. Counted where that
    /// is decided, so a `begin` that then errors is counted.
    #[inline]
    pub fn begun(&self) {
        #[cfg(feature = "introspect")]
        self.begun.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// How many have. Racy against a read still running, which the caller
    /// avoids by reading once the read has returned.
    #[cfg(feature = "introspect")]
    pub fn count(&self) -> u64 {
        self.begun.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Attributes every allocation and free the current thread makes to
/// statistics while it is alive. Scopes nest.
pub struct StatisticsScope {
    #[cfg(feature = "introspect")]
    outer: bool,
}

impl StatisticsScope {
    #[inline]
    pub fn enter() -> Self {
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
/// walk [`pgdt_model::DumpIndex::statistics_heap_bytes`] does. Nothing without the
/// instrument, and the call sites are `cfg`'d, so a shipped build does not walk
/// the index for it.
///
/// **It is the only statistics term a `query` has.** A query loads a cache and
/// keeps no `pgdt_mapping`'s `StatisticsAccount`, so nothing else says what
/// the cache handed it.
#[cfg(feature = "introspect")]
pub fn statistics_loaded(bytes: u64) {
    enabled::statistics_loaded(bytes);
}

/// Compare the account's new `total`, `announced` of it charged ahead of
/// allocations, with the live statistics bytes, keeping the worst difference
/// either way past `allowance`, the uncharged growth its open observers may
/// hold. Nothing without the instrument.
#[inline]
pub fn statistics_account_updated(total: u64, announced: u64, allowance: u64) {
    #[cfg(feature = "introspect")]
    enabled::account_updated(total, announced, allowance);
    #[cfg(not(feature = "introspect"))]
    let _ = (total, announced, allowance);
}
