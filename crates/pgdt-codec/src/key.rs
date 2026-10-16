//! A value's place in PostgreSQL's order, read from its text.
//!
//! A key is a function of a column's [`ComparisonPlan`], the semantics it is
//! compared in and one unescaped text alone, so a field, a filter's literal, a
//! stored bound and a gathered value are ordered by one comparison: the
//! filter's (`pgdt_query::predicate`), the statistics gatherer's
//! (`pgdt_mapping`'s `statistics::gather`) and the summaries' alike.
//!
//! [`ValueKey`] is the API, for every value a plan orders, a nested one's
//! included. What builds one — the dispatch over the type families under
//! `crate::value`, each holding its own key type — is `order`'s, and not the
//! codec's API: `internal` re-exports what a crate above calls. The order a
//! `ValueKey` was read in, and which two orders relate, is `relate`'s.
//!
//! What a literal of each kind has to look like, for the refusal naming it,
//! is `form`'s; the refusal checks a strict parse dispatches through these
//! readers are `crate::grammar`'s.

pub(crate) mod form;
pub(crate) mod order;
mod relate;

use std::borrow::Cow;
use std::cmp::Ordering;
use std::mem::size_of;

use pgdt_model::PostgresInvalidValues;

use self::order::{OrderKey, Side, literal_key, order_key, order_keys};
use self::relate::{NestedOrder, ScalarOrder};
use crate::compare::{CompareKind, ComparisonPlan, ComparisonSemantics};
use crate::value::nested::key::{NestedKey, compare_nested, nested_key};

/// A value's place in its column's order, its representation hidden: the key
/// a filter orders a row by, for an embedder ordering values of a column as
/// a filter of a semantics would — a nested one's included. Nothing in this
/// workspace holds one: the filter, the statistics gatherer and the summary
/// key through `internal` by the kind or tree they already hold, and a
/// stored bound is ordered as gathering kept it, which is `internal`'s
/// alone (`docs/design/decisions.md`, "D79", "D106").
///
/// **A key carries the order it was read in** — its semantics, and its
/// plan's order at every level — which [`ValueKey::compare`] checks before
/// it reads a value, so two keys are ordered only where the semantics'
/// engine relates their orders. There is no `Ord`
/// (`docs/design/decisions.md`, "D56").
#[derive(Debug, Clone)]
pub struct ValueKey {
    semantics: ComparisonSemantics,
    key: Key,
}

#[derive(Debug, Clone)]
enum Key {
    /// A scalar column's value, through its kind's representation.
    Scalar { order: ScalarOrder, key: OrderKey },
    /// An array's, a record's, a range's or a multirange's. Boxed: a
    /// `NestedKey` carries three vectors, and a scalar key is the common
    /// one.
    Nested(Box<(NestedOrder, NestedKey)>),
}

/// The kind a scalar column of `plan` is ordered by in `semantics` — the kind
/// a filter's term compares by — or `None` where `semantics` orders no
/// scalar of `plan`: a nested or unanswerable plan, or one the register gives
/// no order, which DataFusion's semantics orders as its emitted text
/// (`docs/design/decisions.md`, "D40").
fn scalar_kind(
    plan: &ComparisonPlan,
    semantics: ComparisonSemantics,
) -> Option<Cow<'_, CompareKind>> {
    match (plan, semantics) {
        (ComparisonPlan::Compared { kind, .. }, ComparisonSemantics::Postgres) => {
            Some(Cow::Borrowed(kind))
        }
        (ComparisonPlan::Compared { kind, .. }, ComparisonSemantics::DataFusion) => {
            Some(Cow::Owned(kind.datafusion_order()))
        }
        (ComparisonPlan::Refused, ComparisonSemantics::DataFusion) => {
            Some(Cow::Owned(CompareKind::Text { length: None }))
        }
        _ => None,
    }
}

/// The tree a nested column of `plan` is ordered by in `semantics`: only
/// PostgreSQL's, DataFusion's refusing a nested comparison, and only where
/// every position beneath it is ordered ([`ComparisonPlan::orders`]).
fn nested_tree(
    plan: &ComparisonPlan,
    semantics: ComparisonSemantics,
) -> Option<&crate::compare::NestedCompare> {
    match (plan, semantics) {
        (ComparisonPlan::Nested(tree), ComparisonSemantics::Postgres) if plan.orders() => {
            Some(tree)
        }
        _ => None,
    }
}

