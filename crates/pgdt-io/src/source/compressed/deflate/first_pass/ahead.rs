//! The pass's frontier: `gz_seek`'s parallel pass, and the tasks it hands out,
//! run ahead of it on the caller's threads.
//!
//! **The frontier is [`gz_seek::Parallel`] whatever runs beside it**: with no
//! task run it is the serial pass, and with tasks run it builds the same index
//! (`docs/design/decisions.md`, "D117"), so lifting the inflate changes who
//! decoded the bytes, never what the pass records. Nothing here starts a
//! thread of its own or runs a task the caller's
//! [`Parallelism`](crate::Parallelism) did not allow
//! (`docs/design/decisions.md`, "D1").
//!
//! **A task reads what the frontier's feed reads.** On a local file each task
//! is a `spawn_blocking` of its own reading the file positionally, as a unit
//! read is (`docs/design/decisions.md`, "D12"). On a fetched one it reads the
//! pass's one download (`download.rs`), each piece fetched on the runtime and
//! decoded on a `spawn_blocking`, as the fetched arm goes back to the runtime
//! between units (`docs/design/decisions.md`, "D15"), and its speculations are
//! handed out no further ahead than the download holds.
//!
//! **A wait never rests on a thread the runtime has yet to give.** A pulled
//! frontier, stepping on a blocking thread, waits there, running itself a task
//! no blocking thread has started; a fetched one hands the wait back to its
//! read, which awaits the task's landing on the runtime, where a fetch it
//! needs can run.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, mpsc};

use tokio::sync::Notify;

use pgdt_core::{Error, Result};

use super::super::super::Positioned;
use super::super::{INPUT_CHUNK, gzip_error};
#[cfg(any(feature = "http", test))]
use super::download::Download;

/// How many decodes a task may have handed out or held for the frontier,
/// and how many speculations or their guesses: what `Ahead::top_up` hands out
/// to and [`Chunking::task_bytes`] charges, two consumers of one pair. A
/// decode has a room of its own, so guesses held never keep a worker from
/// the decodes they let begin (`docs/design/decisions.md`, "D120").
const DECODES_A_TASK: usize = 2;
const GUESSES_A_TASK: usize = 5;

/// How many compressed bytes a chunk of the file is, which a speculation
/// guesses the start of and the engine then decodes: rapidgzip's length,
/// four input chunks, so a local task reads its chunk in four positioned
/// reads (`docs/design/decisions.md`, "D121").
#[cfg(not(feature = "gzip-1mib-chunks"))]
const CHUNK_BYTES: NonZeroU64 = NonZeroU64::new(4 << 20).unwrap();

/// The input chunk every gzip decode pulls in place of the shipped length,
/// the limit held: the `gzip-first-pass` figure's chunk-length leg, which is
/// never shipped (`scripts/measure.py`, `GZPASS_CHUNK_LEG`).
#[cfg(feature = "gzip-1mib-chunks")]
const CHUNK_BYTES: NonZeroU64 = NonZeroU64::new(INPUT_CHUNK).unwrap();

/// How a pass cuts its file into chunks: their compressed length, the most
/// plaintext one speculation or decode holds, and how many chunks past where
/// it would land just ahead of the frontier a speculation is placed
/// (`gz_seek::Parallel::set_lead`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Chunking {
    pub(super) bytes: NonZeroU64,
    pub(super) limit: usize,
    pub(super) lead: u64,
}

impl Chunking {
    /// The chunking a pass ships with: the limit `gz_seek`'s, a constant of
    /// its writers' blocks rather than of the chunk, a guess it cuts being
    /// continued from its stop; and a lead far enough that the decodes a
    /// guess lets begin begin far enough ahead of the frontier that it seldom
    /// waits for one (`docs/design/decisions.md`, "D120").
    pub(super) const SHIPPED: Chunking =
        Chunking { bytes: CHUNK_BYTES, limit: gz_seek::SPECULATION_LIMIT, lead: 4 };

    /// How many chunks a compressed file `stored_size` bytes long is cut
    /// into, at least one.
    pub(super) fn chunks(self, stored_size: u64) -> u64 {
        stored_size.div_ceil(self.bytes.get()).max(1)
    }

