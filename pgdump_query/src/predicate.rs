//! Post-parse row filtering (`docs/design/mvp.md`, "Predicate filtering").

use crate::Result;
use crate::copy::{decode_field, split_fields};

/// Comparison operator for [`Predicate`]. Ordering operators are
/// deliberately excluded: on unparsed strings they'd be actively misleading
/// for numeric/date columns (`"9" < "10"` is false lexicographically), and
/// are deferred until Phase 2 typed columns can do them correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicateOp {
    Eq,
    Ne,
}

/// A single-column post-parse filter: `column <op> value`. `column` is
/// matched against the queried table's column names (the `COPY` header list,
/// or the `column1`, `column2`, ... placeholders used when the header has
/// none). `value` is compared against each row's decoded (unescaped) field
/// as a plain string — not typed, since Phase 2 typed columns don't exist
/// yet. A NULL field never matches either operator: this MVP has no `IS
/// [NOT] NULL` predicate, so both `=` and `!=` collapse SQL's three-valued
/// NULL comparison to "excluded" rather than guessing which reading a caller
/// wants (`docs/status/history/2026-08-22.md`).
#[derive(Debug, Clone)]
pub struct Predicate {
    pub column: String,
    pub op: PredicateOp,
    pub value: String,
}

impl Predicate {
    /// Evaluate this predicate against `raw_row`'s field at `column_index`
    /// (already resolved from this block's schema by the caller).
    pub(crate) fn matches(&self, raw_row: &[u8], column_index: usize) -> Result<bool> {
        let field = split_fields(raw_row).nth(column_index);
        let decoded = match field {
            Some(f) => decode_field(f)?,
            None => None,
        };
        Ok(match decoded {
            None => false,
            Some(v) => match self.op {
                PredicateOp::Eq => v == self.value.as_str(),
                PredicateOp::Ne => v != self.value.as_str(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eq_matches_the_decoded_value() {
        let p = Predicate { column: "x".into(), op: PredicateOp::Eq, value: "a\tb".into() };
        assert!(p.matches(b"other\ta\\tb", 1).unwrap());
        assert!(!p.matches(b"other\tc", 1).unwrap());
    }

    #[test]
    fn ne_matches_everything_but_the_decoded_value() {
        let p = Predicate { column: "x".into(), op: PredicateOp::Ne, value: "a".into() };
        assert!(p.matches(b"other\tb", 1).unwrap());
        assert!(!p.matches(b"other\ta", 1).unwrap());
    }

    #[test]
    fn null_matches_neither_operator() {
        let eq = Predicate { column: "x".into(), op: PredicateOp::Eq, value: "a".into() };
        let ne = Predicate { column: "x".into(), op: PredicateOp::Ne, value: "a".into() };
        assert!(!eq.matches(b"other\t\\N", 1).unwrap());
        assert!(!ne.matches(b"other\t\\N", 1).unwrap());
    }

    #[test]
    fn missing_column_index_is_treated_as_null() {
        // Can't happen once a caller resolves `column_index` from the
        // block's own schema, but the fallback is still exercised here.
        let p = Predicate { column: "x".into(), op: PredicateOp::Ne, value: "a".into() };
        assert!(!p.matches(b"onlyone", 5).unwrap());
    }
}
