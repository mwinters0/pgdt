//! The worker pool under a bulk range read: one fetcher, N decoders, one queue.
//!
//! [`Pool`] is what [`RangeRead`](crate::RangeRead) runs on at two workers and
//! above; at one worker there is no pool at all and the calling thread does
//! everything, which is the shape `src/range.rs` describes. Nothing here is
//! public: the pool is an implementation of a delivery contract that does not
//! change with the worker count, and the only thing a caller sees of it is that
//! the bytes arrive sooner.
//!
//! See `docs/design/architecture.md`, "The pool: one fetcher, N decoders, and
//! exactly N slots".
//!
//! ```text
//!        fetch stage                 work queue            decoders
//!   ┌──────────────────┐        ┌────────────────┐      ┌──────────────┐
//!   │ source.read_at   │ window │ sync_channel(1)│ ───► │ decode_into  │
//!   │ ascending, one   │ ─────► │  shared, FIFO  │      │ into a slot  │
//!   │ block at a time  │        └────────────────┘      └──────┬───────┘
//!   └──────────────────┘                                       │ (seq, slot)
//!                          ┌────────────────────┐              ▼
//!            free slots ◄──┤  the delivery head  │◄──── results (unordered)
//!         (exactly N of ──►│  RangeRead::read    │
//!          them, reused)   └────────────────────┘
//! ```
//!
//! Four properties are the whole of it, and each is a decision recorded in the
//! architecture doc rather than an implementation detail:
//!
//! * **One fetcher, ascending.** Every source read the pool makes is for one
//!   block's own compressed extent, and the reads go in ascending file order.
//!   That order is a promise a caller's own prefetching source can be built
//!   against; a fetcher free to reorder would degrade such a source to a cache
//!   that misses.
//! * **One block is one unit of work**, drawn from a shared queue rather than
//!   cut per worker up front, because block cost is not uniform and a fixed cut
//!   leaves workers idle at the tail.
//! * **Exactly N decoded slots, reused.** They are handed out by a free list
//!   pre-filled with N buffers, so a worker whose slot is not yet drained cannot
//!   start another block. That is what makes the plan's footprint a number a
//!   caller can promise to a cgroup — and **a worker takes its slot before it
//!   takes work**, which is what keeps exactly-N from deadlocking against the
//!   reorder buffer. [`decode`] carries the argument.
//! * **The delivery head holds none of them while it waits.** It hands its
//!   spent slot back before asking for the next block, so all N circulate for
//!   the length of every wait and N workers overlap N decodes rather than
//!   N − 1. `Pool::slots_while_waiting` is that property as ordinary state,
//!   which is what puts it inside `cargo test` rather than behind a wall clock.
//! * **Drop cancels, and the join is bounded by a decoder call.** Workers poll
//!   the cancel flag inside the decode loop, not between blocks, so dropping a
//!   handle mid-block does not wait out a 128 MiB decode.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::error::{Error, Result};
use crate::source::CompressedSource;
use crate::task::BlockTask;
use crate::window::Window;

/// One block's compressed bytes, on their way from the fetcher to a decoder.
type Fetched = (usize, BlockTask, Window<Vec<u8>>);

/// One block's plaintext, or the fault that stopped it, on its way to the head.
type Decoded = (usize, Result<Vec<u8>>);

/// How deep the work queue is, beyond what the workers and the fetcher hold.
///
/// The plan charges **N + 2** compressed windows and this is where the two
/// come from: one sits in this queue while the fetcher fills the next, so the
/// stage always has somewhere to put block *k+1* while every worker is busy.
/// Coupling the windows to the slots instead would start every worker's next
/// block with a cold source read.
///
/// So this constant and `RangePlan`'s window term are one decision written
/// twice, and they move together: a deeper queue that the plan does not charge
/// for makes the footprint understate, which is the one direction a number
/// promised to a cgroup may not be wrong in.
const QUEUE_DEPTH: usize = 1;

