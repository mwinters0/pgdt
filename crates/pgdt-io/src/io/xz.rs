//! The `.xz` source over a local file: a block decoded whole where the
//! budget holds it, piecewise past it, and the seek-table walk every `.xz`
//! source shares.

use std::future::Future;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bytes::Bytes;

use pgdt_core::{Error, Result};

use super::blocks::{BlockCache, BlockView, DecodedBlock, XzBudget};
use super::local::{BufferPool, POOL_DEPTH};
use super::parallelism::{Parallelism, Partitioning, WorkerMemory};
use super::source::{ByteRangeSource, PartitionRead, RetainedUnit, WaitPolicy};

/// A `ByteRangeSource` decoding an `.xz`-compressed local file on the fly
/// (`docs/design/decisions.md`, "D14", "D15").
///
/// **One file handle, positioned reads only.** Nothing here holds a cursor:
/// `stat` answers
/// [`ByteRangeSource::stored_size`]/[`ByteRangeSource::modified`], and every
/// decode reads its compressed bytes through `pread`, so the handle is shared
/// by everything and disturbs nothing. It is what `xz_seek` pulls from —
/// `std::fs::File` is a `CompressedSource` — which is where this source and
/// [`FetchedXzSource`](super::fetched::FetchedXzSource) part, whose bytes have to be fetched into a window
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
    pub(super) file: Arc<std::fs::File>,
    /// The table, the memory limit, the verification state and the backend as
    /// one value — everything a decode needs that is not bytes, and the only
    /// thing that hands out an `xz_seek::BlockTask`. It holds no source and
    /// needs no lock (`docs/design/decisions.md`, "D14").
    pub(super) layout: xz_seek::Layout,
    /// The layout's own table, aliased rather than copied, so `size()`,
    /// `seek_table()` and the per-read `blocks_in` lookup read one table.
    pub(super) table: Arc<xz_seek::SeekTable>,
    /// The live piecewise decode, or `None` where no read has taken that arm
    /// yet or its last decode failed. Behind a `Mutex` because a handle goes forward only and one
    /// position cannot serve two readers — which is why that arm advises a
    /// single partition ([`xz_partition_advice`]).
    pub(super) live: Arc<Mutex<Option<LiveBlock>>>,
    /// What the caller announced and everything derived from it: the two
    /// pools, which arm a read takes, and what a reader of this file costs.
    /// [`FetchedXzSource`](super::fetched::FetchedXzSource) holds the same value ([`XzBudget`]).
    pub(super) budget: XzBudget,
}

