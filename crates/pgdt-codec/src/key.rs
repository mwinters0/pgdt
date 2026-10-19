//! A value's place in PostgreSQL's order, read from its text.
//!
//! A key is a function of a column's resolution, the semantics it is
//! compared in and one unescaped text alone, so a field, a filter's literal, a
//! stored bound and a gathered value are ordered by one comparison: the
//! filter's (`pgdt_query::predicate`), the statistics gatherer's
//! (`pgdt_mapping`'s `statistics::gather`) and the summaries' alike.
//!
//! [`ValueKey`] is the API, for every value a plan orders, a nested one's
//! included. What builds one — the dispatch over the type families under
//! `crate::value`, each holding its own key type — is `order`'s, and not the
//! codec's API: `internal` re-exports what a crate above calls. The order a
//! `ValueKey` was read in, and which two orders relate, is `relate`'s; the
//! value a key read in DataFusion's semantics hands an engine relating two
//! of its orders is `emitted`'s.
//!
//! What a literal of each kind has to look like, for the refusal naming it,
//! is `form`'s; the refusal checks a strict parse dispatches through these
//! readers are `crate::grammar`'s.

mod emitted;
pub(crate) mod form;
pub(crate) mod order;
mod relate;

use std::borrow::Cow;
use std::cmp::Ordering;
use std::mem::size_of;

use arrow_schema::DataType;
use pgdt_model::PostgresInvalidValues;

pub use self::emitted::EmittedValue;
use self::emitted::emitted_value;
use self::order::{OrderKey, Side, literal_key, order_key, order_keys};
use self::relate::{NestedOrder, ScalarOrder};
use crate::compare::{CompareKind, ComparisonPlan, ComparisonSemantics};
use crate::resolve::ResolvedColumn;
use crate::unrepresentable::holds_value;
use crate::value::nested::key::{NestedKey, nested_key};

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
/// (`docs/design/decisions.md`, "D56"). **A key read in DataFusion's
/// semantics carries its column's emitted Arrow type besides**, never read
/// to order it: what DataFusion coerces two orders by
/// ([`ValueKey::emitted`]; `docs/design/decisions.md`, "D40").
#[derive(Debug, Clone)]
pub struct ValueKey {
    semantics: ComparisonSemantics,
    key: Key,
}

#[derive(Debug, Clone)]
enum Key {
    /// A scalar column's value, through its kind's representation, and in
    /// DataFusion's semantics the type its column emits it as, where that
    /// type holds it.
    Scalar { order: ScalarOrder, key: OrderKey, emitted: Option<DataType> },
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
            Some(Cow::Owned(CompareKind::Text { length: None, varying: false }))
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
    /// The key of one unescaped field of `column`, ordered as `semantics`
    /// orders it. `None` where `semantics` gives the column no order, where
    /// the untyped mode read the column as its text under the other
    /// semantics ([`ResolvedColumn::read_as_text_under`]), or where the text
    /// is not a value of its type, read as a field is: put through its
    /// typmod, and refused where the type's `*_in` refuses it
    /// ([`PostgresInvalidValues::Default`]).
    ///
    /// `column` is the record a query reads the column under, in
    /// `semantics`: its comparison plan orders the key, and in DataFusion's
    /// semantics its Arrow field is the type the key carries.
    pub fn of(column: &ResolvedColumn, semantics: ComparisonSemantics, text: &str) -> Option<Self> {
        Self::read(column, semantics, text, Side::Field(PostgresInvalidValues::Default))
    }

    /// The key of a filter's literal compared against `column` in
    /// `semantics`, read in its type's input grammar
    /// (`docs/design/decisions.md`, "D55"): with no typmod, and in
    /// DataFusion's semantics as DataFusion reads the value. `None` where
    /// [`Self::of`] would be for the column, or where the text is not a
    /// literal of its type.
    pub fn literal(
        column: &ResolvedColumn,
        semantics: ComparisonSemantics,
        text: &str,
    ) -> Option<Self> {
        Self::read(column, semantics, text, Side::Literal)
    }

