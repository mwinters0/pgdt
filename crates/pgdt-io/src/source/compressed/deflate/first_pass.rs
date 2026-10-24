//! The first pass over a gzip file with no complete index: a source reading
//! the file forward from its start — or from the last checkpoint an
//! interrupted pass saved — building the index as it goes, and handing over
//! to the indexed source once it reaches the end.
//!
//! **The index is the first full pass's by-product**
//! (`docs/design/roadmap-P15-gzip.md`, "Settled in the grilling"): a single
//! member states nothing about its own interior, so the scan that maps the
//! file is the decode that finds its checkpoints, placed by `gz_seek`'s rule
//! at [`CHECKPOINT_SPACING`] — a function of the file alone, so a pass
//! resumed from a saved index builds the one an uninterrupted pass builds.
//!
//! **Serial until the end, then the indexed source.** The extent is open until
//! a read meets the file's end, so nothing cuts the file and one forward
//! decode serves the scan; at the end the complete index, its windows in the
//! pass's spill file, opens the source a later run opens from its cache
//! ([`CompressedFileSource`], [`FetchedCompressedSource`]), and every read,
//! partitioning and worker recommendation is that source's from then on — so a
//! cold `query`'s replay reads units.
//!
//! **No window is resident**: each is written as it is placed to a file the
//! run alone holds, in the directory `open`'s caller names
//! (`docs/design/decisions.md`, "D115").
//!
//! **Announced where it opens and where it completes**, on the status channel
//! as the `.xz` walk is: the source says why its run is serial, the
//! arrangement reading only its advice (`docs/design/decisions.md`, "D64").

use std::future::Future;
use std::num::NonZeroU64;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use bytes::Bytes;
use tokio_util::sync::CancellationToken;

use pgdt_core::{Error, Result};

use crate::parallelism::{Parallelism, WorkerMemory};
use crate::partition::Partitioning;
use crate::source::pool::{BufferPool, POOL_DEPTH, PooledBuffer, budget_bytes};
use crate::source::{ByteRangeSource, Extent, RemoteIdentity, WaitPolicy};

use super::super::short_read;
use super::super::{CompressedFileSource, CompressionIndex, FetchedCompressedSource};
use super::{CHECKPOINT_SPACING, GZIP, GzipIndex, INPUT_CHUNK, WindowProvider, gzip_error};

mod pass;
mod spill;

use self::pass::{Inflating, Pass, Route, SKIP_SCRATCH, Served};
use self::spill::PassWindows;

/// What one forward decode of the pass holds besides the plaintext it hands
/// out: the inflater and its window, the window copy a checkpoint is read out
/// of, and the compressed input chunk.
const DECODE_BYTES: u64 = gz_seek::DECODER_BYTES + gz_seek::WINDOW_SIZE as u64 + INPUT_CHUNK;

/// What the pass holds besides the plaintext it hands out: its two forward
/// decodes — the frontier and one behind it — and the scratch a read skipping
/// to its first wanted byte decodes into.
const PASS_BYTES: u64 = 2 * DECODE_BYTES + SKIP_SCRATCH as u64;

/// A gzip file read by its first pass ([`open`](crate::open)'s source for one
/// no cache has indexed), building its index as a forward decode reaches each
/// checkpoint.
///
/// **One forward decode at the frontier**, behind a lock and taken one read
/// at a time, so the checkpoints are placed in file order whoever reads; a
/// read behind it begins a second decode at the last checkpoint at or before
/// its first byte, kept across reads so a scan behind the frontier continues
/// rather than restarting.
///
/// **What the frontier hands out is checked at the member's end**, as every
/// byte of a single member is: its trailer is the only check the file carries,
/// the units' own CRC-32s being recorded from the plaintext this pass decodes.
/// A pass stopped short of it has compared nothing, and a later pass resumed
/// from its index compares the member's trailer against those recorded values
/// combined, so a corrupted prefix is still refused at the end. A read behind
/// the frontier is compared with nothing (`KD132`).
pub(crate) struct FirstPass {
    feed: Feed,
    /// What the pass's status lines name the file by: its origin as a message
    /// to the user names it.
    named: String,
    /// The compressed file's length, as the open's probe found it.
    stored_size: u64,
    /// The forward decodes and the index as far as they have got, locked for
    /// a pulled read whole, a fetched read's decode steps, or a snapshot, never
    /// across an `await`.
    inner: Arc<Mutex<Pass>>,
    /// Serialises reads: the frontier goes forward only, and a fetched feed
    /// releases `inner` while it waits for bytes.
    turn: tokio::sync::Mutex<()>,
    /// Every window the pass has placed, in its spill file, and the provider a
    /// saved index reads them through.
    windows: Arc<PassWindows>,
    /// The plaintext buffers a read is answered in.
    pool: Arc<BufferPool>,
    /// The indexed source, once the pass has reached the end.
    done: OnceLock<Arc<dyn ByteRangeSource>>,
    /// What callers announced, replayed on the indexed source when it is
    /// built.
    hints: Mutex<Hints>,
}

