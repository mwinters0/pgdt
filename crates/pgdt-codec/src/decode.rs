//! Per-type field decode and render-back (`docs/design/decisions.md`,
//! "D44").
//!
//! Pure, synchronous, no I/O (`docs/design/decisions.md`, "D74"). A decode
//! function takes an already-COPY-unescaped `&str` field (what
//! `pgdt_core::copy::decode_field` returns) and returns a plain Rust value; it
//! never takes an Arrow builder — building the array is `batch.rs`'s job
//! (query's), which is also where these are wired to their [`arrow_schema::DataType`].
//!
//! Every `render_*` function is the exact inverse of its `decode_*`
//! counterpart — but [`render_bytea`], which writes the hex form whichever
//! form [`decode_bytea`] read (`docs/design/decisions.md`, "D66") — and produces the same *decoded* text `pgdt_core::copy::decode_field`
//! would have returned for a correctly-formed value — never the raw
//! COPY-escaped bytes on disk. Converting between the two is
//! `pgdt_core::copy::encode_field`'s job (`docs/design/decisions.md`, "D68").
//! `tests/decode.rs`'s round-trip test compares against `SchemaMode::Strings`,
//! which is also decoded text; the on-disk-byte leg is covered separately in
//! `tests/scan.rs`.

use pgdt_model::PostgresInvalidValues;

/// Why a text is read as no value of its type, where a caller must tell the
/// two apart: a parse fails on the first field PostgreSQL refuses, and on no
/// other (`roadmap.md`, "A literal is guaranteed in `*_out`'s form and never
/// read past `*_in`'s"). Why a reader stays narrow and a classifier answers
/// this where it fails: `docs/design/decisions.md`, "D55".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unread {
    /// The type's `*_in` refuses it: found at a check carrying a
    /// `pg-refuses: I<n>` marker, so a release lifting the refusal is found
    /// by walking that invariant. Such a check sits behind a grammar no wider
    /// than the server's at the parts it bounds, or a shortfall's spelling
    /// reaches it — a run-together `+0530` read as an hour of 530, digits
    /// counted before an exponent moved the point — and refuses what the
    /// server reads; so every failure of a date or time reader, its own
    /// refusals included, is put to `crate::datetime_in`, the readers
    /// splitting a text outside `*_out`'s form otherwise than `ParseDateTime`.
    Refused,
    /// A spelling this build reads no value from that the server reads at
    /// some supported major — a shortfall (`docs/design/decisions.md`,
    /// "D55"). Each reader classifies what it fails on by the server's
    /// grammar (`int_unread`, `float_unread`, `numeric_unread`,
    /// `crate::key`'s `jsonb_unread`, and `crate::datetime_in` for the
    /// date and time types). No reader is widened to read one: a blank
    /// around a number, a hexadecimal float, `nan(…)`, an exponent in a
    /// `numeric` field, `0x1F`, `1_000` and `y2001m02d04` stay unparsed, and
    /// a query decoding one fails with `FieldDecode`.
    Unparsed,
}

/// A reader's answer where [`Unread`] matters.
pub type Read<T> = Result<T, Unread>;

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

/// `t`/`f`, COPY TEXT's boolean spelling.
pub fn decode_bool(s: &str) -> Option<bool> {
    match s {
        "t" => Some(true),
        "f" => Some(false),
        _ => None,
    }
}

