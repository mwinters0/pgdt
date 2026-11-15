//! **Parking**: which of a ledger's readers are asked to stop reading, the
//! latest in the file first, and what their parking gives back, so a line
//! alone past its room is held in it (`docs/design/decisions.md`, "D163").
//!
//! The ledger decides when a line needs parks and how many ([`Terms`]); this
//! keeps where each reader last stood and which are asked and parked
//! ([`Readers`]). **The readers asked are those reading later in the file
//! than the line**, holding no line and waiting on none — a reader whose line
//! waits reads nothing already, nor does one standing aside at a yield — the
//! latest first, and as the line's excess falls the earliest of them are let
//! go first.

use std::collections::BTreeMap;
use std::task::Waker;

use crate::parallelism::{Parallelism, WorkerMemory};

/// **What parking readers gives back**, by how many are parked: the fall in
/// the read-buffer budget an arrangement carved for its count — what its
/// readers hold at that count, less what they hold at that many fewer,
/// neither past the budget ([`WorkerMemory::at`]). So parking gives back no
/// more than the carve took for those readers: nothing where the source
/// stated no cost, and nothing for the readers a stated count holds past
/// what its budget affords (`KD21`).
///
/// **Where the source stated no cost, nothing is also all a park could
/// buy**: such a source, a plain file among them, is budgeted no more at its
/// count than at one reader, so the floor — what one reader leaves
/// (`docs/design/decisions.md`, "D161") — already holds beside every reader,
/// and the chunk a parked plain reader drops goes back to a free list kept
/// `POOL_DEPTH` deep, freeing nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Terms(pub(super) Vec<u64>);

impl Terms {
    /// What parking gives back under `parallelism`, carved for readers each
    /// holding `memory`; nothing for one reader, which is never parked.
    pub(crate) fn of(parallelism: Parallelism, memory: Option<WorkerMemory>) -> Self {
        let Some(memory) = memory.filter(|memory| !memory.is_zero()) else {
            return Self::default();
        };
        let (jobs, budget) = (parallelism.jobs(), parallelism.budget_bytes());
        let holds = |reading: usize| memory.at(reading).min(budget);
        Self((0..jobs).map(|parked| holds(jobs) - holds(jobs - parked)).collect())
    }

    /// What `parked` readers give back.
    pub(super) fn of_parked(&self, parked: usize) -> u64 {
        self.0.get(parked).or(self.0.last()).copied().unwrap_or(0)
    }

    /// The fewest readers whose parking gives back `short` bytes, or as many
    /// as give anything back where none covers it.
    pub(super) fn parks_for(&self, short: u64) -> usize {
        let short = short.min(self.of_parked(usize::MAX));
        match short {
            0 => 0,
            short => self.0.iter().position(|&back| back >= short).unwrap_or(0),
        }
    }
}

/// One holder's reader: where it last stood, and whether it is asked to
/// park and has.
#[derive(Debug, Default)]
struct Reader {
    /// Where it last said it reads, or `None` before its first read.
    at: Option<u64>,
    /// Whether the ledger asks it to park.
    asked: bool,
    /// Whether it is parked now.
    parked: bool,
    /// What wakes it, while it is parked.
    waker: Option<Waker>,
}

/// Each live holder's reader, by the holder's identity.
#[derive(Debug, Default)]
pub(super) struct Readers(BTreeMap<u64, Reader>);

impl Readers {
    /// A reader for the holder `id`, standing nowhere yet.
    pub(super) fn join(&mut self, id: u64) {
        self.0.insert(id, Reader::default());
    }

    /// The holder `id`'s reader gone, its holder dropped.
    pub(super) fn leave(&mut self, id: u64) {
        self.0.remove(&id);
    }

    /// The holder `id`'s reader reads from `at` now.
    pub(super) fn stand(&mut self, id: u64, at: u64) {
        if let Some(reader) = self.0.get_mut(&id) {
            reader.at = Some(at);
        }
    }

    /// The holder `id`'s reader stands nowhere until it next stands: it
    /// reads nothing meanwhile, so it is asked nothing.
    pub(super) fn stand_aside(&mut self, id: u64) {
        if let Some(reader) = self.0.get_mut(&id) {
            reader.at = None;
        }
    }

    /// Whether the holder `id`'s reader is asked to park.
    pub(super) fn asked(&self, id: u64) -> bool {
        self.0.get(&id).is_some_and(|reader| reader.asked)
    }

    /// How many are parked now.
    pub(super) fn parked(&self) -> usize {
        self.0.values().filter(|reader| reader.parked).count()
    }

    /// Whether every reader asked has parked: none is left reading to wait
    /// on.
    pub(super) fn settled(&self) -> bool {
        self.0.values().all(|reader| !reader.asked || reader.parked)
    }

    /// **Ask `parks` readers to park**: those standing past `after`, the
    /// latest first, but `skip`'s — the line's own and those whose line holds
    /// or waits — letting every other one go, gathering into `woken` each
    /// parked one let go. Whether any reader's ask moved.
    pub(super) fn ask(
        &mut self,
        after: u64,
        skip: &[u64],
        parks: usize,
        woken: &mut Vec<Waker>,
    ) -> bool {
        let mut later: Vec<(u64, u64)> = self
            .0
            .iter()
            .filter(|(id, _)| !skip.contains(id))
            .filter_map(|(&id, reader)| reader.at.filter(|&at| at > after).map(|at| (at, id)))
            .collect();
        later.sort_unstable_by(|a, b| b.cmp(a));
        let asked: Vec<u64> = later.iter().take(parks).map(|&(_, id)| id).collect();
        let mut moved = false;
        for (id, reader) in &mut self.0 {
            let ask = asked.contains(id);
            if reader.asked == ask {
                continue;
            }
            moved = true;
            reader.asked = ask;
            if !ask {
                woken.extend(reader.waker.take());
            }
        }
        moved
    }

    /// **The holder `id`'s reader parks, or stays parked, woken by `waker`**
    /// — `Some(true)` where it parks now, `Some(false)` where it was already
    /// parked — or `None` where it is not asked, and goes on reading.
    pub(super) fn park(&mut self, id: u64, waker: &Waker) -> Option<bool> {
        let reader = self.0.get_mut(&id).filter(|reader| reader.asked)?;
        let newly = !reader.parked;
        reader.parked = true;
        if !reader.waker.as_ref().is_some_and(|known| known.will_wake(waker)) {
            reader.waker = Some(waker.clone());
        }
        Some(newly)
    }

    /// The holder `id`'s reader resumes.
    pub(super) fn resume(&mut self, id: u64) {
        if let Some(reader) = self.0.get_mut(&id) {
            (reader.parked, reader.waker) = (false, None);
        }
    }
}
