//! What the library can tell an instrumented binary about its own work: which
//! allocations are statistics, how often a read began decoding a block, and
//! where a dynamic filter's row evaluation spends its time.
//!
//! **Off by default and compiled out of every build that does not ask.** The
//! `introspect` feature turns it on, and only a binary's own `introspect`
//! feature does — `pgdt`'s, which turns on `datafusion-cli-pgdump`'s for
//! `sql` (`docs/design/decisions.md`, "D13"): without it, `StatisticsScope` is
//! an empty guard, [`timed!`](crate::timed) is what it wraps beside its part
//! evaluated and dropped, [`row_evaluated!`](crate::row_evaluated) what it
//! wraps and nothing more, and every function recording or reading a figure
//! either absent or doing nothing.
//! The library still installs no allocator; the binary's counting allocator
//! calls `allocated` and `freed`, and this module decides what counts.
//!
//! Each child is one concern whole: what an instrumented binary reads, which
//! this module re-exports, and the hooks a crate above calls to record it,
//! which `internal` re-exports — `decode`'s count, which a test reads rather
//! than a binary, among them.

pub(crate) mod decode;
pub(crate) mod evaluation;
pub(crate) mod statistics;

pub use evaluation::EvaluationPart;
#[cfg(feature = "introspect")]
pub use evaluation::{EvaluationReading, evaluation_reading};
#[cfg(feature = "introspect")]
pub use statistics::{StatisticsReading, allocated, freed, statistics_reading};