/// Why [`decode_bool`] read no value from `s`: `boolin` refuses it, or reads
/// a spelling `boolout` never writes — `true`, `yes`, `on`, `1` or a
/// negation, given by any prefix naming the word alone, in either case, with
/// blanks around it (`docs/design/decisions.md`, "D55").
// pg-refuses: I65 — every refusal here is `boolin`'s.
pub(crate) fn bool_unread(s: &str) -> Unread {
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

/// C's `isspace` in the server's locale over a byte below `0x80`, which is
/// what the input functions below skip: a byte past it is a blank in no
/// locale a server runs in.
fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

pub fn render_bool(v: bool) -> &'static str {
    if v { "t" } else { "f" }
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

/// `pg_strtoint64_safe`'s grammar from v16, its slow path: the value of `b`,
/// its magnitude saturated past any width, or `None` where it refuses `b` as
/// syntax. An underscore stands between two digits, and may follow a prefix
/// but not open a decimal.
fn strtoint(b: &[u8]) -> Option<i128> {
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

/// What [`strtod_token`] found.
enum StrtodToken {
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
fn strtod_token(b: &[u8]) -> Option<(usize, StrtodToken)> {
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

/// Days from a proleptic-Gregorian civil date to the 1970-01-01 epoch.
/// Howard Hinnant's public-domain `days_from_civil` algorithm — correct for
/// negative (BCE, astronomical numbering) years, which is why it's used here
/// rather than any calendar library assuming a year > 0.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (i64::from(m) + 9) % 12; // [0, 11]
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`].
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (y + i64::from(m <= 2), m, d)
}

/// The 100 two-digit decimal pairs end to end, so `v`'s pair is the two bytes
/// at `v * 2` — [`HEX_PAIRS`]'s decimal counterpart. Every zero-padded field a
/// `date`, a `time` or a `timestamp` is written from is two digits wide, or a
/// four-digit year's two of them, so a
/// field becomes an indexed slice and a two-byte copy rather than a trip
/// through `core::fmt` (`docs/design/decisions.md`, "D44").
const DEC_PAIRS_BYTES: [u8; 200] = {
    let digits = *b"0123456789";
    let mut table = [0u8; 200];
    let mut v = 0usize;
    while v < 100 {
        table[v * 2] = digits[v / 10];
        table[v * 2 + 1] = digits[v % 10];
        v += 1;
    }
    table
};

/// The same table as text, converted once at compile time, so a renderer
/// appends with `push_str` and pays no UTF-8 validation — the same shape as
/// [`HEX_PAIRS`].
static DEC_PAIRS: &str = match str::from_utf8(&DEC_PAIRS_BYTES) {
    Ok(s) => s,
    Err(_) => panic!("decimal digits are ASCII"),
};

/// The ten decimal digits, so a leading odd digit is appended as a `&str`
/// slice like every other digit and the whole function stays free of
/// byte-to-`str` conversion.
static DEC_DIGITS: &str = "0123456789";

/// `format!("{value:0width$}")` for an `i64`, digit for digit: sign-aware zero
/// padding, so a `-` leads and the padding zeros follow it, and no padding at
/// all once the digits are already that wide. `width` of `0` is therefore
/// exactly `i64::to_string`, which is what [`push_integer`] is.
///
/// Three properties are load-bearing rather than stylistic
/// (`docs/design/decisions.md`, "D44"):
///
/// - **The digits come out two at a time**, off [`DEC_PAIRS`], the algorithm
///   the standard library's own integer `Display` uses.
/// - **Every digit is a slice of a `&'static str`**, the sign and padding
///   pushed as ASCII `char`s, so nothing here converts bytes to text and no
///   UTF-8 validation is paid per field.
/// - **The whole length is reserved once**, so no `push_str` grows the
///   buffer.
#[inline]
fn push_padded(out: &mut String, value: i64, width: usize) {
    // Pair values, least significant first; a `u64` has at most nine of them
    // ahead of its leading one or two digits.
    let mut pairs = [0u8; 10];
    let mut count = 0;
    let mut magnitude = value.unsigned_abs();
    while magnitude >= 100 {
        pairs[count] = (magnitude % 100) as u8;
        count += 1;
        magnitude /= 100;
    }
    let lead = magnitude as usize;
    let negative = value < 0;
    let digits = 2 * count + if lead >= 10 { 2 } else { 1 };
    out.reserve(width.max(digits + usize::from(negative)));
    if negative {
        out.push('-');
    }
    for _ in (digits + usize::from(negative))..width {
        out.push('0');
    }
    if lead >= 10 {
        out.push_str(&DEC_PAIRS[lead * 2..lead * 2 + 2]);
    } else {
        out.push_str(&DEC_DIGITS[lead..lead + 1]);
    }
    for pair in pairs[..count].iter().rev() {
        let at = usize::from(*pair) * 2;
        out.push_str(&DEC_PAIRS[at..at + 2]);
    }
}

/// An integer field's digits, appended: the same text `i64::to_string`
/// produces, without the `String` it allocates.
///
/// `write!(out, "{value}")` reaches the same `Display` impl through
/// `core::fmt::write`, which this path never uses
/// (`docs/design/decisions.md`, "D44").
#[inline]
pub fn push_integer(out: &mut String, value: i64) {
    push_padded(out, value, 0);
}

/// `format!("{value:02}")` — one table lookup for the two-digit range every
/// calendar and clock field of a well-formed value lives in.
fn push_two(out: &mut String, value: i64) {
    if (0..100).contains(&value) {
        let at = value as usize * 2;
        out.push_str(&DEC_PAIRS[at..at + 2]);
    } else {
        push_padded(out, value, 2);
    }
}

/// `format!("{year:04}")`, as two pairs for the four-digit years and the
/// general path for anything wider — `Date32`'s day range reaches years either
/// side of five million.
fn push_year(out: &mut String, year: i64) {
    if (0..10_000).contains(&year) {
        push_two(out, year / 100);
        push_two(out, year % 100);
    } else {
        push_padded(out, year, 4);
    }
}

/// The fractional-seconds digits, `micros` in `1..1_000_000`: six digits with
/// trailing zeros trimmed, which is what PostgreSQL writes (`00:00:00.5`, not
/// `.500000`). The trim is a `truncate` back to the last non-zero digit,
/// bounded by `start` so it can never reach text the caller had already
/// written.
fn push_fraction(out: &mut String, micros: i64) {
    let start = out.len();
    push_two(out, micros / 10_000);
    push_two(out, (micros / 100) % 100);
    push_two(out, micros % 100);
    let mut end = out.len();
    {
        let bytes = out.as_bytes();
        while end > start && bytes[end - 1] == b'0' {
            end -= 1;
        }
    }
    out.truncate(end);
}

/// The `YYYY-MM-DD` head a `date` and a `timestamp` share, out of a day count.
/// Returns whether the value is before the common era: PostgreSQL writes the
/// `" BC"` marker at the very end, past the time and the zone, so the caller
/// appends it rather than this.
fn push_civil_date(out: &mut String, days: i64) -> bool {
    let (y, m, d) = civil_from_days(days);
    let (out_year, bc) = if y <= 0 { (1 - y, true) } else { (y, false) };
    push_year(out, out_year);
    out.push('-');
    push_two(out, i64::from(m));
    out.push('-');
    push_two(out, i64::from(d));
    bc
}

/// Strip a trailing `" BC"` era marker, PostgreSQL's spelling for a
/// before-common-era date/timestamp. Returns the remaining text and whether
/// the marker was present.
fn split_era(s: &str) -> (&str, bool) {
    match s.strip_suffix(" BC") {
        Some(rest) => (rest, true),
        None => (s, false),
    }
}

/// A date or time part as `strtoint` reads it once `ParseDateTime` has split
/// the text at its signs: ASCII digits only, at least one. A sign is where the
/// server starts a zone (`12:-5:00` is 12:00 at zone `-5`, `+2020-01-01`
/// refused), so a part carrying one is not read here, rather than read as a
/// signed number — a shortfall where the server reads it
/// (`docs/design/decisions.md`, "D55"). Past `i64` is `None` too.
fn unsigned_part(s: &str) -> Option<i64> {
    if s.is_empty() {
        return None;
    }
    s.bytes().try_fold(0i64, |value, b| {
        if !b.is_ascii_digit() {
            return None;
        }
        value.checked_mul(10)?.checked_add(i64::from(b - b'0'))
    })
}

/// `JULIAN_MAXYEAR`, the year `IS_VALID_JULIAN` refuses from (its June, which
/// the day ranges below lie well short of): every year a date or timestamp
/// holds is below it, and a year bounded by it cannot overflow
/// [`days_from_civil`].
const JULIAN_MAX_YEAR: i64 = 5_874_898;

/// The first day a `date` holds, `4714-11-24 BC` — Julian day 0,
/// `DATETIME_MIN_JULIAN` — in days from 1970.
const DATE_MIN_DAYS: i64 = -2_440_588;

/// The first day past a `date`'s range, `5874898-01-01` — `DATE_END_JULIAN`
/// — in days from 1970.
const DATE_END_DAYS: i64 = 2_145_042_906;

/// The days in month `m` (1–12) of astronomical year `y`, by `day_tab` and
/// `isleap` over the proleptic Gregorian calendar, so 1 BC (year 0) is leap.
fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `YYYY-MM-DD`, its era already split off (`bc`), as days from 1970 —
/// [`Unread::Refused`] for a day the calendar does not hold, as `ValidateDate` refuses it.
///
/// The year is three digits or more, as `pg_dump` writes four: a shorter one
/// is read by `DateOrder` (`20-01-01` is a month 20 under `MDY`, the year 2020
/// under `YMD`), so it is refused as a shortfall, as is a month of three
/// digits, which the server reads as a day of the year (D55).
fn civil_days(s: &str, bc: bool) -> Read<i64> {
    let unparsed = Unread::Unparsed;
    let (year, rest) = s.split_once('-').ok_or(unparsed)?;
    let (month, day) = rest.split_once('-').ok_or(unparsed)?;
    if year.len() < 3 || month.len() > 2 {
        return Err(unparsed);
    }
    let part = |s| unsigned_part(s).ok_or(unparsed);
    let (y, m, d) = (part(year)?, part(month)?, part(day)?);
    // pg-refuses: I61 — no year zero either side of the era, a month past
    // twelve, a day past its month's, a year past `IS_VALID_JULIAN`'s.
    if y == 0 || y > JULIAN_MAX_YEAR || !(1..=12).contains(&m) {
        return Err(Unread::Refused);
    }
    let y = astronomical_year(y, bc);
    if !(1..=days_in_month(y, m)).contains(&d) {
        return Err(Unread::Refused);
    }
    Ok(days_from_civil(y, m as u32, d as u32))
}

/// `1 - year` turns a `" BC"`-suffixed calendar year into PostgreSQL's (and
/// the proleptic Gregorian calendar's) astronomical year numbering: 1 BC is
/// astronomical year 0, 44 BC is -43, and so on.
fn astronomical_year(y: i64, bc: bool) -> i64 {
    if bc { 1 - y } else { y }
}

/// `None` for `infinity`/`-infinity` (PostgreSQL's own pseudo-values for
/// "unbounded" — real, but `Date32` has no sentinel for them, so this is a
/// decode failure by construction, not by accident), and for a day outside
/// PostgreSQL's range, which `Date32` holds whole.
///
/// **The anchor for every one of these refusals** ([`decode_interval`]'s
/// infinities and overflowing time part, `NaN` on a `Decimal128`, a
/// timestamp past `i64` microseconds from 1970, which PostgreSQL's range
/// outlasts by three decades, and [`decode_time64_micros`]'s `24:00:00`): a
/// typed column cannot hold the value, nor DataFusion display a `date`, or a
/// timestamp short of `i64`'s end, past `262142-12-31`, which decodes and
/// which `arrow-cast` formats through a calendar ending there. No query asks
/// a decoder for one: the null mode reads it as NULL first
/// (`docs/design/decisions.md`, "D98"), the untyped mode reads its column as
/// text ("D100"), and the refuse mode refuses at planning a query
/// materializing its column ("D99").
pub fn decode_date32(s: &str) -> Option<i32> {
    date_days(s).ok()
}

/// [`decode_date32`] telling a day PostgreSQL refuses from text that is no
/// day here. The infinities are [`Unread::Unparsed`]: values of the type, but
/// no day, which a caller ranks first if it can (`crate::key`).
pub(crate) fn date_days(s: &str) -> Read<i32> {
    if s == "infinity" || s == "-infinity" {
        return Err(Unread::Unparsed);
    }
    let (rest, bc) = split_era(s);
    let days = civil_days(rest, bc)?;
    // pg-refuses: I61 — `IS_VALID_DATE`'s range.
    if !(DATE_MIN_DAYS..DATE_END_DAYS).contains(&days) {
        return Err(Unread::Refused);
    }
    Ok(i32::try_from(days).expect("a date's range is an i32 of days"))
}

pub fn render_date32(days: i32) -> String {
    // Room for every `Date32` PostgreSQL writes: a seven-digit year, `-MM-DD`
    // and ` BC`.
    let mut out = String::with_capacity(16);
    render_date32_into(days, &mut out);
    out
}

/// [`render_date32`] appending to a caller's buffer instead of returning one.
/// The `_into` form is the one that does the work; the owned form above is a
/// wrapper, so the two cannot drift (`docs/design/decisions.md`, "D44").
pub fn render_date32_into(days: i32, out: &mut String) {
    if push_civil_date(out, i64::from(days)) {
        out.push_str(" BC");
    }
}

/// Powers of ten up to a microsecond, indexed by how many fractional digits
/// a time-of-day literal is short of six.
const POW10: [i64; 7] = [1, 10, 100, 1_000, 10_000, 100_000, 1_000_000];

/// `HH:MM:SS[.ffffff]`, no offset. Returns whole seconds since midnight and
/// the microsecond remainder separately, since callers need both a plain
/// `Time64` value (`seconds*1_000_000 + micros`) and, for a timestamp, the
/// same pair added onto a day count. `crate::key` reads it too, for the
/// time half of a `time with time zone` comparison — that type has no Arrow
/// mapping of its own, so its *ordering* is the only path that decodes it.
///
/// [`Unread::Refused`] past `time_overflows`' bounds, which `time`, `timetz` and both
/// timestamps share: `24:00:00` is the last time of day, and `23:59:60` and
/// `24:00:00` are both read, a timestamp's carrying into the next day.
pub(crate) fn parse_time_of_day(s: &str) -> Read<(i64, i64)> {
    let unparsed = Unread::Unparsed;
    let (hms, frac) = s.split_once('.').unwrap_or((s, ""));
    let mut parts = hms.splitn(3, ':');
    let mut part = || parts.next().and_then(unsigned_part).ok_or(unparsed);
    let (h, mi, se) = (part()?, part()?, part()?);
    if parts.next().is_some() || frac.len() > 6 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Err(unparsed);
    }
    // pg-refuses: I61 — `time_overflows`, part by part.
    if h > 24 || mi > 59 || se > 60 {
        return Err(Unread::Refused);
    }
    // `frac` is checked above to be at most six ASCII digits, so padding it
    // to six and parsing the result — two allocations per field — is exactly
    // a scale by a power of ten: `.5` is 500000 µs, `.000001` is 1.
    let mut micros: i64 = 0;
    for b in frac.bytes() {
        micros = micros * 10 + i64::from(b - b'0');
    }
    let (seconds, micros) = (h * 3600 + mi * 60 + se, micros * POW10[6 - frac.len()]);
    // pg-refuses: I61 — and past `24:00:00` as a whole.
    if seconds * 1_000_000 + micros > DAY_MICROS {
        return Err(Unread::Refused);
    }
    Ok((seconds, micros))
}

/// The exact inverse of [`parse_time_of_day`]'s micros-since-midnight value —
/// shared by [`render_time64_micros`] and [`render_timestamp_micros`]'s
/// time-of-day component. PostgreSQL trims trailing zeros from the fraction
/// (`00:00:00.5`, not `.500000`) and omits it entirely when zero.
fn format_hms_frac_into(out: &mut String, total_micros: i64) {
    let seconds = total_micros.div_euclid(1_000_000);
    let micros = total_micros.rem_euclid(1_000_000);
    push_two(out, seconds / 3600);
    out.push(':');
    push_two(out, (seconds % 3600) / 60);
    out.push(':');
    push_two(out, seconds % 60);
    if micros != 0 {
        out.push('.');
        push_fraction(out, micros);
    }
}

/// Arrow's `Time64` day, in microseconds: the first value past it.
const DAY_MICROS: i64 = 86_400_000_000;

/// A `time`'s microseconds since midnight as PostgreSQL holds it, its
/// inclusive upper bound `24:00:00` included — what its comparison orders
/// (`crate::key`), where [`decode_time64_micros`] is what Arrow holds.
pub fn time_of_day_micros(s: &str) -> Read<i64> {
    let (seconds, micros) = parse_time_of_day(s)?;
    Ok(seconds * 1_000_000 + micros)
}

/// `None` for anything unparseable, and for `24:00:00`: a real, valid
/// boundary value (PostgreSQL's inclusive upper bound for `time`) that
/// Arrow's `Time64`, holding `[0, 86400 s)`, cannot, so it is refused as
/// every other unrepresentable value is (the anchor on [`decode_date32`]).
pub fn decode_time64_micros(s: &str) -> Option<i64> {
    time_of_day_micros(s).ok().filter(|&micros| micros < DAY_MICROS)
}

pub fn render_time64_micros(v: i64) -> String {
    // `HH:MM:SS.ffffff`.
    let mut out = String::with_capacity(16);
    render_time64_micros_into(v, &mut out);
    out
}

/// [`render_time64_micros`] appending to a caller's buffer.
pub fn render_time64_micros_into(v: i64, out: &mut String) {
    format_hms_frac_into(out, v);
}

/// Pull a `+HH[:MM[:SS]]` / `-HH[:MM[:SS]]` UTC offset off the end of a
/// timestamptz's time-of-day text, returning what's left and the offset in
/// seconds (positive east of UTC, matching the sign convention `local - offset
/// = UTC`). The offset is the only place a `+`/`-` can occur in this
/// substring — the fractional-seconds part, if any, is digits and a `.` only.
///
/// `timetz_out` writes the same `EncodeTimezone` form (I40), so
/// `crate::key` splits a `time with time zone` with this. What it does
/// with the two halves is not what a timestamp does: the offset is kept, not
/// discarded, because two `timetz` values are equal only when the zone
/// matches as well as the instant.
///
/// Refused past `DecodeTimezone`'s `±15:59:59`, part by part. The
/// run-together `+0530`, which the server reads as `+05:30`, is unparsed, an
/// hour of more than two digits being a shortfall rather than an hour past
/// fifteen (`docs/design/decisions.md`, "D55").
pub(crate) fn extract_offset(s: &str) -> Read<(&str, i64)> {
    let unparsed = Unread::Unparsed;
    let idx = s.find(['+', '-']).ok_or(unparsed)?;
    let (time_only, off) = s.split_at(idx);
    let (sign, rest) = off.split_at(1);
    let sign_mult: i64 = if sign == "-" { -1 } else { 1 };
    let mut parts = rest.splitn(3, ':');
    let hours = parts.next().ok_or(unparsed)?;
    if hours.len() > 2 {
        return Err(unparsed);
    }
    let hh = unsigned_part(hours).ok_or(unparsed)?;
    let mut part = || parts.next().map_or(Some(0), unsigned_part).ok_or(unparsed);
    let (mm, ss) = (part()?, part()?);
    // pg-refuses: I61 — `MAX_TZDISP_HOUR`, and a minute or second past 59.
    if hh > 15 || mm > 59 || ss > 59 {
        return Err(Unread::Refused);
    }
    Ok((time_only, sign_mult * (hh * 3600 + mm * 60 + ss)))
}

/// Microseconds since the 1970-01-01 UTC epoch. `with_tz` selects
/// `timestamp with time zone` parsing (an offset is expected and subtracted
/// out — I4: the offset is always explicit in the data, and this build
/// normalizes to UTC same as `pg_dump`'s own `extra_float_digits`-independent
/// convention) versus `timestamp without time zone` (a bare wall-clock
/// reading, stored as-is). `None` for `infinity`/`-infinity` — real
/// PostgreSQL values, but `Timestamp` has no sentinel for them, same
/// reasoning as [`decode_date32`].
pub fn decode_timestamp_micros(s: &str, with_tz: bool) -> Option<i64> {
    i64::try_from(timestamp_micros_wide(s, with_tz).ok()?).ok()
}

/// [`decode_timestamp_micros`] before it narrows to `i64`: microseconds from
/// the 1970 UTC epoch for any finite value the grammar reads and PostgreSQL's
/// range holds, so a value past what `Timestamp(Microsecond)` holds is told
/// from one that does not parse (`crate::unrepresentable`).
pub(crate) fn timestamp_micros_wide(s: &str, with_tz: bool) -> Read<i128> {
    if s == "infinity" || s == "-infinity" {
        return Err(Unread::Unparsed);
    }
    let (rest, bc) = split_era(s);
    let (date_part, time_part) = rest.split_once(' ').ok_or(Unread::Unparsed)?;
    let days = civil_days(date_part, bc)?;

    let (time_only, offset_secs) =
        if with_tz { extract_offset(time_part)? } else { (time_part, 0) };
    let (seconds, micros) = parse_time_of_day(time_only)?;

    let local_micros =
        i128::from(days) * 86_400_000_000 + i128::from(seconds) * 1_000_000 + i128::from(micros);
    let utc = local_micros - i128::from(offset_secs) * 1_000_000;
    // pg-refuses: I61 — `IS_VALID_TIMESTAMP`, of the instant: an offset can
    // carry a local time on either side of the range across its edge.
    let postgres = utc - POSTGRES_EPOCH_UNIX_MICROS;
    if !(i128::from(MIN_TIMESTAMP)..i128::from(END_TIMESTAMP)).contains(&postgres) {
        return Err(Unread::Refused);
    }
    Ok(utc)
}

/// `MIN_TIMESTAMP`, `4714-11-24 00:00:00 BC`, the first instant a timestamp
/// holds, in microseconds from PostgreSQL's epoch (I49).
const MIN_TIMESTAMP: i64 = -211_813_488_000_000_000;

/// `END_TIMESTAMP`, `294277-01-01 00:00:00`, the first instant past a
/// timestamp's range, in microseconds from PostgreSQL's epoch (I49).
const END_TIMESTAMP: i64 = 9_223_371_331_200_000_000;

/// PostgreSQL's epoch, 2000-01-01 00:00:00 UTC, in microseconds from the
/// Unix epoch.
const POSTGRES_EPOCH_UNIX_MICROS: i128 = 946_684_800_000_000;

/// A finite timestamp's microseconds from PostgreSQL's epoch, 2000-01-01 UTC —
/// the `int64` PostgreSQL stores it as (I49), so every value it admits fits,
/// the three decades [`decode_timestamp_micros`] cannot count from 1970
/// included. What its comparison orders (`crate::key`), where
/// [`decode_timestamp_micros`] is what Arrow holds; [`Unread::Unparsed`] for
/// the infinities, and an [`Unread`] for text that is not a timestamp.
pub fn timestamp_postgres_micros(s: &str, with_tz: bool) -> Read<i64> {
    let postgres = timestamp_micros_wide(s, with_tz)? - POSTGRES_EPOCH_UNIX_MICROS;
    Ok(i64::try_from(postgres).expect("PostgreSQL's range is an i64 of microseconds"))
}

/// [`render_timestamp_micros`] of a [`timestamp_postgres_micros`] value, so a
/// timestamp past what `i64` counts from 1970 is written as the dump writes it.
pub fn render_timestamp_postgres_micros(v: i64, with_tz: bool) -> String {
    let unix = i128::from(v) + POSTGRES_EPOCH_UNIX_MICROS;
    let days = i64::try_from(unix.div_euclid(86_400_000_000)).expect("an i64 of micros is days");
    let of_day = i64::try_from(unix.rem_euclid(86_400_000_000)).expect("under a day");
    let mut out = String::with_capacity(32);
    push_timestamp(days, of_day, with_tz, &mut out);
    out
}

pub fn render_timestamp_micros(v: i64, with_tz: bool) -> String {
    // `YYYY-MM-DD HH:MM:SS.ffffff+00`, with room for the era marker.
    let mut out = String::with_capacity(32);
    render_timestamp_micros_into(v, with_tz, &mut out);
    out
}

/// [`render_timestamp_micros`] appending to a caller's buffer.
pub fn render_timestamp_micros_into(v: i64, with_tz: bool, out: &mut String) {
    push_timestamp(v.div_euclid(86_400_000_000), v.rem_euclid(86_400_000_000), with_tz, out);
}

/// A timestamp's text out of its day count from 1970 and its microseconds
/// into that day.
fn push_timestamp(days: i64, of_day: i64, with_tz: bool, out: &mut String) {
    let bc = push_civil_date(out, days);
    out.push(' ');
    format_hms_frac_into(out, of_day);
    if with_tz {
        out.push_str("+00");
    }
    if bc {
        out.push_str(" BC");
    }
}

/// A signed count of `year`/`mon`/`day` units out of an `interval`'s text.
/// The leading `+` is `AddPostgresIntPart`'s, written on a positive part that
/// follows a negative one (I40); everything else is digits.
fn interval_count(text: &str) -> Option<i64> {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let magnitude: i64 = digits.parse().ok()?;
    Some(if text.starts_with('-') { -magnitude } else { magnitude })
}

/// The `[+|-]HH:MM:SS[.ffffff]` tail of an `interval`, in microseconds.
/// `EncodeInterval` writes one sign for the whole time part — `minus` is set
/// if any of hours, minutes, seconds or the fraction is negative, and the
/// three fields are then printed as absolute values — so the sign is applied
/// to the total rather than per field (I40).
///
/// The hour field is bounded only by the total, so this is not
/// [`decode_time64_micros`]: `720:00:00` is an ordinary `interval` and not a
/// `time`. Every field is checked to be digits, which is what keeps
/// `04:-5:06` — a string no `interval_out` writes and no `interval_in`
/// accepts — from parsing as a negative minute count.
fn interval_time_micros(text: &str) -> Read<i64> {
    let unparsed = Unread::Unparsed;
    let (negative, rest) = match text.strip_prefix(['+', '-']) {
        Some(rest) => (text.starts_with('-'), rest),
        None => (false, text),
    };
    let (hms, frac) = rest.split_once('.').unwrap_or((rest, ""));
    let mut parts = hms.split(':');
    let mut field = |max_len: Option<usize>| -> Read<i128> {
        let digits = parts.next().ok_or(unparsed)?;
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(unparsed);
        }
        if max_len.is_some_and(|n| digits.len() != n) {
            return Err(unparsed);
        }
        digits.parse::<i128>().map_err(|_| unparsed)
    };
    let hours = field(None)?;
    let minutes = field(Some(2))?;
    let seconds = field(Some(2))?;
    if parts.next().is_some() {
        return Err(unparsed);
    }
    // pg-refuses: I62 — `DecodeTimeCommon`'s minute past 59 and second past 60.
    if minutes > 59 || seconds > 60 {
        return Err(Unread::Refused);
    }
    if frac.is_empty() && text.contains('.') {
        return Err(unparsed);
    }
    if frac.len() > 6 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Err(unparsed);
    }
    let micros: i128 = if frac.is_empty() {
        0
    } else {
        let mut padded = frac.to_string();
        padded.push_str(&"0".repeat(6 - padded.len()));
        padded.parse().map_err(|_| unparsed)?
    };
    // pg-refuses: I62 — the `Interval` struct's `int64` microseconds, either
    // sign: the text carries the magnitude, so `i64::MIN` is no value of it.
    let total = hours
        .checked_mul(3600)
        .and_then(|t| t.checked_add(minutes * 60 + seconds))
        .and_then(|t| t.checked_mul(1_000_000))
        .and_then(|t| t.checked_add(micros))
        .and_then(|t| i64::try_from(t).ok())
        .ok_or(Unread::Refused)?;
    Ok(if negative { -total } else { total })
}

/// The three parts of an `interval`'s text as PostgreSQL's `Interval` struct
/// holds them: whole months (a `year` part folded in at twelve each), whole
/// days, and the time tail in **microseconds**. The *comparison* built on
/// this fuses all three into a 128-bit span (`interval_cmp_value`, I40),
/// where the *decode* widens the time to Arrow's nanoseconds — one walk, two
/// consumers, and neither step belongs to it.
///
/// **The grammar covers `interval_out`'s under `IntervalStyle = postgres`**,
/// which `pg_dump` pins on its own connection (I4): an optional
/// `<n> year[s]`, `<n> mon[s]` and `<n> day[s]`, then an optional signed time
/// part, separated by single spaces, with a wholly-zero interval written
/// `00:00:00`. It is a little wider than that grammar — the counted parts are
/// taken in any order, either number of their unit, a `+` on any of them,
/// and the hours at any width — but no other unit: `1 hour`, `1.5 hours`,
/// `P1Y2M` and `1 month` are all spellings `interval_in` takes and
/// `interval_out` never writes, and are unparsed. `infinity`/`-infinity`
/// (v17's, I34) are not in the grammar either, so they fail here and each
/// consumer says what it does about them.
pub(crate) fn interval_parts(text: &str) -> Read<(i32, i32, i64)> {
    let tokens: Vec<&str> = text.split(' ').collect();
    // Years, months and days, each counted at most once.
    let mut counts: [Option<i32>; 3] = [None; 3];
    let mut time = 0i64;
    let mut at = 0;
    while at < tokens.len() {
        let unit = match tokens.get(at + 1).copied() {
            Some("year" | "years") => 0,
            Some("mon" | "mons") => 1,
            Some("day" | "days") => 2,
            // Not a counted part, so this token is the time tail — which is
            // last, and of which there is at most one.
            _ => {
                if at + 1 != tokens.len() {
                    return Err(Unread::Unparsed);
                }
                time = interval_time_micros(tokens[at])?;
                at += 1;
                break;
            }
        };
        // pg-refuses: I62 — a unit given twice, and a count past `int32`
        // but not past `i64`, which is unparsed.
        if counts[unit].is_some() {
            return Err(Unread::Refused);
        }
        let count = interval_count(tokens[at]).ok_or(Unread::Unparsed)?;
        counts[unit] = Some(i32::try_from(count).map_err(|_| Unread::Refused)?);
        at += 2;
    }
    if at != tokens.len() {
        return Err(Unread::Unparsed);
    }
    let [years, months, days] = counts.map(|count| i64::from(count.unwrap_or(0)));
    // pg-refuses: I62 — `itmin2interval`'s month total past `int32`.
    let months = i32::try_from(years * 12 + months).map_err(|_| Unread::Refused)?;
    Ok((months, i32::try_from(days).expect("a day count is an i32"), time))
}

/// `interval`, as Arrow's `Interval(MonthDayNano)` holds it: months, days and
/// a **nanosecond** time part, which are exactly PostgreSQL's own three
/// independent fields. `None` for a spelling outside [`interval_parts`]'s
/// grammar, which `interval_in` may read, and for three classes of value,
/// each a decode failure by construction the way [`decode_date32`]'s
/// infinities are:
///
/// - `infinity` and `-infinity` (v17's, I34) — every field of PostgreSQL's
///   struct is extremal and Arrow has no encoding for the value at all;
/// - a time part past `2562047:47:16.854775807` — PostgreSQL's field is
///   `int64` *microseconds* against Arrow's `int64` nanoseconds, a
///   thousandth of the range, and nothing normalizes hours into days (I40);
/// - a value `interval_in` refuses, which [`interval_parts`] refuses (I62).
// deficiency: KD99 — a field is read as written, not through its column's
// typmod: `AdjustIntervalForTypmod` zeroes the fields right of an `interval`'s
// qualifier and rounds its time part to the precision, and
// `AdjustTimeForTypmod` and `AdjustTimestampForTypmod` round a `time(p)`,
// `timetz(p)`, `timestamp(p)` or `timestamptz(p)` field, so `1 year 3 mons` in
// an `interval year` or `12:00:00.5` in a `time(0)` emits a value the server
// does not hold, and keys and compares as it. No `pg_dump` writes one, its
// `*_out` text being of the stored value. The fix carries each typmod to its
// kind, as `CompareKind::Interval` carries the qualifier, and adjusts the
// field in the read, the key and the bounds, as `typmod_unscaled_digits` does
// a `numeric(p,s)`.
pub fn decode_interval(s: &str) -> Option<(i32, i32, i64)> {
    let (months, days, micros) = interval_parts(s).ok()?;
    Some((months, days, micros.checked_mul(1_000)?))
}

/// `EncodeInterval` under `INTSTYLE_POSTGRES`, the exact inverse of
/// [`decode_interval`] (I40). Three rules carry the whole form, and none of
/// them is `format_hms_frac_into`'s: a part is suppressed when its value is zero,
/// its unit takes an `s` whenever the value is not exactly `1`, and a part
/// that is positive and *follows a negative one* carries a `+`. The time tail
/// is written when it is nonzero or when nothing else was — which is what
/// makes a wholly-zero interval `00:00:00` — with one sign for the whole
/// tail, its hour field at least two digits and unbounded above.
///
/// **`None` for a `nanos` that is not a whole number of microseconds.**
/// PostgreSQL's time field counts microseconds, so no `interval` has such a
/// value and there is no text to write; the value is refused rather than
/// truncated (`docs/design/decisions.md`, "D44"). [`decode_interval`] cannot
/// produce one — it multiplies microseconds by a thousand — so this is
/// reachable only from an `Interval(MonthDayNano)` array a caller built.
pub fn render_interval(months: i32, days: i32, nanos: i64) -> Option<String> {
    if nanos % 1_000 != 0 {
        return None;
    }
    let mut out = String::new();
    let mut is_zero = true;
    let mut is_before = false;
    let months = i64::from(months);
    for (value, unit) in [(months / 12, "year"), (months % 12, "mon"), (i64::from(days), "day")] {
        if value == 0 {
            continue;
        }
        if !is_zero {
            out.push(' ');
        }
        if is_before && value > 0 {
            out.push('+');
        }
        out.push_str(&value.to_string());
        out.push(' ');
        out.push_str(unit);
        if value != 1 {
            out.push('s');
        }
        is_before = value < 0;
        is_zero = false;
    }
    let micros = nanos / 1_000;
    let (hours, rest) = (micros / 3_600_000_000, micros % 3_600_000_000);
    let (minutes, rest) = (rest / 60_000_000, rest % 60_000_000);
    let (seconds, frac) = (rest / 1_000_000, rest % 1_000_000);
    if is_zero || micros != 0 {
        if !is_zero {
            out.push(' ');
        }
        out.push_str(if micros < 0 {
            "-"
        } else if is_before {
            "+"
        } else {
            ""
        });
        out.push_str(&format!("{:02}:{:02}:{:02}", hours.abs(), minutes.abs(), seconds.abs()));
        if frac != 0 {
            let mut digits = format!("{:06}", frac.abs());
            while digits.ends_with('0') {
                digits.pop();
            }
            out.push('.');
            out.push_str(&digits);
        }
    }
    Some(out)
}

/// Nibble value per byte, `BAD_NIBBLE` for anything that is not a hex digit.
/// The two hex decoders below loop per *byte* over a field of any length,
/// so they read a table rather than branching through
/// [`pgdt_core::copy::hex_val`]'s three ranges: a pair becomes two loads, a
/// shift and an or, and validity is one bit test on the accumulated `or`.
const HEX_NIBBLE: [u8; 256] = {
    let mut table = [BAD_NIBBLE; 256];
    let mut b = 0usize;
    while b < 256 {
        table[b] = match b as u8 {
            d @ b'0'..=b'9' => d - b'0',
            d @ b'a'..=b'f' => d - b'a' + 10,
            d @ b'A'..=b'F' => d - b'A' + 10,
            _ => BAD_NIBBLE,
        };
        b += 1;
    }
    table
};

/// Out of a nibble's range, and out of the low four bits, so `hi | lo` of a
/// pair carries "either was bad" in its high nibble.
const BAD_NIBBLE: u8 = 0xFF;

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

/// The 256 lowercase hex pairs end to end, so byte `b`'s pair is the two
/// bytes at `b * 2` — [`HEX_NIBBLE`]'s counterpart in the render direction.
/// The two renderers below loop per *byte* over a value of any length, and
/// each knows its whole output length before it starts, so a byte
/// becomes an indexed slice and a two-byte copy into a pre-sized `String`
/// rather than a trip through `core::fmt`
/// (`docs/design/decisions.md`, "D44").
const HEX_PAIRS_BYTES: [u8; 512] = {
    let digits = *b"0123456789abcdef";
    let mut table = [0u8; 512];
    let mut b = 0usize;
    while b < 256 {
        table[b * 2] = digits[b >> 4];
        table[b * 2 + 1] = digits[b & 0x0F];
        b += 1;
    }
    table
};

/// The same table as text, so a renderer appends a pair with `push_str` and
/// pays no UTF-8 validation; the conversion happens once, at compile time.
static HEX_PAIRS: &str = match str::from_utf8(&HEX_PAIRS_BYTES) {
    Ok(s) => s,
    Err(_) => panic!("hex digits are ASCII"),
};

fn push_hex_pair(out: &mut String, byte: u8) {
    let at = byte as usize * 2;
    out.push_str(&HEX_PAIRS[at..at + 2]);
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

/// Either of `byteaout`'s forms, chosen by a `bytea_output` `pg_dump` does
/// not pin (I4): `hex`, `\x` followed by hex pairs, and `escape`
/// ([`decode_bytea_escape`]). An `escape` value never opens `\x`, so the
/// prefix tells the two apart (I56). The decoded text (a single backslash),
/// not the doubled-backslash form COPY escaping writes to disk (see the
/// module docs).
pub fn decode_bytea(s: &str) -> Option<Vec<u8>> {
    match s.strip_prefix("\\x") {
        Some(hex) => decode_bytea_hex(hex),
        None => decode_bytea_escape(s, usize::MAX),
    }
}

/// The hex pairs after the `\x`.
fn decode_bytea_hex(hex: &str) -> Option<Vec<u8>> {
    let b = hex.as_bytes();
    if !b.len().is_multiple_of(2) {
        return None;
    }
    // The whole field is one `bytea` value, so validity is accumulated and
    // tested once rather than branched on per pair; a bad nibble poisons the
    // high bits of `bad` and the vector is dropped unread.
    let mut out = Vec::with_capacity(b.len() / 2);
    let mut bad = 0u8;
    for [first, second] in b.as_chunks::<2>().0 {
        let hi = HEX_NIBBLE[*first as usize];
        let lo = HEX_NIBBLE[*second as usize];
        bad |= hi | lo;
        out.push(hi << 4 | lo);
    }
    if bad & 0xF0 != 0 {
        return None;
    }
    Some(out)
}

/// `byteaout`'s `escape` form, and no wider (`docs/design/decisions.md`,
/// "D55"): a byte from space to `~` stands for itself but `\`, which is
/// `\\`, and every other byte is `\ooo`. Only the first `limit` bytes are
/// kept, the whole text still checked, so a caller wanting a value's head
/// pays for no more of it.
pub fn decode_bytea_escape(s: &str, limit: usize) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len().min(limit));
    let mut at = 0;
    while let Some(&first) = b.get(at) {
        let (byte, width) = match first {
            b'\\' => match b.get(at + 1..at + 4) {
                _ if b.get(at + 1) == Some(&b'\\') => (b'\\', 2),
                Some(&[o1 @ b'0'..=b'3', o2 @ b'0'..=b'7', o3 @ b'0'..=b'7']) => {
                    let byte = (o1 - b'0') << 6 | (o2 - b'0') << 3 | (o3 - b'0');
                    if ESCAPE_PRINTS_ITSELF.contains(&byte) {
                        return None;
                    }
                    (byte, 4)
                }
                _ => return None,
            },
            byte if ESCAPE_PRINTS_ITSELF.contains(&byte) => (byte, 1),
            _ => return None,
        };
        if out.len() < limit {
            out.push(byte);
        }
        at += width;
    }
    Some(out)
}

