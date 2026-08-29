//! Post-parse row filtering (`docs/design/architecture.md`, "Predicates").

use crate::Result;
use crate::copy::{decode_field, split_fields};

/// Comparison operator for [`Predicate`]. Ordering operators are
/// deliberately excluded: on unparsed strings they'd be actively misleading
/// for numeric/date columns (`"9" < "10"` is false lexicographically), and
/// are deferred; typed/ordering predicates need pushdown to do them
/// correctly (`docs/design/roadmap-P5-pushdown.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicateOp {
    Eq,
    Ne,
    /// `col IS NULL` — matches only a NULL field (`\N`). Rounds out the NULL
    /// semantics `Eq`/`Ne` deliberately can't express (see [`Predicate::value`]).
    IsNull,
    /// `col IS NOT NULL` — matches every non-NULL field.
    IsNotNull,
}

/// A single-column post-parse filter: `column <op> value`. `column` is
/// matched against the queried table's column names (the `COPY` header list,
/// or the `column1`, `column2`, ... placeholders used when the header has
/// none). `value` is compared against each row's decoded (unescaped) field
/// as a plain string — not typed: filtering happens after a row is parsed
/// (typed/ordering predicates are
/// `docs/design/roadmap-P5-pushdown.md`). `value` is `None` for
/// `IsNull`/`IsNotNull`, which need
/// no comparison value; it is always `Some` for `Eq`/`Ne`.
///
/// A NULL field matches neither `Eq` nor `Ne` — SQL's own three-valued
/// logic collapses both to "excluded" — which is exactly why `IsNull`/
/// `IsNotNull` exist: without them there is no way to ask for a NULL
/// explicitly (`docs/status/history/2026-08-22.md`).
#[derive(Debug, Clone)]
pub struct Predicate {
    pub column: String,
    pub op: PredicateOp,
    pub value: Option<String>,
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
        Ok(match self.op {
            PredicateOp::IsNull => decoded.is_none(),
            PredicateOp::IsNotNull => decoded.is_some(),
            PredicateOp::Eq => decoded.is_some_and(|v| Some(v.as_ref()) == self.value.as_deref()),
            PredicateOp::Ne => decoded.is_some_and(|v| Some(v.as_ref()) != self.value.as_deref()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eq_matches_the_decoded_value() {
        let p = Predicate { column: "x".into(), op: PredicateOp::Eq, value: Some("a\tb".into()) };
        assert!(p.matches(b"other\ta\\tb", 1).unwrap());
        assert!(!p.matches(b"other\tc", 1).unwrap());
    }

    #[test]
    fn ne_matches_everything_but_the_decoded_value() {
        let p = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
        assert!(p.matches(b"other\tb", 1).unwrap());
        assert!(!p.matches(b"other\ta", 1).unwrap());
    }

    #[test]
    fn null_matches_neither_eq_nor_ne() {
        let eq = Predicate { column: "x".into(), op: PredicateOp::Eq, value: Some("a".into()) };
        let ne = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
        assert!(!eq.matches(b"other\t\\N", 1).unwrap());
        assert!(!ne.matches(b"other\t\\N", 1).unwrap());
    }

    #[test]
    fn is_null_and_is_not_null() {
        let is_null = Predicate { column: "x".into(), op: PredicateOp::IsNull, value: None };
        let is_not_null = Predicate { column: "x".into(), op: PredicateOp::IsNotNull, value: None };
        assert!(is_null.matches(b"other\t\\N", 1).unwrap());
        assert!(!is_null.matches(b"other\ta", 1).unwrap());
        assert!(!is_not_null.matches(b"other\t\\N", 1).unwrap());
        assert!(is_not_null.matches(b"other\ta", 1).unwrap());
    }

    #[test]
    fn missing_column_index_is_treated_as_null() {
        // Can't happen once a caller resolves `column_index` from the
        // block's own schema, but the fallback is still exercised here.
        let p = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
        assert!(!p.matches(b"onlyone", 5).unwrap());
    }
}
