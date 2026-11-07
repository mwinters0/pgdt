//! The lexing a `record_in` field and a `range_in` bound share.

/// What a [`QuotedRun`] makes of a byte.
pub(crate) enum Lexed {
    /// A byte of the part.
    Byte(u8),
    /// Quoting or an escape, standing for nothing yet.
    Nothing,
    /// A terminator outside quotes, which ends the part and is its
    /// container's to read.
    End,
}

/// One `record_in` field or `range_in` bound, lexed a byte at a time: every
/// byte up to the first terminator outside quotes, with `\` taking the next
/// byte literally and `""` inside quotes meaning one `"`. Quoting may start
/// and stop anywhere, so `["a"b,c)` has the lower bound `ab`.
///
/// **Whitespace is never trimmed.** This is the half `array_in` does
/// differently, and reusing an array-shaped scanner here is what silently
/// eats a composite field's leading blank.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct QuotedRun {
    quoted: bool,
    escaped: bool,
    /// Inside quotes, a `"` read, whose meaning the byte after it decides.
    quote: bool,
}

impl QuotedRun {
    pub(crate) fn byte(&mut self, byte: u8, terminators: &[u8]) -> Lexed {
        if self.escaped {
            self.escaped = false;
            return Lexed::Byte(byte);
        }
        if self.quote {
            self.quote = false;
            if byte == b'"' {
                return Lexed::Byte(b'"');
            }
            self.quoted = false;
        }
        if !self.quoted && terminators.contains(&byte) {
            return Lexed::End;
        }
        match byte {
            b'\\' => self.escaped = true,
            b'"' if !self.quoted => self.quoted = true,
            b'"' => self.quote = true,
            byte => return Lexed::Byte(byte),
        }
        Lexed::Nothing
    }
}
