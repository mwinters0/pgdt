//! DataFusion's filters as the library's filter tree, for the filters a scan
//! can answer itself.
//!
//! **A filter is pushed `Exact` or not at all.** It is `Exact` where it
//! translates here and the library's plan resolves it in
//! [`ComparisonSemantics::DataFusion`](pgdump_query::ComparisonSemantics::DataFusion) —
//! which is where the library answers as DataFusion does over the value the
//! column emits — and `Unsupported` everywhere else, DataFusion then filtering
//! the rows itself. So a pushed filter and a kept one answer alike, and a
//! query's rows do not depend on which its plan chose.
//!
//! **A literal reaches the library as the text the library's own renderer
//! writes for it** ([`pgdump_query::render_field`]), the exact inverse of the
//! decoder that reads it back, so the library compares the very value
//! DataFusion holds; `docs/design/decisions.md`, "D88".

use arrow::datatypes::DataType;
use datafusion::common::ScalarValue;
use datafusion::logical_expr::expr::InList;
use datafusion::logical_expr::{Between, BinaryExpr, Expr, Operator};
use pgdump_query::{
    ColumnResolution, CompareKind, ComparisonPlan, Membership, NestedPlan, Predicate, PredicateOp,
    ResolvedSchema,
};

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
pub(crate) fn translate(filter: &Expr, table: &ResolvedSchema) -> Option<pgdump_query::Expr> {
    use pgdump_query::Expr as L;
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
        Expr::IsUnknown(inner) => L::Term(boolean(inner, PredicateOp::IsNull, None, table)?),
        Expr::IsNotUnknown(inner) => L::Term(boolean(inner, PredicateOp::IsNotNull, None, table)?),
        Expr::IsTrue(inner) => {
            L::Term(boolean(inner, PredicateOp::IsNotDistinctFrom, Some(true), table)?)
        }
        Expr::IsFalse(inner) => {
            L::Term(boolean(inner, PredicateOp::IsNotDistinctFrom, Some(false), table)?)
        }
        Expr::IsNotTrue(inner) => {
            L::Term(boolean(inner, PredicateOp::IsDistinctFrom, Some(true), table)?)
        }
        Expr::IsNotFalse(inner) => {
            L::Term(boolean(inner, PredicateOp::IsDistinctFrom, Some(false), table)?)
        }
        Expr::ScalarFunction(call) if call.name() == UNREPRESENTABLE_FUNCTION => {
            let [Expr::Column(column)] = call.args.as_slice() else { return None };
            let index = table.schema.index_of(&column.name).ok()?;
            L::Term(null_term(index, PredicateOp::IsUnrepresentable, table))
        }
        // A boolean column as the whole predicate is its own truth: `flag` is
        // `flag = true` in three values, a NULL unknown under both.
        Expr::Column(_) => L::Term(boolean(filter, PredicateOp::Eq, Some(true), table)?),
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

/// Column `index` of `table` under `op` against `literal`, the column first:
/// `None` where `op` is no comparing operator, the column is nested, or no
/// text reads back as `literal` ([`literal_text`]).
pub(crate) fn compared(
    index: usize,
    op: Operator,
    literal: &ScalarValue,
    table: &ResolvedSchema,
) -> Option<Predicate> {
    let op = match op {
        Operator::Eq => PredicateOp::Eq,
        Operator::NotEq => PredicateOp::Ne,
        Operator::Lt => PredicateOp::Lt,
        Operator::LtEq => PredicateOp::Le,
        Operator::Gt => PredicateOp::Gt,
        Operator::GtEq => PredicateOp::Ge,
        Operator::IsDistinctFrom => PredicateOp::IsDistinctFrom,
        Operator::IsNotDistinctFrom => PredicateOp::IsNotDistinctFrom,
        _ => return None,
    };
    if !table.plans[index].is_scalar() {
        return None;
    }
    let value = literal_text(literal, index, table)?;
    Some(Predicate { column: table.schema.field(index).name().clone(), op, value: Some(value) })
}

/// `literal` as one value of an `IN` list over column `index` of `table`:
/// `Some(None)` for a `NULL`, which the library's membership reads as SQL's,
/// and `None` where no text reads back as DataFusion's value
/// ([`literal_text`]).
pub(crate) fn member(
    index: usize,
    literal: &ScalarValue,
    table: &ResolvedSchema,
) -> Option<Option<String>> {
    if literal.is_null() {
        return Some(None);
    }
    literal_text(literal, index, table).map(Some)
}

/// `IS NULL` or `IS NOT NULL` on a column of `table`, any type, nested
/// included: neither reads a value.
fn null_test(column: &Expr, op: PredicateOp, table: &ResolvedSchema) -> Option<Predicate> {
    let Expr::Column(column) = column else { return None };
    Some(null_term(table.schema.index_of(&column.name).ok()?, op, table))
}

/// `op`, one of the operators taking no value
/// ([`PredicateOp::takes_no_value`]), on column `index` of `table`.
pub(crate) fn null_term(index: usize, op: PredicateOp, table: &ResolvedSchema) -> Predicate {
    Predicate { column: table.schema.field(index).name().clone(), op, value: None }
}

/// A term on a `Boolean` column of `table` against `value`, spelled as the
/// file spells a boolean.
fn boolean(
    column: &Expr,
    op: PredicateOp,
    value: Option<bool>,
    table: &ResolvedSchema,
) -> Option<Predicate> {
    boolean_term(column_index(column, table)?, op, value, table)
}

/// [`boolean`] on column `index` of `table`: `None` unless it is `Boolean`.
pub(crate) fn boolean_term(
    index: usize,
    op: PredicateOp,
    value: Option<bool>,
    table: &ResolvedSchema,
) -> Option<Predicate> {
    if table.schema.field(index).data_type() != &DataType::Boolean {
        return None;
    }
    let value = value.map(|v| if v { "t" } else { "f" }.to_string());
    Some(Predicate { column: table.schema.field(index).name().clone(), op, value })
}

/// Where `expr`, a bare column, sits in `table` — `None` for anything else,
/// and for a nested column, whose comparisons DataFusion semantics refuses.
fn column_index(expr: &Expr, table: &ResolvedSchema) -> Option<usize> {
    let Expr::Column(column) = expr else { return None };
    let index = table.schema.index_of(&column.name).ok()?;
    table.plans[index].is_scalar().then_some(index)
}

/// Whether column `index` of `table` is a float, whose `IN` DataFusion
/// answers from a set of the values' bits rather than through `=`, so `-0`
/// and `0` stay apart there and nowhere else.
pub(crate) fn is_float(index: usize, table: &ResolvedSchema) -> bool {
    matches!(table.schema.field(index).data_type(), DataType::Float32 | DataType::Float64)
}

/// The text the library reads `literal` from, for comparison with column
/// `index` of `table`: `None` where no text reads back as DataFusion's value.
///
/// **A string literal against a column emitted as text is passed as it
/// stands**, and only where the library compares that column's text bytewise,
/// as DataFusion compares the emitted string — every text-emitted kind.
/// **Any other literal must be of the column's own Arrow type**, and is
/// rendered back into the column's text by
/// the library, so a cast DataFusion wrapped around the column, or one it left
/// on the literal, is not pushed.
fn literal_text(literal: &ScalarValue, index: usize, table: &ResolvedSchema) -> Option<String> {
    if literal.is_null() {
        return None;
    }
    let column_type = table.schema.field(index).data_type();
    if let Some(text) = string_value(literal) {
        let emitted_as_text = match column_type {
            DataType::Utf8View => true,
            DataType::Dictionary(_, value) => value.as_ref() == &DataType::Utf8,
            _ => false,
        };
        return (emitted_as_text && compared_as_text(table, index)).then(|| text.to_string());
    }
    if &literal.data_type() != column_type {
        return None;
    }
    // Where more than one Arrow value renders to the same text, only the one
    // the decoder returns reads back: DataFusion's order tells `NaN`s apart by
    // sign and payload, and the file holds the one `NaN` its decoder makes.
    let reads_back = match literal {
        ScalarValue::Float32(Some(v)) => !v.is_nan() || v.to_bits() == f32::NAN.to_bits(),
        ScalarValue::Float64(Some(v)) => !v.is_nan() || v.to_bits() == f64::NAN.to_bits(),
        // The renderer writes a time of day; a negative one has none.
        ScalarValue::Time64Microsecond(Some(v)) => *v >= 0,
        _ => true,
    };
    if !reads_back {
        return None;
    }
    let array = literal.to_array().ok()?;
    pgdump_query::render_field(&array, 0, &NestedPlan::Scalar).ok().flatten()
}

/// A string literal's text, bare or as a dictionary's value.
fn string_value(literal: &ScalarValue) -> Option<&str> {
    match literal {
        ScalarValue::Utf8(Some(s))
        | ScalarValue::Utf8View(Some(s))
        | ScalarValue::LargeUtf8(Some(s)) => Some(s),
        ScalarValue::Dictionary(_, value) => string_value(value),
        _ => None,
    }
}

/// Whether the library compares column `index` of `table` bytewise over its
/// text in DataFusion semantics: a column with no plan, and every kind DataFusion
/// semantics moves to text.
fn compared_as_text(table: &ResolvedSchema, index: usize) -> bool {
    if table.columns[index] != ColumnResolution::Mapped {
        return true;
    }
    match &table.comparisons[index] {
        ComparisonPlan::Compared { kind, .. } => kind.datafusion_order() == CompareKind::Text,
        ComparisonPlan::Refused => true,
        _ => false,
    }
}
