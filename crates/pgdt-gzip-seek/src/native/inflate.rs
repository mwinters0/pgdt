//! One raw deflate stream decoded natively, block by block, stopping at each
//! block's end as zlib's `Z_BLOCK` does.
//!
//! The decode reads an [`Input`] at an absolute bit and commits its position
//! only past a whole step — a block header, or one symbol with its extra bits
//! — so that running out of input leaves it where the step began, to be
//! taken again once more input is held. A symbol is decoded only with 64 bits
//! held past it, which a whole gzip member always has: its trailer is eight
//! bytes.

use std::sync::OnceLock;

use super::huffman::{
    DISTANCE_BASE, Huffman, Kind, LENGTH_BASE, Values, entry_base, entry_extra, entry_len,
    entry_symbol,
};
use super::input::{Bits, Input, Reader};
use super::sink::Sink;

/// The order a dynamic header lists the code-length code's lengths in.
const CODE_LENGTH_ORDER: [usize; 19] =
    [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// The bits held past a symbol before it is decoded: more than the longest
/// symbol with its extra bits, and no more than a member's trailer.
const SYMBOL_LOOKAHEAD: u64 = 64;

/// The most input a decode holds past where it stands when [`Inflater::run`]
/// stops for more, in bytes: a dynamic block's longest header — its type,
/// its three counts, nineteen code-length code lengths, and 320 code lengths
/// of at most seven bits each, a repeat's extra bits standing for three or
/// more — which is longer than a stored block's header or a symbol's
/// lookahead.
pub(crate) const LONGEST_HEADER: usize =
    (3 + 5 + 5 + 4 + 19 * 3 + (288 + 32) * 7_usize).div_ceil(8);
const _: () = assert!(LONGEST_HEADER * 8 >= SYMBOL_LOOKAHEAD as usize);

/// The fixed block's codes.
pub(super) fn fixed_codes() -> &'static (Huffman, Huffman) {
    static FIXED: OnceLock<(Huffman, Huffman)> = OnceLock::new();
    FIXED.get_or_init(|| {
        let mut lengths = [0u8; 288];
        lengths[..144].fill(8);
        lengths[144..256].fill(9);
        lengths[256..280].fill(7);
        lengths[280..].fill(8);
        let (mut literals, mut distances) = (Huffman::empty(), Huffman::empty());
        literals
            .build_for(&lengths, Kind::Other, Values::Lengths)
            .expect("the fixed literal/length code");
        distances
            .build_for(&[5; 32], Kind::Other, Values::Distances)
            .expect("the fixed distance code");
        (literals, distances)
    })
}

/// Where a run stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    /// A block that is not the stream's last has ended; the next begins at
    /// `bit`.
    Block { bit: u64 },
    /// The stream's last block has ended at `bit`; its trailer begins at the
    /// next byte boundary.
    End { bit: u64 },
    /// The decode needs more input than is held.
    Input,
    /// The sink holds the plaintext limit.
    Limit,
    /// The sink's last 32 KiB hold no marker.
    Clean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// At a block's header.
    Header,
    /// Inside a stored block, `left` bytes of it to come.
    Stored { left: u16 },
    /// Inside a block of codes, fixed or the header's.
    Codes { fixed: bool },
    /// Past the stream's last block.
    Done,
}

/// A raw deflate stream's decode.
pub(crate) struct Inflater {
    state: State,
    /// The block being decoded is the stream's last.
    last: bool,
    literals: Huffman,
    distances: Huffman,
    /// The code-length code, a dynamic header's scratch.
    code_lengths: Huffman,
}

impl Inflater {
    /// A decode at a block's header.
    pub(crate) fn new() -> Box<Inflater> {
        Box::new(Inflater {
            state: State::Header,
            last: false,
            literals: Huffman::empty(),
            distances: Huffman::empty(),
            code_lengths: Huffman::empty(),
        })
    }

