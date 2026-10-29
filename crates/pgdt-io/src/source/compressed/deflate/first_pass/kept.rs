//! The frontier's own blocking thread: every job of a pulled pass run on one
//! `spawn_blocking` kept while the pass lives, so the frontier's decode stays
//! on one thread, and on the core whose caches hold its state, from one scan
//! read to the next (`docs/design/decisions.md`, "D126").
//!
//! **Why a `spawn_blocking` a read would not do**: the runtime's blocking
//! pool wakes one idle thread on a condvar, the one waiting longest in
//! `pgdt`'s build, whose tokio waits on `parking_lot`'s queue — so once the tasks run ahead of the frontier
//! have left threads idle, each read lands on the next of them in turn, each
//! on the core it last ran on; kept, the frontier stays on one thread at every
//! worker count, as a serial pass's does.
//!
//! **What ends the thread** is its channel closing: the pass retiring it at
//! hand-over, the pass dropped, or the thread's runtime shutting down. The
//! channel's one sender sits in a slot held strongly only by a parked task
//! spawned beside the thread on its runtime, and [`Kept`] sends through a
//! weak reference to it, so a runtime shutting down, which drops every task
//! before it waits for its blocking threads
//! (`docs/design/runtime-invariants.md`, "RT28"), closes the channel and the
//! thread leaves rather than holding the shutdown up.

use std::sync::{Arc, Mutex, Weak, mpsc};

use pgdt_core::{Error, Result};

/// One job, run where it is sent.
type Work = Box<dyn FnOnce() + Send>;

/// The kept thread's channel's one sender, taken out to close it.
type Slot = Mutex<Option<mpsc::Sender<Work>>>;

/// The kept thread's channel, as [`Kept`] reaches it: the slot the parked
/// task holds, and that task, for the pass to end it by.
struct Line {
    slot: Weak<Slot>,
    parked: tokio::task::AbortHandle,
}

impl Line {
    /// Queue `work` on the line's thread, or hand it back where the channel
    /// is closed. A send that succeeds is received but where the runtime
    /// shuts down before the thread has started, dropping its task and the
    /// channel with it: once running, the thread leaves only once the sender
    /// is gone and the channel is empty.
    fn send(&self, work: Work) -> std::result::Result<(), Work> {
        let Some(slot) = self.slot.upgrade() else { return Err(work) };
        let sender = lock(&slot);
        match sender.as_ref() {
            Some(sender) => sender.send(work).map_err(|mpsc::SendError(work)| work),
            None => Err(work),
        }
    }

    /// Close the channel, so the thread leaves once it has run what was sent,
    /// and end the parked task.
    fn close(self) {
        if let Some(slot) = self.slot.upgrade() {
            drop(lock(&slot).take());
        }
        self.parked.abort();
    }
}

/// The frontier's thread: started by the first job, kept until the pass
/// [retires](Kept::retire) it or is dropped, or its runtime shuts down, the
/// next job then starting another.
pub(super) struct Kept {
    line: Mutex<Option<Line>>,
}

impl Kept {
    pub(super) fn new() -> Kept {
        Kept { line: Mutex::new(None) }
    }

    /// `work`'s answer, run on the kept thread after every job sent before it;
    /// a panic in it is an [`Error::Join`], as one in a `spawn_blocking` is,
    /// and leaves the thread kept.
    pub(super) async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T> {
        let (answer, answered) = tokio::sync::oneshot::channel();
        self.send(Box::new(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
            let _ = answer.send(outcome);
        }));
        match answered.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => Err(Error::join(std::io::Error::other(
                "a job on a gzip first pass's frontier thread panicked",
            ))),
            Err(dropped) => Err(Error::join(dropped)),
        }
    }

    /// Give the thread back to the runtime once the jobs sent so far are run.
    pub(super) fn retire(&self) {
        if let Some(line) = lock(&self.line).take() {
            line.close();
        }
    }

    /// Queue `work` on the kept thread, starting one where none is kept: none
    /// started yet, retired, or ended with its runtime.
    fn send(&self, work: Work) {
        let mut line = lock(&self.line);
        let work = match line.as_ref() {
            Some(kept) => match kept.send(work) {
                Ok(()) => return,
                Err(work) => work,
            },
            None => work,
        };
        if let Some(ended) = line.take() {
            ended.close();
        }
        let (sender, receiver) = mpsc::channel();
        sender.send(work).expect("the receiver is held");
        let slot = Arc::new(Mutex::new(Some(sender)));
        let weak = Arc::downgrade(&slot);
        let parked = tokio::spawn(async move {
            let _slot = slot;
            std::future::pending::<()>().await;
        });
        drop(tokio::task::spawn_blocking(move || serve(&receiver)));
        *line = Some(Line { slot: weak, parked: parked.abort_handle() });
    }
}

impl Drop for Kept {
    fn drop(&mut self) {
        self.retire();
    }
}

/// Run each job as it comes until the channel closes and is empty.
fn serve(receiver: &mpsc::Receiver<Work>) {
    while let Ok(work) = receiver.recv() {
        work();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests;
