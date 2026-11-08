//! The integers and `oid`: why `int2in` to `int8in` and `oidin` refuse a
//! text, and the readers an order key and a strict parse read one by.

use crate::decode::{Read, Unread};
use crate::value::libc::is_c_space;

/// **`int2in`, `int4in` and `int8in` fed a text a byte at a time**, holding a
/// fixed state however long it runs (`docs/design/decisions.md`, "D144"):
/// `pg_strtoint64_safe`'s grammar from v16 — a decimal between blanks, an
/// underscore between its digits and a `0x`, `0o` or `0b` prefix besides,
/// every text v13 to v15 read and more (I80) — its magnitude saturated past
/// any width, and beside it whether the text is a plain decimal, a sign and
/// digits alone, the spelling `*_out` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IntIn {
    stage: IntStage,
    negative: bool,
    magnitude: i128,
    /// No blank, underscore or prefix read.
    plain: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntStage {
    /// Blanks before the number.
    Lead,
    /// Past the sign, where a prefix may open.
    Signed,
    /// A `0` where a prefix may open, its next byte deciding.
    Zero,
    /// The digits in `radix`: `first` while none and no underscore is read
    /// past the prefix, `underscore` where one was just read.
    Digits {
        radix: u8,
        first: bool,
        underscore: bool,
    },
    /// Blanks after the number.
    Trail,
    Refused,
}

impl Default for IntIn {
    fn default() -> Self {
        Self { stage: IntStage::Lead, negative: false, magnitude: 0, plain: true }
    }
}

impl IntIn {
    /// Read the text's next byte.
    pub(crate) fn byte(&mut self, byte: u8) {
        self.stage = match self.stage {
            IntStage::Lead if is_c_space(byte) => {
                self.plain = false;
                IntStage::Lead
            }
            IntStage::Lead if matches!(byte, b'+' | b'-') => {
                self.negative = byte == b'-';
                IntStage::Signed
            }
            IntStage::Lead | IntStage::Signed if byte == b'0' => IntStage::Zero,
            IntStage::Lead | IntStage::Signed => self.digits(10, true, false, byte),
            IntStage::Zero => {
                let radix = match byte {
                    b'x' | b'X' => 16,
                    b'o' | b'O' => 8,
                    b'b' | b'B' => 2,
                    // A decimal `0`, worth nothing.
                    _ => 0,
                };
                if radix == 0 {
                    self.digits(10, false, false, byte)
                } else {
                    self.plain = false;
                    IntStage::Digits { radix, first: true, underscore: false }
                }
            }
            IntStage::Digits { radix, first, underscore } => {
                self.digits(radix, first, underscore, byte)
            }
            IntStage::Trail if is_c_space(byte) => IntStage::Trail,
            IntStage::Trail | IntStage::Refused => IntStage::Refused,
        };
    }

    /// `byte` read among the digits in `radix`.
    // pg-refuses: I80 — every refusal here is `pg_strtoint*`'s from v16.
    fn digits(&mut self, radix: u8, first: bool, underscore: bool, byte: u8) -> IntStage {
        if let Some(digit) = char::from(byte).to_digit(u32::from(radix)) {
            let magnitude = self.magnitude.saturating_mul(radix.into());
            self.magnitude = magnitude.saturating_add(digit.into());
            return IntStage::Digits { radix, first: false, underscore: false };
        }
        // An underscore stands between two digits, and may follow a prefix
        // but not open a decimal.
        if byte == b'_' && !underscore && !(radix == 10 && first) {
            self.plain = false;
            return IntStage::Digits { radix, first: false, underscore: true };
        }
        if underscore || first || !is_c_space(byte) {
            return IntStage::Refused;
        }
        self.plain = false;
        IntStage::Trail
    }

    /// The text has ended: its value, or `None` where it is no integer.
    fn value(&self) -> Option<i128> {
        let read = match self.stage {
            IntStage::Zero | IntStage::Trail => true,
            IntStage::Digits { first, underscore, .. } => !first && !underscore,
            IntStage::Lead | IntStage::Signed | IntStage::Refused => false,
        };
        read.then(|| if self.negative { -self.magnitude } else { self.magnitude })
    }

