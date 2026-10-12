//! The `.xz` source over a local file: a block decoded whole where the
//! budget holds it, piecewise past it.

use std::future::Future;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bytes::Bytes;

use pgdt_core::{Error, Result};

use crate::parallelism::{Parallelism, WorkerMemory};
use crate::partition::Partitioning;
use crate::source::pool::BufferPool;
use crate::source::{ByteRangeSource, WaitPolicy};

use super::blocks::{BlockCache, BlockView, DecodedBlock, XzBudget};
use super::{LiveBlock, piece_of, xz_short_read};

/// A `ByteRangeSource` decoding an `.xz`-compressed local file on the fly
/// (`docs/design/decisions.md`, "D14", "D15").
///
/// **One file handle, positioned reads only.** Nothing here holds a cursor:
/// `stat` answers
/// [`ByteRangeSource::stored_size`]/[`ByteRangeSource::modified`], and every
/// decode reads its compressed bytes through `pread`, so the handle is shared
/// by everything and disturbs nothing. It is what `xz_seek` pulls from —
/// `std::fs::File` is a `CompressedSource` — which is where this source and
/// [`FetchedXzSource`](crate::FetchedXzSource) part, whose bytes have to be fetched into a window
/// first (`docs/design/decisions.md`, "D15").
///
/// **Two arms, and the budget picks between them**, as the fetched source's do:
///
/// - Where the stated budget holds a whole decoded block, a read decodes the
///   blocks it lands in into slots of its own [`BlockCache`] and slices the
///   answer out of them. `xz_seek::BlockTask` is `Copy` and owns everything a
///   decode needs, and an `xz_seek::Layout` names one without a lock, so no
///   lock is held across a decode; what this arm does take is its own
///   [`BlockCache`]'s retention list.
/// - Where it does not, the block is read **in pieces** through
///   `xz_seek::BlockRead` — one live handle, kept across reads so a forward
///   scan continues rather than restarting, which is what
///   `xz_seek::Reader::read_at` did for this path before ([`LiveBlock`]).
///
/// `Verify::Full` is `xz_seek::Builder::new`'s own default, so nothing here has
/// to ask for it; a whole-block decode compares the check before it returns,
/// and the piecewise arm compares it when it leaves the block, or as the run
/// ends for the block a read left it inside
/// ([`ByteRangeSource::complete_reads`]).
pub struct XzSource {
    path: PathBuf,
    /// The file, read positionally and never seeked: `stat` and every decode
    /// share it, and `Arc<std::fs::File>` is itself an
    /// `xz_seek::CompressedSource`.
    file: Arc<std::fs::File>,
    /// The table, the memory limit, the verification state and the backend as
    /// one value — everything a decode needs that is not bytes, and the only
    /// thing that hands out an `xz_seek::BlockTask`. It holds no source and
    /// needs no lock (`docs/design/decisions.md`, "D14").
    layout: xz_seek::Layout,
    /// The layout's own table, aliased rather than copied, so `size()`,
    /// `seek_table()` and the per-read `blocks_in` lookup read one table.
    table: Arc<xz_seek::SeekTable>,
    /// The live piecewise decode, or `None` where no read has taken that arm
    /// yet, its last decode failed or `complete_reads` has left it. Behind a `Mutex` because a handle goes forward only and one
    /// position cannot serve two readers — which is why that arm advises a
    /// single partition ([`xz_partition_advice`](super::blocks::xz_partition_advice)).
    live: Arc<Mutex<Option<LiveBlock>>>,
    /// What the caller announced and everything derived from it: the two
    /// pools, which arm a read takes, and what a reader of this file costs.
    /// [`FetchedXzSource`](crate::FetchedXzSource) holds the same value ([`XzBudget`]).
    pub(super) budget: XzBudget,
}

