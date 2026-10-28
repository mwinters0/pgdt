//! The decode both the forward pass and a unit decode are: a run of gzip
//! members — header, raw deflate, trailer — fed by the caller a slice at a
//! time, stopping at each event an index is made of.
//!
//! The engine reads nothing itself. Each [`Engine::step`] takes the next bytes
//! of the file, starting at [`Engine::compressed_position`], and space for
//! plaintext, and answers what it consumed and produced and at most one
//! [`Event`]. A step that reaches an event returns at it, so an event's
//! position is the engine's own and never depends on how the caller cut its
//! buffers.
//!
//! **A static block is walked beside the inflater** where the caller has set
//! a mark ([`Engine::set_mark`]): a static block's codes are deflate's own, so
//! a decode can resume before any of its symbols, and the engine walks them
//! (`native::Walk`), capping the inflater's output at the first symbol at or
//! past the mark and stopping there with [`Event::Symbol`]. Every byte is
//! still the inflater's; the walk only says where to stop it
//! (`docs/design/decisions.md`, "D118").
//!
//! **The inflater stops at a deflate stream's end having taken no byte of the
//! trailer**: its reader holds no unused bit where a stream ends, which the
//! engine asserts, and the stream ends on a byte boundary, so the next byte is
//! the trailer's (`docs/design/runtime-invariants.md`,
//! "RT23").

use crate::bgzf::bgzf_member_len;
use crate::error::{Error, Result};
use crate::member::{Header, Trailer};
use crate::native::{Walk, Walked};
use crate::zlib::{Out, Raw, WINDOW_SIZE};

/// What a step stopped at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    /// A member's header begins at the offset [`Engine::member`] carries.
    MemberStart,
    /// The member's header has been read, and is a BGZF member's or not.
    Header { bgzf: bool },
    /// A deflate block that is not its member's last has ended; the next one
    /// begins at compressed bit `bit`.
    Block { bit: u64 },
    /// A static block's header begins at compressed bit `bit`.
    Static { bit: u64 },
    /// The next symbol of a static block begins at compressed bit `bit`, and
    /// is the first at or past the mark that is not the block's end; `last`
    /// says whether the block is its member's final one.
    Symbol { bit: u64, last: bool },
    /// A member's trailer was read and matched: its `ISIZE` always, its CRC-32
    /// where the decode knows the CRC of the member's plaintext before it began
    /// — or, by an [`Engine::unchecked`] engine, only read.
    MemberEnd,
}

/// What one step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Stepped {
    pub(crate) consumed: usize,
    pub(crate) produced: usize,
    pub(crate) event: Option<Event>,
}

impl Stepped {
    pub(crate) fn progressed(&self) -> bool {
        self.consumed > 0 || self.produced > 0 || self.event.is_some()
    }
}

/// Where the framing is.
enum Phase {
    /// Before a member's first byte: after a trailer, or at the start.
    Between,
    Header(Header),
    Deflate,
    Trailer {
        bytes: [u8; Trailer::LEN],
        have: usize,
    },
}

/// The member being decoded, and the CRC-32 of its plaintext so far — kept as
/// the CRC of a prefix ending where the current run began, and the run's own,
/// so that cutting a unit costs a combine rather than a second pass.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Member {
    /// Compressed offset of the member's header.
    pub(crate) offset: u64,
    /// Plaintext offset the member begins at.
    pub(crate) plaintext: u64,
    /// CRC-32 of the member's plaintext before `run_start`, where known: a
    /// decode resumed inside a member without its earlier units' values does
    /// not know it, and compares no trailer CRC.
    pub(crate) prefix: Option<u32>,
    run_start: u64,
    run_crc: u32,
}

impl Member {
    /// A member whose plaintext before `run_start` has CRC `prefix`.
    pub(crate) fn resumed(
        offset: u64,
        plaintext: u64,
        prefix: Option<u32>,
        run_start: u64,
    ) -> Member {
        Member { offset, plaintext, prefix, run_start, run_crc: 0 }
    }

    fn fresh(offset: u64, plaintext: u64) -> Member {
        Member::resumed(offset, plaintext, Some(0), plaintext)
    }
}

/// The most compressed bytes a static block's walk is handed at once: what it
/// holds besides the symbol it stands at.
const WALK_PUSH: usize = 4 * 1024;