    /// What one task ahead of the frontier holds, running and then held for
    /// it, each term a bound `gz_seek` states on what the code writes: a
    /// running speculation, the larger of the two kinds of task, fed a read
    /// of [`INPUT_CHUNK`] on a local file or a piece of a chunk on a fetched
    /// one; [`DECODES_A_TASK`] decodes handed out or held for the frontier,
    /// and [`GUESSES_A_TASK`] guesses with the windows of the decodes queued
    /// behind them (`Ahead::top_up` holding the results to those multiples of
    /// the tasks' count); and the buffer a local task reads its input into.
    /// No decode's plaintext buffer is freed while the pass runs, the pass
    /// keeping each for the next decode, so the buffers never outnumber the
    /// decodes charged here (`docs/design/decisions.md`, "D124") but across a
    /// chain's reset (`KD153`). The
    /// frontier's own worker is charged a task too, and holds the pass, the
    /// decode it serves and the chain's end.
    ///
    /// Deficiency register: `deficiency: KD148` — no term charges what the
    /// allocator keeps of a buffer a task outgrows as it grows — a
    /// speculation's input, its sinks, a decode's events — the way mimalloc
    /// commits a freed page for its purge delay and glibc serves the next
    /// allocation from a thread's arena, trimming only past its threshold.
    /// What a task outgrows is under what it ends up holding, so it is up to
    /// as much again of those terms; they hold little on every file
    /// `gzip-resident` reads. **(c) unowned**; promoted by a reading over a
    /// file whose speculations fill their sinks.
    pub(super) const fn task_bytes(self) -> u64 {
        let (chunk, limit) = (self.bytes.get(), self.limit);
        let feed = if chunk > INPUT_CHUNK { chunk } else { INPUT_CHUNK };
        let speculation = gz_seek::ChunkTask::speculation_bytes(chunk, limit, feed);
        let decode = gz_seek::ChunkTask::decode_bytes(limit);
        let running = if speculation > decode { speculation } else { decode };
        let (decodes, guesses) = (DECODES_A_TASK as u64, GUESSES_A_TASK as u64);
        let held = decodes * decode + guesses * gz_seek::ChunkTask::GUESS_BYTES;
        running + held + INPUT_CHUNK
    }

    /// How many pieces a fetched pass's download may hold with `tasks` run
    /// ahead, two a worker: the piece the frontier stands in, twice as many
    /// chunks ahead of it as tasks run — how far ahead its speculations are
    /// handed out (`gz_seek::Parallel::speculation_through`) — and the piece
    /// after the last, where its speculation stops (`Download::last_chunk`).
    #[cfg(any(feature = "http", test))]
    pub(super) const fn download_pieces(tasks: usize) -> u64 {
        2 * (tasks as u64 + 1)
    }

    /// What a fetched pass's download holds a worker: two pieces of a chunk
    /// each ([`Chunking::download_pieces`]).
    #[cfg(any(feature = "http", test))]
    pub(super) const fn download_bytes(self) -> u64 {
        2 * self.bytes.get()
    }
}

/// The frontier's decode: the parallel pass, the tasks running ahead of it
/// where its feed runs any, and the download a fetched one reads.
pub(super) struct Frontier {
    pub(super) pass: gz_seek::Parallel,
    ahead: Option<Ahead>,
    #[cfg(any(feature = "http", test))]
    download: Option<Arc<Download>>,
    /// How many steps stood waiting for a decode, and of those, how many the
    /// frontier ran itself, no blocking thread having started it.
    #[cfg(test)]
    waits: (usize, usize),
}

impl Frontier {
    pub(super) fn new(pass: gz_seek::Parallel) -> Frontier {
        Frontier {
            pass,
            ahead: None,
            #[cfg(any(feature = "http", test))]
            download: None,
            #[cfg(test)]
            waits: (0, 0),
        }
    }

    /// Let `tasks` run ahead at once, each reading what `lift` reads, of
    /// `stored_size` bytes, on `runtime`: set at each read, so the count
    /// follows the parallelism announced last, a fetched download's room
    /// with it. None is the serial pass.
    pub(super) fn lift(&mut self, lift: &Lift) {
        #[cfg(any(feature = "http", test))]
        if let Reads::Fetched(download) = &lift.reads {
            download.set_room(Chunking::download_pieces(lift.tasks));
            self.download = Some(Arc::clone(download));
        }
        if lift.tasks == 0 && self.ahead.is_none() {
            return;
        }
        let ahead = self.ahead.get_or_insert_with(|| {
            let (landing, landed) = mpsc::channel();
            Ahead {
                reads: lift.reads.clone(),
                stored_size: lift.stored_size,
                runtime: lift.runtime.clone(),
                notify: Arc::clone(&lift.landed),
                tasks: 0,
                running: 0,
                landing,
                landed,
                unstarted: BTreeMap::new(),
                failed: BTreeSet::new(),
            }
        });
        ahead.tasks = lift.tasks;
    }

