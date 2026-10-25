//! The chunks ahead of the frontier: what the speculations guessed, and the
//! decodes through the engine those guesses let begin.
//!
//! **Each chunk's window is resolved in order.** From a place the frontier
//! stands at with its history known — the chain's anchor — a guess beginning
//! there gives the history at its stop: its marked end resolved against the
//! history at its start. So, guess by guess while each begins where the last
//! stops, every chunk's start gets a window before the frontier reaches it,
//! and the engine's decode of each can begin at once, on whatever thread the
//! caller has. Each window rests on every guess before it; the frontier proves
//! one only by standing at that decode's start with that very history, which
//! nothing but the engine's decode of everything before it gives it. A guess
//! the limit cut is followed by the one continuing it from its stop, so a
//! limit only lengthens the chain by a link. **A guess stopping short of
//! where the next begins ends the chain**, refusals or a false start having
//! cut it: the frontier decodes on to the next guess, anchors there, and
//! decodes that chunk itself.
//!
//! Guesses and decodes are held by the bit they begin at, a chunk holding as
//! many as its guess was cut into.
//!
//! **A generation is one anchor's chain**: anchoring again, or a window the
//! frontier contradicts, drops every decode of the last — queued, running or
//! done — as resting on a window nothing proved, and a running one's result
//! is dropped when it comes back.

use std::collections::{BTreeMap, VecDeque};

use crate::chunk::{ChunkDecode, ChunkDecoded};
use crate::speculate::{Guess, Point};

use super::task::TaskId;

/// What the chain holds for a place the frontier stands at.
pub(super) enum Here {
    /// The engine's decode of the chunk beginning here.
    Decoded(ChunkDecoded),
    /// That decode is running: wait for it.
    Running(TaskId),
    /// The frontier decodes the chunk beginning here itself: its decode was
    /// never handed out, or the chain was anchored here.
    Frontier,
    /// Nothing begins here.
    Nothing,
}

pub(super) struct Chain {
    file_len: u64,
    generation: u64,
    /// What each speculation guessed, with its chunk, by the bit it begins
    /// at, until the frontier passes it.
    guesses: BTreeMap<u64, (u64, Guess)>,
    /// Where the last chunk given a decode stops, and the history there as
    /// resolved: `None` where no more can be resolved until the frontier
    /// anchors again.
    end: Option<(Point, Vec<u8>)>,
    queued: VecDeque<ChunkDecode>,
    /// Each of this generation's decodes handed out, by the bit it begins
    /// at, with its chunk and where it begins.
    running: BTreeMap<u64, (u64, Point)>,
    decoded: BTreeMap<u64, ChunkDecoded>,
}

impl Chain {
    pub(super) fn new(file_len: u64) -> Chain {
        Chain {
            file_len,
            generation: 0,
            guesses: BTreeMap::new(),
            end: None,
            queued: VecDeque::new(),
            running: BTreeMap::new(),
            decoded: BTreeMap::new(),
        }
    }

    /// Chunk `chunk`'s guess, with the frontier at `bit`.
    pub(super) fn guessed(&mut self, chunk: u64, guess: Guess, bit: u64) {
        if !behind(&guess, bit) {
            self.guesses.insert(guess.start.bit(), (chunk, guess));
            self.extend();
        }
    }

    /// A decode's result, with the frontier at `bit`.
    pub(super) fn decoded(&mut self, decoded: ChunkDecoded, bit: u64) {
        let start = decoded.start.bit();
        let current =
            decoded.generation == self.generation && self.running.remove(&start).is_some();
        if current && start >= bit {
            self.decoded.insert(start, decoded);
        }
    }

    /// The next decode to hand out, in chunk order.
    pub(super) fn next_task(&mut self) -> Option<ChunkDecode> {
        let task = self.queued.pop_front()?;
        self.running.insert(task.start().bit(), (task.chunk(), task.start()));
        Some(task)
    }

    /// Whether the decode handed out from bit `start` is still wanted.
    pub(super) fn wants(&self, start: u64, generation: u64) -> bool {
        generation == self.generation && self.running.contains_key(&start)
    }

