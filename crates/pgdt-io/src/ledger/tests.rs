use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use super::*;
use crate::parallelism::{Parallelism, WorkerMemory};

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

/// A ledger of `capacity` bytes whose floor refuses no line.
fn limited(capacity: u64) -> Ledger {
    Ledger::limited(capacity, Floor { holds: u64::MAX, allowance: u64::MAX }, Terms::default())
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
    ledger.draw_statistics(u64::MAX);
    assert_eq!((ledger.statistics(), ledger.drawn()), (0, 0));
}

/// **A draw is a line's whole charge**: growth draws the difference, a draw
/// no larger than the line holds is met at once, and a release, a draw for
/// another line or a dropped holder returns it.
#[test]
fn a_draw_is_the_line_s_whole_charge_and_returned_on_release() {
    let ledger = limited(100);
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
    let ledger = limited(100);
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
    let ledger = limited(100);
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
    let ledger = limited(100);
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
    let ledger = limited(100);
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
    let ledger = limited(100);
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
    let ledger = limited(100);
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
    let ledger = limited(10);
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

/// **A charge past the floor is refused at once, wherever its line lies**:
/// the earliest line alone, or a later one behind a waiter, never waits for
/// room no line can give up. What the line held is returned, and a line
/// waiting behind it is woken; a charge at the floor is admitted alone.
#[test]
fn a_charge_past_the_floor_is_refused_and_returns_what_it_held() {
    let floor = Floor { holds: 40, allowance: 1000 };
    let ledger = Ledger::limited(10, floor, Terms::default());
    assert_eq!(ledger.floor(), Some(floor));
    let mut alone = ledger.holder();
    assert_eq!(drawn_now(&mut alone, 0, 40), Admission::Held, "at the floor, admitted alone");
    assert_eq!(drawn_now(&mut alone, 0, 41), Admission::Refused(floor));
    assert_eq!((ledger.drawn(), alone.held()), (0, 0), "what it held is returned");

    let (mut first, mut later) = (ledger.holder(), ledger.holder());
    assert_eq!(drawn_now(&mut first, 0, 30), Admission::Held);
    let woken = Woken::new();
    let mut waiting = pin!(later.draw(100, 20));
    assert_eq!(woken.poll(waiting.as_mut()), Poll::Pending);
    let mut past = ledger.holder();
    assert_eq!(drawn_now(&mut past, 200, 50), Admission::Refused(floor), "a later line too");
    assert_eq!(drawn_now(&mut first, 0, 41), Admission::Refused(floor));
    assert_eq!(woken.times(), 1, "the line behind it is woken");
    assert_eq!(woken.poll(waiting), Poll::Ready(Admission::Held));
    assert_eq!(ledger.drawn(), 20);
    assert_eq!(Ledger::unlimited().floor(), None);
}

/// **The statistics' draw is admitted at once, past the capacity too**, and
/// lines are admitted from what it leaves: a line it leaves no room for
/// waits, and is woken and admitted once the draw falls.
#[test]
fn statistics_draw_first_and_lines_from_what_they_leave() {
    let ledger = limited(100);
    ledger.draw_statistics(60);
    let (mut a, mut b) = (ledger.holder(), ledger.holder());
    assert_eq!(drawn_now(&mut a, 0, 30), Admission::Held);

    let woken = Woken::new();
    let mut draw = pin!(b.draw(100, 20));
    assert_eq!(woken.poll(draw.as_mut()), Poll::Pending);
    ledger.draw_statistics(120);
    assert_eq!((ledger.statistics(), ledger.drawn()), (120, 150), "never declined");
    assert_eq!(woken.times(), 0, "a rise wakes no waiter");
    ledger.draw_statistics(40);
    assert_eq!(woken.times(), 1);
    assert_eq!(woken.poll(draw), Poll::Ready(Admission::Held));
    assert_eq!(ledger.drawn(), 90);
}

/// **A statistics' draw passing the capacity asks later holders to give
/// theirs up, the latest first**, until what passes it is asked for — and
/// never the earliest line, which would be admitted again alone.
#[test]
fn statistics_passing_the_capacity_ask_the_latest_holders_but_never_the_earliest() {
    let ledger = limited(100);
    let mut holders: Vec<Holder> = (0..3).map(|_| ledger.holder()).collect();
    let [h0, h100, h200] = &mut holders[..] else { unreachable!() };
    for (holder, start) in [(&mut *h0, 0), (&mut *h100, 100), (&mut *h200, 200)] {
        assert_eq!(drawn_now(holder, start, 30), Admission::Held);
    }

    // 90 drawn, 30 more: the latest line's 30 covers what passes.
    ledger.draw_statistics(30);
    assert_eq!(drawn_now(h100, 100, 30), Admission::Held, "nor one it does not need");
    assert_eq!(drawn_now(h200, 200, 30), Admission::GiveUp);
    assert_eq!(ledger.drawn(), 90);

    // Statistics past the capacity alone: every later line, never the first.
    ledger.draw_statistics(200);
    assert_eq!(drawn_now(h0, 0, 30), Admission::Held, "the earliest line is never asked");
    assert_eq!(drawn_now(h100, 100, 30), Admission::GiveUp);
    assert_eq!(drawn_now(h0, 0, 40), Admission::Held, "and grows alone past the capacity");
    assert_eq!(ledger.drawn(), 240);
}

/// **The earliest line is admitted past the capacity where no other line
/// holds, whatever the statistics hold**: they give nothing up, so waiting
/// on them would never end.
#[test]
fn the_earliest_line_never_waits_on_statistics() {
    let ledger = limited(100);
    ledger.draw_statistics(150);
    let (mut first, mut second) = (ledger.holder(), ledger.holder());
    assert_eq!(drawn_now(&mut first, 0, 50), Admission::Held);

    let woken = Woken::new();
    let mut draw = pin!(second.draw(100, 10));
    assert_eq!(woken.poll(draw.as_mut()), Poll::Pending, "a second line waits on the first");
    first.release();
    assert_eq!(woken.times(), 1);
    assert_eq!(woken.poll(draw), Poll::Ready(Admission::Held));
    assert_eq!(ledger.drawn(), 160);
}

/// **Readers on threads of their own all finish**: each grows a line to past
/// what the ledger holds beside the others and the statistics, re-reading
/// from zero where it gave up, and nothing is left drawn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_readers_all_finish() {
    let ledger = limited(100);
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
                        Admission::Refused(floor) => panic!("refused under {floor:?}"),
                    }
                    tokio::task::yield_now().await;
                }
                holder.release();
            }
        })
    });
    // The statistics' draw rising and falling under them, as a pass's
    // account does, never past the capacity alone.
    let statistics = ledger.clone();
    let gathering = tokio::spawn(async move {
        for step in 0..200u64 {
            statistics.draw_statistics((step * 7) % 90);
            tokio::task::yield_now().await;
        }
        statistics.draw_statistics(0);
    });
    let all = futures::future::try_join_all(readers);
    tokio::time::timeout(std::time::Duration::from_secs(30), all)
        .await
        .expect("every reader finishes")
        .unwrap();
    gathering.await.unwrap();
    assert_eq!(ledger.drawn(), 0);
}

