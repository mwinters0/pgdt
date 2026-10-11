//! A caller's ask that a scan stop, polled and awaited.

use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::future::{Either, select};
use tokio::sync::Notify;

/// A caller's ask that a scan stop, in both the forms a reader consumes it
/// in: a bit to poll and a signal to await.
///
/// The polled bit is what the mapping loops read once per chunk or leader
/// window, and it is the whole mechanism for a reader whose wait is a
/// `pread` — tens of milliseconds, bounded by the read already in flight. A
/// reader whose wait is a network request has no such bound: its in-flight
/// call can sit inside a retry schedule for minutes, and polling at the far
/// side of it is a program that does not answer. So the same object carries a
/// signal such a reader can select on and **drop** the request rather than
/// wait it out (`docs/design/decisions.md`, "D26").
///
/// Both halves report the same one-way transition — once cancelled, always
/// cancelled — so a reader may use either or both, and which it uses is a
/// property of what it is waiting on rather than of what the caller asked for.
#[derive(Debug, Default)]
pub struct Cancellation {
    flag: AtomicBool,
    signal: Notify,
}

impl Cancellation {
    /// A cancellation nobody has asked for yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask every reader holding this to stop. Idempotent, and never blocks:
    /// it is called from a signal-handling task, and from a source's own read
    /// path in the tests.
    ///
    /// The `SeqCst` store is what [`Cancellation::cancelled`]'s registration
    /// is ordered against; the polled side needs nothing so strong.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.signal.notify_waiters();
    }

    /// Whether cancellation has been asked for. `Relaxed` is the right
    /// ordering: the bit guards nothing but itself — the reader's response is
    /// to finish the chunk it already holds and save the index it already
    /// owns — so nothing is published through it.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    /// Resolves once [`Cancellation::cancel`] has been called, at once if it
    /// already has.
    ///
    /// The waiter is registered *before* the bit is read, which is what makes
    /// this race-free against a `cancel` landing between the two: a cancel
    /// wholly before the registration is seen by the load, and one after it
    /// wakes a waiter that is already there.
    pub async fn cancelled(&self) {
        let mut signalled = pin!(self.signal.notified());
        signalled.as_mut().enable();
        if self.flag.load(Ordering::SeqCst) {
            return;
        }
        signalled.await;
    }

    /// Run `work` until it finishes or this is cancelled, whichever happens
    /// first. `None` is the cancellation, and `work` is **dropped where it
    /// stood** — which is how a reader gives up a request in flight instead
    /// of waiting for the answer it no longer wants.
    pub async fn until_cancelled<F: Future>(&self, work: F) -> Option<F::Output> {
        let work = pin!(work);
        let cancelled = pin!(self.cancelled());
        match select(work, cancelled).await {
            Either::Left((done, _)) => Some(done),
            Either::Right(((), _)) => None,
        }
    }
}

#[cfg(test)]
mod tests;
