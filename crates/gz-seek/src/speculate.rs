//! A speculation: one chunk of a gzip file decoded from a place found in it,
//! before anything ahead of it is decoded, to guess where the chunk begins
//! and stops and what history it stops with — or, in a BGZF file, that guess
//! read off its members' headers and trailers with nothing decoded
//! ([`members`]).
//!
//! The file is cut into chunks of a fixed compressed length, chunk `k`
//! covering the bits from `k` chunks in to `k + 1`. A chunk holding a BGZF
//! header the bytes after its member bear out is read off its member chain
//! from there; any other speculation
//! searches its chunk for the first place a decode may resume
//! ([`find`](crate::find)), and decodes from there — markers for the unknown history, as the
//! [`native`](crate::native) decoder writes them — until the first member
//! start at or past the chunk's end, or block boundary there that the next
//! chunk's search would begin at, so that a guess stops where the next one
//! begins wherever both are right. What it hands back is
//! a [`Guess`]: that start and stop, the plaintext's length between them, and
//! the 32 KiB of history at the stop, still marked. Its plaintext is never
//! handed out: the parallel pass resolves the history once the history at the
//! start is known, and the engine decodes the chunk from it
//! (`docs/design/decisions.md`, "D117").
//!
//! **A limit cuts a guess, never the chain.** Plaintext that would pass the
//! limit ends the guess at the last boundary it reached, marked
//! [`cut`](Guess::cut), and the pass speculates on from that stop
//! ([`Speculation::continuing`]) — a start no search need find — to where the
//! chunk's guess would have stopped, so guesses still chain where each piece
//! reaches a boundary under the limit, which [`SPECULATION_LIMIT`] is chosen
//! for.
//!
//! **A speculation never fails.** A candidate whose decode is refused was not
//! a block's start, and the search goes on past it — up to a few refused only
//! after passing a boundary, past which the bytes are more likely damaged than
//! the guess wrong; then a refusal ends the chunk at the last boundary it
//! reached, the rest left to the pass's own decode, which meets whatever the
//! file holds there and refuses it as the engine does. A chunk in which
//! nothing is found, or a piece reaching no boundary under the limit, hands
//! back nothing.
//!
//! **Nothing here reads a file**: the caller feeds the bytes from
//! [`Speculation::wants`] on, as many at a time as it has, on whatever thread.

use crate::find::{Candidate, Found, find, is_searched_for, stored_length_at};
use crate::member::{Header, Trailer};
use crate::native::{Inflater, Input, MARKER, Marked, Plain, Sink, Stop};
use crate::zlib::WINDOW_SIZE;

use self::members::{Members, Read, Walked};

mod members;

/// The plaintext limit to pass [`Parallel`](crate::Parallel), the most a
/// speculation decodes and so a chunk's decode holds: twice the largest block
/// zlib ends, `lit_bufsize - 1` symbols of at most 258 bytes at
/// its largest `memLevel` (`deflate.c`), so a piece begun at a boundary
/// reaches the next under it, a limit cutting the guess there rather than
/// ending the chain of windows. A writer ending blocks more seldom gives a
/// piece nothing to stop at, and the frontier decodes those bytes itself.
pub const SPECULATION_LIMIT: usize = 2 * ((1 << (9 + 6)) - 1) * 258;

/// A place a decode stands between two steps: where a chunk begins or ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Point {
    /// A block begins at this bit, after one that was not its member's last.
    Block(u64),
    /// A member's header begins at this bit.
    Member(u64),
    /// The file's end, after the last member's trailer.
    End,
}

impl Point {
    /// The bit a decode standing here takes next, the end's being past every
    /// bit.
    pub(crate) fn bit(self) -> u64 {
        match self {
            Point::Block(bit) | Point::Member(bit) => bit,
            Point::End => u64::MAX,
        }
    }
}

