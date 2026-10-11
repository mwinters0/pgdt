//! The geometric types' input functions (I74).
//!
//! Each answers whether its `*_in` reads `text`, read or refused and nothing
//! between, walking it as the C walks a NUL-terminated string: a byte past the
//! end reads as `0`, which no field holds. Not `decode`'s API; a strict parse
//! and a filter's literal call them.

use crate::value::libc::{c_at, is_c_space, strtod};

/// A point, as the geometric input functions read one.
#[derive(Debug, Clone, Copy, Default)]
struct Point {
    x: f64,
    y: f64,
}

/// `at` past any blanks.
fn skip_c_spaces(b: &[u8], at: usize) -> usize {
    at + b.get(at..).map_or(0, |rest| rest.iter().take_while(|&&c| is_c_space(c)).count())
}

/// `float8in_internal` asked for its stopping point, as `single_decode` asks
/// it: the blanks around one number skipped and the number read as
/// [`strtod`] reads it — `None` where nothing is read or the value lies past
/// `double precision`'s range, which it refuses (I74).
fn single_decode(b: &[u8], at: usize) -> Option<(f64, usize)> {
    let (value, end, out_of_range) = strtod(b, skip_c_spaces(b, at))?;
    (!out_of_range).then(|| (value, skip_c_spaces(b, end)))
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

#[cfg(test)]
mod tests;
