//! Per-type field decode and render-back (Phase 2.4, "Type mapping" /
//! "Failure and diagnostics" in `docs/design/roadmap-phase2-typed-columns.md`).
//!
//! Pure, synchronous, no I/O — see `docs/design/layering.md`, L2. A decode
//! function takes an already-COPY-unescaped `&str` field (what
//! `crate::copy::decode_field` returns) and returns a plain Rust value; it
//! never takes an Arrow builder — building the array is `batch.rs`'s job
//! (L3), which is also where these are wired to their [`arrow::datatypes::DataType`].
//!
//! Every `render_*` function is the exact inverse of its `decode_*`
//! counterpart and produces the same *decoded* text `crate::copy::decode_field`
//! would have returned for a correctly-formed value — never the raw
//! COPY-escaped bytes on disk. Converting between the two is
//! `crate::copy::encode_field`'s job, not this module's: this module's whole
//! job is decoded text vs. Arrow value, and reaching past that into
//! COPY-escaping would blur the L1/L2 split `docs/design/layering.md` draws.
//! `tests/decode.rs`'s round-trip test compares against `SchemaMode::Strings`,
//! which is also decoded text, for exactly this reason; the on-disk-byte leg
//! is covered separately in `tests/scan.rs`.

use crate::copy::hex_val;

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
/// generator PostgreSQL's own float formatter also produces — confirmed
/// empirically against `postgres:16-alpine` with `extra_float_digits = 3`
/// (`docs/status/history/2026-08-23.md`). `exp` is the power of ten such that
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
/// (`e+06`, `e-05`, `e+100`), matching observed PostgreSQL output.
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
/// switches to scientific notation at, empirically confirmed (see
/// [`format_shortest`]'s docs).
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
fn civil_from_days(z: i64) -> (i64, u32, u32) {
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
/// decode failure by construction, not by accident — see "Failure and
/// diagnostics" in the phase doc) and for anything out of `Date32`'s `i32`
/// day range.
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
    let (y, m, d) = civil_from_days(i64::from(days));
    let (out_year, bc) = if y <= 0 { (1 - y, true) } else { (y, false) };
    let mut s = format!("{out_year:04}-{m:02}-{d:02}");
    if bc {
        s.push_str(" BC");
    }
    s
}

/// `HH:MM:SS[.ffffff]`, no offset. Returns whole seconds since midnight and
/// the microsecond remainder separately, since callers need both a plain
/// `Time64` value (`seconds*1_000_000 + micros`) and, for a timestamp, the
/// same pair added onto a day count.
fn parse_time_of_day(s: &str) -> Option<(i64, i64)> {
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

/// The exact inverse of [`parse_time_of_day`]'s micros-since-midnight value —
/// shared by [`render_time64_micros`] and [`render_timestamp_micros`]'s
/// time-of-day component. PostgreSQL trims trailing zeros from the fraction
/// (confirmed empirically: `00:00:00.5`, not `.500000` —
/// `docs/status/history/2026-08-23.md`) and omits it entirely when zero.
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

/// `None` for anything unparseable. `24:00:00` is a real, valid boundary
/// value (PostgreSQL's inclusive upper bound for `time`) and decodes to
/// exactly `86_400_000_000` with no special-casing needed.
pub fn decode_time64_micros(s: &str) -> Option<i64> {
    let (seconds, micros) = parse_time_of_day(s)?;
    Some(seconds * 1_000_000 + micros)
}

pub fn render_time64_micros(v: i64) -> String {
    format_hms_frac(v)
}

/// Pull a `+HH[:MM[:SS]]` / `-HH[:MM[:SS]]` UTC offset off the end of a
/// timestamptz's time-of-day text, returning what's left and the offset in
/// seconds (positive east of UTC, matching the sign convention `local - offset
/// = UTC`). The offset is the only place a `+`/`-` can occur in this
/// substring — the fractional-seconds part, if any, is digits and a `.` only.
fn extract_offset(s: &str) -> Option<(&str, i64)> {
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
    let utc_micros = local_micros - i128::from(offset_secs) * 1_000_000;
    i64::try_from(utc_micros).ok()
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

/// Canonical `8-4-4-4-12` hex form, case-insensitive on input (PostgreSQL
/// always dumps lowercase, but nothing forces that on a hand-edited fixture).
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

/// PostgreSQL's default `bytea_output = hex` form, `\x` followed by
/// lowercase hex pairs — the decoded text (a single backslash), not the
/// doubled-backslash form COPY escaping writes to disk (see the module
/// docs).
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

pub fn render_bytea(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("\\x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
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

    /// Ground truth captured live from `postgres:16-alpine`,
    /// `extra_float_digits = 3` (`docs/status/history/2026-08-23.md`) —
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

    #[test]
    fn time_boundary_and_fraction_trimming() {
        assert_eq!(decode_time64_micros("24:00:00"), Some(86_400_000_000));
        assert_eq!(render_time64_micros(86_400_000_000), "24:00:00");
        assert_eq!(decode_time64_micros("00:00:00.000001"), Some(1));
        assert_eq!(render_time64_micros(1), "00:00:00.000001");
        // PostgreSQL trims trailing zeros from the fraction rather than
        // always showing 6 digits -- confirmed live (2026-08-23).
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
    /// 2000. Confirmed by direct computation
    /// (`docs/status/history/2026-08-23.md`): the true representable ceiling
    /// is 294247-01-10, not PostgreSQL's 294276-12-31 -- so PostgreSQL's own
    /// maximum value is a genuine, expected `FieldDecode` overflow for this
    /// mapping, not a bug.
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
        // all -- not exercised by the fixtures (no negative-scale column
        // there), so pinned here from the type's own documented semantics.
        let unscaled = decimal_unscaled_digits("1200", -2).unwrap();
        assert_eq!(unscaled, "12");
        assert_eq!(render_decimal(&unscaled, -2), "1200");
    }
}