/// An event inside a chunk, at a plaintext offset from the chunk's start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    /// A block that is not its member's last ended; the next begins at `bit`.
    Block { bit: u64 },
    /// A member's trailer was read: what it states, unchecked.
    MemberEnd { crc: u32, isize: u32 },
    /// A member's header begins at compressed `offset`.
    MemberStart { offset: u64 },
    /// The member's header has been read, and is a BGZF member's or not.
    Header { bgzf: bool },
    /// A static block's header, beginning at compressed `bit`, has been read:
    /// recorded by a chunk's decode, never a speculation's, for the pass to
    /// see where a checkpoint inside one may fall.
    Static { bit: u64 },
}

impl Event {
    /// The place a decode may resume at that this event stands for.
    pub(crate) fn point(&self) -> Option<Point> {
        match *self {
            Event::Block { bit } => Some(Point::Block(bit)),
            Event::MemberStart { offset } => Some(Point::Member(8 * offset)),
            _ => None,
        }
    }
}

/// A chunk speculated: where a decode of it begins and stops, and what it
/// would hand the next chunk.
pub(crate) struct Guess {
    pub(crate) start: Point,
    /// Where the length of the stored block the chunk begins with is, for a
    /// chunk that begins with one.
    pub(crate) stored: Option<u64>,
    pub(crate) stop: Point,
    /// The plaintext's length from the start to the stop.
    pub(crate) len: u64,
    /// The last 32 KiB the chunk wrote before its stop, no further back than
    /// its member's start: markers among it for bytes of the history before
    /// the chunk's start.
    pub(crate) end: Vec<u16>,
    /// Whether `end` is the whole history at the stop, its member beginning
    /// inside the chunk or at its start; otherwise the history before the
    /// start makes up the rest.
    pub(crate) fresh: bool,
    /// Whether the limit stopped it short of where its chunk's guess stops,
    /// so the pass speculates on from its stop.
    pub(crate) cut: bool,
}

impl Guess {
    /// Whether a decode standing at `point` stands where this chunk begins:
    /// at its start, or, for a chunk beginning with a stored block, at any
    /// header of that block's the search could have read
    /// ([`stored_length_at`]).
    pub(crate) fn begins_at(&self, point: Point) -> bool {
        match (self.start, self.stored, point) {
            (Point::Block(start), Some(len), Point::Block(bit)) => {
                bit >= start && stored_length_at(bit) == len
            }
            _ => self.start == point,
        }
    }

    /// The history at the stop, given `window`, the history at the start:
    /// `None` where a marker reaches before the member's history, which no
    /// decode of a real block does.
    pub(crate) fn history_at_stop(&self, window: &[u8]) -> Option<Vec<u8>> {
        let unknown = WINDOW_SIZE - window.len();
        let kept = if self.fresh { 0 } else { WINDOW_SIZE - self.end.len() };
        let mut out = Vec::with_capacity(kept.min(window.len()) + self.end.len());
        out.extend_from_slice(&window[window.len().saturating_sub(kept)..]);
        for &value in &self.end {
            out.push(match value.checked_sub(MARKER) {
                None => value as u8,
                Some(j) => *window.get(usize::from(j).checked_sub(unknown)?)?,
            });
        }
        Some(out)
    }

    /// The bytes held: what a caller's budget is billed.
    pub(crate) fn held(&self) -> usize {
        2 * self.end.capacity()
    }
}

/// What a speculation came to: its guess, if it found a start.
pub(crate) struct Speculated {
    pub(crate) chunk: u64,
    /// The bit a continuation began at.
    pub(crate) from: Option<u64>,
    pub(crate) found: Option<Guess>,
}

/// Where the decode is in the gzip framing.
enum Phase {
    Header(Header),
    Deflate,
    Trailer,
    Between,
}

/// The decode from one candidate.
struct Decode {
    start: Point,
    stored: Option<u64>,
    phase: Phase,
    /// The next bit the decode takes.
    bit: u64,
    /// What was written while the history mattered, where the chunk began
    /// inside a member.
    marked: Option<Marked>,
    /// Whether the decode writes bytes: from a member's start, or once the
    /// history stopped mattering.
    bytes: bool,
    plain: Plain,
    events: Vec<(u64, Event)>,
    /// The index in `events` of the last place a decode may resume at.
    last_point: Option<usize>,
    /// A boundary past the chunk's end, the next block's type not yet read.
    boundary: Option<u64>,
}