/// The threads under one bulk range read, and the channels between them.
pub(crate) struct Pool {
    /// Decoded blocks and faults, in whatever order they finished.
    ///
    /// `Option` so that [`Pool::drop`] can hang it up before joining: a worker
    /// blocked on a send wakes as soon as the receiver is gone.
    results: Option<Receiver<Decoded>>,
    /// Where a drained slot goes back to. Dropping it releases every worker
    /// waiting for one.
    free: Option<Sender<Vec<u8>>>,
    /// Results that arrived ahead of the delivery head, keyed by block sequence.
    ///
    /// Bounded by the slot count, since a worker cannot start a block without a
    /// slot and every slot in here is one the head has not drained.
    ahead: BTreeMap<usize, Result<Vec<u8>>>,
    /// Set by [`Pool::drop`]. Polled by the workers inside the decode loop and
    /// by the fetcher between blocks.
    cancel: Arc<AtomicBool>,
    /// The fetcher first, then the decoders.
    threads: Vec<JoinHandle<()>>,
    /// How many of the N slots the delivery head is holding right now: those
    /// [`Pool::take`] has handed out and [`Pool::recycle`] has not had back.
    ///
    /// Never more than one, and zero while the head waits, because it hands its
    /// spent slot back before asking for the next block. It is a count rather
    /// than a flag so that the accounting is the pool's own — a flag the head
    /// set would say what the head believes rather than what it has returned.
    head_slots: usize,
    /// The fewest slots the pool had to work with at a moment the delivery head
    /// went to the decoders for a block. `usize::MAX` until it has gone once.
    ///
    /// **This is the whole difference between N workers overlapping N decodes
    /// and N − 1**, and it is ordinary state rather than a rate: a head that
    /// held its spent slot across the wait would leave the pool N − 1 to
    /// circulate for the length of every wait, which is most of the read. Read
    /// by `src/range.rs`'s own tests, which are the only thing that can see it.
    slots_while_waiting: usize,
    /// How many decoders were actually started, which is also how many slots
    /// the free list circulates.
    workers: usize,
}

impl Pool {
    /// Start a fetcher and up to `workers` decoders over `tasks`.
    ///
    /// `None` where the pool could not be built — a thread that would not
    /// start — in which case the caller runs the one-worker path, which needs
    /// no threads and delivers the same bytes. **A pool that cannot be built is
    /// a slower read and never an error**: there is no failure here a caller
    /// could act on that the serial path does not simply absorb.
    ///
    /// `slot_len` is the plan's `decoded_slot_bytes()`, and the free list is
    /// pre-filled with empty buffers that each worker grows once — so a range
    /// that ends after two blocks never allocates the other N − 2 slots.
    pub(crate) fn spawn<S>(
        source: S,
        tasks: Arc<[BlockTask]>,
        workers: usize,
        slot_len: usize,
    ) -> Option<Pool>
    where
        S: CompressedSource + Send + 'static,
    {
        let cancel = Arc::new(AtomicBool::new(false));
        let faulted = Arc::new(AtomicBool::new(false));
        let (work_tx, work_rx) = sync_channel::<Fetched>(QUEUE_DEPTH);
        let (results_tx, results_rx) = std::sync::mpsc::channel::<Decoded>();
        let (free_tx, free_rx) = std::sync::mpsc::channel::<Vec<u8>>();

        let fetcher = {
            let tasks = Arc::clone(&tasks);
            let cancel = Arc::clone(&cancel);
            let faulted = Arc::clone(&faulted);
            let results = results_tx.clone();
            std::thread::Builder::new()
                .name("xz-seek-fetch".to_owned())
                .spawn(move || fetch(&source, &tasks, &work_tx, &results, &cancel, &faulted))
                .ok()?
        };

        let work_rx = Arc::new(Mutex::new(work_rx));
        let free_rx = Arc::new(Mutex::new(free_rx));
        let mut threads = vec![fetcher];
        for i in 0..workers {
            let work_rx = Arc::clone(&work_rx);
            let free_rx = Arc::clone(&free_rx);
            let results = results_tx.clone();
            let cancel = Arc::clone(&cancel);
            let faulted = Arc::clone(&faulted);
            let started = std::thread::Builder::new()
                .name(format!("xz-seek-decode-{i}"))
                .spawn(move || decode(&work_rx, &free_rx, &results, &cancel, &faulted, slot_len));
            match started {
                Ok(handle) => threads.push(handle),
                // Whatever the machine would give is what runs. One decoder is
                // still a pool — a fetch stage overlapping a decode — and it is
                // strictly less memory than the plan reported.
                Err(_) => break,
            }
        }
        // The last sender in this scope: without dropping it, a worker's
        // `recv` on the results channel could never see the pool hang up.
        drop(results_tx);

        let started = threads.len() - 1;
        let mut pool = Pool {
            results: Some(results_rx),
            free: Some(free_tx),
            ahead: BTreeMap::new(),
            cancel,
            threads,
            head_slots: 0,
            slots_while_waiting: usize::MAX,
            workers: started,
        };
        if started == 0 {
            // The fetcher alone delivers nothing. Dropping the pool joins it.
            return None;
        }
        for _ in 0..started {
            pool.recycle_raw(Vec::new());
        }
        Some(pool)
    }

    /// How many decoders are running.
    ///
    /// Normally the plan's worker count; fewer where the machine declined to
    /// start a thread, which is a slower read and never a wrong one.
    pub(crate) fn workers(&self) -> usize {
        self.workers
    }

