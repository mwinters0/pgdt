//! The byte sources — local, `.xz`, fetched, remote — memory and
//! parallelism, and stopping (`docs/design/decisions.md`, "D68"). The `http`
//! feature is this crate's.
//!
//! Three concepts, one tree each: the byte sources, `source` and the arms
//! nested under its read contract; memory and parallelism — the allowance
//! and its discovery (`memory`), its resolution into what a pass runs under
//! (`arrangement`), the arrangement and what a worker holds (`parallelism`),
//! and where a range is cut (`partition`); and stopping, `cancel`.

mod arrangement;
mod cancel;
mod memory;
mod parallelism;
mod partition;
mod source;
#[cfg(test)]
mod test_support;

pub use arrangement::{Allowance, Arrangement, ArrangementRequest, Jobs};
pub use cancel::Cancellation;
pub use memory::{DEFAULT_MEMORY_BUDGET, MemoryLimit, available_memory, discover_memory_limit};
pub use parallelism::{Parallelism, WorkerMemory};
pub use partition::{PartitionBoundaries, PartitionRead, Partitioning, RetainedUnit};
pub use source::{
    ByteRangeSource, FetchedXzSource, KnownCompression, LocalFileSource, Origin, OriginProbe,
    Recognized, RemoteIdentity, SCAN_CHUNK_DEFAULT_SIZE_BYTES, SeekTable, WaitPolicy, XzSource,
    open,
};
#[cfg(feature = "http")]
pub use source::{REMOTE_READ_TIMEOUT, RemoteSource};

/// What a crate above reaches that is not this crate's API: public only
/// because a crate boundary forced it, and named here so a caller reaching
/// past the API names the word.
///
/// The memory constants a test reasons in, and discovery against a root a
/// test built, are here and no embedder's (`docs/design/decisions.md`,
/// "D11").
pub mod internal {
    pub use crate::arrangement::discover_allowance_in;
    pub use crate::memory::{
        MEMORY_MARGIN_PERCENT, MEMORY_RESERVE, MEMORY_UNPOOLED_BOUND, available_memory_in,
        discover_memory_limit_in,
    };
    pub use crate::parallelism::{memory_budget_display, worker_count};
    pub use crate::partition::cut;
    pub use crate::source::is_weak_tag;
}
