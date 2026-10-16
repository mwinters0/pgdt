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
//! codec's API: `internal` re-exports what a crate above calls.
//!
//! What a literal of each kind has to look like, for the refusal naming it,
//! is `form`'s; the refusal checks a strict parse dispatches through these
//! readers are `crate::grammar`'s.

pub(crate) mod form;
pub(crate) mod order;

use std::borrow::Cow;
use std::cmp::Ordering;
use std::mem::size_of;

use pgdt_model::PostgresInvalidValues;

use self::order::{OrderKey, Side, literal_key, order_key, order_keys, order_keys_stored};
use crate::compare::{CompareKind, ComparisonPlan, ComparisonSemantics};
use crate::value::nested::key::{NestedKey, comparable, compare_nested, nested_key};

/// A value's place in its column's order, its representation hidden: the key
/// a filter orders a row by, for a caller ordering values that meet no
/// filter — a summary's extremes, a partition's declared ordering. The
/// statistics gatherer, which keys by its stored sets' kinds and holds no
/// nested key, orders the scalar key beneath through `internal` instead
/// (`docs/design/decisions.md`, "D79").
///
/// There is no `Ord` (`docs/design/decisions.md`, "D56"): two keys are
/// ordered only where they share a representation, which two keys read
/// under one plan in one semantics always do.
#[derive(Debug, Clone)]
pub struct ValueKey(Key);

#[derive(Debug, Clone)]
enum Key {
    /// A scalar column's value, through its kind's representation.
    Scalar(OrderKey),
    /// An array's, a record's, a range's or a multirange's. Boxed: a
    /// `NestedKey` carries three vectors, and a scalar key is the common
    /// one.
    Nested(Box<NestedKey>),
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
            return order_key(&kind, text).map(|key| Self(Key::Scalar(key)));
        }
        let tree = nested_tree(plan, semantics)?;
        let key = nested_key(tree, text, Side::Field(PostgresInvalidValues::Default))?;
        Some(Self(Key::Nested(Box::new(key))))
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
            return literal_key(&kind, text, semantics).map(|key| Self(Key::Scalar(key)));
        }
        let tree = nested_tree(plan, semantics)?;
        Some(Self(Key::Nested(Box::new(nested_key(tree, text, Side::Literal)?))))
    }

    /// The order a filter compares by: a total order over the keys of one
    /// plan in one semantics, of which [`Self::stored_order`] is a
    /// refinement. **`None` where two finite keys' representations differ**
    /// — a finite date's key against a text's, an array's against a
    /// scalar's, a record's against one of another arity.
    ///
    /// deficiency: KD118 — that is narrower than "`None` wherever no order
    /// relates the two". `-infinity`, `infinity` and `NaN` are placed by
    /// rank before any representation is looked at, so a date's
    /// `-infinity` is below a text. Two keys sharing a representation
    /// compare as it orders whatever plans they were read under: a `date`'s
    /// against an `integer`'s or an enum label's position, and two
    /// DataFusion-order `numeric(p,s)` keys of different scales as unscaled
    /// integers, so `1.5` and `1.50` are unequal. No caller in this
    /// workspace compares keys of two plans. **(b) owned by P36**, 36.4.1:
    /// a variant per order, `Decimal` carrying its scale, and the specials
    /// their domain, so the variant match already made answers it.
    pub fn compare(&self, other: &Self) -> Option<Ordering> {
        match (&self.0, &other.0) {
            (Key::Scalar(a), Key::Scalar(b)) => order_keys(a, b),
            (Key::Nested(a), Key::Nested(b)) => comparable(a, b).then(|| compare_nested(a, b)),
            _ => None,
        }
    }

    /// **The order a stored bound and a block's row order are kept in**:
    /// [`Self::compare`], the order a filter compares by, with a float's `-0`
    /// below `0` where it ties them, as IEEE `totalOrder` — the order Arrow
    /// sorts a float by and DataFusion takes its `MIN`/`MAX` in — places
    /// them. A refinement, so an extreme in it is one in the filter's order
    /// and a block sorted in it is sorted in that order too
    /// (`docs/design/decisions.md`, "D79"). `None` where [`Self::compare`]
    /// is.
    pub fn stored_order(&self, other: &Self) -> Option<Ordering> {
        match (&self.0, &other.0) {
            (Key::Scalar(a), Key::Scalar(b)) => order_keys_stored(a, b),
            _ => self.compare(other),
        }
    }

    /// The heap this key holds, as the sizes the allocator was asked for.
    pub fn heap_bytes(&self) -> u64 {
        match &self.0 {
            Key::Scalar(key) => key.heap_bytes(),
            Key::Nested(key) => size_of::<NestedKey>() as u64 + key.heap_bytes(),
        }
    }
}

#[cfg(test)]
mod tests;
