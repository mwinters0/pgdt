//! The pass's frontier: `gz_seek`'s parallel pass, and the tasks it hands out,
//! run ahead of it on the caller's threads.
//!
//! **The frontier is [`gz_seek::Parallel`] whatever runs beside it**: with no
//! task run it is the serial pass, and with tasks run it builds the same index
//! (`docs/design/decisions.md`, "D117"), so lifting the inflate changes who
//! decoded the bytes, never what the pass records. Each task is a
//! `spawn_blocking` of its own reading the file positionally, as a unit read
//! is (`docs/design/decisions.md`, "D12"); nothing here starts a thread of its
//! own or runs a task the caller's [`Parallelism`](crate::Parallelism) did not
//! allow (`docs/design/decisions.md`, "D1").
//!
//! **Only a pulled feed runs tasks.** Each task reads its chunk itself, and the
//! engine's decode of it reads it again, so over a transport every task would
//! download what the frontier then jumps past; a fetched pass is the serial
//! pass until its feed holds what it fetched for the tasks to read.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, mpsc};

use pgdt_core::{Error, Result};

use super::super::super::Positioned;
use super::super::{INPUT_CHUNK, gzip_error};

/// How many compressed bytes a chunk of the file is, which a speculation
/// guesses the start of and the engine then decodes: the input chunk every
/// gzip decode pulls, so a task reads its chunk in one positioned read.
const CHUNK_BYTES: NonZeroU64 = NonZeroU64::new(INPUT_CHUNK).unwrap();

/// The most plaintext a speculation decodes, and so a chunk's decode holds:
/// sixteen times its chunk, the compression ratio past which a chunk is cut
/// short at its last block boundary. Nothing begins there, so the chain of
/// windows ends with it: the frontier decodes on to the next chunk's start and
/// that chunk too before tasks run ahead of it again.
const CHUNK_PLAINTEXT_LIMIT: usize = 16 * INPUT_CHUNK as usize;

/// How a pass cuts its file into chunks: their compressed length, and the most
/// plaintext one speculation or decode holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Chunking {
    pub(super) bytes: NonZeroU64,
    pub(super) limit: usize,
}

impl Chunking {
    /// The chunking a pass ships with.
    pub(super) const SHIPPED: Chunking =
        Chunking { bytes: CHUNK_BYTES, limit: CHUNK_PLAINTEXT_LIMIT };

    /// How many chunks a compressed file `stored_size` bytes long is cut
    /// into, at least one.
    pub(super) fn chunks(self, stored_size: u64) -> u64 {
        stored_size.div_ceil(self.bytes.get()).max(1)
    }

    /// What one task ahead of the frontier holds, running and then held for
    /// it: a speculation's compressed bytes — its chunk, and at most the
    /// limit's worth past it, the block it stops in — its plaintext twice over
    /// while it writes markers, or a decode's window, engine and plaintext,
    /// the speculation being the larger; then one decode's plaintext and two
    /// guesses' marked histories waiting for the frontier (`Ahead::top_up`
    /// holding the results to the tasks' count); and the buffer the task
    /// reads its input into.
    pub(super) const fn task_bytes(self) -> u64 {
        let limit = self.limit as u64;
        let guess = 2 * gz_seek::WINDOW_SIZE as u64;
        self.bytes.get() + limit + 2 * limit + limit + 2 * guess + INPUT_CHUNK
    }
}

/// The frontier's decode: the parallel pass, and the tasks running ahead of
/// it where a pulled feed runs any.
pub(super) struct Frontier {
    pub(super) pass: gz_seek::Parallel,
    ahead: Option<Ahead>,
}

impl Frontier {
    pub(super) fn new(pass: gz_seek::Parallel) -> Frontier {
        Frontier { pass, ahead: None }
    }