    /// The text has ended: a `smallint`, `integer` or `bigint` of `bytes`
    /// bytes, as [`int_in`] answers.
    // pg-refuses: I60 — out of range for the column's width.
    pub(crate) fn finish(&self, bytes: u8) -> Read<i64> {
        let range = int_range(bytes);
        let within = i128::from(*range.start())..=i128::from(*range.end());
        match self.value() {
            Some(value) if within.contains(&value) && self.plain => Ok(value as i64),
            Some(value) if within.contains(&value) => Err(Unread::Unparsed),
            _ => Err(Unread::Refused),
        }
    }
}

/// `uint32in_subr` over glibc's `strtoul`, in base 0 where `base0` holds and
/// base 10 where not, a `0b` prefix read where `binary` does: the oid it
/// reads, or `None` where it refuses the text ([`OidBase`] fed it whole).
#[cfg(test)]
fn oid_in(b: &[u8], base0: bool, binary: bool) -> Option<u32> {
    let mut read = OidBase::new(base0, binary);
    b.iter().for_each(|&byte| read.byte(byte));
    read.value()
}

/// **`oidin` fed a text a byte at a time**, each a fixed state however long
/// the text runs (`docs/design/decisions.md`, "D144"): the decimal digits
/// `oidout` writes, a leading `+` and zeros, which this build reads, and
/// beside them each supported major's reading, saying why a text it does not
/// read is no value — every major's `oidin` refuses it, or one reads it
/// (`docs/design/decisions.md`, "D55").
///
/// The majors part at v16, whose `uint32in_subr` hands `strtoul` base 0
/// where `oidin_subr` handed it base 10, so a `0x` or octal spelling reads
/// from v16 and a decimal one with a leading zero changes its value there;
/// a C23 glibc reads a `0b` prefix in base 0 too. A text one of them reads
/// is not refused: the exception `docs/design/roadmap.md`'s "A literal is
/// guaranteed in `*_out`'s form and never read past `*_in`'s" makes to the
/// newest major's reading.
// deficiency: KD84 — the readers of an `oid` (`str::parse` in the typed read,
// `OidIn` in `key::field_key`) take `010` for 10, as v15 and earlier do,
// where v16 and later read it as octal 8 and refuse `08` (I66). Which the
// dump's server meant is its version's to say, and no reader is handed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OidIn {
    /// The plain decimal's value so far, `None` once the text is not one or
    /// it passes `u32`.
    plain: Option<u32>,
    /// Whether a digit has been read into `plain`.
    digit: bool,
    /// Whether any byte has been read.
    started: bool,
    /// v15's base 10, v16's base 0, and v16's under a C23 glibc.
    majors: [OidBase; 3],
}

impl Default for OidIn {
    fn default() -> Self {
        Self {
            plain: Some(0),
            digit: false,
            started: false,
            majors: [
                OidBase::new(false, false),
                OidBase::new(true, false),
                OidBase::new(true, true),
            ],
        }
    }
}

impl OidIn {
    /// Read the text's next byte.
    pub(crate) fn byte(&mut self, byte: u8) {
        let sign = byte == b'+' && !self.started;
        self.plain = match (byte, self.plain) {
            (b'0'..=b'9', Some(value)) => {
                self.digit = true;
                value.checked_mul(10).and_then(|v| v.checked_add(u32::from(byte - b'0')))
            }
            (_, plain) if sign => plain,
            _ => None,
        };
        self.started = true;
        self.majors.iter_mut().for_each(|major| major.byte(byte));
    }

    /// The text has ended: the `oid` the plain decimal is, or why there is
    /// none.
    pub(crate) fn finish(&self) -> Read<u32> {
        match self.plain {
            Some(value) if self.digit => Ok(value),
            _ if self.majors.iter().any(|major| major.value().is_some()) => Err(Unread::Unparsed),
            _ => Err(Unread::Refused),
        }
    }
}

