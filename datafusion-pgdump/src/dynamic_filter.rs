//! DataFusion's dynamic filters: what a scan holds of them, and each one's
//! state as the library's filter tree (`docs/design/decisions.md`, "D94").
//!
//! A hash join's build side, a TopK's heap and an ungrouped `MIN`/`MAX` each
//! publish a filter into the scan below them, pushed in the `Post` phase of
//! filter pushdown and updated while the query runs. **Every producer
//! re-checks its own rows**, so a scan answers `No` for each filter it holds,
//! and what it may do with one is only ever skip rows the filter rules out.
//!
//! **A filter is translated loosened, never refused.** Its state now is read
//! into the library's tree in DataFusion semantics, as a static filter is
//! ([`crate::pushdown`]), and every row DataFusion's evaluation of that state
//! keeps, or its producer keeps by its own order ([`loosened`]), the
//! translation keeps: a wrongly skipped row is one the join or the TopK never
//! sees. Where a part has no library term — a `hash_lookup`, a
//! `struct(…) IN`, a column under a cast — it stands as whatever keeps every
//! row where it sits: `true` beneath an even number of `NOT`s, `false`
//! beneath an odd one ([`Parity`]). So translating never fails, and the worst
//! a filter can come to is keeping every row.
//!
//! **The replay reads the translation when its first sub-stream is polled**,
//! where it cuts them, **and as they run** ([`ReplayFilter`],
//! `pgdump_query::DynamicFilter`), each time one enters a row group or takes
//! a chunk; the filters are translated again only once one of them has moved.

use std::fmt;
use std::sync::{Arc, Mutex};

use datafusion::common::tree_node::TreeNodeRecursion;
use datafusion::common::{Result, ScalarValue};
use datafusion::logical_expr::Operator;
use datafusion::physical_expr::PhysicalExpr;
use datafusion::physical_expr::expressions::{
    BinaryExpr, CaseExpr, Column, InListExpr, IsNotNullExpr, IsNullExpr, Literal, NotExpr,
};
use datafusion::physical_expr_common::physical_expr::{fmt_sql, snapshot_generation};
use datafusion::physical_plan::apply_expression_roots;
use pgdump_query::{DynamicFilter, Expr as L, Membership, PredicateOp, ResolvedSchema};

use crate::pushdown::{boolean_term, compared, is_float, member, null_term};

#[cfg(test)]
mod tests;

// deficiency: KD56 — an ungrouped aggregate's filter is the `OR` of `col < min`
// for each `MIN` and `col > max` for each `MAX`, each bound shared across its
// partitions and folded with `scalar_min`/`scalar_max`, which in DataFusion
// 55.1 (`aggregate_stream.rs`) read only the untyped `ScalarValue::Null` as no
// bound. A partition's batch holding no value of a column evaluates to a typed
// NULL, which orders below every value, is kept as the `MIN` and is then
// skipped as NULL when the filter is built, so the `MIN` side is gone for the
// rest of the query; and a column with no bound yet is skipped while the
// others' sides are published, so rows its aggregates still need are cut. A
// group or row this scan skips under either is lost to the answer, with
// statistics gathered and rows left undropped, the defaults
// (`tests/aggregate_bounds.rs`). **(c) unowned**; the fix is upstream's.
/// The dynamic filters a scan's plan node holds, in the order they reached
/// it, none twice.
#[derive(Debug, Clone, Default)]
pub(crate) struct DynamicFilters(Vec<Arc<dyn PhysicalExpr>>);

impl DynamicFilters {
    /// These and each of `pushed` not already held, or `None` where every one
    /// is: a producer pushes the filter it already published again on a
    /// second `Post` pass, and holding it twice would read it twice.
    pub(crate) fn with(&self, pushed: Vec<Arc<dyn PhysicalExpr>>) -> Option<Self> {
        let mut held = self.0.clone();
        for filter in pushed {
            if !held.iter().any(|h| h == &filter) {
                held.push(filter);
            }
        }
        (held.len() > self.0.len()).then_some(Self(held))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// `f` over each held filter: what a hash join searches its probe side
    /// for before it computes one at all.
    pub(crate) fn apply(
        &self,
        f: &mut dyn FnMut(&Arc<dyn PhysicalExpr>) -> Result<TreeNodeRecursion>,
    ) -> Result<TreeNodeRecursion> {
        apply_expression_roots(&self.0, f)
    }

    /// The held filters as `EXPLAIN`'s tree rendering writes an expression.
    pub(crate) fn sql(&self) -> impl fmt::Display + '_ {
        Sql(self)
    }
}