impl XzSource {
    /// Open `path` as `.xz`-compressed input, walking its stream footers to
    /// build the seek table before this call returns.
    ///
    /// This does not sniff the magic bytes; content-sniffing recognition
    /// across both source kinds is [`open_local`](crate::open_local).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        // The walk this names is the one [`XzSource::with_table`] exists to
        // skip, so only this constructor emits it — a cached table costs no
        // footer read and earns no line
        // (`docs/design/decisions.md`, "D64").
        tracing::info!(path = %path.display(), "seek table build started");
        let file = Arc::new(std::fs::File::open(&path)?);
        let stored_size = file.metadata()?.len();
        let table = xz_seek::SeekTable::from_source(&file).map_err(Error::xz)?;
        tracing::info!(
            path = %path.display(),
            streams = table.stream_count(),
            blocks = table.block_count(),
            "seek table build complete",
        );
        Self::over(path, file, table, stored_size)
    }

    /// Open `path` from a seek table a previous walk of the *same* file
    /// produced, **without walking it again**
    /// (`docs/design/decisions.md`, "D18").
    ///
    /// `xz_seek` validates the table for internal consistency and against the
    /// file's own length before trusting it, reading **no bytes** — so a
    /// table that does not describe this file costs nothing to reject, and a
    /// block header's own CRC32 is what catches one that survives validation.
    /// A rejected table is an `Error::Xz` carrying `xz_seek::Error::InvalidTable`,
    /// which is the error [`open_local`](crate::open_local) turns into [`Recognized::Mismatch`](crate::Recognized::Mismatch)
    /// rather than a walk.
    pub fn with_table(path: impl AsRef<Path>, table: xz_seek::SeekTable) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = Arc::new(std::fs::File::open(&path)?);
        let stored_size = file.metadata()?.len();
        Self::over(path, file, table, stored_size)
    }

    /// The one place the two constructors agree: the table becomes a
    /// sourceless `xz_seek::Layout`, the source's own copy is aliased out of
    /// it, and the budget policy is built over that table ([`XzBudget`]).
    ///
    /// **`Builder::new()` rather than a configured one**, and both
    /// constructors go through it, so a walked file and a cached one decode
    /// identically — `Verify::Full` included. `stored_size` is the compressed
    /// file's own length, which is all `Builder::layout` ever wanted a source
    /// for: it cross-checks the table against the file it claims to describe
    /// and reads **no bytes**, so a table that does not describe this file is
    /// an `Error::Xz` carrying `xz_seek::Error::InvalidTable` — the error
    /// [`open_local`](crate::open_local) turns into [`Recognized::Mismatch`](crate::Recognized::Mismatch) rather than a walk.
    ///
    /// **What the alias buys is every table read with no lock at all** —
    /// [`ByteRangeSource::size`], [`XzBudget::partitions`],
    /// [`XzSource::default_workers`], the `blocks_in` span arithmetic
    /// [`XzSource::read_by_blocks`] does on every read, and naming the
    /// `BlockTask` a cache miss decodes. It costs no second copy:
    /// `xz_seek::Layout::index_shared` hands out the `Arc` the layout itself
    /// holds.
    ///
    /// **Nothing bills the one copy that is left.** It is unbilled on an
    /// ordering rather than a size: the walk that builds it runs inside
    /// [`XzSource::open`], before any budget is announced
    /// (`docs/design/decisions.md`, "D4").
    // Deficiency register: `deficiency: KD26` — this table is the only term in
    // the memory account that grows with the *file* rather than with the
    // worker count, and a source whose index is not small beside
    // [`MEMORY_UNPOOLED_BOUND`] therefore holds bytes no charge names.
    // **(c) unowned**; closing it means a per-source term, which
    // [`WorkerMemory`] does not have. Billing it buys accuracy and no
    // protection, the walk having already happened.
    fn over(
        path: PathBuf,
        file: Arc<std::fs::File>,
        table: xz_seek::SeekTable,
        stored_size: u64,
    ) -> Result<Self> {
        let layout = xz_seek::Builder::new().layout(table, stored_size).map_err(Error::xz)?;
        let table = layout.index_shared();
        // **The decoder charge includes the compressed input chunk because
        // this source does not lend**: a `std::fs::File` holds nothing, so
        // every decode here builds that buffer, where [`FetchedXzSource`]
        // reads out of bytes it already has and charges `decoder_bytes` plus
        // the window instead. It is what `decode_footprint` answers.
        let budget = XzBudget::over(&table, layout.decode_footprint());
        Ok(Self { path, file, layout, table, live: Arc::new(Mutex::new(None)), budget })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The block at `index`, from the retention list or freshly decoded into a
    /// slot of the block pool.
    ///
    /// Deficiency register: `deficiency: KD20` — two concurrent misses on one
    /// block decode it twice, and on the parallel scan path that is the common
    /// case: every fused worker's chunk-sized tail read lands in the block its
    /// *successor* owns, so a scan does about twice the decode work and its
    /// speedup is capped near half the reader count (`measurements.md`,
    /// `parallel-scan-throughput`'s `.xz` `parse` column). **(c) unowned.** The fix left is an
    /// in-flight map here — not taken, since it puts a second lock in front of
    /// the case that does *not* collide — a wider cut having been measured and
    /// refused ([`BOUNDARIED_PARTITION_UNITS`](crate::partition::BOUNDARIED_PARTITION_UNITS)).
    fn block(
        index: usize,
        cache: &BlockCache,
        layout: &xz_seek::Layout,
        file: &Arc<std::fs::File>,
    ) -> Result<Arc<DecodedBlock>> {
        if let Some(hit) = cache.lookup(index) {
            return Ok(hit);
        }
        // No lock: a `BlockTask` is `Copy` and owns its block, its check and
        // the layout's decode settings, and a `Layout` names one from the table
        // alone.
        let task = layout.block_task(index).ok_or_else(xz_short_read)?;
        let len = usize::try_from(task.uncompressed_len()).map_err(|_| xz_short_read())?;
        // A slot is the file's largest block, so `len` is never above it and a
        // released slot is exactly the length the pool was hinted with.
        let mut slot = cache.slot();
        // A decode that fails drops its slot, which `Drop` releases to the
        // pool as the plain source's failed read does.
        task.decode_into(file, &mut slot.as_mut()[..len]).map_err(Error::xz)?;
        let block = Arc::new(DecodedBlock { slot, len });
        cache.retain(index, Arc::clone(&block));
        Ok(block)
    }

    /// A read served by decoding the blocks it lands in.
    ///
    /// A read inside one block is a **slice of that block**, so the common
    /// case copies nothing; one that straddles a boundary is assembled into a
    /// chunk-pool buffer.
    fn read_by_blocks(
        offset: u64,
        len: usize,
        table: &xz_seek::SeekTable,
        cache: &BlockCache,
        layout: &xz_seek::Layout,
        file: &Arc<std::fs::File>,
        chunks: &Arc<BufferPool>,
    ) -> Result<Bytes> {
        let end = offset.checked_add(len as u64).ok_or_else(xz_short_read)?;
        let covering = table.blocks_in(offset..end);
        if covering.is_empty() {
            return Err(xz_short_read());
        }
        if covering.len() == 1 {
            let index = covering.start;
            let block = Self::block(index, cache, layout, file)?;
            let base = table.blocks[index].uncompressed_offset;
            // `blocks_in` answers the block *covering* the start, so `base` is
            // at or below `offset`; the checked form is what keeps a table
            // that says otherwise a refusal rather than a panic.
            let start = offset
                .checked_sub(base)
                .and_then(|at| usize::try_from(at).ok())
                .ok_or_else(xz_short_read)?;
            if start + len > block.len {
                return Err(xz_short_read());
            }
            return Ok(Bytes::from_owner(BlockView(block)).slice(start..start + len));
        }
        let mut out = chunks.obtain(len);
        let mut covered = 0usize;
        for index in covering {
            let block = Self::block(index, cache, layout, file)?;
            let base = table.blocks[index].uncompressed_offset;
            let Some((at, from, n)) = piece_of(base..base + block.len as u64, offset, end) else {
                continue;
            };
            let within = (from - base) as usize;
            out.as_mut()[at..at + n].copy_from_slice(&block.bytes()[within..within + n]);
            covered += n;
        }
        if covered != len {
            return Err(xz_short_read());
        }
        Ok(Bytes::from_owner(out).slice(..len))
    }

    /// A read served **without holding a whole decoded block**: the covering
    /// blocks are read in pieces through one live `xz_seek::BlockRead`, which
    /// holds the decoder and the compressed input chunk and never the
    /// plaintext.
    ///
    /// It is what a file whose largest block does not fit the stated budget is
    /// read through ([`BlockCache::affordable`]), where decoding a block whole
    /// would allocate more than the caller allowed — up to the whole file, on a
    /// single-block one — and it is the same mechanism
    /// [`FetchedXzSource::read_in_pieces`](crate::FetchedXzSource::read_in_pieces) drives, with a `File` in place of a
    /// fetched window (`docs/design/decisions.md`, "D15").
    ///
    /// **The handle is kept across reads**, which is what makes a forward scan
    /// cost one decode of the file rather than one per read ([`LiveBlock`]).
    /// The mutex is therefore held across the decode, and one position cannot
    /// serve two readers — the single partition [`xz_partition_advice`](super::blocks::xz_partition_advice)'s other
    /// arm advises.
    fn read_in_pieces(
        offset: u64,
        len: usize,
        table: &xz_seek::SeekTable,
        layout: &xz_seek::Layout,
        file: &Arc<std::fs::File>,
        live: &Mutex<Option<LiveBlock>>,
        chunks: &Arc<BufferPool>,
    ) -> Result<Bytes> {
        let end = offset.checked_add(len as u64).ok_or_else(xz_short_read)?;
        let covering = table.blocks_in(offset..end);
        if covering.is_empty() {
            return Err(xz_short_read());
        }
        let mut out = chunks.obtain(len);
        let mut live = live.lock().unwrap_or_else(|e| e.into_inner());
        let mut covered = 0usize;
        for index in covering {
            let task = layout.block_task(index).ok_or_else(xz_short_read)?;
            let Some((at, from, n)) = piece_of(task.uncompressed_range(), offset, end) else {
                continue;
            };
            // A failed decode leaves no state the next read can reason about,
            // exactly as `xz_seek::Reader::read_at` drops its live decode on an
            // error — and the handle is dropped rather than completed, there
            // being nothing to verify against.
            let filled = match LiveBlock::serve(
                &mut live,
                index,
                &task,
                file,
                from,
                &mut out.as_mut()[at..at + n],
            ) {
                Ok(filled) => filled,
                Err(e) => {
                    *live = None;
                    return Err(e);
                }
            };
            covered += filled;
        }
        drop(live);
        if covered != len {
            return Err(xz_short_read());
        }
        Ok(Bytes::from_owner(out).slice(..len))
    }
}

