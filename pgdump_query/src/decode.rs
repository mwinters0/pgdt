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

pub fn decode_f32(s: &str) -> Option<f32> {
    match s {
        "NaN" => Some(f32::NAN),
        "Infinity" => Some(f32::INFINITY),
        "-Infinity" => Some(f32::NEG_INFINITY),
        _ => s.parse().ok(),
    }
}

pub fn decode_f64(s: &str) -> Option<f64> {
    match s {
        "NaN" => Some(f64::NAN),
        "Infinity" => Some(f64::INFINITY),
        "-Infinity" => Some(f64::NEG_INFINITY),
        _ => s.parse().ok(),
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

/// `YYYY-MM-DD`, year unpadded past 4 digits and never negative (BCE is the
/// `" BC"` suffix, handled by the caller) — `split_era` runs first, so this
/// never sees one.
fn parse_ymd(s: &str) -> Option<(i64, u32, u32)> {
    let mut parts = s.splitn(3, '-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((y, m, d))
}

/// `1 - year` turns a `" BC"`-suffixed calendar year into PostgreSQL's (and
/// the proleptic Gregorian calendar's) astronomical year numbering: 1 BC is
/// astronomical year 0, 44 BC is -43, and so on.
fn astronomical_year(y: i64, bc: bool) -> i64 {
    if bc { 1 - y } else { y }
}

/// `None` for `infinity`/`-infinity` (PostgreSQL's own pseudo-values for
/// "unbounded" — real, but `Date32` has no sentinel for them, so this is a
/// decode failure by construction, not by accident) and for anything out of
/// `Date32`'s `i32` day range.
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
    let (y, m, d) = parse_ymd(rest)?;
    let days = days_from_civil(astronomical_year(y, bc), m, d);
    i32::try_from(days).ok()
}

pub fn render_date32(days: i32) -> String {
    // `YYYY-MM-DD`, with room for the era marker; a year outside four digits
    // grows it once and is not a value any dump holds.
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
pub(crate) fn parse_time_of_day(s: &str) -> Option<(i64, i64)> {
    let (hms, frac) = s.split_once('.').unwrap_or((s, ""));
    let mut parts = hms.splitn(3, ':');
    let h: i64 = parts.next()?.parse().ok()?;
    let mi: i64 = parts.next()?.parse().ok()?;
    let se: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || frac.len() > 6 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // `frac` is checked above to be at most six ASCII digits, so padding it
    // to six and parsing the result — two allocations per field — is exactly
    // a scale by a power of ten: `.5` is 500000 µs, `.000001` is 1.
    let mut micros: i64 = 0;
    for b in frac.bytes() {
        micros = micros * 10 + i64::from(b - b'0');
    }
    Some((h * 3600 + mi * 60 + se, micros * POW10[6 - frac.len()]))
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
pub(crate) fn extract_offset(s: &str) -> Option<(&str, i64)> {
    let idx = s.find(['+', '-'])?;
    let (time_only, off) = s.split_at(idx);
    let (sign, rest) = off.split_at(1);
    let sign_mult: i64 = if sign == "-" { -1 } else { 1 };
    let mut parts = rest.splitn(3, ':');
    let hh: i64 = parts.next()?.parse().ok()?;
    let mm: i64 = parts.next().map(str::parse).transpose().ok()?.unwrap_or(0);
    let ss: i64 = parts.next().map(str::parse).transpose().ok()?.unwrap_or(0);
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
/// the 1970 UTC epoch for any finite value the grammar reads, so a value past
/// what `Timestamp(Microsecond)` holds is told from one that does not parse
/// (`crate::unrepresentable`).
pub(crate) fn timestamp_micros_wide(s: &str, with_tz: bool) -> Option<i128> {
    if s == "infinity" || s == "-infinity" {
        return None;
    }
    let (rest, bc) = split_era(s);
    let (date_part, time_part) = rest.split_once(' ')?;
    let (y, m, d) = parse_ymd(date_part)?;
    let days = days_from_civil(astronomical_year(y, bc), m, d);

    let (time_only, offset_secs) =
        if with_tz { extract_offset(time_part)? } else { (time_part, 0) };
    let (seconds, micros) = parse_time_of_day(time_only)?;

    let local_micros =
        i128::from(days) * 86_400_000_000 + i128::from(seconds) * 1_000_000 + i128::from(micros);
    Some(local_micros - i128::from(offset_secs) * 1_000_000)
}

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
/// The hour field is unbounded, so this is not [`decode_time64_micros`]:
/// `720:00:00` is an ordinary `interval` and not a `time`. Every field is
/// checked to be digits, which is what keeps `04:-5:06` — a string no
/// `interval_out` writes and no `interval_in` accepts — from parsing as a
/// negative minute count.
fn interval_time_micros(text: &str) -> Option<i128> {
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
    Some(if negative { -total } else { total })
}

/// The three parts of an `interval`'s text, in PostgreSQL's own units and
/// before any narrowing: whole months (a `year` part folded in at twelve
/// each), whole days, and the time tail in **microseconds**. The last is 128
/// bits because the *comparison* built on this fuses all three into a 128-bit
/// span (`interval_cmp_value`, I40), where the *decode* narrows to Arrow's
/// three fields — one walk, two consumers, and the fusing belongs to neither.
///
/// **The grammar covers `interval_out`'s under `IntervalStyle = postgres`**,
/// which `pg_dump` pins on its own connection (I4): an optional
/// `<n> year[s]`, `<n> mon[s]` and `<n> day[s]`, then an optional signed time
/// part, separated by single spaces, with a wholly-zero interval written
/// `00:00:00`. It is a little wider than that grammar — the counted parts are
/// taken in any order, repeated, either number of their unit, a `+` on any
/// of them, and the hours at any width — but no other unit: `1 hour`,
/// `1.5 hours`, `P1Y2M` and `1 month` are all spellings `interval_in` takes
/// and `interval_out` never writes, and are refused. `infinity`/`-infinity` (v17's, I34) are not in the grammar
/// either, so they fail here and each consumer says what it does about them.
pub(crate) fn interval_parts(text: &str) -> Option<(i64, i64, i128)> {
    let tokens: Vec<&str> = text.split(' ').collect();
    let (mut months, mut days) = (0i64, 0i64);
    let mut time = 0i128;
    let mut at = 0;
    while at < tokens.len() {
        let count = || interval_count(tokens[at]);
        match tokens.get(at + 1).copied() {
            Some("year" | "years") => months = months.checked_add(count()?.checked_mul(12)?)?,
            Some("mon" | "mons") => months = months.checked_add(count()?)?,
            Some("day" | "days") => days = days.checked_add(count()?)?,
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
        }
        at += 2;
    }
    if at != tokens.len() {
        return None;
    }
    Some((months, days, time))
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
/// - a month or day count outside `i32`, which no `Interval` struct can hold
///   and so no dump can contain.
pub fn decode_interval(s: &str) -> Option<(i32, i32, i64)> {
    let (months, days, micros) = interval_parts(s)?;
    let nanos = i64::try_from(micros.checked_mul(1_000)?).ok()?;
    Some((i32::try_from(months).ok()?, i32::try_from(days).ok()?, nanos))
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

/// PostgreSQL's default `bytea_output = hex` form, `\x` followed by
/// lowercase hex pairs — the decoded text (a single backslash), not the
/// doubled-backslash form COPY escaping writes to disk (see the module
/// docs).
pub fn decode_bytea(s: &str) -> Option<Vec<u8>> {
    let hex = s.strip_prefix("\\x")?;
    let b = hex.as_bytes();
    if b.len() % 2 != 0 {
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
/// trailing zeros instead of appending them.
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
            return None;
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

/// Render an unscaled decimal integer (`i128`/`i256`'s own `Display`, e.g.
/// `"-15000000000"`) back to PostgreSQL's fixed-`scale` text form. Exact
/// inverse of [`decimal_unscaled_digits`].
pub fn render_decimal(unscaled: &str, scale: i8) -> String {
    let (neg, digits) = match unscaled.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, unscaled),
    };
    let body = if scale <= 0 {
        format!("{digits}{}", "0".repeat((-scale) as usize))
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
        assert_eq!(decode_bytea("nope"), None);
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
                return None;
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
    /// sit just outside each accepted range, the separators the grammars
    /// use, and one multi-byte character.
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

    #[test]
    fn time_of_day_agrees_with_the_shape_it_replaced() {
        let check =
            |s: &str| assert_eq!(parse_time_of_day(s), prior_shape::parse_time_of_day(s), "{s:?}");
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
            assert_eq!(decode_bytea(&escaped), prior_shape::decode_bytea(&escaped), "{escaped:?}");
            assert_eq!(decode_bytea(s), prior_shape::decode_bytea(s), "{s:?}");
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
