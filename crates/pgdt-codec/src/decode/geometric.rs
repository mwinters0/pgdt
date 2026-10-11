//! The geometric types' input functions (I74).
//!
//! Each answers whether its `*_in` reads `text`, read or refused and nothing
//! between, walking it as the C walks a NUL-terminated string: a byte past the
//! end reads as `0`, which no field holds. Not `decode`'s API; a strict parse
//! and a filter's literal call them.

use super::is_c_space;

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
pub(super) const MAX_ALLOC_SIZE: usize = 0x3fff_ffff;

/// `offsetof(PATH, p)` and `offsetof(POLYGON, p)`: the bytes ahead of the
/// sixteen each point takes.
pub(super) const PATH_HEADER: usize = 16;
pub(super) const POLYGON_HEADER: usize = 40;

/// Whether `npts` points after `header` fit one allocation — what `path_in`
/// and `poly_in` ask before reading a point, their own overflow check
/// refusing only counts past this already.
pub(super) fn fits_allocation(header: usize, npts: usize) -> bool {
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
