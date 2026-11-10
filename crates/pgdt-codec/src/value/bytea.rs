//! `bytea`: both input forms read, the hex form written, and why `byteain`
//! refuses a text.

use pgdt_core::internal::Unescaped;

use crate::decode::{Read, Unread};
use crate::value::digits::HEX_NIBBLE;
use crate::value::digits::push_hex_pair;

/// Why [`decode_bytea`] read no value from `s`: `byteain`
/// refuses it, or reads a spelling this build does not — a blank between hex
/// pairs, any byte the `escape` form writes escaped written as itself, the
/// reverse, or a byte escaped otherwise than `byteaout` escapes it — `\134`
/// for `\\` (`docs/design/decisions.md`, "D55"). A byte at a time, so a
/// field is answered as `Unescaped` reads it with no copy of it ("D142").
pub fn bytea_unread(text: impl Iterator<Item = u8>) -> Unread {
    let mut fold = ByteaFold::default();
    for byte in text {
        fold.byte(byte, &mut |_| {});
    }
    match fold.end() {
        Err(Unread::Refused) => Unread::Refused,
        _ => Unread::Unparsed,
    }
}

/// Either of `byteaout`'s forms, chosen by a `bytea_output` `pg_dump` does
/// not pin (I4): `hex`, `\x` followed by hex pairs, and `escape`
/// ([`decode_bytea_escape`]). An `escape` value never opens `\x`, so the
/// prefix tells the two apart (I56). The decoded text (a single backslash),
/// not the doubled-backslash form COPY escaping writes to disk.
pub fn decode_bytea(s: &str) -> Option<Vec<u8>> {
    match s.strip_prefix("\\x") {
        Some(hex) => {
            let mut out = Vec::new();
            push_hex(hex.as_bytes(), &mut out).then_some(out)
        }
        None => decode_bytea_escape(s, usize::MAX),
    }
}

/// [`decode_bytea`] over a field's bytes as [`Unescaped`] reads them,
/// appended to `out`: the value's one decoded copy, written where its caller
/// keeps it, with no unescaped text of the field made first
/// (`docs/design/decisions.md`, "D141"). `false`, `out` as it was, where
/// `decode_bytea` reads no value from the field's text.
pub fn decode_bytea_into(text: Unescaped<'_>, out: &mut Vec<u8>) -> bool {
    // The hex form `pg_dump` writes, `\x` and pairs no escape spells, is
    // read as a slice; any other spelling a byte at a time, twice — once to
    // count it, so `out` grows once, and once to write it.
    let mut probe = text.clone();
    if probe.next() == Some(b'\\')
        && probe.next() == Some(b'x')
        && let Some(hex) = probe.rest_if_plain()
    {
        return push_hex(hex, out);
    }
    let mut len = 0usize;
    if !read_bytea(text.clone(), |_| len += 1) {
        return false;
    }
    out.reserve(len);
    read_bytea(text, |byte| out.push(byte));
    true
}

/// The hex pairs after the `\x`, appended to `out`, which is left as it was
/// where they are not pairs.
fn push_hex(hex: &[u8], out: &mut Vec<u8>) -> bool {
    if !hex.len().is_multiple_of(2) {
        return false;
    }
    // The whole field is one `bytea` value, so validity is accumulated and
    // tested once rather than branched on per pair; a bad nibble poisons the
    // high bits of `bad` and what was pushed is dropped unread.
    out.reserve(hex.len() / 2);
    let at = out.len();
    let mut bad = 0u8;
    for [first, second] in hex.as_chunks::<2>().0 {
        let hi = HEX_NIBBLE[*first as usize];
        let lo = HEX_NIBBLE[*second as usize];
        bad |= hi | lo;
        out.push(hi << 4 | lo);
    }
    if bad & 0xF0 != 0 {
        out.truncate(at);
        return false;
    }
    true
}

/// Either form a byte at a time, each byte it reads handed to `emit`:
/// [`decode_bytea`]'s grammar over bytes that need not lie in one slice.
pub(crate) fn read_bytea(text: impl Iterator<Item = u8>, mut emit: impl FnMut(u8)) -> bool {
    let mut fold = ByteaFold::default();
    for byte in text {
        fold.byte(byte, &mut emit);
    }
    fold.end().is_ok()
}