/// What a resume primes the inflater with ahead of its first input.
#[derive(Debug, Clone, Copy)]
struct Prime {
    /// A static block's three header bits, for a resume inside one.
    header: Option<u8>,
    /// The bits of the first input byte the resume point is past.
    skip: u8,
}

/// What the engine reads of the stream itself before the inflater goes on.
#[derive(Debug, Clone, Copy)]
enum Pending {
    /// A block's header begins at this bit: its type is read, a static
    /// block's reported.
    Header(u64),
    /// A static block's first symbol begins at `bit`: walked where a mark is
    /// set.
    Body { bit: u64, last: bool },
}

/// A static block being walked: where its next symbol is, and where the walk
/// stopped short of the mark.
struct Walking {
    walk: Box<Walk>,
    /// The block is its member's final one.
    last: bool,
    /// The symbol at or past the mark, by bit and plaintext offset, once the
    /// walk has reached it.
    found: Option<(u64, u64)>,
}

pub(crate) struct Engine {
    raw: Raw,
    phase: Phase,
    /// Compressed offset of the next byte the caller supplies.
    fed: u64,
    /// What the inflater is primed with before its first input.
    prime: Option<Prime>,
    /// The last eight bytes the inflater took, newest in the low byte, and
    /// how many it has taken, up to eight: the bytes a stop's unused bits are
    /// from.
    tail: u64,
    tail_len: u8,
    /// What is to be read of the stream before the inflater goes on.
    pending: Option<Pending>,
    /// The plaintext offset a static block's walk stops at the first symbol
    /// at or past.
    mark: Option<u64>,
    walking: Option<Walking>,
    plaintext: u64,
    member: Member,
    /// The last block boundary reported, so none is reported twice.
    last_boundary: Option<u64>,
    /// Whether a trailer is compared, or only read for whoever knows what
    /// the member held before the decode began.
    checks_trailers: bool,
    /// The last trailer read.
    trailer: Option<Trailer>,
}

impl Engine {
    /// An engine whose next byte begins a member, at compressed `offset` and
    /// plaintext `plaintext`.
    pub(crate) fn at_member(offset: u64, plaintext: u64) -> Result<Engine> {
        Ok(Engine {
            raw: Raw::new()?,
            phase: Phase::Between,
            fed: offset,
            prime: None,
            tail: 0,
            tail_len: 0,
            pending: None,
            mark: None,
            walking: None,
            plaintext,
            member: Member::fresh(offset, plaintext),
            last_boundary: None,
            checks_trailers: true,
            trailer: None,
        })
    }

    /// An engine resumed inside `member`'s deflate data at compressed `bit`
    /// and `plaintext`, the history before it `window`.
    pub(crate) fn at_block(
        bit: u64,
        plaintext: u64,
        window: &[u8],
        member: Member,
    ) -> Result<Engine> {
        Engine::resumed(bit, None, plaintext, window, member)
    }

    /// An engine resumed inside `member`'s static block at compressed `bit`,
    /// before a symbol, and `plaintext`, the history before it `window`;
    /// `last` says whether the block is the member's final one.
    pub(crate) fn at_symbol(
        bit: u64,
        last: bool,
        plaintext: u64,
        window: &[u8],
        member: Member,
    ) -> Result<Engine> {
        // A static block's header: its final-block bit, then type 01.
        let header = u8::from(last) | 0b01 << 1;
        Engine::resumed(bit, Some(header), plaintext, window, member)
    }

    fn resumed(
        bit: u64,
        header: Option<u8>,
        plaintext: u64,
        window: &[u8],
        member: Member,
    ) -> Result<Engine> {
        let mut raw = Raw::new()?;
        raw.set_dictionary(window)?;
        let skip = (bit % 8) as u8;
        Ok(Engine {
            raw,
            phase: Phase::Deflate,
            fed: bit / 8,
            prime: (skip != 0 || header.is_some()).then_some(Prime { header, skip }),
            tail: 0,
            tail_len: 0,
            pending: Some(match header {
                Some(header) => Pending::Body { bit, last: header & 1 != 0 },
                None => Pending::Header(bit),
            }),
            mark: None,
            walking: None,
            plaintext,
            member,
            last_boundary: header.is_none().then_some(bit),
            checks_trailers: true,
            trailer: None,
        })
    }

    /// Walk the static block whose body is entered with a mark set, stopping
    /// at the first symbol at or past plaintext `mark` with [`Event::Symbol`],
    /// which clears the mark; `None` walks none.
    pub(crate) fn set_mark(&mut self, mark: Option<u64>) {
        self.mark = mark;
    }

