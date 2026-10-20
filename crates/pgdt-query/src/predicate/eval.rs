//! A row's three-valued verdict: each resolved term, membership and tree
//! evaluated over one raw row, or over one value with no row around it, in
//! SQL's `True`/`False`/`Unknown` domain (`docs/design/decisions.md`, "D54").

use pgdt_codec::internal::{
    Equality, OrderKey, Side, compare_keys, compare_nested, nested_key, read_key,
};
#[cfg(feature = "introspect")]
use pgdt_core::internal::timed_span;
use pgdt_core::internal::{RawRow, RowSplit};
use pgdt_core::{Error, Result};
use pgdt_core::{instrument::EvaluationPart as Part, internal::timed};
use pgdt_model::PostgresInvalidValues;

use super::resolve::{Comparison, Lookup, ResolvedExpr, ResolvedMembership, ResolvedTerm};
use super::{PredicateOp, Truth};

impl ResolvedTerm {
    /// Evaluate this term against `raw_row`, in SQL's three-valued domain.
    /// `table` and `row_offset` are context for the error a field raises that
    /// does not decode as its mapped type under a comparison that reads it,
    /// `Error::FieldDecode`, worded exactly as the typed build path words it,
    /// and for the `Error::NullRefused` a NULL its column's `NOT NULL`
    /// refuses raises under every operator; a field that does not unescape
    /// raises the row's own error.
    ///
    /// A NULL field is [`Truth::Unknown`] under every comparing operator. The
    /// six that answer two-valued instead are `IsNull`/`IsNotNull` and the two
    /// unrepresentable tests, which compare nothing, and the two
    /// `IS DISTINCT FROM` forms, which count NULL as a value — so
    /// `IsDistinctFrom` on a NULL field is `True` where `Ne` is `Unknown`.
    pub(super) fn eval(
        &self,
        raw_row: RawRow<'_>,
        split: &mut RowSplit,
        table: &str,
        row_offset: u64,
    ) -> Result<Truth> {
        let field = timed!(Part::Locate, split.field(raw_row.bytes(), self.index));
        let located = field.is_some();
        let decoded = match field {
            Some(f) => timed!(Part::Unescape, raw_row.decode(f))?,
            None => None,
        };
        if located
            && decoded.is_none()
            && let Some(refused) =
                null_refused(self.not_null.as_deref(), self.invalid, table, row_offset)
        {
            return Err(refused);
        }
        self.eval_value(decoded.as_deref())
            .ok_or_else(|| self.field_decode(table, row_offset, decoded.as_deref()))
    }

    /// The `Error::FieldDecode` a comparing term raises over a `value` that
    /// is not of its column's type. A value of the type its column's Arrow
    /// type cannot hold keys in PostgreSQL's order, or is read as NULL, and
    /// never reaches here (`docs/design/decisions.md`, "D56", "D98").
    fn field_decode(&self, table: &str, row_offset: u64, value: Option<&str>) -> Error {
        let compared = self.compared.as_ref().expect("an operator taking no value decodes nothing");
        Error::FieldDecode {
            table: table.to_string(),
            column: compared.column.clone(),
            line_offset: row_offset,
            declared_type: compared.declared_type.clone(),
            value: value.unwrap_or_default().to_string(),
        }
    }