/// The bytes `byteaout`'s `escape` form writes as themselves, `\` aside.
const ESCAPE_PRINTS_ITSELF: std::ops::RangeInclusive<u8> = 0x20..=0x7E;

/// [`decode_bytea_escape`]'s inverse: `byteaout`'s `escape` form of `bytes`.
pub fn render_bytea_escape(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &byte in bytes {
        match byte {
            b'\\' => out.push_str("\\\\"),
            byte if ESCAPE_PRINTS_ITSELF.contains(&byte) => out.push(char::from(byte)),
            byte => {
                out.push('\\');
                for shift in [6, 3, 0] {
                    out.push(char::from(b'0' + (byte >> shift & 0o7)));
                }
            }
        }
    }
    out
}

/// `byteaout`'s `hex` form, PostgreSQL's default and the one a typed
/// `bytea` renders back as whichever form the dump held
/// (`docs/design/decisions.md`, "D66").
pub fn render_bytea(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("\\x");
    for byte in bytes {
        push_hex_pair(&mut out, *byte);
    }
    out
}

/// Why [`decode_bytea`] read no value from `s`: `byteain` refuses it, or
/// reads a spelling this build does not — a blank between hex pairs, any
/// byte the `escape` form writes escaped written as itself, or the reverse
/// (`docs/design/decisions.md`, "D55").
// pg-refuses: I69 — every refusal here is `byteain`'s.
pub fn bytea_unread(s: &str) -> Unread {
    let b = s.as_bytes();
    let reads = match b.strip_prefix(b"\\x") {
        // `hex_decode_safe`: a blank between pairs, never inside one.
        Some(hex) => {
            let mut digits = hex.iter().filter(|&&c| !matches!(c, b' ' | b'\n' | b'\t' | b'\r'));
            let mut pairs = hex.split(|&c| matches!(c, b' ' | b'\n' | b'\t' | b'\r'));
            digits.all(u8::is_ascii_hexdigit) && pairs.all(|run| run.len() % 2 == 0)
        }
        None => {
            let mut at = 0;
            loop {
                match b.get(at..) {
                    Some([]) | None => break true,
                    Some([b'\\', b'0'..=b'3', b'0'..=b'7', b'0'..=b'7', ..]) => at += 4,
                    Some([b'\\', b'\\', ..]) => at += 2,
                    Some([b'\\', ..]) => break false,
                    Some(_) => at += 1,
                }
            }
        }
    };
    if reads { Unread::Unparsed } else { Unread::Refused }
}

