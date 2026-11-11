//! The character types: a field against its typmod's length.

use pgdt_core::internal::Unescape;

/// Whether a `character varying(n)` or `character(n)` field, `n` being
/// `length`, is one `varchar_input` or `bpchar_input` refuses: longer than
/// `length` characters, with anything but a blank, `0x20`, past them (I75).
/// The characters are counted in UTF-8, the one encoding read here; a field
/// no longer in bytes than `length` is read without counting, and a field it
/// does not refuse is read as written ([`crate::compare::CompareKind::Text`]).
// deficiency: KD96 — a field longer than `length` only by trailing blanks,
// which `varchar_input` and `bpchar_input` cut to `length` characters (I75),
// keeps them here, and a `character(n)` field shorter than `length`, which
// `bpchar_input` pads, is not padded. Either way a typed read emits a value
// the server does not hold; only a `character varying(n)`'s longer field also
// answers otherwise, keyed, bounded and compared with its blanks, so `<` and
// `=` differ from the server over it, where `character(n)`'s comparison trims
// both sides. No `pg_dump` writes either. The fix cuts the field in the read,
// the key and the bounds, and records per group whether any field was cut: a
// `SchemaMode::Strings` term reads the column as its text from the same set
// (D79), so it reads only a group where none was, never losing a real dump's
// bounds; `=`, `!=` and `IN` answer by the cut value only once `KD82` closes.
// `numeric(p,s)` is rounded where this is not because its rounding decides
// its refusal (I51), and no refusal here needs the cut. **(c) unowned**;
// promoted by a hand-written dump holding one.
// pg-refuses: I75 — a non-blank past the typmod's length.
pub fn char_typmod_refuses(text: &str, length: u32) -> bool {
    if text.len() <= length as usize {
        return false;
    }
    let mut fold = TextFold::default();
    fold.push(text.as_bytes());
    fold.finish().is_some_and(|read| read.typmod_refuses(length))
}

/// **A text read a run of bytes at a time**, the runs needing no common
/// slice: whether they are UTF-8, and what [`char_typmod_refuses`] and a
/// `character`'s comparison read of it, its length and where its trailing
/// blanks begin — so a field is answered off its bytes as `Unescaped` reads
/// them, with no copy of it made (`docs/design/decisions.md`, "D142").
#[derive(Debug, Default, Clone)]
pub struct TextFold {
    read: TextRead,
    /// Every character seen.
    chars: usize,
    /// A character a run ended inside, its bytes so far.
    partial: [u8; 4],
    partial_len: usize,
    invalid: bool,
}

/// What a [`TextFold`] read of a text that is UTF-8.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TextRead {
    /// Its length in bytes.
    pub len: usize,
    /// Its length in bytes without its trailing blanks (`0x20`), which a
    /// `character`'s comparison ignores.
    pub trimmed: usize,
    /// Its characters ahead of its trailing blanks.
    pub trimmed_chars: usize,
}

impl TextRead {
    /// [`char_typmod_refuses`]: a non-blank past `length` characters is a
    /// character before the trailing blanks past them.
    pub fn typmod_refuses(&self, length: u32) -> bool {
        self.trimmed_chars > length as usize
    }
}

impl TextFold {
    /// What a text whose bytes `bytes` yields holds, read a run at a time,
    /// each byte handed to `seen` as it is read; `None` where it is not
    /// UTF-8.
    pub fn read(bytes: impl Iterator<Item = u8>, mut seen: impl FnMut(u8)) -> Option<TextRead> {
        let mut fold = Self::default();
        let mut run = [0u8; 1024];
        let mut at = 0;
        for byte in bytes {
            seen(byte);
            run[at] = byte;
            at += 1;
            if at == run.len() {
                fold.push(&run);
                at = 0;
            }
        }
        fold.push(&run[..at]);
        fold.finish()
    }

    /// Read the next run of the text's bytes.
    pub fn push(&mut self, mut run: &[u8]) {
        if self.invalid {
            return;
        }
        while self.partial_len > 0 {
            let Some((&byte, rest)) = run.split_first() else { return };
            run = rest;
            self.partial[self.partial_len] = byte;
            self.partial_len += 1;
            match std::str::from_utf8(&self.partial[..self.partial_len]) {
                Ok(_) => {
                    let partial = self.partial;
                    self.fold(&partial[..self.partial_len]);
                    self.partial_len = 0;
                }
                Err(e) if e.error_len().is_none() => {}
                Err(_) => return self.invalid = true,
            }
        }
        match std::str::from_utf8(run) {
            Ok(_) => self.fold(run),
            Err(e) => {
                let (valid, rest) = run.split_at(e.valid_up_to());
                self.fold(valid);
                if e.error_len().is_some() {
                    return self.invalid = true;
                }
                self.partial[..rest.len()].copy_from_slice(rest);
                self.partial_len = rest.len();
            }
        }
    }

    /// Fold `valid`, whole characters of the text.
    fn fold(&mut self, valid: &[u8]) {
        for &byte in valid {
            self.read.len += 1;
            // A UTF-8 continuation byte is no character of its own.
            if byte & 0xC0 != 0x80 {
                self.chars += 1;
            }
            if byte != b' ' {
                self.read.trimmed = self.read.len;
                self.read.trimmed_chars = self.chars;
            }
        }
    }

    /// The bytes of whole characters read ahead of the first that is not
    /// UTF-8, or every byte read: what a `Utf8Error`'s `valid_up_to` says of
    /// a text [`Self::finish`] answers `None` for.
    pub fn valid_up_to(&self) -> usize {
        self.read.len
    }

    /// What was read, `None` where the text is not UTF-8.
    pub fn finish(self) -> Option<TextRead> {
        (!self.invalid && self.partial_len == 0).then_some(self.read)
    }
}

/// Where an escaped field's text stops being UTF-8 — the bytes of whole
/// characters ahead of the first that is not, as a `Utf8Error`'s
/// `valid_up_to` says — or `None` where it is UTF-8 throughout: `field` as
/// `COPY` wrote it, read a run at a time as `Unescape` hands them, with no
/// copy of it made. An escape can spell a byte that is not UTF-8 out of
/// bytes that are.
pub fn escaped_utf8_error(field: &[u8]) -> Option<usize> {
    let mut utf8 = TextFold::default();
    let mut unescape = Unescape::default();
    unescape.run(field, |run| utf8.push(run));
    unescape.finish(|run| utf8.push(run));
    let valid_up_to = utf8.valid_up_to();
    utf8.finish().is_none().then_some(valid_up_to)
}

#[cfg(test)]
mod tests;
