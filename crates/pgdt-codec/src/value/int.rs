//! The integers and `oid`: why `int2in` to `int8in` and `oidin` refuse a
//! text, and the readers an order key and a strict parse read one by.

use crate::decode::{Read, Unread};
use crate::value::libc::{is_c_space, strtoint};

/// Why `int2in`, `int4in` or `int8in` reads no value within `range` here from
/// `text`, Rust's parse having read none: from v16 `pg_strtoint16`, `32` and
/// `64` read a decimal between blanks, an underscore between its digits and a
/// `0x`, `0o` or `0b` prefix besides — every text v13 to v15 read and more
/// (I80) — so a text it refuses is refused at every major, as is one it reads
/// past `range` (I60), which v13 to v15 refuse too, as syntax where it is
/// prefixed. What it reads within `range` is unparsed.
// pg-refuses: I80 — every refusal here is `pg_strtoint*`'s from v16.
pub(crate) fn int_unread(text: &str, range: std::ops::RangeInclusive<i64>) -> Unread {
    match strtoint(text.as_bytes()) {
        Some(value) if i128::from(*range.start()) <= value && value <= i128::from(*range.end()) => {
            Unread::Unparsed
        }
        _ => Unread::Refused,
    }
}

/// Why an `oid` this build does not read is no value: every supported
/// major's `oidin` refuses it, or one reads it, where this build reads the
/// decimal digits `oidout` writes, a leading `+` and zeros
/// (`docs/design/decisions.md`, "D55").
///
/// The majors part at v16, whose `uint32in_subr` hands `strtoul` base 0
/// where `oidin_subr` handed it base 10, so a `0x` or octal spelling reads
/// from v16 and a decimal one with a leading zero changes its value there;
/// a C23 glibc reads a `0b` prefix in base 0 too. A text one of them reads
/// is not refused: the exception `docs/design/roadmap.md`'s "A literal is
/// guaranteed in `*_out`'s form and never read past `*_in`'s" makes to the
/// newest major's reading.
// deficiency: KD84 — the readers of an `oid` (`str::parse`, in the typed read
// and `key::field_key`) take `010` for 10, as v15 and earlier do, where
// v16 and later read it as octal 8 and refuse `08` (I66). Which the dump's
// server meant is its version's to say, and no reader is handed it.
pub(crate) fn oid_unread(s: &str) -> Unread {
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
// pg-refuses: I66 — every refusal here is `oidin`'s.
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

/// A `smallint`, `integer` or `bigint`, of `bytes` bytes, field or literal,
/// read as `int2in`, `int4in` and `int8in` read it. Rust's parse is the
/// grammar: the digits `*_out` writes, and of `*_in`'s a leading `+` and
/// leading zeros. A blank around the number, an underscore between digits and
/// a `0x`, `0o` or `0b` prefix, which the server reads, are unparsed
/// (`docs/design/decisions.md`, "D55"), and every other text it does not read
/// is refused ([`int_unread`](crate::value::int::int_unread)).
///
/// **A value past the column's width is refused, as the server refuses it**
/// (I60): `70000` is no value of a `smallint` column, where a literal in
/// DataFusion's semantics is read as an `i64` ([`literal_key`](crate::key::literal_key)).
// pg-refuses: I60 — out of range for the column's width.
pub fn int_in(text: &str, bytes: u8) -> Read<i64> {
    let value = text.parse::<i64>().map_err(|e| match e.kind() {
        std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow => {
            Unread::Refused
        }
        _ => int_unread(text, int_range(bytes)),
    })?;
    if int_range(bytes).contains(&value) { Ok(value) } else { Err(Unread::Refused) }
}

/// The values a signed integer of `bytes` bytes holds.
pub(crate) fn int_range(bytes: u8) -> std::ops::RangeInclusive<i64> {
    match bytes {
        2 => i16::MIN.into()..=i16::MAX.into(),
        4 => i32::MIN.into()..=i32::MAX.into(),
        _ => i64::MIN..=i64::MAX,
    }
}

#[cfg(test)]
mod tests;
