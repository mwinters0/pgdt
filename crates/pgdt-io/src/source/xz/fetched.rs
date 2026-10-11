//! The `.xz` source whose compressed bytes are fetched through another
//! source rather than pulled from a file.

use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bytes::Bytes;

use pgdt_core::internal::DecodeCounter;
use pgdt_core::{Error, Result};

use crate::cancel::Cancellation;
use crate::parallelism::{Parallelism, WorkerMemory};
use crate::partition::Partitioning;
use crate::source::pool::PooledBuffer;
use crate::source::{ByteRangeSource, RemoteIdentity, WaitPolicy};

use super::blocks::{BlockCache, BlockView, DecodedBlock, XzBudget};
use super::{LiveBlock, piece_of, xz_short_read};

/// A `ByteRangeSource` decoding an `.xz`-compressed dump the bytes of which
/// have to be **fetched** rather than pulled
/// (`docs/design/decisions.md`, "D14" and "D18").
///
/// **The difference from [`XzSource`](crate::XzSource) starts at the transport**, and what
/// follows from it — the worker count, the wait policy, the decoder's charge,
/// the state a failed read leaves — is stated where each lives. There the crate holds a `std::fs::File` and reads through it as it decodes;
/// here nothing can `await` inside a decode, so this source fetches a block's
/// whole compressed extent first — `xz_seek::BlockTask::compressed_range`
/// states it before the fetch — wraps it in an `xz_seek::Window` and decodes
/// out of that. The uncompressed side is unchanged: the same [`BlockCache`],
/// the same slot, the same zero-copy slice of a decoded block.
///
/// **Two arms, and the budget picks between them.**
///
/// - Where the stated budget holds a whole decoded block, a read is served
///   exactly as [`XzSource`](crate::XzSource)'s is: decode the block into a pooled slot, retain
///   it, slice the answer out of it.
/// - Where it does not, the block is read **in pieces** through
///   `xz_seek::BlockRead` — the handle skips forward to the first wanted byte,
///   fills the caller's buffer and compares the block's check when we complete
///   it — so what is held is the compressed window and the decoder, never the
///   plaintext. [`XzSource`](crate::XzSource) answers that arm the same way over a file it pulls
///   from, and this one keeps the window and the handle together across reads
///   so that a block a scan sits inside is decoded once (D15,
///   [`HeldBlock`]).
///
/// **Leaving a block completes it**, which is what compares its check, and a
/// run's end leaves the block its last read stopped inside
/// ([`ByteRangeSource::complete_reads`]). Reading a few kilobytes out of a
/// large block therefore pays a decode of the rest.
pub struct FetchedXzSource {
    /// The compressed bytes' transport: every fetch this source makes goes
    /// through it, so a cancellation, an identity precondition or a network
    /// failure arrives here in the words that source already uses.
    source: Arc<dyn ByteRangeSource>,
    /// The table, the memory limit, the verification state and the backend as
    /// one value — everything a decode needs that is not bytes. It holds no
    /// source, which is what lets this type exist (D14).
    layout: xz_seek::Layout,
    /// The layout's own table, aliased rather than copied, so `size()`,
    /// `seek_table()` and the per-read `blocks_in` lookup read one table.
    table: Arc<xz_seek::SeekTable>,
    /// The block the piecewise arm is reading — its fetched window and its
    /// live handle — or `None` where that arm has not run, its last read
    /// failed, a block-arm read has run since, or a piecewise read has it
    /// ([`HeldBlock`]). Behind a `Mutex` because the handle goes
    /// forward only and one position cannot serve two readers, which is why
    /// that arm advises a single partition ([`xz_partition_advice`](crate::source::xz::xz_partition_advice)).
    held: Mutex<Option<HeldBlock>>,
    /// How many block decodes the piecewise arm has begun. Absent without
    /// `introspect`, which is the build that asserts a block a scan sits
    /// inside is decoded once (`pgdt_core::internal::DecodeCounter`).
    decodes: DecodeCounter,
    /// What the caller announced and everything derived from it, which
    /// [`XzSource`](crate::XzSource) holds the same value of: only the decoder charge inside it
    /// is this source's own ([`XzBudget`]).
    pub(super) budget: XzBudget,
}