    /// A step of the pass, the tasks' results it can use handed to it first
    /// and the tasks it has room for handed out, and the task it stands
    /// waiting for where its read is to await it: a pulled frontier waits
    /// here, so its next step goes on with the result.
    pub(super) fn read(
        &mut self,
        input: &[u8],
        out: &mut [u8],
    ) -> Result<(gz_seek::ParallelStep, Option<gz_seek::TaskId>)> {
        self.release();
        let last = self.last_chunk();
        if let Some(ahead) = &mut self.ahead {
            ahead.reap(&mut self.pass);
            ahead.top_up(&mut self.pass, last);
        }
        let step = self.pass.read(input, out).map_err(gzip_error)?;
        self.release();
        let Some(id) = step.wait else { return Ok((step, None)) };
        #[cfg(test)]
        {
            self.waits.0 += 1;
        }
        match &mut self.ahead {
            Some(ahead) if ahead.pulls() => {
                let _ran_here = ahead.wait_for(id, &mut self.pass)?;
                #[cfg(test)]
                {
                    self.waits.1 += usize::from(_ran_here);
                }
                Ok((step, None))
            }
            Some(ahead) => {
                ahead.awaitable(id)?;
                Ok((step, Some(id)))
            }
            None => Err(unrun(id)),
        }
    }

    /// Drop what the download holds behind where the frontier now stands.
    fn release(&self) {
        #[cfg(any(feature = "http", test))]
        if let Some(download) = &self.download {
            download.release_before(self.pass.compressed_position());
        }
    }

    /// The last chunk a speculation may be handed out on: any on a local
    /// file, one the download's room reaches on a fetched one.
    fn last_chunk(&self) -> u64 {
        #[cfg(any(feature = "http", test))]
        if let Some(download) = &self.download {
            return download.last_chunk();
        }
        u64::MAX
    }

    /// How many chunks' decodes the frontier has taken from a task.
    #[cfg(test)]
    pub(super) fn taken(&self) -> u64 {
        self.pass.taken()
    }

    /// How many steps stood waiting for a decode, and how many of those
    /// decodes the frontier ran itself.
    #[cfg(test)]
    pub(super) fn waits(&self) -> (usize, usize) {
        self.waits
    }

    /// What a read does before it steps, with no step: every result that has
    /// come back handed to the pass, and the tasks it has room for handed
    /// out. Answers how many decodes the pass then holds for the frontier,
    /// and how many tasks are still running, so a test can wait for a
    /// decode to land rather than race the frontier to it.
    #[cfg(test)]
    pub(super) fn settle(&mut self) -> (usize, usize) {
        self.release();
        let last = self.last_chunk();
        let Some(ahead) = &mut self.ahead else { return (self.pass.ready(), 0) };
        ahead.reap(&mut self.pass);
        ahead.top_up(&mut self.pass, last);
        (self.pass.ready(), ahead.running)
    }
}

/// What a task reads its chunk from: the file a pulled feed reads, or the
/// download a fetched one reads.
#[derive(Clone)]
pub(super) enum Reads {
    File(Arc<std::fs::File>),
    #[cfg(any(feature = "http", test))]
    Fetched(Arc<Download>),
}

/// What a read lets run ahead of the frontier ([`Frontier::lift`]).
pub(super) struct Lift {
    pub(super) reads: Reads,
    pub(super) stored_size: u64,
    pub(super) runtime: tokio::runtime::Handle,
    pub(super) tasks: usize,
    /// Told each time a task lands, for a read awaiting one.
    pub(super) landed: Arc<Notify>,
}

