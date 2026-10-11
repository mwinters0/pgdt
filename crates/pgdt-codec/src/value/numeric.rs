//! `numeric`: a field put through its typmod, both majors' spellings, the
//! storage format's bounds, the decimal renderer, and its order key
//! ([`NumericKey`]).

use std::cmp::Ordering;

use crate::decode::{Read, Unread};
use crate::value::libc::is_c_space;

/// `NUMERIC_DSCALE_MAX`: the most digits `numeric_in` stores after the point,
/// a trailing zero counting (I63).
pub const NUMERIC_DSCALE_MAX: usize = 16383;

/// The most digits `numeric_in` stores before the point, a leading zero not
/// counting: `NUMERIC_WEIGHT_MAX`, `i16::MAX`, base-10000 digits past the
/// first (I63).
pub const NUMERIC_INTEGER_DIGITS_MAX: usize = 4 * (i16::MAX as usize + 1);

/// Whether `numeric_in` with no typmod stores the number written
/// `[-]int[.frac]` rather than refusing it as overflowing the format (I63):
/// every digit after the point counts toward its display scale, and its
/// weight counts from the first non-zero digit before it. Only the two bounds
/// are checked; the grammar is its reader's.
///
/// What it bounds is a bare `numeric`'s field and every literal, which the
/// server coerces with no typmod. A typmod'd column's field is rounded to its
/// scale before the bounds are checked, and never reaches them (I51).
// pg-refuses: I63 — a display scale or a weight past the storage format's.
pub fn numeric_in_stores(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    frac.len() <= NUMERIC_DSCALE_MAX
        && (int.len() <= NUMERIC_INTEGER_DIGITS_MAX
            || int.trim_start_matches('0').len() <= NUMERIC_INTEGER_DIGITS_MAX)
}

