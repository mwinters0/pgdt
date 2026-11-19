//! **The ledger**: what one run's held lines may hold past their chunk term,
//! carved from its allowance by [`Arrangement::resolve`](crate::Arrangement::resolve),
//! and the order a draw it cannot meet waits in
//! (`docs/design/decisions.md`, "D157").
//!
//! A reader whose held line grows past its chunk term draws the line's whole
//! charge here, line by line, through its [`Holder`]. **A draw that cannot be
//! met waits, and the waiting line earliest in the file is admitted first**:
//! no draw growing a line is met while an earlier line waits, so the leftmost line always
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
//! the earliest waiting line does, and the readers after the earliest line to
//! park, as below; the earliest line is never asked.
//!
//! **A line one reader could not hold is refused** ([`Admission::Refused`]):
//! a draw whose charge passes the ledger's [`Floor`] — what the allowance
//! leaves one reader, whatever the worker count — fails at once, wherever
//! the line lies in the file, since one reader could not hold it whatever
//! waited or gave its room up (`docs/design/decisions.md`, "D161").
//!
//! **Lines and statistics past the room park the latest readers**
//! ([`Holder::park`]): the readers later in the file than the earliest line
//! are asked to park, the latest first, until what their parking gives back
//! covers the excess, and resume as it falls. Only a line alone is admitted on
//! their parking: where no other line holds and the earliest still does not
//! fit, it is admitted once they have, or once none is left reading to ask
//! (`docs/design/decisions.md`, "D163").
//!
//! **A ledger without limit** ([`Ledger::unlimited`]) meets every draw at once:
//! an allowance nobody stated carves nothing to wait on, and refuses nothing.

use std::collections::BTreeMap;
use std::future::poll_fn;
use std::ops::Bound;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

mod park;

use self::park::Readers;
pub(crate) use self::park::Terms;
use crate::source::{ByteRangeSource, ParkedReader};

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
    limits: Limits,
    state: Mutex<State>,
}

/// What a limited ledger was carved with, fixed for its run.
#[derive(Debug)]
struct Limits {
    capacity: u64,
    floor: Floor,
    /// What parking its readers gives back.
    terms: Terms,
}

/// **The most one line may hold**: what the allowance leaves one reader,
/// whatever the worker count — the earliest line admitted alone up to it, its
/// later holders parked, and any line past it refused at once — once what stays beside the line is
/// off it, and the allowance that was carved from, both of which a refusal
/// names (`docs/design/decisions.md`, "D161").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Floor {
    /// The bytes one line may hold at most.
    pub holds: u64,
    /// The bytes that came off it as staying beside the line: the statistics
    /// the run keeps, other live scans' line charges and what the allocator
    /// retains.
    pub beside: u64,
    /// The resident allowance the floor was carved from, in bytes.
    pub allowance: u64,
}

impl Floor {
    /// The refusal of a line drawing `needs` bytes held from `offset`, which
    /// passes this floor.
    pub fn refusal(&self, offset: u64, needs: u64) -> pgdt_core::Error {
        pgdt_core::Error::LineOutgrowsMemory {
            offset,
            needs,
            holds: self.holds,
            beside: self.beside,
            allowance: self.allowance,
        }
    }

    /// The refusal of a line drawing `needs` bytes held from `offset`, where
    /// they pass this floor.
    pub fn check(&self, offset: u64, needs: u64) -> pgdt_core::Result<()> {
        match needs > self.holds {
            true => Err(self.refusal(offset, needs)),
            false => Ok(()),
        }
    }
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
    /// Each live holder's reader, by its identity.
    readers: Readers,
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
    /// The charge passes the ledger's [`Floor`], so no wait would admit it:
    /// its reader fails with [`Floor::refusal`], and what the line held is
    /// returned.
    Refused(Floor),
}

impl Ledger {
    /// The ledger of an allowance nobody stated: every draw is met at once.
    pub fn unlimited() -> Self {
        Self { shared: None }
    }

    /// A ledger of `capacity` bytes, refusing a line past `floor`, its
    /// readers' parking giving back `terms`, which only
    /// [`Arrangement::resolve`](crate::Arrangement::resolve) carves.
    pub(crate) fn limited(capacity: u64, floor: Floor, terms: Terms) -> Self {
        let state = Mutex::new(State::default());
        let limits = Limits { capacity, floor, terms };
        Self { shared: Some(Arc::new(Shared { limits, state })) }
    }

