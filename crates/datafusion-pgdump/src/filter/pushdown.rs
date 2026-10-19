//! DataFusion's filters as the library's filter tree, for the filters a scan
//! can answer itself.
//!
//! **A filter is pushed `Exact` or not at all.** It is `Exact` where it
//! translates here and the library's plan resolves it in
//! [`ComparisonSemantics::DataFusion`](pgdt_codec::compare::ComparisonSemantics::DataFusion) —
//! which is where the library answers as DataFusion does over the value the
//! column emits — and `Unsupported` everywhere else, DataFusion then filtering
//! the rows itself. So a pushed filter and a kept one answer alike, and a
//! query's rows do not depend on which its plan chose.

use datafusion::logical_expr::expr::InList;
use datafusion::logical_expr::{Between, BinaryExpr, Expr, Operator};
use pgdt_codec::ResolvedSchema;
use pgdt_query::{Membership, Predicate, PredicateOp};

use super::{boolean_term, boolean_text, column_index, compared, is_float, member, null_term};
use crate::unrepresentable::UNREPRESENTABLE_FUNCTION;

/// `filter` as the library's tree over `table`'s columns, or `None` where some
/// part of it has no library term that answers as DataFusion does.
///
/// What translates: `AND`, `OR`, `NOT`; a column compared with a literal by
/// any of the eight comparing operators, either side first; `IS [NOT] NULL`;
/// `BETWEEN`, which DataFusion evaluates as the two comparisons; `IN` over a
/// non-empty list of literals, a `NULL` among them, as one membership, but
/// for a float column;
/// a boolean column standing alone or under `IS [NOT] TRUE|FALSE|UNKNOWN`;
/// and `pgdump_unrepresentable` over a bare column, any type, nested included,
/// which only this translation answers ([`crate::unrepresentable`]). A column
/// wrapped in a cast, a function or an arithmetic expression does not.
pub(crate) fn translate(filter: &Expr, table: &ResolvedSchema) -> Option<pgdt_query::Expr> {
    use pgdt_query::Expr as L;
    Some(match filter {
        Expr::BinaryExpr(BinaryExpr { left, op: Operator::And, right }) => {
            L::And(vec![translate(left, table)?, translate(right, table)?])
        }
        Expr::BinaryExpr(BinaryExpr { left, op: Operator::Or, right }) => {
            L::Or(vec![translate(left, table)?, translate(right, table)?])
        }
        Expr::BinaryExpr(BinaryExpr { left, op, right }) => {
            L::Term(comparison(left, *op, right, table)?)
        }
        Expr::Not(inner) => L::Not(Box::new(translate(inner, table)?)),
        Expr::IsNull(inner) => L::Term(null_test(inner, PredicateOp::IsNull, table)?),
        Expr::IsNotNull(inner) => L::Term(null_test(inner, PredicateOp::IsNotNull, table)?),
        Expr::IsUnknown(inner) => L::Term(boolean(inner, PredicateOp::IsNull, table)?),
        Expr::IsNotUnknown(inner) => L::Term(boolean(inner, PredicateOp::IsNotNull, table)?),
        Expr::IsTrue(inner) => {
            L::Term(boolean(inner, PredicateOp::IsNotDistinctFrom(boolean_text(true)), table)?)
        }
        Expr::IsFalse(inner) => {
            L::Term(boolean(inner, PredicateOp::IsNotDistinctFrom(boolean_text(false)), table)?)
        }
        Expr::IsNotTrue(inner) => {
            L::Term(boolean(inner, PredicateOp::IsDistinctFrom(boolean_text(true)), table)?)
        }
        Expr::IsNotFalse(inner) => {
            L::Term(boolean(inner, PredicateOp::IsDistinctFrom(boolean_text(false)), table)?)
        }
        Expr::ScalarFunction(call) if call.name() == UNREPRESENTABLE_FUNCTION => {
            let [Expr::Column(column)] = call.args.as_slice() else { return None };
            let index = table.schema.index_of(&column.name).ok()?;
            L::Term(null_term(index, PredicateOp::IsUnrepresentable, table))
        }
        // A boolean column as the whole predicate is its own truth: `flag` is
        // `flag = true` in three values, a NULL unknown under both.
        Expr::Column(_) => L::Term(boolean(filter, PredicateOp::Eq(boolean_text(true)), table)?),
        Expr::Between(Between { expr, negated, low, high }) => {
            let both = L::And(vec![
                L::Term(comparison(expr, Operator::GtEq, low, table)?),
                L::Term(comparison(expr, Operator::LtEq, high, table)?),
            ]);
            if *negated { L::Not(Box::new(both)) } else { both }
        }
        Expr::InList(InList { expr, list, negated }) => {
            // DataFusion answers a long list from a set of the values rather
            // than through its comparison ([`is_float`]).
            let index = column_index(expr, table)?;
            if list.is_empty() || is_float(index, table) {
                return None;
            }
            let values = list
                .iter()
                .map(|value| match value {
                    Expr::Literal(value, _) => member(index, value, table),
                    _ => None,
                })
                .collect::<Option<_>>()?;
            let any =
                L::In(Membership { column: table.schema.field(index).name().clone(), values });
            if *negated { L::Not(Box::new(any)) } else { any }
        }
        _ => return None,
    })
}

/// `left op right`, one side a column of `table` and the other a literal.
fn comparison(
    left: &Expr,
    op: Operator,
    right: &Expr,
    table: &ResolvedSchema,
) -> Option<Predicate> {
    let (column, op, literal) = match (left, right) {
        (Expr::Column(_), Expr::Literal(value, _)) => (left, op, value),
        (Expr::Literal(value, _), Expr::Column(_)) => (right, op.swap()?, value),
        _ => return None,
    };
    compared(column_index(column, table)?, op, literal, table)
}

/// `IS NULL` or `IS NOT NULL` on a column of `table`, any type, nested
/// included: neither reads a value.
fn null_test(column: &Expr, op: PredicateOp, table: &ResolvedSchema) -> Option<Predicate> {
    let Expr::Column(column) = column else { return None };
    Some(null_term(table.schema.index_of(&column.name).ok()?, op, table))
}

/// `op` on a `Boolean` column of `table`.
fn boolean(column: &Expr, op: PredicateOp, table: &ResolvedSchema) -> Option<Predicate> {
    boolean_term(column_index(column, table)?, op, table)
}
