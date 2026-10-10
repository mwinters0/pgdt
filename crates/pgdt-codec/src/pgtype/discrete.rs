//! What a crate above calls on [`Discrete`] that is not the register's API,
//! so `internal` re-exports it.

use super::Discrete;

/// What a crate above calls on [`Discrete`] that is not its API: public only
/// because a crate boundary forced it, and reached through `internal`.
pub trait DiscreteExt {
    fn largest(self) -> i64;
}

impl DiscreteExt for Discrete {
    /// The subtype's largest value, in the key a bound of it is read into:
    /// a bound here has no successor, and the canonical function raises
    /// `integer`, `bigint` or `date out of range` on it (I46). A leaf's key is
    /// an `i64` whatever the column's width, so the width is stated here
    /// rather than found in the key. A literal's bound past the width itself,
    /// `int4range '[1,3000000000)'`, is refused before any successor is taken,
    /// as `int4in` refuses it (I60); a field's is read as written.
    ///
    /// A date's is `5874897-12-31` as days since 1970, the day before
    /// `IS_VALID_DATE`'s exclusive `DATE_END_JULIAN`.
    fn largest(self) -> i64 {
        match self {
            Discrete::Int4 => i64::from(i32::MAX),
            Discrete::Int8 => i64::MAX,
            Discrete::Date => 2_145_042_905,
        }
    }
}
