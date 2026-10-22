//! The compressed source whose bytes are fetched through another source
//! rather than pulled from a file, and the window a fetched unit is decoded
//! out of.

use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bytes::Bytes;
use tokio_util::sync::CancellationToken;

use pgdt_core::internal::DecodeCounter;
use pgdt_core::{Error, Result};

use crate::parallelism::{Parallelism, WorkerMemory};
use crate::partition::Partitioning;
use crate::source::pool::PooledBuffer;
use crate::source::{ByteRangeSource, Extent, RemoteIdentity, WaitPolicy};

use super::blocks::{BlockCache, Claim, DecodeBudget, DecodedBlock};
use super::{Codec, CompressionIndex, LiveBlock, Positioned, codec_over, piece_of, short_read};

/// A `ByteRangeSource` decoding a compressed dump the bytes of which have to
/// be **fetched** rather than pulled, whatever its codec
/// (`docs/design/decisions.md`, "D14" and "D18").
///
/// **The difference from [`CompressedFileSource`](crate::CompressedFileSource)
/// starts at the transport**, and what follows from it — the worker count,
/// the wait policy, the decoder's charge, the state a failed read leaves — is
/// stated where each lives. There a decode reads a file as it goes; here
/// nothing can `await` inside a decode, so this source fetches a unit's whole
/// compressed extent first — the codec states it before the fetch — wraps it
/// in a [`Window`] and decodes out of that. The plaintext side is unchanged:
/// the same [`BlockCache`], the same slot, the same zero-copy slice of a
/// decoded unit.
///
/// **Two arms, and the budget picks between them.**
///
/// - Where the stated budget holds a whole decoded unit, a read is served
///   exactly as the local source's is: decode the unit into a pooled slot,
///   retain it, slice the answer out of it.
/// - Where it does not, the unit is read **in pieces** through a live handle
///   — skipped forward to the first wanted byte, filling the caller's buffer
///   and comparing the unit's check when we complete it — so what is held is
///   the compressed window and the decoder, never the plaintext. This source
///   keeps the window and the handle together across reads so that a unit a
///   scan sits inside is decoded once (D15, [`HeldBlock`]).
///
/// **Leaving a unit completes it**, which is what compares its check, and a
/// run's end leaves the unit its last read stopped inside
/// ([`ByteRangeSource::complete_reads`]). Reading a few kilobytes out of a
/// large unit therefore pays a decode of the rest.
pub struct FetchedCompressedSource {
    /// The compressed bytes' transport: every fetch this source makes goes
    /// through it, so a cancellation, an identity precondition or a network
    /// failure arrives here in the words that source already uses.
    source: Arc<dyn ByteRangeSource>,
    /// The file's units and how to decode one. It holds no source, which is
    /// what lets this type exist (D14).
    codec: Arc<dyn Codec>,
    /// The unit the piecewise arm is reading — its fetched window and its
    /// live handle — or `None` where that arm has not run, its last read
    /// failed, a block-arm read has run since, `complete_reads` has left it,
    /// or a piecewise read has it ([`HeldBlock`]). Behind a `Mutex` because
    /// the handle goes forward only and one position cannot serve two readers,
    /// which is why that arm advises a single partition
    /// ([`partition_advice`](super::blocks::partition_advice)).
    held: Mutex<Option<HeldBlock>>,
    /// How many unit decodes the piecewise arm has begun. Absent without
    /// `introspect`, which is the build that asserts a unit a scan sits
    /// inside is decoded once (`pgdt_core::internal::DecodeCounter`).
    decodes: DecodeCounter,
    /// What the caller announced and everything derived from it, which
    /// [`CompressedFileSource`](crate::CompressedFileSource) holds the same
    /// value of: only the decoder charge inside it is this source's own
    /// ([`DecodeBudget`]).
    budget: DecodeBudget,
}

