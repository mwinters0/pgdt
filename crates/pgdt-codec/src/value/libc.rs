//! The C library calls PostgreSQL's input functions make, as glibc answers
//! them: `isspace` and `strtod`. Shared by the families whose input functions
//! call them.

use std::io::Write;

use crate::value::source::{Refused, Walked};

/// `b[at]` as C reads it, `0` past the end.
pub(crate) fn c_at(b: &[u8], at: usize) -> u8 {
    b.get(at).copied().unwrap_or(0)
}

/// The run of ASCII digits from `at`, as its length.
fn digit_run(b: &[u8], at: usize) -> usize {
    b.get(at..).map_or(0, |rest| rest.iter().take_while(|c| c.is_ascii_digit()).count())
}

/// The longest prefix of `b[at..]` glibc's `strtod` reads in the C locale,
/// which is the server's `LC_NUMERIC`: a sign, then `inf` or `infinity`,
/// `nan` with an optional parenthesized run of letters, digits and `_`,
/// hexadecimal digits after `0x` ([`hex_float`]), or decimal ones with an
/// optional point and exponent. Its value, where it ends, and whether it lies
/// past `double precision`'s range — overflowed to an infinity, or a nonzero
/// spelling read as zero, where `strtod` sets `ERANGE`. `None` where it reads
/// nothing.
pub(crate) fn strtod(b: &[u8], at: usize) -> Option<(f64, usize, bool)> {
    let negative = c_at(b, at) == b'-';
    let i = at + usize::from(matches!(c_at(b, at), b'+' | b'-'));
    let signed = |v: f64| if negative { -v } else { v };
    let word = |w: &[u8]| b.get(i..i + w.len()).is_some_and(|s| s.eq_ignore_ascii_case(w));
    if word(b"inf") {
        let end = if word(b"infinity") { i + 8 } else { i + 3 };
        return Some((signed(f64::INFINITY), end, false));
    }
    if word(b"nan") {
        let mut end = i + 3;
        if c_at(b, end) == b'(' {
            let run = b[end + 1..]
                .iter()
                .take_while(|&&c| c.is_ascii_alphanumeric() || c == b'_')
                .count();
            if c_at(b, end + 1 + run) == b')' {
                end += run + 2;
            }
        }
        return Some((f64::NAN, end, false));
    }
    if c_at(b, i) == b'0' && matches!(c_at(b, i + 1), b'x' | b'X') {
        return Some(match hex_float(b, i + 2) {
            Some((value, end, out_of_range)) => (signed(value), end, out_of_range),
            // `0x` with no digit after it is the number `0`, the `x` unread.
            None => (signed(0.0), i + 1, false),
        });
    }
    let whole = digit_run(b, i);
    let mut end = i + whole;
    let mut fraction = 0;
    if c_at(b, end) == b'.' {
        fraction = digit_run(b, end + 1);
        end += 1 + fraction;
    }
    if whole + fraction == 0 {
        return None;
    }
    let mantissa_end = end;
    if matches!(c_at(b, end), b'e' | b'E') {
        let sign = usize::from(matches!(c_at(b, end + 1), b'+' | b'-'));
        let digits = digit_run(b, end + 1 + sign);
        if digits > 0 {
            end += 1 + sign + digits;
        }
    }
    // Every byte read is ASCII, and Rust's parse rounds to nearest as glibc's
    // does (I59), so the two agree on the value and on where it overflows.
    let value: f64 = std::str::from_utf8(&b[at..end]).ok()?.parse().ok()?;
    let out_of_range = value.is_infinite()
        || (value == 0.0 && b[i..mantissa_end].iter().any(|c| matches!(c, b'1'..=b'9')));
    Some((value, end, out_of_range))
}

