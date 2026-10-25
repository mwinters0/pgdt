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
//! A fetched BGZF file is read here too, the rule coalescing its members as
//! the local arm's walk does (`docs/design/decisions.md`, "D116").
//!
//! **One partition until the end, then the indexed source.** The extent is
//! open until a read meets the file's end, so nothing cuts the file and one
//! forward pass serves the scan — `gz_seek`'s parallel pass, which runs chunks
//! of the file ahead of it on as many of the caller's threads as its
//! parallelism allows, lifting the inflate but not the scan (`ahead.rs`), a
//! fetched file's tasks and frontier reading one download of it
//! (`download.rs`); at the end the complete index, its windows in the
//! pass's spill file, opens the source a later run opens from its cache
//! ([`CompressedFileSource`],
//! [`FetchedCompressedSource`](crate::FetchedCompressedSource)), and every
//! read, partitioning and worker recommendation is that source's from then on
//! — so a cold `query`'s replay planned once the pass has ended reads units,
//! one planned before with one reader (`docs/design/decisions.md`, "D19").
//!
//! **No window is resident**: each is written as it is placed to a file the
//! run alone holds, in the directory `open`'s caller names
//! (`docs/design/decisions.md`, "D115").
//!
//! **Announced where its first read begins and where it completes**, on the
//! status channel as the `.xz` walk is: the source says why its scan is one
//! reader and whether its inflate runs ahead of it, the arrangement reading
//! only its advice (`docs/design/decisions.md`, "D64").

use std::future::Future;
use std::num::{NonZeroU64, NonZeroUsize};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use bytes::Bytes;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use pgdt_core::{Error, Result};

use crate::parallelism::{Parallelism, WorkerMemory, worker_count};
use crate::partition::Partitioning;
use crate::source::pool::{BufferPool, POOL_DEPTH, PooledBuffer, budget_bytes};
use crate::source::{ByteRangeSource, Extent, RemoteIdentity, WaitPolicy};

#[cfg(any(feature = "http", test))]
use super::super::FetchedCompressedSource;
use super::super::{CompressedFileSource, CompressionIndex, short_read};
use super::{CHECKPOINT_SPACING, GZIP, GzipIndex, INPUT_CHUNK, WindowProvider, gzip_error};

mod ahead;
#[cfg(any(feature = "http", test))]
mod download;
mod inflating;
mod pass;
mod spill;

use self::ahead::{Chunking, Frontier, Lift, Reads};
#[cfg(any(feature = "http", test))]
use self::download::Download;
use self::inflating::Inflating;
use self::pass::{Job, Pass, Route, SKIP_SCRATCH, Served};
use self::spill::PassWindows;

/// What one forward decode of the pass holds besides the plaintext it hands
/// out: the inflater and its window, the window copy a checkpoint is read out
/// of, and the compressed input chunk.
const DECODE_BYTES: u64 = gz_seek::DECODER_BYTES + gz_seek::WINDOW_SIZE as u64 + INPUT_CHUNK;

/// What the pass holds besides the plaintext it hands out: its two decodes —
/// the frontier and one behind it, a forward decode at most — and the scratch
/// a read skipping to its first wanted byte decodes into.
const PASS_BYTES: u64 = 2 * DECODE_BYTES + SKIP_SCRATCH as u64;

/// A gzip file read by its first pass ([`open`](crate::open)'s source for one
/// no cache has indexed), building its index as a forward decode reaches each
/// checkpoint.
///
/// **One forward pass at the frontier**, behind a lock and taken one read
/// at a time, so the checkpoints are placed in file order whoever reads,
/// whichever of the caller's threads decoded the chunks it takes; a
/// read behind it begins a second decode at the last checkpoint at or before
/// its first byte, kept across reads so a scan behind the frontier continues
/// rather than restarting.
///
/// **What the frontier hands out is checked at the member's end**, as every
/// byte of a single member is: its trailer is the only check the file carries,
/// the units' own CRC-32s being recorded from the plaintext this pass decodes.
/// A pass stopped short of it has compared nothing, and a later pass resumed
/// from its index compares the member's trailer against those recorded values
/// combined, so a corrupted prefix is still refused at the end. **A read
/// behind the frontier inside a closed unit is compared against that unit's
/// recorded CRC-32** as the decode leaves the unit, or as the run's reads
/// complete ([`ByteRangeSource::complete_reads`]); one inside the open unit
/// decodes the frontier's own bytes and is the frontier's to check.
pub(crate) struct FirstPass {
    feed: Feed,
    /// What the pass's status lines name the file by: its origin as a message
    /// to the user names it.
    named: String,
    /// The compressed file's length, as the open's probe found it.
    stored_size: u64,
    /// The decodes and the index as far as they have got, locked for
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
    /// How the file is cut into chunks for the tasks run ahead of the
    /// frontier.
    chunking: Chunking,
    /// The started line, said by the first read, once the parallelism it
    /// runs under is announced.
    started: Mutex<Option<Started>>,
    /// The one download a fetched feed's frontier and tasks read.
    #[cfg(any(feature = "http", test))]
    download: Option<Arc<Download>>,
    /// Told each time a task ahead lands, for a fetched read awaiting one.
    landed: Arc<Notify>,
}