    /// The fewest slots this pool has had to work with while the delivery head
    /// waited on it.
    ///
    /// `workers()` where the head releases its spent slot before waiting, and
    /// one less where it does not — the difference between N overlapping
    /// decodes and N − 1, asserted rather than left to a wall clock. `None`
    /// before the head has waited at all, which is a range short enough that
    /// every block was already stashed.
    #[cfg(test)]
    pub(crate) fn slots_while_waiting(&self) -> Option<usize> {
        (self.slots_while_waiting != usize::MAX).then_some(self.slots_while_waiting)
    }

    /// The plaintext of block `seq`, blocking until it arrives.
    ///
    /// Results arrive in whatever order the decoders finish, so anything that
    /// is not the block at the delivery head is stashed and returned later.
    /// **Ordered delivery is here and nowhere else**: no worker knows or cares
    /// what the head is waiting for.
    pub(crate) fn take(&mut self, seq: usize, task: &BlockTask) -> Result<Vec<u8>> {
        if let Some(done) = self.ahead.remove(&seq) {
            return self.hand_to_head(done);
        }
        // The head is about to wait on the decoders, which is the one moment
        // the slot accounting is about: every slot the head is not holding is
        // one a worker can be decoding into for the length of this wait.
        self.slots_while_waiting = self
            .slots_while_waiting
            .min(self.workers.saturating_sub(self.head_slots));
        loop {
            // The borrow of the results channel ends with this statement, so
            // the stash below is a plain `&mut self` again.
            let received = match self.results.as_ref() {
                Some(results) => results.recv(),
                None => return Err(self.lost(task)),
            };
            match received {
                Ok((got, done)) if got == seq => return self.hand_to_head(done),
                Ok((got, done)) => {
                    self.ahead.insert(got, done);
                }
                // Every decoder is gone and the block never came. Nothing in a
                // correct pool reaches this: the fetcher issues work
                // ascending, so every sequence at or below a fault has been
                // dispatched and every dispatched block sends a result. It is
                // a panicked worker, and it is reported as the caller's source
                // failing rather than as the file being damaged, because the
                // file is not what went wrong.
                Err(_) => return Err(self.lost(task)),
            }
        }
    }

    /// Note that a block is on its way to the delivery head, which now holds
    /// the slot it arrived in.
    ///
    /// A fault hands the head nothing, so the count moves only on the `Ok` arm.
    fn hand_to_head(&mut self, done: Result<Vec<u8>>) -> Result<Vec<u8>> {
        self.head_slots += usize::from(done.is_ok());
        done
    }

    /// Hand a drained slot back, so a worker can start another block.
    ///
    /// The head calls this **before** asking for its next block, so the slot is
    /// back in circulation for the whole of the wait. Only a slot this pool
    /// handed out reaches here: the handle's own first buffer is not one of the
    /// N, and recycling it would grow the pool by one and make the plan's
    /// footprint understate — `RangeRead::take_from_pool` is where that is
    /// known, since the handle is what allocated it.
    pub(crate) fn recycle(&mut self, slot: Vec<u8>) {
        self.recycle_raw(slot);
        self.head_slots = self.head_slots.saturating_sub(1);
    }

    fn recycle_raw(&mut self, slot: Vec<u8>) {
        if let Some(free) = self.free.as_ref() {
            let _ = free.send(slot);
        }
    }

    /// The error a pool that stopped delivering is reported as.
    fn lost(&self, task: &BlockTask) -> Error {
        Error::Io {
            compressed_offset: task.compressed_range().start,
            source: std::io::Error::other(
                "xz-seek: the bulk read's decoders stopped before delivering this block",
            ),
        }
    }
}