/// An `inet` or `cidr` value as `network_in` reads it: its family (`true`
/// for IPv6), its netmask's bits, and its address left-aligned in sixteen
/// bytes. `None` where the server refuses the text.
///
/// The family is IPv6 wherever the text holds a `:`. An IPv4 `inet` is
/// dotted decimal, `0` to `255` an octet, leading zeros and a trailing dot
/// read, and a netmask past the octets written only where they reach it
/// (`10.1.2/24`); a `cidr` reads an abbreviated or hex network besides,
/// with a classful netmask where none is written (`10` is `10.0.0.0/8`).
/// An IPv6 value of either takes an IPv4 tail and a netmask written with no
/// leading zero. A `cidr` refuses a bit set below its netmask.
///
/// A port of `inet_net_pton.c`, its quirks included: a netmask is
/// accumulated in a wrapping `int`, as `-fwrapv` builds the server
/// (`1.2.3.4/4294967304` is a `/8`), and an IPv4 tail may be short or hold
/// an empty octet (`::1..2.3` is `::1.0.2.3`).
// pg-refuses: I67 — every refusal here is `network_in`'s.
pub(crate) fn network_in(text: &str, cidr: bool) -> Option<(bool, u8, [u8; 16])> {
    let b = text.as_bytes();
    let v6 = b.contains(&b':');
    let mut addr = [0u8; 16];
    let bits = if v6 {
        inet_pton_ipv6(b, &mut addr)?
    } else if cidr {
        cidr_pton_ipv4(b, &mut addr)?
    } else {
        inet_pton_ipv4(b, &mut addr)?
    };
    let maxbits = if v6 { 128 } else { 32 };
    let bits = u8::try_from(bits).ok().filter(|&bits| bits <= maxbits)?;
    // `addressOK`: no bit set below the netmask.
    if cidr && (bits..maxbits).any(|bit| addr[usize::from(bit / 8)] & (0x80 >> (bit % 8)) != 0) {
        return None;
    }
    Some((v6, bits, addr))
}