/// The tasks running ahead of the frontier, and the channel their results
/// come back on.
struct Ahead {
    reads: Reads,
    stored_size: u64,
    runtime: tokio::runtime::Handle,
    notify: Arc<Notify>,
    /// How many may run at once.
    tasks: usize,
    running: usize,
    landing: mpsc::Sender<Landed>,
    landed: mpsc::Receiver<Landed>,
    /// Each task handed out, until its result lands: empty once a blocking
    /// thread or the frontier has taken it to run.
    unstarted: BTreeMap<gz_seek::TaskId, Slot>,
    /// Tasks that panicked, which the pass would otherwise wait for forever.
    failed: BTreeSet<gz_seek::TaskId>,
}

/// A task handed out, taken by whichever runs it first.
type Slot = Arc<Mutex<Option<gz_seek::ChunkTask>>>;

/// What a task came back with.
enum Landed {
    Done(gz_seek::ChunkDone),
    Failed(gz_seek::TaskId),
}

impl Ahead {
    /// Whether the tasks read a local file, so a wait is made here.
    fn pulls(&self) -> bool {
        matches!(self.reads, Reads::File(_))
    }

    /// Hand the pass every result that has come back.
    fn reap(&mut self, pass: &mut gz_seek::Parallel) {
        while let Ok(landed) = self.landed.try_recv() {
            self.land(landed, pass);
        }
    }

    /// Hand out tasks while fewer than `tasks` run: a decode while the pass
    /// has fewer than [`DECODES_A_TASK`] times that many handed out or held
    /// for the frontier, and otherwise a speculation, on a chunk at or before
    /// `last`, while it has fewer than [`GUESSES_A_TASK`] times as many
    /// handed out or guesses held ([`Chunking::task_bytes`]).
    ///
    /// None is handed out from when a speculation finds nothing until the
    /// frontier next crosses a block or member start
    /// (`docs/design/decisions.md`, "D125"), so over a member whose writer ends no block before its last
    /// only the speculations handed out before the first lands run, and the
    /// pass is read at its serial pace at every count (`gzip-first-pass`).
    fn top_up(&mut self, pass: &mut gz_seek::Parallel, last: u64) {
        while self.running < self.tasks {
            let decode = match pass.decodes_pending() < DECODES_A_TASK * self.tasks {
                true => pass.decode_task(),
                false => None,
            };
            let task =
                decode.or_else(|| match pass.guesses_pending() < GUESSES_A_TASK * self.tasks {
                    true => pass.speculation_through(last),
                    false => None,
                });
            let Some(task) = task else { return };
            self.spawn(task);
        }
    }

    /// Wait for task `id`'s result, handing the pass every result that comes
    /// back meanwhile — or run it here where no blocking thread has started
    /// it, so a wait never rests on a thread the runtime has yet to give:
    /// answers whether it ran here.
    fn wait_for(&mut self, id: gz_seek::TaskId, pass: &mut gz_seek::Parallel) -> Result<bool> {
        let Some(slot) = self.unstarted.get(&id) else {
            // Not handed out, or landed: the pass waits for a landed task
            // only where it failed, its result never handed over.
            self.outcome(id)?;
            return Err(unrun(id));
        };
        let file = match &self.reads {
            Reads::File(file) => Arc::clone(file),
            #[cfg(any(feature = "http", test))]
            Reads::Fetched(_) => unreachable!("only a pulled frontier waits where it stands"),
        };
        if let Some(task) = take(slot) {
            let landed = run_caught(task, &file, self.stored_size);
            self.land(landed, pass);
            return self.outcome(id).map(|()| true);
        }
        loop {
            let landed = self.landed.recv().map_err(|_| unrun(id))?;
            if self.land(landed, pass) == id {
                return self.outcome(id).map(|()| false);
            }
        }
    }

    /// Whether a read may await task `id`: one handed out and not yet
    /// landed, its result then coming back on the channel. A landed one the
    /// pass still waits for failed.
    fn awaitable(&self, id: gz_seek::TaskId) -> Result<()> {
        if self.unstarted.contains_key(&id) {
            return Ok(());
        }
        self.outcome(id)?;
        Err(unrun(id))
    }

    /// What task `id`, landed, came to: its failure where it panicked.
    fn outcome(&self, id: gz_seek::TaskId) -> Result<()> {
        match self.failed.contains(&id) {
            true => Err(Error::Io(std::io::Error::other(
                "a task of this gzip file's first pass failed before its result came back",
            ))),
            false => Ok(()),
        }
    }