/// Where the compressed bytes come from: a local file, pulled inside the
/// blocking task the decode runs in, or a transport fetched through between
/// decode steps (`docs/design/decisions.md`, "D15").
pub(crate) enum Feed {
    File { path: PathBuf, file: Arc<std::fs::File> },
    Fetched(Arc<dyn ByteRangeSource>),
}

impl Feed {
    /// The local file at `path`.
    pub(crate) fn file(path: &std::path::Path) -> Result<Feed> {
        Ok(Feed::File { path: path.to_path_buf(), file: Arc::new(std::fs::File::open(path)?) })
    }
}

impl FirstPass {
    /// A pass from the file's first byte, placing checkpoints at
    /// [`CHECKPOINT_SPACING`] and spilling their windows into `spill`, its
    /// status lines naming the file `named` — its origin as a message to the
    /// user names it.
    pub(crate) fn fresh(
        named: String,
        feed: Feed,
        stored_size: u64,
        spill: &Path,
    ) -> Result<FirstPass> {
        Self::spaced(named, feed, stored_size, CHECKPOINT_SPACING, spill)
    }

    /// A pass named `named` from the file's first byte, placing checkpoints
    /// at `spacing` and spilling their windows into `spill`, saying it has
    /// started.
    pub(crate) fn spaced(
        named: String,
        feed: Feed,
        stored_size: u64,
        spacing: NonZeroU64,
        spill: &Path,
    ) -> Result<FirstPass> {
        let forward = gz_seek::Forward::new(spacing).map_err(gzip_error)?;
        let windows = PassWindows::fresh(spill);
        let pass = Self::over(named, feed, stored_size, Inflating::new(forward), 0, None, windows);
        tracing::info!(
            source = %pass.named,
            codec = %GZIP,
            "first pass started, reading serially while it builds the index",
        );
        Ok(pass)
    }