    /// `value`, or NULL where the query reads it as NULL.
    #[inline]
    fn as_read<'v>(&self, value: Option<&'v str>) -> Option<&'v str> {
        match &self.unrepresentable {
            Some(read) => value.filter(|text| !read.cannot_hold(text)),
            None => value,
        }
    }

    /// This term's answer for one **already unescaped** value of its column,
    /// `None` being SQL NULL — the row path's whole comparison, with no row
    /// in it. [`Self::eval`] is this after splitting and unescaping a field;
    /// a value that did not come from a row, such as a statistic's dictionary
    /// entry, is answered by the same code and so cannot be answered
    /// differently.
    ///
    /// `None` back is a non-NULL value that is not a value of the column's
    /// type under this term's comparison — what the row path reports as
    /// `Error::FieldDecode`, the only case in which it returns nothing. The
    /// operators taking no value and the two equalities comparing text never
    /// decode, so they always answer.
    ///
    /// **A value the query reads as NULL is answered as one**
    /// (`docs/design/decisions.md`, "D98") — but by the two unrepresentable
    /// tests, which ask of the text itself whether it is such a value.
    #[inline]
    pub(super) fn eval_value(&self, value: Option<&str>) -> Option<Truth> {
        let Some(compared) = self.compared.as_ref() else {
            return Some(Truth::of(match self.op {
                PredicateOp::IsNull => self.as_read(value).is_none(),
                PredicateOp::IsNotNull => self.as_read(value).is_some(),
                _ => {
                    let unrepresentable = value
                        .zip(self.tested.as_ref())
                        .is_some_and(|(text, test)| timed!(Part::Compare, test.cannot_hold(text)));
                    unrepresentable == matches!(self.op, PredicateOp::IsUnrepresentable)
                }
            }));
        };
        let value = self.as_read(value);
        let Some(text) = value else {
            return Some(match self.op {
                PredicateOp::IsDistinctFrom(_) => Truth::True,
                PredicateOp::IsNotDistinctFrom(_) => Truth::False,
                _ => Truth::Unknown,
            });
        };
        // The key and the comparison are timed apart, where the instrument
        // is built in (`pgdt_core::timed!`), each key dropped where
        // it was when it was a temporary of the comparison.
        Some(Truth::of(match &compared.comparison {
            Comparison::Ordered { kind, bound } => {
                let ord = {
                    let key = timed!(Part::Key, read_key(kind, text, self.invalid))?;
                    timed!(Part::Compare, compare_keys(&key, bound))
                };
                match self.op {
                    PredicateOp::Lt(_) => ord.is_lt(),
                    PredicateOp::Le(_) => ord.is_le(),
                    PredicateOp::Gt(_) => ord.is_gt(),
                    _ => ord.is_ge(),
                }
            }
            Comparison::Equal(Equality::Canonical(bound)) => {
                timed!(Part::Compare, bound.matches(text)) == self.wants_equal()
            }
            Comparison::Equal(Equality::Trimmed(bound)) => {
                timed!(Part::Compare, text.trim_end_matches(' ') == bound.as_str())
                    == self.wants_equal()
            }
            Comparison::Equal(Equality::Decoded { kind, bound }) => {
                let key = timed!(Part::Key, read_key(kind, text, self.invalid))?;
                timed!(Part::Compare, compare_keys(&key, bound)).is_eq() == self.wants_equal()
            }
            // Two-valued throughout: a NULL *inside* the container is a value
            // of it, and only the whole field being NULL is unknown — which
            // was decided above, before any of this runs.
            Comparison::Nested(nested) => {
                let ord = {
                    let key = timed!(
                        Part::Key,
                        nested_key(&nested.plan, text, Side::Field(self.invalid))
                    )?;
                    timed!(Part::Compare, compare_nested(&key, &nested.bound))
                };
                match self.op {
                    PredicateOp::Lt(_) => ord.is_lt(),
                    PredicateOp::Le(_) => ord.is_le(),
                    PredicateOp::Gt(_) => ord.is_gt(),
                    PredicateOp::Ge(_) => ord.is_ge(),
                    _ => ord.is_eq() == self.wants_equal(),
                }
            }
        }))
    }

    /// Whether `raw_row` makes this term [`Truth::False`] — answered where
    /// [`ResolvedExpr::matches`] may never have evaluated the term, so **it
    /// raises nothing**: a field that does not unescape or decode answers
    /// `false`, as a NULL or a missing field does, and the error stays where
    /// evaluation reaches it (`docs/design/decisions.md`, "D54").
    pub(crate) fn is_false(&self, raw_row: RawRow<'_>, split: &mut RowSplit) -> bool {
        let Some(field) = split.field(raw_row.bytes(), self.index) else { return false };
        let Ok(Some(value)) = raw_row.decode(field) else { return false };
        self.eval_value(Some(&value)) == Some(Truth::False)
    }

    /// Whether this term keeps the rows its comparison called equal — `Eq`
    /// and `IsNotDistinctFrom` do, `Ne` and `IsDistinctFrom` do not, and no
    /// other operator reaches it.
    pub(super) fn wants_equal(&self) -> bool {
        matches!(self.op, PredicateOp::Eq(_) | PredicateOp::IsNotDistinctFrom(_))
    }
}

