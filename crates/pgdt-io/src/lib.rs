//! The byte sources — local, compressed, fetched, remote — and memory and
//! parallelism (`docs/design/decisions.md`, "D68"). The `http` feature is
//! this crate's.
//!
//! Two concepts, one tree each: the byte sources, `source` and the arms
//! nested under its read contract; and memory and parallelism — the allowance
//! and its discovery (`memory`), its resolution into what a pass runs under
//! (`arrangement`), the arrangement and what a worker holds (`parallelism`),
//! what a run's held lines may draw past their chunk term (`ledger`), the
//! physical cores a process may run on (`cores`), and where a range is
//! cut (`partition`).

#![forbid(unsafe_code)]

mod arrangement;
mod cores;
mod ledger;
mod memory;
mod parallelism;
mod partition;
mod source;
#[cfg(test)]
mod test_support;

pub use arrangement::{Allowance, Arrangement, ArrangementRequest, Jobs};
pub use ledger::{Admission, Floor, Holder, Ledger};
pub use memory::{DEFAULT_MEMORY_BUDGET, MemoryLimit, available_memory, discover_memory_limit};
pub use parallelism::{Parallelism, WorkerMemory};
pub use partition::{PartitionBoundaries, PartitionRead, Partitioning, RetainedUnit};
pub use source::{
    ByteRangeSource, CompressedFileSource, CompressionIndex, Extent, FetchedCompressedSource,
    GzipIndex, KnownCompression, LocalFileSource, Origin, OriginProbe, ParkedReader, Recognized,
    RemoteIdentity, SCAN_CHUNK_DEFAULT_SIZE_BYTES, SeekTable, WaitPolicy, WindowProvider, open,
};
#[cfg(feature = "http")]
pub use source::{REMOTE_READ_TIMEOUT, RemoteSource};

/// What a crate above reaches that is not this crate's API: public only
/// because a crate boundary forced it, and named here so a caller reaching
/// past the API names the word.
///
/// The memory constants a test reasons in are here and no embedder's;
/// discovery against a root a test built is this crate's own
/// (`docs/design/decisions.md`, "D11").
pub mod internal {
    /// An arrangement of a parallelism a test states, at any budget, under
    /// no allowance: what a test above this crate plans a replay under.
    #[cfg(feature = "test-support")]
    pub use crate::arrangement::arrangement_of;
    pub use crate::memory::{MEMORY_MARGIN_PERCENT, MEMORY_RESERVE, MEMORY_UNPOOLED_BOUND};
    pub use crate::parallelism::{memory_budget_display, worker_count};
    pub use crate::partition::cut;
    /// A gzip file's complete index, built by a forward pass over the file
    /// held in memory at a spacing the test states, its windows held beside
    /// it: what a test above this crate builds an index through.
    #[cfg(feature = "test-support")]
    pub use crate::source::gzip_indexed_in_memory;
    pub use crate::source::is_weak_tag;
}