/// `uint32in_subr` over glibc's `strtoul` fed a byte at a time, in base 0
/// where `base0` holds and base 10 where not, a `0b` prefix read where
/// `binary` does. A negative value is read where it fits an `int`, wrapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OidBase {
    base0: bool,
    binary: bool,
    stage: OidStage,
    negative: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OidStage {
    Lead,
    /// Past the sign, where base 0 reads a prefix.
    Signed,
    /// A `0` where base 0 reads a prefix, its next byte deciding.
    Zero,
    /// `0x` or `0b`, a digit of its base next deciding it a prefix rather
    /// than an octal `0` followed by a letter.
    Prefix {
        base: u32,
    },
    /// The digits in `base`, `count` of them, their value below `ULONG_MAX`.
    Digits {
        base: u32,
        count: usize,
        magnitude: u64,
    },
    Trail {
        magnitude: u64,
    },
    Refused,
}

impl OidBase {
    fn new(base0: bool, binary: bool) -> Self {
        Self { base0, binary, stage: OidStage::Lead, negative: false }
    }

    fn byte(&mut self, byte: u8) {
        self.stage = match self.stage {
            OidStage::Lead if is_c_space(byte) => OidStage::Lead,
            OidStage::Lead if matches!(byte, b'+' | b'-') => {
                self.negative = byte == b'-';
                OidStage::Signed
            }
            OidStage::Lead | OidStage::Signed if self.base0 && byte == b'0' => OidStage::Zero,
            OidStage::Lead | OidStage::Signed => Self::digits(10, 0, 0, byte),
            OidStage::Zero => match byte {
                b'x' | b'X' => OidStage::Prefix { base: 16 },
                b'b' | b'B' if self.binary => OidStage::Prefix { base: 2 },
                _ => Self::digits(8, 1, 0, byte),
            },
            // Not a prefix: an octal `0`, then a letter no digit or blank.
            OidStage::Prefix { base } if !char::from(byte).is_digit(base) => OidStage::Refused,
            OidStage::Prefix { base } => Self::digits(base, 0, 0, byte),
            OidStage::Digits { base, count, magnitude } => {
                Self::digits(base, count, magnitude, byte)
            }
            OidStage::Trail { magnitude } if is_c_space(byte) => OidStage::Trail { magnitude },
            OidStage::Trail { .. } | OidStage::Refused => OidStage::Refused,
        };
    }

    /// `byte` read among `count` digits in `base` worth `magnitude`.
    fn digits(base: u32, count: usize, magnitude: u64, byte: u8) -> OidStage {
        match char::from(byte).to_digit(base) {
            // Past `ULONG_MAX`, `ERANGE`.
            Some(digit) => match magnitude.checked_mul(base.into()) {
                Some(value) => match value.checked_add(digit.into()) {
                    Some(magnitude) => OidStage::Digits { base, count: count + 1, magnitude },
                    None => OidStage::Refused,
                },
                None => OidStage::Refused,
            },
            None if count > 0 && is_c_space(byte) => OidStage::Trail { magnitude },
            None => OidStage::Refused,
        }
    }

    /// The text has ended: the oid it reads, or `None` where refused.
    // pg-refuses: I66 — every refusal here is `oidin`'s.
    fn value(&self) -> Option<u32> {
        let magnitude = match self.stage {
            // An octal `0`.
            OidStage::Zero => 0,
            OidStage::Digits { count, magnitude, .. } if count > 0 => magnitude,
            OidStage::Trail { magnitude } => magnitude,
            _ => return None,
        };
        let cvt = if self.negative { magnitude.wrapping_neg() } else { magnitude };
        let oid = cvt as u32;
        (cvt == u64::from(oid) || cvt == oid as i32 as i64 as u64).then_some(oid)
    }
}

/// A `smallint`, `integer` or `bigint`, of `bytes` bytes, field or literal,
/// read as `int2in`, `int4in` and `int8in` read it ([`IntIn`] fed it whole):
/// the digits `*_out` writes, and of `*_in`'s a leading `+` and leading
/// zeros. A blank around the number, an underscore between digits and a
/// `0x`, `0o` or `0b` prefix, which the server reads, are unparsed
/// (`docs/design/decisions.md`, "D55"), and every other text is refused.
///
/// **A value past the column's width is refused, as the server refuses it**
/// (I60): `70000` is no value of a `smallint` column, where a literal in
/// DataFusion's semantics is read as an `i64` ([`literal_key`](crate::key::order::literal_key)).
pub fn int_in(text: &str, bytes: u8) -> Read<i64> {
    let mut read = IntIn::default();
    text.bytes().for_each(|byte| read.byte(byte));
    read.finish(bytes)
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
