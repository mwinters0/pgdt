//! `real` and `double precision`: their readers and renderers, why
//! `float4in` or `float8in` refuses a text, and a float field read under
//! [`PostgresInvalidValues`] — not `decode`'s API, which reads each scalar's
//! text as its value, but what a crate above calls, so `internal` re-exports
//! it.

use pgdt_model::PostgresInvalidValues;

use crate::decode::{Read, Unread};
use crate::value::libc::{StrtodToken, is_c_space, strtod_token};

/// A float field, as [`float_in`] reads it — or, under
/// [`PostgresInvalidValues::Ignore`], a spelling it refuses as out of range
/// read as the parse rounds it: past the largest finite value, that value of
/// its sign, as `*_out` meant it (I57), and below the smallest, zero.
pub fn float_field<F: Float>(text: &str, invalid: PostgresInvalidValues) -> Option<F> {
    match (float_in::<F>(text), invalid) {
        (Ok(value), _) => Some(value),
        (Err(Unread::Refused), PostgresInvalidValues::Ignore) => {
            // deficiency: KD108 — Rust's parse reads no blank and no
            // hexadecimal spelling, so ` 1e400` and `0x1p1024`, which
            // `float_unread` refuses as out of range, are not rounded but
            // unread, and a query reading one fails.
            let value = text.parse::<F>().ok()?;
            Some(if value.is_infinite() { value.largest_of_sign() } else { value })
        }
        (Err(_), _) => None,
    }
}

/// What [`float_in`] needs of `f32` and `f64`.
pub trait Float: std::str::FromStr + Copy {
    /// The significand's bits, the implicit one included.
    const SIGNIFICAND_BITS: u32;
    /// The power of two of the largest finite value's leading bit.
    const MAX_EXP: i64;
    /// The power of two of the least subnormal.
    const MIN_SUBNORMAL_EXP: i64;
    fn is_infinite(self) -> bool;
    fn is_zero(self) -> bool;
    /// The type's largest finite value, of this value's sign.
    fn largest_of_sign(self) -> Self;
}

impl Float for f32 {
    const SIGNIFICAND_BITS: u32 = f32::MANTISSA_DIGITS;
    const MAX_EXP: i64 = f32::MAX_EXP as i64 - 1;
    const MIN_SUBNORMAL_EXP: i64 = f32::MIN_EXP as i64 - f32::MANTISSA_DIGITS as i64;
    fn is_infinite(self) -> bool {
        f32::is_infinite(self)
    }
    fn is_zero(self) -> bool {
        self == 0.0
    }
    fn largest_of_sign(self) -> Self {
        f32::MAX.copysign(self)
    }
}

impl Float for f64 {
    const SIGNIFICAND_BITS: u32 = f64::MANTISSA_DIGITS;
    const MAX_EXP: i64 = f64::MAX_EXP as i64 - 1;
    const MIN_SUBNORMAL_EXP: i64 = f64::MIN_EXP as i64 - f64::MANTISSA_DIGITS as i64;
    fn is_infinite(self) -> bool {
        f64::is_infinite(self)
    }
    fn is_zero(self) -> bool {
        self == 0.0
    }
    fn largest_of_sign(self) -> Self {
        f64::MAX.copysign(self)
    }
}

/// Split the decimal digits and exponent out of Rust's own shortest
/// round-trip scientific formatting (`{:e}`), which is exactly the digit
/// generator PostgreSQL's own float formatter produces under
/// `extra_float_digits = 3`. `exp` is the power of ten such that
/// `value == 0.<digits> * 10^(exp+1)` (equivalently, the position of the
/// decimal point is `exp + 1` digits from the left).
fn shortest_digits(mantissa_exp: &str) -> (String, i32) {
    let (mantissa, exp_str) = mantissa_exp.split_once('e').expect("LowerExp always emits 'e'");
    let exp: i32 = exp_str.parse().expect("LowerExp exponent is always a plain integer");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    (digits, exp)
}

/// Format `digits`/`exp` (see [`shortest_digits`]) the way PostgreSQL's
/// `float4out`/`float8out` do: scientific if the exponent falls outside
/// `[-4, sig_digits)` (the classic `%g` rule, using `FLT_DIG`/`DBL_DIG` — 6
/// and 15 — as the threshold regardless of how many significant digits
/// `extra_float_digits = 3` actually produced), fixed-point otherwise.
/// Exponent form always carries an explicit sign and at least two digits
/// (`e+06`, `e-05`, `e+100`).
fn format_shortest(neg: bool, digits: &str, exp: i32, sig_digits: i32) -> String {
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if exp < -4 || exp >= sig_digits {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", exp.abs()));
    } else {
        let dp = exp + 1;
        if dp <= 0 {
            out.push_str("0.");
            out.push_str(&"0".repeat((-dp) as usize));
            out.push_str(digits);
        } else if (dp as usize) >= digits.len() {
            out.push_str(digits);
            out.push_str(&"0".repeat(dp as usize - digits.len()));
        } else {
            out.push_str(&digits[..dp as usize]);
            out.push('.');
            out.push_str(&digits[dp as usize..]);
        }
    }
    out
}

/// `real`'s text, read by [`float_in`].
pub fn decode_f32(s: &str) -> Option<f32> {
    float_in(s).ok()
}

/// `double precision`'s text, read by [`float_in`].
pub fn decode_f64(s: &str) -> Option<f64> {
    float_in(s).ok()
}

