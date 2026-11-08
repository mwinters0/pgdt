//! **The order key's slice readers** an integer, `oid`, `boolean`, an enum
//! and `uuid` were read by, which the push machines in [`super`] replaced:
//! kept so `differential.rs` can hold each machine to them over generated
//! and damaged texts.

use crate::compare::CompareKind;
use crate::decode::{Read, Unread};
use crate::key::order::OrderKey;
use crate::value::digits::HEX_NIBBLE;
use crate::value::int::int_range;
use crate::value::libc::is_c_space;

/// `field_key`'s arm for each of the five kinds.
pub(super) fn field_key(kind: &CompareKind, text: &str) -> Read<OrderKey> {
    Ok(match kind {
        CompareKind::Bool => OrderKey::Bool(decode_bool(text).ok_or_else(|| bool_unread(text))?),
        CompareKind::Int { bytes } => OrderKey::Int(int_in(text, *bytes)?),
        CompareKind::UnsignedInt => {
            OrderKey::Int(text.parse::<u32>().map_err(|_| oid_unread(text))?.into())
        }
        CompareKind::Enum { labels, exact, .. } => {
            let position = labels.iter().position(|l| l == text);
            let refused = *exact || text.len() >= 64;
            let unread = if refused { Unread::Refused } else { Unread::Unparsed };
            OrderKey::Int(position.ok_or(unread)? as i64)
        }
        CompareKind::Uuid => OrderKey::Bytes(decode_uuid(text).ok_or(Unread::Refused)?.to_vec()),
        _ => unreachable!("only the five kinds are held to their slice readers"),
    })
}

fn int_unread(text: &str, range: std::ops::RangeInclusive<i64>) -> Unread {
    match pg_strtoint(text.as_bytes()) {
        Some(value) if i128::from(*range.start()) <= value && value <= i128::from(*range.end()) => {
            Unread::Unparsed
        }
        _ => Unread::Refused,
    }
}

/// `pg_strtoint64_safe`'s grammar from v16, its slow path: the value of `b`,
/// its magnitude saturated past any width, or `None` where it refuses `b` as
/// syntax. An underscore stands between two digits, and may follow a prefix
/// but not open a decimal.
fn pg_strtoint(b: &[u8]) -> Option<i128> {
    let mut at = b.iter().take_while(|&&c| is_c_space(c)).count();
    let negative = b.get(at) == Some(&b'-');
    at += usize::from(matches!(b.get(at), Some(b'+' | b'-')));
    let radix = match b.get(at..at + 2) {
        Some([b'0', b'x' | b'X']) => 16,
        Some([b'0', b'o' | b'O']) => 8,
        Some([b'0', b'b' | b'B']) => 2,
        _ => 10,
    };
    if radix != 10 {
        at += 2;
    }
    let first = at;
    let digit = |at: usize| b.get(at).and_then(|&c| char::from(c).to_digit(radix));
    let mut magnitude: i128 = 0;
    loop {
        if let Some(d) = digit(at) {
            magnitude = magnitude.saturating_mul(i128::from(radix)).saturating_add(i128::from(d));
            at += 1;
        } else if b.get(at) == Some(&b'_') {
            if radix == 10 && at == first {
                return None;
            }
            at += 1;
            digit(at)?;
        } else {
            break;
        }
    }
    if at == first || !b[at..].iter().all(|&c| is_c_space(c)) {
        return None;
    }
    Some(if negative { -magnitude } else { magnitude })
}

fn oid_unread(s: &str) -> Unread {
    let b = s.as_bytes();
    let read = [(false, false), (true, false), (true, true)]
        .into_iter()
        .any(|(base0, binary)| oid_in(b, base0, binary).is_some());
    if read { Unread::Unparsed } else { Unread::Refused }
}

/// `uint32in_subr` over glibc's `strtoul`, in base 0 where `base0` holds and
/// base 10 where not, a `0b` prefix read where `binary` does: the oid it
/// reads, or `None` where it refuses the text. A negative value is read where
/// it fits an `int`, wrapped.
fn oid_in(b: &[u8], base0: bool, binary: bool) -> Option<u32> {
    let mut at = b.iter().take_while(|&&c| is_c_space(c)).count();
    let negative = matches!(b.get(at), Some(b'-'));
    if matches!(b.get(at), Some(b'+' | b'-')) {
        at += 1;
    }
    let base = match (base0, b.get(at..)) {
        (true, Some([b'0', b'x' | b'X', d, ..])) if d.is_ascii_hexdigit() => 16,
        (true, Some([b'0', b'b' | b'B', b'0' | b'1', ..])) if binary => 2,
        (true, Some([b'0', ..])) => 8,
        _ => 10,
    };
    if matches!(base, 16 | 2) {
        at += 2;
    }
    let digits = b[at..].iter().take_while(|&&c| char::from(c).is_digit(base)).count();
    if digits == 0 {
        return None;
    }
    let mut magnitude = 0u64;
    for &c in &b[at..at + digits] {
        // Past `ULONG_MAX`, `ERANGE`.
        let digit = u64::from(char::from(c).to_digit(base)?);
        magnitude = magnitude.checked_mul(u64::from(base))?.checked_add(digit)?;
    }
    if !b[at + digits..].iter().all(|&c| is_c_space(c)) {
        return None;
    }
    let cvt = if negative { magnitude.wrapping_neg() } else { magnitude };
    let oid = cvt as u32;
    (cvt == u64::from(oid) || cvt == oid as i32 as i64 as u64).then_some(oid)
}

fn int_in(text: &str, bytes: u8) -> Read<i64> {
    let value = text.parse::<i64>().map_err(|e| match e.kind() {
        std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow => {
            Unread::Refused
        }
        _ => int_unread(text, int_range(bytes)),
    })?;
    if int_range(bytes).contains(&value) { Ok(value) } else { Err(Unread::Refused) }
}

fn decode_bool(s: &str) -> Option<bool> {
    match s {
        "t" => Some(true),
        "f" => Some(false),
        _ => None,
    }
}

fn bool_unread(s: &str) -> Unread {
    let word = s.trim_matches(|c: char| c.is_ascii() && is_c_space(c as u8));
    // `parse_bool_with_len`: the word is a prefix of the one its first
    // letter opens, and `o` alone opens both `on` and `off`.
    let prefix_of =
        |full: &str| word.len() <= full.len() && word.eq_ignore_ascii_case(&full[..word.len()]);
    let reads = match word.as_bytes().first() {
        Some(b't' | b'T') => prefix_of("true"),
        Some(b'f' | b'F') => prefix_of("false"),
        Some(b'y' | b'Y') => prefix_of("yes"),
        Some(b'n' | b'N') => prefix_of("no"),
        Some(b'o' | b'O') => word.len() >= 2 && (prefix_of("on") || prefix_of("off")),
        Some(b'0' | b'1') => word.len() == 1,
        _ => false,
    };
    if reads { Unread::Unparsed } else { Unread::Refused }
}

fn decode_uuid(s: &str) -> Option<[u8; 16]> {
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