impl ResolvedMembership {
    /// What the membership answers where no value matched: `Unknown` if the
    /// list holds a NULL, else `False`.
    fn unmatched(&self) -> Truth {
        if self.null { Truth::Unknown } else { Truth::False }
    }

    /// The membership's answer for one already unescaped value, `None`
    /// being SQL NULL — [`ResolvedTerm::eval_value`]'s counterpart, `None`
    /// back where the value is not of the column's type.
    pub(super) fn eval_value(&self, value: Option<&str>) -> Option<Truth> {
        if self.terms.is_empty() {
            return Some(self.unmatched());
        }
        let value = match &self.unrepresentable {
            Some(read) => value.filter(|text| !read.cannot_hold(text)),
            None => value,
        };
        let Some(text) = value else { return Some(Truth::Unknown) };
        Some(if self.lookup.contains(text, &self.terms, self.invalid)? {
            Truth::True
        } else {
            self.unmatched()
        })
    }

    /// [`Self::eval_value`] over `raw_row`'s field, raising what the first
    /// `=` term would: the row's own error where the field does not unescape,
    /// `Error::FieldDecode` where it does not decode. A membership with no
    /// value to compare reads no field.
    fn eval(
        &self,
        raw_row: RawRow<'_>,
        split: &mut RowSplit,
        table: &str,
        row_offset: u64,
    ) -> Result<Truth> {
        let Some(first) = self.terms.first() else { return Ok(self.unmatched()) };
        let field = timed!(Part::Locate, split.field(raw_row.bytes(), self.index));
        let located = field.is_some();
        let decoded = match field {
            Some(field) => timed!(Part::Unescape, raw_row.decode(field))?,
            None => None,
        };
        if located
            && decoded.is_none()
            && let Some(refused) =
                null_refused(self.not_null.as_deref(), self.invalid, table, row_offset)
        {
            return Err(refused);
        }
        self.eval_value(decoded.as_deref())
            .ok_or_else(|| first.field_decode(table, row_offset, decoded.as_deref()))
    }

    /// Whether every row this membership makes `True` makes `term` `True`
    /// too: `term` is one of the four ordering operators over the same
    /// column, and every value in the list meets it as a row equal to that
    /// value would. Answered for the two lookups whose equality settles what
    /// an ordering term reads — the text the file spells, or a key in the
    /// ordering term's own kind — and `false` for any other.
    fn implies(&self, term: &ResolvedTerm) -> bool {
        if term.index != self.index || !term.op.is_ordering() || self.terms.is_empty() {
            return false;
        }
        let Some(Comparison::Ordered { kind: ordered, bound }) =
            term.compared.as_ref().map(|c| &c.comparison)
        else {
            return false;
        };
        let meets = |key: &OrderKey| {
            let ord = compare_keys(key, bound);
            match term.op {
                PredicateOp::Lt(_) => ord.is_lt(),
                PredicateOp::Le(_) => ord.is_le(),
                PredicateOp::Gt(_) => ord.is_gt(),
                _ => ord.is_ge(),
            }
        };
        match &self.lookup {
            Lookup::Canonical(_) => self.terms.iter().all(|value| {
                matches!(
                    value.compared.as_ref().map(|c| &c.comparison),
                    Some(Comparison::Equal(Equality::Canonical(spellings)))
                        if term.eval_value(Some(spellings.first())) == Some(Truth::True)
                )
            }),
            Lookup::Decoded { kind, keys } => kind == ordered && keys.iter().all(meets),
            // So a join over `character(n)` keys keeps its bounds per row:
            // speed, not rows.
            Lookup::Trimmed(_) | Lookup::Nested { .. } | Lookup::Each => false,
        }
    }
}