/// Where a pass started, for its started line.
#[derive(Clone, Copy)]
enum Started {
    Fresh,
    Resumed { checkpoint: usize, plaintext: u64 },
}

/// Where the compressed bytes come from: a local file, pulled inside the
/// blocking task the decode runs in, or a transport fetched through between
/// decode steps (`docs/design/decisions.md`, "D15"), which only the remote
/// source and the tests construct, so `http` or a test build carries it.
pub(crate) enum Feed {
    File {
        path: PathBuf,
        file: Arc<std::fs::File>,
    },
    #[cfg(any(feature = "http", test))]
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
    /// at `spacing` and spilling their windows into `spill`, its first read
    /// saying it has started.
    pub(crate) fn spaced(
        named: String,
        feed: Feed,
        stored_size: u64,
        spacing: NonZeroU64,
        spill: &Path,
    ) -> Result<FirstPass> {
        Self::chunked(named, feed, stored_size, spacing, Chunking::SHIPPED, spill)
    }

    /// [`FirstPass::spaced`], cutting the file as `chunking` says for the
    /// tasks run ahead of the frontier.
    fn chunked(
        named: String,
        feed: Feed,
        stored_size: u64,
        spacing: NonZeroU64,
        chunking: Chunking,
        spill: &Path,
    ) -> Result<FirstPass> {
        let pass = gz_seek::Parallel::new(spacing, chunking.bytes, chunking.limit, stored_size)
            .map_err(gzip_error)?;
        let windows = PassWindows::fresh(spill);
        let frontier = Inflating::new(Frontier::new(pass));
        let started = Started::Fresh;
        Ok(Self::over(named, feed, stored_size, frontier, 0, None, windows, chunking, started))
    }