impl FetchedCompressedSource {
    /// Read `source` as compressed from an index a previous build of the
    /// *same* object produced, **without building it again**
    /// (`docs/design/decisions.md`, "D18").
    ///
    /// `stored_size` is the compressed object's own length, which the caller
    /// has from its [`Origin`](crate::Origin) probe; the codec validates the
    /// index against it and for internal consistency, reading **no bytes**.
    /// An index that does not describe this object is the error
    /// [`open`](crate::open) turns into
    /// [`Recognized::Mismatch`](crate::Recognized::Mismatch) rather than a
    /// rebuild.
    pub fn with_index(
        source: Arc<dyn ByteRangeSource>,
        stored_size: u64,
        index: CompressionIndex,
    ) -> Result<Self> {
        Ok(Self::assembled(source, codec_over(index, stored_size)?))
    }

    /// The one place a constructor's pieces are put together: the budget
    /// policy is built with this source's own decoder charge
    /// ([`DecodeBudget`]).
    fn assembled(source: Arc<dyn ByteRangeSource>, codec: Arc<dyn Codec>) -> Self {
        // What one decode here retains besides the plaintext: the decoder over
        // a source that **lends** — a [`Window`] is read through `slice_at`,
        // so no compressed input chunk is built — plus the largest window
        // this file's units would make us fetch.
        //
        // **The window is charged rather than booked as unpooled.** Its length
        // is known before the fetch, and a term that can be priced and is not
        // is the falsification of [`MEMORY_UNPOOLED_BOUND`] rather than an
        // instance of it (`docs/design/decisions.md`, "D15").
        let decode_bytes = codec.lending_footprint().saturating_add(codec.widest_compressed());
        let budget = DecodeBudget::over(codec.largest_unit(), decode_bytes);
        Self { source, codec, held: Mutex::new(None), decodes: DecodeCounter::default(), budget }
    }

    /// One unit's whole compressed extent, fetched and wrapped in the window
    /// a decode reads through.
    ///
    /// The window refuses every byte outside that range, so a mis-sized fetch
    /// is an error rather than a byte served silently from somewhere else.
    async fn window(&self, index: usize) -> Result<Window> {
        let extent = self.codec.compressed(index).ok_or_else(short_read)?;
        let len = usize::try_from(extent.end - extent.start).map_err(|_| short_read())?;
        let bytes = self.source.read_range(extent.start, len).await?;
        Ok(Window::new(extent.start, self.codec.compressed_size(), bytes))
    }

    /// Unit `index`, from the retention list, from another reader's fetch
    /// and decode of it, or fetched and decoded whole into a slot of the
    /// block pool ([`BlockCache::claim`]).
    ///
    /// **A future dropped while it holds the claim gives the unit up**: the
    /// claim's `Drop` brings its flight down empty, so a reader waiting on it
    /// claims again, and a decode already handed to `spawn_blocking` runs on
    /// to a result nobody retains.
    async fn block(&self, index: usize, cache: &Arc<BlockCache>) -> Result<Arc<DecodedBlock>> {
        let decoding = loop {
            match cache.claim(index) {
                Claim::Hit(block) => return Ok(block),
                Claim::Wait(flight) => {
                    if let Some(block) = flight.landed().await {
                        return Ok(block);
                    }
                }
                Claim::Decode(decoding) => break decoding,
            }
        };
        let unit = self.codec.plaintext(index).ok_or_else(short_read)?;
        let len = usize::try_from(unit.end - unit.start).map_err(|_| short_read())?;
        let window = self.window(index).await?;
        // A slot is the file's largest unit, so `len` is never above it.
        let slot = cache.slot();
        let codec = Arc::clone(&self.codec);
        let slot = tokio::task::spawn_blocking(move || -> Result<PooledBuffer> {
            let mut slot = slot;
            codec.decode(index, &window, &mut slot.as_mut()[..len])?;
            Ok(slot)
        })
        .await
        .map_err(Error::join)??;
        Ok(decoding.land(DecodedBlock { slot, len }))
    }