/// **A `bytea` text read a byte at a time**, its runs needing no common
/// slice: whether [`decode_bytea`] reads it, each byte it reads handed on as
/// it goes, and where it does not, whether `byteain` refuses it — one reading
/// for both answers (`docs/design/decisions.md`, "D144").
#[derive(Debug, Clone, Copy)]
pub(crate) struct ByteaFold {
    form: Form,
    /// Whether [`decode_bytea`]'s grammar reads what has been read.
    decodes: bool,
    /// Whether `byteain`'s refuses it.
    refused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// Nothing read.
    Start,
    /// A first `\\` read, which an `x` makes the hex form's.
    Opened,
    /// The hex form's pairs: a pair's first digit read where `half`, its
    /// value `high`. `hex_decode_safe` takes a blank between pairs, never
    /// inside one, which [`decode_bytea`] does not.
    Hex { half: bool, high: u8 },
    /// The `escape` form.
    Escape(Escape),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escape {
    Plain,
    Backslash,
    /// So many octal digits read, their value so far.
    Octal {
        digits: u8,
        value: u8,
    },
}

impl Default for ByteaFold {
    fn default() -> Self {
        Self { form: Form::Start, decodes: true, refused: false }
    }
}

impl ByteaFold {
    /// A fold of the `escape` form alone, its first byte read as that
    /// form's.
    fn escape() -> Self {
        Self { form: Form::Escape(Escape::Plain), ..Self::default() }
    }

    fn refuse(&mut self) {
        self.decodes = false;
        self.refused = true;
    }

    /// Read the text's next byte, a byte [`decode_bytea`] reads of it
    /// handed to `emit` while it reads every one so far.
    // pg-refuses: I69 — every refusal this fold makes is `byteain`'s.
    pub(crate) fn byte(&mut self, byte: u8, emit: &mut impl FnMut(u8)) {
        if self.refused {
            return;
        }
        self.form = match self.form {
            Form::Start if byte == b'\\' => Form::Opened,
            Form::Start => return self.escaped(Escape::Plain, byte, emit),
            Form::Opened if byte == b'x' => Form::Hex { half: false, high: 0 },
            Form::Opened => return self.escaped(Escape::Backslash, byte, emit),
            Form::Hex { half, high } => {
                let nibble = HEX_NIBBLE[byte as usize];
                if nibble < 16 {
                    if half && self.decodes {
                        emit(high << 4 | nibble);
                    }
                    Form::Hex { half: !half, high: nibble }
                } else if matches!(byte, b' ' | b'\n' | b'\t' | b'\r') && !half {
                    self.decodes = false;
                    Form::Hex { half, high }
                } else {
                    self.refuse();
                    return;
                }
            }
            Form::Escape(escape) => return self.escaped(escape, byte, emit),
        };
    }

    /// `byteaout`'s `escape` form, and no wider (`docs/design/decisions.md`,
    /// "D55"): a byte from space to `~` stands for itself but `\\`, which is
    /// `\\\\`, and every other byte is `\ooo`. `byteain` reads any byte as
    /// itself but `\\`, and `\\\\` and `\ooo` as `byteaout` writes them.
    fn escaped(&mut self, escape: Escape, byte: u8, emit: &mut impl FnMut(u8)) {
        let next = match (escape, byte) {
            (Escape::Plain, b'\\') => Escape::Backslash,
            (Escape::Plain, byte) => {
                self.decodes &= ESCAPE_PRINTS_ITSELF.contains(&byte);
                if self.decodes {
                    emit(byte);
                }
                Escape::Plain
            }
            (Escape::Backslash, b'\\') => {
                if self.decodes {
                    emit(b'\\');
                }
                Escape::Plain
            }
            (Escape::Backslash, b'0'..=b'3') => Escape::Octal { digits: 1, value: byte - b'0' },
            (Escape::Octal { digits, value }, b'0'..=b'7') => {
                let value = value << 3 | (byte - b'0');
                if digits < 2 {
                    Escape::Octal { digits: digits + 1, value }
                } else {
                    self.decodes &= !ESCAPE_PRINTS_ITSELF.contains(&value);
                    if self.decodes {
                        emit(value);
                    }
                    Escape::Plain
                }
            }
            _ => return self.refuse(),
        };
        self.form = Form::Escape(next);
    }