    /// A pass named `named` resumed at the last checkpoint of `index`, an
    /// incomplete index a previous pass over the *same* file saved, its
    /// windows read through the provider riding on it and those it places
    /// spilled into `spill`, its first read saying it has started and from
    /// which checkpoint.
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
        Self::resumed_chunked(named, feed, stored_size, index, Chunking::SHIPPED, spill)
    }

    /// [`FirstPass::resumed`], cutting the file as `chunking` says.
    fn resumed_chunked(
        named: String,
        feed: Feed,
        stored_size: u64,
        index: GzipIndex,
        chunking: Chunking,
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
        let pass =
            gz_seek::Parallel::resume(&index, &window, chunking.bytes, chunking.limit, stored_size)
                .map_err(gzip_error)?;
        // A pass resumed at a member start places that checkpoint again.
        let placed = checkpoints.len() - usize::from(last.start == gz_seek::Start::Member);
        let base = checkpoints.len();
        let pass_windows = PassWindows::after(windows, base, spill);
        let frontier = Inflating::new(Frontier::new(pass));
        let started = Started::Resumed { checkpoint: base - 1, plaintext: last.plaintext };
        let resumed_over = Some(index);
        Ok(Self::over(
            named,
            feed,
            stored_size,
            frontier,
            placed,
            resumed_over,
            pass_windows,
            chunking,
            started,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn over(
        named: String,
        feed: Feed,
        stored_size: u64,
        frontier: Inflating<Frontier>,
        placed: usize,
        resumed_over: Option<gz_seek::Index>,
        windows: PassWindows,
        chunking: Chunking,
        started: Started,
    ) -> FirstPass {
        let pass = Pass::new(frontier, placed, resumed_over);
        #[cfg(any(feature = "http", test))]
        let download = match &feed {
            Feed::File { .. } => None,
            Feed::Fetched(raw) => {
                Some(Arc::new(Download::new(Arc::clone(raw), chunking.bytes, stored_size)))
            }
        };
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
            chunking,
            started: Mutex::new(Some(started)),
            #[cfg(any(feature = "http", test))]
            download,
            landed: Arc::new(Notify::new()),
        }
    }

    /// How many tasks may run ahead of the frontier: every worker the
    /// announced parallelism affords at a task's charge but the one the
    /// frontier is, none before any is announced, and no more than there are
    /// chunks past the first (`ahead.rs`).
    fn ahead_tasks(&self) -> usize {
        let parallelism = self.hints.lock().unwrap_or_else(|e| e.into_inner()).parallelism;
        let task = WorkerMemory::per_worker(self.charges().1);
        let ahead = self.chunking.chunks(self.stored_size) - 1;
        let afforded = worker_count(parallelism.unwrap_or_default(), task).saturating_sub(1);
        afforded.min(usize::try_from(ahead).unwrap_or(usize::MAX))
    }

    /// What the pass holds besides its tasks, and what a task ahead holds: a
    /// fetched file's download adds its pieces to each
    /// (`Chunking::download_bytes`), a local file's nothing.
    fn charges(&self) -> (u64, u64) {
        #[cfg(any(feature = "http", test))]
        let download = if self.download.is_some() { self.chunking.download_bytes() } else { 0 };
        #[cfg(not(any(feature = "http", test)))]
        let download = 0;
        (PASS_BYTES + download, self.chunking.task_bytes() + download)
    }

    /// Say the pass has started, once, at its first read: the codec, where it
    /// resumes from if it does, and whether its inflate runs ahead on `tasks`
    /// more workers than the frontier or serially (`docs/design/decisions.md`,
    /// "D64").
    fn announce(&self, tasks: usize) {
        let Some(started) = self.started.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return;
        };
        let (source, codec, workers) = (&self.named, GZIP, tasks + 1);
        match (started, tasks) {
            (Started::Fresh, 0) => tracing::info!(
                %source,
                %codec,
                "first pass started, reading serially while it builds the index",
            ),
            (Started::Fresh, _) => tracing::info!(
                %source,
                %codec,
                workers,
                "first pass started, inflating in parallel while it builds the index, its scan \
                 one reader",
            ),
            (Started::Resumed { checkpoint, plaintext }, 0) => tracing::info!(
                %source,
                %codec,
                checkpoint,
                plaintext,
                "first pass started, resuming from a saved checkpoint and reading serially \
                 while it builds the index",
            ),
            (Started::Resumed { checkpoint, plaintext }, _) => tracing::info!(
                %source,
                %codec,
                checkpoint,
                plaintext,
                workers,
                "first pass started, resuming from a saved checkpoint and inflating in \
                 parallel while it builds the index, its scan one reader",
            ),
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
        let tasks = self.ahead_tasks();
        self.announce(tasks);
        let (out, filled, ended) = self.run(Job::Read { offset, len }, tasks).await?;
        let out = out.expect("a read is answered in a buffer");
        if ended {
            self.hand_over()?;
        }
        drop(turn);
        if strict && filled < len {
            return Err(short_read());
        }
        Ok(Bytes::from_owner(out).slice(..filled))
    }

    /// `job` run over the pass on its feed, `tasks` let run ahead of a pulled
    /// frontier: what a read filled — its buffer, how many bytes, and whether
    /// the frontier met the file's end.
    async fn run(&self, job: Job, tasks: usize) -> Result<(Option<PooledBuffer>, usize, bool)> {
        match &self.feed {
            Feed::File { file, .. } => self.run_pulled(Arc::clone(file), job, tasks).await,
            #[cfg(any(feature = "http", test))]
            Feed::Fetched(raw) => self.run_fetched(Arc::clone(raw), job, tasks).await,
        }
    }

    /// `job`, its decode pulling its compressed bytes from the file, the
    /// whole of it inside one blocking task, `tasks` let run ahead of the
    /// frontier each on a blocking task of its own.
    async fn run_pulled(
        &self,
        file: Arc<std::fs::File>,
        job: Job,
        tasks: usize,
    ) -> Result<(Option<PooledBuffer>, usize, bool)> {
        let (inner, windows, pool) =
            (Arc::clone(&self.inner), Arc::clone(&self.windows), Arc::clone(&self.pool));
        let stored_size = self.stored_size;
        let lift = Lift {
            reads: Reads::File(Arc::clone(&file)),
            stored_size,
            runtime: tokio::runtime::Handle::current(),
            tasks,
            landed: Arc::clone(&self.landed),
        };
        tokio::task::spawn_blocking(move || {
            let mut out = job.obtain(&pool);
            let mut pass = inner.lock().unwrap_or_else(|e| e.into_inner());
            pass.lift(&lift);
            let Some(route) = job.route(&mut pass)? else {
                return Ok((None, 0, false));
            };
            let mut filled = 0usize;
            loop {
                match pass.serve(
                    route,
                    job.offset(),
                    job.buffer(&mut out),
                    &mut filled,
                    &windows,
                    stored_size,
                )? {
                    Served::Full => return Ok((out, filled, false)),
                    Served::Ended => return Ok((out, filled, matches!(route, Route::Frontier))),
                    Served::Needs(at) => pass.refill_from_file(route, &file, at, stored_size)?,
                    Served::Waits => unreachable!("a pulled frontier waits where it stands"),
                }
            }
        })
        .await
        .map_err(Error::join)?
    }

    /// `job`, its decode fed bytes fetched through the transport, back on
    /// the runtime for each fetch and each wait for a task ahead: the
    /// frontier's out of the download its tasks read, a read behind it
    /// straight from the transport, the download holding nothing it has
    /// passed.
    #[cfg(any(feature = "http", test))]
    async fn run_fetched(
        &self,
        raw: Arc<dyn ByteRangeSource>,
        job: Job,
        tasks: usize,
    ) -> Result<(Option<PooledBuffer>, usize, bool)> {
        let stored_size = self.stored_size;
        let download = Arc::clone(self.download.as_ref().expect("a fetched feed has its download"));
        let lift = Arc::new(Lift {
            reads: Reads::Fetched(Arc::clone(&download)),
            stored_size,
            runtime: tokio::runtime::Handle::current(),
            tasks,
            landed: Arc::clone(&self.landed),
        });
        let mut state: Option<(Option<PooledBuffer>, usize, Route)> = None;
        let mut supply: Option<(u64, Bytes)> = None;
        loop {
            let (inner, windows, pool) =
                (Arc::clone(&self.inner), Arc::clone(&self.windows), Arc::clone(&self.pool));
            let given = supply.take();
            let carried = state.take();
            let lift = Arc::clone(&lift);
            let stepped = tokio::task::spawn_blocking(move || {
                let mut pass = inner.lock().unwrap_or_else(|e| e.into_inner());
                pass.lift(&lift);
                let (mut out, mut filled, route) = match carried {
                    Some(carried) => carried,
                    None => {
                        let out = job.obtain(&pool);
                        match job.route(&mut pass)? {
                            Some(route) => (out, 0, route),
                            None => return Ok(None),
                        }
                    }
                };
                if let Some((at, bytes)) = given {
                    pass.supply(route, at, &bytes);
                }
                let served = pass.serve(
                    route,
                    job.offset(),
                    job.buffer(&mut out),
                    &mut filled,
                    &windows,
                    stored_size,
                )?;
                Ok::<_, Error>(Some((out, filled, route, served)))
            })
            .await
            .map_err(Error::join)??;
            let Some((out, filled, route, served)) = stepped else {
                return Ok((None, 0, false));
            };
            match served {
                Served::Full => return Ok((out, filled, false)),
                Served::Ended => return Ok((out, filled, matches!(route, Route::Frontier))),
                Served::Needs(at) => {
                    let bytes = match route {
                        Route::Frontier => download.read_frontier(at).await?,
                        Route::Behind => {
                            let want = INPUT_CHUNK.min(stored_size.saturating_sub(at)) as usize;
                            raw.read_range(at, want).await?
                        }
                    };
                    supply = Some((at, bytes));
                    state = Some((out, filled, route));
                }
                Served::Waits => {
                    self.landed.notified().await;
                    state = Some((out, filled, route));
                }
            }
        }
    }

    /// Leave the decode behind the frontier where it sits inside a closed
    /// unit, decoding the unit to its end so its check is compared.
    ///
    /// Deficiency register: `deficiency: KD138` — run with no tasks, so on a
    /// fetched pass its lift sets the download's room to two pieces, whatever
    /// a read's lift last set, at every `complete_reads`, a save's included,
    /// until the next read: a task handed out under the wider room is refused
    /// the pieces past two and hands back nothing, the frontier decoding those
    /// bytes itself. **(c) unowned**; promoted by a reading of a fetched pass
    /// at a stated `--jobs` falling short of a local one's.
    async fn leave_behind(&self) -> Result<()> {
        let _turn = self.turn.lock().await;
        self.run(Job::Leave, 0).await.map(|_| ())
    }

    /// The pass has reached the end: the indexed source over the complete
    /// index, its windows still read out of the spill file, which every call
    /// is from now on, with what callers announced replayed on it, saying
    /// what the index holds.
    fn hand_over(&self) -> Result<()> {
        let index = self.lock().ended.clone().expect("the frontier reached the end");
        #[cfg(any(feature = "http", test))]
        if let Some(download) = &self.download {
            download.clear();
        }
        let windows = Arc::clone(&self.windows) as Arc<dyn WindowProvider>;
        let index = GzipIndex { index, windows: Some(windows) };
        let (units, members) = (index.unit_count(), index.member_count());
        let index = CompressionIndex::Gzip(index);
        let done: Arc<dyn ByteRangeSource> = match &self.feed {
            Feed::File { path, .. } => Arc::new(CompressedFileSource::with_index(path, index)?),
            #[cfg(any(feature = "http", test))]
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
            #[cfg(any(feature = "http", test))]
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
                (None, Some(frontier)) => {
                    Extent::Open { at_least: frontier.decode.pass.position() }
                }
                (None, None) => Extent::Open { at_least: 0 },
            })
        })
    }

    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        Box::pin(async move {
            match &self.feed {
                #[cfg(any(feature = "http", test))]
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
                #[cfg(any(feature = "http", test))]
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

    /// On a local file, the machine's cores capped at the file's chunk count:
    /// the scan is one partition whatever this says, and every worker past
    /// the frontier runs a chunk ahead of it. A fetched one, one, as the
    /// fetched source answers: its tasks run only under a stated count, each
    /// reading the one download its frontier reads (`docs/design/decisions.md`,
    /// "D2"). The indexed source's, once the pass has handed over to it.
    fn default_workers(&self) -> usize {
        if let Some(done) = self.done.get() {
            return done.default_workers();
        }
        match &self.feed {
            Feed::File { .. } => {
                let cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
                let chunks = self.chunking.chunks(self.stored_size);
                cores.min(usize::try_from(chunks).unwrap_or(usize::MAX))
            }
            #[cfg(any(feature = "http", test))]
            Feed::Fetched(_) => 1,
        }
    }

    /// The whole pass at one worker: two decodes, each charged a forward
    /// one's — the frontier and one behind it — and the skip scratch, its
    /// windows being on disk. Past one, a task ahead a worker, each charged
    /// what one holds running and waiting for the frontier, the pass's own
    /// charge inside the first's — on a file of more than one chunk, where a
    /// task can run at all. A fetched file's download adds its pieces to
    /// every worker (`Chunking::download_bytes`).
    fn default_worker_memory(&self) -> Option<WorkerMemory> {
        if let Some(done) = self.done.get() {
            return done.default_worker_memory();
        }
        let (pass, task) = self.charges();
        let ahead = self.chunking.chunks(self.stored_size) > 1;
        let per_worker = if ahead { pass.max(task) } else { pass };
        Some(WorkerMemory::per_worker(per_worker))
    }

    /// Leave the decode behind the frontier, comparing the check of a closed
    /// unit it sits inside, and then the indexed source's, once there is one.
    /// Nothing else: a single member's only check is its trailer, which a
    /// pass stopped short of it has not reached, and reaching it would mean
    /// decoding the rest of the file.
    fn complete_reads(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            self.leave_behind().await?;
            match self.done.get() {
                Some(done) => done.complete_reads().await,
                None => Ok(()),
            }
        })
    }
}

#[cfg(test)]
mod tests;
