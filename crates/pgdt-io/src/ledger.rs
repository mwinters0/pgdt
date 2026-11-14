//! **The ledger**: what one run's held lines may hold past their chunk term,
//! carved from its allowance by [`Arrangement::resolve`](crate::Arrangement::resolve),
//! and the order a draw it cannot meet waits in
//! (`docs/design/decisions.md`, "D157").
//!
//! A reader whose held line grows past its chunk term draws the line's whole
//! charge here, line by line, through its [`Holder`]. **A draw that cannot be
//! met waits, and the waiting line earliest in the file is admitted first**:
//! no draw is met while an earlier line waits, so the leftmost line always
//! progresses and no pass deadlocks. **When the earliest waiting line cannot
//! be admitted, the holders of later lines are asked to give theirs up**, the
//! latest in the file first, until it can be; a holder asked gives its draw up
//! only when it next draws, never under it, so a reader suspended mid-line
//! keeps what it holds, and one told [`Admission::GiveUp`] re-reads its line
//! from the line's start, drawing anew.
//!
//! **A pass's statistics draw on the same room, ahead of every line**
//! ([`Ledger::draw_statistics`]): their draw never waits and is never declined
//! for a line, and lines are admitted from what it leaves, so what a block's
//! statistics hold never depends on when a long line was held. A draw passing
//! the room asks the holders of lines to give theirs up, the latest first, as
//! the earliest waiting line does; the earliest line is never asked.
//!
//! **A ledger without limit** ([`Ledger::unlimited`]) meets every draw at once:
//! an allowance nobody stated carves nothing to wait on.

use std::collections::BTreeMap;
use std::future::poll_fn;
use std::ops::Bound;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

/// One run's ledger: a capacity in bytes, or none, and what its holders hold
/// against it now. A clone is the same ledger, shared by every reader of the
/// run, never a second one carved alike.
#[derive(Debug, Clone)]
pub struct Ledger {
    shared: Option<Arc<Shared>>,
}

/// A limited ledger's state, behind one lock: a draw's admission reads every
/// line at once.
#[derive(Debug)]
struct Shared {
    capacity: u64,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    /// What every line holds, summed.
    drawn: u64,
    /// What the statistics' draw holds, beside the lines.
    statistics: u64,
    /// The next [`Holder`]'s identity.
    next_holder: u64,
    /// Each line holding or waiting, in file order.
    lines: BTreeMap<Key, Line>,
}

/// A line's start in the file, and the holder drawing for it, which breaks a
/// tie no two readers of one pass meet.
type Key = (u64, u64);

/// What one holder's line holds and waits for.
#[derive(Debug, Default)]
struct Line {
    held: u64,
    /// The charge it waits to hold in all, while it waits.
    wants: Option<u64>,
    /// What wakes it, while it waits.
    waker: Option<Waker>,
    /// Whether an earlier waiting line asked it to give its draw up.
    give_up: bool,
}

/// What a [`Holder::draw`] answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// The line holds the charge it drew.
    Held,
    /// The line gave up what it held, an earlier one needing it: its reader
    /// drops the line and re-reads it from its start, drawing anew.
    GiveUp,
}

impl Ledger {
    /// The ledger of an allowance nobody stated: every draw is met at once.
    pub fn unlimited() -> Self {
        Self { shared: None }
    }

    /// A ledger of `capacity` bytes, which only
    /// [`Arrangement::resolve`](crate::Arrangement::resolve) carves.
    pub(crate) fn limited(capacity: u64) -> Self {
        let state = Mutex::new(State::default());
        Self { shared: Some(Arc::new(Shared { capacity, state })) }
    }

    /// The bytes it may lend, or `None` without limit.
    pub fn capacity(&self) -> Option<u64> {
        self.shared.as_ref().map(|shared| shared.capacity)
    }

    /// What its lines and its statistics hold now: zero without limit, which
    /// keeps no account.
    pub fn drawn(&self) -> u64 {
        self.shared.as_ref().map_or(0, |shared| {
            let state = shared.lock();
            state.drawn + state.statistics
        })
    }

