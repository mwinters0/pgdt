//! `bytea`: both input forms read, the hex form written, and why `byteain`
//! refuses a text.

use crate::decode::Unread;
use crate::value::digits::HEX_NIBBLE;
use crate::value::digits::push_hex_pair;

/// Why [`decode_bytea`] read no value from `s`: `byteain`
/// refuses it, or reads a spelling this build does not — a blank between hex
/// pairs, any byte the `escape` form writes escaped written as itself, or the
/// reverse (`docs/design/decisions.md`, "D55").
// pg-refuses: I69 — every refusal here is `byteain`'s.
pub fn bytea_unread(s: &str) -> Unread {
    let b = s.as_bytes();
    let reads = match b.strip_prefix(b"\\x") {
        // `hex_decode_safe`: a blank between pairs, never inside one.
        Some(hex) => {
            let mut digits = hex.iter().filter(|&&c| !matches!(c, b' ' | b'\n' | b'\t' | b'\r'));
            let mut pairs = hex.split(|&c| matches!(c, b' ' | b'\n' | b'\t' | b'\r'));
            digits.all(u8::is_ascii_hexdigit) && pairs.all(|run| run.len() % 2 == 0)
        }
        None => {
            let mut at = 0;
            loop {
                match b.get(at..) {
                    Some([]) | None => break true,
                    Some([b'\\', b'0'..=b'3', b'0'..=b'7', b'0'..=b'7', ..]) => at += 4,
                    Some([b'\\', b'\\', ..]) => at += 2,
                    Some([b'\\', ..]) => break false,
                    Some(_) => at += 1,
                }
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
        Some(hex) => decode_bytea_hex(hex),
        None => decode_bytea_escape(s, usize::MAX),
    }
}

/// The hex pairs after the `\x`.
fn decode_bytea_hex(hex: &str) -> Option<Vec<u8>> {
    let b = hex.as_bytes();
    if !b.len().is_multiple_of(2) {
        return None;
    }
    // The whole field is one `bytea` value, so validity is accumulated and
    // tested once rather than branched on per pair; a bad nibble poisons the
    // high bits of `bad` and the vector is dropped unread.
    let mut out = Vec::with_capacity(b.len() / 2);
    let mut bad = 0u8;
    for [first, second] in b.as_chunks::<2>().0 {
        let hi = HEX_NIBBLE[*first as usize];
        let lo = HEX_NIBBLE[*second as usize];
        bad |= hi | lo;
        out.push(hi << 4 | lo);
    }
    if bad & 0xF0 != 0 {
        return None;
    }
    Some(out)
}

/// `byteaout`'s `escape` form, and no wider (`docs/design/decisions.md`,
/// "D55"): a byte from space to `~` stands for itself but `\`, which is
/// `\\`, and every other byte is `\ooo`. Only the first `limit` bytes are
/// kept, the whole text still checked, so a caller wanting a value's head
/// pays for no more of it.
pub fn decode_bytea_escape(s: &str, limit: usize) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len().min(limit));
    let mut at = 0;
    while let Some(&first) = b.get(at) {
        let (byte, width) = match first {
            b'\\' => match b.get(at + 1..at + 4) {
                _ if b.get(at + 1) == Some(&b'\\') => (b'\\', 2),
                Some(&[o1 @ b'0'..=b'3', o2 @ b'0'..=b'7', o3 @ b'0'..=b'7']) => {
                    let byte = (o1 - b'0') << 6 | (o2 - b'0') << 3 | (o3 - b'0');
                    if ESCAPE_PRINTS_ITSELF.contains(&byte) {
                        return None;
                    }
                    (byte, 4)
                }
                _ => return None,
            },
            byte if ESCAPE_PRINTS_ITSELF.contains(&byte) => (byte, 1),
            _ => return None,
        };
        if out.len() < limit {
            out.push(byte);
        }
        at += width;
    }
    Some(out)
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