    /// The same engine comparing no trailer: each is read and kept at
    /// [`Engine::last_trailer`], for a caller that knows the member's plaintext
    /// before this decode began to compare.
    pub(crate) fn unchecked(mut self) -> Engine {
        self.checks_trailers = false;
        self
    }

    /// The last trailer read.
    pub(crate) fn last_trailer(&self) -> Option<Trailer> {
        self.trailer
    }

    /// The plaintext offset of the next byte produced.
    pub(crate) fn plaintext(&self) -> u64 {
        self.plaintext
    }

    /// The compressed offset of the next byte the caller supplies.
    pub(crate) fn compressed_position(&self) -> u64 {
        self.fed
    }

    /// Between members, with nothing of the next one read.
    pub(crate) fn is_between(&self) -> bool {
        matches!(self.phase, Phase::Between)
    }

    pub(crate) fn member(&self) -> &Member {
        &self.member
    }

    /// The CRC-32 of the plaintext produced since the run began: the member's
    /// start, the resume point, or the last [`Engine::cut`].
    pub(crate) fn run_crc(&self) -> u32 {
        self.member.run_crc
    }

    /// End the current run here, folding it into the member's prefix, and
    /// answer its CRC-32.
    pub(crate) fn cut(&mut self) -> u32 {
        let run = self.member.run_crc;
        let len = self.plaintext - self.member.run_start;
        self.member.prefix =
            self.member.prefix.map(|prefix| crate::zlib::crc32_combine(prefix, run, len));
        self.member.run_crc = 0;
        self.member.run_start = self.plaintext;
        run
    }

    /// The window: the last plaintext of this member, up to 32 KiB of it.
    pub(crate) fn dictionary(&self, buf: &mut [u8; WINDOW_SIZE]) -> usize {
        self.raw.dictionary(buf)
    }

    /// Take the next bytes of the file, `input`, beginning at
    /// [`Engine::compressed_position`], and produce plaintext into `out`.
    pub(crate) fn step(&mut self, input: &[u8], out: &mut [u8]) -> Result<Stepped> {
        self.step_into(input, &mut Out::slice(out))
    }

