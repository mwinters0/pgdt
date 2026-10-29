//! The instrument the parallel first pass's scheduling is tuned on: what a
//! speculated chunk costs on one thread, and [`pgdt_gzip_seek::Parallel`] driven as
//! `pgdt-io`'s `first_pass/ahead.rs` drives it, over a file held in memory and
//! with no scan beside it. Never a figure: it rebuilds in seconds where `pgdt`
//! takes a quarter of an hour, and a lever moves the pass here first.
//!
//! ```sh
//! spec_cost <file.gz> forward|spec|par<jobs>
//! ```
//!
//! # The three shapes
//!
//! - **`forward`**: the serial first pass, [`pgdt_gzip_seek::Forward`] over the whole
//!   file.
//! - **`spec`**: every chunk's speculation as `pgdt-io` cuts them (4 MiB
//!   chunks, [`pgdt_gzip_seek::SPECULATION_LIMIT`]), one after another on this
//!   thread, each fed a MiB at a time. Its time over `forward`'s is a
//!   speculated chunk's cost in serial chunks.
//! - **`par<jobs>`**: the frontier on this thread and `jobs - 1` tasks at a
//!   time on as many worker threads, the tasks topped up before each read as
//!   `Ahead::top_up` tops them up — while fewer run than the workers, a decode
//!   while fewer than `DECODES` (default 2, `ahead.rs`'s `DECODES_A_TASK`)
//!   times the tasks are handed out or held, else a speculation while fewer
//!   than `GUESSES` (default 5, its `GUESSES_A_TASK`) times as many are — and
//!   a decode the frontier waits for run here where no worker has started
//!   it, as `Ahead::wait_for` runs it; each speculation placed `LEAD`
//!   (default 4, `Chunking::SHIPPED`'s) chunks past where it would land. The
//!   environment's `DECODES`, `GUESSES` and `LEAD` try another room and
//!   lead.
//!
//! # What `par` prints
//!
//! Beside the wall time and the plaintext: the tasks handed out by kind, and
//! each kind's time running (`spec_cpu`, `decode_cpu`), a task the frontier
//! ran itself included (`ran_here`); how much of the workers' time was idle
//! (`idle`, the share of `jobs - 1` times the wall time those two leave, so
//! understated by what the frontier ran); results the pass no longer wanted when they landed
//! (`unwanted`); the frontier's time split between its own engine
//! (`f_decode`), handing out a taken decode (`f_serve`) and waiting for one
//! (`f_wait`), how many waits there were and how long before the frontier
//! reached it the decode waited for was handed out, queue time included, on
//! average (`head_ms`); the chunks
//! the frontier took
//! from a decode (`taken`), and the runs they came in — each a stretch of
//! taken chunks the frontier's engine decoded nothing between — as their
//! count, mean and longest.

use std::collections::BTreeMap;
use std::hint::black_box;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;

use pgdt_gzip_seek::{
    ChunkDone, ChunkTask, Forward, Parallel, Progress, SPECULATION_LIMIT, TaskId,
};

const MIB: usize = 1 << 20;

/// The spacing the pass places checkpoints at, `pgdt-io`'s.
const SPACING: NonZeroU64 = NonZeroU64::new(24 << 20).unwrap();

/// The chunk length `pgdt-io` cuts a file into.
const CHUNK: NonZeroU64 = NonZeroU64::new(4 * MIB as u64).unwrap();

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("a .gz file");
    let shape = args.next().expect("forward, spec or par<jobs>");
    let file = Arc::new(std::fs::read(&path).expect("the file read"));
    let t = Instant::now();
    let line = match shape.as_str() {
        "forward" => format!("plaintext={}", forward(&file)),
        "spec" => speculations(&file),
        _ => {
            let jobs = shape.strip_prefix("par").and_then(|j| j.parse().ok());
            parallel(&file, jobs.filter(|&j| j > 0).expect("par<jobs>, jobs at least 1"))
        }
    };
    println!("{shape} s={:.3} {line}", t.elapsed().as_secs_f64());
}

/// The serial first pass: the plaintext it produced.
fn forward(file: &[u8]) -> u64 {
    let mut pass = Forward::new(SPACING).expect("a pass");
    let mut out = vec![0u8; MIB];
    let (mut taken, mut delivered) = (0usize, 0u64);
    loop {
        let end = (taken + MIB).min(file.len());
        let step = pass.read(&file[taken..end], &mut out).expect("a step");
        taken += step.progress.consumed;
        delivered += step.progress.produced as u64;
        if step.progress.produced > 0 {
            black_box(out[step.progress.produced - 1]);
        }
        if step.progress == Progress::default() && step.checkpoint.is_none() && end == file.len() {
            break;
        }
    }
    pass.finish().expect("an index");
    delivered
}