impl ResolvedExpr {
    /// Whether `raw_row` survives this filter: its root evaluates
    /// [`Truth::True`]. `Unknown` and `False` both drop the row, which is
    /// what makes the collapse at the root sound though it is not sound under
    /// a `Not` (`docs/design/decisions.md`, "D54").
    pub(crate) fn matches(
        &self,
        raw_row: RawRow<'_>,
        split: &mut RowSplit,
        table: &str,
        row_offset: u64,
    ) -> Result<bool> {
        Ok(self.eval(false, raw_row, split, table, row_offset)?.is_true())
    }

    /// Kleene evaluation, left to right, stopping as soon as **the root's**
    /// value is determined.
    ///
    /// `exact` is what makes that "the root's" rather than "this node's". A
    /// caller that cannot tell `False` from `Unknown` — [`Self::matches`], and
    /// any `And`/`Or` whose own caller cannot — lets this node return `False`
    /// for an unknown and stop at the first non-`True` conjunct. Only
    /// [`Self::Not`] distinguishes them, so it is the one node that evaluates
    /// its child exactly, and everything beneath a `Not` is exact too.
    ///
    /// A decode failure is therefore raised only where evaluation reaches it:
    /// which rows error depends on where the term sits in the tree, and on
    /// whether a `Not` sits above it (`docs/design/decisions.md`, "D54").
    pub(super) fn eval(
        &self,
        exact: bool,
        raw_row: RawRow<'_>,
        split: &mut RowSplit,
        table: &str,
        row_offset: u64,
    ) -> Result<Truth> {
        Ok(match self {
            Self::Term(term) => term.eval(raw_row, split, table, row_offset)?,
            Self::In(membership) => membership.eval(raw_row, split, table, row_offset)?,
            Self::And(children) => {
                let mut unknown = false;
                for child in children {
                    match child.eval(exact, raw_row, split, table, row_offset)? {
                        Truth::True => {}
                        Truth::False => return Ok(Truth::False),
                        Truth::Unknown if exact => unknown = true,
                        Truth::Unknown => return Ok(Truth::False),
                    }
                }
                if unknown { Truth::Unknown } else { Truth::True }
            }
            Self::Or(children) => {
                let mut unknown = false;
                for child in children {
                    match child.eval(exact, raw_row, split, table, row_offset)? {
                        Truth::True => return Ok(Truth::True),
                        Truth::Unknown if exact => unknown = true,
                        Truth::False | Truth::Unknown => {}
                    }
                }
                if unknown { Truth::Unknown } else { Truth::False }
            }
            Self::Not(inner) => inner.eval(true, raw_row, split, table, row_offset)?.not(),
        })
    }