/// The hexadecimal number `strtod` reads from `at`, just past `0x`: hex
/// digits with an optional point, at least one of them, then an optional `p`
/// and a decimal binary exponent; its magnitude rounded to nearest, ties to
/// even, as glibc rounds it, where it ends, and whether it lies past
/// `double precision`'s range. `None` where no digit follows.
fn hex_float(b: &[u8], at: usize) -> Option<(f64, usize, bool)> {
    let mut i = at;
    let mut hex = HexDigits::default();
    let mut point = false;
    loop {
        let c = c_at(b, i);
        if c == b'.' && !point {
            point = true;
            i += 1;
            continue;
        }
        let Some(digit) = char::from(c).to_digit(16) else { break };
        hex.digit(digit, point);
        i += 1;
    }
    if hex.digits == 0 {
        return None;
    }
    let mut end = i;
    let mut power = 0;
    if matches!(c_at(b, i), b'p' | b'P') {
        let sign = usize::from(matches!(c_at(b, i + 1), b'+' | b'-'));
        let run = digit_run(b, i + 1 + sign);
        if run > 0 {
            power = b[i + 1 + sign..i + 1 + sign + run]
                .iter()
                .fold(0i64, |n, &d| saturated_digit(n, d - b'0'));
            if c_at(b, i + 1) == b'-' {
                power = -power;
            }
            end = i + 1 + sign + run;
        }
    }
    let (value, out_of_range) = hex.value(power)?;
    Some((value, end, out_of_range))
}

/// `n` with the decimal digit `d` appended, saturated far past any exponent
/// a double reaches either way.
fn saturated_digit(n: i64, d: u8) -> i64 {
    (n * 10 + i64::from(d)).min(1 << 40)
}

/// **A hexadecimal significand as `strtod` reads it**, a digit at a time:
/// the leading significant digits, at most fifteen — sixty bits, past the
/// fifty-three a double keeps plus a rounding bit — and whether any digit
/// past them is nonzero; `exponent` is the power of two of `mantissa`'s last
/// bit.
#[derive(Debug, Clone, Copy, Default)]
struct HexDigits {
    mantissa: u64,
    kept: u32,
    sticky: bool,
    exponent: i64,
    digits: usize,
}

impl HexDigits {
    /// Read the hex digit `digit`, after the point where `point`.
    fn digit(&mut self, digit: u32, point: bool) {
        self.digits += 1;
        if self.mantissa == 0 && digit == 0 {
            self.exponent -= if point { 4 } else { 0 };
        } else if self.kept < 15 {
            self.mantissa = self.mantissa << 4 | u64::from(digit);
            self.kept += 1;
            self.exponent -= if point { 4 } else { 0 };
        } else {
            self.sticky |= digit != 0;
            self.exponent += if point { 0 } else { 4 };
        }
    }

    /// The magnitude times two to `power`, rounded to nearest, ties to even,
    /// as glibc rounds it, and whether it lies past `double precision`'s
    /// range.
    fn value(&self, power: i64) -> Option<(f64, bool)> {
        let Self { mantissa, sticky, .. } = *self;
        let exponent = self.exponent + power;
        if mantissa == 0 {
            return Some((0.0, false));
        }
        let bits = i64::from(64 - mantissa.leading_zeros());
        let top = exponent + bits - 1;
        if top > 1023 {
            return Some((f64::INFINITY, true));
        }
        // The bits the result keeps: fifty-three for a normal double, fewer
        // the further a subnormal's leading bit sits below 2^-1022.
        let precision = if top >= -1022 { 53 } else { 53 - (-1022 - top) };
        if precision < 0 {
            return Some((0.0, true));
        }
        let shift = bits - precision;
        let rounded = if shift > 0 {
            let shift = shift as u32;
            let (kept, rest, half) =
                (mantissa >> shift, mantissa & ((1 << shift) - 1), 1u64 << (shift - 1));
            kept + u64::from(rest > half || (rest == half && (sticky || kept & 1 == 1)))
        } else {
            mantissa << (-shift) as u32
        };
        if precision < 53 {
            // `rounded` counts multiples of 2^-1074, a carry into bit 52
            // being the least normal double, which its bit pattern reads as.
            let value = f64::from_bits(rounded);
            return Some((value, value == 0.0));
        }
        let (fraction, top) = if rounded == 1 << 53 { (0, top + 1) } else { (rounded, top) };
        if top > 1023 {
            return Some((f64::INFINITY, true));
        }
        let biased = u64::try_from(top + 1023).ok()?;
        Some((f64::from_bits(biased << 52 | (fraction & ((1 << 52) - 1))), false))
    }
}

/// How many significant decimal digits [`Strtod`] keeps: past the 767 that
/// can decide how a decimal rounds to a double, a digit beyond them deciding
/// only whether a tie is one, which a nonzero digit appended in their place
/// answers.
const KEPT_DIGITS: usize = 800;