impl Decode {
    /// A decode from `start`, its next bit `bit`: past a block's header the
    /// search read into the inflater, or at the header itself — a member's,
    /// or a block's a continuation's fresh inflater reads.
    fn new(start: Point, stored: Option<u64>, bit: u64) -> Decode {
        let (phase, marked) = match start {
            Point::Member(_) => (Phase::Header(Header::new(bit / 8)), None),
            _ => (Phase::Deflate, Some(Marked::new())),
        };
        Decode {
            start,
            stored,
            phase,
            bit,
            bytes: marked.is_none(),
            marked,
            plain: Plain::new(),
            events: Vec::new(),
            last_point: None,
            boundary: None,
        }
    }

    fn produced(&self) -> usize {
        self.marked_len() + self.plain.produced()
    }

    /// How much was written marked, before the decode went on in bytes.
    fn marked_len(&self) -> usize {
        self.marked.as_ref().map_or(0, Sink::produced)
    }

    /// The value written at plaintext offset `i`, which must still be held: a
    /// byte, or a marker.
    fn value(&self, i: u64) -> u16 {
        let (i, marked) = (i as usize, self.marked_len());
        match &self.marked {
            Some(sink) if i < marked => sink.get(i),
            _ => u16::from(self.plain.get(i - marked)),
        }
    }

    /// Whether the decode is still writing markers.
    fn is_marked(&self) -> bool {
        self.marked.is_some() && !self.bytes
    }

    /// Record `event` here. A place a decode may resume at is the last a
    /// guess may stop at, which reads only the 32 KiB before it, so the sinks
    /// are released up to there.
    fn push(&mut self, event: Event) {
        let at = self.produced();
        if event.point().is_some() {
            self.last_point = Some(self.events.len());
            let keep = at.saturating_sub(WINDOW_SIZE);
            let marked = self.marked_len();
            if let Some(sink) = &mut self.marked {
                sink.release_before(keep);
            }
            self.plain.release_before(keep.saturating_sub(marked));
        }
        self.events.push((at as u64, event));
    }
}

/// Why a decode cannot go on.
enum Halt {
    /// The bytes are refused.
    Refused,
    /// The plaintext holds the limit.
    Limit,
}

enum State {
    /// Reading a BGZF member chain, the chunk searched as any other's where
    /// it holds none.
    Members(Members),
    /// Searching from this bit.
    Finding(u64),
    Decoding(Decode),
    Done(Option<Guess>),
}

/// One chunk's speculation, fed by its caller: `Send`, holding no borrow, so
/// it runs on whatever thread the caller has.
///
/// It holds the file's bytes from the place its decode began on — about a
/// chunk of them, and the bytes of the block or member it stops in — and of
/// its plaintext only what a guess may read, the 32 KiB before the last
/// place it may stop and all written since (`Decode::push`): a few windows
/// and a block, and so at most the plaintext limit, twice over while it is
/// still writing markers, beside a few kilobytes of code tables; reading a
/// BGZF chunk's members, the chunk's bytes, the member it stops in and the
/// header after that alone.
///
/// Deficiency register: `deficiency: KD143` — on `pg_dump`'s level-6 text a
/// speculation's decode rarely goes clean, so it writes nearly its whole
/// chunk marked, at more than the engine's cost of inflating the same bytes;
/// beside it, each chunk decode zeroes a buffer of the limit's size before the
/// engine writes into it (`chunk.rs`). At the four-worker baseline the pass is
/// no faster than serial: a decode the chain queues waits for a worker to end
/// a speculation, the frontier waiting for it in turn, and workers stand idle
/// while guesses fill the room `pgdt-io` holds the pass's results to; at the
/// physical core count the pass is bound by its workers, short of its scan's
/// floor. **(b) owned by 15.17.4**, the pass at four workers, **and 15.17.3**,
/// the zeroed buffer.
pub(crate) struct Speculation {
    chunk: u64,
    /// The bit a continuation of a cut guess begins at.
    from: Option<u64>,
    /// Where the chunk's search ends, and its decode may stop: the next
    /// chunk's first bit.
    until: u64,
    /// The most plaintext the chunk holds.
    limit: usize,
    input: Input,
    inflater: Box<Inflater>,
    state: State,
    /// How many candidates have been refused after passing a boundary.
    refused_past_a_boundary: u32,
}

