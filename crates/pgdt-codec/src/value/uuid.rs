//! `uuid`: its reader and its renderer.

use crate::value::digits::push_hex_pair;
use crate::value::libc::HEX_NIBBLE;

/// `uuid_in`'s grammar, field and literal alike: 32 hex digits in either case
/// (PostgreSQL always dumps the canonical lowercase `8-4-4-4-12`, but nothing
/// forces that on a hand-written dump), each group of four but the last
/// optionally followed by one hyphen, the whole optionally in braces.
///
/// It runs over bytes rather than chars: `-` and the braces are ASCII, and a
/// non-ASCII byte is not a hex digit either way.
// pg-refuses: I64 — every refusal here is `string_to_uuid`'s.
pub fn decode_uuid(s: &str) -> Option<[u8; 16]> {
    let b = match s.as_bytes() {
        [b'{', inner @ .., b'}'] => inner,
        b => b,
    };
    let mut bytes = [0u8; 16];
    let mut bad = 0u8;
    let mut at = 0;
    for (i, byte) in bytes.iter_mut().enumerate() {
        let hi = HEX_NIBBLE[*b.get(at)? as usize];
        let lo = HEX_NIBBLE[*b.get(at + 1)? as usize];
        bad |= hi | lo;
        *byte = hi << 4 | lo;
        at += 2;
        // One hyphen after a group of four but the last; a hyphen anywhere
        // else is a byte no hex digit matches.
        if i % 2 == 1 && i < 15 && b.get(at) == Some(&b'-') {
            at += 1;
        }
    }
    if bad & 0xF0 != 0 || at != b.len() {
        return None;
    }
    Some(bytes)
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