    /// The plaintext offset unit `index` starts at.
    fn base(&self, index: usize) -> Result<u64> {
        self.codec.plaintext(index).map(|unit| unit.start).ok_or_else(short_read)
    }

    /// A read served by decoding whole units — the local source's loop over
    /// fetched windows, its own only because the fetch awaits; the slice and
    /// the piece copy are [`DecodedBlock`]'s.
    async fn read_by_blocks(
        &self,
        offset: u64,
        len: usize,
        cache: &Arc<BlockCache>,
    ) -> Result<Bytes> {
        let end = offset.checked_add(len as u64).ok_or_else(short_read)?;
        let covering = self.codec.units_in(offset..end);
        if covering.is_empty() {
            return Err(short_read());
        }
        if covering.len() == 1 {
            let index = covering.start;
            let block = self.block(index, cache).await?;
            return block.slice(self.base(index)?, offset, len);
        }
        let mut out = self.budget.pool.obtain(len);
        let mut covered = 0usize;
        for index in covering {
            let block = self.block(index, cache).await?;
            covered += block.copy_piece(self.base(index)?, offset, end, out.as_mut());
        }
        if covered != len {
            return Err(short_read());
        }
        Ok(Bytes::from_owner(out).slice(..len))
    }

    /// A read served **without holding a whole decoded unit**: each covering
    /// unit is fetched, skipped forward to the first wanted byte and read
    /// into the caller's buffer, and each but the last it touches is
    /// completed.
    ///
    /// It is the local source's mechanism over a fetched window instead of a
    /// file, which is the one asymmetry between the providers (D15). What it
    /// holds is the compressed window, the decoder and one chunk buffer —
    /// never the unit's plaintext, which is the point.
    ///
    /// **The window and the handle are kept between reads together**
    /// ([`HeldBlock`]), as the local twin keeps its handle ([`LiveBlock`]): a
    /// forward scan inside one unit fetches *and* decodes that unit once
    /// rather than once per read (`docs/design/decisions.md`, "D15").
    pub(super) async fn read_in_pieces(&self, offset: u64, len: usize) -> Result<Bytes> {
        let end = offset.checked_add(len as u64).ok_or_else(short_read)?;
        let covering = self.codec.units_in(offset..end);
        if covering.is_empty() {
            return Err(short_read());
        }
        let mut out = self.budget.pool.obtain(len);
        // Taken for this read's duration, so a concurrent read finds an empty
        // slot and begins a unit of its own rather than sharing this
        // position — which is what pins the arm to one partition
        // ([`partition_advice`](super::blocks::partition_advice), `KD35`).
        let mut held = self.take_held();
        let mut covered = 0usize;
        for index in covering {
            let unit = self.codec.plaintext(index).ok_or_else(short_read)?;
            let Some((at, from, n)) = piece_of(unit, offset, end) else {
                continue;
            };
            let seat = match held.take_if(|kept| kept.seats(index, from)) {
                Some(kept) => kept.into_seat(),
                None => {
                    // Completing and dropping the outgoing unit goes
                    // **before** the next window is fetched: holding both
                    // across a boundary would be one window more than
                    // `decode_bytes` counts (D15). The drain is a decode, so
                    // it goes off the runtime like every other one.
                    Self::leave(held.take()).await?;
                    let window = self.window(index).await?;
                    self.decodes.begun();
                    Seat { window, live: None }
                }
            };
            let codec = Arc::clone(&self.codec);
            let (returned, block, filled) =
                tokio::task::spawn_blocking(move || -> Result<(PooledBuffer, HeldBlock, usize)> {
                    let mut out = out;
                    // A handle is begun here rather than on the runtime's own
                    // task: it builds the decoder's state, which is the
                    // thread every other decode call already runs on.
                    let mut block = seat.seat(&*codec, index)?;
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
            return Err(short_read());
        }
        // Past every way out, so a failed read retains nothing — `held` is
        // dropped on the way out instead, rather than leaving state a later
        // read would have to reason about. The local arm clears its live handle
        // on a decode error and leaves it seated on a short read, that handle
        // being a position a later read may continue from where a window is not.
        //
        // A concurrent read that began a unit of its own may have put it back
        // since, and whichever is displaced here is completed rather than
        // dropped: its bytes are already out, and it is the only place their
        // check is left to be compared.
        let displaced =
            std::mem::replace(&mut *self.held.lock().unwrap_or_else(|e| e.into_inner()), held);
        Self::leave(displaced).await?;
        Ok(Bytes::from_owner(out).slice(..len))
    }

    /// The retained unit, taken out of its slot.
    fn take_held(&self) -> Option<HeldBlock> {
        self.held.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Leave `held`, if there is a unit, comparing its check — a decode, so
    /// off the runtime like every other one.
    async fn leave(held: Option<HeldBlock>) -> Result<()> {
        let Some(leaving) = held else {
            return Ok(());
        };
        tokio::task::spawn_blocking(move || leaving.leave()).await.map_err(Error::join)?
    }

    /// How many unit decodes this source's piecewise arm has begun.
    ///
    /// **An instrument, never a figure** (`CLAUDE.md`): a build carrying
    /// `introspect` is never timed, and what this exists for is a test that
    /// asserts a unit a forward scan sits inside is decoded once rather than
    /// once per read (`docs/design/decisions.md`, "D15").
    #[cfg(feature = "introspect")]
    pub fn block_decodes_begun(&self) -> u64 {
        self.decodes.count()
    }
}

/// Compressed bytes already fetched, addressed in the file's own coordinates:
/// the [`Positioned`] read a fetched unit is decoded out of.
///
/// **Its coordinates are the file's**: it is told the whole compressed file's
/// size and reports that, so nothing renumbers. **It refuses what it does not
/// hold**: a read whose bytes are not in the window is an error naming both
/// ranges, never a short return — which would mean end of file to the codec
/// and report a fetch arithmetic mistake as a damaged file.
pub(super) struct Window {
    base: u64,
    file_size: u64,
    bytes: Bytes,
}

impl Window {
    /// A window holding `bytes`, the compressed file's bytes from `base`, in
    /// a file of `file_size` bytes.
    fn new(base: u64, file_size: u64, bytes: Bytes) -> Self {
        Self { base, file_size, bytes }
    }

    /// The file-absolute range this window holds, its end saturating so a
    /// read it cannot answer still reaches the refusal.
    fn range(&self) -> Range<u64> {
        self.base..self.base.saturating_add(self.bytes.len() as u64)
    }
}

impl Positioned for Window {
    /// The bytes at `offset` where the window holds every one the file would
    /// give: clipped to the file's end, a range the file has nothing of is
    /// `Ok(0)` as a real source's is, and one the window does not hold is
    /// refused.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        let asked_end = offset.saturating_add(buf.len() as u64);
        let end = asked_end.min(self.file_size);
        let n = end.saturating_sub(offset);
        if n == 0 {
            return Ok(0);
        }
        let held = self.range();
        if offset < held.start || end > held.end {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "a read of {offset}..{asked_end} falls outside the fetched window, which \
                     holds {}..{} of a {}-byte file",
                    held.start, held.end, self.file_size
                ),
            ));
        }
        let lo = (offset - held.start) as usize;
        let n = n as usize;
        buf[..n].copy_from_slice(&self.bytes[lo..lo + n]);
        Ok(n)
    }

    fn size(&self) -> std::io::Result<u64> {
        Ok(self.file_size)
    }

    /// The bytes themselves, wherever the window holds the whole range and
    /// the file has every one of them; declined otherwise, which costs a copy
    /// where a short answer would mean end of file.
    fn slice_at(&self, offset: u64, len: usize) -> Option<&[u8]> {
        let end = offset.checked_add(len as u64)?;
        let held = self.range();
        if end > self.file_size || offset < held.start || end > held.end {
            return None;
        }
        let lo = (offset - held.start) as usize;
        Some(&self.bytes[lo..lo + len])
    }
}

