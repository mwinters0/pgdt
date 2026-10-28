//! A chunk decoded by the engine: from where a speculation guessed it begins,
//! with the history the parallel pass resolved for it there, to the first
//! block boundary or member start at or past where the speculation stopped.
//!
//! **This is the plaintext a taken chunk hands out**
//! (`docs/design/decisions.md`, "D117"): the decode a unit is resumed with
//! (D112), begun from a window the pass supplies rather than one an index
//! recorded, and run on whatever thread the caller has. It records the events
//! an index is made of — each static block's start among them, where a
//! checkpoint the pass owes may fall inside it, and of a BGZF member, where
//! none is owed but at its start, only its start, header and end (D123) — and
//! each piece's CRC-32, and
//! reads each trailer without comparing it, the member's plaintext before the
//! chunk being the pass's to know. Whether its start and window were right is settled where the pass
//! takes it: only standing at its start, with the very history it began with,
//! so by the engine's own decode of everything before it.
//!
//! **It never fails.** Bytes the engine refuses, or plaintext past what the
//! speculation found there, hand back nothing, and the pass's own decode
//! meets those bytes and refuses them as the serial pass does.

use crate::engine::{Engine, Event as EngineEvent, Member};
use crate::speculate::{EVENTS_A_GUESS, Event, Point, STATE_BYTES};
use crate::zlib::{DECODER_BYTES, WINDOW_SIZE};

/// The most events one guess spans: [`EVENTS_A_GUESS`], and the trailer, start
/// and header a speculation passes between two of its checks of the count.
const EVENTS_A_GUESSED: usize = EVENTS_A_GUESS + 3;

/// The most pieces a chunk's decode records: one at each block's or member's
/// end its guess spans.
pub(crate) const PIECES_A_DECODE: usize = EVENTS_A_GUESSED;

/// The most events a chunk's decode records: those its guess spans, a static
/// block's start beside each block's end, and its stop. Inside a BGZF member
/// it records three a member, which a chain read counts
/// (`docs/design/decisions.md`, "D123").
pub(crate) const EVENTS_A_DECODE: usize = 2 * EVENTS_A_GUESSED + 1;

const _: () = assert!(
    std::mem::size_of::<ChunkDecode>() + std::mem::size_of::<Running>() <= STATE_BYTES as usize
);

/// The most bytes one chunk's decode holds written, under `limit`, handed out
/// or done: its window, its plaintext and the byte past it, its events and
/// pieces, and its engine while it runs ([`DECODER_BYTES`]). Bytes a buffer
/// has room for and never wrote are not counted (`docs/design/decisions.md`,
/// "D124").
pub(crate) const fn most_held(limit: usize) -> u64 {
    let events = EVENTS_A_DECODE * std::mem::size_of::<(u64, Event)>();
    let pieces = PIECES_A_DECODE * std::mem::size_of::<(u64, u64, u32)>();
    let buffers = WINDOW_SIZE + limit + 1 + events + pieces;
    buffers as u64 + DECODER_BYTES + STATE_BYTES
}

/// A chunk's plaintext from its start, as the engine decoded it.
pub(crate) struct Decoded {
    pub(crate) stop: Point,
    pub(crate) plain: Vec<u8>,
    /// Every event after the start, the stop's included, by plaintext offset
    /// from the start.
    pub(crate) events: Vec<(u64, Event)>,
    /// Each piece's plaintext range and CRC-32, in order, cut at every block's
    /// end it records and member's end.
    pub(crate) pieces: Vec<(u64, u64, u32)>,
}

impl Decoded {
    /// The chunk's plaintext length.
    pub(crate) fn len(&self) -> u64 {
        self.plain.len() as u64
    }

    fn held(&self) -> usize {
        self.plain.capacity()
            + std::mem::size_of::<(u64, Event)>() * self.events.capacity()
            + std::mem::size_of::<(u64, u64, u32)>() * self.pieces.capacity()
    }
}

/// What a chunk's decode came to.
pub(crate) struct ChunkDecoded {
    pub(crate) chunk: u64,
    pub(crate) generation: u64,
    pub(crate) start: Point,
    /// The history it began with, which the pass compares with its own.
    pub(crate) window: Vec<u8>,
    pub(crate) found: Option<Decoded>,
    /// The plaintext buffer it was lent, where it found nothing to hold in it.
    pub(crate) spare: Vec<u8>,
}

impl ChunkDecoded {
    pub(crate) fn held(&self) -> usize {
        self.window.capacity()
            + self.spare.capacity()
            + self.found.as_ref().map_or(0, Decoded::held)
    }

