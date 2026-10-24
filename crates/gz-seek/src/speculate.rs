//! A speculation: one chunk of a gzip file decoded from a place found in it,
//! before anything ahead of it is decoded.
//!
//! The file is cut into chunks of a fixed compressed length, chunk `k`
//! covering the bits from `k` chunks in to `k + 1`. A speculation searches its
//! chunk for the first place a decode may resume ([`find`](crate::find)), and
//! decodes from there — markers for the unknown history, as the
//! [`native`](crate::native) decoder writes them — until the first block
//! boundary or member start at or past the chunk's end, recording every event
//! an index is made of on the way. So where it stops is where the next
//! chunk's search begins, and a pass that reaches a chunk's start exactly —
//! by its own decode or another chunk's — takes the chunk's plaintext as
//! decoded.
//!
//! **A speculation never fails.** A candidate whose decode is refused was not
//! a block's start, and the search goes on past it — up to a few refused only
//! after passing a boundary, past which the bytes are more likely damaged than
//! the guess wrong; then a refusal, or plaintext that would pass the limit,
//! ends the chunk at the last boundary it reached, the rest left to the pass's
//! own decode, which meets whatever the file holds there and refuses it as the
//! engine does. A chunk in which nothing is found hands back nothing.
//!
//! **Nothing here reads a file**: the caller feeds the bytes from
//! [`Speculation::wants`] on, as many at a time as it has, on whatever thread.

use crate::find::{Candidate, Found, find, stored_length_at};
use crate::member::{Header, Trailer};
use crate::native::{Inflater, Input, Marked, Plain, Sink, Stop};
use crate::zlib;

/// A place a decode stands between two steps: where a chunk begins or ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Point {
    /// A block that is not its member's last begins at this bit.
    Block(u64),
    /// A member's header begins at this bit.
    Member(u64),
    /// The file's end, after the last member's trailer.
    End,
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
}

impl Event {
    /// The place a decode may resume at that this event stands for.
    fn point(&self) -> Option<Point> {
        match *self {
            Event::Block { bit } => Some(Point::Block(bit)),
            Event::MemberStart { offset } => Some(Point::Member(8 * offset)),
            _ => None,
        }
    }
}

/// A chunk decoded: everything a pass standing at its start takes.
pub(crate) struct Chunk {
    pub(crate) start: Point,
    /// Where the length of the stored block the chunk begins with is, for a
    /// chunk that begins with one.
    pub(crate) stored: Option<u64>,
    pub(crate) stop: Point,
    /// The plaintext written before the history stopped mattering: markers
    /// among it, to be resolved against the history at the start.
    pub(crate) marked: Vec<u16>,
    /// The plaintext after it, its first `history` bytes the 32 KiB before
    /// it rather than plaintext of the chunk's.
    pub(crate) plain: Vec<u8>,
    pub(crate) history: usize,
    /// Every event after the start, the stop's included, by plaintext offset
    /// from the start.
    pub(crate) events: Vec<(u64, Event)>,
    /// The CRC-32 of each piece of the bytes part of the plaintext, cut at
    /// every event inside it: its pieces in order.
    pub(crate) plain_crcs: Vec<u32>,
}

impl Chunk {
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

    /// The chunk's plaintext length.
    pub(crate) fn len(&self) -> u64 {
        (self.marked.len() + self.plain.len() - self.history) as u64
    }

    /// The offsets the plaintext is cut at for its CRC-32s inside the range
    /// from `from` to `to`, both ends included.
    pub(crate) fn cuts(&self, from: u64, to: u64) -> Vec<u64> {
        let mut cuts = vec![from];
        for &(at, _) in &self.events {
            if at > from && at < to && cuts.last() != Some(&at) {
                cuts.push(at);
            }
        }
        cuts.push(to);
        cuts
    }

    /// The bytes held, whatever their kind: what a caller's budget is billed.
    pub(crate) fn held(&self) -> usize {
        2 * self.marked.capacity() + self.plain.capacity()
    }
}

/// What a speculation came to: the chunk it decoded, if it found one.
pub struct Speculated {
    pub(crate) chunk: u64,
    pub(crate) found: Option<Chunk>,
}

impl Speculated {
    /// Which chunk of the file this is.
    pub fn chunk(&self) -> u64 {
        self.chunk
    }

    /// The bytes it holds until the pass takes or drops it.
    pub fn held(&self) -> usize {
        self.found.as_ref().map_or(0, Chunk::held)
    }
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
}

impl Decode {
    fn produced(&self) -> usize {
        self.marked.as_ref().map_or(0, |m| m.out.len()) + self.plain.produced()
    }

    /// Whether the decode is still writing markers.
    fn is_marked(&self) -> bool {
        self.marked.is_some() && !self.bytes
    }