/// **A number read as glibc's `strtod` reads it, fed a byte at a time**, for
/// a grammar reading no byte after a number that could continue one: where
/// `strtod` would read on into a byte and then leave it unread — an exponent
/// marker or its sign with no digit after, `0x`'s `x` with no hex digit, a
/// word's letters past `inf`, a `nan` tag left open — such a grammar refuses
/// that byte, so the reader refuses there rather than hold what it read
/// ahead (`docs/design/decisions.md`, "D144", "D146"). What it holds is
/// fixed: a decimal's leading significant digits ([`KEPT_DIGITS`]), a
/// hexadecimal's ([`HexDigits`]), and counts.
#[derive(Debug, Clone, Default)]
pub(crate) struct Strtod {
    stage: Stage,
    negative: bool,
    /// `0.` and a decimal's significant digits after it, as ASCII, the
    /// first nonzero on: the text its value is parsed from, kept between
    /// numbers.
    digits: Vec<u8>,
    /// A nonzero digit past [`KEPT_DIGITS`].
    sticky: bool,
    /// The power of ten of the point after the digits read, before the
    /// exponent: the value is `0.<digits> × 10^point`.
    point: i64,
    /// Whether a mantissa digit was read.
    any: bool,
    hex: HexDigits,
    /// The exponent's digits, saturated, and its sign.
    exponent: i64,
    exponent_negative: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Start,
    Signed,
    /// Decimal digits before any point; `zero` where they are one `0`, which
    /// `x` makes a hexadecimal prefix.
    Whole {
        zero: bool,
    },
    Fraction,
    /// `e` or `E`, then its sign.
    Marker,
    MarkerSign,
    Exponent,
    /// Past `0x`, before a hex digit, a point read where `point`.
    HexPrefix {
        point: bool,
    },
    Hex {
        point: bool,
    },
    /// `p` or `P`, then its sign.
    HexMarker,
    HexMarkerSign,
    HexExponent,
    /// So many letters of `infinity` read.
    Inf(u8),
    /// So many letters of `nan` read.
    Nan(u8),
    /// Inside `nan(`'s tag.
    Tag,
    /// `nan(…)` closed.
    Tagged,
}

impl Strtod {
    /// Whether `byte` begins a number.
    pub(crate) fn starts(byte: u8) -> bool {
        matches!(byte, b'0'..=b'9' | b'.' | b'+' | b'-' | b'i' | b'I' | b'n' | b'N')
    }

    /// Read `byte`: `Ok(true)` where it is the number's, `Ok(false)` where the
    /// number ended before it, which it does not read, and `Err` where
    /// `strtod` reads nothing or would leave a byte read unread.
    pub(crate) fn byte(&mut self, byte: u8) -> Walked<bool> {
        let lower = byte.to_ascii_lowercase();
        let digit = byte.is_ascii_digit();
        self.stage = match self.stage {
            Stage::Start if matches!(byte, b'+' | b'-') => {
                self.negative = byte == b'-';
                Stage::Signed
            }
            Stage::Start | Stage::Signed => match lower {
                b'0' => {
                    self.any = true;
                    Stage::Whole { zero: true }
                }
                b'1'..=b'9' => {
                    self.whole(byte);
                    Stage::Whole { zero: false }
                }
                b'.' => Stage::Fraction,
                b'i' => Stage::Inf(1),
                b'n' => Stage::Nan(1),
                _ => return Err(Refused),
            },
            Stage::Whole { zero: true } if lower == b'x' => Stage::HexPrefix { point: false },
            Stage::Whole { .. } if digit => {
                self.whole(byte);
                Stage::Whole { zero: false }
            }
            Stage::Whole { .. } if byte == b'.' => Stage::Fraction,
            Stage::Fraction if digit => {
                self.fraction(byte);
                Stage::Fraction
            }
            Stage::Whole { .. } | Stage::Fraction if lower == b'e' && self.any => Stage::Marker,
            Stage::Whole { .. } | Stage::Fraction if self.any => return Ok(false),
            Stage::Marker if matches!(byte, b'+' | b'-') => {
                self.exponent_negative = byte == b'-';
                Stage::MarkerSign
            }
            Stage::Marker | Stage::MarkerSign | Stage::Exponent if digit => {
                self.exponent = saturated_digit(self.exponent, byte - b'0');
                Stage::Exponent
            }
            Stage::Exponent | Stage::Tagged => return Ok(false),
            Stage::HexPrefix { point } | Stage::Hex { point } if byte.is_ascii_hexdigit() => {
                let value = char::from(byte).to_digit(16).expect("a hex digit");
                self.hex.digit(value, point);
                Stage::Hex { point }
            }
            Stage::HexPrefix { point: false } if byte == b'.' => Stage::HexPrefix { point: true },
            Stage::Hex { point: false } if byte == b'.' => Stage::Hex { point: true },
            Stage::Hex { .. } if lower == b'p' => Stage::HexMarker,
            Stage::Hex { .. } => return Ok(false),
            Stage::HexMarker if matches!(byte, b'+' | b'-') => {
                self.exponent_negative = byte == b'-';
                Stage::HexMarkerSign
            }
            Stage::HexMarker | Stage::HexMarkerSign | Stage::HexExponent if digit => {
                self.exponent = saturated_digit(self.exponent, byte - b'0');
                Stage::HexExponent
            }
            Stage::HexExponent => return Ok(false),
            Stage::Inf(read) if read < 8 && lower == b"infinity"[usize::from(read)] => {
                Stage::Inf(read + 1)
            }
            Stage::Nan(read) if read < 3 && lower == b"nan"[usize::from(read)] => {
                Stage::Nan(read + 1)
            }
            Stage::Nan(3) if byte == b'(' => Stage::Tag,
            Stage::Inf(3 | 8) | Stage::Nan(3) => return Ok(false),
            Stage::Tag if byte.is_ascii_alphanumeric() || byte == b'_' => Stage::Tag,
            Stage::Tag if byte == b')' => Stage::Tagged,
            _ => return Err(Refused),
        };
        Ok(true)
    }