    /// A pass named `named` resumed at the last checkpoint of `index`, an
    /// incomplete index a previous pass over the *same* file saved, its
    /// windows read through the provider riding on it and those it places
    /// spilled into `spill`, saying it has started and from which checkpoint.
    ///
    /// **Checked against the file, reading no bytes of it**: an index whose
    /// last checkpoint lies at or past the file's end, or that `gz_seek`
    /// cannot resume, does not describe this file, which
    /// [`open`](crate::open) turns into
    /// [`Recognized::Mismatch`](crate::Recognized::Mismatch).
    pub(crate) fn resumed(
        named: String,
        feed: Feed,
        stored_size: u64,
        index: GzipIndex,
        spill: &Path,
    ) -> Result<FirstPass> {
        let GzipIndex { index, windows } = index;
        let checkpoints = index.checkpoints();
        let last = *checkpoints.last().expect("an index holds its first checkpoint");
        if last.bit / 8 >= stored_size {
            return Err(gzip_error(gz_seek::Error::InvalidIndex {
                reason: "the index's last checkpoint lies past the file's end",
            }));
        }
        let mut window = vec![0u8; last.window_len()];
        if !window.is_empty() {
            let provider = windows.as_ref().ok_or_else(|| {
                Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "a gzip index resumed inside a member was handed in with no window provider",
                ))
            })?;
            provider.read_window(checkpoints.len() - 1, &mut window)?;
        }
        let forward = gz_seek::Forward::resume(&index, &window).map_err(gzip_error)?;
        // A pass resumed at a member start places that checkpoint again.
        let placed = checkpoints.len() - usize::from(last.start == gz_seek::Start::Member);
        let base = checkpoints.len();
        let pass_windows = PassWindows::after(windows, base, spill);
        let inflating = Inflating::new(forward);
        let pass =
            Self::over(named, feed, stored_size, inflating, placed, Some(index), pass_windows);
        tracing::info!(
            source = %pass.named,
            codec = %GZIP,
            checkpoint = base - 1,
            plaintext = last.plaintext,
            "first pass started, resuming from a saved checkpoint and reading serially while \
             it builds the index",
        );
        Ok(pass)
    }

    fn over(
        named: String,
        feed: Feed,
        stored_size: u64,
        frontier: Inflating,
        placed: usize,
        resumed_over: Option<gz_seek::Index>,
        windows: PassWindows,
    ) -> FirstPass {
        let pass = Pass::new(frontier, placed, resumed_over);
        FirstPass {
            feed,
            named,
            stored_size,
            inner: Arc::new(Mutex::new(pass)),
            turn: tokio::sync::Mutex::new(()),
            windows: Arc::new(windows),
            pool: Arc::new(BufferPool::default()),
            done: OnceLock::new(),
            hints: Mutex::new(Hints::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Pass> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The index as far as the pass has got, its windows read through the
    /// pass's own provider.
    fn index(&self) -> GzipIndex {
        let index = self.lock().index();
        GzipIndex { index, windows: Some(Arc::clone(&self.windows) as Arc<dyn WindowProvider>) }
    }

    /// Up to `len` bytes at `offset`, short only at the file's end.
    async fn read_up_to(&self, offset: u64, len: usize, strict: bool) -> Result<Bytes> {
        if let Some(done) = self.done.get() {
            return delegated(done, offset, len, strict).await;
        }
        let turn = self.turn.lock().await;
        if let Some(done) = self.done.get() {
            drop(turn);
            return delegated(done, offset, len, strict).await;
        }
        let (out, filled, ended) = match &self.feed {
            Feed::File { file, .. } => self.read_pulled(Arc::clone(file), offset, len).await?,
            Feed::Fetched(raw) => self.read_fetched(Arc::clone(raw), offset, len).await?,
        };
        if ended {
            self.hand_over()?;
        }
        drop(turn);
        if strict && filled < len {
            return Err(short_read());
        }
        Ok(Bytes::from_owner(out).slice(..filled))
    }

    /// A read whose decode pulls its compressed bytes from the file, the
    /// whole read inside one blocking task.
    async fn read_pulled(
        &self,
        file: Arc<std::fs::File>,
        offset: u64,
        len: usize,
    ) -> Result<(PooledBuffer, usize, bool)> {
        let (inner, windows, pool) =
            (Arc::clone(&self.inner), Arc::clone(&self.windows), Arc::clone(&self.pool));
        let stored_size = self.stored_size;
        tokio::task::spawn_blocking(move || {
            let mut out = pool.obtain(len);
            let mut pass = inner.lock().unwrap_or_else(|e| e.into_inner());
            let route = pass.route(offset, &windows)?;
            let mut filled = 0usize;
            loop {
                match pass.serve(
                    route,
                    offset,
                    &mut out.as_mut()[..len],
                    &mut filled,
                    &windows,
                    stored_size,
                )? {
                    Served::Full => return Ok((out, filled, false)),
                    Served::Ended => return Ok((out, filled, matches!(route, Route::Frontier))),
                    Served::Needs(at) => pass.refill_from_file(route, &file, at, stored_size)?,
                }
            }
        })
        .await
        .map_err(Error::join)?
    }

    /// A read whose decode is fed bytes fetched through the transport, back
    /// on the runtime for each fetch.
    async fn read_fetched(
        &self,
        raw: Arc<dyn ByteRangeSource>,
        offset: u64,
        len: usize,
    ) -> Result<(PooledBuffer, usize, bool)> {
        let stored_size = self.stored_size;
        let mut state: Option<(PooledBuffer, usize, Route)> = None;
        let mut supply: Option<(u64, Bytes)> = None;
        loop {
            let (inner, windows, pool) =
                (Arc::clone(&self.inner), Arc::clone(&self.windows), Arc::clone(&self.pool));
            let given = supply.take();
            let carried = state.take();
            let (out, filled, route, served) = tokio::task::spawn_blocking(move || {
                let mut pass = inner.lock().unwrap_or_else(|e| e.into_inner());
                let (mut out, mut filled, route) = match carried {
                    Some(carried) => carried,
                    None => (pool.obtain(len), 0, pass.route(offset, &windows)?),
                };
                if let Some((at, bytes)) = given {
                    pass.supply(route, at, &bytes);
                }
                let served = pass.serve(
                    route,
                    offset,
                    &mut out.as_mut()[..len],
                    &mut filled,
                    &windows,
                    stored_size,
                )?;
                Ok::<_, Error>((out, filled, route, served))
            })
            .await
            .map_err(Error::join)??;
            match served {
                Served::Full => return Ok((out, filled, false)),
                Served::Ended => return Ok((out, filled, matches!(route, Route::Frontier))),
                Served::Needs(at) => {
                    let want = INPUT_CHUNK.min(stored_size.saturating_sub(at)) as usize;
                    let bytes = raw.read_range(at, want).await?;
                    supply = Some((at, bytes));
                    state = Some((out, filled, route));
                }
            }
        }
    }

    /// The pass has reached the end: the indexed source over the complete
    /// index, its windows still read out of the spill file, which every call
    /// is from now on, with what callers announced replayed on it, saying
    /// what the index holds.
    fn hand_over(&self) -> Result<()> {
        let index = self.lock().ended.clone().expect("the frontier reached the end");
        let windows = Arc::clone(&self.windows) as Arc<dyn WindowProvider>;
        let index = GzipIndex { index, windows: Some(windows) };
        let (units, members) = (index.unit_count(), index.member_count());
        let index = CompressionIndex::Gzip(index);
        let done: Arc<dyn ByteRangeSource> = match &self.feed {
            Feed::File { path, .. } => Arc::new(CompressedFileSource::with_index(path, index)?),
            Feed::Fetched(raw) => Arc::new(FetchedCompressedSource::with_index(
                Arc::clone(raw),
                self.stored_size,
                index,
            )?),
        };
        tracing::info!(
            source = %self.named,
            codec = %GZIP,
            units,
            members,
            "first pass complete, the index built",
        );
        let hints = self.hints.lock().unwrap_or_else(|e| e.into_inner());
        hints.replay(&*done);
        // Set under the hints' lock, so a hint arriving now is replayed or
        // forwarded, never neither.
        let _ = self.done.set(done);
        Ok(())
    }

    /// The raw transport, for what this source answers by it.
    fn fetched(&self) -> Option<&Arc<dyn ByteRangeSource>> {
        match &self.feed {
            Feed::Fetched(raw) => Some(raw),
            Feed::File { .. } => None,
        }
    }

    /// Record a hint and forward it where it goes now: to the indexed source
    /// once there is one.
    fn hint(&self, record: impl FnOnce(&mut Hints), forward: impl FnOnce(&dyn ByteRangeSource)) {
        let mut hints = self.hints.lock().unwrap_or_else(|e| e.into_inner());
        record(&mut hints);
        if let Some(done) = self.done.get() {
            forward(&**done);
        }
    }
}

/// `len` bytes at `offset` from the indexed source, strictly or up to its end.
async fn delegated(
    done: &Arc<dyn ByteRangeSource>,
    offset: u64,
    len: usize,
    strict: bool,
) -> Result<Bytes> {
    if strict { done.read_range(offset, len).await } else { done.read_at_most(offset, len).await }
}

/// What callers have announced, replayed on the indexed source when the pass
/// hands over to it.
#[derive(Default)]
struct Hints {
    read_size: Option<usize>,
    parallelism: Option<Parallelism>,
    wait: Option<WaitPolicy>,
    cancel: Option<CancellationToken>,
    in_flight: Option<bool>,
}

impl Hints {
    fn replay(&self, done: &dyn ByteRangeSource) {
        if let Some(len) = self.read_size {
            done.hint_read_size(len);
        }
        if let Some(parallelism) = self.parallelism {
            done.hint_parallelism(parallelism);
        }
        if let Some(policy) = self.wait {
            done.hint_wait_policy(policy);
        }
        if let Some(cancel) = &self.cancel {
            done.hint_cancellation(cancel.clone());
        }
        if let Some(binds) = self.in_flight {
            done.hint_in_flight_identity(binds);
        }
    }
}

impl ByteRangeSource for FirstPass {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        Box::pin(async move {
            if len == 0 {
                return Ok(Bytes::new());
            }
            self.read_up_to(offset, len, true).await
        })
    }

    /// The read a scan to an open end makes: short only at the file's end,
    /// which the pass learns by reaching it.
    fn read_at_most(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        Box::pin(async move {
            if len == 0 {
                return Ok(Bytes::new());
            }
            self.read_up_to(offset, len, false).await
        })
    }

    /// Open at the frontier until the pass reaches the end, the indexed
    /// source's exact extent from then on.
    fn extent(&self) -> Pin<Box<dyn Future<Output = Result<Extent>> + Send + '_>> {
        Box::pin(async move {
            if let Some(done) = self.done.get() {
                return done.extent().await;
            }
            let pass = self.lock();
            Ok(match (&pass.ended, &pass.frontier) {
                (Some(index), _) => Extent::Exact(index.end().expect("complete").plaintext),
                (None, Some(frontier)) => Extent::Open { at_least: frontier.forward.position() },
                (None, None) => Extent::Open { at_least: 0 },
            })
        })
    }

    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        Box::pin(async move {
            match &self.feed {
                Feed::Fetched(raw) => raw.modified().await,
                Feed::File { file, .. } => {
                    let file = Arc::clone(file);
                    let mtime = tokio::task::spawn_blocking(move || {
                        file.metadata().and_then(|m| m.modified())
                    })
                    .await
                    .map_err(Error::join)?
                    .map_err(Error::from)?;
                    Ok(Some(mtime))
                }
            }
        })
    }

    /// The compressed file's own length, live: a `stat` on the file, or the
    /// transport's answer (`docs/design/decisions.md`, "D21").
    fn stored_size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        Box::pin(async move {
            match &self.feed {
                Feed::Fetched(raw) => raw.stored_size().await,
                Feed::File { file, .. } => {
                    let file = Arc::clone(file);
                    tokio::task::spawn_blocking(move || file.metadata().map(|m| m.len()))
                        .await
                        .map_err(Error::join)?
                        .map_err(Error::from)
                }
            }
        })
    }

    fn hint_read_size(&self, len: usize) {
        self.pool.hint(len);
        self.hint(|hints| hints.read_size = Some(len), |done| done.hint_read_size(len));
    }

    fn hint_parallelism(&self, parallelism: Parallelism) {
        self.pool.set_limits(budget_bytes(parallelism), POOL_DEPTH);
        self.hint(
            |hints| hints.parallelism = Some(parallelism),
            |done| done.hint_parallelism(parallelism),
        );
    }

    fn hint_wait_policy(&self, policy: WaitPolicy) {
        self.pool.set_policy(policy);
        self.hint(|hints| hints.wait = Some(policy), |done| done.hint_wait_policy(policy));
    }

    fn hint_cancellation(&self, cancel: CancellationToken) {
        if let Some(raw) = self.fetched() {
            raw.hint_cancellation(cancel.clone());
        }
        let forwarded = cancel.clone();
        self.hint(|hints| hints.cancel = Some(cancel), |done| done.hint_cancellation(forwarded));
    }

    fn remote_identity(&self) -> Option<RemoteIdentity> {
        self.fetched().and_then(|raw| raw.remote_identity())
    }

    fn hint_in_flight_identity(&self, binds: bool) {
        if let Some(raw) = self.fetched() {
            raw.hint_in_flight_identity(binds);
        }
        self.hint(
            |hints| hints.in_flight = Some(binds),
            |done| done.hint_in_flight_identity(binds),
        );
    }

    fn in_flight_unchecked(&self) -> Option<String> {
        self.fetched().and_then(|raw| raw.in_flight_unchecked())
    }

    /// The index as far as the pass has got — its last unit open until the
    /// end — which a save persists and a later pass resumes from.
    fn compression_index(&self) -> Option<CompressionIndex> {
        match self.done.get() {
            Some(done) => done.compression_index(),
            None => Some(CompressionIndex::Gzip(self.index())),
        }
    }

    /// One partition until the end, a forward decode and the skip scratch
    /// costing what they hold: nothing cuts a file whose extent is open.
    fn partitions(&self, range: Range<u64>) -> Partitioning {
        match self.done.get() {
            Some(done) => done.partitions(range),
            None => Partitioning::single(DECODE_BYTES + SKIP_SCRATCH as u64),
        }
    }

    fn block_decode_bytes(&self) -> Option<u64> {
        self.done.get().and_then(|done| done.block_decode_bytes())
    }

    /// One reader: the pass is serial, and only a later run reads the units
    /// it finds in parallel.
    fn default_workers(&self) -> usize {
        self.done.get().map_or(1, |done| done.default_workers())
    }

    /// The whole pass: two forward decodes — the frontier and one behind it
    /// — and the skip scratch, its windows being on disk.
    fn default_worker_memory(&self) -> Option<WorkerMemory> {
        match self.done.get() {
            Some(done) => done.default_worker_memory(),
            None => Some(WorkerMemory::per_worker(PASS_BYTES)),
        }
    }

    /// The indexed source's, once there is one. Before it, nothing: a single
    /// member's only check is its trailer, which a pass stopped short of it
    /// has not reached, and reaching it would mean decoding the rest of the
    /// file.
    fn complete_reads(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            match self.done.get() {
                Some(done) => done.complete_reads().await,
                None => Ok(()),
            }
        })
    }
}

#[cfg(test)]
mod tests;