    /// The plaintext buffer it holds, for the pass to lend the next decode.
    pub(crate) fn into_buffer(self) -> Vec<u8> {
        self.found.map_or(self.spare, |found| found.plain)
    }
}

/// One chunk's decode through the engine, fed by its caller: `Send`, holding
/// no borrow.
///
/// It holds its window, and once fed an engine and room for the plaintext
/// the speculation found from its start to its stop, and a byte to show a
/// decode running past it, which nothing zeroes: the engine writes it, in the
/// buffer the pass lent it at dispatch where it did ([`ChunkDecode::lend`]);
/// and
/// its events and pieces, at most twice the events its guess counted against
/// `EVENTS_A_GUESS`, a BGZF member's being its start, header and end alone
/// (`docs/design/decisions.md`, "D123").
pub(crate) struct ChunkDecode {
    chunk: u64,
    generation: u64,
    start: Point,
    window: Vec<u8>,
    /// Where the speculation stopped: the decode stops at the first boundary
    /// or member start at or past this bit, or at the file's end.
    until: u64,
    file_len: u64,
    /// The most plaintext the decode may write before it stops.
    limit: usize,
    /// The plaintext buffer lent it, until it begins, or handed back by a
    /// decode the engine refused.
    buffer: Vec<u8>,
    state: State,
}

enum State {
    /// Not yet fed: no engine made.
    Ready,
    Running(Box<Running>),
    Done(Option<Decoded>),
}

struct Running {
    engine: Engine,
    /// The plaintext so far, with capacity for one byte past the limit, so
    /// that running past it shows.
    out: Vec<u8>,
    events: Vec<(u64, Event)>,
    pieces: Vec<(u64, u64, u32)>,
    /// Where the piece being decoded began.
    piece_from: u64,
    /// The member start the decode begins at, which the pass has passed.
    skip_start: bool,
    /// Whether the member being decoded has a BGZF member's header: not known
    /// of one the decode begins inside, which a decoded guess bounds.
    bgzf: bool,
}

impl ChunkDecode {
    /// Chunk `chunk` decoded from `start`, with `window` the history before it,
    /// to the first boundary or member start at or past `until`, writing at
    /// most `limit` bytes of plaintext; `generation` is the pass's, for the
    /// result to be recognised by.
    pub(crate) fn new(
        (chunk, generation): (u64, u64),
        start: Point,
        window: Vec<u8>,
        until: Point,
        limit: u64,
        file_len: u64,
    ) -> ChunkDecode {
        let limit = usize::try_from(limit).unwrap_or(usize::MAX - 1);
        let until = match until {
            Point::Block(bit) | Point::Member(bit) => bit,
            Point::End => u64::MAX,
        };
        ChunkDecode {
            chunk,
            generation,
            start,
            window,
            until,
            file_len,
            limit,
            buffer: Vec::new(),
            state: State::Ready,
        }
    }

    /// Write the plaintext into `buffer` rather than one made for this decode:
    /// the pass's, kept from a decode it has done with, so no buffer is freed
    /// while the pass runs (`docs/design/decisions.md`, "D124").
    pub(crate) fn lend(&mut self, buffer: Vec<u8>) {
        self.buffer = buffer;
    }