/// The byte at `at`, `0` past the end, as a C string reads its terminator.
fn c_byte(b: &[u8], at: usize) -> u8 {
    b.get(at).copied().unwrap_or(0)
}

/// A netmask's digits from `*at`, the first already known to be one,
/// accumulated as the server's `int` accumulates them, wrapping.
fn wrapping_bits(b: &[u8], at: &mut usize) -> i32 {
    let mut bits = 0i32;
    while c_byte(b, *at).is_ascii_digit() {
        bits = bits.wrapping_mul(10).wrapping_add(i32::from(b[*at] - b'0'));
        *at += 1;
    }
    bits
}

/// `inet_net_pton_ipv4`: an `inet`'s IPv4 address, `None` on `ENOENT` or
/// `EMSGSIZE`.
fn inet_pton_ipv4(b: &[u8], dst: &mut [u8; 16]) -> Option<i32> {
    let mut at = 0;
    let mut len = 0;
    let mut ch = c_byte(b, at);
    while ch.is_ascii_digit() {
        let mut octet = 0u32;
        while ch.is_ascii_digit() {
            octet = octet * 10 + u32::from(ch - b'0');
            if octet > 255 {
                return None;
            }
            at += 1;
            ch = c_byte(b, at);
        }
        if len == 4 {
            return None;
        }
        dst[len] = octet as u8;
        len += 1;
        if ch == 0 || ch == b'/' {
            break;
        }
        if ch != b'.' {
            return None;
        }
        at += 1;
        ch = c_byte(b, at);
    }
    let mut bits = -1;
    if ch == b'/' && c_byte(b, at + 1).is_ascii_digit() && len > 0 {
        at += 1;
        bits = wrapping_bits(b, &mut at);
        ch = c_byte(b, at);
        if ch != 0 || bits > 32 {
            return None;
        }
    }
    if ch != 0 || len == 0 {
        return None;
    }
    if bits == -1 {
        if len != 4 {
            return None;
        }
        bits = 32;
    }
    // A netmask may not reach past the octets written; C's division
    // truncates a wrapped negative one toward zero, so it passes here.
    if bits / 8 > len as i32 {
        return None;
    }
    Some(bits)
}

/// `inet_cidr_pton_ipv4` at four bytes: a `cidr`'s IPv4 network, `None` on
/// `ENOENT` or `EMSGSIZE`.
fn cidr_pton_ipv4(b: &[u8], dst: &mut [u8; 16]) -> Option<i32> {
    let mut len = 0;
    let mut at = 1;
    let mut ch = c_byte(b, 0);
    if ch == b'0' && matches!(c_byte(b, 1), b'x' | b'X') && c_byte(b, 2).is_ascii_hexdigit() {
        // A string of nibbles, an odd last one the high half of its byte.
        at = 2;
        let mut nibbles = 0;
        let mut byte = 0u8;
        loop {
            ch = c_byte(b, at);
            at += 1;
            if !ch.is_ascii_hexdigit() {
                break;
            }
            let nibble = HEX_NIBBLE[usize::from(ch)];
            byte = if nibbles == 0 { nibble } else { byte << 4 | nibble };
            nibbles += 1;
            if nibbles == 2 {
                push_octet(dst, &mut len, byte)?;
                nibbles = 0;
            }
        }
        if nibbles == 1 {
            push_octet(dst, &mut len, byte << 4)?;
        }
    } else if ch.is_ascii_digit() {
        loop {
            let mut octet = 0u32;
            loop {
                octet = octet * 10 + u32::from(ch - b'0');
                if octet > 255 {
                    return None;
                }
                ch = c_byte(b, at);
                at += 1;
                if !ch.is_ascii_digit() {
                    break;
                }
            }
            push_octet(dst, &mut len, octet as u8)?;
            if ch == 0 || ch == b'/' {
                break;
            }
            if ch != b'.' {
                return None;
            }
            ch = c_byte(b, at);
            at += 1;
            if !ch.is_ascii_digit() {
                return None;
            }
        }
    } else {
        return None;
    }
    // `at` is one past `ch`, as the C's `src` is.
    let mut bits = -1;
    if ch == b'/' && c_byte(b, at).is_ascii_digit() && len > 0 {
        bits = wrapping_bits(b, &mut at);
        if c_byte(b, at) != 0 {
            return None;
        }
        if bits > 32 {
            return None;
        }
        ch = 0;
    }
    if ch != 0 || len == 0 {
        return None;
    }
    if bits == -1 {
        // The class's netmask, widened to the octets written.
        bits = match dst[0] {
            240.. => 32,
            224.. => 8,
            192.. => 24,
            128.. => 16,
            _ => 8,
        };
        bits = bits.max(len as i32 * 8);
        if bits == 8 && dst[0] == 224 {
            bits = 4;
        }
    }
    // The network extended with zero bytes to cover its netmask, which
    // four bytes must hold.
    while bits > len as i32 * 8 {
        push_octet(dst, &mut len, 0)?;
    }
    Some(bits)
}

/// One more of an IPv4 network's four bytes, `None` past the fourth
/// (`EMSGSIZE`).
fn push_octet(dst: &mut [u8; 16], len: &mut usize, byte: u8) -> Option<()> {
    if *len == 4 {
        return None;
    }
    dst[*len] = byte;
    *len += 1;
    Some(())
}