impl ByteRangeSource for XzSource {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        Box::pin(async move {
            // A zero-length read covers no blocks, so it is answered here
            // rather than left to look like a read past the end.
            if len == 0 {
                return Ok(Bytes::new());
            }
            let chunks = Arc::clone(&self.budget.pool);
            let table = Arc::clone(&self.table);
            let layout = self.layout.clone();
            let file = Arc::clone(&self.file);
            let live = Arc::clone(&self.live);
            let blocks = self.budget.block_path().cloned();
            tokio::task::spawn_blocking(move || match blocks {
                Some(cache) => {
                    Self::read_by_blocks(offset, len, &table, &cache, &layout, &file, &chunks)
                }
                None => Self::read_in_pieces(offset, len, &table, &layout, &file, &live, &chunks),
            })
            .await
            .map_err(Error::join)?
        })
    }

    /// The uncompressed length, from the seek table built at `open` — no
    /// further decode, no I/O, and no lock.
    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        Box::pin(async move { Ok(self.table.uncompressed_size()) })
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

    /// The compressed file's own on-disk length — a `stat` on the handle every
    /// decode also reads through, never the stream-index walk
    /// [`XzSource::size`] answers from memory
    /// (`docs/design/decisions.md`, "D21").
    fn stored_size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        Box::pin(async move {
            let file = Arc::clone(&self.file);
            let len = tokio::task::spawn_blocking(move || file.metadata().map(|m| m.len()))
                .await
                .map_err(Error::join)?
                .map_err(Error::from)?;
            Ok(len)
        })
    }

    fn hint_read_size(&self, len: usize) {
        self.budget.hint_read_size(len);
    }

    fn hint_parallelism(&self, parallelism: Parallelism) {
        self.budget.hint_parallelism(parallelism);
    }

    /// **The permission reaches the chunk pool only.** A chunk buffer is taken
    /// by one `read_range` and returned when its bytes are dropped, so a loop
    /// releasing each chunk before its next read holds one of them — which is
    /// the discipline
    /// [`BufferPool::obtain`] documents. The **block** pool's holder is
    /// [`BlockCache`], which *retains*: it is the exempt class, exactly as the
    /// replay loop is, so it is left at [`WaitPolicy::NeverWait`] whatever a
    /// loop permits (`docs/design/decisions.md`, "D5").
    ///
    /// A read that needs both takes the **chunk** slot first and the block
    /// slot inside it ([`XzSource::read_by_blocks`]), never the other way
    /// round, so a waiting reader holds no block slot at all.
    fn hint_wait_policy(&self, policy: WaitPolicy) {
        self.budget.pool.set_policy(policy);
    }

    fn seek_table(&self) -> Option<xz_seek::SeekTable> {
        Some((*self.table).clone())
    }

    fn partitions(&self, range: Range<u64>) -> Partitioning {
        self.budget.partitions(&self.table, range)
    }

    fn block_decode_bytes(&self) -> Option<u64> {
        self.budget.block_decode_bytes()
    }

    /// **The cores this process was given** — `available_parallelism()`, which
    /// is already the minimum of the affinity mask and every ancestor cgroup's
    /// CPU quota, rounded down and floored at one
    /// (`docs/design/runtime-invariants.md`, `RT7`). So a container told
    /// `--cpus=3.5` recommends three, and nothing here re-derives what `std`
    /// already reads.
    ///
    /// **Decode is the one shape that demonstrably scales**
    /// (`parallel-scan-throughput`; `docs/design/decisions.md`, "D2"). What
    /// the caller's budget affords still binds afterwards
    /// (`worker_count`).
    ///
    /// **Capped at this file's own block count**, `cut` cutting
    /// at block boundaries — and capped *here*, where the count is
    /// recommended, because the recommendation is multiplied into a budget
    /// request ([`ByteRangeSource::default_worker_memory`]) and printed beside
    /// it ([`Parallelism::discover_for`]).
    ///
    /// A failure to read the count answers **one** rather than propagating.
    fn default_workers(&self) -> usize {
        let cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        cores.min(self.table.block_count().max(1))
    }

    /// **What readers of this file hold**: [`XzBudget::block_worker_memory`] —
    /// the per-reader charge plus the block pool's retention list, the shape
    /// `worker_count` and [`Parallelism::fit`] both solve
    /// against to hand back a count. `None` where there is no block path to
    /// buy — a file with no blocks, or one whose largest block is not a length
    /// on this target — since the piecewise arm has no second worker to
    /// give at any budget.
    ///
    /// **It is charged at the chunk a scan settles at**
    /// ([`XzBudget::charged_chunk_bytes`]), which is what
    /// [`BlockCache::affordable`] compares a budget against once the file is
    /// open. *Rejected:* charging the unannounced pool's [`POOL_MAX_BYTES`](crate::source::pool::POOL_MAX_BYTES)
    /// ceiling instead, which runs the recommendation high and resolves fewer
    /// readers than fit.
    fn default_worker_memory(&self) -> Option<WorkerMemory> {
        self.budget.block_worker_memory()
    }

    /// Leave the live piecewise block, if a read left one seated, comparing
    /// its check ([`LiveBlock`]). The whole-block arm compares before a read
    /// returns, so this is the only block whose bytes can have gone out
    /// unverified — seated by the piecewise arm, and still seated where a
    /// raised budget moved later reads to the block arm.
    fn complete_reads(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let file = Arc::clone(&self.file);
            let live = Arc::clone(&self.live);
            tokio::task::spawn_blocking(move || {
                let leaving = live.lock().unwrap_or_else(|e| e.into_inner()).take();
                leaving.map_or(Ok(()), |leaving| leaving.leave(&file))
            })
            .await
            .map_err(Error::join)?
        })
    }
}

#[cfg(test)]
mod tests;