/// Every chunk's speculation on this thread.
fn speculations(file: &[u8]) -> String {
    let mut pass =
        Parallel::new(SPACING, CHUNK, SPECULATION_LIMIT, file.len() as u64).expect("a pass");
    let (mut chunks, mut found, mut held) = (0u64, 0u64, 0usize);
    while let Some(task) = pass.task() {
        let done = run(task, file);
        held += done.held();
        found += u64::from(done.held() > 0);
        chunks += 1;
    }
    format!("chunks={chunks} with_guess={found} held={held}")
}

/// A task fed from the file until it is done.
fn run(mut task: ChunkTask, file: &[u8]) -> ChunkDone {
    while let Some(at) = task.wants() {
        let at = usize::try_from(at).expect("an offset").min(file.len());
        let end = (at + MIB).min(file.len());
        task.supply(&file[at..end]);
        if at == end && task.wants() == Some(at as u64) {
            break;
        }
    }
    task.finish()
}

/// A task handed out, taken by whichever runs it first.
type Slot = Arc<Mutex<Option<ChunkTask>>>;

/// What the tasks and the frontier did.
#[derive(Default)]
struct Tally {
    specs: u64,
    decodes: u64,
    unwanted: u64,
    spec_s: f64,
    decode_s: f64,
    waits: u64,
    ran_here: u64,
    /// Summed over the waits: how long the decode waited for had run when
    /// the frontier reached it.
    head: f64,
    f_decode: f64,
    f_serve: f64,
    f_wait: f64,
    runs: Vec<u64>,
}

/// A count from the environment, or `default`.
fn env_count(name: &str, default: usize) -> usize {
    std::env::var(name).map_or(default, |v| v.parse().expect("a count"))
}

/// The pass and its tasks, driven as `ahead.rs` drives them.
struct Driver {
    pass: Parallel,
    tasks: usize,
    decodes_a_task: usize,
    guesses_a_task: usize,
    work: mpsc::Sender<Slot>,
    landed: mpsc::Receiver<(ChunkDone, f64)>,
    /// Each task handed out until it lands, and when it was handed out.
    unstarted: BTreeMap<TaskId, (Slot, Instant)>,
    running: usize,
    tally: Tally,
}

impl Driver {
    /// Hand the pass one result.
    fn land(&mut self, done: ChunkDone, s: f64) -> TaskId {
        let id = done.id();
        match id.is_decode() {
            true => self.tally.decode_s += s,
            false => self.tally.spec_s += s,
        }
        self.tally.unwanted += u64::from(!self.pass.wants(id));
        self.pass.done(done);
        self.unstarted.remove(&id);
        self.running -= 1;
        id
    }

    /// Every result that has come back, handed to the pass.
    fn reap(&mut self) {
        while let Ok((done, s)) = self.landed.try_recv() {
            self.land(done, s);
        }
    }

    /// `Ahead::top_up`.
    fn top_up(&mut self) {
        while self.running < self.tasks {
            let decode = match self.pass.decodes_pending() < self.decodes_a_task * self.tasks {
                true => self.pass.decode_task(),
                false => None,
            };
            let task = decode.or_else(|| {
                match self.pass.guesses_pending() < self.guesses_a_task * self.tasks {
                    true => self.pass.speculation_through(u64::MAX),
                    false => None,
                }
            });
            let Some(task) = task else { return };
            let id = task.id();
            match id.is_decode() {
                true => self.tally.decodes += 1,
                false => self.tally.specs += 1,
            }
            let slot: Slot = Arc::new(Mutex::new(Some(task)));
            self.unstarted.insert(id, (Arc::clone(&slot), Instant::now()));
            self.running += 1;
            self.work.send(slot).expect("the workers");
        }
    }

    /// `Ahead::wait_for`: task `id` run here where no worker has started it,
    /// or its result waited for.
    fn wait_for(&mut self, id: TaskId, file: &[u8]) {
        let waited = Instant::now();
        self.tally.waits += 1;
        let (slot, handed) = self.unstarted.get(&id).cloned().expect("a task handed out");
        self.tally.head += (waited - handed).as_secs_f64();
        let task = slot.lock().expect("a slot").take();
        if let Some(task) = task {
            self.tally.ran_here += 1;
            let ran = Instant::now();
            let done = run(task, file);
            self.land(done, ran.elapsed().as_secs_f64());
        } else {
            loop {
                let (done, s) = self.landed.recv().expect("a result");
                if self.land(done, s) == id {
                    break;
                }
            }
        }
        self.tally.f_wait += waited.elapsed().as_secs_f64();
    }
}

