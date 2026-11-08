//! The geometric types' input functions (I74).
//!
//! One walk answers whether a type's `*_in` reads a text, read or refused and
//! nothing between, fed it a byte at a time ([`GeometricWalk`]). Not
//! `decode`'s API; a strict parse reads by it.

mod walk;

pub(crate) use walk::GeometricWalk;

/// A point, as the geometric input functions read one.
#[derive(Debug, Clone, Copy, Default)]
struct Point {
    x: f64,
    y: f64,
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

#[cfg(test)]
mod differential;
#[cfg(test)]
mod prior_shape;
#[cfg(test)]
mod tests;