/// A filter literal compared with a `numeric(p,s)` column, as an unscaled
/// integer digit string at `scale` decimal places — the form
/// [`i128::from_str`]/[`i256::from_str`] accept directly. **Exact**: the
/// server coerces a literal with no typmod (I63), so a digit finer than the
/// scale is no value this key can carry and is refused rather than rounded
/// (`docs/design/decisions.md`, "D55"), and no precision applies. A field is
/// read by [`typmod_unscaled_digits`] instead. `None` for `NaN`, which a
/// decimal has no representation for.
///
/// Digits are shifted toward `scale` — at a negative scale (PostgreSQL 15
/// and later) the implied trailing zeros are divided out rather than
/// appended, and an all-zero digit string answers `0`, which is how
/// `numeric_out` prints zero at any scale (I51).
pub fn decimal_unscaled_digits(s: &str, scale: i8) -> Option<String> {
    if s == "NaN" {
        return None;
    }
    let (neg, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (int_part, frac_part) = s.split_once('.').unwrap_or((s, ""));
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    if !int_part.bytes().all(|b| b.is_ascii_digit())
        || !frac_part.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    // `int_part ++ frac_part` is the digit string, and it is never
    // materialized: `digit(i)` indexes into whichever half `i` falls in, so
    // the whole of what follows — drop `cut` trailing digits, trim leading
    // zeros, append `pad` — is arithmetic on indices and the answer is
    // written once, into a `String` of its final length.
    let total = int_part.len() + frac_part.len();
    let digit = |i: usize| {
        if i < int_part.len() {
            int_part.as_bytes()[i]
        } else {
            frac_part.as_bytes()[i - int_part.len()]
        }
    };
    let shift = i32::from(scale) - frac_part.len() as i32;
    let (keep, pad) = if shift >= 0 {
        (total, shift as usize)
    } else {
        let cut = (-shift) as usize;
        if cut > total {
            return (0..total).all(|i| digit(i) == b'0').then(|| "0".to_string());
        }
        let keep = total - cut;
        // The digits divided out have to be the zeros the typmod implies; a
        // non-zero among them means the text is not a value of this column.
        if (keep..total).any(|i| digit(i) != b'0') {
            return None;
        }
        (keep, 0)
    };
    let mut start = 0;
    while start < keep && digit(start) == b'0' {
        start += 1;
    }
    // All-zero digits collapse to `"0"` whatever the padding would have
    // been, and carry no sign: `-0.00` at scale 2 is `0`, not `-000`.
    if start == keep {
        return Some("0".to_string());
    }
    let mut out = String::with_capacity(usize::from(neg) + keep - start + pad);
    if neg {
        out.push('-');
    }
    out.extend((start..keep).map(|i| char::from(digit(i))));
    out.extend(std::iter::repeat_n('0', pad));
    Some(out)
}

/// A `numeric(p,s)` field as `COPY` stores it, as an unscaled integer digit
/// string at `scale` decimal places: `apply_typmod` rounds it to the scale,
/// half away from zero, and then refuses one holding more than `precision`
/// digits (I51). `numeric_out` writes every stored value at its scale already,
/// so this rounds nothing a `pg_dump` wrote; a hand-written field is rounded
/// as a restore would store it, `1.005` in a `numeric(10,2)` reading as
/// `1.01`.
///
/// The grammar is [`decimal_unscaled_digits`]'s, `[-]digits[.digits]`; a
/// value past the precision is [`Unread::Refused`], and so is a text
/// `numeric_in` refuses at every major ([`numeric_unread`]), any other being
/// [`Unread::Unparsed`] — `NaN` among them, which bypasses the typmod and has
/// no decimal representation. `scale` is `numerictypmodin`'s whole range,
/// -1000 to 1000, so a column held as text for a scale no Arrow decimal
/// carries is read by this too.
pub fn typmod_unscaled_digits(s: &str, precision: u16, scale: i16) -> Read<String> {
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

/// [`typmod_unscaled_digits`] once the text is split: `int_part` and
/// `frac_part` ASCII digits, not both empty.
// pg-refuses: I51 — a value past the precision once rounded to the scale.
fn typmod_round(
    neg: bool,
    int_part: &str,
    frac_part: &str,
    precision: u16,
    scale: i16,
) -> Read<String> {
    // As in `decimal_unscaled_digits`, `int_part ++ frac_part` is indexed and
    // never materialized: the unscaled value is its first `keep` digits,
    // incremented where the first digit past the scale is 5 or more, then
    // `pad` zeros.
    let total = int_part.len() + frac_part.len();
    let digit = |i: usize| {
        if i < int_part.len() {
            int_part.as_bytes()[i]
        } else {
            frac_part.as_bytes()[i - int_part.len()]
        }
    };
    let shift = i64::from(scale) - frac_part.len() as i64;
    let (keep, pad, round_up) = match usize::try_from(-shift) {
        // Every digit past the scale is cut. Past the last written one, the
        // first cut digit is an implied leading zero, which rounds down.
        Ok(cut) if cut > total => (0, 0, false),
        Ok(cut) => (total - cut, 0, cut > 0 && digit(total - cut) >= b'5'),
        Err(_) => (total, usize::try_from(shift).expect("a positive shift"), false),
    };
    let start = (0..keep).find(|&i| digit(i) != b'0').unwrap_or(keep);
    // Zero, rounded or written, carries no sign: `-0.004` at scale 2 is `0`.
    if start == keep && !round_up {
        return Ok("0".to_string());
    }
    let sign = usize::from(neg);
    let mut out = String::with_capacity(sign + keep - start + 1 + pad);
    if neg {
        out.push('-');
    }
    out.extend((start..keep).map(|i| char::from(digit(i))));
    if round_up {
        // The carry turns a run of trailing nines into zeros and raises the
        // digit before them, or adds a leading `1` where every digit was a
        // nine: rounding can raise the weight, which is why the precision is
        // checked after it.
        let nines = out[sign..].bytes().rev().take_while(|&b| b == b'9').count();
        out.truncate(out.len() - nines);
        if out.len() > sign {
            let last = out.pop().expect("a digit is past the sign");
            out.push(char::from(last as u8 + 1));
        } else {
            out.push('1');
        }
        out.extend(std::iter::repeat_n('0', nines));
    }
    if out.len() - sign + pad > usize::from(precision) {
        return Err(Unread::Refused);
    }
    out.extend(std::iter::repeat_n('0', pad));
    Ok(out)
}

/// What a `numeric` column's declaration does with a value `numeric_in` reads:
/// keeps it within the storage format's bounds (I63), rounds it to `scale` and
/// holds it to `precision` (I51), or a typmod this build does not read, which
/// does one of those to a value it cannot tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NumericColumn {
    Bare,
    Typmod { precision: u16, scale: i16 },
    UnreadTypmod,
}

/// Why `numeric_in` reads no value of `column` here from `text`, a reader
/// having read none: refused only where every major refuses it. From v16 it
/// reads a non-decimal integer and an underscore between digits, and before
/// it an exponent `strtol` reads, a blank after the `e` included, and refuses
/// one at `PG_INT32_MAX / 2` that v16 reads, so neither grammar holds the
/// other's every text (I82). Each value read is then held to `column`.
pub(crate) fn numeric_unread(text: &str, column: NumericColumn) -> Unread {
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

/// The most bits a value with [`NUMERIC_INTEGER_DIGITS_MAX`] digits can need,
/// one more than `10^131072`'s width less one: it is the one width the two
/// sides of the bound share.
const NUMERIC_INTEGER_BITS_MAX: usize = 435_412;

impl NumericSpelling {
    /// Whether `column` stores the value: within the storage format's bounds
    /// for a bare one (I63), within the precision once rounded to the scale
    /// for a typmod'd one (I51), and an infinity under no typmod (I34).
    // pg-refuses: I63 — a display scale or a weight past the storage format's.
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

/// An integer's ASCII `digits` in `radix` (2, 8 or 16), written in decimal.
/// Quadratic in their number, which a caller bounds.
fn radix_to_decimal(digits: &[u8], radix: u32) -> String {
    const LIMB: u64 = 1_000_000_000;
    // As many digits a step as keep the multiplier at most 2^32.
    let step = (32 / radix.trailing_zeros()) as usize;
    let mut limbs: Vec<u64> = vec![0];
    for chunk in digits.chunks(step) {
        let multiplier = u64::from(radix).pow(chunk.len() as u32);
        let mut carry = chunk.iter().fold(0u64, |n, &d| {
            n * u64::from(radix) + u64::from(char::from(d).to_digit(radix).expect("a digit"))
        });
        for limb in &mut limbs {
            let value = *limb * multiplier + carry;
            *limb = value % LIMB;
            carry = value / LIMB;
        }
        while carry > 0 {
            limbs.push(carry % LIMB);
            carry /= LIMB;
        }
    }
    let mut out = limbs.last().expect("one limb at least").to_string();
    for limb in limbs.iter().rev().skip(1) {
        out.push_str(&format!("{limb:09}"));
    }
    out
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
// pg-refuses: I82 — every refusal here is `numeric_in`'s before v16.
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
// pg-refuses: I82 — every refusal here is `numeric_in`'s from v16.
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

/// Render an unscaled decimal integer (`i128`/`i256`'s own `Display`, e.g.
/// `"-15000000000"`) back to PostgreSQL's fixed-`scale` text form. Exact
/// inverse of [`decimal_unscaled_digits`]: at a scale of zero or below, zero
/// is `0` rather than a zero followed by the scale's.
pub fn render_decimal(unscaled: &str, scale: i8) -> String {
    let (neg, digits) = match unscaled.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, unscaled),
    };
    let body = if scale <= 0 {
        if digits == "0" {
            return digits.to_string();
        }
        format!("{digits}{}", "0".repeat(usize::from(scale.unsigned_abs())))
    } else {
        let scale = scale as usize;
        let padded = if digits.len() <= scale {
            format!("{}{digits}", "0".repeat(scale + 1 - digits.len()))
        } else {
            digits.to_string()
        };
        let split = padded.len() - scale;
        format!("{}.{}", &padded[..split], &padded[split..])
    };
    if neg { format!("-{body}") } else { body }
}

/// One side of a bare-`numeric` comparison: the sign, and the digits either
/// side of the point with the *insignificant* ones removed — leading zeros
/// from the integer part, trailing zeros from the fraction.
///
/// Normalizing is what makes this PostgreSQL's own order: `cmp_numerics`
/// compares by value and never by display scale (I33), so `1.5` and `1.50`
/// are one value that a bare `numeric` column writes two ways.
///
/// Digit *strings* rather than a big integer, because the column is
/// arbitrary-precision: the file may hold a thousand digits, past every
/// fixed-width type including `i256`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumericKey {
    /// False for a zero of either written sign — PostgreSQL has one zero, and
    /// a filter literal is free to spell it `-0`.
    negative: bool,
    /// Integer digits, leading zeros stripped; empty for a magnitude below 1.
    int: String,
    /// Fraction digits, trailing zeros stripped; empty for an integer.
    frac: String,
}

impl NumericKey {
    /// The heap its two digit strings hold.
    pub(crate) fn heap_bytes(&self) -> u64 {
        (self.int.capacity() + self.frac.capacity()) as u64
    }

    /// `[-]digits[.digits]`: no exponent, no sign but `-`, at least one digit
    /// somewhere, and a point at either end or leading zeros allowed, which
    /// `numeric_out` never writes (`grammar::form::accepted_form` lists them). It is
    /// the same *lexical* grammar
    /// [`typmod_unscaled_digits`](crate::value::numeric::typmod_unscaled_digits)
    /// accepts for a typmod'd column's field, which is read through it and
    /// [`Self::from_unscaled`] instead; this reads a bare column's field and
    /// every literal. The storage format's bounds are the caller's
    /// ([`numeric_in_stores`](crate::value::numeric::numeric_in_stores)), a
    /// typmod'd column's field never reaching them.
    pub(crate) fn parse(text: &str) -> Option<Self> {
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
        Some(Self::from_parts(negative, int, frac))
    }

    /// An unscaled digit string at `scale` places, as
    /// [`typmod_unscaled_digits`](crate::value::numeric::typmod_unscaled_digits)
    /// writes it, split at its point.
    pub(crate) fn from_unscaled(unscaled: &str, scale: i16) -> Self {
        let (negative, digits) = match unscaled.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, unscaled),
        };
        match usize::try_from(scale) {
            Ok(places) if places >= digits.len() => Self::from_parts(
                negative,
                "",
                &format!("{}{digits}", "0".repeat(places - digits.len())),
            ),
            Ok(places) => {
                let (int, frac) = digits.split_at(digits.len() - places);
                Self::from_parts(negative, int, frac)
            }
            Err(_) => {
                let zeros = "0".repeat(usize::from(scale.unsigned_abs()));
                Self::from_parts(negative, &format!("{digits}{zeros}"), "")
            }
        }
    }

    /// Drop the insignificant digits from an already-split sign/integer part/
    /// fraction, which is the whole of the normalization. Shared: a `numeric`
    /// field is split straight out of the text, while a `jsonb` number's
    /// point has to be moved by its exponent first.
    pub(crate) fn from_parts(negative: bool, int: &str, frac: &str) -> Self {
        let int = int.trim_start_matches('0');
        let frac = frac.trim_end_matches('0');
        Self {
            negative: negative && !(int.is_empty() && frac.is_empty()),
            int: int.to_string(),
            frac: frac.to_string(),
        }
    }

    /// Sign first, then magnitude: how many integer digits, then those digits,
    /// then the fraction. Each stage is a byte comparison over ASCII digits,
    /// which is why the normalization above has to have happened — with
    /// trailing zeros stripped, a fraction that is a prefix of another is the
    /// smaller of the two.
    pub(crate) fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => return Ordering::Greater,
            (true, false) => return Ordering::Less,
            _ => {}
        }
        let magnitude = self
            .int
            .len()
            .cmp(&other.int.len())
            .then_with(|| self.int.as_bytes().cmp(other.int.as_bytes()))
            .then_with(|| self.frac.as_bytes().cmp(other.frac.as_bytes()));
        if self.negative { magnitude.reverse() } else { magnitude }
    }
}

#[cfg(test)]
mod tests;