/// `inet_cidr_pton_ipv6` at sixteen bytes, which `inet` and `cidr` share:
/// `None` on `ENOENT`.
fn inet_pton_ipv6(b: &[u8], dst: &mut [u8; 16]) -> Option<i32> {
    let mut tmp = [0u8; 16];
    let mut tp = 0;
    let mut colonp = None;
    let mut at = 0;
    if c_byte(b, 0) == b':' {
        if c_byte(b, 1) != b':' {
            return None;
        }
        at = 1;
    }
    let mut curtok = at;
    let mut saw_xdigit = false;
    let mut val = 0u32;
    let mut digits = 0;
    let mut bits = -1;
    loop {
        let ch = c_byte(b, at);
        at += 1;
        if ch == 0 {
            break;
        }
        if ch.is_ascii_hexdigit() {
            val = val << 4 | u32::from(HEX_NIBBLE[usize::from(ch)]);
            digits += 1;
            if digits > 4 {
                return None;
            }
            saw_xdigit = true;
            continue;
        }
        if ch == b':' {
            curtok = at;
            if !saw_xdigit {
                if colonp.is_some() {
                    return None;
                }
                colonp = Some(tp);
                continue;
            }
            if c_byte(b, at) == 0 || tp + 2 > 16 {
                return None;
            }
            tmp[tp] = (val >> 8) as u8;
            tmp[tp + 1] = val as u8;
            tp += 2;
            saw_xdigit = false;
            digits = 0;
            val = 0;
            continue;
        }
        if ch == b'.' && tp + 4 <= 16 && getv4(&b[curtok..], &mut tmp[tp..tp + 4], &mut bits) {
            tp += 4;
            saw_xdigit = false;
            break;
        }
        if ch == b'/' && getbits(&b[at..], &mut bits) {
            break;
        }
        return None;
    }
    if saw_xdigit {
        if tp + 2 > 16 {
            return None;
        }
        tmp[tp] = (val >> 8) as u8;
        tmp[tp + 1] = val as u8;
        tp += 2;
    }
    if bits == -1 {
        bits = 128;
    }
    if let Some(colon) = colonp {
        // The groups after `::` moved to the end, zeros in their place.
        if tp == 16 {
            return None;
        }
        let n = tp - colon;
        for i in 1..=n {
            tmp[16 - i] = tmp[colon + n - i];
            tmp[colon + n - i] = 0;
        }
        tp = 16;
    }
    if tp != 16 {
        return None;
    }
    *dst = tmp;
    Some(bits)
}

/// `getbits`: an IPv6 netmask, `0` to `128` with no leading zero.
fn getbits(b: &[u8], bits: &mut i32) -> bool {
    let mut val = 0;
    let mut n = 0;
    for &ch in b {
        if !ch.is_ascii_digit() {
            return false;
        }
        if n != 0 && val == 0 {
            return false;
        }
        n += 1;
        val = val * 10 + i32::from(ch - b'0');
        if val > 128 {
            return false;
        }
    }
    if n == 0 {
        return false;
    }
    *bits = val;
    true
}

/// `getv4`: an IPv6 value's IPv4 tail into the four bytes `dst`, octets with
/// no leading zero, at most four, and a netmask after it by [`getbits`].
fn getv4(b: &[u8], dst: &mut [u8], bits: &mut i32) -> bool {
    let mut val = 0u32;
    let mut n = 0;
    let mut len = 0;
    for (i, &ch) in b.iter().enumerate() {
        if ch.is_ascii_digit() {
            if n != 0 && val == 0 {
                return false;
            }
            n += 1;
            val = val * 10 + u32::from(ch - b'0');
            if val > 255 {
                return false;
            }
            continue;
        }
        if ch == b'.' || ch == b'/' {
            if len > 3 {
                return false;
            }
            dst[len] = val as u8;
            len += 1;
            if ch == b'/' {
                return getbits(&b[i + 1..], bits);
            }
            val = 0;
            n = 0;
            continue;
        }
        return false;
    }
    if n == 0 || len > 3 {
        return false;
    }
    dst[len] = val as u8;
    true
}

/// Why a `macaddr` this build does not read is no value: `macaddr_in`
/// refuses it, or reads it — six octets in one of its seven `sscanf`
/// layouts, separated by `:` or `-`, grouped by `.` or `-` in fours or `:`
/// or `-` in sixes, or not at all, a digit run as short as one and blanks
/// around them — where this build reads `macaddr_out`'s colon-separated
/// pairs (`docs/design/decisions.md`, "D55").
///
/// `%x` is modelled for the hex digits it takes and the blanks it skips. An
/// octet past eight significant digits, which glibc truncates, is left
/// unvalued.
// deficiency: KD85 — a run opening with a sign or a `0x` is read by glibc's
// `%x`, which this does not model, so a text in which any layout tried meets
// one is counted read and never refused, `08:00:2b:01:02:-1` and
// `0x0800.2b01.0203` among those the server refuses (I68) — and so is a
// malformed dash-separated one, `222:222` meeting its `-` where `%2x` reads.
pub(crate) fn macaddr_unread(s: &str) -> Unread {
    // `x` is `%x`, `2` is `%2x`, anything else a byte the text must hold.
    const LAYOUTS: [&[u8]; 7] = [
        b"x:x:x:x:x:x",
        b"x-x-x-x-x-x",
        b"222:222",
        b"222-222",
        b"22.22.22",
        b"22-22-22",
        b"222222",
    ];
    for layout in LAYOUTS {
        match macaddr_scan(s.as_bytes(), layout) {
            MacScan::Unmodelled => return Unread::Unparsed,
            // The first layout taking six octets decides.
            // pg-refuses: I68 — an octet past 255.
            MacScan::Six(octets) => {
                return if octets.iter().all(|o| o.is_none_or(|o| o <= 255)) {
                    Unread::Unparsed
                } else {
                    Unread::Refused
                };
            }
            MacScan::Short => {}
        }
    }
    // pg-refuses: I68 — no layout takes six octets.
    Unread::Refused
}

/// What one `sscanf` layout of [`macaddr_unread`] makes of a text.
enum MacScan {
    /// Six octets and nothing after them but blanks, `None` for a run past
    /// eight significant digits, which glibc truncates to an `unsigned int`.
    Six([Option<u32>; 6]),
    /// Fewer than six, or a seventh field after them.
    Short,
    /// A sign or a `0x`, which glibc reads by rules the model does not
    /// follow.
    Unmodelled,
}

fn macaddr_scan(b: &[u8], layout: &[u8]) -> MacScan {
    let mut octets = [None; 6];
    let mut count = 0;
    let mut at = 0;
    for &field in layout {
        if !matches!(field, b'x' | b'2') {
            if b.get(at) != Some(&field) {
                return MacScan::Short;
            }
            at += 1;
            continue;
        }
        at += b[at..].iter().take_while(|&&c| is_c_space(c)).count();
        if let Some([b'+' | b'-', ..] | [b'0', b'x' | b'X', ..]) = b.get(at..) {
            return MacScan::Unmodelled;
        }
        let width = if field == b'2' { 2 } else { usize::MAX };
        let run = b[at..].iter().take(width).take_while(|c| c.is_ascii_hexdigit()).count();
        if run == 0 {
            return MacScan::Short;
        }
        let digits = &b[at..at + run];
        at += run;
        let significant = &digits[digits.iter().take_while(|&&c| c == b'0').count()..];
        octets[count] = (significant.len() <= 8).then(|| {
            significant
                .iter()
                .fold(0, |value, &c| value << 4 | u32::from(HEX_NIBBLE[usize::from(c)]))
        });
        count += 1;
    }
    // `%1s`: a seventh field wherever anything but blanks follows.
    if b[at..].iter().all(|&c| is_c_space(c)) { MacScan::Six(octets) } else { MacScan::Short }
}