    /// The bytes it may lend, or `None` without limit.
    pub fn capacity(&self) -> Option<u64> {
        self.shared.as_ref().map(|shared| shared.limits.capacity)
    }

    /// The most one line may hold, or `None` without limit: what a read over
    /// mapped blocks holds its longest line against before it reads a byte.
    pub fn floor(&self) -> Option<Floor> {
        self.shared.as_ref().map(|shared| shared.limits.floor)
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
    /// The readers after the earliest line are asked to park meanwhile, as
    /// the lines give theirs up only when they next draw
    /// (`docs/design/decisions.md`, "D163"). A draw falling wakes every
    /// waiting line, which may now be admitted.
    pub fn draw_statistics(&self, total: u64) {
        let Some(shared) = &self.shared else { return };
        let mut woken = Vec::new();
        {
            let mut state = shared.lock();
            let fell = total < state.statistics;
            state.statistics = total;
            let short = state.drawn.saturating_add(total).saturating_sub(shared.limits.capacity);
            if let Some(&earliest) = state.lines.keys().next() {
                state.ask_later(earliest, short, &mut woken);
            }
            if fell {
                state.wake_waiters(&mut woken);
            }
            state.rebalance(&shared.limits, &mut woken);
        }
        woken.into_iter().for_each(Waker::wake);
    }

    /// A holder for one reader, drawing for one line at a time.
    pub fn holder(&self) -> Holder {
        let id = self.shared.as_ref().map_or(0, |shared| {
            let mut state = shared.lock();
            state.next_holder += 1;
            let id = state.next_holder;
            state.readers.join(id);
            id
        });
        Holder { ledger: self.clone(), id, line: None, held: 0 }
    }

    /// How many of its readers are parked now: none without limit.
    pub fn parked(&self) -> usize {
        self.shared.as_ref().map_or(0, |shared| shared.lock().readers.parked())
    }
}

/// **One reader's draws**, for one line at a time: a draw for another line
/// returns the last one's first, and dropping the holder returns what it
/// holds. A reader that stands where it reads ([`Holder::stand`]) may be
/// asked to park ([`Holder::park`]).
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
    /// met at once, unless its holder has been asked to give it up.
    ///
    /// **The earliest line is admitted past the capacity where no other line
    /// holds**, whatever the statistics' draw holds, and never waits on
    /// statistics, which give nothing up (`docs/design/decisions.md`,
    /// "D157") — once the readers later in the file it asks to park have,
    /// or have left (`docs/design/decisions.md`, "D163"). **A charge past
    /// the ledger's [`Floor`] is refused** at once, wherever its line lies,
    /// what it held returned
    /// (`docs/design/decisions.md`, "D161").
    ///
    /// Dropped while it waits, the line keeps what it held and waits for
    /// nothing.
    pub async fn draw(&mut self, start: u64, charge: u64) -> Admission {
        if self.line.is_some_and(|line| line != start) {
            self.release();
        }
        self.line = Some(start);
        #[cfg(feature = "introspect")]
        let before = self.held;
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
            Admission::Refused(_) => self.release(),
        }
        // The moments an instrumented binary reads its heap at
        // (`pgdt_core::instrument::LineMoment`).
        #[cfg(feature = "introspect")]
        match admission {
            Admission::Held if before == 0 => {
                self.told(pgdt_core::instrument::LineMoment::Admitted, start, charge);
            }
            Admission::GiveUp if before > 0 => {
                self.told(pgdt_core::instrument::LineMoment::Returned, start, before);
            }
            _ => {}
        }
        admission
    }

    /// Tell an instrumented binary `moment` of the line at `start`, holding
    /// `charge`, with what the ledger holds now.
    #[cfg(feature = "introspect")]
    fn told(&self, moment: pgdt_core::instrument::LineMoment, start: u64, charge: u64) {
        let drawn = self.ledger.capacity().map(|_| self.ledger.drawn());
        pgdt_core::internal::line_moment(pgdt_core::instrument::LineEvent {
            moment,
            start,
            charge,
            drawn,
        });
    }

    /// Return what the line holds: its row was emitted, or its reader drops
    /// it.
    pub fn release(&mut self) {
        #[cfg(feature = "introspect")]
        let returned = self.line.filter(|_| self.held > 0).map(|start| (start, self.held));
        if let (Some(start), Some(shared)) = (self.line.take(), &self.ledger.shared) {
            let mut woken = Vec::new();
            {
                let mut state = shared.lock();
                if let Some(line) = state.lines.remove(&(start, self.id)) {
                    state.drawn -= line.held;
                }
                state.wake_waiters(&mut woken);
                state.rebalance(&shared.limits, &mut woken);
            }
            woken.into_iter().for_each(Waker::wake);
        }
        self.held = 0;
        #[cfg(feature = "introspect")]
        if let Some((start, held)) = returned {
            self.told(pgdt_core::instrument::LineMoment::Returned, start, held);
        }
    }

    /// What its line holds.
    pub fn held(&self) -> u64 {
        self.held
    }

    /// **Say that its reader reads from `at`**, before it reads there, and
    /// whether it is asked to park ([`Holder::park`]): a reader that never
    /// stands is never asked. Never asked without limit.
    pub fn stand(&mut self, at: u64) -> bool {
        let Some(shared) = &self.ledger.shared else { return false };
        let mut woken = Vec::new();
        let asked = {
            let mut state = shared.lock();
            state.readers.stand(self.id, at);
            state.rebalance(&shared.limits, &mut woken);
            state.readers.asked(self.id)
        };
        woken.into_iter().for_each(Waker::wake);
        asked
    }

    /// **Say that its reader reads nothing until it next stands**, as it is
    /// suspended at a yield: no park is asked of it meanwhile, and a line it
    /// was asked for waits on it no longer, so a caller holding its stream
    /// back stalls no line (`docs/design/decisions.md`, "D163"). Nothing
    /// without limit.
    pub fn stand_aside(&mut self) {
        let Some(shared) = &self.ledger.shared else { return };
        let mut woken = Vec::new();
        {
            let mut state = shared.lock();
            state.readers.stand_aside(self.id);
            state.rebalance(&shared.limits, &mut woken);
        }
        woken.into_iter().for_each(Waker::wake);
    }

    /// **Park its reader until the ledger no longer asks it to**, holding
    /// no read of `source` meanwhile: the park is announced to `source`
    /// ([`ParkedReader`]), so its pools give back the reader's share, and a
    /// line waiting on the park is woken. Returns at once where it is not
    /// asked, the park announced and withdrawn; dropped, the reader resumes (`docs/design/decisions.md`,
    /// "D163").
    pub async fn park(&mut self, source: &dyn ByteRangeSource) {
        let Some(shared) = self.ledger.shared.clone() else { return };
        let _parked = ParkedReader::park(source);
        let mut parking = Parking { shared: &shared, id: self.id };
        poll_fn(|cx| parking.poll(cx)).await;
    }

    /// Leave the ledger's readers, as its holder drops: a line waiting on
    /// its park waits for it no longer.
    fn leave(&mut self) {
        let Some(shared) = &self.ledger.shared else { return };
        let mut woken = Vec::new();
        {
            let mut state = shared.lock();
            state.readers.leave(self.id);
            state.wake_waiters(&mut woken);
            state.rebalance(&shared.limits, &mut woken);
        }
        woken.into_iter().for_each(Waker::wake);
    }
}

