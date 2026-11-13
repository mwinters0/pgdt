use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use super::*;

/// A waker counting its wakes, so a test says which waiter a change woke.
#[derive(Default)]
struct Count(AtomicUsize);

impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// A waiter's waker, and how many times it has been woken.
struct Woken {
    count: Arc<Count>,
    waker: Waker,
}

impl Woken {
    fn new() -> Self {
        let count = Arc::new(Count::default());
        Self { waker: Waker::from(Arc::clone(&count)), count }
    }

    fn times(&self) -> usize {
        self.count.0.load(Ordering::SeqCst)
    }

    fn poll<F: Future>(&self, future: Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(&self.waker))
    }
}

/// `holder`'s draw of `charge` for the line at `start`, met without waiting.
fn drawn_now(holder: &mut Holder, start: u64, charge: u64) -> Admission {
    let woken = Woken::new();
    match woken.poll(pin!(holder.draw(start, charge))) {
        Poll::Ready(admission) => admission,
        Poll::Pending => panic!("the draw for the line at {start} waited"),
    }
}

/// **A ledger without limit meets every draw at once** and keeps no account.
#[test]
fn an_unlimited_ledger_meets_every_draw() {
    let ledger = Ledger::unlimited();
    let (mut a, mut b) = (ledger.holder(), ledger.holder());
    assert_eq!(drawn_now(&mut a, 0, u64::MAX), Admission::Held);
    assert_eq!(drawn_now(&mut b, 10, u64::MAX), Admission::Held);
    assert_eq!((ledger.capacity(), ledger.drawn(), a.held()), (None, 0, u64::MAX));
}

/// **A draw is a line's whole charge**: growth draws the difference, a draw
/// no larger than the line holds is met at once, and a release, a draw for
/// another line or a dropped holder returns it.
#[test]
fn a_draw_is_the_line_s_whole_charge_and_returned_on_release() {
    let ledger = Ledger::limited(100);
    let mut a = ledger.holder();
    assert_eq!(drawn_now(&mut a, 0, 40), Admission::Held);
    assert_eq!(drawn_now(&mut a, 0, 60), Admission::Held);
    assert_eq!(drawn_now(&mut a, 0, 50), Admission::Held);
    assert_eq!((ledger.drawn(), a.held()), (60, 60));
    a.release();
    assert_eq!((ledger.drawn(), a.held()), (0, 0));

    assert_eq!(drawn_now(&mut a, 10, 30), Admission::Held);
    assert_eq!(drawn_now(&mut a, 20, 70), Admission::Held);
    assert_eq!(ledger.drawn(), 70);
    drop(a);
    assert_eq!(ledger.drawn(), 0);
}

/// **A draw past what is left waits**, and is woken and admitted once a line
/// before it returns its draw.
#[test]
fn a_draw_past_what_is_left_waits_until_a_line_returns_its_draw() {
    let ledger = Ledger::limited(100);
    let (mut a, mut b) = (ledger.holder(), ledger.holder());
    assert_eq!(drawn_now(&mut a, 0, 70), Admission::Held);

    let woken = Woken::new();
    let mut draw = pin!(b.draw(100, 50));
    assert_eq!(woken.poll(draw.as_mut()), Poll::Pending);
    assert_eq!(ledger.drawn(), 70);
    a.release();
    assert_eq!(woken.times(), 1);
    assert_eq!(woken.poll(draw), Poll::Ready(Admission::Held));
    assert_eq!(ledger.drawn(), 50);
}

