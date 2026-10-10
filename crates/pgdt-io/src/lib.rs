//! Byte sources — local, `.xz`, fetched, remote — the memory limit and its
//! discovery, and worker sizing (`docs/design/decisions.md`, "D68"). The
//! `http` feature is this crate's.

mod io;

pub use io::{
    ByteRangeSource, Cancellation, DEFAULT_MEMORY_BUDGET, FetchedXzSource, KnownCompression,
    LocalFileSource, MEMORY_MARGIN_PERCENT, MEMORY_RESERVE, MEMORY_UNPOOLED_BOUND, MemoryLimit,
    Origin, OriginProbe, Parallelism, PartitionBoundaries, PartitionRead, Partitioning, Recognized,
    RemoteIdentity, RetainedUnit, SCAN_CHUNK_DEFAULT_SIZE_BYTES, WaitPolicy, WorkerMemory,
    XzSource, available_memory, available_memory_in, discover_memory_limit,
    discover_memory_limit_in, open, open_local, statistics_allowance, walk_seek_table,
};
#[cfg(feature = "http")]
pub use io::{REMOTE_READ_TIMEOUT, RemoteSource, open_remote};

/// What a crate above reaches that is not this crate's API: public only
/// because a crate boundary forced it, and named here so a caller reaching
/// past the API names the word.
pub mod internal {
    pub use crate::io::{cut, is_weak_tag, memory_budget_display, worker_count};
}
