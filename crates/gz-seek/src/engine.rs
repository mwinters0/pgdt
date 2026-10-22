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
//! **The inflater stops at a deflate stream's end having taken no byte of the
//! trailer**: its reader holds fewer than eight unused bits wherever it stops,
//! as zlib's `data_type` contract states, and the stream ends on a byte
//! boundary, so the next byte is the trailer's (`docs/design/runtime-invariants.md`,
//! "RT23").

use crate::error::{Error, Result};
use crate::member::{Header, Trailer};
use crate::zlib::{Raw, WINDOW_SIZE};

/// What a step stopped at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    /// A member's header begins at [`Engine::member_offset`].
    MemberStart,
    /// A deflate block that is not its member's last has ended; the next one
    /// begins at compressed bit `bit`.
    Block { bit: u64 },
    /// A member's trailer was read and matched.
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

pub(crate) struct Engine {
    raw: Raw,
    phase: Phase,
    /// Compressed offset of the next byte the caller supplies.
    fed: u64,
    /// The bits of the next input byte a resume has already passed.
    skip_bits: Option<u8>,
    plaintext: u64,
    member: Member,
    /// The last block boundary reported, so none is reported twice.
    last_boundary: Option<u64>,
}

impl Engine {
    /// An engine whose next byte begins a member, at compressed `offset` and
    /// plaintext `plaintext`.
    pub(crate) fn at_member(offset: u64, plaintext: u64) -> Result<Engine> {
        Ok(Engine {
            raw: Raw::new()?,
            phase: Phase::Between,
            fed: offset,
            skip_bits: None,
            plaintext,
            member: Member::fresh(offset, plaintext),
            last_boundary: None,
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
        let mut raw = Raw::new()?;
        raw.set_dictionary(window)?;
        let skip = (bit % 8) as u8;
        Ok(Engine {
            raw,
            phase: Phase::Deflate,
            fed: bit / 8,
            skip_bits: (skip != 0).then_some(skip),
            plaintext,
            member,
            last_boundary: Some(bit),
        })
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
        if header.is_done() {
            self.raw.reset()?;
            self.last_boundary = None;
            self.phase = Phase::Deflate;
        }
        Ok(Stepped { consumed, ..Stepped::default() })
    }

    fn inflate(&mut self, input: &[u8], out: &mut [u8]) -> Result<Stepped> {
        let mut input = input;
        let mut primed = 0;
        if let Some(skip) = self.skip_bits {
            let Some(&first) = input.first() else {
                return Ok(Stepped::default());
            };
            self.raw.prime(8 - skip, u32::from(first >> skip))?;
            self.skip_bits = None;
            self.fed += 1;
            input = &input[1..];
            primed = 1;
        }
        let at = self.fed;
        let located = |e| match e {
            Error::Deflate { message, .. } => Error::Deflate { offset: at, message },
            other => other,
        };
        let mut inflated = self.raw.inflate(input, out).map_err(located)?;
        if inflated.at_final_block_end() {
            // The stream ends at the next byte boundary, which takes no more
            // input; a stop here would otherwise look like no progress.
            let more = self
                .raw
                .inflate(&input[inflated.consumed..], &mut out[inflated.produced..])
                .map_err(located)?;
            inflated = more.after(inflated);
        }
        self.fed += inflated.consumed as u64;
        let produced = inflated.produced;
        self.member.run_crc = crate::zlib::crc32(self.member.run_crc, &out[..produced]);
        self.plaintext += produced as u64;
        let consumed = primed + inflated.consumed;
        if inflated.stream_end {
            assert_eq!(
                inflated.unused_bits(),
                0,
                "gz-seek: the inflater ended a stream holding unused bits, which zlib's \
                 contract rules out (docs/design/runtime-invariants.md, RT23)"
            );
            self.phase = Phase::Trailer { bytes: [0; Trailer::LEN], have: 0 };
            return Ok(Stepped { consumed, produced, event: None });
        }
        let mut event = None;
        if inflated.at_block_boundary() {
            let bit = 8 * self.fed - inflated.unused_bits();
            if self.last_boundary != Some(bit) {
                self.last_boundary = Some(bit);
                event = Some(Event::Block { bit });
            }
        }
        Ok(Stepped { consumed, produced, event })
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
