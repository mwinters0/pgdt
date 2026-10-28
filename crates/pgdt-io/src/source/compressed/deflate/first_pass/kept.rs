//! The frontier's own blocking thread: every job of a pulled pass run on one
//! `spawn_blocking` kept while its reads keep coming, so the frontier's decode
//! stays on one thread, and on the core whose caches hold its state, from one
//! scan read to the next (`docs/design/decisions.md`, "D126").
//!
//! **Why a `spawn_blocking` a read would not do**: the runtime's blocking
//! pool wakes one idle thread on a condvar, and the platform picks which — on
//! Linux the one waiting longest — so once the tasks run ahead of the frontier
//! have left threads idle, each read lands on the next of them in turn, each
//! on the core it last ran on; kept, the frontier stays on one thread at every
//! worker count, as a serial pass's does.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use pgdt_core::{Error, Result};

/// How long the kept thread waits for the pass's next job before giving its
/// thread back to the runtime: far longer than a scan takes between reads,
/// and short enough that a runtime shutting down, which waits for every
/// blocking task, does not wait on a pass nobody reads.
const IDLE: Duration = Duration::from_millis(100);

/// One job, run where it is sent.
type Work = Box<dyn FnOnce() + Send>;

/// Where a job for the kept thread is sent: none while no thread is kept.
type Queue = Mutex<Option<mpsc::Sender<Work>>>;

/// The frontier's thread: started by the first job, kept until the pass
/// [retires](Kept::retire) it, is dropped, or sends nothing for [`IDLE`], the
/// next job then starting another.
pub(super) struct Kept {
    queue: Arc<Queue>,
}

impl Kept {
    pub(super) fn new() -> Kept {
        Kept { queue: Arc::new(Mutex::new(None)) }
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
        drop(lock(&self.queue).take());
    }

    /// Queue `work` on the kept thread, starting one where none is kept.
    fn send(&self, work: Work) {
        let mut queue = lock(&self.queue);
        // A thread leaving for want of work clears the queue under this lock
        // first, so a send that succeeds is always received.
        let work = match queue.as_ref() {
            Some(sender) => match sender.send(work) {
                Ok(()) => return,
                Err(mpsc::SendError(work)) => work,
            },
            None => work,
        };
        let (sender, receiver) = mpsc::channel();
        sender.send(work).expect("the receiver is held");
        let queue_held = Arc::downgrade(&self.queue);
        drop(tokio::task::spawn_blocking(move || serve(&receiver, &queue_held)));
        *queue = Some(sender);
    }
}

/// Run each job as it comes until the pass stops sending: its queue retired
/// or dropped, or nothing sent for [`IDLE`].
fn serve(receiver: &mpsc::Receiver<Work>, queue: &Weak<Queue>) {
    loop {
        match receiver.recv_timeout(IDLE) {
            Ok(work) => work(),
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {
                let Some(queue) = queue.upgrade() else { return };
                let mut queue = lock(&queue);
                // Anything sent before the lock was taken is run first.
                if let Ok(work) = receiver.try_recv() {
                    drop(queue);
                    work();
                    continue;
                }
                *queue = None;
                return;
            }
        }
    }
}

fn lock(queue: &Queue) -> std::sync::MutexGuard<'_, Option<mpsc::Sender<Work>>> {
    queue.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests;