/// The unit the fetched piecewise arm is reading: the compressed window it
/// fetched, and the live handle decoding out of it.
///
/// **It is [`LiveBlock`] with its source carried beside it.** The local arm's
/// source is the file, held by the source itself and handed to each call;
/// here the source is one unit's fetched extent, so the window travels with
/// the handle and the pair is what a read keeps
/// (`docs/design/decisions.md`, "D15").
///
/// **One unit's window at a time is what this source's [`DecodeBudget`]
/// charges** — the decoder plus this file's largest window — so the unit
/// being left is completed and dropped before the next one's window is
/// fetched.
struct HeldBlock {
    /// The unit's whole compressed extent, as fetched.
    window: Window,
    /// The handle decoding out of it.
    live: LiveBlock,
}

impl HeldBlock {
    /// Whether this handle can serve `from` out of unit `index`.
    fn seats(&self, index: usize, from: u64) -> bool {
        self.live.seats(index, from)
    }

    /// Skip to `from` and fill `out` out of the window.
    fn fill(&mut self, from: u64, out: &mut [u8]) -> Result<usize> {
        self.live.fill(&self.window, from, out)
    }

    /// Leave the unit, comparing its check — the call that verifies bytes
    /// this reader never held whole, and the reason it is not a knob.
    fn leave(self) -> Result<()> {
        let HeldBlock { window, live } = self;
        live.leave(&window)
    }