/// The compressed bytes a speculation's input has room for before it grows:
/// its chunk, and as much again for the block it stops in, which a caller
/// feeding it a chunk at a time hands it whole.
fn input_room(chunk_bytes: u64) -> usize {
    usize::try_from(2 * chunk_bytes).unwrap_or(usize::MAX)
}

/// How many candidates may be refused after passing a boundary before the
/// next refusal ends the chunk instead: a bound on what damaged bytes cost.
const REFUSALS_PAST_A_BOUNDARY: u32 = 4;

impl Speculation {
    /// Chunk `chunk` of a file `file_len` bytes long cut into chunks of
    /// `chunk_bytes`, holding at most `limit` bytes of plaintext.
    pub(crate) fn new(chunk: u64, chunk_bytes: u64, limit: usize, file_len: u64) -> Speculation {
        let from = chunk * chunk_bytes;
        Speculation {
            chunk,
            from: None,
            until: 8 * (from + chunk_bytes),
            limit,
            input: Input::with_capacity(from, file_len, input_room(chunk_bytes)),
            inflater: Inflater::new(),
            state: State::Members(Members::new(from)),
            refused_past_a_boundary: 0,
        }
    }

    /// Chunk `chunk`'s speculation continued from `from`, the stop of a guess
    /// the limit cut, to where the chunk's guess would have stopped: a BGZF
    /// member there read off its chain, a block there decoded from with
    /// nothing searched for first, a plain member's header found by a search
    /// beginning at it.
    pub(crate) fn continuing(
        chunk: u64,
        from: Point,
        chunk_bytes: u64,
        limit: usize,
        file_len: u64,
    ) -> Speculation {
        let bit = match from {
            Point::Block(bit) | Point::Member(bit) => bit,
            Point::End => 8 * file_len,
        };
        let mut task = Speculation::new(chunk, chunk_bytes, limit, file_len);
        task.from = Some(bit);
        task.input = Input::with_capacity(bit / 8, file_len, input_room(chunk_bytes));
        task.state = match from {
            Point::Block(bit) => State::Decoding(Decode::new(Point::Block(bit), None, bit)),
            Point::Member(bit) => State::Members(Members::at(bit / 8)),
            Point::End => State::Done(None),
        };
        task
    }

    /// Which chunk of the file this is.
    pub(crate) fn chunk(&self) -> u64 {
        self.chunk
    }

    /// The bit a continuation begins at; `None` for a chunk's own.
    pub(crate) fn from(&self) -> Option<u64> {
        self.from
    }

    /// The compressed offset of the next byte the speculation wants, `None`
    /// once it is done.
    pub(crate) fn wants(&self) -> Option<u64> {
        match self.state {
            State::Done(_) => None,
            _ => Some(self.input.end()),
        }
    }

    /// Take the file's bytes from [`Speculation::wants`] on, as many as the
    /// caller has, and decode as far as they go.
    pub(crate) fn supply(&mut self, bytes: &[u8]) {
        if self.wants().is_none() {
            return;
        }
        self.input.push(bytes);
        self.advance();
    }

    /// What the speculation came to; one not yet done hands back nothing.
    pub(crate) fn finish(self) -> Speculated {
        let found = match self.state {
            State::Done(found) => found,
            _ => None,
        };
        Speculated { chunk: self.chunk, from: self.from, found }
    }

