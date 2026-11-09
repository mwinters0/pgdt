//! **The slice readers a `numeric` field was read by**, which
//! [`NumericIn`](super::NumericIn) replaced: kept so `differential.rs` can
//! hold the machine to them over generated and damaged texts
//! (`key/order/differential.rs`).

use arrow_buffer::i256;

use super::input::{NUMERIC_INTEGER_BITS_MAX, radix_to_decimal, typmod_round};
use super::{
    NUMERIC_DSCALE_MAX, NUMERIC_INTEGER_DIGITS_MAX, NumericColumn, NumericKey, numeric_in_stores,
};
use crate::compare::CompareKind;
use crate::decode::{Read, Unread};
use crate::key::order::{OrderKey, special_order_key};
use crate::value::libc::is_c_space;

/// `field_key`'s arm for each `numeric` kind.
pub(crate) fn field_key(kind: &CompareKind, text: &str) -> Read<OrderKey> {
    if let Some(special) = special_order_key(kind, text) {
        return Ok(special);
    }
    let unparsed = Unread::Unparsed;
    Ok(match kind {
        CompareKind::Decimal { precision, scale } => OrderKey::Decimal(
            i256::from_string(&typmod_unscaled_digits(text, *precision, i16::from(*scale))?)
                .ok_or(unparsed)?,
        ),
        CompareKind::Numeric { typmod: Some(typmod), .. } => {
            OrderKey::Numeric(NumericKey::from_unscaled(
                &typmod_unscaled_digits(text, typmod.precision, typmod.scale)?,
                typmod.scale,
            ))
        }
        CompareKind::Numeric { infinities: bare, typmod: None } => {
            let key = parse(text).ok_or_else(|| {
                let column = if *bare { NumericColumn::Bare } else { NumericColumn::UnreadTypmod };
                numeric_unread(text, column)
            })?;
            if *bare && !numeric_in_stores(text) {
                return Err(Unread::Refused);
            }
            OrderKey::Numeric(key)
        }
        _ => unreachable!("only a `numeric` is held to its slice readers"),
    })
}

pub(crate) fn typmod_unscaled_digits(s: &str, precision: u16, scale: i16) -> Read<String> {
    let (neg, rest) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (int_part, frac_part) = rest.split_once('.').unwrap_or((rest, ""));
    if (int_part.is_empty() && frac_part.is_empty())
        || !int_part.bytes().all(|b| b.is_ascii_digit())
        || !frac_part.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(numeric_unread(s, NumericColumn::Typmod { precision, scale }));
    }
    typmod_round(neg, int_part, frac_part, precision, scale)
}

pub(crate) fn parse(text: &str) -> Option<NumericKey> {
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (int, frac) = rest.split_once('.').unwrap_or((rest, ""));
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    if !int.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(NumericKey::from_parts(negative, int, frac))
}

/// Why `numeric_in` reads no value of `column` here from `text`, a reader
/// having read none: refused only where every major refuses it. From v16 it
/// reads a non-decimal integer and an underscore between digits, and before
/// it an exponent `strtol` reads, a blank after the `e` included, and refuses
/// one at `PG_INT32_MAX / 2` that v16 reads, so neither grammar holds the
/// other's every text (I82). Each value read is then held to `column`.
fn numeric_unread(text: &str, column: NumericColumn) -> Unread {
    let b = text.as_bytes();
    let refused = |spelling: Option<NumericSpelling>| spelling.is_none_or(|s| !s.stored(column));
    if refused(numeric_spelling_v14(b)) && refused(numeric_spelling_v16(b)) {
        Unread::Refused
    } else {
        Unread::Unparsed
    }
}

/// A text `numeric_in` reads, as it reads it.
enum NumericSpelling {
    NaN,
    Infinite,
    /// ASCII `digits`, the point after the first `point` of them — before
    /// the first where negative, past the last where beyond — and the
    /// display scale it stores.
    Decimal {
        negative: bool,
        digits: Vec<u8>,
        point: i64,
        dscale: i64,
    },
    /// An integer of ASCII `digits` in `radix`.
    Radix {
        negative: bool,
        digits: Vec<u8>,
        radix: u32,
    },
}