    /// What the statistics' draw holds now: zero without limit.
    pub fn statistics(&self) -> u64 {
        self.shared.as_ref().map_or(0, |shared| shared.lock().statistics)
    }

    /// **Hold `total` bytes in all for the run's statistics**, in place of
    /// what their draw held — admitted at once, ahead of every line, and
    /// never waiting or declined for one: what statistics may hold is bounded
    /// by their own allowance, which a block that does not fit declines
    /// against ([`Arrangement::statistics_allowance`](crate::Arrangement::statistics_allowance)).
    ///
    /// **A draw passing the capacity asks the holders of lines to give theirs
    /// up**, the latest in the file first, until what passes it is asked for,
    /// each when it next draws; **the earliest line is never asked**, being
    /// admitted past the capacity where no other line holds, so asking it
    /// would buy a re-read and no room (`docs/design/decisions.md`, "D157").
    /// A draw falling wakes every waiting line, which may now be admitted.
    pub fn draw_statistics(&self, total: u64) {
        let Some(shared) = &self.shared else { return };
        let mut woken = Vec::new();
        {
            let mut state = shared.lock();
            let fell = total < state.statistics;
            state.statistics = total;
            let short = state.drawn.saturating_add(total).saturating_sub(shared.capacity);
            if let Some(&earliest) = state.lines.keys().next() {
                state.ask_later(earliest, short, &mut woken);
            }
            if fell {
                state.wake_waiters(&mut woken);
            }
        }
        woken.into_iter().for_each(Waker::wake);
    }

    /// A holder for one reader, drawing for one line at a time.
    pub fn holder(&self) -> Holder {
        let id = self.shared.as_ref().map_or(0, |shared| {
            let mut state = shared.lock();
            state.next_holder += 1;
            state.next_holder
        });
        Holder { ledger: self.clone(), id, line: None, held: 0 }
    }
}

/// **One reader's draws**, for one line at a time: a draw for another line
/// returns the last one's first, and dropping the holder returns what it
/// holds.
#[derive(Debug)]
pub struct Holder {
    ledger: Ledger,
    id: u64,
    /// The start of the line it draws for.
    line: Option<u64>,
    held: u64,
}

impl Holder {
    /// **Hold `charge` bytes in all for the line starting at `start`**,
    /// waiting until it is admitted ([`Ledger`]'s order) — the line's whole
    /// charge, never its growth, so a draw no larger than what it holds is
    /// met at once.
    ///
    /// **The earliest line is admitted past the capacity where no other line
    /// holds**, whatever the statistics' draw holds, so a line the ledger
    /// cannot hold is read as it was before a ledger charged it, and never
    /// waits on statistics, which give nothing up: refusing one for memory
    /// is the floor's, which parks readers first
    /// (`docs/design/decisions.md`, "D157").
    ///
    /// Dropped while it waits, the line keeps what it held and waits for
    /// nothing.
    pub async fn draw(&mut self, start: u64, charge: u64) -> Admission {
        if self.line.is_some_and(|line| line != start) {
            self.release();
        }
        self.line = Some(start);
        let admission = match self.ledger.shared.clone() {
            None => Admission::Held,
            Some(shared) => {
                let mut wait = Wait { shared: &shared, key: (start, self.id), settled: false };
                poll_fn(|cx| wait.poll(charge, cx)).await
            }
        };
        match admission {
            Admission::Held => self.held = self.held.max(charge),
            Admission::GiveUp => (self.line, self.held) = (None, 0),
        }
        admission
    }

    /// Return what the line holds: its row was emitted, or its reader drops
    /// it.
    pub fn release(&mut self) {
        if let (Some(start), Some(shared)) = (self.line.take(), &self.ledger.shared) {
            let mut woken = Vec::new();
            {
                let mut state = shared.lock();
                if let Some(line) = state.lines.remove(&(start, self.id)) {
                    state.drawn -= line.held;
                }
                state.wake_waiters(&mut woken);
            }
            woken.into_iter().for_each(Waker::wake);
        }
        self.held = 0;
    }