/// A `real` or `double precision`, field or literal, read as `float4in` and
/// `float8in` read it (`docs/design/decisions.md`, "D55"). Rust's parse is the
/// grammar: every spelling `*_out` writes, and of `*_in`'s the ones it reads
/// for free — a sign, a point with no digit on one side, an exponent in either
/// case, `inf`, `infinity` and `nan` in any case. A blank around the number,
/// a hexadecimal one and `nan(…)`, which glibc's `strtod` reads, are
/// unparsed, and every other text it does not read is refused
/// ([`float_unread`]).
///
/// **A value past the type's range is refused, as the server refuses it**: a
/// spelling in digits read as an infinity, I57's rounded largest finite value
/// among them, and a nonzero one read as zero. A subnormal is read. Both are
/// told from the parse's result alone, the parse rounding correctly to the
/// type as `strtod` and `strtof` do (I59).
// pg-refuses: I59 — out of range for the type: overflowed, or underflowed to zero.
pub(crate) fn float_in<F: Float>(text: &str) -> Read<F> {
    let value = text.parse::<F>().map_err(|_| float_unread::<F>(text))?;
    let out_of_range = if value.is_infinite() {
        text.bytes().any(|b| b.is_ascii_digit())
    } else {
        value.is_zero()
            && text
                .split(['e', 'E'])
                .next()
                .unwrap_or(text)
                .bytes()
                .any(|b| matches!(b, b'1'..=b'9'))
    };
    if out_of_range { Err(Unread::Refused) } else { Ok(value) }
}

/// Why `float4in` or `float8in` reads no value here from `text`, Rust's parse
/// having read none: the server reads what glibc's `strtof` and `strtod` read
/// between blanks — a decimal, a hexadecimal number, `inf`, `infinity`, and
/// `nan` with an optional parenthesized tag of letters, digits and `_` — and
/// refuses everything else (I81), and a number read but out of range for the
/// type (I59). What it reads is unparsed.
// pg-refuses: I81 — every refusal here is `float4in`'s or `float8in`'s.
fn float_unread<F: Float>(text: &str) -> Unread {
    let b = text.as_bytes();
    let start = b.iter().take_while(|&&c| is_c_space(c)).count();
    let Some((len, token)) = strtod_token(&b[start..]) else {
        return Unread::Refused;
    };
    if !b[start + len..].iter().all(|&c| is_c_space(c)) {
        return Unread::Refused;
    }
    let refused = match token {
        // Rust's parse reads every decimal `strtod` does, so this reads it.
        StrtodToken::Decimal => {
            matches!(float_in::<F>(&text[start..start + len]), Err(Unread::Refused))
        }
        StrtodToken::Word => false,
        StrtodToken::Hex { digits, integer, exponent } => {
            hex_float_out_of_range::<F>(&b[start..][digits], integer, exponent)
        }
    };
    if refused { Unread::Refused } else { Unread::Unparsed }
}

/// Whether the hexadecimal number whose digits are `digits` — a point after
/// the first `integer` of them, where `digits` holds one — times two to
/// `exponent` is one glibc rounds to an infinity or, not being zero, to zero
/// in `F`, rounding to nearest, ties to even (I59).
fn hex_float_out_of_range<F: Float>(digits: &[u8], integer: usize, exponent: i64) -> bool {
    let nibbles: Vec<u32> = digits
        .iter()
        .filter(|&&c| c != b'.')
        .map(|&c| char::from(c).to_digit(16).expect("a hex digit"))
        .collect();
    let Some(first) = nibbles.iter().position(|&n| n != 0) else {
        return false;
    };
    // The bits from the leading one on.
    let bit = |i: usize| {
        let at = i + (nibbles[first].leading_zeros() as usize - 28);
        nibbles.get(first + at / 4).is_some_and(|n| n >> (3 - at % 4) & 1 == 1)
    };
    let width = (nibbles.len() - first) * 4 - (nibbles[first].leading_zeros() as usize - 28);
    // The leading one's power of two.
    let lead =
        4 * (integer as i64 - first as i64) - 1 - (nibbles[first].leading_zeros() as i64 - 28)
            + exponent;
    if lead > F::MAX_EXP {
        return true;
    }
    if lead == F::MAX_EXP {
        // Every significand bit set and the next one too rounds up past the
        // largest finite value.
        let bits = F::SIGNIFICAND_BITS as usize;
        return (0..=bits).all(bit);
    }
    // The least subnormal's power of two: half of it rounds to zero, being a
    // tie, and anything above half to it.
    let least = F::MIN_SUBNORMAL_EXP;
    lead < least - 1 || (lead == least - 1 && !(1..width).any(bit))
}

/// `FLT_DIG` — the significant-digit threshold PostgreSQL's `float4out`
/// switches to scientific notation at (see [`format_shortest`]'s docs).
const FLT_DIG: i32 = 6;

/// `DBL_DIG`, `float8out`'s equivalent threshold.
const DBL_DIG: i32 = 15;

pub fn render_f32(v: f32) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity".to_string() } else { "-Infinity".to_string() };
    }
    let (digits, exp) = shortest_digits(&format!("{:e}", v.abs()));
    format_shortest(v.is_sign_negative(), &digits, exp, FLT_DIG)
}

pub fn render_f64(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity".to_string() } else { "-Infinity".to_string() };
    }
    let (digits, exp) = shortest_digits(&format!("{:e}", v.abs()));
    format_shortest(v.is_sign_negative(), &digits, exp, DBL_DIG)
}

#[cfg(test)]
mod tests;