/// Cancellation is `Drop`, and `Drop` is the only cancellation.
///
/// **It must not panic**, because a caller drops the handle from a blocking
/// thread inside an async runtime, where a panic is a lost cache rather than a
/// clean exit. So a worker that panicked is joined and its panic discarded: it
/// is a bug in this crate, and unwinding into the dropper turns one bug into
/// two.
///
/// The order matters. Setting the flag alone wakes nobody — a thread parked in
/// `recv` is not polling anything — so both channel ends the handle owns are
/// hung up first: the free list, which releases a worker waiting for a slot,
/// and the results channel, which fails a worker's send. The fetcher notices
/// the flag between blocks and drops the work queue behind it, which releases
/// the rest.
impl Drop for Pool {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.free = None;
        self.results = None;
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// The fetch stage: one block's compressed extent at a time, ascending.
///
/// It is the only thing in the pool that touches the caller's source, which is
/// what makes the ascending promise checkable and what holds a remote source to
/// one round trip per block rather than to one per worker.
fn fetch<S: CompressedSource>(
    source: &S,
    tasks: &[BlockTask],
    work: &SyncSender<Fetched>,
    results: &Sender<Decoded>,
    cancel: &AtomicBool,
    faulted: &AtomicBool,
) {
    // The source's *own* size, not the table's. A window is told the file size
    // so that a read running off the end of the file short-returns where the
    // file would — which is how a truncated file reaches `Truncated` here
    // exactly as it does on the seeking path, instead of arriving as a window
    // that refuses a read it should have answered short.
    let file_size = match source.size() {
        Ok(size) => size,
        Err(e) => {
            faulted.store(true, Ordering::Relaxed);
            let _ = results.send((
                0,
                Err(Error::Io {
                    compressed_offset: 0,
                    source: e,
                }),
            ));
            return;
        }
    };

    for (seq, task) in tasks.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) || faulted.load(Ordering::Relaxed) {
            return;
        }
        let extent = task.compressed_range();
        let Ok(len) = usize::try_from(extent.end - extent.start) else {
            faulted.store(true, Ordering::Relaxed);
            let _ = results.send((
                seq,
                Err(Error::Io {
                    compressed_offset: extent.start,
                    source: std::io::Error::other(
                        "xz-seek: this block's compressed extent does not fit in memory",
                    ),
                }),
            ));
            return;
        };
        let mut bytes = vec![0u8; len];
        let mut got = 0usize;
        while got < len {
            match source.read_at(extent.start + got as u64, &mut bytes[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) => {
                    faulted.store(true, Ordering::Relaxed);
                    let _ = results.send((
                        seq,
                        Err(Error::Io {
                            compressed_offset: extent.start + got as u64,
                            source: e,
                        }),
                    ));
                    return;
                }
            }
        }
        // A short fetch is the file ending early, and it is left to the decode
        // to say so: the window short-returns exactly where the file does, and
        // `Truncated` is raised with the offset the seeking path would name.
        bytes.truncate(got);
        if work
            .send((seq, *task, Window::new(extent.start, file_size, bytes)))
            .is_err()
        {
            return;
        }
    }
}

/// One decoder: take a slot, take work, fill the slot, hand it on.
///
/// **The slot is taken first, and the order is what keeps the pool from
/// deadlocking.** The delivery head holds one slot while it drains it and
/// stashes every result that arrived early, so the slots the head is holding
/// are exactly the ones it cannot release until the block it is waiting for
/// arrives. Taking work first admits the state where a worker has dequeued that
/// very block and every remaining slot is already in the head's stash: the
/// worker waits for a slot the head will not release, and the head waits for
/// the block that worker holds. Taking the slot first makes *"dequeued"* imply
/// *"holds a slot"*, and the queue is FIFO over an ascending sequence, so the
/// block at the head is either already being decoded by a worker that can
/// finish or is still in a queue that an idle worker — slot in hand — will draw
/// from.
///
/// **It never reads `faulted`.** By the same FIFO argument a block already
/// taken is a block the delivery head may still be waiting for; abandoning it
/// on someone else's fault is the same deadlock by another route. What the
/// fault does is stop the *fetcher*, which is what "fetching and decoding past
/// the fault stop as soon as it is found" costs to do safely.
fn decode(
    work: &Mutex<Receiver<Fetched>>,
    free: &Mutex<Receiver<Vec<u8>>>,
    results: &Sender<Decoded>,
    cancel: &AtomicBool,
    faulted: &AtomicBool,
    slot_len: usize,
) {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(mut slot) = recv(free) else {
            return;
        };
        let Ok((seq, task, window)) = recv(work) else {
            return;
        };
        if slot.len() < slot_len {
            // `vec![0u8; n]` reaches `alloc_zeroed`, where growing an empty
            // `Vec` writes the zeros a byte at a time in an unoptimized build.
            slot = vec![0u8; slot_len];
        }
        match task.decode_into_until(&window, &mut slot, Some(cancel)) {
            Ok(true) => {
                if results.send((seq, Ok(slot))).is_err() {
                    return;
                }
            }
            // Cancelled between decoder calls. Nothing is owed off a dropped
            // handle, so the slot and the window go with the thread.
            Ok(false) => return,
            Err(e) => {
                faulted.store(true, Ordering::Relaxed);
                let _ = results.send((seq, Err(e)));
                return;
            }
        }
    }
}

/// Receive from a queue shared by every worker.
///
/// A poisoned lock is taken anyway: the mutex guards a channel endpoint, which
/// a panic while parked in `recv` cannot leave half-modified, and refusing to
/// take it would turn one worker's bug into a hung pool.
fn recv<T>(queue: &Mutex<Receiver<T>>) -> std::result::Result<T, ()> {
    let guard = match queue.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.recv().map_err(|_| ())
}
