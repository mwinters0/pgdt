//! Per-type field decode and render-back (`docs/design/decisions.md`,
//! "D44").
//!
//! Pure, synchronous, no I/O (`docs/design/decisions.md`, "D74"). A decode
//! function takes an already-COPY-unescaped `&str` field (what
//! `crate::copy::decode_field` returns) and returns a plain Rust value; it
//! never takes an Arrow builder — building the array is `batch.rs`'s job
//! (L3), which is also where these are wired to their [`arrow::datatypes::DataType`].
//!
//! Every `render_*` function is the exact inverse of its `decode_*`
//! counterpart and produces the same *decoded* text `crate::copy::decode_field`
//! would have returned for a correctly-formed value — never the raw
//! COPY-escaped bytes on disk. Converting between the two is
//! `crate::copy::encode_field`'s job (`docs/design/decisions.md`, "D68").
//! `tests/decode.rs`'s round-trip test compares against `SchemaMode::Strings`,
//! which is also decoded text; the on-disk-byte leg is covered separately in
//! `tests/scan.rs`.

/// `t`/`f`, COPY TEXT's boolean spelling.
pub fn decode_bool(s: &str) -> Option<bool> {
    match s {
        "t" => Some(true),
        "f" => Some(false),
        _ => None,
    }
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

/// `real`'s text. A finite spelling reads as a finite value: one rounding past
/// the type's largest finite one, as `float4out` can at `extra_float_digits`
/// of `-2` or less, is that largest value of its sign (I57).
pub fn decode_f32(s: &str) -> Option<f32> {
    match s {
        "NaN" => Some(f32::NAN),
        "Infinity" => Some(f32::INFINITY),
        "-Infinity" => Some(f32::NEG_INFINITY),
        _ => s
            .parse::<f32>()
            .ok()
            .map(|v| finite_spelling(s, v, f32::is_infinite, f32::MAX.copysign(v))),
    }
}

/// `double precision`'s text, read as [`decode_f32`] reads `real`'s: a finite
/// spelling past `DBL_MAX` — `float8out`'s `1.79769313486232e+308` under
/// `--extra-float-digits=0` — is `±DBL_MAX` (I57).
pub fn decode_f64(s: &str) -> Option<f64> {
    match s {
        "NaN" => Some(f64::NAN),
        "Infinity" => Some(f64::INFINITY),
        "-Infinity" => Some(f64::NEG_INFINITY),
        _ => s
            .parse::<f64>()
            .ok()
            .map(|v| finite_spelling(s, v, f64::is_infinite, f64::MAX.copysign(v))),
    }
}

/// `parsed` unless the parse overflowed a spelling written in digits, which
/// `*_out` writes only for a finite value (I57): then `largest`, the finite
/// value of `parsed`'s sign nearest the text. A spelling ending in a letter
/// keeps the parse's reading: it is one of the infinity's spellings, which the
/// parse accepts more of than `*_out` writes.
// deficiency: KD75 — a field spelled past the largest finite value is read
// as it, where `float8in` refuses the spelling and a restore fails the table's
// `COPY` (I57). The ceiling binds a field (`roadmap.md`, "A literal is
// guaranteed in `*_out`'s form and never read past `*_in`'s"), so it is to fail
// the parse reading it, aborting it — not an unrepresentable value, that
// category being our front end's limit (D96) — a known-failure row first.
#[inline]
fn finite_spelling<F: Copy>(s: &str, parsed: F, is_infinite: fn(F) -> bool, largest: F) -> F {
    if is_infinite(parsed) && s.as_bytes().last().is_some_and(u8::is_ascii_digit) {
        largest
    } else {
        parsed
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
/// `date`, a `time` or a `timestamp` is written from is two digits wide, so a
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
/// slice like every other piece and the whole function stays free of
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
/// - **Every piece is a slice of a `&'static str`**, so nothing here converts
///   bytes to text and no UTF-8 validation is paid per field.
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
pub(crate) fn push_integer(out: &mut String, value: i64) {
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
/// refused), so a part carrying one is refused here rather than read as a
/// signed number — a shortfall where the server reads it
/// (`docs/design/decisions.md`, "D55"). Past `i64` is refused as `strtoint`'s
/// `ERANGE` is.
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
/// `None` for a day the calendar does not hold, as `ValidateDate` refuses it.
///
/// The year is three digits or more, as `pg_dump` writes four: a shorter one
/// is read by `DateOrder` (`20-01-01` is a month 20 under `MDY`, the year 2020
/// under `YMD`), so it is refused as a shortfall, as is a month of three
/// digits, which the server reads as a day of the year (D55).
fn civil_days(s: &str, bc: bool) -> Option<i64> {
    let (year, rest) = s.split_once('-')?;
    let (month, day) = rest.split_once('-')?;
    if year.len() < 3 || month.len() > 2 {
        return None;
    }
    let (y, m, d) = (unsigned_part(year)?, unsigned_part(month)?, unsigned_part(day)?);
    // pg-refuses: I61 — no year zero either side of the era, a month past
    // twelve, a day past its month's, a year past `IS_VALID_JULIAN`'s.
    if y == 0 || y > JULIAN_MAX_YEAR || !(1..=12).contains(&m) {
        return None;
    }
    let y = astronomical_year(y, bc);
    if !(1..=days_in_month(y, m)).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m as u32, d as u32))
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
    if s == "infinity" || s == "-infinity" {
        return None;
    }
    let (rest, bc) = split_era(s);
    let days = civil_days(rest, bc)?;
    // pg-refuses: I61 — `IS_VALID_DATE`'s range.
    if !(DATE_MIN_DAYS..DATE_END_DAYS).contains(&days) {
        return None;
    }
    i32::try_from(days).ok()
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
/// same pair added onto a day count. `crate::predicate` reads it too, for the
/// time half of a `time with time zone` comparison — that type has no Arrow
/// mapping of its own, so its *ordering* is the only path that decodes it.
///
/// `None` past `time_overflows`' bounds, which `time`, `timetz` and both
/// timestamps share: `24:00:00` is the last time of day, and `23:59:60` and
/// `24:00:00` are both read, a timestamp's carrying into the next day.
pub(crate) fn parse_time_of_day(s: &str) -> Option<(i64, i64)> {
    let (hms, frac) = s.split_once('.').unwrap_or((s, ""));
    let mut parts = hms.splitn(3, ':');
    let h = unsigned_part(parts.next()?)?;
    let mi = unsigned_part(parts.next()?)?;
    let se = unsigned_part(parts.next()?)?;
    if parts.next().is_some() || frac.len() > 6 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // pg-refuses: I61 — `time_overflows`, part by part.
    if h > 24 || mi > 59 || se > 60 {
        return None;
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
    (seconds * 1_000_000 + micros <= DAY_MICROS).then_some((seconds, micros))
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
/// (`crate::predicate`), where [`decode_time64_micros`] is what Arrow holds.
pub(crate) fn time_of_day_micros(s: &str) -> Option<i64> {
    let (seconds, micros) = parse_time_of_day(s)?;
    Some(seconds * 1_000_000 + micros)
}

/// `None` for anything unparseable, and for `24:00:00`: a real, valid
/// boundary value (PostgreSQL's inclusive upper bound for `time`) that
/// Arrow's `Time64`, holding `[0, 86400 s)`, cannot, so it is refused as
/// every other unrepresentable value is (the anchor on [`decode_date32`]).
pub fn decode_time64_micros(s: &str) -> Option<i64> {
    time_of_day_micros(s).filter(|&micros| micros < DAY_MICROS)
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
/// `crate::predicate` splits a `time with time zone` with this. What it does
/// with the two halves is not what a timestamp does: the offset is kept, not
/// discarded, because two `timetz` values are equal only when the zone
/// matches as well as the instant.
///
/// `None` past `DecodeTimezone`'s `±15:59:59`, part by part. The run-together
/// `+0530` the server reads as `+05:30` is refused as an hour past fifteen, a
/// shortfall (`docs/design/decisions.md`, "D55").
pub(crate) fn extract_offset(s: &str) -> Option<(&str, i64)> {
    let idx = s.find(['+', '-'])?;
    let (time_only, off) = s.split_at(idx);
    let (sign, rest) = off.split_at(1);
    let sign_mult: i64 = if sign == "-" { -1 } else { 1 };
    let mut parts = rest.splitn(3, ':');
    let hh = unsigned_part(parts.next()?)?;
    let mm = parts.next().map(unsigned_part).unwrap_or(Some(0))?;
    let ss = parts.next().map(unsigned_part).unwrap_or(Some(0))?;
    // pg-refuses: I61 — `MAX_TZDISP_HOUR`, and a minute or second past 59.
    if hh > 15 || mm > 59 || ss > 59 {
        return None;
    }
    Some((time_only, sign_mult * (hh * 3600 + mm * 60 + ss)))
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
    i64::try_from(timestamp_micros_wide(s, with_tz)?).ok()
}

/// [`decode_timestamp_micros`] before it narrows to `i64`: microseconds from
/// the 1970 UTC epoch for any finite value the grammar reads and PostgreSQL's
/// range holds, so a value past what `Timestamp(Microsecond)` holds is told
/// from one that does not parse (`crate::unrepresentable`).
pub(crate) fn timestamp_micros_wide(s: &str, with_tz: bool) -> Option<i128> {
    if s == "infinity" || s == "-infinity" {
        return None;
    }
    let (rest, bc) = split_era(s);
    let (date_part, time_part) = rest.split_once(' ')?;
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
    (i128::from(MIN_TIMESTAMP) <= postgres && postgres < i128::from(END_TIMESTAMP)).then_some(utc)
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
/// included. What its comparison orders (`crate::predicate`), where
/// [`decode_timestamp_micros`] is what Arrow holds; `None` as that is for the
/// infinities and for text that is not a timestamp.
pub(crate) fn timestamp_postgres_micros(s: &str, with_tz: bool) -> Option<i64> {
    i64::try_from(timestamp_micros_wide(s, with_tz)? - POSTGRES_EPOCH_UNIX_MICROS).ok()
}

/// [`render_timestamp_micros`] of a [`timestamp_postgres_micros`] value, so a
/// timestamp past what `i64` counts from 1970 is written as the dump writes it.
pub(crate) fn render_timestamp_postgres_micros(v: i64, with_tz: bool) -> String {
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
fn interval_time_micros(text: &str) -> Option<i64> {
    let (negative, rest) = match text.strip_prefix(['+', '-']) {
        Some(rest) => (text.starts_with('-'), rest),
        None => (false, text),
    };
    let (hms, frac) = rest.split_once('.').unwrap_or((rest, ""));
    let mut parts = hms.split(':');
    let mut field = |max_len: Option<usize>| -> Option<i128> {
        let digits = parts.next()?;
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if max_len.is_some_and(|n| digits.len() != n) {
            return None;
        }
        digits.parse::<i128>().ok()
    };
    let hours = field(None)?;
    let minutes = field(Some(2))?;
    let seconds = field(Some(2))?;
    if parts.next().is_some() {
        return None;
    }
    // pg-refuses: I62 — `DecodeTimeCommon`'s minute past 59 and second past 60.
    if minutes > 59 || seconds > 60 {
        return None;
    }
    if frac.is_empty() && text.contains('.') {
        return None;
    }
    if frac.len() > 6 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let micros: i128 = if frac.is_empty() {
        0
    } else {
        let mut padded = frac.to_string();
        padded.push_str(&"0".repeat(6 - padded.len()));
        padded.parse().ok()?
    };
    let total = hours
        .checked_mul(3600)?
        .checked_add(minutes * 60 + seconds)?
        .checked_mul(1_000_000)?
        .checked_add(micros)?;
    // pg-refuses: I62 — the `Interval` struct's `int64` microseconds, either
    // sign: the text carries the magnitude, so `i64::MIN` is no value of it.
    let total = i64::try_from(total).ok()?;
    Some(if negative { -total } else { total })
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
/// `interval_out` never writes, and are refused. `infinity`/`-infinity`
/// (v17's, I34) are not in the grammar either, so they fail here and each
/// consumer says what it does about them.
pub(crate) fn interval_parts(text: &str) -> Option<(i32, i32, i64)> {
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
                    return None;
                }
                time = interval_time_micros(tokens[at])?;
                at += 1;
                break;
            }
        };
        // pg-refuses: I62 — a unit given twice, and a count past `int32`.
        if counts[unit].is_some() {
            return None;
        }
        counts[unit] = Some(i32::try_from(interval_count(tokens[at])?).ok()?);
        at += 2;
    }
    if at != tokens.len() {
        return None;
    }
    let [years, months, days] = counts.map(|count| i64::from(count.unwrap_or(0)));
    // pg-refuses: I62 — `itmin2interval`'s month total past `int32`.
    let months = i32::try_from(years * 12 + months).ok()?;
    Some((months, i32::try_from(days).ok()?, time))
}

/// `interval`, as Arrow's `Interval(MonthDayNano)` holds it: months, days and
/// a **nanosecond** time part, which are exactly PostgreSQL's own three
/// independent fields. `None` for three classes of value, each a decode
/// failure by construction the way [`decode_date32`]'s infinities are:
///
/// - `infinity` and `-infinity` (v17's, I34) — every field of PostgreSQL's
///   struct is extremal and Arrow has no encoding for the value at all;
/// - a time part past `2562047:47:16.854775807` — PostgreSQL's field is
///   `int64` *microseconds* against Arrow's `int64` nanoseconds, a
///   thousandth of the range, and nothing normalizes hours into days (I40);
/// - a value `interval_in` refuses, which [`interval_parts`] refuses (I62).
pub fn decode_interval(s: &str) -> Option<(i32, i32, i64)> {
    let (months, days, micros) = interval_parts(s)?;
    Some((months, days, micros.checked_mul(1_000)?))
}

/// `EncodeInterval` under `INTSTYLE_POSTGRES`, the exact inverse of
/// [`decode_interval`] (I40). Three rules carry the whole form, and none of
/// them is `format_hms_frac`'s: a part is suppressed when its value is zero,
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
/// [`crate::copy::hex_val`]'s three ranges: a pair becomes two loads, a
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

/// Canonical `8-4-4-4-12` hex form, case-insensitive on input (PostgreSQL
/// always dumps lowercase, but nothing forces that on a hand-edited fixture).
///
/// Hyphens are dropped wherever they fall and exactly 32 hex digits must
/// remain. The filter runs over bytes rather than chars: `-` is ASCII, so
/// the two leave the same sequence, and a non-ASCII byte is not a hex digit
/// either way.
///
/// Deficiency register: `deficiency: KD80` — `uuid_in` takes a hyphen only
/// after a group of four digits, so a literal with one leading, trailing,
/// doubled or mid-group reads here where the server refuses it.
pub fn decode_uuid(s: &str) -> Option<[u8; 16]> {
    let mut nibbles = s.bytes().filter(|b| *b != b'-');
    let mut bytes = [0u8; 16];
    let mut bad = 0u8;
    for byte in &mut bytes {
        let hi = HEX_NIBBLE[nibbles.next()? as usize];
        let lo = HEX_NIBBLE[nibbles.next()? as usize];
        bad |= hi | lo;
        *byte = hi << 4 | lo;
    }
    if bad & 0xF0 != 0 || nibbles.next().is_some() {
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

/// Parse a `numeric` field's text into an unscaled integer digit string at
/// `scale` decimal places — the form [`i128::from_str`]/[`i256::from_str`]
/// accept directly, so `batch.rs` only needs to pick the right width, never
/// do arithmetic of its own. `None` for `NaN`: PostgreSQL's numeric `NaN`
/// bypasses the column's own precision/scale check and is reachable through
/// *any* numeric column (confirmed: `fixtures/*/types/*.sql`,
/// `public.t_numeric.v_small numeric(10,2)`) — `Decimal128`/`Decimal256` have
/// no representation for it, so this is the decode-failure path by
/// construction, same as date/timestamp infinities.
///
/// A typed `numeric(p,s)` column always renders with exactly `s` fractional
/// digits (PostgreSQL applies the typmod before output), so the "shift
/// digits toward `scale`" step below is exact rather than a rounding
/// approximation — including PG15+'s negative-scale numerics, which print
/// with no fractional digits at all and where this divides out the implied
/// trailing zeros instead of appending them. The one value written with fewer
/// digits than a negative scale divides out is zero, which `numeric_out`
/// prints as `0` at any scale (I51), so an all-zero digit string answers `0`
/// there.
///
/// Deficiency register: `deficiency: KD81` — a field is not put through
/// `apply_typmod`, which rounds it to the scale and then refuses one past the
/// precision (I51): `1.005` in a `numeric(10,2)`, which the server stores as
/// `1.01`, is refused here and read unrounded by `NumericKey` on a text-held
/// `p > 76` column, and `123456789012` is read where the server refuses it.
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

#[cfg(test)]
mod tests {
    use arrow::datatypes::i256;

    use super::*;

    #[test]
    fn bool_round_trips() {
        assert_eq!(decode_bool("t"), Some(true));
        assert_eq!(decode_bool("f"), Some(false));
        assert_eq!(decode_bool("x"), None);
        assert_eq!(render_bool(true), "t");
        assert_eq!(render_bool(false), "f");
    }

    #[test]
    fn float_special_values_round_trip() {
        for (text, v) in
            [("NaN", f64::NAN), ("Infinity", f64::INFINITY), ("-Infinity", f64::NEG_INFINITY)]
        {
            let decoded = decode_f64(text).unwrap();
            assert_eq!(decoded.is_nan(), v.is_nan());
            if !v.is_nan() {
                assert_eq!(decoded, v);
            }
            assert_eq!(render_f64(decoded), text);
        }
        assert_eq!(render_f64(decode_f64("-0").unwrap()), "-0");
    }

    /// `float8out` and `float4out` at a precision short enough to round the
    /// largest finite value past it (I57): every `--extra-float-digits` that
    /// does, down to `-15`'s one digit, reads as that value, and a spelling
    /// nearer zero than the rounding keeps its own reading.
    #[test]
    fn a_finite_spelling_past_the_largest_finite_value_reads_as_it() {
        for text in ["1.79769313486232e+308", "1.8e+308", "2e+308", "1e+309"] {
            assert_eq!(decode_f64(text), Some(f64::MAX), "{text}");
            assert_eq!(decode_f64(&format!("-{text}")), Some(-f64::MAX), "-{text}");
        }
        for text in ["3.403e+38", "3.4e+39", "4e+38"] {
            assert_eq!(decode_f32(text), Some(f32::MAX), "{text}");
            assert_eq!(decode_f32(&format!("-{text}")), Some(-f32::MAX), "-{text}");
        }
        assert_eq!(decode_f64("1.7976931348623157e+308"), Some(f64::MAX));
        assert_eq!(decode_f32("3.40282e+38"), Some(3.40282e38));
        assert_eq!(decode_f64("Infinity"), Some(f64::INFINITY));
        assert_eq!(decode_f64("-Infinity"), Some(f64::NEG_INFINITY));
    }

    /// Ground truth from a live server under `extra_float_digits = 3` —
    /// exercises the fixed/scientific switch at both ends and at both
    /// `FLT_DIG`/`DBL_DIG` thresholds, which a fixture round-trip alone
    /// can't be relied on to hit.
    #[test]
    fn float_formatting_matches_postgresql_exactly() {
        let cases: &[(&str, f64)] = &[
            ("1e+100", 1e100),
            ("1e+20", 1e20),
            ("1e+21", 1e21),
            ("1.2345678901234568e+20", 1.2345678901234568e20),
            ("0.00015", 1.5e-4),
            ("1.5e-05", 1.5e-5),
            ("0.0001", 0.0001),
            ("-1e+100", -1e100),
            ("1e-300", 1e-300),
            ("0", 0.0),
            ("1.234567890123456e+15", 1234567890123456.0),
            ("100000000000000", 1e14),
            ("1e+15", 1e15),
            ("123456789012345", 123456789012345.0),
        ];
        for (text, v) in cases {
            assert_eq!(render_f64(*v), *text, "value {v}");
            assert_eq!(decode_f64(text), Some(*v), "text {text}");
        }
        let f32_cases: &[(&str, f32)] = &[
            ("1e+06", 1e6),
            ("100000", 1e5),
            ("1.2345678e+07", 1.234_567_8e7),
            ("1.1754944e-38", 1.175_494_4e-38),
        ];
        for (text, v) in f32_cases {
            assert_eq!(render_f32(*v), *text, "value {v}");
            assert_eq!(decode_f32(text), Some(*v), "text {text}");
        }
    }

    #[test]
    fn date_boundary_values() {
        for text in ["0001-01-01", "9999-12-31", "0044-01-01 BC", "10000-01-01"] {
            let days = decode_date32(text).expect(text);
            assert_eq!(render_date32(days), text, "{text}");
        }
        assert_eq!(decode_date32("infinity"), None);
        assert_eq!(decode_date32("-infinity"), None);
    }

    #[test]
    fn epoch_and_negative_days() {
        assert_eq!(decode_date32("1970-01-01"), Some(0));
        assert_eq!(decode_date32("1969-12-31"), Some(-1));
        assert_eq!(render_date32(0), "1970-01-01");
    }

    /// The range's two ends are `DATETIME_MIN_JULIAN` and `DATE_END_JULIAN`
    /// as days from 1970, which `timestamp.h` states as Julian days (I61).
    #[test]
    fn the_date_range_constants_are_julian_days_from_1970() {
        let julian_1970 = 2_440_588;
        assert_eq!(days_from_civil(-4713, 11, 24), DATE_MIN_DAYS);
        assert_eq!(DATE_MIN_DAYS + julian_1970, 0);
        assert_eq!(days_from_civil(JULIAN_MAX_YEAR, 1, 1), DATE_END_DAYS);
        assert_eq!(DATE_END_DAYS + julian_1970, 2_147_483_494);
        let postgres_epoch_days = 10_957;
        assert_eq!(days_from_civil(2000, 1, 1), postgres_epoch_days);
        let day = 86_400_000_000;
        assert_eq!(i128::from(MIN_TIMESTAMP), (DATE_MIN_DAYS - postgres_epoch_days) as i128 * day);
        let end_days = days_from_civil(294_277, 1, 1) - postgres_epoch_days;
        assert_eq!(i128::from(END_TIMESTAMP), i128::from(end_days) * day);
    }

    /// **A date is bounded part by part as `ValidateDate` bounds it, and to
    /// `IS_VALID_DATE`'s range** (I61). Each refusal was cast on PostgreSQL
    /// 16; the shortfalls beside them are spellings the server reads by
    /// `DateOrder` or as a day of the year, which this build does not (D55).
    #[test]
    fn a_date_the_calendar_does_not_hold_is_refused() {
        for text in [
            "2020-02-30",
            "2019-02-29",
            "2020-13-01",
            "2020-00-01",
            "2020-01-00",
            "2020-01-32",
            "2020-04-31",
            "0000-01-01",
            "0000-01-01 BC",
            "+2020-01-01",
            "2020-+1-01",
            "2020-01-+1",
            "4714-11-23 BC",
            "5874898-01-01",
            "99999999999999999999-01-01",
        ] {
            assert_eq!(decode_date32(text), None, "{text}");
        }
        // Shortfalls: the server reads each.
        for text in ["20-01-01", "1-01-01", "2020-001-05"] {
            assert_eq!(decode_date32(text), None, "{text}");
        }
        for (text, read_as) in [
            ("2020-02-29", "2020-02-29"),
            ("0001-02-29 BC", "0001-02-29 BC"),
            ("020-01-01", "0020-01-01"),
            ("2020-1-5", "2020-01-05"),
            ("2020-01-001", "2020-01-01"),
            ("4714-11-24 BC", "4714-11-24 BC"),
            ("5874897-12-31", "5874897-12-31"),
        ] {
            let days = decode_date32(text).expect(text);
            assert_eq!(render_date32(days), read_as, "{text}");
        }
    }

    /// **A time of day is bounded as `time_overflows` bounds it**, and a
    /// part carrying a sign is refused rather than read as a signed number:
    /// the server reads `12:-5:00` as 12:00 at zone `-5`, where the prior
    /// reading made it 11:55 (I61).
    #[test]
    fn a_time_past_time_overflows_is_refused() {
        for text in [
            "12:60:00",
            "12:05:61",
            "25:00:00",
            "24:00:00.000001",
            "24:00:01",
            "24:01:00",
            "23:59:60.5",
            "12:-5:00",
            "12:+5:00",
            "12:05:+5",
            "+12:00:00",
            "-1:00:00",
        ] {
            assert_eq!(time_of_day_micros(text), None, "{text}");
        }
        for (text, micros) in [
            ("12:59:60", 46_800_000_000),
            ("23:59:60", DAY_MICROS),
            ("24:00:00", DAY_MICROS),
            ("012:05:00", 43_500_000_000),
        ] {
            assert_eq!(time_of_day_micros(text), Some(micros), "{text}");
        }
    }

    /// **A numeric zone is bounded as `DecodeTimezone` bounds it**,
    /// `±15:59:59`, part by part (I61).
    #[test]
    fn an_offset_past_fifteen_hours_is_refused() {
        for text in [
            "12:00:00+16",
            "12:00:00-16",
            "12:00:00+05:60",
            "12:00:00+05:30:60",
            "12:00:00+0530",
            "12:00:00+-5",
            "12:00:00+05:+3",
        ] {
            assert_eq!(extract_offset(text), None, "{text}");
        }
        for (text, seconds) in [
            ("12:00:00+15:59:59", 57_599),
            ("12:00:00-15:59:59", -57_599),
            ("12:00:00-00:44:30", -2_670),
            ("12:00:00+05", 18_000),
        ] {
            assert_eq!(extract_offset(text), Some(("12:00:00", seconds)), "{text}");
        }
    }

    /// **A timestamp is bounded to `IS_VALID_TIMESTAMP`'s range by its
    /// instant**, after its date and time are bounded as a `date`'s and a
    /// `time`'s are, so an offset carries a local time across either end
    /// (I61). Each was cast on PostgreSQL 16.
    #[test]
    fn a_timestamp_past_postgresqls_range_is_refused() {
        for (text, with_tz) in [
            ("2020-02-30 00:00:00", false),
            ("2020-01-01 12:60:00", false),
            ("2020-01-01 24:00:01", false),
            ("2020-01-01 12:-5:00", false),
            ("0000-01-01 00:00:00", false),
            ("4714-11-23 23:59:59.999999 BC", false),
            ("294277-01-01 00:00:00", false),
            ("2020-01-01 00:00:00+16", true),
            ("2020-01-01 12:-5:00", true),
            ("4714-11-24 00:00:00+01 BC", true),
            ("294276-12-31 23:00:00-02", true),
        ] {
            assert_eq!(timestamp_micros_wide(text, with_tz), None, "{text}");
        }
        let at = |text: &str, with_tz: bool| timestamp_postgres_micros(text, with_tz).expect(text);
        assert_eq!(at("4714-11-24 00:00:00 BC", false), MIN_TIMESTAMP);
        assert_eq!(at("294276-12-31 23:59:59.999999", false), END_TIMESTAMP - 1);
        assert_eq!(at("4714-11-23 23:00:00-02 BC", true), MIN_TIMESTAMP + 3_600_000_000);
        assert_eq!(at("294276-12-31 23:30:00+01", true), END_TIMESTAMP - 5_400_000_000);
        assert_eq!(at("2020-01-01 24:00:00", false), at("2020-01-02 00:00:00", false));
        assert_eq!(at("2020-01-01 23:59:60", false), at("2020-01-02 00:00:00", false));
    }

    /// `interval_out` under `IntervalStyle = postgres` (I40), round-tripped
    /// through the triple. Every string here is a real server's answer (four
    /// of them `fixtures/*/types/default.sql`'s `t_interval`), because the
    /// three rules that decide the form are all sign-conditional and none of
    /// them is reachable from the fixture's own values: a unit takes an `s`
    /// whenever the count is not exactly `1` — so `-1 mons` — and a part
    /// that is positive and follows a negative one carries a `+`, the time
    /// tail included.
    #[test]
    fn interval_round_trips_every_shape_of_encode_interval() {
        let cases: &[(&str, (i32, i32, i64))] = &[
            ("1 year 2 mons 3 days 04:05:06", (14, 3, 14_706_000_000_000)),
            ("-1 days", (0, -1, 0)),
            ("00:00:00", (0, 0, 0)),
            ("01:30:00", (0, 0, 5_400_000_000_000)),
            ("1 mon", (1, 0, 0)),
            ("30 days", (0, 30, 0)),
            ("720:00:00", (0, 0, 2_592_000_000_000_000)),
            ("100 years", (1200, 0, 0)),
            ("-11 mons", (-11, 0, 0)),
            ("11 mons", (11, 0, 0)),
            ("-1 years -2 mons", (-14, 0, 0)),
            ("2 mons", (2, 0, 0)),
            ("00:00:00.123", (0, 0, 123_000_000)),
            ("-00:00:00.000001", (0, 0, -1_000)),
            // The `+` on a positive part that follows a negative one, at each
            // of the three places it can appear.
            ("-1 days +01:00:00", (0, -1, 3_600_000_000_000)),
            ("-1 mons +1 day", (-1, 1, 0)),
            ("-1 mons +00:00:01", (-1, 0, 1_000_000_000)),
            // A negative time tail after a positive day, which takes `-`
            // rather than the day's sign.
            ("1 day -00:00:00.5", (0, 1, -500_000_000)),
            ("-1 mons -1 days -01:02:03.456789", (-1, -1, -3_723_456_789_000)),
        ];
        for (text, parts) in cases {
            assert_eq!(decode_interval(text), Some(*parts), "decode {text}");
            let (months, days, nanos) = *parts;
            assert_eq!(
                render_interval(months, days, nanos).as_deref(),
                Some(*text),
                "render {text}"
            );
        }
    }

    /// The three classes with no `Interval(MonthDayNano)` encoding, each a
    /// decode failure rather than a wrong value.
    #[test]
    fn interval_refuses_what_month_day_nano_cannot_hold() {
        // v17's infinities (I34): not in `EncodeInterval`'s grammar at all.
        assert_eq!(decode_interval("infinity"), None);
        assert_eq!(decode_interval("-infinity"), None);
        // The ceiling is Arrow's nanoseconds, a thousandth of PostgreSQL's
        // microsecond range (I40) — one microsecond either side of it.
        assert_eq!(
            decode_interval("2562047:47:16.854775"),
            Some((0, 0, 9_223_372_036_854_775_000))
        );
        assert_eq!(decode_interval("2562047:47:16.854776"), None);
        assert_eq!(decode_interval("-2562047:47:16.854776"), None);
        // Nothing normalizes hours into days, so an ordinary unnormalized
        // value reaches it: `interval '100000000 hours'` is written
        // `100000000:00:00` and is forty times over.
        assert_eq!(decode_interval("100000000:00:00"), None);
        // A month or day count past `i32`, which no `Interval` struct holds.
        assert_eq!(decode_interval("2147483648 days"), None);
        assert_eq!(decode_interval("2147483648 mons"), None);
        assert_eq!(decode_interval("2147483647 days"), Some((0, 2_147_483_647, 0)));
    }

    /// **What `interval_in` refuses is refused, and its edges read** (I62):
    /// a minute past 59 or a second past 60, a count or month total past
    /// `int32`, a time past `int64` microseconds, and a unit given twice.
    /// Each was cast on PostgreSQL 16; `00:90:00` once read as `01:30:00`.
    #[test]
    fn an_interval_interval_in_refuses_is_refused() {
        for text in [
            "00:90:00",
            "00:59:61",
            "-00:60:00",
            "2147483648 days",
            "-2147483649 days",
            "2147483648 mons",
            "178956970 years 8 mons",
            "-178956970 years -9 mons",
            "2562047788:00:54.775808",
            "-2562047788:00:54.775808",
            "1 day 1 day",
            "1 mon 1 mons",
            "1 year 1 mon 1 day 1 year",
        ] {
            assert_eq!(interval_parts(text), None, "{text}");
        }
        let micros_max = i64::MAX;
        for (text, parts) in [
            ("00:59:60", (0, 0, 3_600_000_000)),
            ("-00:59:60", (0, 0, -3_600_000_000)),
            ("2147483647 days", (0, i32::MAX, 0)),
            ("-2147483648 days", (0, i32::MIN, 0)),
            ("178956970 years 7 mons", (i32::MAX, 0, 0)),
            ("-178956970 years -8 mons", (i32::MIN, 0, 0)),
            ("178956971 years -12 mons", (2_147_483_640, 0, 0)),
            ("2562047788:00:54.775807", (0, 0, micros_max)),
            ("-2562047788:00:54.775807", (0, 0, -micros_max)),
            ("1 day 1 year", (12, 1, 0)),
        ] {
            assert_eq!(interval_parts(text), Some(parts), "{text}");
        }
    }

    /// The literal grammar is `interval_out`'s and no wider — the same
    /// refusal the ordering path makes, reached through the decoder that
    /// shares its walk.
    #[test]
    fn interval_refuses_spellings_interval_out_never_writes() {
        for text in ["1 month", "1.5 hours", "P1Y2M", "1 hour", "04:-5:06", "1 year 2"] {
            assert_eq!(decode_interval(text), None, "{text}");
        }
    }

    /// The one value `render_interval` refuses, and the only refusal on the
    /// way *out* rather than in: Arrow counts nanoseconds where PostgreSQL's
    /// field counts microseconds, so a nanosecond count with a remainder is
    /// not an `interval` at all. `decode_interval` cannot produce one, which
    /// is why the whole-microsecond neighbours are asserted beside it —
    /// truncating would have written `00:00:00.000001` for all three.
    #[test]
    fn render_refuses_a_sub_microsecond_nanosecond_count() {
        assert_eq!(render_interval(0, 0, 1), None);
        assert_eq!(render_interval(0, 0, -1), None);
        assert_eq!(render_interval(0, 0, 1_001), None);
        assert_eq!(render_interval(1, 1, 999), None);
        assert_eq!(render_interval(0, 0, 1_000).as_deref(), Some("00:00:00.000001"));
        assert_eq!(render_interval(0, 0, 2_000).as_deref(), Some("00:00:00.000002"));
    }

    #[test]
    fn time_boundary_and_fraction_trimming() {
        // PostgreSQL's inclusive bound, past Arrow's day: refused by the
        // decoder, ordered by the comparison, and rendered back all the same.
        assert_eq!(decode_time64_micros("24:00:00"), None);
        assert_eq!(time_of_day_micros("24:00:00"), Some(86_400_000_000));
        assert_eq!(decode_time64_micros("23:59:59.999999"), Some(86_399_999_999));
        assert_eq!(render_time64_micros(86_400_000_000), "24:00:00");
        assert_eq!(decode_time64_micros("00:00:00.000001"), Some(1));
        assert_eq!(render_time64_micros(1), "00:00:00.000001");
        // PostgreSQL trims trailing zeros from the fraction rather than
        // always showing 6 digits.
        assert_eq!(render_time64_micros(500_000), "00:00:00.5");
        assert_eq!(render_time64_micros(100_000), "00:00:00.1");
    }

    #[test]
    fn timestamp_boundary_values_without_tz() {
        for text in [
            "2024-01-01 00:00:00",
            "2024-01-01 00:00:00.123456",
            "0001-01-01 00:00:00",
            "0044-01-01 00:00:00 BC",
        ] {
            let micros = decode_timestamp_micros(text, false).expect(text);
            assert_eq!(render_timestamp_micros(micros, false), text, "{text}");
        }
        assert_eq!(decode_timestamp_micros("infinity", false), None);
        assert_eq!(decode_timestamp_micros("-infinity", false), None);
    }

    /// PostgreSQL's own documented maximum timestamp is defined relative to
    /// *its* epoch (2000-01-01); `Timestamp(Microsecond)` is `i64` micros
    /// since the Unix epoch (1970-01-01), 30 years earlier, so the same
    /// bit width runs out about 30 years sooner counted from 1970 than from
    /// 2000: the true representable ceiling is 294247-01-10, not
    /// PostgreSQL's 294276-12-31 — so PostgreSQL's own maximum value is a
    /// genuine, expected `FieldDecode` overflow for this mapping, not a bug.
    #[test]
    fn postgresqls_own_max_timestamp_overflows_the_unix_epoch_i64_range() {
        assert_eq!(decode_timestamp_micros("294276-12-31 23:59:59.999999", false), None);
        assert_eq!(decode_timestamp_micros("294276-12-31 23:59:59.999999+00", true), None);
        // A date well inside both ceilings still round-trips fine.
        let text = "294246-12-31 23:59:59.999999";
        let micros = decode_timestamp_micros(text, false).unwrap();
        assert_eq!(render_timestamp_micros(micros, false), text);
    }

    #[test]
    fn timestamptz_normalizes_and_renders_with_utc_offset() {
        for text in
            ["2023-12-31 18:30:00+00", "0001-01-01 00:00:00+00", "0044-01-01 00:00:00+00 BC"]
        {
            let micros = decode_timestamp_micros(text, true).expect(text);
            assert_eq!(render_timestamp_micros(micros, true), text, "{text}");
        }
        // A non-UTC offset is still normalized correctly even though every
        // real `pg_dump` output uses +00 (I4) -- +05:00 local is 05:00 UTC
        // earlier in wall-clock terms.
        let with_offset = decode_timestamp_micros("2024-01-01 05:00:00+05", true).unwrap();
        let utc = decode_timestamp_micros("2024-01-01 00:00:00+00", true).unwrap();
        assert_eq!(with_offset, utc);
    }

    #[test]
    fn uuid_round_trips() {
        for text in ["00000000-0000-0000-0000-000000000000", "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11"]
        {
            let bytes = decode_uuid(text).expect(text);
            assert_eq!(render_uuid(&bytes), text);
        }
        assert_eq!(decode_uuid("not-a-uuid"), None);
    }

    #[test]
    fn bytea_round_trips_including_empty() {
        for text in ["\\x", "\\xdeadbeef00ff", "\\x5c6261636b736c617368"] {
            let bytes = decode_bytea(text).expect(text);
            assert_eq!(render_bytea(&bytes), text);
        }
        assert_eq!(decode_bytea("\\xnope"), None);
    }

    /// `byteaout`'s `escape` form, every byte through it and back, beside
    /// what the server writes for a few (`select '\x005c7f41'::bytea` under
    /// `bytea_output = escape` is `\000\\\177A`).
    #[test]
    fn bytea_escape_form_round_trips_every_byte() {
        let every_byte: Vec<u8> = (0..=u8::MAX).collect();
        let text = render_bytea_escape(&every_byte);
        assert_eq!(decode_bytea(&text), Some(every_byte.clone()));
        assert_eq!(decode_bytea_escape(&text, 3), Some(every_byte[..3].to_vec()));
        for b in 0..=u8::MAX {
            assert_eq!(decode_bytea(&render_bytea_escape(&[b])), Some(vec![b]), "{b}");
        }
        assert_eq!(render_bytea_escape(&[0x00, 0x5c, 0x7f, 0x41]), "\\000\\\\\\177A");
        assert_eq!(decode_bytea("\\000\\\\\\177A"), Some(vec![0x00, 0x5c, 0x7f, 0x41]));
        // The empty value is the empty text, and no escape spelling opens
        // `\x` (I56).
        assert_eq!(decode_bytea(""), Some(Vec::new()));
        assert!(!text.starts_with("\\x"));
    }

    /// What `byteaout` never writes is refused (`docs/design/decisions.md`,
    /// "D55"), though `byteain` reads some of it: a printable byte in octal,
    /// an octal escape past a byte, one cut short, a lone backslash, a byte
    /// outside space to `~` written bare.
    #[test]
    fn bytea_escape_form_refuses_what_byteaout_never_writes() {
        for text in ["\\101", "\\134", "\\400", "\\08", "\\00", "a\\", "\\y", "tab\there", "é"] {
            assert_eq!(decode_bytea(text), None, "{text:?}");
        }
    }

    #[test]
    fn decimal_numeric_and_boundary_scales() {
        // 38-digit precision, scale 10 (fits Decimal128).
        let text = "1234567890123456789012345678.1234567890";
        let unscaled = decimal_unscaled_digits(text, 10).unwrap();
        assert_eq!(unscaled.parse::<i128>().unwrap().to_string(), unscaled);
        assert_eq!(render_decimal(&unscaled, 10), text);

        // 39-digit precision (Decimal256).
        let text39 = "123456789.1234567890";
        let unscaled39 = decimal_unscaled_digits(text39, 10).unwrap();
        let as_i256 = i256::from_string(&unscaled39).unwrap();
        assert_eq!(render_decimal(&as_i256.to_string(), 10), text39);

        for (text, scale) in
            [("0.0000000000", 10), ("-1.5000000000", 10), ("0.00", 2), ("-1.50", 2), ("100.00", 2)]
        {
            let unscaled = decimal_unscaled_digits(text, scale).unwrap();
            assert_eq!(render_decimal(&unscaled, scale), text);
        }
    }

    /// **`numeric_in`'s two storage bounds, each at its edge** (I63): a
    /// display scale of 16383 digits, trailing zeros counting, and 131072
    /// integer digits, leading zeros not counting, whatever the sign.
    #[test]
    fn numeric_in_stores_up_to_its_display_scale_and_weight() {
        let zeros = |n| "0".repeat(n);
        for (text, stored) in [
            (format!("1.{}", zeros(16383)), true),
            (format!("1.{}", zeros(16384)), false),
            (format!("0.{}", zeros(16384)), false),
            (format!("-.{}1", zeros(16382)), true),
            (format!("1{}", zeros(131071)), true),
            (format!("1{}", zeros(131072)), false),
            (format!("-1{}", zeros(131072)), false),
            (format!("{}1.5", zeros(200_000)), true),
            ("0".to_string(), true),
        ] {
            assert_eq!(numeric_in_stores(&text), stored, "{}", text.len());
        }
    }

    #[test]
    fn nan_numeric_has_no_decimal_representation() {
        assert_eq!(decimal_unscaled_digits("NaN", 2), None);
    }

    #[test]
    fn negative_scale_numeric() {
        // PG15+ negative-scale numerics print with no fractional digits at
        // all — not exercised by the fixtures (no negative-scale column
        // there), so pinned here from the type's own documented semantics.
        let unscaled = decimal_unscaled_digits("1200", -2).unwrap();
        assert_eq!(unscaled, "12");
        assert_eq!(render_decimal(&unscaled, -2), "1200");
    }

    /// **Zero is `0` at every negative scale**, which is how `numeric_out`
    /// prints it whatever the typmod, though every other value of the column
    /// carries the scale's trailing zeros: it decodes, to `0`, and renders
    /// back as `0`. A short digit string that is not zero stays refused, it
    /// being no multiple of the scale's power of ten; and `i8::MIN` renders,
    /// its magnitude being past `i8`.
    #[test]
    fn a_negative_scale_zero_is_zero_and_renders_as_written() {
        for scale in i8::MIN..0 {
            let zeros = usize::from(scale.unsigned_abs());
            for text in ["0", "-0", "0.0", "00"] {
                assert_eq!(
                    decimal_unscaled_digits(text, scale).as_deref(),
                    Some("0"),
                    "{text:?} at scale {scale}"
                );
            }
            assert_eq!(render_decimal("0", scale), "0", "at scale {scale}");

            let one = format!("1{}", "0".repeat(zeros));
            assert_eq!(decimal_unscaled_digits(&one, scale).as_deref(), Some("1"), "at {scale}");
            assert_eq!(render_decimal("1", scale), one, "at scale {scale}");
            let short = format!("1{}", "0".repeat(zeros - 1));
            assert_eq!(decimal_unscaled_digits(&short, scale), None, "{short:?} at {scale}");
        }
    }
}

/// The straightforward `format!`/`parse` spellings of the four scalar
/// decoders, the two hex renderers and the three date/time renderers, kept as
/// the oracle the allocation-free forms above are checked against. Those forms
/// are asked to be exactly this, so the corpora below are generated rather
/// than listed: a disagreement on an input nobody thought to write down is a
/// test failure and not a report from the field.
#[cfg(test)]
mod prior_shape {
    use super::civil_from_days;
    use crate::copy::hex_val;

    fn format_hms_frac(total_micros: i64) -> String {
        let seconds = total_micros.div_euclid(1_000_000);
        let micros = total_micros.rem_euclid(1_000_000);
        let h = seconds / 3600;
        let mi = (seconds % 3600) / 60;
        let se = seconds % 60;
        if micros == 0 {
            format!("{h:02}:{mi:02}:{se:02}")
        } else {
            let mut frac = format!("{micros:06}");
            while frac.ends_with('0') {
                frac.pop();
            }
            format!("{h:02}:{mi:02}:{se:02}.{frac}")
        }
    }

    pub fn render_date32(days: i32) -> String {
        let (y, m, d) = civil_from_days(i64::from(days));
        let (out_year, bc) = if y <= 0 { (1 - y, true) } else { (y, false) };
        let mut s = format!("{out_year:04}-{m:02}-{d:02}");
        if bc {
            s.push_str(" BC");
        }
        s
    }

    pub fn render_time64_micros(v: i64) -> String {
        format_hms_frac(v)
    }

    pub fn render_timestamp_micros(v: i64, with_tz: bool) -> String {
        let days = v.div_euclid(86_400_000_000);
        let time_micros = v.rem_euclid(86_400_000_000);
        let (y, m, d) = civil_from_days(days);
        let (out_year, bc) = if y <= 0 { (1 - y, true) } else { (y, false) };
        let mut s = format!("{out_year:04}-{m:02}-{d:02} {}", format_hms_frac(time_micros));
        if with_tz {
            s.push_str("+00");
        }
        if bc {
            s.push_str(" BC");
        }
        s
    }

    pub fn render_uuid(bytes: &[u8; 16]) -> String {
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        format!(
            "{}-{}-{}-{}-{}",
            hex(&bytes[0..4]),
            hex(&bytes[4..6]),
            hex(&bytes[6..8]),
            hex(&bytes[8..10]),
            hex(&bytes[10..16])
        )
    }

    pub fn render_bytea(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(2 + bytes.len() * 2);
        s.push_str("\\x");
        for b in bytes {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    pub fn parse_time_of_day(s: &str) -> Option<(i64, i64)> {
        let (hms, frac) = s.split_once('.').unwrap_or((s, ""));
        let mut parts = hms.splitn(3, ':');
        let h: i64 = parts.next()?.parse().ok()?;
        let mi: i64 = parts.next()?.parse().ok()?;
        let se: i64 = parts.next()?.parse().ok()?;
        if parts.next().is_some() || frac.len() > 6 || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let mut frac_digits = frac.to_string();
        frac_digits.push_str(&"0".repeat(6 - frac_digits.len()));
        let micros: i64 = if frac_digits.is_empty() { 0 } else { frac_digits.parse().ok()? };
        Some((h * 3600 + mi * 60 + se, micros))
    }

    pub fn decode_uuid(s: &str) -> Option<[u8; 16]> {
        let clean: String = s.chars().filter(|c| *c != '-').collect();
        if clean.len() != 32 {
            return None;
        }
        let b = clean.as_bytes();
        let mut bytes = [0u8; 16];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = (hex_val(b[i * 2])? << 4 | hex_val(b[i * 2 + 1])?) as u8;
        }
        Some(bytes)
    }

    pub fn decode_bytea(s: &str) -> Option<Vec<u8>> {
        let hex = s.strip_prefix("\\x")?;
        if hex.len() % 2 != 0 {
            return None;
        }
        let b = hex.as_bytes();
        let mut out = Vec::with_capacity(hex.len() / 2);
        for i in (0..b.len()).step_by(2) {
            out.push((hex_val(b[i])? << 4 | hex_val(b[i + 1])?) as u8);
        }
        Some(out)
    }

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
        let digits = format!("{int_part}{frac_part}");
        let shift = i32::from(scale) - frac_part.len() as i32;
        let unscaled = if shift >= 0 {
            format!("{digits}{}", "0".repeat(shift as usize))
        } else {
            let cut = (-shift) as usize;
            if cut > digits.len() {
                return digits.bytes().all(|b| b == b'0').then(|| "0".to_string());
            }
            let (keep, dropped) = digits.split_at(digits.len() - cut);
            if !dropped.bytes().all(|b| b == b'0') {
                return None;
            }
            keep.to_string()
        };
        let unscaled = if unscaled.is_empty() { "0" } else { unscaled.trim_start_matches('0') };
        let unscaled = if unscaled.is_empty() { "0" } else { unscaled };
        Some(if neg && unscaled != "0" { format!("-{unscaled}") } else { unscaled.to_string() })
    }
}

#[cfg(test)]
mod differential {
    use super::*;

    /// A deterministic 64-bit LCG, so a failure is reproducible from the
    /// seed alone and the corpus does not have to be committed.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 11
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }

        fn pick(&mut self, from: &[u8]) -> char {
            char::from(from[self.below(from.len())])
        }
    }

    /// Bytes that reach a hex or digit loop: the valid ones, the ones that
    /// sit just outside each accepted range, and the separators the grammars
    /// use; `fuzz` and `shaped` add one multi-byte character.
    const ALPHABET: &[u8] = b"0123456789abcdefABCDEFgGxX-.:+ \\/`z@\x7f";

    fn fuzz(seed: u64, len: usize, f: impl Fn(&str)) {
        let mut rng = Rng(seed);
        for _ in 0..20_000 {
            let n = rng.below(len) + 1;
            let mut s = String::new();
            for _ in 0..n {
                if rng.below(32) == 0 {
                    s.push('é');
                } else {
                    s.push(rng.pick(ALPHABET));
                }
            }
            f(&s);
        }
    }

    /// The free-form corpus above almost never lands on a *well-formed*
    /// literal — one in twenty thousand, for a time of day — so each decoder
    /// also gets a corpus built to its own grammar and then perturbed, which
    /// is where the accepting branches actually get exercised.
    fn shaped(seed: u64, build: impl Fn(&mut Rng) -> String, f: impl Fn(&str)) {
        let mut rng = Rng(seed);
        for _ in 0..20_000 {
            let s = build(&mut rng);
            // Two thirds are left well-formed; the rest are damaged at one
            // position, which is what puts a rejecting branch beside every
            // accepting one.
            let s = if rng.below(3) == 0 && !s.is_empty() {
                let mut chars: Vec<char> = s.chars().collect();
                let at = rng.below(chars.len());
                chars[at] = if rng.below(16) == 0 { 'é' } else { rng.pick(ALPHABET) };
                chars.into_iter().collect()
            } else {
                s
            };
            f(&s);
        }
    }

    fn digits(rng: &mut Rng, count: usize) -> String {
        (0..count).map(|_| rng.pick(b"0123456789")).collect()
    }

    /// Whether `time_overflows` or a signed part refuses `s`, which the
    /// shape [`parse_time_of_day`] replaced read: an hour past 24, a minute
    /// past 59, a second past 60 or a whole past `24:00:00`, or a part
    /// carrying a sign, the server's zone (I61).
    fn past_time_overflows(s: &str) -> bool {
        let (hms, frac) = s.split_once('.').unwrap_or((s, ""));
        let parts: Vec<&str> = hms.split(':').collect();
        if parts.iter().any(|p| !p.bytes().all(|b| b.is_ascii_digit())) {
            return true;
        }
        let [h, mi, se] = [0, 1, 2].map(|i| parts[i].parse::<i64>().unwrap());
        let fraction = !frac.is_empty() && frac.bytes().any(|b| b != b'0');
        h > 24 || mi > 59 || se > 60 || (h * 3600 + mi * 60 + se, fraction) > (86_400, false)
    }

    /// **The rewrite reads what the prior shape read, inside the bounds the
    /// server keeps**: past them the prior shape read on where this refuses.
    #[test]
    fn time_of_day_agrees_with_the_shape_it_replaced() {
        let check = |s: &str| match (parse_time_of_day(s), prior_shape::parse_time_of_day(s)) {
            (None, Some(_)) => assert!(past_time_overflows(s), "{s:?}"),
            (got, prior) => assert_eq!(got, prior, "{s:?}"),
        };
        fuzz(1, 20, check);
        shaped(
            11,
            |rng| {
                let widths = [rng.below(3) + 1, rng.below(3) + 1, rng.below(3) + 1];
                let h = digits(rng, widths[0]);
                let mi = digits(rng, widths[1]);
                let se = digits(rng, widths[2]);
                let mut s = format!("{h}:{mi}:{se}");
                // Zero fractional digits through eight: six is the limit, so
                // the widths either side of it are both in the corpus.
                let frac = rng.below(9);
                if frac > 0 {
                    s.push('.');
                    s.push_str(&digits(rng, frac));
                }
                s
            },
            check,
        );
        // Every fractional width, which is what the multiply stands in for.
        for (text, want) in [
            ("00:00:00", 0),
            ("00:00:00.5", 500_000),
            ("00:00:00.05", 50_000),
            ("00:00:00.005", 5_000),
            ("00:00:00.0005", 500),
            ("00:00:00.00005", 50),
            ("00:00:00.000005", 5),
            ("00:00:00.000000", 0),
        ] {
            assert_eq!(parse_time_of_day(text), Some((0, want)), "{text}");
        }
        assert_eq!(parse_time_of_day("00:00:00.0000005"), None);
    }

    #[test]
    fn uuid_agrees_with_the_shape_it_replaced() {
        let check = |s: &str| assert_eq!(decode_uuid(s), prior_shape::decode_uuid(s), "{s:?}");
        fuzz(2, 40, check);
        shaped(
            22,
            |rng| {
                // 31, 32 or 33 hex digits, with hyphens scattered through
                // them rather than only at the canonical four positions.
                let count = 31 + rng.below(3);
                let mut s = String::new();
                for _ in 0..count {
                    if rng.below(6) == 0 {
                        s.push('-');
                    }
                    s.push(rng.pick(b"0123456789abcdefABCDEF"));
                }
                s
            },
            check,
        );
        // A canonical value, and the two ways to be the wrong length.
        let canonical = "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11";
        assert!(decode_uuid(canonical).is_some());
        assert_eq!(decode_uuid(&canonical[..35]), None);
        assert_eq!(decode_uuid(&format!("{canonical}0")), None);
        // Hyphens are dropped wherever they fall, which is what the byte
        // filter has to keep doing.
        assert_eq!(decode_uuid("-a0eebc999c0b4ef8bb6d6bb9bd380a11-"), decode_uuid(canonical));
    }

    #[test]
    fn bytea_agrees_with_the_shape_it_replaced() {
        let check = |s: &str| {
            let escaped = format!("\\x{s}");
            // A text without the prefix is the `escape` form, which the
            // shape replaced did not read.
            assert_eq!(decode_bytea(&escaped), prior_shape::decode_bytea(&escaped), "{escaped:?}");
        };
        fuzz(3, 24, check);
        // Both parities of length, including the empty `\x`.
        shaped(
            33,
            |rng| {
                let count = rng.below(17);
                (0..count).map(|_| rng.pick(b"0123456789abcdefABCDEF")).collect()
            },
            check,
        );
    }

    /// The two hex renderers take bytes rather than text, so their corpus is
    /// enumerated rather than fuzzed: every entry of the pair table is
    /// reachable, and for the `uuid` every entry at every one of the sixteen
    /// positions the hyphens are interleaved into.
    #[test]
    fn hex_render_agrees_with_the_shape_it_replaced() {
        let every_byte: Vec<u8> = (0..=u8::MAX).collect();
        assert_eq!(render_bytea(&every_byte), prior_shape::render_bytea(&every_byte));
        for b in 0..=u8::MAX {
            assert_eq!(render_bytea(&[b]), prior_shape::render_bytea(&[b]), "{b}");
        }
        // The empty value (`\x` alone) and every length either side of the
        // pair-chunking the decoder's inverse does.
        for len in 0..=33usize {
            let v: Vec<u8> = (0..len).map(|i| every_byte[(i * 7 + 3) % 256]).collect();
            assert_eq!(render_bytea(&v), prior_shape::render_bytea(&v), "{v:?}");
        }

        for at in 0..16 {
            for b in 0..=u8::MAX {
                let mut bytes = [0u8; 16];
                bytes[at] = b;
                assert_eq!(render_uuid(&bytes), prior_shape::render_uuid(&bytes), "{at} {b}");
            }
        }
        let mut rng = Rng(5);
        for _ in 0..20_000 {
            let mut bytes = [0u8; 16];
            for b in &mut bytes {
                *b = rng.next() as u8;
            }
            assert_eq!(render_uuid(&bytes), prior_shape::render_uuid(&bytes), "{bytes:?}");
        }
    }

    /// The three date/time renderers take integers rather than text, so their
    /// corpus is enumerated and swept rather than fuzzed. Three regions
    /// matter and each is covered on its own: the dense one every real value
    /// lands in, a stride across the whole width of the argument type, and
    /// the values only a caller-assembled Arrow array can hold — a negative
    /// `Time64`, an hour past 99, a year past four digits — which are what
    /// the two-digit and four-digit fast paths have to fall out of rather
    /// than panic on.
    #[test]
    fn date_and_time_render_agrees_with_the_shape_it_replaced() {
        // Every day of a six-century window around the epoch, then a stride
        // over the whole of `i32` — which reaches years either side of five
        // million and so the wide-year path — and both ends exactly.
        for days in -200_000..=200_000i32 {
            assert_eq!(render_date32(days), prior_shape::render_date32(days), "{days}");
        }
        let mut days = i32::MIN;
        loop {
            assert_eq!(render_date32(days), prior_shape::render_date32(days), "{days}");
            match days.checked_add(100_003) {
                Some(next) => days = next,
                None => break,
            }
        }
        for days in [i32::MIN, i32::MAX, 0, -1, 1, 719_468] {
            assert_eq!(render_date32(days), prior_shape::render_date32(days), "{days}");
        }

        // Every fractional width the trim can stop at, against a stride over
        // the whole day — the `.5`/`.000001`/no-fraction branches.
        let fractions =
            [0i64, 1, 10, 100, 1_000, 10_000, 100_000, 500_000, 120_000, 123_456, 999_999];
        for second in (0..86_401i64).step_by(97) {
            for frac in fractions {
                let v = second * 1_000_000 + frac;
                assert_eq!(render_time64_micros(v), prior_shape::render_time64_micros(v), "{v}");
            }
        }
        for v in
            [0i64, -1, 1, 86_400_000_000, 360_000_000_000, -360_000_000_000, i64::MIN, i64::MAX]
        {
            assert_eq!(render_time64_micros(v), prior_shape::render_time64_micros(v), "{v}");
        }

        // Timestamps in the range a dump actually holds, at both spellings,
        // then random `i64`s — which reach BC years, six-digit years and the
        // negative time-of-day the `rem_euclid` cannot produce but
        // `render_time64_micros` can.
        for day in (-800_000..=800_000i64).step_by(4_001) {
            for frac in fractions {
                for with_tz in [false, true] {
                    let v = day * 86_400_000_000 + frac;
                    assert_eq!(
                        render_timestamp_micros(v, with_tz),
                        prior_shape::render_timestamp_micros(v, with_tz),
                        "{v} {with_tz}"
                    );
                }
            }
        }
        let mut rng = Rng(7);
        for _ in 0..20_000 {
            let v = rng.next() as i64;
            for with_tz in [false, true] {
                assert_eq!(
                    render_timestamp_micros(v, with_tz),
                    prior_shape::render_timestamp_micros(v, with_tz),
                    "{v} {with_tz}"
                );
                assert_eq!(render_time64_micros(v), prior_shape::render_time64_micros(v), "{v}");
            }
        }
        for v in [i64::MIN, i64::MAX, 0, -1, 1, -86_400_000_000] {
            for with_tz in [false, true] {
                assert_eq!(
                    render_timestamp_micros(v, with_tz),
                    prior_shape::render_timestamp_micros(v, with_tz),
                    "{v} {with_tz}"
                );
            }
        }
    }

    /// [`push_integer`] is asked to be `i64::to_string`, so it is checked
    /// against it: every value of a two-digit and three-digit width, where the
    /// pair loop's odd/even tail lives, both ends of every integer type a
    /// column can resolve to, and a random sweep of the whole `i64` range.
    #[test]
    fn integer_render_agrees_with_to_string() {
        let check = |v: i64| {
            let mut out = String::from("|");
            push_integer(&mut out, v);
            assert_eq!(out, format!("|{v}"), "{v}");
        };
        for v in -1_000..=1_000i64 {
            check(v);
        }
        for v in [
            0,
            i64::from(i16::MIN),
            i64::from(i16::MAX),
            i64::from(i32::MIN),
            i64::from(i32::MAX),
            i64::from(u32::MAX),
            i64::MIN,
            i64::MAX,
            -9,
            -10,
            -99,
            -100,
            9,
            10,
            99,
            100,
            999,
            1_000,
        ] {
            check(v);
        }
        let mut rng = Rng(9);
        for _ in 0..50_000 {
            let v = rng.next() as i64;
            check(v);
            check(v % 1_000_000);
            check(v >> 32);
        }
    }

    /// The `_into` forms are what the owned ones call, so the property worth
    /// asserting is the one that would break if that ever stopped being true:
    /// appending to a buffer that already holds text leaves what was there
    /// alone and appends exactly the owned form.
    #[test]
    fn the_sink_forms_append_exactly_what_the_owned_forms_return() {
        let mut out = String::from("head\t");
        render_date32_into(19_889, &mut out);
        out.push('\t');
        render_time64_micros_into(49_530_123_456, &mut out);
        out.push('\t');
        render_timestamp_micros_into(1_718_000_000_000_000, false, &mut out);
        out.push('\t');
        render_timestamp_micros_into(1_718_000_000_000_000, true, &mut out);
        assert_eq!(
            out,
            format!(
                "head\t{}\t{}\t{}\t{}",
                render_date32(19_889),
                render_time64_micros(49_530_123_456),
                render_timestamp_micros(1_718_000_000_000_000, false),
                render_timestamp_micros(1_718_000_000_000_000, true),
            )
        );
        // A trimmed fraction truncates back to its own last non-zero digit
        // and never into the caller's text, even when that text ends in one.
        let mut zero_tailed = String::from("00");
        render_time64_micros_into(500_000, &mut zero_tailed);
        assert_eq!(zero_tailed, "0000:00:00.5");
    }

    #[test]
    fn numeric_agrees_with_the_shape_it_replaced() {
        for scale in [-3i8, -1, 0, 1, 2, 6, 10] {
            let check = |s: &str| {
                assert_eq!(
                    decimal_unscaled_digits(s, scale),
                    prior_shape::decimal_unscaled_digits(s, scale),
                    "{s:?} at scale {scale}"
                );
            };
            fuzz(u64::from(scale.unsigned_abs()) + 4, 16, check);
            shaped(
                u64::from(scale.unsigned_abs()) + 44,
                |rng| {
                    // Leading zeros, all-zero values and both signs, since
                    // the trim, the collapse to `"0"` and the sign are the
                    // three places the two forms could disagree.
                    let mut s = String::new();
                    if rng.below(3) == 0 {
                        s.push('-');
                    }
                    let int = rng.below(6);
                    let frac = rng.below(12);
                    if rng.below(4) == 0 {
                        s.push_str(&"0".repeat(int + frac));
                        if frac > 0 {
                            s.insert(s.len() - frac, '.');
                        }
                        return s;
                    }
                    s.push_str(&digits(rng, int));
                    if frac > 0 || rng.below(2) == 0 {
                        s.push('.');
                        s.push_str(&digits(rng, frac));
                    }
                    s
                },
                check,
            );
        }
    }
}