    /// An integer digit, `1` to `9` or after one.
    fn whole(&mut self, byte: u8) {
        self.any = true;
        if self.kept() == 0 && byte == b'0' {
            return;
        }
        self.point = self.point.saturating_add(1);
        self.keep(byte);
    }

    /// A digit after the point.
    fn fraction(&mut self, byte: u8) {
        self.any = true;
        if self.kept() == 0 && byte == b'0' {
            self.point = self.point.saturating_sub(1);
            return;
        }
        self.keep(byte);
    }

    /// How many significant digits are kept.
    fn kept(&self) -> usize {
        self.digits.len().saturating_sub(2)
    }

    fn keep(&mut self, byte: u8) {
        if self.digits.is_empty() {
            self.digits.extend_from_slice(b"0.");
        }
        if self.kept() < KEPT_DIGITS {
            self.digits.push(byte);
        } else {
            self.sticky |= byte != b'0';
        }
    }

    /// The number has ended: its value — `Err` where `strtod` reads nothing
    /// or would leave a byte read unread, or the value lies past `double
    /// precision`'s range, which `float8in_internal` refuses — and the
    /// reader readied for the next, its buffer kept.
    pub(crate) fn finish(&mut self) -> Walked<f64> {
        let mut digits = std::mem::take(&mut self.digits);
        let read = std::mem::take(self);
        let power = read.signed_exponent();
        let magnitude = match read.stage {
            Stage::Whole { .. } | Stage::Fraction | Stage::Exponent if read.any => {
                decimal(&mut digits, read.sticky, read.point, power)
            }
            Stage::Hex { .. } | Stage::HexExponent => match read.hex.value(power) {
                Some((value, false)) => Ok(value),
                _ => Err(Refused),
            },
            Stage::Inf(3 | 8) => Ok(f64::INFINITY),
            Stage::Nan(3) | Stage::Tagged => Ok(f64::NAN),
            _ => Err(Refused),
        };
        digits.clear();
        self.digits = digits;
        magnitude.map(|magnitude| if read.negative { -magnitude } else { magnitude })
    }

    fn signed_exponent(&self) -> i64 {
        if self.exponent_negative { -self.exponent } else { self.exponent }
    }
}

/// `text`, `0.` and the significant digits, times `10^(point + exponent)`,
/// a nonzero digit appended where `sticky`, rounded to nearest as glibc
/// rounds it (I59) — `Err` where it overflows to an infinity or, not being
/// zero, underflows to it. No digit is zero.
fn decimal(text: &mut Vec<u8>, sticky: bool, point: i64, exponent: i64) -> Walked<f64> {
    if text.len() <= 2 {
        return Ok(0.0);
    }
    if sticky {
        text.push(b'1');
    }
    write!(text, "e{}", point.saturating_add(exponent)).map_err(|_| Refused)?;
    // Rust's parse rounds to nearest as glibc's does (I59).
    let value: f64 =
        std::str::from_utf8(text).map_err(|_| Refused)?.parse().map_err(|_| Refused)?;
    if value.is_infinite() || value == 0.0 { Err(Refused) } else { Ok(value) }
}