impl NumericSpelling {
    /// Whether `column` stores the value: within the storage format's bounds
    /// for a bare one (I63), within the precision once rounded to the scale
    /// for a typmod'd one (I51), and an infinity under no typmod (I34).
    fn stored(self, column: NumericColumn) -> bool {
        match (self, column) {
            (Self::NaN, _) | (_, NumericColumn::UnreadTypmod) => true,
            (Self::Infinite, column) => column == NumericColumn::Bare,
            (Self::Decimal { negative, digits, point, dscale }, column) => {
                let Some(first) = digits.iter().position(|&d| d != b'0') else {
                    return column != NumericColumn::Bare || dscale <= NUMERIC_DSCALE_MAX as i64;
                };
                let NumericColumn::Typmod { precision, scale } = column else {
                    return dscale <= NUMERIC_DSCALE_MAX as i64
                        && point - first as i64 <= NUMERIC_INTEGER_DIGITS_MAX as i64;
                };
                // The digits the scale keeps end at `cut`: none rounds to
                // zero, or to one unit of the scale; more than the precision
                // can only grow by rounding.
                let cut = point + i64::from(scale);
                if cut <= 0 {
                    return true;
                }
                if cut - first as i64 > i64::from(precision) {
                    return false;
                }
                // Bounded by the precision and the scale, so small.
                let len = digits.len() as i64;
                let zeros = |n: i64| "0".repeat(usize::try_from(n).unwrap_or(0));
                let digits = std::str::from_utf8(&digits).expect("ASCII digits");
                let (int, frac) = match point {
                    _ if point <= 0 => (String::new(), format!("{}{digits}", zeros(-point))),
                    _ if point >= len => (format!("{digits}{}", zeros(point - len)), String::new()),
                    _ => {
                        let (int, frac) = digits.split_at(point as usize);
                        (int.to_string(), frac.to_string())
                    }
                };
                typmod_round(negative, &int, &frac, precision, scale).is_ok()
            }
            (Self::Radix { negative, digits, radix }, column) => {
                let Some(first) = digits.iter().position(|&d| d != b'0') else {
                    return true;
                };
                let digits = &digits[first..];
                let lead = char::from(digits[0]).to_digit(radix).expect("a digit of the radix");
                let bits = (digits.len() - 1) * radix.trailing_zeros() as usize
                    + (32 - lead.leading_zeros()) as usize;
                let decimal = match column {
                    NumericColumn::Bare if bits > NUMERIC_INTEGER_BITS_MAX => return false,
                    NumericColumn::Bare if bits < NUMERIC_INTEGER_BITS_MAX => return true,
                    NumericColumn::Bare => {
                        return radix_to_decimal(digits, radix).len() <= NUMERIC_INTEGER_DIGITS_MAX;
                    }
                    // Past four bits a digit it is past the precision
                    // whatever the scale rounds.
                    NumericColumn::Typmod { precision, scale }
                        if bits as i64 > 4 * (i64::from(precision) - i64::from(scale) + 1) =>
                    {
                        return false;
                    }
                    NumericColumn::Typmod { .. } => radix_to_decimal(digits, radix),
                    NumericColumn::UnreadTypmod => unreachable!("answered above"),
                };
                let NumericColumn::Typmod { precision, scale } = column else {
                    unreachable!("answered above")
                };
                typmod_round(negative, &decimal, "", precision, scale).is_ok()
            }
        }
    }
}

/// Whether `b` opens at `at` with `word`, in either case, as
/// `pg_strncasecmp` compares.
fn opens_with(b: &[u8], at: usize, word: &[u8]) -> bool {
    b.get(at..at + word.len()).is_some_and(|s| s.eq_ignore_ascii_case(word))
}

/// The value `numeric_in` reads from `b` in v14 and v15, or `None` where it
/// refuses it as syntax or as an exponent out of bounds: `set_var_from_str`
/// reading its exponent by `strtol`. v13 reads the same texts but the
/// infinities.
fn numeric_spelling_v14(b: &[u8]) -> Option<NumericSpelling> {
    let blank = |at: usize| b.get(at..).is_some_and(|rest| rest.iter().all(|&c| is_c_space(c)));
    let at = b.iter().take_while(|&&c| is_c_space(c)).count();
    if opens_with(b, at, b"NaN") {
        return blank(at + 3).then_some(NumericSpelling::NaN);
    }
    for word in [&b"Infinity"[..], b"+Infinity", b"-Infinity", b"inf", b"+inf", b"-inf"] {
        if opens_with(b, at, word) {
            return blank(at + word.len()).then_some(NumericSpelling::Infinite);
        }
    }
    let negative = b.get(at) == Some(&b'-');
    let mut at = at + usize::from(matches!(b.get(at), Some(b'+' | b'-')));
    let (mut digits, mut integer, mut point) = (Vec::new(), 0i64, false);
    if b.get(at) == Some(&b'.') {
        point = true;
        at += 1;
    }
    if !b.get(at).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    while let Some(&c) = b.get(at) {
        match c {
            b'0'..=b'9' => {
                digits.push(c);
                integer += i64::from(!point);
            }
            b'.' if point => return None,
            b'.' => point = true,
            _ => break,
        }
        at += 1;
    }
    let mut dscale = digits.len() as i64 - integer;
    let mut exponent = 0i64;
    if matches!(b.get(at), Some(b'e' | b'E')) {
        // `strtol`: blanks, a sign, then at least one digit.
        let mut cp = at + 1;
        cp += b[cp..].iter().take_while(|&&c| is_c_space(c)).count();
        let minus = b.get(cp) == Some(&b'-');
        cp += usize::from(matches!(b.get(cp), Some(b'+' | b'-')));
        let run = b[cp..].iter().take_while(|c| c.is_ascii_digit()).count();
        if run == 0 {
            return None;
        }
        let magnitude = b[cp..cp + run]
            .iter()
            .fold(0i64, |n, d| n.saturating_mul(10).saturating_add(i64::from(d - b'0')));
        exponent = if minus { -magnitude } else { magnitude };
        if exponent.abs() >= i64::from(i32::MAX / 2) {
            return None;
        }
        at = cp + run;
        dscale = (dscale - exponent).max(0);
    }
    blank(at).then_some(NumericSpelling::Decimal {
        negative,
        digits,
        point: integer + exponent,
        dscale,
    })
}

