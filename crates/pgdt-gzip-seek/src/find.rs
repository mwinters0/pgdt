//! Finding where a gzip file's deflate data may resume, in bytes whose
//! context is unknown: a dynamic or a stored block's start, or a member's
//! header.
//!
//! A guess, never a finding. Any bit can begin something that reads as a
//! header, so what this answers is a place worth decoding from, and the
//! parallel pass ([`Parallel`](crate::Parallel)) takes a decode from it only
//! where the engine's decode before it ends there exactly, with the history
//! that decode began with. A fixed block is not looked for: its header is
//! three bits with nothing to check, so it would be found everywhere, and the
//! decode before reaches past it instead; nor is a block marked last. What is checked, cheapest first: a dynamic header's
//! fields in range and its code-length code complete, then the whole header as
//! zlib reads it; a stored header's length beside its complement, and its
//! padding zero, as every encoder writes it; a member's magic, method and
//! reserved flags.

use crate::member::GZIP_MAGIC;
use crate::native::{Inflater, Input};

/// Where a decode may begin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Candidate {
    /// A block's header at this bit, read into the inflater, whose body
    /// begins at `body`; for a stored block, the bit its length begins at.
    Block { bit: u64, body: u64, stored: Option<u64> },
    /// A member's header at this byte.
    Member { offset: u64 },
}

/// What a search came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Found {
    At(Candidate),
    /// The search stands at this bit and needs more input to go on.
    Input(u64),
    /// Nothing begins before the search's end.
    Nothing,
}

/// The bits a search wants past a candidate before an unfinished input's end.
///
/// Deficiency register: `deficiency: KD140` — a dynamic header's cheap
/// checks read up to 74 bits, its code-length code's included, so within
/// that of an unfinished input's end they read zeros for the bytes not yet
/// fed, and a true block whose check fails on them is passed over for good,
/// the guess beginning at a later candidate or none and the frontier decoding
/// the gap. **(c) unowned**; promoted by a speculation seen to miss its
/// chunk's first block.
const CHEAP_BITS: u64 = 64;

/// Search `input` from bit `from` to bit `until` for the first candidate,
/// leaving a block candidate's header read into `inflater`.
pub(crate) fn find(input: &Input, from: u64, until: u64, inflater: &mut Inflater) -> Found {
    let complete = input.is_complete();
    let end = input.end_bit();
    for bit in from..until.min(end) {
        if !complete && end - bit < CHEAP_BITS {
            return Found::Input(bit);
        }
        if bit % 8 == 0 && is_member_header(input, bit / 8) {
            return Found::At(Candidate::Member { offset: bit / 8 });
        }
        let bits = input.peek(bit);
        let plausible = match bits & 7 {
            // Not last, dynamic.
            0b100 => dynamic_is_plausible(input, bit, bits),
            // Not last, stored.
            0b000 => stored_is_plausible(bit, input),
            _ => false,
        };
        if !plausible {
            continue;
        }
        match inflater.header_at(input, bit) {
            Ok(Some(body)) => {
                let stored = (bits & 7 == 0).then(|| stored_length_at(bit));
                return Found::At(Candidate::Block { bit, body, stored });
            }
            Ok(None) if !complete => return Found::Input(bit),
            Ok(None) | Err(_) => {}
        }
    }
    if until > end && !complete { Found::Input(end.max(from)) } else { Found::Nothing }
}

/// Whether a block whose header's first bits are `bits` is one a search
/// looks for: dynamic or stored, and not the stream's last.
pub(crate) fn is_searched_for(bits: u64) -> bool {
    matches!(bits & 7, 0b100 | 0b000)
}

/// gzip's magic, deflate's method and no reserved flag at byte `offset`.
fn is_member_header(input: &Input, offset: u64) -> bool {
    let magic =
        GZIP_MAGIC.iter().enumerate().all(|(i, &b)| input.byte(offset + i as u64) == Some(b));
    magic && input.byte(offset + 3).is_some_and(|flags| flags & 0xe0 == 0)
}

/// A dynamic header's counts in range and its code-length code complete, the
/// header's first 17 bits being `bits`'s.
fn dynamic_is_plausible(input: &Input, bit: u64, bits: u64) -> bool {
    let literals = (bits >> 3) & 31;
    let distances = (bits >> 8) & 31;
    if literals > 29 || distances > 29 {
        return false;
    }
    let listed = ((bits >> 13) & 15) as usize + 4;
    let lengths = input.peek(bit + 17);
    let mut count = [0u32; 8];
    for i in 0..listed {
        count[((lengths >> (3 * i)) & 7) as usize] += 1;
    }
    // Complete: the lengths' Kraft sum, in units of 2^-7, exactly one.
    let kraft: u32 = (1..8).map(|len| count[len] << (7 - len)).sum();
    kraft == 128
}

/// The bit a stored block whose header begins at `bit` states its length at:
/// the byte boundary after its three header bits.
///
/// **A stored header has aliases.** Its header is three zero bits and its
/// padding is zeros, so a search reading it from an earlier bit whose bits
/// up to the boundary are also zero — the high bit of an ASCII byte before
/// it, say — and whose length falls at the same byte boundary, so at most
/// seven bits before it, reads the same length there, and decodes the same
/// block. The search answers the first; a pass at any later one of them
/// stands at that block (`Guess::begins_at`).
pub(crate) fn stored_length_at(bit: u64) -> u64 {
    (bit + 3).div_ceil(8) * 8
}

/// A stored header: its padding zero, and its length the complement of the
/// one after it.
fn stored_is_plausible(bit: u64, input: &Input) -> bool {
    let aligned = stored_length_at(bit);
    let padding = aligned - bit - 3;
    if input.peek(bit + 3) & ((1 << padding) - 1) != 0 {
        return false;
    }
    let word = input.peek(aligned);
    (word & 0xffff) as u16 == !((word >> 16) & 0xffff) as u16
}

#[cfg(test)]
mod tests;