/// A ledger of `capacity` bytes refusing no line, its three readers each
/// giving back 100 bytes parked: `[0, 100, 200]` by how many park.
fn parking(capacity: u64) -> Ledger {
    let terms = Terms::of(Parallelism::workers(3, 300), Some(WorkerMemory::per_worker(100)));
    Ledger::limited(capacity, Floor { holds: u64::MAX, allowance: u64::MAX }, terms)
}

/// **What parking gives back is the carve's fall**: each reader's term
/// where the budget holds every reader, nothing where the source stated no
/// cost, and nothing for the readers a stated count holds past what its
/// budget affords until the count parked falls under it (`KD21`).
#[test]
fn parking_gives_back_the_budget_s_fall() {
    let memory = Some(WorkerMemory::per_worker(100));
    assert_eq!(Terms::of(Parallelism::workers(3, 300), memory), Terms(vec![0, 100, 200]));
    assert_eq!(Terms::of(Parallelism::workers(3, 300), None), Terms::default());
    assert_eq!(Terms::of(Parallelism::workers(4, 200), memory), Terms(vec![0, 0, 0, 100]));
    let terms = Terms::of(Parallelism::workers(3, 300), memory);
    assert_eq!((terms.parks_for(0), terms.parks_for(1), terms.parks_for(100)), (0, 1, 1));
    assert_eq!((terms.parks_for(101), terms.parks_for(500)), (2, 2));
    assert_eq!(Terms::default().parks_for(500), 0);
}

/// **A line alone past the room parks the latest reader first**, and is
/// admitted once what it gives back covers the line; a reader reading
/// before the line is never asked; the parked reader resumes as the line's
/// draw is returned, announcing its park and its resume to its source.
#[test]
fn a_line_alone_past_the_room_parks_the_latest_reader_until_it_fits() {
    let ledger = parking(10);
    let source = crate::test_support::ParkCounting::default();
    let (mut line, mut before, mut early, mut late) =
        (ledger.holder(), ledger.holder(), ledger.holder(), ledger.holder());
    assert!(!before.stand(50) && !early.stand(200) && !late.stand(300));

    let (woken, parked_woken) = (Woken::new(), Woken::new());
    {
        let mut draw = pin!(line.draw(100, 60));
        assert_eq!(woken.poll(draw.as_mut()), Poll::Pending, "waits on the park");
        assert_eq!((before.stand(60), early.stand(210), late.stand(310)), (false, false, true));
        let mut parked = pin!(late.park(&source));
        assert_eq!(parked_woken.poll(parked.as_mut()), Poll::Pending);
        assert_eq!((ledger.parked(), source.parked()), (1, 1));
        assert_eq!(woken.times(), 1, "the park wakes the line");
        assert_eq!(woken.poll(draw), Poll::Ready(Admission::Held));
        assert_eq!(ledger.drawn(), 60);
    }
    assert_eq!((ledger.parked(), source.parked()), (0, 0), "dropped, it resumes");
    assert!(late.stand(320), "still asked while the line holds");
    {
        let mut parked = pin!(late.park(&source));
        assert_eq!(parked_woken.poll(parked.as_mut()), Poll::Pending);
        line.release();
        assert_eq!(parked_woken.times(), 1, "the release resumes it");
        assert_eq!(parked_woken.poll(parked), Poll::Ready(()));
    }
    assert_eq!((ledger.parked(), source.parked()), (0, 0));
    assert!(!late.stand(330));
}