/// **The waiting line earliest in the file is admitted first**: a later
/// waiter that would fit stays waiting while an earlier one waits, and is
/// admitted once that one is.
#[test]
fn the_earliest_waiting_line_is_admitted_first() {
    let ledger = Ledger::limited(100);
    let (mut first, mut late, mut early) = (ledger.holder(), ledger.holder(), ledger.holder());
    assert_eq!(drawn_now(&mut first, 0, 90), Admission::Held);

    let (late_woken, early_woken) = (Woken::new(), Woken::new());
    let mut late_draw = pin!(late.draw(300, 20));
    let mut early_draw = pin!(early.draw(100, 20));
    assert_eq!(late_woken.poll(late_draw.as_mut()), Poll::Pending);
    assert_eq!(early_woken.poll(early_draw.as_mut()), Poll::Pending);

    first.release();
    assert_eq!((late_woken.times(), early_woken.times()), (1, 1));
    // Room for both, and the later one asks first: it waits on the earlier.
    assert_eq!(late_woken.poll(late_draw.as_mut()), Poll::Pending);
    assert_eq!(early_woken.poll(early_draw), Poll::Ready(Admission::Held));
    assert_eq!(late_woken.times(), 2, "the earlier line's admission wakes the next");
    assert_eq!(late_woken.poll(late_draw), Poll::Ready(Admission::Held));
    assert_eq!(ledger.drawn(), 40);
}

/// **A line earlier than every waiter is admitted past them** where it
/// fits: waiting holds back only the lines after the one waiting.
#[test]
fn a_line_before_every_waiter_is_admitted_where_it_fits() {
    let ledger = Ledger::limited(100);
    let (mut first, mut late, mut early) = (ledger.holder(), ledger.holder(), ledger.holder());
    assert_eq!(drawn_now(&mut first, 0, 80), Admission::Held);
    let woken = Woken::new();
    let mut late_draw = pin!(late.draw(300, 30));
    assert_eq!(woken.poll(late_draw.as_mut()), Poll::Pending);
    assert_eq!(drawn_now(&mut early, 100, 15), Admission::Held);
    assert_eq!(ledger.drawn(), 95);
}

/// **When the earliest waiting line cannot be admitted, later holders give
/// theirs up, the latest first, until it can be** — each when it next
/// draws, whatever it asks for — and no earlier holder, nor any later one
/// past what it needs, is asked. One that gave up draws again behind it.
#[test]
fn later_holders_give_up_latest_first_until_the_earliest_fits() {
    let ledger = Ledger::limited(100);
    let mut holders: Vec<Holder> = (0..4).map(|_| ledger.holder()).collect();
    let [h0, h200, h300, h400] = &mut holders[..] else { unreachable!() };
    assert_eq!(drawn_now(h0, 0, 10), Admission::Held);
    for (holder, start) in [(&mut *h200, 200), (&mut *h300, 300), (&mut *h400, 400)] {
        assert_eq!(drawn_now(holder, start, 30), Admission::Held);
    }

    // 100 drawn, 50 wanted: the two latest lines' 60 covers it.
    let mut waiter = ledger.holder();
    let woken = Woken::new();
    let mut draw = pin!(waiter.draw(100, 50));
    assert_eq!(woken.poll(draw.as_mut()), Poll::Pending);

    assert_eq!(drawn_now(h0, 0, 10), Admission::Held, "an earlier line is never asked");
    assert_eq!(drawn_now(h200, 200, 30), Admission::Held, "nor one it does not need");
    assert_eq!(drawn_now(h400, 400, 30), Admission::GiveUp);
    assert_eq!((h400.held(), ledger.drawn()), (0, 70));
    assert_eq!(woken.times(), 1);
    assert_eq!(woken.poll(draw.as_mut()), Poll::Pending, "still 20 short");

    assert_eq!(drawn_now(h300, 300, 40), Admission::GiveUp);
    assert_eq!(woken.times(), 2);
    assert_eq!(woken.poll(draw), Poll::Ready(Admission::Held));
    assert_eq!(ledger.drawn(), 90);

    // Re-read from its start, the line that gave up waits behind it.
    let again = Woken::new();
    assert_eq!(again.poll(pin!(h400.draw(400, 30))), Poll::Pending);
}