/// `macaddr8_in`: the eight bytes the server reads, a six-byte address
/// widened by `FF:FE` in its middle, `None` where it refuses the text. Hex
/// pairs, each separated from the next by `:`, `-` or `.` — one of them
/// throughout — or by nothing, blanks around the whole, and its quirks: a
/// separator after the last pair, and a single stray character after six or
/// eight pairs, are read.
// pg-refuses: I68 — every refusal here is `macaddr8_in`'s.
pub(crate) fn macaddr8_in(s: &str) -> Option<[u8; 8]> {
    let b = s.as_bytes();
    let mut at = b.iter().take_while(|&&c| is_c_space(c)).count();
    let mut bytes = [0u8; 8];
    let mut count = 0;
    let mut spacer = None;
    while at + 1 < b.len() {
        if count == 8 {
            return None;
        }
        let (hi, lo) = (HEX_NIBBLE[usize::from(b[at])], HEX_NIBBLE[usize::from(b[at + 1])]);
        if (hi | lo) & 0xF0 != 0 {
            return None;
        }
        bytes[count] = hi << 4 | lo;
        count += 1;
        at += 2;
        if let Some(&c @ (b':' | b'-' | b'.')) = b.get(at) {
            if *spacer.get_or_insert(c) != c {
                return None;
            }
            at += 1;
        }
        if (count == 6 || count == 8) && b.get(at).is_some_and(|&c| is_c_space(c)) {
            if !b[at..].iter().all(|&c| is_c_space(c)) {
                return None;
            }
            at = b.len();
        }
    }
    match count {
        8 => Some(bytes),
        6 => Some([bytes[0], bytes[1], bytes[2], 0xFF, 0xFE, bytes[3], bytes[4], bytes[5]]),
        _ => None,
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

/// Whether a `character varying(n)` or `character(n)` field, `n` being
/// `length`, is one `varchar_input` or `bpchar_input` refuses: longer than
/// `length` characters, with anything but a blank, `0x20`, past them (I75).
/// The characters are counted in UTF-8, the one encoding read here; a field
/// no longer in bytes than `length` is read without counting, and a field it
/// does not refuse is read as written ([`crate::pgtype::CompareKind::Text`]).
// deficiency: KD96 — a field longer than `length` only by trailing blanks,
// which `varchar_input` and `bpchar_input` cut to `length` characters (I75),
// keeps them here, and a `character(n)` field shorter than `length`, which
// `bpchar_input` pads, is not padded. Either way a typed read emits a value
// the server does not hold; only a `character varying(n)`'s longer field also
// answers otherwise, keyed, bounded and compared with its blanks, so `<` and
// `=` differ from the server over it, where `character(n)`'s comparison trims
// both sides. No `pg_dump` writes either. The fix cuts the field in the read,
// the key and the bounds, and records per group whether any field was cut: a
// `SchemaMode::Strings` term reads the column as its text from the same set
// (D79), so it reads only a group where none was, never losing a real dump's
// bounds; `=`, `!=` and `IN` answer by the cut value only once `KD82` closes.
// `numeric(p,s)` is rounded where this is not because its rounding decides
// its refusal (I51), and no refusal here needs the cut. **(c) unowned**;
// promoted by a hand-written dump holding one.
// pg-refuses: I75 — a non-blank past the typmod's length.
pub fn char_typmod_refuses(text: &str, length: u32) -> bool {
    let length = length as usize;
    if text.len() <= length {
        return false;
    }
    text.char_indices().nth(length).is_some_and(|(cut, _)| text[cut..].bytes().any(|b| b != b' '))
}

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

/// Whether `json_in` reads `text`, which it stores as written: one JSON value
/// in RFC 8259's grammar, with blanks of `' '`, `\t`, `\n` and `\r` around
/// any token. A string holds no byte below `0x20` and no escape but the eight
/// single-character ones and `\u` with four hex digits; **what a `\u` escape
/// names is not checked**, so `\u0000` and a lone surrogate half, which
/// `jsonb_in` refuses, are read, as is a number past `numeric`'s range.
///
/// Iterative, over a stack of the containers open, so no depth refuses: the
/// server's recursion is bounded by its `max_stack_depth`, a setting of the
/// server restoring the dump (I72).
// pg-refuses: I72 — every refusal here is `json_in`'s.
pub(crate) fn json_in(text: &str) -> bool {
    let b = text.as_bytes();
    let mut at = 0;
    // The containers open around the value being read, innermost last:
    // `true` for an object.
    let mut open: Vec<bool> = Vec::new();
    loop {
        // A value is expected at `at`.
        json_blanks(b, &mut at);
        let ok = match b.get(at) {
            Some(b'{') => {
                at += 1;
                json_blanks(b, &mut at);
                if b.get(at) == Some(&b'}') {
                    at += 1;
                    true
                } else {
                    if !json_member_key(b, &mut at) {
                        return false;
                    }
                    open.push(true);
                    continue;
                }
            }
            Some(b'[') => {
                at += 1;
                json_blanks(b, &mut at);
                if b.get(at) == Some(&b']') {
                    at += 1;
                    true
                } else {
                    open.push(false);
                    continue;
                }
            }
            Some(b'"') => json_string(b, &mut at),
            Some(b'-' | b'0'..=b'9') => json_number(b, &mut at),
            _ => ["true", "false", "null"].iter().any(|word| {
                let found = b[at..].starts_with(word.as_bytes());
                at += if found { word.len() } else { 0 };
                found
            }),
        };
        if !ok {
            return false;
        }
        // A value has been read: close what it ends, or go on to the next.
        loop {
            json_blanks(b, &mut at);
            let Some(&object) = open.last() else { return at == b.len() };
            match b.get(at) {
                Some(b',') => {
                    at += 1;
                    if object && !json_member_key(b, &mut at) {
                        return false;
                    }
                    break;
                }
                Some(b'}') if object => {
                    at += 1;
                    open.pop();
                }
                Some(b']') if !object => {
                    at += 1;
                    open.pop();
                }
                _ => return false,
            }
        }
    }
}

/// `json_lex`'s blanks.
fn json_blanks(b: &[u8], at: &mut usize) {
    while matches!(b.get(*at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        *at += 1;
    }
}

/// An object member's key and its `:`, blanks around each.
fn json_member_key(b: &[u8], at: &mut usize) -> bool {
    json_blanks(b, at);
    if !(b.get(*at) == Some(&b'"') && json_string(b, at)) {
        return false;
    }
    json_blanks(b, at);
    let colon = b.get(*at) == Some(&b':');
    *at += usize::from(colon);
    colon
}

/// `json_lex_string` without de-escaping, its opening `"` at `at`.
fn json_string(b: &[u8], at: &mut usize) -> bool {
    *at += 1;
    loop {
        match b.get(*at) {
            Some(b'"') => {
                *at += 1;
                return true;
            }
            Some(b'\\') => match b.get(*at + 1) {
                Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => *at += 2,
                Some(b'u') => match b.get(*at + 2..*at + 6) {
                    Some(hex) if hex.iter().all(u8::is_ascii_hexdigit) => *at += 6,
                    _ => return false,
                },
                _ => return false,
            },
            Some(0x00..=0x1f) | None => return false,
            Some(_) => *at += 1,
        }
    }
}

/// `json_lex_number`: `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`. A
/// letter, digit or `_` right after it would join the token in the server's
/// lexer and fail it; here it fails as the next token, which no position takes.
fn json_number(b: &[u8], at: &mut usize) -> bool {
    let digits = |at: &mut usize| {
        let start = *at;
        while b.get(*at).is_some_and(u8::is_ascii_digit) {
            *at += 1;
        }
        *at > start
    };
    *at += usize::from(b.get(*at) == Some(&b'-'));
    match b.get(*at) {
        Some(b'0') => *at += 1,
        Some(b'1'..=b'9') => {
            digits(at);
        }
        _ => return false,
    }
    if b.get(*at) == Some(&b'.') {
        *at += 1;
        if !digits(at) {
            return false;
        }
    }
    if matches!(b.get(*at), Some(b'e' | b'E')) {
        *at += 1;
        *at += usize::from(matches!(b.get(*at), Some(b'+' | b'-')));
        if !digits(at) {
            return false;
        }
    }
    true
}

/// `VARBITMAXLEN`, the most bits a `bit` or `bit varying` value holds, which
/// bounds a hex spelling's digits at a quarter of it (I73).
const VARBIT_MAX_BITS: usize = i32::MAX as usize - 7;

/// Whether `bit_in`, or `varbit_in` where `varying`, reads `text` as a field
/// of a column whose typmod is `length`: binary digits, or after a leading `x`
/// or `X` hex digits of four bits each, a leading `b` or `B` optional before
/// binary ones; then exactly `length` bits for `bit(n)` and at most `length`
/// for `bit varying(n)`, any number for a column with no typmod. Nothing is
/// trimmed (I73).
// pg-refuses: I73 — every refusal here is `bit_in`'s or `varbit_in`'s.
pub(crate) fn bit_in(text: &str, varying: bool, length: Option<u32>) -> bool {
    let (hex, digits) = match text.as_bytes() {
        [b'x' | b'X', rest @ ..] => (true, rest),
        [b'b' | b'B', rest @ ..] => (false, rest),
        all => (false, all),
    };
    if hex && digits.len() > VARBIT_MAX_BITS / 4 {
        return false;
    }
    let bits = if hex { digits.len() * 4 } else { digits.len() };
    let fits = match length.map(|n| n as usize) {
        None => true,
        Some(n) if varying => bits <= n,
        Some(n) => bits == n,
    };
    fits && if hex {
        digits.iter().all(u8::is_ascii_hexdigit)
    } else {
        digits.iter().all(|&d| matches!(d, b'0' | b'1'))
    }
}

// ---------------------------------------------------------------------------
// The geometric types' input functions (I74)
// ---------------------------------------------------------------------------
//
// Each answers whether its `*_in` reads `text`, read or refused and nothing
// between, walking it as the C walks a NUL-terminated string: a byte past the
// end reads as `0`, which no field holds.

/// A point, as the geometric input functions read one.
#[derive(Debug, Clone, Copy, Default)]
struct Point {
    x: f64,
    y: f64,
}

/// `b[at]` as C reads it, `0` past the end.
fn c_at(b: &[u8], at: usize) -> u8 {
    b.get(at).copied().unwrap_or(0)
}

/// `at` past any blanks.
fn skip_c_spaces(b: &[u8], at: usize) -> usize {
    at + b.get(at..).map_or(0, |rest| rest.iter().take_while(|&&c| is_c_space(c)).count())
}

/// The run of ASCII digits from `at`, as its length.
fn digit_run(b: &[u8], at: usize) -> usize {
    b.get(at..).map_or(0, |rest| rest.iter().take_while(|c| c.is_ascii_digit()).count())
}

/// `float8in_internal` asked for its stopping point, as `single_decode` asks
/// it: the blanks around one number skipped and the number read as
/// [`strtod`] reads it — `None` where nothing is read or the value lies past
/// `double precision`'s range, which it refuses (I74).
fn single_decode(b: &[u8], at: usize) -> Option<(f64, usize)> {
    let (value, end, out_of_range) = strtod(b, skip_c_spaces(b, at))?;
    (!out_of_range).then(|| (value, skip_c_spaces(b, end)))
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
    // The leading significant digits, at most fifteen — sixty bits, past the
    // fifty-three a double keeps plus a rounding bit — and whether any digit
    // past them is nonzero; `exponent` is the power of two of
    // `mantissa`'s last bit.
    let (mut mantissa, mut kept, mut sticky) = (0u64, 0u32, false);
    let (mut exponent, mut digits, mut point) = (0i64, 0usize, false);
    loop {
        let c = c_at(b, i);
        if c == b'.' && !point {
            point = true;
            i += 1;
            continue;
        }
        let Some(digit) = char::from(c).to_digit(16) else { break };
        digits += 1;
        i += 1;
        if mantissa == 0 && digit == 0 {
            exponent -= if point { 4 } else { 0 };
        } else if kept < 15 {
            mantissa = mantissa << 4 | u64::from(digit);
            kept += 1;
            exponent -= if point { 4 } else { 0 };
        } else {
            sticky |= digit != 0;
            exponent += if point { 0 } else { 4 };
        }
    }
    if digits == 0 {
        return None;
    }
    let mut end = i;
    if matches!(c_at(b, i), b'p' | b'P') {
        let sign = usize::from(matches!(c_at(b, i + 1), b'+' | b'-'));
        let run = digit_run(b, i + 1 + sign);
        if run > 0 {
            // Saturated far past any exponent a double reaches either way.
            let power = b[i + 1 + sign..i + 1 + sign + run]
                .iter()
                .fold(0i64, |n, &d| (n * 10 + i64::from(d - b'0')).min(1 << 40));
            exponent += if c_at(b, i + 1) == b'-' { -power } else { power };
            end = i + 1 + sign + run;
        }
    }
    if mantissa == 0 {
        return Some((0.0, end, false));
    }
    let bits = i64::from(64 - mantissa.leading_zeros());
    let top = exponent + bits - 1;
    if top > 1023 {
        return Some((f64::INFINITY, end, true));
    }
    // The bits the result keeps: fifty-three for a normal double, fewer the
    // further a subnormal's leading bit sits below 2^-1022.
    let precision = if top >= -1022 { 53 } else { 53 - (-1022 - top) };
    if precision < 0 {
        return Some((0.0, end, true));
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
        // `rounded` counts multiples of 2^-1074, a carry into bit 52 being
        // the least normal double, which its bit pattern reads as.
        let value = f64::from_bits(rounded);
        return Some((value, end, value == 0.0));
    }
    let (fraction, top) = if rounded == 1 << 53 { (0, top + 1) } else { (rounded, top) };
    if top > 1023 {
        return Some((f64::INFINITY, end, true));
    }
    let biased = u64::try_from(top + 1023).ok()?;
    Some((f64::from_bits(biased << 52 | (fraction & ((1 << 52) - 1))), end, false))
}

/// `pair_decode` asked for its stopping point: `x,y`, optionally in
/// parentheses, the blanks after a closing one skipped.
fn pair_decode(b: &[u8], at: usize) -> Option<(Point, usize)> {
    let mut i = skip_c_spaces(b, at);
    let delimited = c_at(b, i) == b'(';
    i += usize::from(delimited);
    let (x, i) = single_decode(b, i)?;
    if c_at(b, i) != b',' {
        return None;
    }
    let (y, mut i) = single_decode(b, i + 1)?;
    if delimited {
        if c_at(b, i) != b')' {
            return None;
        }
        i = skip_c_spaces(b, i + 1);
    }
    Some((Point { x, y }, i))
}

/// `path_decode` asked for its stopping point: `npts` points, each followed
/// by an optional `,`, the last's included, opened by `[` only where `open` allows it, or by a `(`
/// holding them all — the first of two, or the last `(` in the text — and
/// closed to match, the blanks after each closing bracket skipped. The first
/// two points and where it stopped.
fn path_decode(b: &[u8], at: usize, open: bool, npts: usize) -> Option<([Point; 2], usize)> {
    let mut i = skip_c_spaces(b, at);
    let is_open = c_at(b, i) == b'[';
    let mut depth = 0;
    if is_open {
        if !open {
            return None;
        }
        depth = 1;
        i += 1;
    } else if c_at(b, i) == b'(' {
        // The first of two, which `pair_decode` then reads as the first
        // point's, or the last `(` in the text, skipped with the blanks after.
        let inner = skip_c_spaces(b, i + 1);
        if c_at(b, inner) == b'(' || b[i..].iter().rposition(|&c| c == b'(') == Some(0) {
            depth = 1;
            i = inner;
        }
    }
    let mut first = [Point::default(); 2];
    for k in 0..npts {
        let (point, next) = pair_decode(b, i)?;
        if let Some(slot) = first.get_mut(k) {
            *slot = point;
        }
        i = next + usize::from(c_at(b, next) == b',');
    }
    while depth > 0 {
        match c_at(b, i) {
            b')' => {}
            b']' if is_open && depth == 1 => {}
            _ => return None,
        }
        depth -= 1;
        i = skip_c_spaces(b, i + 1);
    }
    Some((first, i))
}

/// `pair_count`: the points a path or polygon holds, half its commas rounded
/// up — `None` where their count is even.
fn pair_count(b: &[u8]) -> Option<usize> {
    let commas = bytecount(b, b',');
    (commas % 2 == 1).then(|| commas.div_ceil(2))
}

/// How many times `byte` occurs in `b`.
fn bytecount(b: &[u8], byte: u8) -> usize {
    b.iter().filter(|&&c| c == byte).count()
}

/// `MaxAllocSize`, past which `palloc` refuses the value a path or polygon
/// of so many points would need.
const MAX_ALLOC_SIZE: usize = 0x3fff_ffff;

/// `offsetof(PATH, p)` and `offsetof(POLYGON, p)`: the bytes ahead of the
/// sixteen each point takes.
const PATH_HEADER: usize = 16;
const POLYGON_HEADER: usize = 40;

/// Whether `npts` points after `header` fit one allocation — what `path_in`
/// and `poly_in` ask before reading a point, their own overflow check
/// refusing only counts past this already.
fn fits_allocation(header: usize, npts: usize) -> bool {
    npts.checked_mul(16).and_then(|n| n.checked_add(header)).is_some_and(|n| n <= MAX_ALLOC_SIZE)
}

/// `EPSILON`, within which `FPzero` and `FPeq` call two values equal.
const GEO_EPSILON: f64 = 1.0e-6;

fn fp_zero(a: f64) -> bool {
    a.abs() <= GEO_EPSILON
}

fn fp_eq(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= GEO_EPSILON
}

/// `float8_eq`: a NaN equal to a NaN alone.
fn float8_eq(a: f64, b: f64) -> bool {
    if a.is_nan() { b.is_nan() } else { !b.is_nan() && a == b }
}

/// `point_eq_point`: within `EPSILON` on both axes, or exactly where a NaN
/// is involved.
fn point_eq_point(p: Point, q: Point) -> bool {
    if [p.x, p.y, q.x, q.y].iter().any(|v| v.is_nan()) {
        return float8_eq(p.x, q.x) && float8_eq(p.y, q.y);
    }
    fp_eq(p.x, q.x) && fp_eq(p.y, q.y)
}

/// `float8_mi`, `None` where it raises an overflow.
fn float8_mi(a: f64, b: f64) -> Option<f64> {
    let r = a - b;
    (!(r.is_infinite() && !a.is_infinite() && !b.is_infinite())).then_some(r)
}

/// `float8_mul`, `None` where it raises an overflow or an underflow.
fn float8_mul(a: f64, b: f64) -> Option<f64> {
    let r = a * b;
    let overflow = r.is_infinite() && !a.is_infinite() && !b.is_infinite();
    let underflow = r == 0.0 && a != 0.0 && b != 0.0;
    (!overflow && !underflow).then_some(r)
}

/// `float8_div`, `None` where it raises a division by zero, an overflow or an
/// underflow — as v14 on raise them, or as v13 does where `v13`, which also
/// refuses a NaN divided by zero, and calls a nonzero value divided by an
/// infinity an underflow. The two agree on overflow, a quotient by an
/// infinity never being one.
fn float8_div(a: f64, b: f64, v13: bool) -> Option<f64> {
    if b == 0.0 && (v13 || !a.is_nan()) {
        return None;
    }
    let r = a / b;
    let overflow = r.is_infinite() && !a.is_infinite();
    let underflow = r == 0.0 && a != 0.0 && (v13 || !b.is_infinite());
    (!overflow && !underflow).then_some(r)
}

/// Whether `line_in` constructs a line through two distinct points without
/// raising: `point_sl`'s slope, then `line_construct`'s intercept — as v14
/// on compute them, or as v13 does where `v13`, whose vertical slope is
/// `DBL_MAX` rather than an infinity and which has no horizontal case.
fn line_constructs(p: Point, q: Point, v13: bool) -> bool {
    let vertical = if v13 { f64::MAX } else { f64::INFINITY };
    let slope = if fp_eq(p.x, q.x) {
        Some(vertical)
    } else if fp_eq(p.y, q.y) {
        Some(0.0)
    } else {
        float8_mi(p.y, q.y).zip(float8_mi(p.x, q.x)).and_then(|(dy, dx)| float8_div(dy, dx, v13))
    };
    let Some(slope) = slope else { return false };
    if (v13 && slope == f64::MAX) || (!v13 && (slope.is_infinite() || slope == 0.0)) {
        return true;
    }
    float8_mul(slope, p.x).and_then(|mx| float8_mi(p.y, mx)).is_some()
}

/// Whether `point_in` reads `text`: `x,y`, optionally in parentheses (I74).
// pg-refuses: I74 — every refusal here is `point_in`'s.
pub(crate) fn point_in(text: &str) -> bool {
    let b = text.as_bytes();
    pair_decode(b, 0).is_some_and(|(_, end)| end == b.len())
}

/// Whether `lseg_in` reads `text`: two points, in `[…]`, `(…)` or bare
/// (I74).
// pg-refuses: I74 — every refusal here is `lseg_in`'s.
pub(crate) fn lseg_in(text: &str) -> bool {
    let b = text.as_bytes();
    path_decode(b, 0, true, 2).is_some_and(|(_, end)| end == b.len())
}

/// Whether `box_in` reads `text`: two points, in `(…)` or bare, never in
/// `[…]` (I74).
// pg-refuses: I74 — every refusal here is `box_in`'s.
pub(crate) fn box_in(text: &str) -> bool {
    let b = text.as_bytes();
    path_decode(b, 0, false, 2).is_some_and(|(_, end)| end == b.len())
}

/// Whether `path_in` reads `text`: as many points as its commas say, open in
/// `[…]` or closed in `(…)`, one more `(…)` allowed around them (I74).
// pg-refuses: I74 — every refusal here is `path_in`'s.
pub(crate) fn path_in(text: &str) -> bool {
    let b = text.as_bytes();
    let Some(npts) = pair_count(b) else { return false };
    let mut i = skip_c_spaces(b, 0);
    // A single leading `(`, the last in the text, is the path's own.
    let wrapped = c_at(b, i) == b'(' && b[i..].iter().rposition(|&c| c == b'(') == Some(0);
    i += usize::from(wrapped);
    if !fits_allocation(PATH_HEADER, npts) {
        return false;
    }
    let Some((_, mut i)) = path_decode(b, i, true, npts) else { return false };
    if wrapped {
        if c_at(b, i) != b')' {
            return false;
        }
        i = skip_c_spaces(b, i + 1);
    }
    i == b.len()
}

/// Whether `poly_in` reads `text`: as many points as its commas say, in
/// `(…)` or bare, never in `[…]` (I74).
// pg-refuses: I74 — every refusal here is `poly_in`'s.
pub(crate) fn poly_in(text: &str) -> bool {
    let b = text.as_bytes();
    let Some(npts) = pair_count(b) else { return false };
    fits_allocation(POLYGON_HEADER, npts)
        && path_decode(b, 0, false, npts).is_some_and(|(_, end)| end == b.len())
}

/// Whether `circle_in` reads `text`: a center and a radius neither negative
/// nor read past its range, in `<…>`, `(…)` or bare (I74).
// pg-refuses: I74 — every refusal here is `circle_in`'s.
pub(crate) fn circle_in(text: &str) -> bool {
    let b = text.as_bytes();
    let mut i = skip_c_spaces(b, 0);
    let mut depth = 0;
    if c_at(b, i) == b'<' {
        depth = 1;
        i += 1;
    } else if c_at(b, i) == b'(' {
        let inner = skip_c_spaces(b, i + 1);
        if c_at(b, inner) == b'(' {
            depth = 1;
            i = inner;
        }
    }
    let Some((_, i)) = pair_decode(b, i) else { return false };
    let i = i + usize::from(c_at(b, i) == b',');
    let Some((radius, mut i)) = single_decode(b, i) else { return false };
    // A NaN radius is read.
    if radius < 0.0 {
        return false;
    }
    while depth > 0 {
        match c_at(b, i) {
            b')' => {}
            b'>' if depth == 1 => {}
            _ => return false,
        }
        depth -= 1;
        i = skip_c_spaces(b, i + 1);
    }
    i == b.len()
}

/// Whether `line_in` reads `text`: `{A,B,C}` with `A` and `B` not both zero,
/// or two distinct points, as `lseg_in` reads them, whose line it can
/// construct. **Refused only where every supported major refuses it**
/// (I74): v13 builds the line from two points by arithmetic v14 changed, and
/// neither refuses a subset of what the other does.
// pg-refuses: I74 — every refusal here is `line_in`'s.
pub(crate) fn line_in(text: &str) -> bool {
    let b = text.as_bytes();
    let i = skip_c_spaces(b, 0);
    if c_at(b, i) != b'{' {
        let Some(([p, q], end)) = path_decode(b, i, true, 2) else { return false };
        return end == b.len()
            && !point_eq_point(p, q)
            && (line_constructs(p, q, false) || line_constructs(p, q, true));
    }
    let mut i = i + 1;
    let mut coefficients = [0.0; 3];
    for (k, coefficient) in coefficients.iter_mut().enumerate() {
        let Some((value, next)) = single_decode(b, i) else { return false };
        if c_at(b, next) != if k < 2 { b',' } else { b'}' } {
            return false;
        }
        *coefficient = value;
        i = next + 1;
    }
    skip_c_spaces(b, i) == b.len() && !(fp_zero(coefficients[0]) && fp_zero(coefficients[1]))
}

#[cfg(test)]
mod tests;

/// The straightforward `format!`/`parse` spellings of the four scalar
/// decoders, the two hex renderers and the three date/time renderers, kept as
/// the oracle the allocation-free forms above are checked against. Those forms
/// are asked to be exactly this but where a test names the difference, so the
/// corpora below are generated rather
/// than listed: a disagreement on an input nobody thought to write down is a
/// test failure and not a report from the field.
#[cfg(test)]
mod prior_shape;

#[cfg(test)]
mod differential;