/// One reader's park on a limited ledger: dropped, the reader is parked no
/// longer.
struct Parking<'a> {
    shared: &'a Shared,
    id: u64,
}

impl Parking<'_> {
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let mut woken = Vec::new();
        let poll = {
            let mut state = self.shared.lock();
            match state.readers.park(self.id, cx.waker()) {
                // What it gave back may admit the line that asked it.
                Some(true) => {
                    state.wake_waiters(&mut woken);
                    Poll::Pending
                }
                Some(false) => Poll::Pending,
                None => Poll::Ready(()),
            }
        };
        woken.into_iter().for_each(Waker::wake);
        poll
    }
}

impl Drop for Parking<'_> {
    fn drop(&mut self) {
        self.shared.lock().readers.resume(self.id);
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        self.release();
        self.leave();
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
            &self.shared.limits,
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
            state.rebalance(&self.shared.limits, &mut woken);
        }
        woken.into_iter().for_each(Waker::wake);
    }
}

impl State {
    /// Admit `key`'s line to `charge` in all, give its draw up where it was
    /// asked to, or leave it waiting — asking later lines to give theirs up
    /// where it is the earliest waiting, and the latest readers to park where
    /// it is alone past the capacity — gathering into `woken` the waiters
    /// whose answer may have moved.
    fn poll_draw(
        &mut self,
        limits: &Limits,
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
            self.rebalance(limits, woken);
            return Poll::Ready(Admission::GiveUp);
        }
        if charge <= line.held {
            (line.wants, line.waker) = (None, None);
            return Poll::Ready(Admission::Held);
        }
        if charge > limits.floor.holds {
            // Never admitted, however long it waited: what it holds is its
            // holder's to return, and a later line may have waited on it.
            (line.wants, line.waker) = (None, None);
            self.wake_waiters(woken);
            return Poll::Ready(Admission::Refused(limits.floor));
        }
        let held = line.held;
        let others = self.drawn - held;
        let beside = others.saturating_add(self.statistics);
        let earliest = self.lines.range(..key).all(|(_, line)| line.wants.is_none());
        let fits = beside.saturating_add(charge) <= limits.capacity;
        let line = self.lines.get_mut(&key).expect("the line was entered above");
        line.wants = Some(charge);
        // Alone past the capacity, it is admitted once the readers asked to
        // park have (`docs/design/decisions.md`, "D163").
        if earliest && (fits || others == 0 && self.parked_enough(limits, woken)) {
            self.drawn += charge - held;
            let line = self.lines.get_mut(&key).expect("the line was entered above");
            (line.held, line.wants, line.waker) = (charge, None, None);
            // What is left may hold the next waiting line.
            self.wake_waiters(woken);
            self.rebalance(limits, woken);
            return Poll::Ready(Admission::Held);
        }
        let line = self.lines.get_mut(&key).expect("the line was entered above");
        if !line.waker.as_ref().is_some_and(|known| known.will_wake(waker)) {
            line.waker = Some(waker.clone());
        }
        if earliest && others > 0 {
            self.ask_later(
                key,
                beside.saturating_add(charge).saturating_sub(limits.capacity),
                woken,
            );
        }
        // A reader whose line waits reads nothing, and is asked nothing.
        self.rebalance(limits, woken);
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