    /// The text has ended: `Ok` where [`decode_bytea`] reads it, and
    /// otherwise whether `byteain` refuses it.
    pub(crate) fn end(&mut self) -> Read<()> {
        match self.form {
            Form::Opened | Form::Hex { half: true, .. } => self.refuse(),
            Form::Escape(escape) if escape != Escape::Plain => self.refuse(),
            _ => {}
        }
        if self.decodes {
            Ok(())
        } else if self.refused {
            Err(Unread::Refused)
        } else {
            Err(Unread::Unparsed)
        }
    }
}

/// `byteaout`'s `escape` form, and no wider (`docs/design/decisions.md`,
/// "D55"): a byte from space to `~` stands for itself but `\`, which is
/// `\\`, and every other byte is `\ooo`. Only the first `limit` bytes are
/// kept, the whole text still checked, so a caller wanting a value's head
/// pays for no more of it.
pub fn decode_bytea_escape(s: &str, limit: usize) -> Option<Vec<u8>> {
    bytea_escape_head(s.bytes(), limit)
}

/// [`decode_bytea_escape`] over a text's bytes a byte at a time, as
/// [`Unescaped`] reads a field's (`docs/design/decisions.md`, "D142").
pub fn bytea_escape_head(text: impl Iterator<Item = u8>, limit: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.size_hint().0.min(limit));
    let mut fold = ByteaFold::escape();
    for byte in text {
        fold.byte(byte, &mut |byte| {
            if out.len() < limit {
                out.push(byte);
            }
        });
    }
    fold.end().is_ok().then_some(out)
}

/// **[`bytea_escape_head`]'s and [`bytea_unread`]'s answers over a text
/// read a run at a time**, the runs needing no common slice: what a
/// statistic reads of a long `bytea` as its runs go by, with no copy of it
/// held (`docs/design/decisions.md`, "D142").
#[derive(Debug, Clone)]
pub struct ByteaRuns {
    escape: ByteaFold,
    either: ByteaFold,
    head: Vec<u8>,
    limit: usize,
}

impl ByteaRuns {
    /// Nothing read yet, the `escape` form's head kept to `limit` bytes.
    pub fn new(limit: usize) -> Self {
        let (escape, either) = (ByteaFold::escape(), ByteaFold::default());
        Self { escape, either, head: Vec::new(), limit }
    }

    /// Read the text's next run.
    pub fn push(&mut self, run: &[u8]) {
        let Self { escape, either, head, limit } = self;
        for &byte in run {
            escape.byte(byte, &mut |byte| {
                if head.len() < *limit {
                    head.push(byte);
                }
            });
            either.byte(byte, &mut |_| {});
        }
    }

    /// The text has ended: [`bytea_escape_head`]'s answer, and whether
    /// [`bytea_unread`] answers [`Unread::Refused`].
    pub fn finish(mut self) -> (Option<Vec<u8>>, bool) {
        let head = self.escape.end().is_ok().then_some(self.head);
        (head, self.either.end() == Err(Unread::Refused))
    }
}

/// The bytes `byteaout`'s `escape` form writes as themselves, `\` aside.
const ESCAPE_PRINTS_ITSELF: std::ops::RangeInclusive<u8> = 0x20..=0x7E;

/// [`decode_bytea_escape`]'s inverse: `byteaout`'s `escape` form of `bytes`.
pub fn render_bytea_escape(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &byte in bytes {
        match byte {
            b'\\' => out.push_str("\\\\"),
            byte if ESCAPE_PRINTS_ITSELF.contains(&byte) => out.push(char::from(byte)),
            byte => {
                out.push('\\');
                for shift in [6, 3, 0] {
                    out.push(char::from(b'0' + (byte >> shift & 0o7)));
                }
            }
        }
    }
    out
}

/// `byteaout`'s `hex` form, PostgreSQL's default and the one a typed
/// `bytea` renders back as whichever form the dump held
/// (`docs/design/decisions.md`, "D66").
pub fn render_bytea(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("\\x");
    for byte in bytes {
        push_hex_pair(&mut out, *byte);
    }
    out
}

#[cfg(test)]
mod tests;
