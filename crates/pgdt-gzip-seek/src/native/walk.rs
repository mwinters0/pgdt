//! A fixed block's symbols walked without producing a byte: where a decode
//! may resume inside one.
//!
//! A fixed block's codes are deflate's own rather than its header's, so a
//! decode can begin at any of its symbols given a synthetic three-bit header
//! and the history before it. No inflater stops there, so the engine walks
//! the block's codes beside its inflater — counting the plaintext each
//! symbol stands for and writing none — to find the first symbol at or past
//! the mark the forward pass sets, and caps the inflater's output there
//! (`engine.rs`). Nothing it decodes is handed out, so it is not an
//! inflater of the crate's own (`docs/design/decisions.md`, "D117").

use super::huffman::{
    DISTANCE_BASE, Huffman, LENGTH_BASE, entry_base, entry_extra, entry_len, entry_symbol,
};
use super::inflate::fixed_codes;
use super::input::{Bits, Input};

/// Where a walk stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Walked {
    /// The next symbol begins at compressed `bit` and plaintext `plaintext`,
    /// at or past the mark, and is not the block's end.
    Mark { bit: u64, plaintext: u64 },
    /// The block's end-of-block code has been read.
    End,
    /// The next symbol runs past the bytes held.
    Input,
}

/// A walk over one fixed block's symbols, from a symbol boundary on.
pub(crate) struct Walk {
    input: Input,
    /// The compressed bit the next symbol begins at.
    bit: u64,
    /// The plaintext offset the next symbol's bytes begin at.
    plaintext: u64,
}

impl Walk {
    /// A walk from the symbol at compressed `bit` and plaintext `plaintext`,
    /// holding `held`, the file's bytes from the one `bit` is in.
    pub(crate) fn new(bit: u64, plaintext: u64, held: &[u8]) -> Walk {
        let mut input = Input::new(bit / 8, u64::MAX);
        input.push(held);
        Walk { input, bit, plaintext }
    }

    /// The file offset of the next byte the walk wants.
    pub(crate) fn end(&self) -> u64 {
        self.input.end()
    }

    /// Append the file's bytes from [`Walk::end`] on.
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.input.push(bytes);
    }

    /// Stop holding the bytes before the one the next symbol begins in.
    pub(crate) fn release(&mut self) {
        self.input.release_all_before(self.bit / 8);
    }

    /// The plaintext offset the next symbol's bytes begin at: every symbol
    /// before it walked.
    pub(crate) fn plaintext(&self) -> u64 {
        self.plaintext
    }

    /// Walk on until the next symbol begins at or past plaintext `mark`, the
    /// block ends, or a symbol runs past the bytes held.
    ///
    /// # Errors
    ///
    /// A symbol a fixed block cannot hold, by zlib's message.
    pub(crate) fn run(&mut self, mark: u64) -> Result<Walked, &'static str> {
        let (literals, distances) = fixed_codes();
        let (bit, plaintext, walked) =
            walk((literals, distances), &self.input, self.bit, self.plaintext, mark);
        (self.bit, self.plaintext) = (bit, plaintext);
        walked
    }
}

/// [`Walk::run`]'s loop, from the symbol at compressed `bit` and plaintext
/// `plaintext`: where it stopped, and why. Never inlined, as `inflate.rs`'s
/// `codes` is not: its state stays in registers, and a profile names it.
///
/// The bits are held across symbols ([`Bits`]) and each entry carries its
/// symbol's base and extra bits, so a symbol costs a refill, a lookup and,
/// for a length, the lookup of its distance in the same word. Bits past the
/// held bytes read as zeros: a code decoded from them is whole only where it
/// ends before them, a prefix code being decided by its own bits alone, which
/// is all the comparison against `end` asks.
#[inline(never)]
fn walk(
    (literals, distances): (&Huffman, &Huffman),
    input: &Input,
    bit: u64,
    mut plaintext: u64,
    mark: u64,
) -> (u64, u64, Result<Walked, &'static str>) {
    let end = input.end_bit();
    let mut bits = Bits::at(input, bit);
    let walked = loop {
        let at = bits.position();
        bits.refill(input);
        let held = bits.word;
        // Every literal/length code is a fixed block's, and those it cannot
        // hold are refused by their symbol below.
        let Some(entry) = literals.entry(held) else {
            break Err("invalid literal/length code");
        };
        let (symbol, len) = (entry_symbol(entry), entry_len(entry));
        if at + u64::from(len) > end {
            break Ok(Walked::Input);
        }
        if symbol == 256 {
            bits.consume(len);
            break Ok(Walked::End);
        }
        if plaintext >= mark {
            break Ok(Walked::Mark { bit: at, plaintext });
        }
        if symbol < 256 {
            bits.consume(len);
            plaintext += 1;
            continue;
        }
        if usize::from(symbol) >= 257 + LENGTH_BASE.len() {
            break Err("invalid literal/length code");
        }
        let extra = entry_extra(entry);
        let length = entry_base(entry) as u64 + ((held >> len) & ((1 << extra) - 1));
        let mut used = len + extra;
        let Some(entry) = distances.entry(held >> used) else {
            break Err("invalid distance code");
        };
        if usize::from(entry_symbol(entry)) >= DISTANCE_BASE.len() {
            break Err("invalid distance code");
        }
        used += entry_len(entry) + entry_extra(entry);
        if at + u64::from(used) > end {
            break Ok(Walked::Input);
        }
        bits.consume(used);
        plaintext += length;
    };
    (bits.position(), plaintext, walked)
}