    fn advance(&mut self) {
        loop {
            let state = std::mem::replace(&mut self.state, State::Done(None));
            self.state = match state {
                State::Done(found) => State::Done(found),
                State::Members(mut members) => {
                    match members.read(&self.input, self.until, self.limit) {
                        Read::Input => {
                            self.state = State::Members(members);
                            return;
                        }
                        Read::Done(found) => State::Done(found.map(read_off)),
                        Read::Search => State::Finding(8 * members.from),
                    }
                }
                State::Finding(from) => {
                    match find(&self.input, from, self.until, &mut self.inflater) {
                        Found::At(candidate) => State::Decoding(self.begin(candidate)),
                        Found::Input(at) => {
                            self.input.release_before(at / 8);
                            self.state = State::Finding(at);
                            return;
                        }
                        Found::Nothing => State::Done(None),
                    }
                }
                State::Decoding(mut decode) => match self.step(&mut decode) {
                    Ok(Some(stop)) => {
                        let at = decode.produced() as u64;
                        State::Done(Some(guess(&decode, decode.events.len(), stop, at)))
                    }
                    Ok(None) => {
                        self.state = State::Decoding(decode);
                        return;
                    }
                    // A limit cuts the guess at the last boundary reached, the
                    // pass speculating on from there.
                    Err(Halt::Limit) => match decode.last_point {
                        Some(i) => State::Done(Some(truncated(&decode, i, true))),
                        None => State::Done(None),
                    },
                    // A refused candidate was most likely no place a decode
                    // resumes, even one whose garbage passed a boundary; past
                    // a few of those the bytes are more likely damaged, and
                    // the chunk ends at the last boundary reached.
                    Err(Halt::Refused) => match decode.last_point {
                        Some(i) if self.refused_past_a_boundary >= REFUSALS_PAST_A_BOUNDARY => {
                            State::Done(Some(truncated(&decode, i, false)))
                        }
                        last => {
                            self.refused_past_a_boundary += u32::from(last.is_some());
                            // Past every alias of a stored header, which all
                            // decode alike.
                            State::Finding(match (decode.start, decode.stored) {
                                (_, Some(len)) => len - 2,
                                (Point::Block(bit) | Point::Member(bit), None) => bit + 1,
                                (Point::End, None) => unreachable!("no decode begins at the end"),
                            })
                        }
                    },
                },
            };
            if let State::Done(_) = self.state {
                return;
            }
        }
    }

    /// A decode from `candidate`, whose block header the search has read.
    fn begin(&mut self, candidate: Candidate) -> Decode {
        match candidate {
            Candidate::Block { bit, body, stored } => Decode::new(Point::Block(bit), stored, body),
            Candidate::Member { offset } => {
                Decode::new(Point::Member(8 * offset), None, 8 * offset)
            }
        }
    }

