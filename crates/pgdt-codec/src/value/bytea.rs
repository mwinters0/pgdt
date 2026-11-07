//! `bytea`: both input forms read, the hex form written, and why `byteain`
//! refuses a text.

use pgdt_core::internal::Unescaped;

use crate::decode::Unread;
use crate::value::digits::HEX_NIBBLE;
use crate::value::digits::push_hex_pair;

/// Why [`decode_bytea`] read no value from `s`: `byteain`
/// refuses it, or reads a spelling this build does not — a blank between hex
/// pairs, any byte the `escape` form writes escaped written as itself, the
/// reverse, or a byte escaped otherwise than `byteaout` escapes it — `\134`
/// for `\\` (`docs/design/decisions.md`, "D55").
// pg-refuses: I69 — every refusal here is `byteain`'s.
pub fn bytea_unread(text: impl Iterator<Item = u8>) -> Unread {
    // A byte at a time, so a field is answered as `Unescaped` reads it with
    // no copy of it (`docs/design/decisions.md`, "D142").
    let mut text = text.peekable();
    let opened = text.next_if_eq(&b'\\').is_some();
    let reads = if opened && text.next_if_eq(&b'x').is_some() {
        // `hex_decode_safe`: a blank between pairs, never inside one.
        let mut run = 0usize;
        loop {
            match text.next() {
                None => break run.is_multiple_of(2),
                Some(b' ' | b'\n' | b'\t' | b'\r') if run.is_multiple_of(2) => run = 0,
                Some(c) if c.is_ascii_hexdigit() => run += 1,
                Some(_) => break false,
            }
        }
    } else {
        // A `\` taken looking for the `x` is the escape form's first byte.
        let mut text = opened.then_some(b'\\').into_iter().chain(text);
        loop {
            match text.next() {
                None => break true,
                Some(b'\\') => match text.next() {
                    Some(b'\\') => {}
                    Some(b'0'..=b'3') => {
                        let octal = |c: Option<u8>| matches!(c, Some(b'0'..=b'7'));
                        if !(octal(text.next()) && octal(text.next())) {
                            break false;
                        }
                    }
                    _ => break false,
                },
                Some(_) => {}
            }
        }
    };
    if reads { Unread::Unparsed } else { Unread::Refused }
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
    read_unescaped(text, Some(out))
}

/// Whether [`decode_bytea`] reads a value from a field's bytes as
/// [`Unescaped`] reads them, holding none of them.
pub fn bytea_reads(text: Unescaped<'_>) -> bool {
    read_unescaped(text, None)
}

/// [`decode_bytea_into`], or with no `out` [`bytea_reads`]. The hex form
/// `pg_dump` writes, `\x` and pairs no escape spells, is read as a slice;
/// any other spelling a byte at a time, twice where it is kept — once to
/// count it, so `out` grows once, and once to write it.
fn read_unescaped(text: Unescaped<'_>, out: Option<&mut Vec<u8>>) -> bool {
    let mut probe = text.clone();
    if probe.next() == Some(b'\\')
        && probe.next() == Some(b'x')
        && let Some(hex) = probe.rest_if_plain()
    {
        return match out {
            Some(out) => push_hex(hex, out),
            None => hex.len().is_multiple_of(2) && hex.iter().all(|&b| HEX_NIBBLE[b as usize] < 16),
        };
    }
    let mut len = 0usize;
    if !read_bytea(text.clone(), |_| len += 1) {
        return false;
    }
    if let Some(out) = out {
        out.reserve(len);
        read_bytea(text, |byte| out.push(byte));
    }
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
pub(crate) fn read_bytea(mut text: impl Iterator<Item = u8>, emit: impl FnMut(u8)) -> bool {
    let first = text.next();
    let second = if first == Some(b'\\') { text.next() } else { None };
    if second == Some(b'x') {
        return read_hex(text, emit);
    }
    read_escape(first.into_iter().chain(second).chain(text), emit)
}

/// The hex pairs after the `\x`, a byte at a time.
fn read_hex(mut text: impl Iterator<Item = u8>, mut emit: impl FnMut(u8)) -> bool {
    loop {
        let Some(first) = text.next() else { return true };
        let Some(second) = text.next() else { return false };
        let (hi, lo) = (HEX_NIBBLE[first as usize], HEX_NIBBLE[second as usize]);
        if (hi | lo) & 0xF0 != 0 {
            return false;
        }
        emit(hi << 4 | lo);
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
    let reads = read_escape(text, |byte| {
        if out.len() < limit {
            out.push(byte);
        }
    });
    reads.then_some(out)
}

/// [`decode_bytea_escape`]'s grammar, each byte it reads handed to `emit`.
fn read_escape(mut text: impl Iterator<Item = u8>, mut emit: impl FnMut(u8)) -> bool {
    while let Some(first) = text.next() {
        let byte = match first {
            b'\\' => match text.next() {
                Some(b'\\') => b'\\',
                Some(o1 @ b'0'..=b'3') => {
                    let (Some(o2 @ b'0'..=b'7'), Some(o3 @ b'0'..=b'7')) =
                        (text.next(), text.next())
                    else {
                        return false;
                    };
                    let byte = (o1 - b'0') << 6 | (o2 - b'0') << 3 | (o3 - b'0');
                    if ESCAPE_PRINTS_ITSELF.contains(&byte) {
                        return false;
                    }
                    byte
                }
                _ => return false,
            },
            byte if ESCAPE_PRINTS_ITSELF.contains(&byte) => byte,
            _ => return false,
        };
        emit(byte);
    }
    true
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