    /// **Ask the readers after the earliest busy line to park as the lines
    /// and statistics pass the capacity**, the latest in the file first, and
    /// let every other reader resume — gathering into `woken` the parked
    /// readers let go and, where any reader's ask moved, every waiting line —
    /// and answer what they hold past the capacity, the earliest line's
    /// waiting charge included where it is alone
    /// (`docs/design/decisions.md`, "D163").
    fn rebalance(&mut self, limits: &Limits, woken: &mut Vec<Waker>) -> u64 {
        let busy = |line: &Line| line.held > 0 || line.wants.is_some();
        let first = self.lines.iter().find(|(_, line)| busy(line));
        let (short, after, parks) = match first {
            None => (0, u64::MAX, 0),
            Some((&(start, _), line)) => {
                let alone = self.drawn == line.held;
                let waiting = match (alone, line.wants) {
                    (true, Some(wants)) => wants.saturating_sub(line.held),
                    _ => 0,
                };
                let holds = self.drawn.saturating_add(self.statistics).saturating_add(waiting);
                let short = holds.saturating_sub(limits.capacity);
                (short, start, limits.terms.parks_for(short))
            }
        };
        // The line's own reader, and every reader whose line holds or waits.
        let skip: Vec<u64> =
            self.lines.iter().filter(|(_, line)| busy(line)).map(|(&(_, id), _)| id).collect();
        if self.readers.ask(after, &skip, parks, woken) {
            self.wake_waiters(woken);
        }
        short
    }

    /// Whether the line alone past the capacity may be admitted: what the
    /// parked readers give back covers it, or every reader asked has parked,
    /// none being left to ask.
    fn parked_enough(&mut self, limits: &Limits, woken: &mut Vec<Waker>) -> bool {
        let short = self.rebalance(limits, woken);
        limits.terms.of_parked(self.readers.parked()) >= short || self.readers.settled()
    }
}

#[cfg(test)]
mod tests;