    /// Hand the unit to a read as something to decode out of.
    fn into_seat(self) -> Seat {
        Seat { window: self.window, live: Some(self.live) }
    }
}

/// What a read of the fetched piecewise arm carries into its decode: the
/// unit's compressed window, and the handle already on it where the last
/// read left one open.
///
/// **The two cases converge where they are built rather than where they are
/// decided.** Which one applies is known on the runtime's own task, but
/// beginning a handle reads the unit's header and builds the decoder's
/// state — the work every other decode call is moved off the runtime for —
/// so the absent handle travels as a `None` and [`Seat::seat`] resolves it
/// inside the blocking closure.
struct Seat {
    window: Window,
    live: Option<LiveBlock>,
}

impl Seat {
    /// The unit to read out of, beginning one where no handle came with the
    /// window.
    fn seat(self, codec: &dyn Codec, index: usize) -> Result<HeldBlock> {
        let live = match self.live {
            Some(live) => live,
            None => LiveBlock::begin(codec, index, &self.window)?,
        };
        Ok(HeldBlock { window: self.window, live })
    }
}

impl std::fmt::Debug for FetchedCompressedSource {
    /// The transport behind it is `dyn`, so what is printable is this file's
    /// shape and what it may hold.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchedCompressedSource")
            .field("codec", &self.codec.name())
            .field("units", &self.codec.unit_count())
            .field("decode_bytes", &self.budget.decode_bytes)
            .finish_non_exhaustive()
    }
}