    /// Hand the pass one result, answering which task it came from.
    fn land(&mut self, landed: Landed, pass: &mut gz_seek::Parallel) -> gz_seek::TaskId {
        self.running -= 1;
        let id = match landed {
            Landed::Done(done) => {
                let id = done.id();
                pass.done(done);
                id
            }
            Landed::Failed(id) => {
                self.failed.insert(id);
                id
            }
        };
        self.unstarted.remove(&id);
        id
    }

    /// Run `task` where its feed's tasks run, its result sent back: on a
    /// blocking thread of its own unless the frontier has taken it to run
    /// first, or on the runtime over the download, each piece decoded on a
    /// blocking thread.
    fn spawn(&mut self, task: gz_seek::ChunkTask) {
        let (stored_size, landing, notify) =
            (self.stored_size, self.landing.clone(), Arc::clone(&self.notify));
        let id = task.id();
        let slot: Slot = Arc::new(Mutex::new(Some(task)));
        let ours = Arc::clone(&slot);
        self.unstarted.insert(id, slot);
        self.running += 1;
        // Detached: a result nobody waits for any more lands in a channel
        // whose receiver may be gone, and is dropped.
        match &self.reads {
            Reads::File(file) => {
                let file = Arc::clone(file);
                drop(self.runtime.spawn_blocking(move || {
                    if let Some(task) = take(&ours) {
                        let _ = landing.send(run_caught(task, &file, stored_size));
                        notify.notify_one();
                    }
                }));
            }
            #[cfg(any(feature = "http", test))]
            Reads::Fetched(download) => {
                let download = Arc::clone(download);
                drop(self.runtime.spawn(async move {
                    if let Some(task) = take(&ours) {
                        let _ = landing.send(run_downloaded(task, &download).await);
                        notify.notify_one();
                    }
                }));
            }
        }
    }
}

/// The task in `slot`, where nothing has taken it yet.
fn take(slot: &Slot) -> Option<gz_seek::ChunkTask> {
    slot.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// `task` fed from the download until it is done, each piece it is handed
/// decoded on a blocking thread, a panic landing as its failure. A piece the
/// download refuses or fails to fetch leaves it unfinished, so it hands back
/// nothing and the frontier meets those bytes, and the failure, itself.
#[cfg(any(feature = "http", test))]
async fn run_downloaded(mut task: gz_seek::ChunkTask, download: &Download) -> Landed {
    let id = task.id();
    while let Some(at) = task.wants() {
        let Ok(Some(bytes)) = download.read(at).await else { break };
        let ended = bytes.is_empty();
        match tokio::task::spawn_blocking(move || {
            task.supply(&bytes);
            task
        })
        .await
        {
            Ok(supplied) => task = supplied,
            Err(_) => return Landed::Failed(id),
        }
        if ended && task.wants() == Some(at) {
            break;
        }
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task.finish())) {
        Ok(done) => Landed::Done(done),
        Err(_) => Landed::Failed(id),
    }
}

/// `task` run to its result, a panic landing as its failure.
fn run_caught(task: gz_seek::ChunkTask, file: &std::fs::File, stored_size: u64) -> Landed {
    let id = task.id();
    let run = std::panic::AssertUnwindSafe(|| run_task(task, file, stored_size));
    match std::panic::catch_unwind(run) {
        Ok(done) => Landed::Done(done),
        Err(_) => Landed::Failed(id),
    }
}

/// `task` fed from `file` until it is done. A read that fails leaves it
/// unfinished, so it hands back nothing and the frontier meets those bytes,
/// and the failure, itself.
fn run_task(
    mut task: gz_seek::ChunkTask,
    file: &std::fs::File,
    stored_size: u64,
) -> gz_seek::ChunkDone {
    let mut buf = Vec::new();
    while let Some(at) = task.wants() {
        let want = INPUT_CHUNK.min(stored_size.saturating_sub(at)) as usize;
        buf.resize(want, 0);
        let Ok(got) = Positioned::read_at(file, at, &mut buf) else { break };
        task.supply(&buf[..got]);
        if got == 0 && task.wants() == Some(at) {
            break;
        }
    }
    task.finish()
}

/// The pass waiting for a task this frontier never ran: a defect, refused
/// rather than waited on forever.
fn unrun(id: gz_seek::TaskId) -> Error {
    Error::Io(std::io::Error::other(format!(
        "this gzip file's first pass waits for the task on chunk {} that nothing runs",
        id.chunk()
    )))
}