    pub(crate) fn chunk(&self) -> u64 {
        self.chunk
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Where the decode begins.
    pub(crate) fn start(&self) -> Point {
        self.start
    }

    /// The bytes it holds before it is fed: its window, and the buffer lent
    /// it.
    pub(crate) fn held(&self) -> usize {
        self.window.capacity() + self.buffer.capacity()
    }

    /// The compressed offset of the next byte the decode wants, `None` once it
    /// is done.
    pub(crate) fn wants(&self) -> Option<u64> {
        match &self.state {
            State::Ready => Some(match self.start {
                Point::Block(bit) | Point::Member(bit) => bit / 8,
                Point::End => self.file_len,
            }),
            State::Running(running) => Some(running.engine.compressed_position()),
            State::Done(_) => None,
        }
    }

    /// Take the file's bytes from [`ChunkDecode::wants`] on, as many as the
    /// caller has, and decode as far as they go.
    pub(crate) fn supply(&mut self, bytes: &[u8]) {
        if let State::Ready = self.state {
            self.state = match self.begin() {
                Some(running) => State::Running(Box::new(running)),
                None => State::Done(None),
            };
        }
        let State::Running(running) = &mut self.state else { return };
        match running.run(bytes, self.until, self.file_len, self.limit) {
            Ok(None) => {}
            Ok(Some(stop)) => {
                let State::Running(running) = std::mem::replace(&mut self.state, State::Ready)
                else {
                    unreachable!("running")
                };
                let Running { out, events, pieces, .. } = *running;
                self.state = State::Done(Some(Decoded { stop, plain: out, events, pieces }));
            }
            Err(Refused) => {
                if let State::Running(running) =
                    std::mem::replace(&mut self.state, State::Done(None))
                {
                    self.buffer = running.out;
                }
            }
        }
    }

    /// What the decode came to; one not yet done hands back nothing.
    pub(crate) fn finish(self) -> ChunkDecoded {
        let ChunkDecode { chunk, generation, start, window, buffer, state, .. } = self;
        let (found, spare) = match state {
            State::Done(found) => (found, buffer),
            State::Running(running) => (None, running.out),
            State::Ready => (None, buffer),
        };
        ChunkDecoded { chunk, generation, start, window, found, spare }
    }

    fn begin(&mut self) -> Option<Running> {
        let engine = match self.start {
            Point::Block(bit) => {
                Engine::at_block(bit, 0, &self.window, Member::resumed(0, 0, None, 0)).ok()?
            }
            Point::Member(bit) => Engine::at_member(bit / 8, 0).ok()?,
            Point::End => return None,
        };
        // A lent buffer already has the room; one not lent is made for it.
        let mut out = std::mem::take(&mut self.buffer);
        out.clear();
        out.reserve_exact(self.limit + 1);
        Some(Running {
            engine: engine.unchecked(),
            out,
            events: Vec::new(),
            pieces: Vec::new(),
            piece_from: 0,
            skip_start: matches!(self.start, Point::Member(_)),
            bgzf: false,
        })
    }
}

/// The engine refused the bytes, or the decode ran past its limit.
struct Refused;

impl Running {
    /// Decode `input`, the bytes from the engine's position on: `Some` with
    /// the stop once the decode reaches it, `None` for more input.
    fn run(
        &mut self,
        input: &[u8],
        until: u64,
        file_len: u64,
        limit: usize,
    ) -> Result<Option<Point>, Refused> {
        let mut taken = 0;
        loop {
            let step =
                self.engine.step_appending(&input[taken..], &mut self.out).map_err(|_| Refused)?;
            taken += step.consumed;
            if self.out.len() > limit {
                return Err(Refused);
            }
            if let Some(stop) = step.event.and_then(|event| self.event(event, until)) {
                return Ok(Some(stop));
            }
            let at = self.engine.compressed_position();
            if self.engine.is_between() && at >= file_len {
                return Ok(Some(Point::End));
            }
            if !step.progressed() {
                // Short of the stop at the file's end: the file is truncated
                // there, which the pass's own decode reports.
                return if at >= file_len { Err(Refused) } else { Ok(None) };
            }
        }
    }

    /// Record `event`, answering the stop where it is one. Inside a BGZF
    /// member, a block's end is recorded only as the stop, and a static
    /// block's start not at all.
    fn event(&mut self, event: EngineEvent, until: u64) -> Option<Point> {
        let at = self.out.len() as u64;
        match event {
            EngineEvent::MemberStart if self.skip_start => {
                self.skip_start = false;
                None
            }
            EngineEvent::MemberStart => {
                self.bgzf = false;
                let offset = self.engine.member().offset;
                self.events.push((at, Event::MemberStart { offset }));
                (8 * offset >= until).then_some(Point::Member(8 * offset))
            }
            EngineEvent::Header { bgzf } => {
                self.bgzf = bgzf;
                self.events.push((at, Event::Header { bgzf }));
                None
            }
            EngineEvent::Block { bit } if self.bgzf && bit < until => None,
            EngineEvent::Block { bit } => {
                self.cut();
                self.events.push((at, Event::Block { bit }));
                (bit >= until).then_some(Point::Block(bit))
            }
            EngineEvent::Static { .. } if self.bgzf => None,
            EngineEvent::Static { bit } => {
                self.events.push((at, Event::Static { bit }));
                None
            }
            // No mark is set, so no static block is walked.
            EngineEvent::Symbol { .. } => None,
            EngineEvent::MemberEnd => {
                self.cut();
                let trailer = self.engine.last_trailer().expect("a member's end reads its trailer");
                self.events.push((at, Event::MemberEnd { crc: trailer.crc, isize: trailer.isize }));
                None
            }
        }
    }

    /// End the piece being decoded here.
    fn cut(&mut self) {
        let crc = self.engine.cut();
        let to = self.out.len() as u64;
        if to > self.piece_from {
            self.pieces.push((self.piece_from, to, crc));
            self.piece_from = to;
        }
    }
}

#[cfg(test)]
mod tests;
