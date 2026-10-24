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
//! nothing but the engine's decode of everything before it gives it. **A guess
//! stopping short of where the next begins ends the chain**, a limit or a
//! refusal having cut it: the frontier decodes on to the next guess, anchors
//! there, and decodes that chunk itself.
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
    chunk_bytes: u64,
    file_len: u64,
    generation: u64,
    /// What each speculation guessed, by chunk, until the frontier passes it.
    guesses: BTreeMap<u64, Guess>,
    /// Where the last chunk given a decode stops, and the history there as
    /// resolved: `None` where no more can be resolved until the frontier
    /// anchors again.
    end: Option<(Point, Vec<u8>)>,
    queued: VecDeque<ChunkDecode>,
    /// Each of this generation's decodes handed out, by chunk, with where it
    /// begins.
    running: BTreeMap<u64, Point>,
    decoded: BTreeMap<u64, ChunkDecoded>,
}

impl Chain {
    pub(super) fn new(chunk_bytes: u64, file_len: u64) -> Chain {
        Chain {
            chunk_bytes,
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
            self.guesses.insert(chunk, guess);
            self.extend();
        }
    }

    /// A decode's result, with the frontier at `bit`.
    pub(super) fn decoded(&mut self, decoded: ChunkDecoded, bit: u64) {
        let current =
            decoded.generation == self.generation && self.running.remove(&decoded.chunk).is_some();
        if current && start_bit(decoded.start) >= bit {
            self.decoded.insert(decoded.chunk, decoded);
        }
    }

    /// The next decode to hand out, in chunk order.
    pub(super) fn next_task(&mut self) -> Option<ChunkDecode> {
        let task = self.queued.pop_front()?;
        self.running.insert(task.chunk(), task.start());
        Some(task)
    }

    /// Whether a decode handed out is still wanted.
    pub(super) fn wants(&self, chunk: u64, generation: u64) -> bool {
        generation == self.generation && self.running.contains_key(&chunk)
    }

    /// Drop what the frontier, standing at `bit`, has passed.
    pub(super) fn pass(&mut self, bit: u64) {
        self.guesses.retain(|_, guess| !behind(guess, bit));
        self.decoded.retain(|_, decoded| start_bit(decoded.start) >= bit);
        self.queued.retain(|task| start_bit(task.start()) >= bit);
        if self.end.as_ref().is_some_and(|(point, _)| start_bit(*point) < bit) {
            self.end = None;
        }
    }

    /// What begins at `point`, where the frontier stands with `history`
    /// before it: a guess beginning there and nothing else anchors the chain.
    pub(super) fn at(&mut self, point: Point, history: impl FnOnce() -> Vec<u8>) -> Here {
        if let Some(chunk) = self.decoded.iter().find(|(_, d)| d.start == point).map(|(&k, _)| k) {
            return Here::Decoded(self.decoded.remove(&chunk).expect("a decoded chunk"));
        }
        if let Some((&chunk, _)) = self.running.iter().find(|&(_, &start)| start == point) {
            return Here::Running(TaskId::decode(chunk, self.generation));
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

    /// The bytes the chain holds: the guesses, the decodes done, and the
    /// windows of those queued and of its end.
    pub(super) fn held(&self) -> usize {
        self.guesses.values().map(Guess::held).sum::<usize>()
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

    /// The guess beginning at `point`, and its chunk.
    fn guess_at(&self, point: Point) -> Option<(u64, &Guess)> {
        let chunk = start_bit(point).checked_div(8 * self.chunk_bytes)?;
        [chunk.checked_sub(1), Some(chunk)]
            .into_iter()
            .flatten()
            .filter_map(|k| self.guesses.get(&k).map(|guess| (k, guess)))
            .find(|(_, guess)| guess.begins_at(point))
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

/// The bit a decode standing at `point` takes next, the end's being past
/// every bit.
pub(super) fn start_bit(point: Point) -> u64 {
    match point {
        Point::Block(bit) | Point::Member(bit) => bit,
        Point::End => u64::MAX,
    }
}

/// Whether a decode at `bit` is past every place `guess` begins.
fn behind(guess: &Guess, bit: u64) -> bool {
    match guess.stored {
        Some(len) => bit + 3 > len,
        None => start_bit(guess.start) < bit,
    }
}
