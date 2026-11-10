//! A row's three-valued verdict: each resolved term, membership and tree
//! evaluated over one raw row, or over one value with no row around it, in
//! SQL's `True`/`False`/`Unknown` domain (`docs/design/decisions.md`, "D54").

use std::borrow::Cow;
use std::cmp::Ordering;
use std::ops::Range;

use pgdt_codec::compare::CompareKind;
use pgdt_codec::internal::{
    BytewiseRuns, Equality, FieldOrder, FieldText, OrderKey, Side, SpellingRuns, TextFold,
    compare_keys, compare_nested, jsonb_order, nested_field_order, nested_key, read_key,
};
#[cfg(feature = "introspect")]
use pgdt_core::internal::timed_span;
use pgdt_core::internal::{RawField, RawRow, RowSplit, Unescape, printout, printout_head};
use pgdt_core::{Error, Result};
use pgdt_core::{instrument::EvaluationPart as Part, internal::timed};
use pgdt_model::PostgresInvalidValues;

use super::resolve::{
    Comparison, Lookup, NestedComparison, ResolvedExpr, ResolvedMembership, ResolvedTerm, Texts,
};
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
    /// A NULL field is [`Truth::Unknown`] under every operator but six. The
    /// six that answer two-valued are `IsNull`/`IsNotNull` and the two
    /// unrepresentable tests, which compare nothing, and the two
    /// `IS DISTINCT FROM` forms, which count NULL as a value — so
    /// `IsDistinctFrom` on a NULL field is `True` where `Ne` is `Unknown`.
    ///
    /// **An escaped field is read off its bytes where the term reads no
    /// more of it than a bound** — a text, `character` or `bytea` ordering,
    /// an `=` against a literal's spellings or trimmed, a NULL test — with
    /// no copy of it made — where it orders a nested field, read as the
    /// output grammars' walk reads it, and where it orders or equates a
    /// `jsonb` field, by its pairs' positions; any other is unescaped first
    /// (`docs/design/decisions.md`, "D150", "D151", "D152").
    pub(super) fn eval(
        &self,
        raw_row: RawRow<'_>,
        split: &mut RowSplit,
        table: &str,
        row_offset: u64,
    ) -> Result<Truth> {
        let field = timed!(Part::Locate, split.field(raw_row.bytes(), self.index));
        let runs = self.runs();
        let walks = self.walks();
        let jsonb = self.jsonb();
        let escaped = runs.is_some() || walks.is_some() || jsonb.is_some();
        let read = match field {
            Some(f) => timed!(Part::Unescape, read(raw_row, f, escaped))?,
            None => Read::Missing,
        };
        let value = match &read {
            Read::Text(text) => Some(text.as_ref()),
            Read::Escaped(bytes) => {
                let truth = match (runs, walks, jsonb) {
                    (Some(runs), ..) => self.eval_runs(runs, bytes)?,
                    (None, Some(nested), _) => self.eval_walk(nested, bytes)?,
                    (None, None, Some(bound)) => self.eval_jsonb(bound, bytes)?,
                    (None, None, None) => {
                        unreachable!("a field is left escaped only for a term reading its runs")
                    }
                };
                return truth
                    .ok_or_else(|| self.field_decode(table, row_offset, printout_head(bytes)));
            }
            Read::Null => {
                if let Some(refused) =
                    null_refused(self.not_null.as_deref(), self.invalid, table, row_offset)
                {
                    return Err(refused);
                }
                None
            }
            Read::Missing => None,
        };
        self.eval_value(value)
            .ok_or_else(|| self.field_decode(table, row_offset, value.map(printout)))
    }

    /// The `Error::FieldDecode` a comparing term raises over a value that
    /// is not of its column's type, `quoted` as the typed build path quotes
    /// it. A value of the type its column's Arrow type cannot hold keys in
    /// PostgreSQL's order, or is read as NULL, and never reaches here (`docs/design/decisions.md`, "D56", "D98").
    fn field_decode(&self, table: &str, row_offset: u64, quoted: Option<String>) -> Error {
        let compared = self.compared.as_ref().expect("an operator taking no value decodes nothing");
        Error::FieldDecode {
            table: table.to_string(),
            column: compared.column.clone(),
            line_offset: row_offset,
            declared_type: compared.declared_type.clone(),
            value: quoted.unwrap_or_default(),
        }
    }

    /// What this term reads of a field's unescaped text a run at a time,
    /// holding no more of it than its bound, or `None` where it reads the
    /// text otherwise: a nested value, walked ([`Self::walks`]), and whole a
    /// key of another kind and a value its type cannot hold tested or read
    /// as NULL.
    fn runs(&self) -> Option<TermRuns<'_>> {
        if self.unrepresentable.is_some() {
            return None;
        }
        let Some(compared) = &self.compared else {
            return self.tested.is_none().then_some(TermRuns::Nothing);
        };
        match &compared.comparison {
            Comparison::Ordered { kind, bound } => {
                FieldOrder::new(kind, bound).map(TermRuns::Order)
            }
            Comparison::Equal(Equality::Canonical(spellings)) => {
                Some(TermRuns::Spellings(spellings.runs()))
            }
            Comparison::Equal(Equality::Trimmed(bound)) => {
                Some(TermRuns::Trimmed(BytewiseRuns::trimmed(bound.as_bytes())))
            }
            Comparison::Equal(Equality::Decoded { .. }) | Comparison::Nested(_) => None,
        }
    }

    /// The nested comparison this term orders a field by as the output
    /// grammars' walk reads it, off an escaped field's bytes, or `None`:
    /// a value its type cannot hold, tested or read as NULL, is read whole.
    fn walks(&self) -> Option<&NestedComparison> {
        if self.unrepresentable.is_some() {
            return None;
        }
        match &self.compared.as_ref()?.comparison {
            Comparison::Nested(nested) => Some(nested),
            _ => None,
        }
    }

    /// The `jsonb` key this term orders or equates a field against by its
    /// pairs' positions, off an escaped field's bytes, or `None`: a value
    /// its type cannot hold, tested or read as NULL, is read whole.
    fn jsonb(&self) -> Option<&OrderKey> {
        if self.unrepresentable.is_some() {
            return None;
        }
        match &self.compared.as_ref()?.comparison {
            Comparison::Ordered { kind: CompareKind::Jsonb, bound }
            | Comparison::Equal(Equality::Decoded { kind: CompareKind::Jsonb, bound }) => {
                Some(bound)
            }
            _ => None,
        }
    }

    /// This term's answer for an escaped `jsonb` field, `bytes` as `COPY`
    /// wrote them, read off them by its pairs' positions:
    /// [`Self::eval_value`]'s over its text, and the error unescaping it
    /// raises where that text is not UTF-8.
    fn eval_jsonb(&self, bound: &OrderKey, bytes: &[u8]) -> Result<Option<Truth>> {
        utf8_checked(bytes)?;
        Ok(timed!(Part::Compare, jsonb_order(FieldText::escaped(bytes), Some(bound)))
            .map(|ord| self.nested(ord)))
    }

    /// This term's answer for an escaped nested field, `bytes` as `COPY`
    /// wrote them, walked off them: [`Self::eval_value`]'s over its text,
    /// and the error unescaping it raises where that text is not UTF-8.
    fn eval_walk(&self, nested: &NestedComparison, bytes: &[u8]) -> Result<Option<Truth>> {
        utf8_checked(bytes)?;
        Ok(timed!(
            Part::Compare,
            nested_field_order(
                &nested.plan,
                FieldText::escaped(bytes),
                &nested.bound,
                self.invalid
            )
        )
        .map(|ord| self.nested(ord)))
    }

    /// The truth of this nested or `jsonb` term's operator where the field
    /// orders `ord` against its bound: two-valued throughout, a NULL
    /// *inside* the container being a value of it.
    fn nested(&self, ord: Ordering) -> Truth {
        Truth::of(match self.op {
            PredicateOp::Lt(_) => ord.is_lt(),
            PredicateOp::Le(_) => ord.is_le(),
            PredicateOp::Gt(_) => ord.is_gt(),
            PredicateOp::Ge(_) => ord.is_ge(),
            _ => ord.is_eq() == self.wants_equal(),
        })
    }

    /// This term's answer for an escaped field, `bytes` as `COPY` wrote
    /// them, read off them by `runs`: [`Self::eval_value`]'s over its text,
    /// and the error unescaping it raises where that text is not UTF-8.
    fn eval_runs(&self, mut runs: TermRuns<'_>, bytes: &[u8]) -> Result<Option<Truth>> {
        timed!(Part::Compare, {
            let mut utf8 = TextFold::default();
            unescape_runs(bytes, |run| {
                utf8.push(run);
                runs.push(run);
            });
            let valid_up_to = utf8.valid_up_to();
            if utf8.finish().is_none() {
                return Err(Error::InvalidUtf8 { valid_up_to });
            }
            Ok(match runs {
                // A non-NULL field, of which a test reading no text reads
                // nothing: the empty text answers as any would.
                TermRuns::Nothing => self.eval_value(Some("")),
                TermRuns::Order(order) => order.finish().map(|ord| self.ordered(ord)),
                TermRuns::Spellings(spellings) => Some(self.equal(spellings.finish())),
                TermRuns::Trimmed(trimmed) => Some(self.equal(trimmed.finish().is_eq())),
            })
        })
    }

    /// The truth of this ordering term's operator where the field orders
    /// `ord` against its bound.
    fn ordered(&self, ord: Ordering) -> Truth {
        Truth::of(match self.op {
            PredicateOp::Lt(_) => ord.is_lt(),
            PredicateOp::Le(_) => ord.is_le(),
            PredicateOp::Gt(_) => ord.is_gt(),
            _ => ord.is_ge(),
        })
    }

    /// The truth of this equality term's operator where the field is its
    /// literal or not.
    fn equal(&self, equal: bool) -> Truth {
        Truth::of(equal == self.wants_equal())
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
                // A bytewise kind's field is ordered with no key of it
                // built, and a `jsonb`'s by its pairs' positions (D152).
                let ord = match FieldOrder::new(kind, bound) {
                    Some(order) => timed!(Part::Compare, order.of(text))?,
                    None if *kind == CompareKind::Jsonb => {
                        timed!(Part::Compare, jsonb_order(FieldText::plain(text), Some(bound)))?
                    }
                    None => {
                        let key = timed!(Part::Key, read_key(kind, text, self.invalid))?;
                        timed!(Part::Compare, compare_keys(&key, bound))
                    }
                };
                return Some(self.ordered(ord));
            }
            Comparison::Equal(Equality::Canonical(bound)) => {
                timed!(Part::Compare, bound.matches(text)) == self.wants_equal()
            }
            Comparison::Equal(Equality::Trimmed(bound)) => {
                timed!(Part::Compare, text.trim_end_matches(' ') == bound.as_str())
                    == self.wants_equal()
            }
            Comparison::Equal(Equality::Decoded { kind: CompareKind::Jsonb, bound }) => {
                timed!(Part::Compare, jsonb_order(FieldText::plain(text), Some(bound)))?.is_eq()
                    == self.wants_equal()
            }
            Comparison::Equal(Equality::Decoded { kind, bound }) => {
                let key = timed!(Part::Key, read_key(kind, text, self.invalid))?;
                timed!(Part::Compare, compare_keys(&key, bound)).is_eq() == self.wants_equal()
            }
            // Only the whole field being NULL is unknown, which was decided
            // above, before any of this runs; read as the output grammars'
            // walk reads it, with no key of it built (D151).
            Comparison::Nested(nested) => {
                let ord = timed!(
                    Part::Compare,
                    nested_field_order(
                        &nested.plan,
                        FieldText::plain(text),
                        &nested.bound,
                        self.invalid
                    )
                )?;
                return Some(self.nested(ord));
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
        let runs = self.runs();
        let walks = self.walks();
        let jsonb = self.jsonb();
        match read(raw_row, field, runs.is_some() || walks.is_some() || jsonb.is_some()) {
            Ok(Read::Text(value)) => self.eval_value(Some(&value)) == Some(Truth::False),
            Ok(Read::Escaped(bytes)) => {
                let truth = match (runs, walks, jsonb) {
                    (Some(runs), ..) => self.eval_runs(runs, bytes),
                    (None, Some(nested), _) => self.eval_walk(nested, bytes),
                    (None, None, Some(bound)) => self.eval_jsonb(bound, bytes),
                    (None, None, None) => return false,
                };
                matches!(truth, Ok(Some(Truth::False)))
            }
            Ok(Read::Null | Read::Missing) | Err(_) => false,
        }
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
    ///
    /// An escaped field is read off its bytes where the lookup probes for a
    /// literal's text, collected no further than its longest literal
    /// (`docs/design/decisions.md`, "D150").
    fn eval(
        &self,
        raw_row: RawRow<'_>,
        split: &mut RowSplit,
        table: &str,
        row_offset: u64,
    ) -> Result<Truth> {
        let Some(first) = self.terms.first() else { return Ok(self.unmatched()) };
        let field = timed!(Part::Locate, split.field(raw_row.bytes(), self.index));
        let texts = self.texts();
        let read = match field {
            Some(field) => timed!(Part::Unescape, read(raw_row, field, texts.is_some()))?,
            None => Read::Missing,
        };
        let value = match &read {
            Read::Text(text) => Some(text.as_ref()),
            Read::Escaped(bytes) => {
                let (texts, trimmed) =
                    texts.expect("a field is left escaped only for a lookup reading its runs");
                return self.eval_runs(texts, trimmed, bytes);
            }
            Read::Null => {
                if let Some(refused) =
                    null_refused(self.not_null.as_deref(), self.invalid, table, row_offset)
                {
                    return Err(refused);
                }
                None
            }
            Read::Missing => None,
        };
        self.eval_value(value)
            .ok_or_else(|| first.field_decode(table, row_offset, value.map(printout)))
    }

    /// The literals this membership's lookup probes a field's text for, and
    /// whether it trims the text's trailing blanks first, or `None` where it
    /// keys the text or reads it through its terms.
    fn texts(&self) -> Option<(&Texts, bool)> {
        if self.unrepresentable.is_some() {
            return None;
        }
        match &self.lookup {
            Lookup::Canonical(texts) => Some((texts, false)),
            Lookup::Trimmed(texts) => Some((texts, true)),
            Lookup::Decoded { .. } | Lookup::Nested { .. } | Lookup::Each => None,
        }
    }

    /// This membership's answer for an escaped field, `bytes` as `COPY`
    /// wrote them, its text collected no further than `texts`' longest,
    /// which no longer text equals: [`Self::eval_value`]'s over its text,
    /// and the error unescaping it raises where that text is not UTF-8.
    fn eval_runs(&self, texts: &Texts, trimmed: bool, bytes: &[u8]) -> Result<Truth> {
        timed!(Part::Lookup, {
            let mut utf8 = TextFold::default();
            let mut collected = Collected::new(texts.longest(), trimmed);
            unescape_runs(bytes, |run| {
                utf8.push(run);
                collected.push(run);
            });
            let valid_up_to = utf8.valid_up_to();
            if utf8.finish().is_none() {
                return Err(Error::InvalidUtf8 { valid_up_to });
            }
            let found = collected.text().is_some_and(|text| {
                let text = std::str::from_utf8(text).expect("a text checked UTF-8 whole");
                texts.contains(text)
            });
            Ok(if found { Truth::True } else { self.unmatched() })
        })
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

    /// [`Self::matches`] of a row this tree reads no field of
    /// ([`Self::reads_fields`]), which a replay counts without holding its
    /// bytes: the answer is the tree's alone, the same for every row.
    pub(crate) fn matches_unread(&self, table: &str, row_offset: u64) -> Result<bool> {
        debug_assert!(!self.reads_fields(), "a filter reading a field was asked of no row");
        self.matches(RawRow::unchecked(&[]), &mut RowSplit::default(), table, row_offset)
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

/// A located field as a term or a membership reads it.
enum Read<'a> {
    /// The row has no such field.
    Missing,
    /// SQL `NULL`.
    Null,
    /// Its text, unescaped.
    Text(Cow<'a, str>),
    /// An escaped field as `COPY` wrote it, left for a reader taking its
    /// text a run at a time.
    Escaped(&'a [u8]),
}

/// The field at `field` of `raw_row`, an escaped one left as `COPY` wrote
/// it where its reader reads its `runs`, and unescaped otherwise.
fn read(raw_row: RawRow<'_>, field: Range<usize>, runs: bool) -> Result<Read<'_>> {
    if !runs {
        return Ok(match raw_row.decode(field)? {
            Some(text) => Read::Text(text),
            None => Read::Null,
        });
    }
    Ok(match raw_row.read(field)? {
        RawField::Null => Read::Null,
        RawField::Plain(text) => Read::Text(Cow::Borrowed(text)),
        RawField::Escaped(bytes) => Read::Escaped(bytes),
    })
}

/// The error unescaping `bytes`, a field as `COPY` wrote it, raises where
/// its text is not UTF-8, read off them with no copy made.
fn utf8_checked(bytes: &[u8]) -> Result<()> {
    let mut utf8 = TextFold::default();
    unescape_runs(bytes, |run| utf8.push(run));
    let valid_up_to = utf8.valid_up_to();
    match utf8.finish() {
        Some(_) => Ok(()),
        None => Err(Error::InvalidUtf8 { valid_up_to }),
    }
}

/// An escaped field's text, `bytes` unescaped, handed to `run` a stretch
/// at a time: a stretch no escape spells as a slice of `bytes`, so none of
/// it is copied.
fn unescape_runs(bytes: &[u8], mut run: impl FnMut(&[u8])) {
    let mut unescape = Unescape::default();
    unescape.run(bytes, &mut run);
    unescape.finish(&mut run);
}

/// What a term reads of a field's unescaped text a run at a time
/// ([`ResolvedTerm::runs`]).
enum TermRuns<'t> {
    /// A NULL test reading no text.
    Nothing,
    /// A text, `character` or `bytea` field ordered against its bound.
    Order(FieldOrder<'t>),
    /// A field against a literal's `*_out` spellings.
    Spellings(SpellingRuns<'t>),
    /// A `character` field, trimmed, against a literal trimmed.
    Trimmed(BytewiseRuns<'t>),
}

impl TermRuns<'_> {
    fn push(&mut self, run: &[u8]) {
        match self {
            Self::Nothing => {}
            Self::Order(order) => order.push(run),
            Self::Spellings(spellings) => spellings.push(run),
            Self::Trimmed(trimmed) => trimmed.push(run),
        }
    }
}

/// A field's text collected a run at a time no further than `longest`
/// bytes, past which no literal of a membership is it, trimmed of its
/// trailing blanks where the lookup trims.
struct Collected {
    text: Vec<u8>,
    longest: usize,
    trimmed: bool,
    /// Blanks read and not yet collected, where `trimmed`: a later
    /// non-blank collects them, and the text's end drops them.
    blanks: usize,
    /// Whether the text ran past `longest`.
    past: bool,
}

impl Collected {
    fn new(longest: usize, trimmed: bool) -> Self {
        Self { text: Vec::new(), longest, trimmed, blanks: 0, past: false }
    }

    fn push(&mut self, run: &[u8]) {
        if self.past {
            return;
        }
        let kept = match run.iter().rposition(|&byte| byte != b' ') {
            _ if !self.trimmed => run,
            None => return self.blanks += run.len(),
            Some(last) => &run[..=last],
        };
        let blanks = std::mem::take(&mut self.blanks);
        if self.text.len() + blanks + kept.len() > self.longest {
            self.past = true;
            self.text = Vec::new();
            return;
        }
        self.text.resize(self.text.len() + blanks, b' ');
        self.text.extend_from_slice(kept);
        if self.trimmed {
            self.blanks = run.len() - kept.len();
        }
    }

    /// The text collected, or `None` where it ran past `longest`.
    fn text(&self) -> Option<&[u8]> {
        (!self.past).then_some(self.text.as_slice())
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