impl FetchedXzSource {
    /// Read `source` as `.xz` from a seek table a previous walk of the *same*
    /// object produced, **without walking it again**
    /// (`docs/design/decisions.md`, "D18").
    ///
    /// `stored_size` is the compressed object's own length, which the caller
    /// has from its [`Origin`](crate::Origin) probe; `xz_seek` validates the table against it
    /// and for internal consistency, reading **no bytes**. A table that does
    /// not describe this object is an `Error::Xz` carrying `xz_seek::Error::InvalidTable`,
    /// which is what [`open_remote`](crate::open_remote) turns into [`Recognized::Mismatch`](crate::Recognized::Mismatch)
    /// rather than a walk.
    pub fn with_table(
        source: Arc<dyn ByteRangeSource>,
        stored_size: u64,
        table: xz_seek::SeekTable,
    ) -> Result<Self> {
        // `Builder::new()` rather than a configured one, for `XzSource`'s
        // reason: it is the shortcut through exactly the defaults
        // `Reader::new` takes, `Verify::Full` included, so the two sources
        // decode identically.
        let layout = xz_seek::Builder::new().layout(table, stored_size).map_err(Error::xz)?;
        Ok(Self::assembled(source, layout))
    }

    /// The one place a constructor's pieces are put together: the table is
    /// aliased out of the layout, and the budget policy is built over it with
    /// this source's own decoder charge ([`XzBudget`]).
    fn assembled(source: Arc<dyn ByteRangeSource>, layout: xz_seek::Layout) -> Self {
        let table = layout.index_shared();
        // What one decode here retains besides the plaintext: the decoder over
        // a source that **lends** — an `xz_seek::Window` is read through
        // `slice_at`, so no compressed input chunk is built — plus the largest
        // window this file's blocks would make us fetch. `total_size()` rather
        // than `unpadded_size` because that is what `compressed_range()` asks
        // for: a window cut shorter would stop before the block's check.
        //
        // **The window is charged rather than booked as unpooled.** Its length
        // is known before the fetch, and a term that can be priced and is not
        // is the falsification of [`MEMORY_UNPOOLED_BOUND`] rather than an
        // instance of it (`docs/design/decisions.md`, "D15").
        let widest_window = table.blocks.iter().map(|block| block.total_size()).max().unwrap_or(0);
        let budget = XzBudget::over(&table, layout.decoder_bytes().saturating_add(widest_window));
        Self {
            source,
            layout,
            table,
            held: Mutex::new(None),
            decodes: DecodeCounter::default(),
            budget,
        }
    }

    /// One block's whole compressed extent, fetched and wrapped in the window
    /// a decode reads through.
    ///
    /// The window refuses every byte outside that range, so a mis-sized fetch
    /// is an error rather than a byte served silently from somewhere else.
    async fn window(&self, task: &xz_seek::BlockTask) -> Result<xz_seek::Window<Bytes>> {
        let extent = task.compressed_range();
        let len = usize::try_from(extent.end - extent.start).map_err(|_| xz_short_read())?;
        let bytes = self.source.read_range(extent.start, len).await?;
        Ok(xz_seek::Window::new(extent.start, self.table.compressed_file_size, bytes))
    }

    /// The task for block `index`, or the read-past-the-end error.
    ///
    /// **No lock is taken.** An `xz_seek::Layout` answers this from the table
    /// alone, which is why neither source locks anything to name a block
    /// ([`XzSource::block`](crate::XzSource::block)).
    fn task(&self, index: usize) -> Result<xz_seek::BlockTask> {
        self.layout.block_task(index).ok_or_else(xz_short_read)
    }

