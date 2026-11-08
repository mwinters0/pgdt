//! `uuid`: its reader and its renderer.

use crate::value::digits::HEX_NIBBLE;
use crate::value::digits::push_hex_pair;

/// `uuid_in`'s grammar, field and literal alike ([`UuidIn`] fed it whole).
pub fn decode_uuid(s: &str) -> Option<[u8; 16]> {
    let mut read = UuidIn::default();
    s.bytes().for_each(|byte| read.byte(byte));
    read.finish()
}

/// **`uuid_in` fed a text a byte at a time**, holding the sixteen bytes it
/// reads however long the text runs (`docs/design/decisions.md`, "D144"):
/// 32 hex digits in either case (PostgreSQL always dumps the canonical
/// lowercase `8-4-4-4-12`, but nothing forces that on a hand-written dump),
/// each group of four but the last optionally followed by one hyphen, the
/// whole optionally in braces.
///
/// It runs over bytes rather than chars: `-` and the braces are ASCII, and a
/// non-ASCII byte is not a hex digit either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct UuidIn {
    bytes: [u8; 16],
    /// Hex digits read, up to 32.
    digits: u8,
    /// Whether a hyphen may come next, after a group of four but the last.
    hyphen: bool,
    /// The text opened with a brace, which closes it.
    braced: bool,
    /// What follows the last digit: nothing, or a brace and nothing.
    closed: bool,
    started: bool,
    refused: bool,
}

impl UuidIn {
    /// Read the text's next byte.
    pub(crate) fn byte(&mut self, byte: u8) {
        let started = std::mem::replace(&mut self.started, true);
        if self.refused {
            return;
        }
        if !started && byte == b'{' {
            self.braced = true;
        } else if self.digits == 32 {
            // A brace closing what one opened, and nothing after it.
            self.refused = !self.braced || self.closed || byte != b'}';
            self.closed = true;
        } else if self.hyphen && byte == b'-' {
            self.hyphen = false;
        } else {
            let nibble = HEX_NIBBLE[usize::from(byte)];
            self.refused = nibble & 0xF0 != 0;
            let at = usize::from(self.digits / 2);
            self.bytes[at] = self.bytes[at] << 4 | nibble & 0x0F;
            self.digits += 1;
            // One hyphen after a group of four but the last, past which
            // only a brace may follow; a hyphen anywhere else is a byte no
            // hex digit matches.
            self.hyphen = self.digits.is_multiple_of(4);
        }
    }

    /// The text has ended: the value, or `None` where `uuid_in` refuses it.
    // pg-refuses: I64 — every refusal here is `string_to_uuid`'s.
    pub(crate) fn finish(&self) -> Option<[u8; 16]> {
        let ended = self.digits == 32 && self.braced == self.closed;
        (ended && !self.refused).then_some(self.bytes)
    }
}

pub fn render_uuid(bytes: &[u8; 16]) -> String {
    // `8-4-4-4-12`: 32 hex digits and four hyphens.
    let mut out = String::with_capacity(36);
    for (i, byte) in bytes.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        push_hex_pair(&mut out, *byte);
    }
    out
}

#[cfg(test)]
mod tests;