impl XzSource {
    /// Open `path` as `.xz`-compressed input, walking its stream footers to
    /// build the seek table before this call returns.
    ///
    /// This does not sniff the magic bytes; content-sniffing recognition
    /// across both source kinds is [`open_local`](super::origin::open_local).
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
    /// which is the error [`open_local`](super::origin::open_local) turns into [`Recognized::Mismatch`](super::origin::Recognized::Mismatch)
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
    /// [`open_local`](super::origin::open_local) turns into [`Recognized::Mismatch`](super::origin::Recognized::Mismatch) rather than a walk.
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
    /// refused ([`BOUNDARIED_PARTITION_UNITS`](super::parallelism::BOUNDARIED_PARTITION_UNITS)).
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
    pub(super) fn read_by_blocks(
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
    /// [`FetchedXzSource::read_in_pieces`](super::fetched::FetchedXzSource::read_in_pieces) drives, with a `File` in place of a
    /// fetched window (`docs/design/decisions.md`, "D15").
    ///
    /// **The handle is kept across reads**, which is what makes a forward scan
    /// cost one decode of the file rather than one per read ([`LiveBlock`]).
    /// The mutex is therefore held across the decode, and one position cannot
    /// serve two readers — the single partition [`xz_partition_advice`]'s other
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

/// The live piecewise decode: one `xz_seek::BlockRead` and the block it sits
/// in, kept between reads.
///
/// **It is the state machine both piecewise arms run**, generic over the
/// `xz_seek::CompressedSource` a decode call is handed — a `std::fs::File`
/// [`XzSource`] pulls from, or a `HeldBlock`'s fetched window. What the two
/// arms do not share is the loop around it, their scheduling differing: the
/// local one decodes a whole read inside one `spawn_blocking`, the fetched one
/// returns to the runtime between blocks to fetch the next window
/// (`docs/design/decisions.md`, "D15").
///
/// **It is `xz_seek::Reader`'s own live decode with the handle held here.** That
/// reader continued a forward read out of the decode it already had and
/// restarted the covering block otherwise, which is what keeps a forward scan
/// of a single-block file linear in the file rather than quadratic in it; the
/// handle is what carries that property across the mechanism swap
/// (`docs/design/decisions.md`, "D15").
///
/// **Leaving a block completes it**, which is where its check is compared —
/// `xz_seek::Verify::Full`'s seek-away escape, now a call we make — and the
/// block a run's last read stopped inside is left as that run's reading ends
/// ([`ByteRangeSource::complete_reads`]). Dropping a handle part-way compares
/// nothing, so a handle is dropped uncompleted only where its read failed or
/// its stream carries no check ([`LiveBlock::leave`]).
pub(super) struct LiveBlock {
    /// Which block the handle is decoding.
    pub(super) index: usize,
    /// The handle. Forward-only, so it serves a read at or after
    /// `read.position()` and nothing behind it.
    pub(super) read: xz_seek::BlockRead,
}

impl LiveBlock {
    /// Begin block `index` out of `source`.
    pub(super) fn begin<S: xz_seek::CompressedSource>(
        index: usize,
        task: &xz_seek::BlockTask,
        source: &S,
    ) -> Result<Self> {
        Ok(Self { index, read: task.begin(source).map_err(Error::xz)? })
    }

    /// Whether this handle can serve `from` out of block `index`.
    ///
    /// **Continuing is never dearer than restarting**, so there is no
    /// arithmetic to do: restarting skips `from − block.start` and continuing
    /// skips `from − position`, and a live handle's position is never before
    /// its block's start. That is `xz_seek::Reader`'s own route choice with the
    /// comparison resolved.
    pub(super) fn seats(&self, index: usize, from: u64) -> bool {
        self.index == index && !self.read.is_finished() && self.read.position() <= from
    }

    /// Skip forward to `from` and fill `out`, answering how many bytes the
    /// block had for it.
    ///
    /// `xz_seek::BlockRead::read` hands over what one decoder call produced
    /// rather than filling, and answers zero at the block's end, so the loop is
    /// what turns it into this crate's fill contract.
    pub(super) fn fill<S: xz_seek::CompressedSource>(
        &mut self,
        source: &S,
        from: u64,
        out: &mut [u8],
    ) -> Result<usize> {
        let mut skip = [0u8; PIECEWISE_SKIP_CHUNK];
        self.read.advance_to(source, from, &mut skip).map_err(Error::xz)?;
        let mut filled = 0usize;
        while filled < out.len() {
            let got = self.read.read(source, &mut out[filled..]).map_err(Error::xz)?;
            if got == 0 {
                break;
            }
            filled += got;
        }
        Ok(filled)
    }

    /// Leave the block, comparing its check.
    ///
    /// A `--check=none` stream has nothing to compare, so draining it would
    /// decode a block's remainder to verify nothing — which is the one case
    /// `Verify::Full`'s escape also skips.
    pub(super) fn leave<S: xz_seek::CompressedSource>(self, source: &S) -> Result<()> {
        if self.read.task().check() == xz_seek::Check::None {
            drop(self.read);
            return Ok(());
        }
        self.read.complete(source).map_err(Error::xz)
    }

    /// Fill `out` from block `index` at `from` through the slot's handle:
    /// the live one where it can reach that byte, a fresh one otherwise, the
    /// block it leaves completed first.
    ///
    /// This is the local arm's whole loop body, which decodes inside the one
    /// `spawn_blocking` its read already sits in; the fetched arm drives the
    /// same four calls around an `await` instead
    /// ([`FetchedXzSource::read_in_pieces`](super::fetched::FetchedXzSource::read_in_pieces)).
    fn serve<S: xz_seek::CompressedSource>(
        live: &mut Option<LiveBlock>,
        index: usize,
        task: &xz_seek::BlockTask,
        source: &S,
        from: u64,
        out: &mut [u8],
    ) -> Result<usize> {
        if !live.as_ref().is_some_and(|seated| seated.seats(index, from)) {
            if let Some(leaving) = live.take() {
                leaving.leave(source)?;
            }
            *live = Some(LiveBlock::begin(index, task, source)?);
        }
        live.as_mut().expect("just seated").fill(source, from, out)
    }
}

/// The error a read past the end of the uncompressed stream is — the same
/// `UnexpectedEof` a short `read_exact_at` raises on the plain source, so
/// both sources report a caller/source disagreement identically.
pub(super) fn xz_short_read() -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "xz stream ended before the requested range",
    ))
}