    /// Drop what the frontier, standing at `bit`, has passed.
    pub(super) fn pass(&mut self, bit: u64) {
        self.guesses.retain(|_, (_, guess)| !behind(guess, bit));
        self.decoded.retain(|&start, _| start >= bit);
        self.queued.retain(|task| task.start().bit() >= bit);
        if self.end.as_ref().is_some_and(|(point, _)| point.bit() < bit) {
            self.end = None;
        }
    }

    /// What begins at `point`, where the frontier stands with `history`
    /// before it: a guess beginning there and nothing else anchors the chain.
    pub(super) fn at(&mut self, point: Point, history: impl FnOnce() -> Vec<u8>) -> Here {
        let bit = point.bit();
        if self.decoded.get(&bit).is_some_and(|d| d.start == point) {
            return Here::Decoded(self.decoded.remove(&bit).expect("a decoded chunk"));
        }
        if let Some(&(chunk, start)) = self.running.get(&bit)
            && start == point
        {
            return Here::Running(TaskId::decode(chunk, bit, self.generation));
        }
        if let Some(i) = self.queued.iter().position(|task| task.start() == point) {
            self.queued.remove(i);
            return Here::Frontier;
        }
        let Some((_, guess)) = self.guess_at(point) else { return Here::Nothing };
        let end = resolved(guess, &history());
        self.reset();
        self.end = end;
        self.extend();
        Here::Frontier
    }

    /// Drop every decode, the chain ending until the frontier anchors again.
    pub(super) fn reset(&mut self) {
        self.generation += 1;
        self.end = None;
        self.queued.clear();
        self.running.clear();
        self.decoded.clear();
    }

    /// The decodes handed out and the results held.
    pub(super) fn pending(&self) -> usize {
        self.guesses.len() + self.running.len() + self.decoded.len()
    }

    /// The decodes done, held for the frontier.
    pub(super) fn ready(&self) -> usize {
        self.decoded.len()
    }

    /// The bytes the chain holds: the guesses, the decodes done, and the
    /// windows of those queued and of its end.
    pub(super) fn held(&self) -> usize {
        self.guesses.values().map(|(_, guess)| guess.held()).sum::<usize>()
            + self.decoded.values().map(ChunkDecoded::held).sum::<usize>()
            + self.queued.iter().map(ChunkDecode::held).sum::<usize>()
            + self.end.as_ref().map_or(0, |(_, window)| window.capacity())
    }

    /// Queue a decode of each guess beginning where the chain ends.
    fn extend(&mut self) {
        while let Some((point, window)) = self.end.take() {
            let Some((chunk, guess)) = self.guess_at(point) else {
                self.end = Some((point, window));
                return;
            };
            let next = resolved(guess, &window);
            let (stop, len) = (guess.stop, guess.len);
            let id = (chunk, self.generation);
            self.queued.push_back(ChunkDecode::new(id, point, window, stop, len, self.file_len));
            self.end = next;
        }
    }

    /// The guess beginning at `point`, and its chunk: one found there, or at
    /// an alias of a stored header there, every one of which lies within the
    /// byte before the length it reads
    /// ([`stored_length_at`](crate::find::stored_length_at)).
    fn guess_at(&self, point: Point) -> Option<(u64, &Guess)> {
        let bit = point.bit();
        self.guesses
            .range(bit.saturating_sub(8)..=bit)
            .rev()
            .map(|(_, (chunk, guess))| (*chunk, guess))
            .find(|(_, guess)| guess.begins_at(point))
    }

    /// How many times the chain was anchored or dropped.
    #[cfg(test)]
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }
}

/// Where the chain stands after `guess`, begun with `window`: its stop and
/// the history there, `None` past the file's end or where a marker reaches
/// before the member's history.
fn resolved(guess: &Guess, window: &[u8]) -> Option<(Point, Vec<u8>)> {
    match guess.stop {
        Point::End => None,
        stop => guess.history_at_stop(window).map(|history| (stop, history)),
    }
}

/// Whether a decode at `bit` is past every place `guess` begins.
fn behind(guess: &Guess, bit: u64) -> bool {
    match guess.stored {
        Some(len) => bit + 3 > len,
        None => guess.start.bit() < bit,
    }
}