    /// Block `index`, from the retention list or fetched and decoded whole
    /// into a slot of the block pool.
    ///
    /// Deficiency register: two concurrent misses on one block fetch and
    /// decode it twice, exactly as `KD20` describes for the local source; the
    /// marker sits there, this being the same shape and not a second one.
    async fn block(&self, index: usize, cache: &Arc<BlockCache>) -> Result<Arc<DecodedBlock>> {
        if let Some(hit) = cache.lookup(index) {
            return Ok(hit);
        }
        let task = self.task(index)?;
        let len = usize::try_from(task.uncompressed_len()).map_err(|_| xz_short_read())?;
        let window = self.window(&task).await?;
        // A slot is the file's largest block, so `len` is never above it.
        let slot = cache.slot();
        let slot = tokio::task::spawn_blocking(move || -> Result<PooledBuffer> {
            let mut slot = slot;
            task.decode_into(&window, &mut slot.as_mut()[..len]).map_err(Error::xz)?;
            Ok(slot)
        })
        .await
        .map_err(Error::join)??;
        let block = Arc::new(DecodedBlock { slot, len });
        cache.retain(index, Arc::clone(&block));
        Ok(block)
    }

    /// A read served by decoding whole blocks — [`XzSource::read_by_blocks`](crate::XzSource::read_by_blocks)'s
    /// arithmetic over fetched windows.
    async fn read_by_blocks(
        &self,
        offset: u64,
        len: usize,
        cache: &Arc<BlockCache>,
    ) -> Result<Bytes> {
        let end = offset.checked_add(len as u64).ok_or_else(xz_short_read)?;
        let covering = self.table.blocks_in(offset..end);
        if covering.is_empty() {
            return Err(xz_short_read());
        }
        if covering.len() == 1 {
            let index = covering.start;
            let block = self.block(index, cache).await?;
            let base = self.table.blocks[index].uncompressed_offset;
            let start = offset
                .checked_sub(base)
                .and_then(|at| usize::try_from(at).ok())
                .ok_or_else(xz_short_read)?;
            if start + len > block.len {
                return Err(xz_short_read());
            }
            return Ok(Bytes::from_owner(BlockView(block)).slice(start..start + len));
        }
        let mut out = self.budget.pool.obtain(len);
        let mut covered = 0usize;
        for index in covering {
            let block = self.block(index, cache).await?;
            let base = self.table.blocks[index].uncompressed_offset;
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

    /// A read served **without holding a whole decoded block**: each covering
    /// block is fetched, skipped forward to the first wanted byte and read
    /// into the caller's buffer, and each but the last it touches is
    /// completed.
    ///
    /// It is [`XzSource::read_in_pieces`](crate::XzSource::read_in_pieces)'s mechanism over a fetched window
    /// instead of a file, which is the one asymmetry between the providers
    /// (D15). What it holds is the compressed window, the decoder and one chunk
    /// buffer — never the block's plaintext, which is the point.
    ///
    /// **The window and the handle are kept between reads together**
    /// ([`HeldBlock`]), as the local twin keeps its handle ([`LiveBlock`]): a
    /// forward scan inside one block fetches *and* decodes that block once
    /// rather than once per read (`docs/design/decisions.md`, "D15").
    async fn read_in_pieces(&self, offset: u64, len: usize) -> Result<Bytes> {
        let end = offset.checked_add(len as u64).ok_or_else(xz_short_read)?;
        let covering = self.table.blocks_in(offset..end);
        if covering.is_empty() {
            return Err(xz_short_read());
        }
        let mut out = self.budget.pool.obtain(len);
        // Taken for this read's duration, so a concurrent read finds an empty
        // slot and begins a block of its own rather than sharing this
        // position — which is what pins the arm to one partition
        // ([`xz_partition_advice`], `KD35`).
        let mut held = self.take_held();
        let mut covered = 0usize;
        for index in covering {
            let task = self.task(index)?;
            let Some((at, from, n)) = piece_of(task.uncompressed_range(), offset, end) else {
                continue;
            };
            let seat = match held.take_if(|kept| kept.seats(index, from)) {
                Some(kept) => kept.into_seat(),
                None => {
                    // Completing and dropping the outgoing block goes
                    // **before** the next window is fetched: holding both
                    // across a boundary would be one window more than
                    // `decode_bytes` counts (D15). The drain is a decode, so
                    // it goes off the runtime like every other one.
                    Self::leave(held.take()).await?;
                    let window = self.window(&task).await?;
                    self.decodes.begun();
                    Seat { window, live: None }
                }
            };
            let (returned, block, filled) =
                tokio::task::spawn_blocking(move || -> Result<(PooledBuffer, HeldBlock, usize)> {
                    let mut out = out;
                    // A handle is begun here rather than on the runtime's own
                    // task: it builds the decoder's dictionary, which is the
                    // thread every other decode call already runs on.
                    let mut block = seat.seat(index, &task)?;
                    let filled = block.fill(from, &mut out.as_mut()[at..at + n])?;
                    Ok((out, block, filled))
                })
                .await
                .map_err(Error::join)??;
            out = returned;
            held = Some(block);
            covered += filled;
        }
        if covered != len {
            return Err(xz_short_read());
        }
        // Past every way out, so a failed read retains nothing — `held` is
        // dropped on the way out instead, rather than leaving state a later
        // read would have to reason about. The local arm clears its live handle
        // on a decode error and leaves it seated on a short read, that handle
        // being a position a later read may continue from where a window is not.
        //
        // A concurrent read that began a block of its own may have put it back
        // since, and whichever is displaced here is completed rather than
        // dropped: its bytes are already out, and it is the only place their
        // check is left to be compared.
        let displaced =
            std::mem::replace(&mut *self.held.lock().unwrap_or_else(|e| e.into_inner()), held);
        Self::leave(displaced).await?;
        Ok(Bytes::from_owner(out).slice(..len))
    }

    /// The retained block, taken out of its slot.
    fn take_held(&self) -> Option<HeldBlock> {
        self.held.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Leave `held`, if there is a block, comparing its check — a decode, so
    /// off the runtime like every other one.
    async fn leave(held: Option<HeldBlock>) -> Result<()> {
        let Some(leaving) = held else {
            return Ok(());
        };
        tokio::task::spawn_blocking(move || leaving.leave()).await.map_err(Error::join)?
    }

    /// How many block decodes this source's piecewise arm has begun.
    ///
    /// **An instrument, never a figure** (`../../CLAUDE.md`): a build carrying
    /// `introspect` is never timed, and what this exists for is a test that
    /// asserts a block a forward scan sits inside is decoded once rather than
    /// once per read (`docs/design/decisions.md`, "D15").
    #[cfg(feature = "introspect")]
    pub fn block_decodes_begun(&self) -> u64 {
        self.decodes.count()
    }
}

/// The block the fetched piecewise arm is reading: the compressed window it
/// fetched, and the live handle decoding out of it.
///
/// **It is [`LiveBlock`] with its source carried beside it.** The local arm's
/// source is the file, held by the source itself and handed to each call;
/// here the source is one block's fetched extent, so the window travels with
/// the handle and the pair is what a read keeps
/// (`docs/design/decisions.md`, "D15").
///
/// **One block's window at a time is what this source's [`XzBudget`] charges**
/// — the decoder plus this file's largest window — so the block being left is
/// completed and dropped before the next one's window is fetched.
struct HeldBlock {
    /// The block's whole compressed extent, as fetched.
    window: xz_seek::Window<Bytes>,
    /// The handle decoding out of it.
    live: LiveBlock,
}

impl HeldBlock {
    /// Begin block `index` out of a window just fetched for it.
    fn begin(
        index: usize,
        task: &xz_seek::BlockTask,
        window: xz_seek::Window<Bytes>,
    ) -> Result<Self> {
        let live = LiveBlock::begin(index, task, &window)?;
        Ok(Self { window, live })
    }

    /// Whether this handle can serve `from` out of block `index`.
    fn seats(&self, index: usize, from: u64) -> bool {
        self.live.seats(index, from)
    }

    /// Skip to `from` and fill `out` out of the window.
    fn fill(&mut self, from: u64, out: &mut [u8]) -> Result<usize> {
        self.live.fill(&self.window, from, out)
    }

    /// Leave the block, comparing its check — the call that verifies bytes
    /// this reader never held whole, and the reason it is not a knob.
    fn leave(self) -> Result<()> {
        let HeldBlock { window, live } = self;
        live.leave(&window)
    }

    /// Hand the block to a read as something to decode out of.
    fn into_seat(self) -> Seat {
        Seat { window: self.window, live: Some(self.live) }
    }
}

/// What a read of the fetched piecewise arm carries into its decode: the
/// block's compressed window, and the handle already on it where the last
/// read left one open.
///
/// **The two cases converge where they are built rather than where they are
/// decided.** Which one applies is known on the runtime's own task, but
/// beginning a handle reads the block header and builds the decoder's
/// dictionary — the work every other decode call is moved off the runtime for
/// — so the absent handle travels as a `None` and
/// [`Seat::seat`] resolves it inside the blocking closure.
struct Seat {
    window: xz_seek::Window<Bytes>,
    live: Option<LiveBlock>,
}

impl Seat {
    /// The block to read out of, beginning one where no handle came with the
    /// window.
    fn seat(self, index: usize, task: &xz_seek::BlockTask) -> Result<HeldBlock> {
        match self.live {
            Some(live) => Ok(HeldBlock { window: self.window, live }),
            None => HeldBlock::begin(index, task, self.window),
        }
    }
}

impl std::fmt::Debug for FetchedXzSource {
    /// The transport behind it is `dyn`, so what is printable is this file's
    /// shape and what it may hold.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchedXzSource")
            .field("streams", &self.table.stream_count())
            .field("blocks", &self.table.block_count())
            .field("decode_bytes", &self.budget.decode_bytes)
            .finish_non_exhaustive()
    }
}

impl ByteRangeSource for FetchedXzSource {
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
            match self.budget.block_path().cloned() {
                Some(cache) => {
                    // This arm holds a block's plaintext beside the window it
                    // decoded from, so a block the piecewise arm left behind
                    // would be one window more than `decode_bytes` counts —
                    // and a budget raised mid-run is what reaches here with
                    // one retained (`docs/design/decisions.md`, "D15"). It is
                    // completed on the way out, its bytes having gone out
                    // before their check; the local arm leaves its live
                    // handle seated in the same case ([`LiveBlock`]), holding
                    // no window beside it, for the run's end to complete.
                    Self::leave(self.take_held()).await?;
                    self.read_by_blocks(offset, len, &cache).await
                }
                None => self.read_in_pieces(offset, len).await,
            }
        })
    }

    /// The uncompressed length, from the seek table — no decode, no fetch.
    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        Box::pin(async move { Ok(self.table.uncompressed_size()) })
    }

    /// The transport's, unchanged: the compression layer moves no file and
    /// changes no identity (`docs/design/decisions.md`, "D21").
    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        self.source.modified()
    }

    /// The compressed object's own stored length — the transport's `size`,
    /// never the stream-index total [`FetchedXzSource::size`] answers.
    fn stored_size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        self.source.stored_size()
    }

    fn hint_read_size(&self, len: usize) {
        self.budget.hint_read_size(len);
    }

    fn hint_parallelism(&self, parallelism: Parallelism) {
        self.budget.hint_parallelism(parallelism);
    }

    /// **Ignored: this source never takes a wait**, and the reason is where its
    /// chunk buffer is obtained rather than what the loop promises.
    ///
    /// Every other source takes its buffer inside a `spawn_blocking` closure,
    /// so a `WaitPolicy::MayWait` holder blocks a blocking-pool thread while a
    /// sibling releases. Here the fetch has to `await`, so the buffer is
    /// obtained on the runtime's own task — and `BufferPool::obtain` waits on a
    /// `Condvar`, which on the `current_thread` runtime this project dispatches
    /// from (`docs/design/decisions.md`, "D12") would block the very thread the
    /// releasing sibling needs. A wait that should not have been permitted is a
    /// hang with nothing to measure ([`WaitPolicy::NeverWait`]), so it is
    /// refused here rather than granted and hoped for.
    ///
    /// The cost is the direction that constant is already safe in: the pool
    /// allocates past its budget rather than blocking, which is memory instead
    /// of a deadlock. Making the wait available again means obtaining the
    /// buffer inside the decode's closure, which is a rearrangement this
    /// phase's "correctness only" scope does not buy anything by.
    fn hint_wait_policy(&self, _policy: WaitPolicy) {}

    /// Announced to the transport rather than kept here: what a cancellation
    /// interrupts is the fetch, and a decode between two fetches is bounded by
    /// one block.
    fn hint_cancellation(&self, cancel: Arc<Cancellation>) {
        self.source.hint_cancellation(cancel);
    }

    /// The transport's, unchanged — the object's identity is the object's,
    /// whatever is compressed inside it.
    fn remote_identity(&self) -> Option<RemoteIdentity> {
        self.source.remote_identity()
    }

    /// Announced to the transport, which is what carries the precondition on
    /// every fetch — so every window a walk or a block decode asks for is
    /// pinned to the version this run opened on, without either knowing
    /// (`docs/design/decisions.md`, "D21").
    fn hint_in_flight_identity(&self, binds: bool) {
        self.source.hint_in_flight_identity(binds);
    }

    /// The transport's: what pins a fetch is what checks a decoded block.
    fn in_flight_unchecked(&self) -> Option<String> {
        self.source.in_flight_unchecked()
    }

    fn seek_table(&self) -> Option<xz_seek::SeekTable> {
        Some((*self.table).clone())
    }

    /// [`xz_partition_advice`](crate::source::xz::xz_partition_advice), as the local source's is: the cut points are a
    /// property of the file rather than of the transport, and reading them off
    /// the arm actually taken is what keeps
    /// `pgdt_query::stream::compressed_block_path_declined` honest here.
    fn partitions(&self, range: Range<u64>) -> Partitioning {
        self.budget.partitions(&self.table, range)
    }

    fn block_decode_bytes(&self) -> Option<u64> {
        self.budget.block_decode_bytes()
    }

    /// **One**, where the local twin recommends `min(cores, block_count)` over
    /// the same bytes ([`XzSource::default_workers`](crate::XzSource::default_workers)) — this is the one source
    /// whose recommendation differs from its local twin's, and the file's
    /// block count says what may be *cut*, not what a network should be asked
    /// for at once.
    ///
    /// **The reason is not that this phase defers network tuning**, though it
    /// does (`docs/design/decisions.md`, "D2"): it is that the
    /// two errors are not symmetric. Recommending too few costs throughput on
    /// a link, and the caller types `--jobs` to take it back. Recommending too
    /// many opens `min(cores, block_count)` concurrent connections to a *third
    /// party's* server for a command that carries no flag saying so. So a
    /// later reading showing the link underused does not raise this number —
    /// it argues for raising `--jobs`, which is the caller's to type.
    fn default_workers(&self) -> usize {
        1
    }

    /// What a reader of *this file* holds, which the transport does not change
    /// — the same shape [`XzSource::default_worker_memory`](crate::XzSource::default_worker_memory) answers, with the
    /// compressed window folded into the decoder term. It is a recommendation
    /// about memory rather than about concurrency, so D2's one worker for a
    /// fetched source does not reach it, nor the plain remote source's
    /// silence, which has no block structure to charge for.
    fn default_worker_memory(&self) -> Option<WorkerMemory> {
        self.budget.block_worker_memory()
    }

    /// Leave the retained block, if a piecewise read left one, comparing its
    /// check out of the window held beside it — so no fetch
    /// ([`ByteRangeSource::complete_reads`], as the local twin does).
    fn complete_reads(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move { Self::leave(self.take_held()).await })
    }
}

#[cfg(test)]
mod tests;