/// The held filters, each in its state now — `DynamicFilter [ … ]`, and
/// `DynamicFilter [ empty ]` before its first update, as DataFusion writes
/// one — as the one conjunction they are.
impl fmt::Display for DynamicFilters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, filter) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" AND ")?;
            }
            write!(f, "{filter}")?;
        }
        Ok(())
    }
}

/// [`DynamicFilters::sql`].
struct Sql<'a>(&'a DynamicFilters);

impl fmt::Display for Sql<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, filter) in self.0.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" AND ")?;
            }
            write!(f, "{}", fmt_sql(filter.as_ref()))?;
        }
        Ok(())
    }
}

/// The dynamic filters a scan holds, as each of its sub-streams reads them:
/// their conjunction, each [`loosened`] over the schema the scan emits.
///
/// **Its generation is the sum of theirs**, each of which only rises, so it
/// moves whenever one of them does; and the translation last made is kept
/// until it has, since the replay asks at every row group it enters and
/// every chunk it takes, and a join's filter, once complete, never moves
/// again.
pub(crate) struct ReplayFilter {
    held: DynamicFilters,
    table: ResolvedSchema,
    translated: Mutex<Option<(u64, Arc<L>)>>,
}

impl ReplayFilter {
    pub(crate) fn new(held: DynamicFilters, table: ResolvedSchema) -> Self {
        Self { held, table, translated: Mutex::new(None) }
    }
}

impl fmt::Debug for ReplayFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ReplayFilter({})", self.held)
    }
}

impl DynamicFilter for ReplayFilter {
    /// Each filter's generation read under its own lock, summed. Not
    /// `DynamicFilterTracker`, whose one-load "moved?" sits behind a
    /// `changed(&mut self)` subscribing one consumer, so a scan's sub-streams
    /// sharing it would share a lock.
    fn generation(&self) -> u64 {
        self.held.0.iter().fold(0, |sum, filter| sum.wrapping_add(snapshot_generation(filter)))
    }

    /// The generation is read before the states it is paired with: a filter
    /// moving in between is translated at its new state under the old
    /// generation, and so read again at the next group or chunk, where the other order
    /// would pair an old state with the new generation and keep it.
    fn current(&self) -> (u64, Arc<L>) {
        let generation = self.generation();
        let mut translated =
            self.translated.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((at, state)) = translated.as_ref()
            && *at == generation
        {
            return (generation, Arc::clone(state));
        }
        let state = Arc::new(L::And(
            self.held.0.iter().map(|filter| loosened(filter, &self.table)).collect(),
        ));
        *translated = Some((generation, Arc::clone(&state)));
        (generation, state)
    }
}

/// `filter`'s state now as the library's tree over `table`, **keeping every
/// row DataFusion's evaluation of it keeps and every row its producer keeps
/// by its own order** — the two part at a float's zeros ([`comparison`]) —
/// and exactly DataFusion's rows wherever each part has a library term that
/// answers as DataFusion does.
///
/// `table` is the scan's own schema: a physical column is read by its index,
/// as DataFusion evaluates it, and stands for nothing unless the field there
/// carries its name.
pub(crate) fn loosened(filter: &Arc<dyn PhysicalExpr>, table: &ResolvedSchema) -> L {
    translate(filter, Parity::Even, table)
}

/// How many `NOT`s stand above a part of a filter, as far as loosening it
/// cares. Beneath an even number, a row is kept where the part is true, so a
/// loosened part must be true wherever the part is; beneath an odd number, a
/// row is kept where the part is false, so it must be false wherever the part
/// is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parity {
    Even,
    Odd,
}

impl Parity {
    fn flip(self) -> Self {
        match self {
            Self::Even => Self::Odd,
            Self::Odd => Self::Even,
        }
    }

