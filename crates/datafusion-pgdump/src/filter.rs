//! DataFusion's expressions read as the library's filter tree: a filter
//! pushed into a scan, translated exactly or not at all ([`pushdown`]), and a
//! dynamic filter a scan holds, translated loosened ([`dynamic`]). Both build
//! their terms from the leaves here, and neither reads the other.
//!
//! **A literal reaches the library as the text the library's own renderer
//! writes for it** ([`pgdt_query::render_field`]), the exact inverse of the
//! decoder that reads it back, so the library compares the very value
//! DataFusion holds; `docs/design/decisions.md`, "D88".

mod dynamic;
mod pushdown;

use arrow::datatypes::DataType;
use datafusion::common::ScalarValue;
use datafusion::logical_expr::{Expr, Operator};
use pgdt_codec::{ColumnResolution, ResolvedSchema, compare::CompareKind, compare::ComparisonPlan};
use pgdt_query::{Predicate, PredicateOp};

pub(crate) use dynamic::{DynamicFilters, ReplayFilter};
pub(crate) use pushdown::translate;

/// Column `index` of `table` under `op` against `literal`, the column first:
/// `None` where `op` is no comparing operator, the column is nested, or no
/// text reads back as `literal` ([`literal_text`]).
fn compared(
    index: usize,
    op: Operator,
    literal: &ScalarValue,
    table: &ResolvedSchema,
) -> Option<Predicate> {
    let op: fn(String) -> PredicateOp = match op {
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
    if !table.columns()[index].plan().is_scalar() {
        return None;
    }
    let value = literal_text(literal, index, table)?;
    Some(Predicate { column: table.schema().field(index).name().clone(), op: op(value) })
}

/// `literal` as one value of an `IN` list over column `index` of `table`:
/// `Some(None)` for a `NULL`, which the library's membership reads as SQL's,
/// and `None` where no text reads back as DataFusion's value
/// ([`literal_text`]).
fn member(index: usize, literal: &ScalarValue, table: &ResolvedSchema) -> Option<Option<String>> {
    if literal.is_null() {
        return Some(None);
    }
    literal_text(literal, index, table).map(Some)
}

/// `op` on column `index` of `table`, whatever its type — the NULL and
/// unrepresentable tests read no value.
fn null_term(index: usize, op: PredicateOp, table: &ResolvedSchema) -> Predicate {
    Predicate { column: table.schema().field(index).name().clone(), op }
}

/// `op` on column `index` of `table`: `None` unless the column is `Boolean`.
/// A value `op` carries is spelled by [`boolean_text`].
fn boolean_term(index: usize, op: PredicateOp, table: &ResolvedSchema) -> Option<Predicate> {
    if table.schema().field(index).data_type() != &DataType::Boolean {
        return None;
    }
    Some(null_term(index, op, table))
}

/// `value` spelled as the file spells a boolean.
fn boolean_text(value: bool) -> String {
    if value { "t" } else { "f" }.to_string()
}

/// Whether column `index` of `table` is a float, whose `IN` DataFusion
/// answers from a set of the values' bits rather than through `=`, so `-0`
/// and `0` stay apart there and nowhere else.
fn is_float(index: usize, table: &ResolvedSchema) -> bool {
    matches!(table.schema().field(index).data_type(), DataType::Float32 | DataType::Float64)
}

/// Where `expr`, a bare column, sits in `table` — `None` for anything else,
/// and for a nested column, whose comparisons DataFusion semantics refuses.
fn column_index(expr: &Expr, table: &ResolvedSchema) -> Option<usize> {
    let Expr::Column(column) = expr else { return None };
    let index = table.schema().index_of(&column.name).ok()?;
    table.columns()[index].plan().is_scalar().then_some(index)
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
    let column_type = table.schema().field(index).data_type();
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
    pgdt_query::render_field(&array, 0, &table.columns()[index]).ok().flatten()
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
    if *table.columns()[index].resolution() != ColumnResolution::Mapped {
        return true;
    }
    match table.columns()[index].comparison() {
        ComparisonPlan::Compared { kind, .. } => {
            matches!(kind.datafusion_order(), CompareKind::Text { .. })
        }
        ComparisonPlan::Refused => true,
        _ => false,
    }
}