    fn push(&mut self, event: Event) {
        let at = self.produced() as u64;
        if event.point().is_some() {
            self.last_point = Some(self.events.len());
        }
        self.events.push((at, event));
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
    /// Searching from this bit.
    Finding(u64),
    Decoding(Decode),
    Done(Option<Chunk>),
}

/// One chunk's speculation, fed by its caller: `Send`, holding no borrow, so
/// it runs on whatever thread the caller has.
///
/// It holds the file's bytes from the place its decode began on — about a
/// chunk of them, and the bytes of the block it stops in — and at most the
/// plaintext limit, twice over while it is still writing markers, beside a
/// few kilobytes of code tables.
pub struct Speculation {
    chunk: u64,
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
            until: 8 * (from + chunk_bytes),
            limit,
            input: Input::new(from, file_len),
            inflater: Inflater::new(),
            state: State::Finding(8 * from),
            refused_past_a_boundary: 0,
        }
    }

    /// Which chunk of the file this is.
    pub fn chunk(&self) -> u64 {
        self.chunk
    }

    /// The compressed offset of the next byte the speculation wants, `None`
    /// once it is done.
    pub fn wants(&self) -> Option<u64> {
        match self.state {
            State::Done(_) => None,
            _ => Some(self.input.end()),
        }
    }

    /// Take the file's bytes from [`Speculation::wants`] on, as many as the
    /// caller has, and decode as far as they go.
    pub fn supply(&mut self, bytes: &[u8]) {
        if self.wants().is_none() {
            return;
        }
        self.input.push(bytes);
        self.advance();
    }

    /// What the speculation came to; one not yet done hands back nothing.
    pub fn finish(self) -> Speculated {
        let found = match self.state {
            State::Done(found) => found,
            _ => None,
        };
        Speculated { chunk: self.chunk, found }
    }

    fn advance(&mut self) {
        loop {
            let state = std::mem::replace(&mut self.state, State::Done(None));
            self.state = match state {
                State::Done(found) => State::Done(found),
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
                    Ok(Some(stop)) => State::Done(Some(self.complete(decode, stop))),
                    Ok(None) => {
                        self.state = State::Decoding(decode);
                        return;
                    }
                    // A refused candidate was most likely no place a decode
                    // resumes, even one whose garbage passed a boundary; past
                    // a few of those the bytes are more likely damaged, and
                    // the chunk ends at the last boundary reached.
                    Err(Halt::Limit) => match decode.last_point {
                        Some(i) => {
                            let stop = decode.events[i].1.point().expect("a point");
                            State::Done(Some(self.truncate(decode, i, stop)))
                        }
                        None => State::Done(None),
                    },
                    Err(Halt::Refused) => match decode.last_point {
                        Some(i) if self.refused_past_a_boundary >= REFUSALS_PAST_A_BOUNDARY => {
                            let stop = decode.events[i].1.point().expect("a point");
                            State::Done(Some(self.truncate(decode, i, stop)))
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

    /// A decode from `candidate`.
    fn begin(&mut self, candidate: Candidate) -> Decode {
        let (start, stored, phase, bit, marked) = match candidate {
            Candidate::Block { bit, body, stored } => {
                (Point::Block(bit), stored, Phase::Deflate, body, Some(Marked::new()))
            }
            Candidate::Member { offset } => {
                let header = Phase::Header(Header::new(offset));
                (Point::Member(8 * offset), None, header, 8 * offset, None)
            }
        };
        let bytes = marked.is_none();
        let plain = Plain::new();
        let events = Vec::new();
        Decode { start, stored, phase, bit, marked, bytes, plain, events, last_point: None }
    }

    /// Decode as far as the input goes: `Some` with where the chunk stops once
    /// it does, `None` for more input, `Err` where it cannot go on.
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
                    let limit = self.limit;
                    let stop = if decode.is_marked() {
                        let sink = decode.marked.as_mut().expect("marked");
                        self.inflater.run(&self.input, &mut decode.bit, sink, limit)
                    } else {
                        let before = decode.marked.as_ref().map_or(0, |m| m.out.len());
                        let limit = limit.saturating_sub(before);
                        self.inflater.run(&self.input, &mut decode.bit, &mut decode.plain, limit)
                    }
                    .map_err(|_| Halt::Refused)?;
                    match stop {
                        Stop::Block { bit } => {
                            decode.push(Event::Block { bit });
                            if bit >= self.until {
                                return Ok(Some(Point::Block(bit)));
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

    /// The chunk the decode built, ending at `stop`.
    fn complete(&self, decode: Decode, stop: Point) -> Chunk {
        let Decode { start, stored, marked, plain, events, .. } = decode;
        let marked = marked.map_or_else(Vec::new, |m| m.out);
        let history = plain.history;
        let mut chunk = Chunk {
            start,
            stored,
            stop,
            marked,
            plain: plain.out,
            history,
            events,
            plain_crcs: Vec::new(),
        };
        let from = chunk.marked.len() as u64;
        let cuts = chunk.cuts(from, chunk.len());
        chunk.plain_crcs = cuts
            .windows(2)
            .map(|piece| {
                let at = |offset: u64| (offset - from) as usize + chunk.history;
                zlib::crc32(0, &chunk.plain[at(piece[0])..at(piece[1])])
            })
            .collect();
        chunk
    }

    /// The chunk the decode built, cut at its event `i`, which stands for
    /// `stop`.
    fn truncate(&self, mut decode: Decode, i: usize, stop: Point) -> Chunk {
        let at = decode.events[i].0 as usize;
        decode.events.truncate(i + 1);
        let marked = decode.marked.as_ref().map_or(0, |m| m.out.len());
        if at <= marked {
            if let Some(m) = decode.marked.as_mut() {
                m.out.truncate(at);
            }
            decode.plain = Plain::new();
        } else {
            let keep = decode.plain.history + at - marked;
            decode.plain.out.truncate(keep);
        }
        self.complete(decode, stop)
    }
}

#[cfg(test)]
mod tests;