/// **A later holder waiting when it is asked is woken to give up**, and a
/// waiter asked keeps what it holds until then.
#[test]
fn a_waiting_later_holder_is_woken_to_give_up() {
    let ledger = Ledger::limited(100);
    let (mut first, mut late, mut early) = (ledger.holder(), ledger.holder(), ledger.holder());
    assert_eq!(drawn_now(&mut first, 0, 50), Admission::Held);
    assert_eq!(drawn_now(&mut late, 300, 40), Admission::Held);

    let late_woken = Woken::new();
    let mut late_draw = pin!(late.draw(300, 60));
    assert_eq!(late_woken.poll(late_draw.as_mut()), Poll::Pending);
    assert_eq!(ledger.drawn(), 90, "a waiter keeps what it holds");

    let early_woken = Woken::new();
    let mut early_draw = pin!(early.draw(100, 40));
    assert_eq!(early_woken.poll(early_draw.as_mut()), Poll::Pending);
    assert_eq!(late_woken.times(), 1);
    assert_eq!(late_woken.poll(late_draw), Poll::Ready(Admission::GiveUp));
    assert_eq!(early_woken.poll(early_draw), Poll::Ready(Admission::Held));
    assert_eq!(ledger.drawn(), 90);
}

/// **A waiter dropped stops waiting and keeps what it held**, and a later
/// line waiting behind it is woken.
#[test]
fn a_dropped_waiter_stops_waiting_and_keeps_what_it_held() {
    let ledger = Ledger::limited(100);
    let (mut first, mut early, mut late) = (ledger.holder(), ledger.holder(), ledger.holder());
    assert_eq!(drawn_now(&mut first, 0, 60), Admission::Held);
    assert_eq!(drawn_now(&mut early, 100, 20), Admission::Held);

    let late_woken = Woken::new();
    let mut late_draw = pin!(late.draw(200, 10));
    {
        let early_woken = Woken::new();
        let mut early_draw = pin!(early.draw(100, 50));
        assert_eq!(early_woken.poll(early_draw.as_mut()), Poll::Pending);
        assert_eq!(late_woken.poll(late_draw.as_mut()), Poll::Pending, "behind a waiter");
    }
    assert_eq!(late_woken.times(), 1, "the earlier waiter's drop wakes it");
    assert_eq!((early.held(), ledger.drawn()), (20, 80));
    assert_eq!(late_woken.poll(late_draw), Poll::Ready(Admission::Held));
    assert_eq!(ledger.drawn(), 90);
}

/// **The earliest line is admitted past the capacity where nothing else is
/// drawn**, so a ledger never stops a line it cannot hold; anything else
/// drawn is asked back first.
#[test]
fn the_earliest_line_is_admitted_past_the_capacity_alone() {
    let ledger = Ledger::limited(10);
    let mut alone = ledger.holder();
    assert_eq!(drawn_now(&mut alone, 0, 50), Admission::Held);
    alone.release();

    let mut later = ledger.holder();
    assert_eq!(drawn_now(&mut later, 100, 5), Admission::Held);
    let woken = Woken::new();
    let mut draw = pin!(alone.draw(0, 50));
    assert_eq!(woken.poll(draw.as_mut()), Poll::Pending);
    assert_eq!(drawn_now(&mut later, 100, 5), Admission::GiveUp);
    assert_eq!(woken.poll(draw), Poll::Ready(Admission::Held));
    assert_eq!(ledger.drawn(), 50);
}

/// **Readers on threads of their own all finish**: each grows a line to past
/// what the ledger holds beside the others, re-reading from zero where it
/// gave up, and nothing is left drawn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_readers_all_finish() {
    let ledger = Ledger::limited(100);
    let readers = (0..8u64).map(|reader| {
        let mut holder = ledger.holder();
        tokio::spawn(async move {
            for line in 0..20u64 {
                let start = (line * 8 + reader) * 1000;
                let mut charge = 10;
                while charge <= 60 {
                    match holder.draw(start, charge).await {
                        Admission::Held => charge += 10,
                        Admission::GiveUp => charge = 10,
                    }
                    tokio::task::yield_now().await;
                }
                holder.release();
            }
        })
    });
    let all = futures::future::try_join_all(readers);
    tokio::time::timeout(std::time::Duration::from_secs(30), all)
        .await
        .expect("every reader finishes")
        .unwrap();
    assert_eq!(ledger.drawn(), 0);
}