    /// Decode as far as the input goes: `Some` with where the chunk stops once
    /// it does, `None` for more input, `Err` where it cannot go on.
    ///
    /// Deficiency register: `deficiency: KD139` — past the chunk's end a
    /// decoded guess stops at the first block boundary the next chunk's
    /// search would begin at, while the next chunk's guess begins at the
    /// first BGZF header it holds where it holds one: a plain member running
    /// past a chunk's end into BGZF members gives two guesses that never meet,
    /// the frontier decoding from the one's stop to the other's start itself.
    /// **(c) unowned**; promoted by a profile of a plain-then-BGZF file's pass
    /// showing that decode.
    fn step(&mut self, decode: &mut Decode) -> Result<Option<Point>, Halt> {
        let complete = self.input.is_complete();
        // A refusal searches on from just past the candidate, so the bytes
        // from it on are held for as long as the decode runs.
        if let Point::Block(bit) | Point::Member(bit) = decode.start {
            self.input.release_before(bit / 8);
        }
        loop {
            match &mut decode.phase {
                Phase::Deflate => {
                    // Past the chunk's end, the decode stops at the first
                    // boundary the next chunk's search would begin at.
                    if let Some(bit) = decode.boundary {
                        if self.input.end_bit() < bit + 3 {
                            return if complete { Err(Halt::Refused) } else { Ok(None) };
                        }
                        decode.boundary = None;
                        if is_searched_for(self.input.peek(bit)) {
                            return Ok(Some(Point::Block(bit)));
                        }
                    }
                    let limit = self.limit;
                    let stop = if decode.is_marked() {
                        let sink = decode.marked.as_mut().expect("marked");
                        self.inflater.run(&self.input, &mut decode.bit, sink, limit)
                    } else {
                        let before = decode.marked_len();
                        let limit = limit.saturating_sub(before);
                        self.inflater.run(&self.input, &mut decode.bit, &mut decode.plain, limit)
                    }
                    .map_err(|_| Halt::Refused)?;
                    match stop {
                        Stop::Block { bit } => {
                            decode.push(Event::Block { bit });
                            if bit >= self.until {
                                decode.boundary = Some(bit);
                            }
                        }
                        Stop::End { bit } => {
                            decode.bit = bit.div_ceil(8) * 8;
                            decode.phase = Phase::Trailer;
                        }
                        Stop::Input if complete => return Err(Halt::Refused),
                        Stop::Input => return Ok(None),
                        Stop::Limit => return Err(Halt::Limit),
                        Stop::Clean => {
                            let window = decode.marked.as_ref().expect("marked").window();
                            decode.plain = Plain::after(window);
                            decode.bytes = true;
                        }
                    }
                }
                Phase::Trailer => {
                    let held = self.input.from(decode.bit / 8);
                    let Some(bytes) = held.get(..Trailer::LEN) else {
                        return if complete { Err(Halt::Refused) } else { Ok(None) };
                    };
                    let trailer = Trailer::parse(bytes.try_into().expect("a trailer's bytes"));
                    decode.push(Event::MemberEnd { crc: trailer.crc, isize: trailer.isize });
                    decode.bit += 8 * Trailer::LEN as u64;
                    decode.phase = Phase::Between;
                }
                Phase::Between => {
                    let offset = decode.bit / 8;
                    if complete && offset == self.input.end() {
                        return Ok(Some(Point::End));
                    }
                    if offset == self.input.end() {
                        return Ok(None);
                    }
                    decode.push(Event::MemberStart { offset });
                    if decode.bit >= self.until {
                        return Ok(Some(Point::Member(decode.bit)));
                    }
                    // A member has no history before it: bytes from here.
                    if decode.is_marked() {
                        decode.bytes = true;
                    } else {
                        decode.plain.member_start();
                    }
                    decode.phase = Phase::Header(Header::new(offset));
                }
                Phase::Header(header) => {
                    let taken =
                        header.read(self.input.from(decode.bit / 8)).map_err(|_| Halt::Refused)?;
                    decode.bit += 8 * taken as u64;
                    if !header.is_done() {
                        return if complete { Err(Halt::Refused) } else { Ok(None) };
                    }
                    let bgzf = crate::bgzf::bgzf_member_len(header.bytes()).is_some();
                    decode.push(Event::Header { bgzf });
                    self.inflater.reset();
                    decode.phase = Phase::Deflate;
                }
            }
        }
    }
}

/// The guess a BGZF chunk's chain is read as: from its first member to its
/// stop, with no history.
fn read_off(walked: Walked) -> Guess {
    Guess {
        start: Point::Member(8 * walked.first),
        stored: None,
        stop: walked.stop.map_or(Point::End, |at| Point::Member(8 * at)),
        len: walked.plaintext,
        end: Vec::new(),
        fresh: true,
        cut: walked.cut,
    }
}

/// The guess the decode makes, cut at its event `i`, the last place it
/// reached that a decode may resume at — by the limit where `cut`.
fn truncated(decode: &Decode, i: usize, cut: bool) -> Guess {
    let (at, event) = decode.events[i];
    Guess { cut, ..guess(decode, i + 1, event.point().expect("a point"), at) }
}

/// The guess a decode makes standing at `stop`, plaintext offset `at`, its
/// first `events` events behind it.
fn guess(decode: &Decode, events: usize, stop: Point, at: u64) -> Guess {
    let floor = decode.events[..events]
        .iter()
        .rev()
        .find(|(_, e)| matches!(e, Event::MemberStart { .. }))
        .map(|&(p, _)| p)
        .or(matches!(decode.start, Point::Member(_)).then_some(0));
    let end = match stop {
        Point::Block(_) => {
            let from = at.saturating_sub(WINDOW_SIZE as u64).max(floor.unwrap_or(0));
            (from..at).map(|i| decode.value(i)).collect()
        }
        // Nothing of the history reaches past a member's start.
        Point::Member(_) | Point::End => Vec::new(),
    };
    let fresh = floor.is_some() || !matches!(stop, Point::Block(_));
    Guess { start: decode.start, stored: decode.stored, stop, len: at, end, fresh, cut: false }
}

#[cfg(test)]
mod tests;