    /// Read the block header at `bit` as the decode's next step, answering
    /// the bit its body begins at, or `None` where it runs past the held
    /// input.
    ///
    /// # Errors
    ///
    /// A header zlib refuses, by zlib's message.
    pub(crate) fn header_at(
        &mut self,
        input: &Input,
        bit: u64,
    ) -> Result<Option<u64>, &'static str> {
        let mut reader = Reader::new(input, bit);
        let header = self.header(&mut reader);
        if reader.overran() {
            return Ok(None);
        }
        self.state = header?;
        Ok(Some(reader.bit))
    }

    /// Back to a block's header, for a new stream.
    pub(crate) fn reset(&mut self) {
        self.state = State::Header;
        self.last = false;
    }

    /// Decode from `bit` into `sink` to the first [`Stop`] — a block's end,
    /// the stream's, the input's, `limit` bytes held, or a marked sink come
    /// clean in a block of codes — advancing `bit` past each step taken.
    ///
    /// # Errors
    ///
    /// What zlib refuses of a stream, by zlib's message.
    pub(crate) fn run<S: Sink>(
        &mut self,
        input: &Input,
        bit: &mut u64,
        sink: &mut S,
        limit: usize,
    ) -> Result<Stop, &'static str> {
        loop {
            match self.state {
                State::Done => unreachable!("a finished stream is not run"),
                State::Header => {
                    let mut reader = Reader::new(input, *bit);
                    let header = self.header(&mut reader);
                    if reader.overran() {
                        return Ok(Stop::Input);
                    }
                    self.state = header?;
                    *bit = reader.bit;
                }
                State::Stored { left: 0 } => {
                    sink.block_ended();
                    return Ok(self.block_end(*bit));
                }
                State::Stored { left } => {
                    let held = input.from(*bit / 8);
                    let room = limit.saturating_sub(sink.produced());
                    let n = held.len().min(usize::from(left)).min(room);
                    if n == 0 {
                        return Ok(if room == 0 { Stop::Limit } else { Stop::Input });
                    }
                    sink.bytes(&held[..n]);
                    *bit += 8 * n as u64;
                    self.state = State::Stored { left: left - n as u16 };
                }
                State::Codes { fixed } => {
                    let tables = if fixed {
                        let (literals, distances) = fixed_codes();
                        (literals, distances)
                    } else {
                        (&self.literals, &self.distances)
                    };
                    let stop = codes(tables, input, bit, sink, limit)?;
                    return Ok(self.after_codes(stop, *bit));
                }
            }
        }
    }

    /// A run of codes stopped: at its block's end, or short of it.
    fn after_codes(&mut self, stop: Option<Stop>, bit: u64) -> Stop {
        match stop {
            Some(stop) => stop,
            None => self.block_end(bit),
        }
    }

    /// The block being decoded ended, the next beginning at `bit`.
    fn block_end(&mut self, bit: u64) -> Stop {
        if self.last {
            self.state = State::Done;
            Stop::End { bit }
        } else {
            self.state = State::Header;
            Stop::Block { bit }
        }
    }

    /// Read a block's header — its last-block bit and type, and a dynamic
    /// block's codes or a stored block's length — answering the state its
    /// body is decoded in.
    fn header(&mut self, reader: &mut Reader<'_>) -> Result<State, &'static str> {
        self.last = reader.bits(1) == 1;
        Ok(match reader.bits(2) {
            0 => {
                reader.align();
                let len = reader.bits(16) as u16;
                let nlen = reader.bits(16) as u16;
                if len != !nlen {
                    return Err("invalid stored block lengths");
                }
                State::Stored { left: len }
            }
            1 => State::Codes { fixed: true },
            2 => {
                self.dynamic(reader)?;
                State::Codes { fixed: false }
            }
            _ => return Err("invalid block type"),
        })
    }

    /// A dynamic block's code lengths, and the two codes built from them.
    fn dynamic(&mut self, reader: &mut Reader<'_>) -> Result<(), &'static str> {
        let literals = reader.bits(5) as usize + 257;
        let distances = reader.bits(5) as usize + 1;
        let listed = reader.bits(4) as usize + 4;
        if literals > 286 || distances > 30 {
            return Err("too many length or distance symbols");
        }
        let mut code_lengths = [0u8; 19];
        for &symbol in &CODE_LENGTH_ORDER[..listed] {
            code_lengths[symbol] = reader.bits(3) as u8;
        }
        self.code_lengths
            .build(&code_lengths, Kind::CodeLengths)
            .map_err(|_| "invalid code lengths set")?;
        let mut lengths = [0u8; 286 + 30];
        let total = literals + distances;
        let mut i = 0;
        while i < total {
            if reader.overran() {
                return Ok(());
            }
            let (symbol, len) =
                self.code_lengths.decode(reader.peek()).ok_or("invalid code lengths set")?;
            reader.bit += u64::from(len);
            let (value, repeat) = match symbol {
                0..=15 => (symbol as u8, 1),
                16 => {
                    if i == 0 {
                        return Err("invalid bit length repeat");
                    }
                    (lengths[i - 1], 3 + reader.bits(2) as usize)
                }
                17 => (0, 3 + reader.bits(3) as usize),
                _ => (0, 11 + reader.bits(7) as usize),
            };
            if i + repeat > total {
                return Err("invalid bit length repeat");
            }
            lengths[i..i + repeat].fill(value);
            i += repeat;
        }
        if reader.overran() {
            return Ok(());
        }
        if lengths[256] == 0 {
            return Err("invalid code -- missing end-of-block");
        }
        self.literals
            .build_for(&lengths[..literals], Kind::Other, Values::Lengths)
            .map_err(|_| "invalid literal/lengths set")?;
        self.distances
            .build_for(&lengths[literals..total], Kind::Other, Values::Distances)
            .map_err(|_| "invalid distances set")?;
        Ok(())
    }
}

/// Decode symbols until the block's end — `None` — or until `stop` says
/// why not. Never inlined: the loop keeps its state in registers rather than
/// its caller's frame, and a profile names it apart from its caller.
#[inline(never)]
fn codes<S: Sink>(
    (literals, distances): (&Huffman, &Huffman),
    input: &Input,
    bit: &mut u64,
    sink: &mut S,
    limit: usize,
) -> Result<Option<Stop>, &'static str> {
    let end = input.end_bit();
    let mut bits = Bits::at(input, *bit);
    // One comparison stands for the limit and a marked sink coming clean,
    // which within a block only a copy moves.
    let mut stop = sink.stop_at(limit);
    let result = loop {
        let produced = sink.produced();
        if produced >= stop {
            break Ok(Some(if produced >= limit { Stop::Limit } else { Stop::Clean }));
        }
        if end - bits.position() < SYMBOL_LOOKAHEAD {
            break Ok(Some(Stop::Input));
        }
        bits.refill(input);
        let held = bits.word;
        let Some(entry) = literals.entry(held) else {
            break Err("invalid literal/length code");
        };
        let (symbol, len) = (entry_symbol(entry), entry_len(entry));
        if symbol < 256 {
            sink.literal(symbol as u8);
            bits.consume(len);
            // A second literal from the same bits, which hold more than two
            // codes, where it neither reaches the stop nor is a length.
            if produced + 1 < stop
                && let Some(next) = literals.entry(held >> len)
                && entry_symbol(next) < 256
            {
                sink.literal(entry_symbol(next) as u8);
                bits.consume(entry_len(next));
            }
            continue;
        }
        if symbol == 256 {
            bits.consume(len);
            sink.block_ended();
            break Ok(None);
        }
        // The entries carry each base and its extra bits; a symbol with no
        // base is one zlib refuses.
        if usize::from(symbol) >= 257 + LENGTH_BASE.len() {
            break Err("invalid literal/length code");
        }
        let mut used = len;
        let extra = entry_extra(entry);
        let length = entry_base(entry) + ((held >> used) & ((1 << extra) - 1)) as usize;
        used += extra;
        let Some(entry) = distances.entry(held >> used) else {
            break Err("invalid distance code");
        };
        if usize::from(entry_symbol(entry)) >= DISTANCE_BASE.len() {
            break Err("invalid distance code");
        }
        used += entry_len(entry);
        let extra = entry_extra(entry);
        let distance = entry_base(entry) + ((held >> used) & ((1 << extra) - 1)) as usize;
        used += extra;
        if !sink.copy(distance, length) {
            break Err("invalid distance too far back");
        }
        bits.consume(used);
        stop = sink.stop_at(limit);
    };
    *bit = bits.position();
    result
}
