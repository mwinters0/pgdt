//! A float field read under [`PostgresInvalidValues`], and what the float
//! reader needs of `f32` and `f64`: not `decode`'s API, which reads each
//! scalar's text as its value, but what a crate above calls, so `internal`
//! re-exports it.

use pgdt_model::PostgresInvalidValues;

use super::{Unread, float_in};

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
