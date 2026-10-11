use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

/// A guard that records its own drop, so a test can assert that a future
/// given up on was released rather than merely left unpolled.
struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// The signal reports a cancellation that landed **before** anyone waited
/// on it: a reader that reaches its await after the ask must not wait for
/// a second one that will never come.
#[tokio::test]
async fn the_signal_resolves_at_once_for_a_cancellation_already_asked_for() {
    let cancel = Cancellation::new();
    assert!(!cancel.is_cancelled());
    cancel.cancel();
    assert!(cancel.is_cancelled());
    cancel.cancelled().await;
}

/// And it reports one that lands **while** a waiter is registered, which
/// is the ordinary case: the waiter is parked before `cancel` is called.
#[tokio::test]
async fn a_registered_waiter_is_woken_by_a_later_cancel() {
    let cancel = Cancellation::new();
    tokio::join!(cancel.cancelled(), async {
        assert!(!cancel.is_cancelled(), "the waiter parks before the ask");
        cancel.cancel();
    });
}

/// `cancel` is idempotent, and the polled bit is one-way.
#[tokio::test]
async fn cancelling_twice_says_the_same_thing() {
    let cancel = Cancellation::new();
    cancel.cancel();
    cancel.cancel();
    assert!(cancel.is_cancelled());
    cancel.cancelled().await;
}

/// Work that finishes before anyone cancels is handed back whole.
#[tokio::test]
async fn until_cancelled_yields_work_that_finished_first() {
    let cancel = Cancellation::new();
    assert_eq!(cancel.until_cancelled(async { 7u8 }).await, Some(7));
    assert!(!cancel.is_cancelled(), "asking nothing cancels nothing");
}

/// **The point of the awaitable form.** Work that never finishes is given
/// up on at the cancellation *and dropped there* — which is what lets a
/// reader release a request in flight instead of waiting out whatever
/// deadline sits under it.
#[tokio::test]
async fn until_cancelled_drops_the_work_it_gave_up_on() {
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = DropFlag(Arc::clone(&dropped));
    let forever = async move {
        let _guard = guard;
        std::future::pending::<u8>().await
    };

    let cancel = Cancellation::new();
    let (given_up, ()) = tokio::join!(cancel.until_cancelled(forever), async {
        assert!(!dropped.load(Ordering::SeqCst), "still in flight before the ask");
        cancel.cancel();
    });

    assert_eq!(given_up, None, "cancellation, not an answer");
    assert!(dropped.load(Ordering::SeqCst), "the request in flight was released");
}