    /// [`Engine::step`], appending the plaintext to `out` inside its
    /// capacity, which is never zeroed first.
    pub(crate) fn step_appending(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<Stepped> {
        self.step_into(input, &mut Out::spare(out))
    }

    fn step_into(&mut self, input: &[u8], out: &mut Out<'_>) -> Result<Stepped> {
        match self.phase {
            Phase::Between => Ok(self.between(input)),
            Phase::Header(_) => self.header(input),
            Phase::Deflate => self.inflate(input, out),
            Phase::Trailer { .. } => self.trailer(input),
        }
    }

    fn between(&mut self, input: &[u8]) -> Stepped {
        if input.is_empty() {
            return Stepped::default();
        }
        let offset = self.fed;
        self.member = Member::fresh(offset, self.plaintext);
        self.phase = Phase::Header(Header::new(offset));
        Stepped { event: Some(Event::MemberStart), ..Stepped::default() }
    }

    fn header(&mut self, input: &[u8]) -> Result<Stepped> {
        let Phase::Header(header) = &mut self.phase else {
            unreachable!("called in the header phase")
        };
        let consumed = header.read(input)?;
        self.fed += consumed as u64;
        let mut event = None;
        if header.is_done() {
            event = Some(Event::Header { bgzf: bgzf_member_len(header.head()).is_some() });
            self.raw.reset()?;
            self.last_boundary = None;
            self.pending = Some(Pending::Header(8 * self.fed));
            self.walking = None;
            self.phase = Phase::Deflate;
        }
        Ok(Stepped { consumed, event, ..Stepped::default() })
    }

    fn inflate(&mut self, input: &[u8], out: &mut Out<'_>) -> Result<Stepped> {
        let mut input = input;
        let mut consumed = 0;
        if let Some(Prime { header, skip }) = self.prime {
            let (mut bits, mut value) = (0u8, 0u32);
            if skip != 0 {
                let Some(&first) = input.first() else {
                    return Ok(Stepped::default());
                };
                (bits, value) = (8 - skip, u32::from(first >> skip));
                self.remember(&input[..1]);
                self.fed += 1;
                input = &input[1..];
                consumed = 1;
            }
            if let Some(header) = header {
                (bits, value) = (bits + 3, u32::from(header) | value << 3);
            }
            self.raw.prime(bits, value)?;
            self.prime = None;
        }
        match self.pending {
            Some(Pending::Header(bit)) => {
                let Some(header) = self.peek(bit, 3, input) else {
                    return Ok(Stepped { consumed, ..Stepped::default() });
                };
                self.pending = None;
                // Type 01: a static block, whose first symbol follows its
                // three header bits.
                if header >> 1 == 0b01 {
                    self.pending = Some(Pending::Body { bit: bit + 3, last: header & 1 != 0 });
                    return Ok(Stepped {
                        consumed,
                        produced: 0,
                        event: Some(Event::Static { bit }),
                    });
                }
            }
            Some(Pending::Body { bit, last }) => {
                self.pending = None;
                if self.mark.is_some() {
                    let held = self.taken_from(bit);
                    self.walking = Some(Walking {
                        walk: Box::new(Walk::new(bit, self.plaintext, &held)),
                        last,
                        found: None,
                    });
                }
            }
            None => {}
        }
        let (event, held) = self.walk_on(input);
        if let Some(event) = event {
            return Ok(Stepped { consumed, produced: 0, event: Some(event) });
        }
        let input = &input[..held];
        let room = self.room(out.room());
        let at = self.fed;
        let located = |e| match e {
            Error::Deflate { message, .. } => Error::Deflate { offset: at, message },
            other => other,
        };
        let mut inflated = self.raw.inflate(input, out, room).map_err(located)?;
        if inflated.at_final_block_end() {
            // The stream ends at the next byte boundary, which takes no more
            // input; a stop here would otherwise look like no progress.
            let more = self
                .raw
                .inflate(&input[inflated.consumed..], out, room - inflated.produced)
                .map_err(located)?;
            inflated = more.after(inflated);
        }
        debug_assert_eq!(out.written().len(), inflated.produced);
        self.took(&input[..inflated.consumed], out.written());
        let (consumed, produced) = (consumed + inflated.consumed, inflated.produced);
        if inflated.stream_end {
            assert_eq!(
                inflated.unused_bits(),
                0,
                "gz-seek: the inflater ended a stream holding unused bits, which zlib's \
                 contract rules out (docs/design/runtime-invariants.md, RT23)"
            );
            self.walking = None;
            self.phase = Phase::Trailer { bytes: [0; Trailer::LEN], have: 0 };
            return Ok(Stepped { consumed, produced, event: None });
        }
        let mut event = self.symbol_reached();
        if inflated.at_block_boundary() {
            let bit = 8 * self.fed - inflated.unused_bits();
            if self.last_boundary != Some(bit) {
                self.last_boundary = Some(bit);
                self.walking = None;
                self.pending = Some(Pending::Header(bit));
                event = Some(Event::Block { bit });
            }
        }
        Ok(Stepped { consumed, produced, event })
    }

    /// The inflater took `input` and wrote `out`.
    fn took(&mut self, input: &[u8], out: &[u8]) {
        self.remember(input);
        self.fed += input.len() as u64;
        self.member.run_crc = crate::zlib::crc32(self.member.run_crc, out);
        self.plaintext += out.len() as u64;
    }

    /// Keep the last of `bytes` the inflater took in [`Engine::tail`].
    fn remember(&mut self, bytes: &[u8]) {
        for &byte in &bytes[bytes.len().saturating_sub(8)..] {
            self.tail = self.tail << 8 | u64::from(byte);
        }
        self.tail_len = self.tail_len.saturating_add(bytes.len().min(8) as u8).min(8);
    }

    /// The bytes from the one compressed `bit` is in to the next untaken
    /// one: among the last eight the inflater took, a stop holding fewer
    /// than 64 bits unused (`docs/design/runtime-invariants.md`, "RT23").
    fn taken_from(&self, bit: u64) -> Vec<u8> {
        let held = usize::try_from(self.fed.saturating_sub(bit / 8)).unwrap_or(usize::MAX);
        assert!(
            held <= usize::from(self.tail_len),
            "gz-seek: a stop held more unused bits than the bytes it took"
        );
        (0..held).rev().map(|i| (self.tail >> (8 * i)) as u8).collect()
    }

    /// The `n` bits from compressed `bit` on, read from the bytes the
    /// inflater took and then `input`, the bytes from the next untaken one:
    /// `None` where `input` does not reach them.
    fn peek(&self, bit: u64, n: u32, input: &[u8]) -> Option<u32> {
        let mut bytes = self.taken_from(bit);
        let wanted = usize::try_from((bit + u64::from(n)).div_ceil(8) - bit / 8).ok()?;
        let more = wanted.saturating_sub(bytes.len());
        bytes.extend_from_slice(input.get(..more)?);
        let word = bytes.iter().rev().fold(0u64, |word, &byte| word << 8 | u64::from(byte));
        Some((word >> (bit % 8)) as u32 & ((1 << n) - 1))
    }

    /// Walk the static block on over `input`, the bytes from the next untaken
    /// one, handing the walk a few KiB at a time: [`Event::Symbol`] where the
    /// walk's stop is reached already, and how much of `input` the inflater
    /// may take — no more than the walk has been handed, so the walk has read
    /// every byte the inflater took, holding a few KiB of them.
    fn walk_on(&mut self, input: &[u8]) -> (Option<Event>, usize) {
        let Some(walking) = &mut self.walking else { return (None, input.len()) };
        let walk = &mut walking.walk;
        loop {
            if walking.found.is_none() {
                match walk.run(self.mark.unwrap_or(u64::MAX)) {
                    Ok(Walked::Mark { bit, plaintext }) => walking.found = Some((bit, plaintext)),
                    Ok(Walked::Input) => {}
                    // Past the block's end the inflater stops at it; a symbol
                    // the walk refuses, the inflater refuses too, at that
                    // symbol.
                    Ok(Walked::End) | Err(_) => {
                        self.walking = None;
                        return (None, input.len());
                    }
                }
            }
            walk.release();
            let from = usize::try_from(walk.end() - self.fed).unwrap_or(usize::MAX);
            if walking.found.is_some() || from >= input.len() {
                break;
            }
            walk.push(&input[from..input.len().min(from + WALK_PUSH)]);
        }
        let held = usize::try_from(walk.end() - self.fed).unwrap_or(usize::MAX);
        (self.symbol_reached(), held.min(input.len()))
    }

    /// [`Event::Symbol`], where the inflater stands at the walk's stop.
    fn symbol_reached(&mut self) -> Option<Event> {
        let walking = self.walking.as_mut()?;
        let (bit, plaintext) = walking.found?;
        if plaintext != self.plaintext {
            return None;
        }
        walking.found = None;
        self.mark = None;
        Some(Event::Symbol { bit, last: walking.last })
    }

    /// How much of `room` the inflater may fill: in a static block being
    /// walked, no further than the walk has reached, or its stop.
    fn room(&self, room: usize) -> usize {
        let Some(walking) = &self.walking else { return room };
        let cap = walking.found.map_or(walking.walk.plaintext(), |(_, plaintext)| plaintext);
        usize::try_from(cap - self.plaintext).unwrap_or(usize::MAX).min(room)
    }

    fn trailer(&mut self, input: &[u8]) -> Result<Stepped> {
        let Phase::Trailer { bytes, have } = &mut self.phase else {
            unreachable!("called in the trailer phase")
        };
        let consumed = (Trailer::LEN - *have).min(input.len());
        bytes[*have..*have + consumed].copy_from_slice(&input[..consumed]);
        *have += consumed;
        self.fed += consumed as u64;
        if *have < Trailer::LEN {
            return Ok(Stepped { consumed, ..Stepped::default() });
        }
        let trailer = Trailer::parse(bytes);
        self.phase = Phase::Between;
        self.trailer = Some(trailer);
        if !self.checks_trailers {
            return Ok(Stepped { consumed, produced: 0, event: Some(Event::MemberEnd) });
        }
        let member = self.member;
        let len = self.plaintext - member.plaintext;
        if trailer.isize != len as u32 {
            return Err(Error::MemberLength {
                offset: member.offset,
                stored: trailer.isize,
                computed: len as u32,
            });
        }
        if let Some(prefix) = member.prefix {
            let run_len = self.plaintext - member.run_start;
            let computed = crate::zlib::crc32_combine(prefix, member.run_crc, run_len);
            if computed != trailer.crc {
                return Err(Error::MemberCheck {
                    offset: member.offset,
                    stored: trailer.crc,
                    computed,
                });
            }
        }
        Ok(Stepped { consumed, produced: 0, event: Some(Event::MemberEnd) })
    }
}

#[cfg(test)]
mod tests;
