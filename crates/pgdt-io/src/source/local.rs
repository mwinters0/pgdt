//! The local file source: positioned reads into the buffer pool.

use std::future::Future;
use std::ops::Range;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;

use pgdt_core::{Error, Result};

use crate::parallelism::Parallelism;
use crate::partition::Partitioning;

use super::pool::{BufferPool, POOL_DEPTH, POOL_MAX_BYTES, PooledBuffer, budget_bytes};
use super::{ByteRangeSource, Extent, WaitPolicy};

/// How many read chunks a plain file's partition holds
/// ([`LocalFileSource::partitions`]).
///
/// **A partition is not one chunk, because a worker reads its piece and then
/// one chunk more**: `pgdt_mapping::interior::scan_region` reads `[start, end)` and
/// a chunk-sized tail read follows to finish the line ending at or past `end`,
/// so those bytes are the next partition's body read a second time
/// (`docs/design/decisions.md`, "D52"). The waste is `chunk / partition`, paid
/// per partition, so this multiple caps it at the multiple's reciprocal
/// (`docs/design/decisions.md`, "D9").
///
/// **The product is capped at [`POOL_MAX_BYTES`].** Nothing allocates a
/// partition-length buffer, so what the cap bounds is the **cut** — and,
/// because this number is also [`Partitioning::partition_bytes`], what a
/// budget is solved against (`KD25`). The multiple therefore shrinks as the
/// announced chunk grows, reaching **one** at [`POOL_MAX_BYTES`] and above.
///
/// **The shipped default sits exactly on the cap** — this multiple of
/// [`crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES`] is [`POOL_MAX_BYTES`] — and the two constants
/// are justified independently, so
/// [`a_shipped_plain_partition_is_eight_whole_chunks`] is what stops a later
/// change to either from silently capping the default configuration.
/// *Rejected:* deriving this from [`POOL_MAX_BYTES`].
pub(crate) const PLAIN_PARTITION_CHUNKS: usize = 8;

/// A local file, read via blocking positioned reads on a `spawn_blocking`
/// task (`tokio` has no native async positioned-read). See
/// `docs/design/decisions.md`, "D10".
pub struct LocalFileSource {
    path: PathBuf,
    file: Arc<std::fs::File>,
    pool: Arc<BufferPool>,
}

impl LocalFileSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = std::fs::File::open(&path)?;
        Ok(Self { path, file: Arc::new(file), pool: Arc::new(BufferPool::default()) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl ByteRangeSource for LocalFileSource {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        Box::pin(async move {
            let file = Arc::clone(&self.file);
            let pool = Arc::clone(&self.pool);
            // A read that fails drops its buffer, whose `Drop` releases it to
            // the pool as any holder's does.
            //
            // **The slot is taken inside the blocking task, not before it.**
            // `obtain` blocks for a loop that granted the wait, and blocking the
            // runtime thread that is meant to be driving the sibling reads which
            // would free a slot is the one way to turn that wait into a
            // deadlock.
            let slot = tokio::task::spawn_blocking(move || -> std::io::Result<PooledBuffer> {
                let mut slot = pool.obtain(len);
                file.read_exact_at(&mut slot.as_mut()[..len], offset)?;
                Ok(slot)
            })
            .await
            .map_err(Error::join)?
            .map_err(Error::from)?;
            Ok(Bytes::from_owner(slot).slice(..len))
        })
    }

    fn extent(&self) -> Pin<Box<dyn Future<Output = Result<Extent>> + Send + '_>> {
        Box::pin(async move {
            let file = Arc::clone(&self.file);
            let len = tokio::task::spawn_blocking(move || file.metadata().map(|m| m.len()))
                .await
                .map_err(Error::join)?
                .map_err(Error::from)?;
            Ok(Extent::Exact(len))
        })
    }

    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        Box::pin(async move {
            let file = Arc::clone(&self.file);
            let mtime =
                tokio::task::spawn_blocking(move || file.metadata().and_then(|m| m.modified()))
                    .await
                    .map_err(Error::join)?
                    .map_err(Error::from)?;
            Ok(Some(mtime))
        })
    }

    /// **The pool's one non-size rule.** A read loop announcing its chunk size
    /// is what lets a buffer of that length be kept above [`POOL_MAX_BYTES`],
    /// so a raised `ReadOptions::chunk_size_bytes` keeps its pooling instead of
    /// paying a fresh `calloc` per chunk. The cost is bounded and is the
    /// caller's own number: [`BufferPool::slots`] buffers of the size it asked
    /// for.
    fn hint_read_size(&self, len: usize) {
        self.pool.hint(len);
    }

    /// **Anywhere, [`PLAIN_PARTITION_CHUNKS`] read chunks each.** A positioned
    /// read costs the same at every offset, so nothing about this source
    /// prefers one split point to another.
    ///
    /// The unit it is a multiple of is the pool's own slot size — the length a
    /// read loop announced, or the ceiling where none has
    /// ([`BufferPool::slot_bytes`]) — so a chunk already at or past
    /// [`POOL_MAX_BYTES`] is a partition on its own.
    ///
    /// **It is the cut size, and not what a partition holds.**
    /// `pgdt_mapping::interior::scan_region` reads a [`PartitionRead::Chunked`](crate::PartitionRead::Chunked)
    /// piece one announced chunk at a time and the interior split caps what is
    /// outstanding at [`BufferPool::slots`], so this source holds
    /// `POOL_DEPTH` chunks flat in the reader count while
    /// [`Partitioning::partition_bytes`] bills [`PLAIN_PARTITION_CHUNKS`] of
    /// them per reader (`docs/design/decisions.md`, "D4").
    // Deficiency register: `deficiency: KD25` — so plain readers are bounded
    // by a charge describing nothing the path holds: the bill grows with the
    // count while what is held is flat at `POOL_DEPTH` chunks, and above a
    // budget of that bill times `jobs` nothing bounds them at all. **(c) unowned.** Closing it means billing the pool's real depth and
    // recommending what a reader costs; a reading of a parallel plain scan on a real
    // device is what reopens it.
    fn partitions(&self, _range: Range<u64>) -> Partitioning {
        let chunk = self.pool.slot_bytes();
        let bytes = chunk.saturating_mul(PLAIN_PARTITION_CHUNKS).min(POOL_MAX_BYTES).max(chunk);
        Partitioning::anywhere(bytes as u64)
    }

    /// **The budget sizes the free list; the worker count does not.** This
    /// source has one read unit, and a chunk buffer is taken by one
    /// `read_range` and returned when its caller drops the bytes — so what the
    /// free list has to hold is the
    /// *replay* path's depth, which is what [`POOL_DEPTH`] is, and not a
    /// worker count (`docs/design/decisions.md`, "D9").
    ///
    /// Where the leader schedules concurrent readers over this source, a
    /// `parse` at `--jobs n` runs `n` fused workers against at most
    /// [`POOL_DEPTH`] chunk slots, so above that many the extra workers block for a slot
    /// rather than allocating.
    fn hint_parallelism(&self, parallelism: Parallelism) {
        self.pool.set_limits(budget_bytes(parallelism), POOL_DEPTH);
    }

    /// **One read unit, so one policy to state.** A loop that permits a wait
    /// gets one: its reads block for a free chunk buffer rather than
    /// allocating past the stated budget. A loop that does not permit one
    /// allocates, because the buffers it is holding may be the only ones that
    /// could free a slot.
    fn hint_wait_policy(&self, policy: WaitPolicy) {
        self.pool.set_policy(policy);
    }
}

#[cfg(test)]
mod tests;