    /// What a part with no library term stands as: whatever keeps every row
    /// the part might.
    fn anything(self) -> L {
        match self {
            Self::Even => L::And(Vec::new()),
            Self::Odd => L::Or(Vec::new()),
        }
    }

    /// What a part that is `NULL` on every row stands as: a row is never kept
    /// on its account, so it is false where a row needs it true and true where
    /// a row needs it false.
    fn unknown(self) -> L {
        self.flip().anything()
    }
}

fn translate(expr: &Arc<dyn PhysicalExpr>, parity: Parity, table: &ResolvedSchema) -> L {
    // A dynamic filter, however deep, is read as it stands now.
    match expr.snapshot() {
        Ok(Some(now)) => return translate(&now, parity, table),
        Ok(None) => {}
        Err(_) => return parity.anything(),
    }
    if let Some(binary) = expr.downcast_ref::<BinaryExpr>() {
        let (left, right) = (binary.left(), binary.right());
        return match binary.op() {
            Operator::And => {
                L::And(vec![translate(left, parity, table), translate(right, parity, table)])
            }
            Operator::Or => {
                L::Or(vec![translate(left, parity, table), translate(right, parity, table)])
            }
            &op => comparison(left, op, right, parity, table),
        };
    }
    if let Some(not) = expr.downcast_ref::<NotExpr>() {
        return L::Not(Box::new(translate(not.arg(), parity.flip(), table)));
    }
    if let Some(test) = expr.downcast_ref::<IsNullExpr>() {
        return null_test(test.arg(), PredicateOp::IsNull, parity, table);
    }
    if let Some(test) = expr.downcast_ref::<IsNotNullExpr>() {
        return null_test(test.arg(), PredicateOp::IsNotNull, parity, table);
    }
    if let Some(list) = expr.downcast_ref::<InListExpr>() {
        return in_list(list, parity, table);
    }
    if let Some(case) = expr.downcast_ref::<CaseExpr>() {
        return case_branches(case, parity, table);
    }
    if let Some(literal) = expr.downcast_ref::<Literal>() {
        return match literal.value() {
            ScalarValue::Boolean(Some(true)) => L::And(Vec::new()),
            ScalarValue::Boolean(Some(false)) => L::Or(Vec::new()),
            ScalarValue::Boolean(None) | ScalarValue::Null => parity.unknown(),
            _ => parity.anything(),
        };
    }
    // A boolean column standing alone is its own truth, as `= true`.
    column_at(expr, table)
        .and_then(|index| boolean_term(index, PredicateOp::Eq, Some(true), table))
        .map_or_else(|| parity.anything(), L::Term)
}

/// `left op right`, one side a column of `table` and the other a literal.
///
/// **A float compared with a zero of either sign has no term.** A TopK's heap
/// and an aggregate's `MIN`/`MAX` order a float by IEEE `totalOrder`, `-0`
/// below `0`, and publish their threshold as a comparison DataFusion
/// evaluates with `-0` made `0`, as the library compares too: under `f > -0`,
/// a `MAX` holding `-0` rejects the `0` it would still have taken. So no
/// state answering as DataFusion evaluates it keeps what the producer needs
/// there, and the part stands as whatever keeps every row. Upstream, as of
/// DataFusion 55.1.0: `apply_cmp` runs `normalize_cmp_input` over both
/// operands, where TopK's heap compares row-format keys and `MaxAccumulator`
/// folds with arrow's `max`, whose float `is_gt` is `total_cmp`; a release
/// whose producers' thresholds hold with the zeros made equal leaves this
/// rule costing pruning at a zero and saving nothing.
fn comparison(
    left: &Arc<dyn PhysicalExpr>,
    op: Operator,
    right: &Arc<dyn PhysicalExpr>,
    parity: Parity,
    table: &ResolvedSchema,
) -> L {
    let literal = |expr: &Arc<dyn PhysicalExpr>| {
        expr.downcast_ref::<Literal>().map(|literal| literal.value().clone())
    };
    let sides = match (column_at(left, table), literal(right)) {
        (Some(index), Some(value)) => Some((index, op, value)),
        _ => match (literal(left), column_at(right, table), op.swap()) {
            (Some(value), Some(index), Some(op)) => Some((index, op, value)),
            _ => None,
        },
    };
    let Some((index, op, value)) = sides else { return parity.anything() };
    if is_float(index, table) && is_zero(&value) {
        return parity.anything();
    }
    if value.is_null() {
        return match op {
            Operator::IsDistinctFrom => L::Term(null_term(index, PredicateOp::IsNotNull, table)),
            Operator::IsNotDistinctFrom => L::Term(null_term(index, PredicateOp::IsNull, table)),
            Operator::Eq
            | Operator::NotEq
            | Operator::Lt
            | Operator::LtEq
            | Operator::Gt
            | Operator::GtEq => parity.unknown(),
            _ => parity.anything(),
        };
    }
    compared(index, op, &value, table).map_or_else(|| parity.anything(), L::Term)
}