impl ValueKey {
    /// The key of one unescaped field of a column of `plan`, ordered as
    /// `semantics` orders it. `None` where `semantics` gives the column no
    /// order, or where the text is not a value of its type, read as a field
    /// is: put through its typmod, and refused where the type's `*_in`
    /// refuses it ([`PostgresInvalidValues::Default`]).
    pub fn of(plan: &ComparisonPlan, semantics: ComparisonSemantics, text: &str) -> Option<Self> {
        if let Some(kind) = scalar_kind(plan, semantics) {
            let key = order_key(&kind, text)?;
            return Some(Self {
                semantics,
                key: Key::Scalar { order: ScalarOrder::of(&kind), key },
            });
        }
        let tree = nested_tree(plan, semantics)?;
        let key = nested_key(tree, text, Side::Field(PostgresInvalidValues::Default))?;
        Some(Self { semantics, key: Key::Nested(Box::new((NestedOrder::of(tree)?, key))) })
    }

    /// The key of a filter's literal compared against a column of `plan` in
    /// `semantics`, read in its type's input grammar
    /// (`docs/design/decisions.md`, "D55"): with no typmod, and in
    /// DataFusion's semantics as DataFusion reads the value. `None` where
    /// `semantics` gives the column no order, or the text is not a literal
    /// of its type.
    pub fn literal(
        plan: &ComparisonPlan,
        semantics: ComparisonSemantics,
        text: &str,
    ) -> Option<Self> {
        if let Some(kind) = scalar_kind(plan, semantics) {
            let key = literal_key(&kind, text, semantics)?;
            return Some(Self {
                semantics,
                key: Key::Scalar { order: ScalarOrder::of(&kind), key },
            });
        }
        let tree = nested_tree(plan, semantics)?;
        let key = nested_key(tree, text, Side::Literal)?;
        Some(Self { semantics, key: Key::Nested(Box::new((NestedOrder::of(tree)?, key))) })
    }

    /// The order a filter compares by: a total order over the keys of one
    /// plan in one semantics. Two keys of different plans compare where the
    /// semantics' engine resolves `a < b` between their orders without
    /// consulting a setting, with its answer: in PostgreSQL's, `smallint`
    /// against `bigint`, an integer against a `numeric`, a float or an `oid`,
    /// two `numeric` scales, a `date` against a `timestamp` and a `time`
    /// against an `interval`, their special values included. They answer
    /// **`None` wherever it resolves none**: a `date` against an `integer`,
    /// a `macaddr` against a `macaddr8`, two enum types, a pair ordered only
    /// under `TimeZone`, two keys read in two semantics, and two nested keys
    /// but of one order at every level, as the server relates no two array,
    /// range or record types.
    ///
    /// deficiency: KD119 — in DataFusion's semantics two keys compare only
    /// where their orders are one, where DataFusion's comparison coercion
    /// relates two emitted types it casts between: two integer widths, an
    /// integer against a float or a decimal, two decimal scales, a `Date32`
    /// against a `Timestamp`, and a text against a number or a time, which
    /// it parses. **(b) owned by 36.4.5**, which answers those pairs as
    /// DataFusion 55.1.0 casts them.
    pub fn compare(&self, other: &Self) -> Option<Ordering> {
        if self.semantics != other.semantics {
            return None;
        }
        match (&self.key, &other.key) {
            (Key::Scalar { order: a, key: x }, Key::Scalar { order: b, key: y }) => {
                match self.semantics {
                    ComparisonSemantics::Postgres => relate::postgres(a, x, b, y),
                    ComparisonSemantics::DataFusion => (a == b).then(|| order_keys(x, y))?,
                }
            }
            (Key::Nested(a), Key::Nested(b)) => (a.0 == b.0).then(|| compare_nested(&a.1, &b.1)),
            _ => None,
        }
    }

    /// The heap this key holds, as the sizes the allocator was asked for: a
    /// nested key's tree of orders beside its value, and none of an enum's
    /// labels, which its plan shares.
    pub fn heap_bytes(&self) -> u64 {
        match &self.key {
            Key::Scalar { key, .. } => key.heap_bytes(),
            Key::Nested(nested) => {
                let (order, key) = &**nested;
                size_of::<(NestedOrder, NestedKey)>() as u64 + order.heap_bytes() + key.heap_bytes()
            }
        }
    }
}

#[cfg(test)]
mod tests;