    /// What its line holds.
    pub fn held(&self) -> u64 {
        self.held
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        self.release();
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic under the lock leaves counts no later draw can trust, as it
        // leaves the reader that held it.
        self.state.lock().expect("a ledger's lock is never poisoned")
    }
}

/// One draw's wait on a limited ledger: dropped unsettled, its line stops
/// waiting.
struct Wait<'a> {
    shared: &'a Shared,
    key: Key,
    settled: bool,
}

impl Wait<'_> {
    fn poll(&mut self, charge: u64, cx: &mut Context<'_>) -> Poll<Admission> {
        let mut woken = Vec::new();
        let poll = self.shared.lock().poll_draw(
            self.shared.capacity,
            self.key,
            charge,
            cx.waker(),
            &mut woken,
        );
        woken.into_iter().for_each(Waker::wake);
        self.settled = poll.is_ready();
        poll
    }
}

impl Drop for Wait<'_> {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        let mut woken = Vec::new();
        {
            let mut state = self.shared.lock();
            if let Some(line) = state.lines.get_mut(&self.key) {
                line.wants = None;
                line.waker = None;
                if line.held == 0 {
                    state.lines.remove(&self.key);
                }
            }
            // A later line may have waited behind this one.
            state.wake_waiters(&mut woken);
        }
        woken.into_iter().for_each(Waker::wake);
    }
}

impl State {
    /// Admit `key`'s line to `charge` in all, give its draw up where it was
    /// asked to, or leave it waiting — asking later lines to give theirs up
    /// where it is the earliest waiting — gathering into `woken` the waiters
    /// whose answer may have moved.
    fn poll_draw(
        &mut self,
        capacity: u64,
        key: Key,
        charge: u64,
        waker: &Waker,
        woken: &mut Vec<Waker>,
    ) -> Poll<Admission> {
        let line = self.lines.entry(key).or_default();
        if line.give_up {
            let held = line.held;
            self.lines.remove(&key);
            self.drawn -= held;
            self.wake_waiters(woken);
            return Poll::Ready(Admission::GiveUp);
        }
        if charge <= line.held {
            (line.wants, line.waker) = (None, None);
            return Poll::Ready(Admission::Held);
        }
        let held = line.held;
        let others = self.drawn - held;
        let beside = others.saturating_add(self.statistics);
        let earliest = self.lines.range(..key).all(|(_, line)| line.wants.is_none());
        if earliest && (others == 0 || beside.saturating_add(charge) <= capacity) {
            self.drawn += charge - held;
            let line = self.lines.get_mut(&key).expect("the line was entered above");
            (line.held, line.wants, line.waker) = (charge, None, None);
            // What is left may hold the next waiting line.
            self.wake_waiters(woken);
            return Poll::Ready(Admission::Held);
        }
        let line = self.lines.get_mut(&key).expect("the line was entered above");
        line.wants = Some(charge);
        if !line.waker.as_ref().is_some_and(|known| known.will_wake(waker)) {
            line.waker = Some(waker.clone());
        }
        if earliest {
            self.ask_later(key, beside.saturating_add(charge).saturating_sub(capacity), woken);
        }
        Poll::Pending
    }

    /// Ask the lines after `key` to give up what they hold, the latest
    /// first, until `short` bytes are asked for or none is left; one asked
    /// already counts.
    fn ask_later(&mut self, key: Key, mut short: u64, woken: &mut Vec<Waker>) {
        let later = self.lines.range_mut((Bound::Excluded(key), Bound::Unbounded));
        for (_, line) in later.rev() {
            if short == 0 {
                break;
            }
            if line.held == 0 {
                continue;
            }
            if !line.give_up {
                line.give_up = true;
                woken.extend(line.waker.take());
            }
            short = short.saturating_sub(line.held);
        }
    }

    /// Gather every waiting line's waker: what it waited on may have moved.
    fn wake_waiters(&mut self, woken: &mut Vec<Waker>) {
        woken.extend(self.lines.values_mut().filter_map(|line| line.waker.take()));
    }
}

#[cfg(test)]
mod tests;