    /// Let `tasks` run ahead at once, each pulling its bytes from `file`, of
    /// `stored_size` bytes, on `runtime`'s blocking threads: set at each read,
    /// so the count follows the parallelism announced last. None is the
    /// serial pass.
    pub(super) fn lift(&mut self, lift: &Lift) {
        if lift.tasks == 0 && self.ahead.is_none() {
            return;
        }
        let ahead = self.ahead.get_or_insert_with(|| {
            let (landing, landed) = mpsc::channel();
            Ahead {
                file: Arc::clone(&lift.file),
                stored_size: lift.stored_size,
                runtime: lift.runtime.clone(),
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
    /// and the tasks it has room for handed out; where it stands waiting for
    /// one, the step waits for it, so the next goes on with its result.
    pub(super) fn read(&mut self, input: &[u8], out: &mut [u8]) -> Result<gz_seek::ParallelStep> {
        if let Some(ahead) = &mut self.ahead {
            ahead.reap(&mut self.pass);
            ahead.top_up(&mut self.pass);
        }
        let step = self.pass.read(input, out).map_err(gzip_error)?;
        if let Some(id) = step.wait {
            match &mut self.ahead {
                Some(ahead) => ahead.wait_for(id, &mut self.pass)?,
                None => return Err(unrun(id)),
            }
        }
        Ok(step)
    }

    /// How many chunks' decodes the frontier has taken from a task.
    #[cfg(test)]
    pub(super) fn taken(&self) -> u64 {
        self.pass.taken()
    }
}

/// What a pulled read lets run ahead of the frontier ([`Frontier::lift`]).
pub(super) struct Lift {
    pub(super) file: Arc<std::fs::File>,
    pub(super) stored_size: u64,
    pub(super) runtime: tokio::runtime::Handle,
    pub(super) tasks: usize,
}

/// The tasks running ahead of the frontier, and the channel their results
/// come back on.
struct Ahead {
    file: Arc<std::fs::File>,
    stored_size: u64,
    runtime: tokio::runtime::Handle,
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
    /// Hand the pass every result that has come back.
    fn reap(&mut self, pass: &mut gz_seek::Parallel) {
        while let Ok(landed) = self.landed.try_recv() {
            self.land(landed, pass);
        }
    }

    /// Hand out tasks while fewer than `tasks` run and the pass holds fewer
    /// than twice that many handed out or waiting for it — a chunk's guess and
    /// its decode each counting — so what waits for the frontier is at most
    /// one decode a task ([`Chunking::task_bytes`]).
    fn top_up(&mut self, pass: &mut gz_seek::Parallel) {
        while self.running < self.tasks && pass.pending() < 2 * self.tasks {
            let Some(task) = pass.task() else { return };
            self.spawn(task);
        }
    }

    /// Wait for task `id`'s result, handing the pass every result that comes
    /// back meanwhile — or run it here where no blocking thread has started
    /// it, so a wait never rests on a thread the runtime has yet to give.
    fn wait_for(&mut self, id: gz_seek::TaskId, pass: &mut gz_seek::Parallel) -> Result<()> {
        let Some(slot) = self.unstarted.get(&id) else {
            // Not handed out, or landed: the pass waits for a landed task
            // only where it failed, its result never handed over.
            self.outcome(id)?;
            return Err(unrun(id));
        };
        if let Some(task) = take(slot) {
            let landed = run_caught(task, &self.file, self.stored_size);
            self.land(landed, pass);
            return self.outcome(id);
        }
        loop {
            let landed = self.landed.recv().map_err(|_| unrun(id))?;
            if self.land(landed, pass) == id {
                return self.outcome(id);
            }
        }
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

    /// Run `task` on a blocking thread of its own, its result sent back,
    /// unless the frontier has taken it to run first.
    fn spawn(&mut self, task: gz_seek::ChunkTask) {
        let (file, stored_size, landing) =
            (Arc::clone(&self.file), self.stored_size, self.landing.clone());
        let id = task.id();
        let slot: Slot = Arc::new(Mutex::new(Some(task)));
        let ours = Arc::clone(&slot);
        self.unstarted.insert(id, slot);
        self.running += 1;
        // Detached: a result nobody waits for any more lands in a channel
        // whose receiver may be gone, and is dropped.
        drop(self.runtime.spawn_blocking(move || {
            if let Some(task) = take(&ours) {
                let _ = landing.send(run_caught(task, &file, stored_size));
            }
        }));
    }
}

/// The task in `slot`, where nothing has taken it yet.
fn take(slot: &Slot) -> Option<gz_seek::ChunkTask> {
    slot.lock().unwrap_or_else(|e| e.into_inner()).take()
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