/// **A line alone is admitted once no reader is left reading to ask**:
/// where every reader's parking falls short of it, each later reader is
/// asked, and the line is admitted once each has parked or left — never
/// asking a reader whose own line waits, which reads nothing.
#[test]
fn a_line_alone_is_admitted_once_none_is_left_reading_to_ask() {
    let ledger = parking(10);
    let source = crate::test_support::ParkCounting::default();
    let (woken, parked_woken) = (Woken::new(), Woken::new());
    let (mut line, mut leaves, mut parks) = (ledger.holder(), ledger.holder(), ledger.holder());
    assert!(!leaves.stand(200) && !parks.stand(300));
    {
        let mut draw = pin!(line.draw(100, 500));
        assert_eq!(woken.poll(draw.as_mut()), Poll::Pending);
        assert!(leaves.stand(210) && parks.stand(310), "each asked, the line passing them both");
        let mut parked = pin!(parks.park(&source));
        assert_eq!(parked_woken.poll(parked.as_mut()), Poll::Pending);
        assert_eq!(woken.poll(draw.as_mut()), Poll::Pending, "one reader still reads");
        drop(leaves);
        assert_eq!(woken.poll(draw), Poll::Ready(Admission::Held));
        assert_eq!(ledger.parked(), 1);
    }
    assert!(parks.stand(320), "still asked while the line holds");
    {
        let mut parked = pin!(parks.park(&source));
        assert_eq!(parked_woken.poll(parked.as_mut()), Poll::Pending);
        line.release();
        assert_eq!(parked_woken.poll(parked), Poll::Ready(()));
    }

    let (mut waits, mut reads, mut ahead) = (ledger.holder(), ledger.holder(), ledger.holder());
    assert_eq!(drawn_now(&mut ahead, 0, 5), Admission::Held);
    assert!(!reads.stand(400));
    let waiting = Woken::new();
    let mut wait = pin!(waits.draw(300, 20));
    assert_eq!(waiting.poll(wait.as_mut()), Poll::Pending);
    ahead.release();
    let mut draw = pin!(line.draw(100, 60));
    assert_eq!(woken.poll(draw.as_mut()), Poll::Pending);
    assert!(reads.stand(410), "the reader reading is asked; the one waiting is not");
    let mut parked = pin!(reads.park(&source));
    assert_eq!(parked_woken.poll(parked.as_mut()), Poll::Pending);
    assert_eq!(woken.poll(draw), Poll::Ready(Admission::Held));
    assert_eq!(ledger.parked(), 1);
}

/// **A ledger whose readers give nothing back asks none to park**: the
/// line alone is admitted past the room at once, as one reader's is.
#[test]
fn a_line_alone_parks_no_reader_whose_parking_gives_nothing_back() {
    let ledger = limited(10);
    let (mut line, mut later) = (ledger.holder(), ledger.holder());
    assert!(!later.stand(200));
    assert_eq!(drawn_now(&mut line, 100, 60), Admission::Held);
    assert!(!later.stand(210));
    assert_eq!(ledger.parked(), 0);
}

/// **Parking readers on threads of their own all finish**: each stands
/// before each line and parks where asked, then grows its line past the
/// room, re-reading where it gave up, and nothing is left drawn or parked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_parking_readers_all_finish() {
    let terms = Terms::of(Parallelism::workers(8, 400), Some(WorkerMemory::per_worker(50)));
    let ledger = Ledger::limited(100, Floor { holds: u64::MAX, allowance: u64::MAX }, terms);
    let source = Arc::new(crate::test_support::ParkCounting::default());
    let readers = (0..8u64).map(|reader| {
        let (mut holder, source) = (ledger.holder(), Arc::clone(&source));
        tokio::spawn(async move {
            for line in 0..20u64 {
                let start = (line * 8 + reader) * 1000;
                let mut charge = 10;
                while charge <= 300 {
                    if holder.stand(start + charge) {
                        holder.park(source.as_ref()).await;
                    }
                    match holder.draw(start, charge).await {
                        Admission::Held => charge += 30,
                        Admission::GiveUp => charge = 10,
                        Admission::Refused(floor) => panic!("refused under {floor:?}"),
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
    assert_eq!((ledger.drawn(), ledger.parked(), source.parked()), (0, 0, 0));
}
