//! `boolean`: its reader, its renderer and why `boolin` refuses a text.

use crate::decode::{Read, Unread};
use crate::value::libc::is_c_space;

/// `t`/`f`, COPY TEXT's boolean spelling.
pub fn decode_bool(s: &str) -> Option<bool> {
    match s {
        "t" => Some(true),
        "f" => Some(false),
        _ => None,
    }
}

/// **`boolin` fed a text a byte at a time**, holding at most its word's
/// first letters however long the text runs (`docs/design/decisions.md`,
/// "D144"): `t` or `f`, `boolout`'s spelling, read as its value, and of any
/// other text whether `boolin` refuses it or reads a spelling `boolout` never
/// writes — `true`, `yes`, `on`, `1` or a negation, given by any prefix
/// naming the word alone, in either case, with blanks around it
/// (`docs/design/decisions.md`, "D55").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct BoolIn {
    stage: BoolStage,
    /// The word's letters, no keyword being longer.
    word: [u8; 5],
    len: u8,
    /// Whether a blank was read around the word.
    blanks: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum BoolStage {
    #[default]
    Lead,
    Word,
    Trail,
    /// A byte past the word's trailing blanks, or a word longer than any.
    Refused,
}

impl BoolIn {
    /// Read the text's next byte.
    pub(crate) fn byte(&mut self, byte: u8) {
        let blank = is_c_space(byte);
        self.blanks |= blank;
        self.stage = match self.stage {
            BoolStage::Lead | BoolStage::Trail if blank => self.stage,
            BoolStage::Word if blank => BoolStage::Trail,
            BoolStage::Lead | BoolStage::Word if usize::from(self.len) < self.word.len() => {
                self.word[usize::from(self.len)] = byte;
                self.len += 1;
                BoolStage::Word
            }
            _ => BoolStage::Refused,
        };
    }

    /// The text has ended: its value, where it is `t` or `f`, or why there
    /// is none.
    // pg-refuses: I65 — every refusal here is `boolin`'s.
    pub(crate) fn finish(&self) -> Read<bool> {
        let word = &self.word[..usize::from(self.len)];
        match (self.stage, word) {
            (BoolStage::Refused, _) => Err(Unread::Refused),
            (_, b"t") if !self.blanks => Ok(true),
            (_, b"f") if !self.blanks => Ok(false),
            _ if bool_reads(word) => Err(Unread::Unparsed),
            _ => Err(Unread::Refused),
        }
    }
}

/// Whether `parse_bool_with_len` reads `word`, its blanks trimmed: a prefix
/// of the one its first letter opens, and `o` alone opens both `on` and
/// `off`.
fn bool_reads(word: &[u8]) -> bool {
    let prefix_of =
        |full: &[u8]| word.len() <= full.len() && word.eq_ignore_ascii_case(&full[..word.len()]);
    match word.first() {
        Some(b't' | b'T') => prefix_of(b"true"),
        Some(b'f' | b'F') => prefix_of(b"false"),
        Some(b'y' | b'Y') => prefix_of(b"yes"),
        Some(b'n' | b'N') => prefix_of(b"no"),
        Some(b'o' | b'O') => word.len() >= 2 && (prefix_of(b"on") || prefix_of(b"off")),
        Some(b'0' | b'1') => word.len() == 1,
        _ => false,
    }
}

pub fn render_bool(v: bool) -> &'static str {
    if v { "t" } else { "f" }
}

#[cfg(test)]
mod tests;