    /// [`Self::of`] or [`Self::literal`], as `side` says.
    fn read(
        column: &ResolvedColumn,
        semantics: ComparisonSemantics,
        text: &str,
        side: Side,
    ) -> Option<Self> {
        // A column read as its text compares in its widening's semantics
        // alone: under PostgreSQL's it keeps its declared plan, which
        // DataFusion's would order as that type where it compares the text
        // bytewise (`docs/design/decisions.md`, "D100").
        if column.read_as_text_under().is_some_and(|widened| widened != semantics) {
            return None;
        }
        let plan = column.comparison();
        if let Some(kind) = scalar_kind(plan, semantics) {
            let key = match side {
                Side::Literal => literal_key(&kind, text, semantics)?,
                Side::Field(_) => order_key(&kind, text)?,
            };
            let order = ScalarOrder::of(&kind, semantics);
            let emitted = (semantics == ComparisonSemantics::DataFusion)
                .then(|| column.data_type())
                .filter(|data_type| holds_value(data_type, column.plan(), text))
                .cloned();
            return Some(Self { semantics, key: Key::Scalar { order, key, emitted } });
        }
        let tree = nested_tree(plan, semantics)?;
        let key = nested_key(tree, text, side)?;
        Some(Self { semantics, key: Key::Nested(Box::new((NestedOrder::of(tree)?, key))) })
    }

    /// The order a filter compares by: a total order over the keys of one
    /// plan in one semantics. Two keys of different plans compare where the
    /// semantics' engine resolves `a < b` between their orders without
    /// consulting a setting, with its answer: in PostgreSQL's, `smallint`
    /// against `bigint`, an integer against a `numeric`, a float or an `oid`,
    /// two `numeric` scales, a `date` against a `timestamp`, a `time`
    /// against an `interval` and a `character(n)` against a `text` or a
    /// `character varying`, each its own way, their special values included;
    /// and two nested keys of one type at every level, their leaves related
    /// so, as `numeric(5,1)[]` against `numeric[]`. They answer **`None`
    /// wherever it resolves none**: a `date` against an `integer`, a
    /// `macaddr` against a `macaddr8`, two enum types, a pair ordered only
    /// under `TimeZone`, two keys read in two semantics, and two nested keys
    /// whose types differ at any level — two range types over one subtype,
    /// two records whose columns are two composite types of one shape — but
    /// two composite types at the top, which compare where their column
    /// types are one.
    ///
    /// In DataFusion's semantics, two keys compare here only where their
    /// orders are one. Two of its orders its comparison coercion relates —
    /// two integer widths, an integer against a float, a decimal or a
    /// `Date32`, a text against a number or a time — are related by its own
    /// coercion and cast, which `datafusion_pgdump::compare_value_keys`
    /// answers through [`Self::emitted`]: the codec links no arrow-cast
    /// (`docs/design/decisions.md`, "D74").
    pub fn compare(&self, other: &Self) -> Option<Ordering> {
        if self.semantics != other.semantics {
            return None;
        }
        match (&self.key, &other.key) {
            (Key::Scalar { order: a, key: x, .. }, Key::Scalar { order: b, key: y, .. }) => {
                match self.semantics {
                    ComparisonSemantics::Postgres => relate::postgres(a, x, b, y),
                    ComparisonSemantics::DataFusion => (a == b).then(|| order_keys(x, y))?,
                }
            }
            // Only PostgreSQL's semantics keys a nested value.
            (Key::Nested(a), Key::Nested(b)) => relate::nested(&a.0, &a.1, &b.0, &b.1),
            _ => None,
        }
    }

    /// **The value as its column emits it, beside the emitted Arrow type**,
    /// for an engine relating two of DataFusion's orders by its own coercion
    /// and cast. `None` for a key read in PostgreSQL's semantics, for a
    /// nested key, and for a value its column's type cannot hold — one past
    /// either tier the provider reads as unrepresentable, a special value
    /// among them (`docs/design/decisions.md`, "D96") — which therefore
    /// answers nothing against another order. A literal's emitted type is
    /// its column's, which may not hold it: a `smallint` literal is read as
    /// an `i64`.
    pub fn emitted(&self) -> Option<(&DataType, EmittedValue<'_>)> {
        match &self.key {
            Key::Scalar { order, key, emitted: Some(data_type) } => {
                Some((data_type, emitted_value(order, key)?))
            }
            _ => None,
        }
    }

    /// The heap this key holds, as the sizes the allocator was asked for: a
    /// nested key's tree of orders beside its value, a dictionary type's two
    /// boxes, and none of an enum's labels or a timestamp's zone, which its
    /// plan and its schema share.
    pub fn heap_bytes(&self) -> u64 {
        match &self.key {
            Key::Scalar { key, emitted, .. } => {
                let boxes = match emitted {
                    Some(DataType::Dictionary(..)) => 2 * size_of::<DataType>() as u64,
                    _ => 0,
                };
                key.heap_bytes() + boxes
            }
            Key::Nested(nested) => {
                let (order, key) = &**nested;
                size_of::<(NestedOrder, NestedKey)>() as u64 + order.heap_bytes() + key.heap_bytes()
            }
        }
    }
}

#[cfg(test)]
mod tests;
