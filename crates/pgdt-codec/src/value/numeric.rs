//! `numeric`: a field put through its typmod, read by [`NumericIn`] with
//! both majors' spellings beside it, the storage format's bounds, the decimal
//! renderer, and its order key ([`NumericKey`]).

use std::cmp::Ordering;

use crate::decode::Read;

mod input;
pub use input::{NUMERIC_DSCALE_MAX, NUMERIC_INTEGER_DIGITS_MAX};
pub(crate) use input::{NumericColumn, NumericIn};

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
/// value past the precision is [`Unread::Refused`](crate::decode::Unread::Refused), and so is a text
/// `numeric_in` refuses at every major, any other being
/// [`Unread::Unparsed`](crate::decode::Unread::Unparsed) — `NaN` among them, which bypasses the typmod and has
/// no decimal representation. `scale` is `numerictypmodin`'s whole range,
/// -1000 to 1000, so a column held as text for a scale no Arrow decimal
/// carries is read by this too. It is [`NumericIn`] fed `s` whole.
pub fn typmod_unscaled_digits(s: &str, precision: u16, scale: i16) -> Read<String> {
    NumericIn::whole(NumericColumn::Typmod { precision, scale }, s.as_bytes()).unscaled()
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

    /// A filter literal, read by [`NumericIn`]'s grammar with no bound: the
    /// storage format's bounds are the caller's
    /// ([`numeric_in_stores`](crate::value::numeric::numeric_in_stores)). A
    /// field is read by `key::order::field_key`.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let read = NumericIn::whole(NumericColumn::UnreadTypmod, text.as_bytes());
        let (negative, int, frac) = read.parts().ok()?;
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

    /// The `double precision` `numeric_float8` casts this to, which is
    /// `float8in` reading its text: the nearest double, as `strtod` rounds,
    /// and `None` where that overflows or a nonzero value rounds to zero,
    /// both of which `float8in` refuses as out of range.
    pub(crate) fn to_f64(&self) -> Option<f64> {
        let sign = if self.negative { "-" } else { "" };
        let int = if self.int.is_empty() { "0" } else { &self.int };
        let frac = if self.frac.is_empty() { "0" } else { &self.frac };
        let double: f64 = format!("{sign}{int}.{frac}").parse().ok()?;
        let zero = self.int.is_empty() && self.frac.is_empty();
        (double.is_finite() && (double != 0.0 || zero)).then_some(double)
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
pub(crate) mod prior_shape;
#[cfg(test)]
mod tests;