    /// This tree as a row is evaluated against it where only whether the root
    /// is `True` decides anything and a failing leaf keeps the row — a
    /// dynamic filter's ([`crate::stream::DynamicFilter`]) — **with every
    /// ordering term a membership in the same conjunction implies dropped**
    /// ([`ResolvedMembership::implies`]): a join's filter bounds its keys and
    /// lists them, and each bound reads, unescapes and keys a field the list
    /// answers alone (`docs/design/decisions.md`, "D54" and "D93").
    ///
    /// **Only where the evaluation is not exact**, reached from the root
    /// through `And` and `Or` alone: there a conjunction is `True` exactly
    /// where each conjunct is, and a membership `True` makes each term it
    /// implies `True`, so the root is `True` on the same rows. Beneath a
    /// `Not`, where a conjunction's `Unknown` and `False` are different
    /// answers, the subtree stands as it is. A conjunction nested in another
    /// is read as one, and one left with a single conjunct is that conjunct.
    ///
    /// A row the dropped term would have failed to key — a value that is
    /// not of its column's type, which a file's `*_out` text never is — is
    /// answered by the membership instead, as any row a leaf earlier in the
    /// tree rules out is (`docs/design/decisions.md`, "D54").
    pub(crate) fn for_rows(&self) -> ResolvedExpr {
        match self {
            Self::Term(_) | Self::In(_) | Self::Not(_) => self.clone(),
            Self::Or(children) => Self::Or(children.iter().map(Self::for_rows).collect()),
            Self::And(_) => {
                let mut conjuncts = Vec::new();
                self.conjuncts(&mut conjuncts);
                let memberships: Vec<&ResolvedMembership> = conjuncts
                    .iter()
                    .filter_map(|c| match c {
                        Self::In(membership) => Some(membership),
                        _ => None,
                    })
                    .collect();
                let mut kept: Vec<ResolvedExpr> = conjuncts
                    .into_iter()
                    .filter(|c| match c {
                        Self::Term(term) => !memberships.iter().any(|m| m.implies(term)),
                        _ => true,
                    })
                    .map(Self::for_rows)
                    .collect();
                if kept.len() == 1 { kept.pop().expect("one conjunct") } else { Self::And(kept) }
            }
        }
    }

    /// Whether evaluating this tree reads a field at all. An empty
    /// conjunction — the `Expr::And(vec![])` a query with no filter carries —
    /// reads nothing, and a stream whose projection is also empty therefore
    /// decodes nothing and skips the bulk UTF-8 validation
    /// (`docs/design/decisions.md`, "D27").
    pub(crate) fn reads_fields(&self) -> bool {
        match self {
            Self::Term(_) => true,
            Self::In(membership) => !membership.terms.is_empty(),
            Self::And(children) | Self::Or(children) => children.iter().any(Self::reads_fields),
            Self::Not(inner) => inner.reads_fields(),
        }
    }
}

impl Lookup {
    /// Whether `text` equals one of the values, `None` where it is not a
    /// value of the column's type — which the first `=` term would have
    /// raised on.
    fn contains(
        &self,
        text: &str,
        terms: &[ResolvedTerm],
        invalid: PostgresInvalidValues,
    ) -> Option<bool> {
        Some(match self {
            Self::Canonical(values) => timed!(Part::Lookup, values.contains(text)),
            Self::Trimmed(values) => {
                timed!(Part::Lookup, values.contains(text.trim_end_matches(' ')))
            }
            Self::Decoded { kind, keys } => {
                let key = timed!(Part::Key, read_key(kind, text, invalid))?;
                timed!(Part::Lookup, keys.binary_search_by(|k| compare_keys(k, &key)).is_ok())
            }
            Self::Nested { plan, keys } => {
                let key = timed!(Part::Key, nested_key(plan, text, Side::Field(invalid)))?;
                timed!(Part::Lookup, keys.binary_search_by(|k| compare_nested(k, &key)).is_ok())
            }
            Self::Each => {
                for term in terms {
                    if term.eval_value(Some(text))? == Truth::True {
                        return Some(true);
                    }
                }
                false
            }
        })
    }
}

/// The [`Error::NullRefused`] a term or a membership raises over a NULL field
/// of `column`, where it is read under `invalid`, or `None`.
fn null_refused(
    not_null: Option<&str>,
    invalid: PostgresInvalidValues,
    table: &str,
    row_offset: u64,
) -> Option<Error> {
    let column = not_null.filter(|_| invalid != PostgresInvalidValues::Ignore)?;
    Some(Error::NullRefused {
        table: table.to_string(),
        column: column.to_string(),
        line: None,
        line_offset: row_offset,
    })
}

#[cfg(test)]
mod tests;