/// The value `numeric_in` reads from `b` from v16, or `None` where it refuses
/// it as syntax or as an exponent out of bounds: a non-decimal integer after
/// `0x`, `0o` or `0b`, and an underscore between digits, the exponent's too.
fn numeric_spelling_v16(b: &[u8]) -> Option<NumericSpelling> {
    let blank = |at: usize| b.get(at..).is_some_and(|rest| rest.iter().all(|&c| is_c_space(c)));
    let start = b.iter().take_while(|&&c| is_c_space(c)).count();
    let negative = b.get(start) == Some(&b'-');
    let mut at = start + usize::from(matches!(b.get(start), Some(b'+' | b'-')));
    let digit = |at: usize| b.get(at).is_some_and(u8::is_ascii_digit);
    if !digit(at) && b.get(at) != Some(&b'.') {
        // `NaN` takes no sign.
        let (end, spelling) = if opens_with(b, start, b"NaN") {
            (start + 3, NumericSpelling::NaN)
        } else if opens_with(b, at, b"Infinity") {
            (at + 8, NumericSpelling::Infinite)
        } else if opens_with(b, at, b"inf") {
            (at + 3, NumericSpelling::Infinite)
        } else {
            return None;
        };
        return blank(end).then_some(spelling);
    }
    let radix = match b.get(at..at + 2) {
        Some([b'0', b'x' | b'X']) => 16,
        Some([b'0', b'o' | b'O']) => 8,
        Some([b'0', b'b' | b'B']) => 2,
        _ => 10,
    };
    if radix != 10 {
        at += 2;
        let first = at;
        let of_radix = |at: usize| b.get(at).is_some_and(|&c| char::from(c).is_digit(radix));
        let mut digits = Vec::new();
        loop {
            if of_radix(at) {
                digits.push(b[at]);
            } else if b.get(at) == Some(&b'_') && of_radix(at + 1) {
                // An underscore is followed by a digit, which is read next.
            } else if b.get(at) == Some(&b'_') {
                return None;
            } else {
                break;
            }
            at += 1;
        }
        return (at > first && blank(at)).then_some(NumericSpelling::Radix {
            negative,
            digits,
            radix,
        });
    }
    let (mut digits, mut integer, mut point) = (Vec::new(), 0i64, false);
    if b.get(at) == Some(&b'.') {
        point = true;
        at += 1;
    }
    if !digit(at) {
        return None;
    }
    while let Some(&c) = b.get(at) {
        match c {
            b'0'..=b'9' => {
                digits.push(c);
                integer += i64::from(!point);
            }
            b'.' if point || b.get(at + 1) == Some(&b'_') => return None,
            b'.' => point = true,
            b'_' if digit(at + 1) => {}
            b'_' => return None,
            _ => break,
        }
        at += 1;
    }
    let mut dscale = digits.len() as i64 - integer;
    let mut exponent = 0i64;
    if matches!(b.get(at), Some(b'e' | b'E')) {
        at += 1;
        let minus = b.get(at) == Some(&b'-');
        at += usize::from(matches!(b.get(at), Some(b'+' | b'-')));
        if !digit(at) {
            return None;
        }
        while let Some(&c) = b.get(at) {
            match c {
                b'0'..=b'9' => {
                    exponent = exponent * 10 + i64::from(c - b'0');
                    if exponent > i64::from(i32::MAX / 2) {
                        return None;
                    }
                }
                b'_' if digit(at + 1) => {}
                b'_' => return None,
                _ => break,
            }
            at += 1;
        }
        if minus {
            exponent = -exponent;
        }
        dscale = (dscale - exponent).max(0);
    }
    blank(at).then_some(NumericSpelling::Decimal {
        negative,
        digits,
        point: integer + exponent,
        dscale,
    })
}