/// How many uncompressed bytes a piecewise read throws away per decoder call
/// while skipping to its first wanted byte.
///
/// On the stack, so a read that holds no whole block allocates nothing beyond
/// the buffer it is filling — which is the budget that arm exists to respect —
/// and large enough that skipping a 24 MiB block is thousands of decoder calls
/// rather than millions. It is `xz_seek`'s own drain chunk, which sizes the
/// same skip on the other side of `complete`.
const PIECEWISE_SKIP_CHUNK: usize = 4096;

/// Which part of a read `block` answers: where in the caller's buffer it lands,
/// the first uncompressed offset wanted from it, and how many bytes — or `None`
/// where the two ranges do not actually overlap.
///
/// One function because all four of the `.xz` read arms ask it of the same two
/// ranges, and four copies of the arithmetic can disagree about a boundary.
pub(super) fn piece_of(block: Range<u64>, offset: u64, end: u64) -> Option<(usize, u64, usize)> {
    let from = block.start.max(offset);
    let to = block.end.min(end);
    if to <= from {
        return None;
    }
    Some((usize::try_from(from - offset).ok()?, from, usize::try_from(to - from).ok()?))
}

/// An `.xz` source's partitioning advice, read off the **read path it took**
/// rather than off the seek table.
///
/// Two answers, and which one applies is `blocks`:
///
/// **Block-decoding** — the boundaries are the block starts inside
/// `range`, so a partition is a whole number of blocks. A partition costs
/// what one concurrent reader holds — [`BlockCache::reader_bytes`], the
/// per-reader term of the [`BlockCache::worker_memory`] whose `at(1)`
/// [`BlockCache::affordable`] compares a budget against.
///
/// **Piecewise** — one partition, whatever the table says: one live
/// `xz_seek::BlockRead` goes forward only, so two workers on different
/// partitions would each force the other's block to be left and begun again.
/// That is true of both providers, each holding one handle behind one mutex
/// ([`LiveBlock`], and `docs/design/decisions.md`, "D15").
///
/// Taken as a free function over the two pieces of state it reads, so the
/// piecewise arm is assertable against a table that *does* have blocks.
pub(super) fn xz_partition_advice(
    table: &xz_seek::SeekTable,
    blocks: Option<&BlockCache>,
    chunk_bytes: u64,
    decode_bytes: u64,
    range: Range<u64>,
) -> Partitioning {
    let Some(cache) = blocks else {
        // **The piecewise arm charges the chunk buffer and not the
        // decoder**: on both providers the decoder is a fixed cost of the
        // source rather than of a concurrent reader
        // ([`Partitioning::partition_bytes`]).
        //
        // Deficiency register: `deficiency: KD35` — one partition is not
        // what the budget affords: a decoder retains far less than a decoded
        // block, so several would fit where the blocks they decode do not,
        // and the budget that sends a file down this arm is exactly the one
        // that would benefit. **What pins the count at one is not the
        // budget**: [`LiveBlock`] is forward-only behind one mutex on both
        // providers, so two readers would force each other's restarts. The
        // fetched arm's statelessness would have let it serve N partitions
        // for want of cut points alone, and was spent to stop it re-decoding
        // a block a scan sits inside ([`HeldBlock`]) — this is where that
        // option went. **(c) unowned**;
        // closing it means a handle per reader — which `xz_seek::BlockRead`
        // being `Send` and borrowing nothing already allows — and a figure
        // over the count, and the figure is the half this cannot skip.
        return Partitioning::single(chunk_bytes);
    };
    let covering = table.blocks_in(range.clone());
    let at = covering
        .filter_map(|i| {
            let start = table.blocks[i].uncompressed_offset;
            (start > range.start && start < range.end).then_some(start)
        })
        .collect();
    // Three statements the piecewise arm above does not make, each
    // because a read inside a decoded block is a zero-copy slice of it:
    //
    // - **the retained unit is the partition, not the chunk**, so a caller
    //   adding its span allowance on top would count the partition's
    //   already-charged bytes twice ([`RetainedUnit`]);
    // - **a worker reads one of this source's units in one call**, which
    //   at the shipped cut width is the whole piece
    //   ([`PartitionRead::Whole`]). The unit is `cache.unit`, read off the
    //   cache rather than recomputed and non-zero by
    //   `BlockCache::for_table`'s own guard;
    // - **the block pool's retention list is stated beside the per-reader
    //   charge**, which is what lets `worker_count` solve
    //   for a count rather than divide by one
    //   ([`BlockCache::worker_memory`]).
    Partitioning::at(at, cache.reader_bytes(chunk_bytes, decode_bytes))
        .pooling(cache.unit as u64, POOL_DEPTH)
        .retaining(RetainedUnit::Partition)
        .reading(PartitionRead::Whole { unit: cache.unit as u64 })
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
    /// open. *Rejected:* charging the unannounced pool's [`POOL_MAX_BYTES`](super::local::POOL_MAX_BYTES)
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

/// Build an `.xz` file's seek table by driving `xz_seek`'s footer walk over an
/// **asynchronous** source, and say how many fetches it took.
///
/// `xz_seek::Walk` is the footer parse with the reads taken out: it hands out
/// a range and takes the bytes back, holding no borrow and no lifetime, so the
/// fetch is ours and may `await`. That is the whole reason a dump reached over
/// the network can be `.xz` at all —
/// `xz_seek::Reader` pulls through a *synchronous* positional trait, and
/// nothing can `await` inside it
/// (`docs/design/decisions.md`, "D14").
///
/// `leading` is what the caller already holds of the file's first bytes — an
/// [`Origin`](super::origin::Origin) probe's, normally. The walk's first request is the six magic
/// bytes at offset zero, which is exactly what that probe fetched, so a caller
/// passing them spends no round trip on them. Passing an empty slice is
/// correct and costs one.
///
/// **The count it returns is the file's, not this driver's.** The walk asks for
/// a footer, an index, a header and at least one padding probe per stream, in a
/// strictly backward chain where each request's position comes out of the bytes
/// of the one before it — so neither coalescing nor concurrency buys anything
/// on its own (`docs/design/decisions.md`, "D18").
///
/// Deficiency register: `deficiency: KD36` — this driver fetches exactly what
/// it is asked for, so a cold walk over a dump compressed as many small streams
/// is at least four round trips a stream before a row is read. The remedy is a **straddling
/// window**: one read per stream positioned backward from the pending request
/// and sized to cover what the walk asks for next, which answers three of every
/// stream's four requests because a stream's header is adjacent to its
/// predecessor's padding, footer and index. **One fetch a stream is then a
/// floor, not a cost still looking for a remedy** — the distance to the next
/// boundary is knowable only from the index just read, and this file family has
/// no stream stride to predict it from. What would beat the floor is not a
/// better driver but a forward table build, which stops paying separately for
/// the walk at all. **(c) unowned**; promoted by a phase that tunes the network,
/// which is where the fetch policy this source defers belongs, and which is also
/// what would take the figure.
pub async fn walk_seek_table(
    source: &dyn ByteRangeSource,
    stored_size: u64,
    leading: &[u8],
) -> Result<(xz_seek::SeekTable, usize)> {
    let (mut walk, mut request) = xz_seek::Walk::begin(stored_size);
    let mut fetches = 0usize;
    loop {
        let asked = usize::try_from(request.len()).map_err(|_| xz_short_read())?;
        let step = if request.start() == 0 && asked <= leading.len() {
            walk.supply(request, &leading[..asked]).map_err(Error::xz)?
        } else {
            fetches += 1;
            let bytes = source.read_range(request.start(), asked).await?;
            walk.supply(request, &bytes).map_err(Error::xz)?
        };
        match step {
            xz_seek::Step::Need(next) => request = next,
            xz_seek::Step::Done(table) => return Ok((table, fetches)),
        }
    }
}