impl ByteRangeSource for FetchedCompressedSource {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        Box::pin(async move {
            // A zero-length read covers no units, so it is answered here
            // rather than left to look like a read past the end.
            if len == 0 {
                return Ok(Bytes::new());
            }
            match self.budget.block_path().cloned() {
                Some(cache) => {
                    // This arm holds a unit's plaintext beside the window it
                    // decoded from, so a unit the piecewise arm left behind
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

    /// The plaintext length, from the index — no decode, no fetch, and
    /// exact, the index being complete.
    fn extent(&self) -> Pin<Box<dyn Future<Output = Result<Extent>> + Send + '_>> {
        Box::pin(async move { Ok(Extent::Exact(self.codec.plaintext_size())) })
    }

    /// The transport's, unchanged: the compression layer moves no file and
    /// changes no identity (`docs/design/decisions.md`, "D21").
    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        self.source.modified()
    }

    /// The compressed object's own stored length — the transport's `size`,
    /// never the index total [`FetchedCompressedSource::extent`] answers.
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
    /// buffer inside the decode's closure, a rearrangement no reading has yet
    /// asked for.
    fn hint_wait_policy(&self, _policy: WaitPolicy) {}

    /// Announced to the transport rather than kept here: what a cancellation
    /// interrupts is the fetch, and a decode between two fetches is bounded by
    /// one unit.
    fn hint_cancellation(&self, cancel: CancellationToken) {
        self.source.hint_cancellation(cancel);
    }

    /// The transport's, unchanged — the object's identity is the object's,
    /// whatever is compressed inside it.
    fn remote_identity(&self) -> Option<RemoteIdentity> {
        self.source.remote_identity()
    }

    /// Announced to the transport, which is what carries the precondition on
    /// every fetch — so every window an index build or a unit decode asks for
    /// is pinned to the version this run opened on, without either knowing
    /// (`docs/design/decisions.md`, "D21").
    fn hint_in_flight_identity(&self, binds: bool) {
        self.source.hint_in_flight_identity(binds);
    }

    /// The transport's: what pins a fetch is what checks a decoded unit.
    fn in_flight_unchecked(&self) -> Option<String> {
        self.source.in_flight_unchecked()
    }

    fn compression_index(&self) -> Option<CompressionIndex> {
        Some(self.codec.index())
    }

    /// [`partition_advice`](super::blocks::partition_advice), as the local
    /// source's is: the cut points are a property of the file rather than of
    /// the transport, and reading them off the arm actually taken is what
    /// keeps `pgdt_query`'s `compressed_block_path_declined` honest here.
    fn partitions(&self, range: Range<u64>) -> Partitioning {
        self.budget.partitions(&*self.codec, range)
    }

    fn block_decode_bytes(&self) -> Option<u64> {
        self.budget.block_decode_bytes()
    }

    /// **One**, where the local twin recommends `min(cores, unit_count)` over
    /// the same bytes
    /// ([`CompressedFileSource::default_workers`](crate::CompressedFileSource::default_workers))
    /// — this is the one source whose recommendation differs from its local
    /// twin's, and the file's unit count says what may be *cut*, not what a
    /// network should be asked for at once.
    ///
    /// **The reason is not that the network is untuned**, though it is
    /// (`docs/design/decisions.md`, "D2"): it is that the two errors are not
    /// symmetric. Recommending too few costs throughput on a link, and the
    /// caller types `--jobs` to take it back. Recommending too many opens
    /// `min(cores, unit_count)` concurrent connections to a *third party's*
    /// server for a command that carries no flag saying so. So a later reading
    /// showing the link underused does not raise this number — it argues for
    /// raising `--jobs`, which is the caller's to type.
    fn default_workers(&self) -> usize {
        1
    }

    /// What a reader of *this file* holds, which the transport does not change
    /// — the same shape the local source answers, with the compressed window
    /// folded into the decoder term. It is a recommendation about memory
    /// rather than about concurrency, so D2's one worker for a fetched source
    /// does not reach it, nor the plain remote source's silence, which has no
    /// unit structure to charge for.
    fn default_worker_memory(&self) -> Option<WorkerMemory> {
        self.budget.block_worker_memory()
    }

    /// Leave the retained unit, if a piecewise read left one, comparing its
    /// check out of the window held beside it — so no fetch
    /// ([`ByteRangeSource::complete_reads`], as the local twin does).
    fn complete_reads(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move { Self::leave(self.take_held()).await })
    }
}

#[cfg(test)]
mod tests;