/// Whether `value` is a float zero, of either sign.
fn is_zero(value: &ScalarValue) -> bool {
    match value {
        ScalarValue::Float32(Some(v)) => *v == 0.0,
        ScalarValue::Float64(Some(v)) => *v == 0.0,
        _ => false,
    }
}

/// `IS NULL` or `IS NOT NULL` on a column of `table`, any type.
fn null_test(
    arg: &Arc<dyn PhysicalExpr>,
    op: PredicateOp,
    parity: Parity,
    table: &ResolvedSchema,
) -> L {
    column_at(arg, table)
        .map_or_else(|| parity.anything(), |index| L::Term(null_term(index, op, table)))
}

/// A column of `table` in an `IN` list of literals, as one membership
/// (`docs/design/decisions.md`, "D53"), and `NOT IN` as its negation.
fn in_list(list: &InListExpr, parity: Parity, table: &ResolvedSchema) -> L {
    if list.negated() {
        L::Not(Box::new(membership(list, parity.flip(), table)))
    } else {
        membership(list, parity, table)
    }
}

/// Whether a row's value is among `list`'s, a `NULL` in it answering as SQL's
/// does in the library's membership too. **A float's is loosened only where a
/// row needs it true**: DataFusion answers it from a set of the values' bits,
/// so `-0` is not in a list holding `0` there, where the library's `=`
/// equates them ([`is_float`]). That is asked before the list is read, so a
/// float's `NOT IN` holding a `NULL`, which keeps no row, keeps every row
/// here; no producer publishes one.
fn membership(list: &InListExpr, parity: Parity, table: &ResolvedSchema) -> L {
    let Some(index) = column_at(list.expr(), table) else { return parity.anything() };
    if parity == Parity::Odd && is_float(index, table) {
        return parity.anything();
    }
    let values: Option<Vec<Option<String>>> = list
        .list()
        .iter()
        .map(|item| member(index, item.downcast_ref::<Literal>()?.value(), table))
        .collect();
    let Some(values) = values else { return parity.anything() };
    L::In(Membership { column: table.schema.field(index).name().clone(), values })
}

/// A `CASE` as its branches alone: the branch answering a row is one of its
/// `THEN`s or its `ELSE` — a missing `ELSE` is `NULL`, neither true nor false
/// — so the `CASE` is true only where some branch is, and false only where
/// some branch is. Which branch a row takes is never asked, which is how a
/// partitioned join's routing on `hash_repartition` needs no term.
fn case_branches(case: &CaseExpr, parity: Parity, table: &ResolvedSchema) -> L {
    let branches: Vec<L> = case
        .when_then_expr()
        .iter()
        .map(|(_, then)| then)
        .chain(case.else_expr())
        .map(|branch| translate(branch, parity, table))
        .collect();
    match parity {
        Parity::Even => L::Or(branches),
        Parity::Odd => L::And(branches),
    }
}

/// Where `expr`, a bare column, sits in `table`: by its index, as DataFusion
/// reads it, and only where the field there carries its name.
fn column_at(expr: &Arc<dyn PhysicalExpr>, table: &ResolvedSchema) -> Option<usize> {
    let column = expr.downcast_ref::<Column>()?;
    let field = table.schema.fields().get(column.index())?;
    (field.name() == column.name()).then_some(column.index())
}
