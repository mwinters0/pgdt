//! Every byte source and what a reader of one is told: the read contract and
//! its vocabulary, the plain file, the `.xz` file pulled or fetched, the remote
//! object, the origin a `--source` argument resolves to, the memory allowance
//! and its discovery, worker sizing, and cancellation. One file a concern; this
//! one names what the crate exports.

mod blocks;
mod cancel;
mod fetched;
mod local;
mod memory;
mod origin;
mod parallelism;
#[cfg(feature = "http")]
mod remote;
mod source;
mod xz;

pub use self::cancel::Cancellation;
pub use self::fetched::FetchedXzSource;
pub use self::local::LocalFileSource;
pub use self::memory::{
    DEFAULT_MEMORY_BUDGET, MEMORY_MARGIN_PERCENT, MEMORY_RESERVE, MEMORY_UNPOOLED_BOUND,
    MemoryLimit, available_memory, available_memory_in, discover_memory_limit,
    discover_memory_limit_in, statistics_allowance,
};
pub use self::origin::{KnownCompression, Origin, OriginProbe, Recognized, open, open_local};
pub use self::parallelism::{
    Parallelism, Partitioning, WorkerMemory, cut, memory_budget_display, worker_count,
};
#[cfg(feature = "http")]
pub use self::remote::{REMOTE_READ_TIMEOUT, RemoteSource, open_remote};
pub use self::source::{
    ByteRangeSource, PartitionBoundaries, PartitionRead, RemoteIdentity, RetainedUnit,
    SCAN_CHUNK_DEFAULT_SIZE_BYTES, WaitPolicy, is_weak_tag,
};
pub use self::xz::{XzSource, walk_seek_table};

#[cfg(test)]
mod tests;
