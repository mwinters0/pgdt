//! DataFusion's dynamic filters: what a scan holds of them, and each one's
//! state as the library's filter tree
//! (`docs/design/roadmap-P27-dynamic-filters.md`).
//!
//! A hash join's build side, a TopK's heap and an ungrouped `MIN`/`MAX` each
//! publish a filter into the scan below them, pushed in the `Post` phase of
//! filter pushdown and updated while the query runs. **Every producer
//! re-checks its own rows**, so a scan answers `No` for each filter it holds,
//! and what it may do with one is only ever skip rows the filter rules out.
//!
//! **A filter is translated loosened, never refused.** Its state now is read
//! into the library's tree in Arrow semantics, as a static filter is
//! ([`crate::pushdown`]), and every row DataFusion's evaluation of that state
//! keeps, the translation keeps: a wrongly skipped row is one the join or the
//! TopK never sees. Where a part has no library term — a `hash_lookup`, a
//! `struct(…) IN`, a column under a cast — it stands as whatever keeps every
//! row where it sits: `true` beneath an even number of `NOT`s, `false`
//! beneath an odd one ([`Parity`]). So translating never fails, and the worst
//! a filter can come to is keeping every row.

use std::fmt;
use std::sync::Arc;

use datafusion::common::tree_node::TreeNodeRecursion;
use datafusion::common::{Result, ScalarValue};
use datafusion::logical_expr::Operator;
use datafusion::physical_expr::PhysicalExpr;
use datafusion::physical_expr::expressions::{
    BinaryExpr, CaseExpr, Column, InListExpr, IsNotNullExpr, IsNullExpr, Literal, NotExpr,
};
use datafusion::physical_expr_common::physical_expr::fmt_sql;
use datafusion::physical_plan::apply_expression_roots;
use pgdump_query::{Expr as L, PredicateOp, ResolvedSchema};

use crate::pushdown::{boolean_term, compared, is_float, null_term};

#[cfg(test)]
mod tests;

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

/// `filter`'s state now as the library's tree over `table`, **keeping every
/// row DataFusion's evaluation of it keeps**, and exactly those wherever each
/// part has a library term that answers as DataFusion does.
///
/// `table` is the scan's own schema: a physical column is read by its index,
/// as DataFusion evaluates it, and stands for nothing unless the field there
/// carries its name.
#[cfg_attr(not(test), expect(dead_code, reason = "no replay consumes a dynamic filter yet"))]
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

/// A column of `table` in an `IN` list of literals, as the `Or` of its `=`
/// terms (`docs/design/decisions.md`, "D53"), and `NOT IN` as its negation.
fn in_list(list: &InListExpr, parity: Parity, table: &ResolvedSchema) -> L {
    if list.negated() {
        L::Not(Box::new(membership(list, parity.flip(), table)))
    } else {
        membership(list, parity, table)
    }
}

/// Whether a row's value is among `list`'s. A `NULL` in the list matches no
/// row, so it leaves the `Or`, though it makes the membership never false.
/// **A float's is loosened only where a row needs it true**: DataFusion
/// answers it from a set of the values' bits, so `-0` is not in a list
/// holding `0` there, where the library's `=` equates them ([`is_float`]).
fn membership(list: &InListExpr, parity: Parity, table: &ResolvedSchema) -> L {
    let Some(index) = column_at(list.expr(), table) else { return parity.anything() };
    let mut values = Vec::with_capacity(list.len());
    for item in list.list() {
        let Some(literal) = item.downcast_ref::<Literal>() else { return parity.anything() };
        values.push(literal.value());
    }
    if parity == Parity::Odd && values.iter().any(|value| value.is_null()) {
        return L::And(Vec::new());
    }
    if parity == Parity::Odd && is_float(index, table) {
        return parity.anything();
    }
    let terms: Option<Vec<L>> = values
        .into_iter()
        .filter(|value| !value.is_null())
        .map(|value| compared(index, Operator::Eq, value, table).map(L::Term))
        .collect();
    terms.map_or_else(|| parity.anything(), L::Or)
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