/// The pass driven as `ahead.rs` drives it, `jobs - 1` tasks ahead.
fn parallel(file: &Arc<Vec<u8>>, jobs: usize) -> String {
    let tasks = jobs - 1;
    let mut pass =
        Parallel::new(SPACING, CHUNK, SPECULATION_LIMIT, file.len() as u64).expect("a pass");
    pass.set_lead(env_count("LEAD", 4) as u64);
    let (work, queue) = mpsc::channel::<Slot>();
    let queue = Arc::new(Mutex::new(queue));
    let (landing, landed) = mpsc::channel::<(ChunkDone, f64)>();
    let workers: Vec<_> = (0..tasks)
        .map(|_| {
            let (queue, landing, file) = (Arc::clone(&queue), landing.clone(), Arc::clone(file));
            std::thread::spawn(move || {
                loop {
                    // The guard dropped before the task runs, so every worker
                    // runs one at once.
                    let Ok(slot) = queue.lock().expect("the queue").recv() else { return };
                    let Some(task) = slot.lock().expect("a slot").take() else { continue };
                    let t = Instant::now();
                    let done = run(task, &file);
                    let _ = landing.send((done, t.elapsed().as_secs_f64()));
                }
            })
        })
        .collect();
    let mut driver = Driver {
        pass,
        tasks,
        decodes_a_task: env_count("DECODES", 2),
        guesses_a_task: env_count("GUESSES", 5),
        work,
        landed,
        unstarted: BTreeMap::new(),
        running: 0,
        tally: Tally::default(),
    };
    let mut out = vec![0u8; MIB];
    let (mut delivered, mut run_len, mut last_taken) = (0u64, 0u64, 0u64);
    let t = Instant::now();
    loop {
        driver.reap();
        driver.top_up();
        let at = usize::try_from(driver.pass.compressed_position()).expect("an offset");
        let end = (at + MIB).min(file.len());
        let stepped = Instant::now();
        let step = driver.pass.read(&file[at..end], &mut out).expect("a step");
        let s = stepped.elapsed().as_secs_f64();
        delivered += step.progress.produced as u64;
        let taken = driver.pass.taken();
        run_len += taken - last_taken;
        last_taken = taken;
        if step.progress.consumed > 0 {
            driver.tally.f_decode += s;
            if run_len > 0 {
                driver.tally.runs.push(run_len);
                run_len = 0;
            }
        } else {
            driver.tally.f_serve += s;
        }
        if let Some(id) = step.wait {
            driver.wait_for(id, file);
            continue;
        }
        if step.needs_input() && end == file.len() {
            break;
        }
    }
    let wall = t.elapsed().as_secs_f64();
    if run_len > 0 {
        driver.tally.runs.push(run_len);
    }
    let Driver { pass, work, tally, .. } = driver;
    let taken = pass.taken();
    pass.finish().expect("an index");
    drop(work);
    for worker in workers {
        worker.join().expect("a worker");
    }
    report(&tally, delivered, taken, tasks, wall)
}

/// What `par` prints.
fn report(tally: &Tally, delivered: u64, taken: u64, tasks: usize, wall: f64) -> String {
    let busy = tally.spec_s + tally.decode_s;
    let idle = if tasks == 0 { 0.0 } else { 1.0 - busy / (tasks as f64 * wall) };
    let runs = tally.runs.len();
    let mean = if runs == 0 { 0.0 } else { taken as f64 / runs as f64 };
    let head = if tally.waits == 0 { 0.0 } else { 1e3 * tally.head / tally.waits as f64 };
    format!(
        "plaintext={delivered} specs={} decodes={} unwanted={} spec_cpu={:.3} decode_cpu={:.3} \
         idle={idle:.2} f_decode={:.3} f_serve={:.3} f_wait={:.3} waits={} head_ms={head:.2} \
         ran_here={} taken={taken} runs={runs} mean_run={mean:.2} longest={}",
        tally.specs,
        tally.decodes,
        tally.unwanted,
        tally.spec_s,
        tally.decode_s,
        tally.f_decode,
        tally.f_serve,
        tally.f_wait,
        tally.waits,
        tally.ran_here,
        tally.runs.iter().max().unwrap_or(&0),
    )
}