/// C's `isspace` in the server's locale over a byte below `0x80`, which is
/// what the input functions below skip: a byte past it is a blank in no
/// locale a server runs in.
pub(crate) fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

/// What [`strtod_token`] found.
pub(crate) enum StrtodToken {
    Decimal,
    /// `inf`, `infinity` or `nan`, any tag included.
    Word,
    /// A hexadecimal number: where its digits and any point among them lie
    /// in the text, how many digits precede the point, and its binary
    /// exponent, saturated far past any type's range.
    Hex {
        digits: std::ops::Range<usize>,
        integer: usize,
        exponent: i64,
    },
}

/// The longest prefix of `b` glibc's `strtod` converts in the C locale, and
/// what it is: `None` where it converts none. A sign, then a word, a
/// hexadecimal number — `0x`, hex digits with at most one point among them,
/// at least one digit, then a `p` exponent only where a digit follows it — or
/// a decimal one, shaped alike with an `e` exponent. `0x` with no digit after
/// it is the decimal `0`.
pub(crate) fn strtod_token(b: &[u8]) -> Option<(usize, StrtodToken)> {
    let mut at = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let word = |at: usize, word: &[u8]| {
        b.get(at..at + word.len()).is_some_and(|s| s.eq_ignore_ascii_case(word))
    };
    if word(at, b"inf") {
        at += 3;
        if word(at, b"inity") {
            at += 5;
        }
        return Some((at, StrtodToken::Word));
    }
    if word(at, b"nan") {
        at += 3;
        if b.get(at) == Some(&b'(') {
            let tag =
                b[at + 1..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'_').count();
            if b.get(at + 1 + tag) == Some(&b')') {
                at += tag + 2;
            }
        }
        return Some((at, StrtodToken::Word));
    }
    // The digits of `radix` from `at` on.
    let run = |at: usize, radix: u32| {
        b.get(at..)
            .map_or(0, |rest| rest.iter().take_while(|c| char::from(**c).is_digit(radix)).count())
    };
    // A `marker` exponent at `at`: its length, and its value, saturated.
    let exponent = |at: usize, marker: u8| {
        if !b.get(at).is_some_and(|c| c.to_ascii_lowercase() == marker) {
            return (0, 0);
        }
        let signed = usize::from(matches!(b.get(at + 1), Some(b'+' | b'-')));
        let digits = run(at + 1 + signed, 10);
        if digits == 0 {
            return (0, 0);
        }
        let magnitude = b[at + 1 + signed..at + 1 + signed + digits]
            .iter()
            .fold(0i64, |n, d| (n * 10 + i64::from(d - b'0')).min(1 << 40));
        let value = if b[at + 1] == b'-' { -magnitude } else { magnitude };
        (1 + signed + digits, value)
    };
    // A mantissa of `radix` digits from `at`: where its point is, and its
    // length, the point consumed only beside a digit.
    let mantissa = |at: usize, radix: u32| {
        let integer = run(at, radix);
        let fraction =
            if b.get(at + integer) == Some(&b'.') { run(at + integer + 1, radix) } else { 0 };
        let point = usize::from(b.get(at + integer) == Some(&b'.'));
        (integer, fraction, if integer + fraction == 0 { 0 } else { integer + point + fraction })
    };
    if matches!(b.get(at..at + 2), Some([b'0', b'x' | b'X'])) {
        let (integer, _, len) = mantissa(at + 2, 16);
        if len > 0 {
            let first = at + 2;
            let (exponent_len, exponent) = exponent(first + len, b'p');
            let digits = first..first + len;
            return Some((
                first + len + exponent_len,
                StrtodToken::Hex { digits, integer, exponent },
            ));
        }
    }
    let (_, _, len) = mantissa(at, 10);
    if len == 0 {
        return None;
    }
    let (exponent_len, _) = exponent(at + len, b'e');
    Some((at + len + exponent_len, StrtodToken::Decimal))
}
