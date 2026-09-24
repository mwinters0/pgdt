//! The map's statistics as DataFusion's, so `COUNT(*)`, `COUNT(<column>)`,
//! `MIN`, `MAX`, `SUM` and a `COUNT(DISTINCT <column>)` beside another count
//! can be answered without reading a row (`docs/design/decisions.md`, "D89"), a join's sides can be
//! weighed by the bytes each emits ([`byte_size`]), and a sort the scan's
//! recorded order already satisfies is not planned ([`output_orderings`]).
//!
//! **`Exact` is a promise, and a wrong one is a wrong answer with no error**:
//! DataFusion's `AggregateStatistics` rule replaces the aggregate with the
//! literal it finds, and reads nothing but `Exact`. So a bound reaches
//! `Exact` only where the summary covers every group of every block *and* the
//! stored bound is the value it came from; everything else it can still bound
//! is `Inexact`, which no answer is read from.
//!
//! **An answer from here does not raise what reading the column would.** A
//! column holding a value its Arrow type cannot represent (`KD8`) refuses on
//! the row that holds it, while a NULL count read off the text and a bound
//! that decodes both describe the column truthfully — so a `COUNT` of such a
//! column answers here and refuses when read (`docs/design/decisions.md`,
//! "D89").

use std::sync::Arc;

use arrow::compute::SortOptions;
use arrow::datatypes::{DataType, Schema};
use datafusion::common::stats::Precision;
use datafusion::common::{ColumnStatistics, ScalarValue, Statistics};
use datafusion::physical_expr::expressions::Column;
use datafusion::physical_expr::{LexOrdering, PhysicalSortExpr};
use pgdump_query::{ComparisonSemantics, DumpIndex, ResolvedSchema, Sortedness, TableName};

/// What the dump's map says about `table`, in the schema `resolved` states and
/// in DataFusion's own comparison semantics — the order the emitted values
/// carry, which is the order a `MIN` or `MAX` of them would be taken in.
pub(crate) fn table_statistics(
    index: &DumpIndex,
    table: &TableName,
    resolved: &ResolvedSchema,
) -> Statistics {
    let summary = pgdump_query::table_summary(index, table, resolved, ComparisonSemantics::Arrow);
    // Every block of a complete map carries its row count, whether or not it
    // gathered statistics.
    let rows = Precision::Exact(summary.rows as usize);
    let column_statistics: Vec<ColumnStatistics> = summary
        .columns
        .iter()
        .enumerate()
        .map(|(i, column)| {
            let data_type = resolved.schema.field(i).data_type();
            let mut statistics = ColumnStatistics::new_unknown();
            statistics.null_count = count(column.nulls, column.nulls_complete);
            // Never the union's size where a group kept no dictionary: that
            // is only a floor, and a low distinct count inflates a join's
            // estimate.
            statistics.distinct_count = column
                .distinct
                .and_then(|distinct| usize::try_from(distinct).ok())
                .map_or(Precision::Absent, Precision::Exact);
            statistics.byte_size = byte_size(data_type, rows, column.value_bytes);
            // deficiency: KD45 — an enum is emitted `Dictionary`, and
            // DataFusion types a `MIN` or `MAX` of one as the dictionary's value
            // type, not the column's (`get_min_max_result_type`), so a statistic
            // in the column's own type would be a literal of the wrong type in
            // a plan that no longer checks its schema; and it orders the labels
            // as text, where the stored bounds are in declaration order, so no
            // bound here is DataFusion's extreme. **(b) owned by P27**, whose
            // dynamic filters compare an enum in that label order too; the fix
            // is bounds kept in it, handed over in the value type.
            if matches!(data_type, DataType::Dictionary(..)) {
                return statistics;
            }
            statistics.min_value = bound(column.min.as_ref(), column.bounds_complete);
            statistics.max_value = bound(column.max.as_ref(), column.bounds_complete);
            statistics.sum_value = sum(column.sum, data_type);
            statistics
        })
        .collect();
    let total_byte_size = total_byte_size(&column_statistics);
    Statistics { num_rows: rows, total_byte_size, column_statistics }
}

/// A column's sum as DataFusion's `SUM` would answer it: a 16-, 32- or 64-bit
/// integer's wrapped at 64 bits, as the `Int64` its argument is cast to wraps,
/// an `oid`'s at 64 unsigned ones, and a `Decimal128`'s at 128, in the
/// column's own type, which `SUM` widens without checking the value — arrow's
/// same-scale decimal cast does not validate, and the blind session in
/// `tests/statistics.rs` fails on `public.spans.huge` once it does. Each
/// narrows the library's 128-bit wrapping sum exactly
/// (`docs/design/decisions.md`, "D91").
fn sum(sum: Option<i128>, data_type: &DataType) -> Precision<ScalarValue> {
    let Some(sum) = sum else { return Precision::Absent };
    match data_type {
        DataType::Int16 | DataType::Int32 | DataType::Int64 => {
            Precision::Exact(ScalarValue::Int64(Some(sum as i64)))
        }
        DataType::UInt32 => Precision::Exact(ScalarValue::UInt64(Some(sum as u64))),
        DataType::Decimal128(precision, scale) => {
            Precision::Exact(ScalarValue::Decimal128(Some(sum), *precision, *scale))
        }
        _ => Precision::Absent,
    }
}

/// The Arrow bytes a scan emits of a column of `data_type` over `rows` rows,
/// its values' text being `value_bytes` long (`docs/design/decisions.md`,
/// "D91"):
///
/// - its width for every row of a fixed-width type, and a bit a row of a
///   boolean, exact where the rows are;
/// - the text's length for a `Utf8View`, the bytes its views point at, and for
///   a `Binary` a bound above its decoded bytes, `Inexact` both, a view's
///   bytes living in a buffer the batches share (`docs/design/decisions.md`,
///   "D46");
/// - for an enum's `Dictionary`, its keys' width a row and its values' text,
///   which bounds the labels each batch's dictionary holds, `Inexact` — true
///   while `StringDictionaryBuilder` keeps only the labels appended since its
///   last `finish`, which the estimate target in `tests/statistics.rs`
///   measures batch by batch;
/// - and `Absent` for a nested type, whose text bounds nothing of its leaves'.
pub(crate) fn byte_size(
    data_type: &DataType,
    rows: Precision<usize>,
    value_bytes: Option<u64>,
) -> Precision<usize> {
    let text = value_bytes.and_then(|bytes| usize::try_from(bytes).ok());
    match data_type {
        DataType::Boolean => per_row(rows, |rows| Some(rows.div_ceil(8))),
        DataType::FixedSizeBinary(width) => {
            per_row(rows, |rows| rows.checked_mul(usize::try_from(*width).ok()?))
        }
        DataType::Utf8View | DataType::Binary => text.map_or(Precision::Absent, Precision::Inexact),
        DataType::Dictionary(key, _) => {
            let keys = rows.get_value().zip(key.primitive_width());
            let keys = keys.and_then(|(rows, width)| rows.checked_mul(width));
            keys.zip(text)
                .and_then(|(keys, text)| keys.checked_add(text))
                .map_or(Precision::Absent, Precision::Inexact)
        }
        _ => match data_type.primitive_width() {
            Some(width) => per_row(rows, |rows| rows.checked_mul(width)),
            None => Precision::Absent,
        },
    }
}

/// `bytes` of `rows`, as exact as the rows are.
fn per_row(rows: Precision<usize>, bytes: impl Fn(usize) -> Option<usize>) -> Precision<usize> {
    match rows {
        Precision::Exact(rows) => bytes(rows).map_or(Precision::Absent, Precision::Exact),
        Precision::Inexact(rows) => bytes(rows).map_or(Precision::Absent, Precision::Inexact),
        Precision::Absent => Precision::Absent,
    }
}

/// The bytes a scan emits of every column `columns` describes: their sum,
/// `Exact` only where every one is, and `Absent` where any is.
pub(crate) fn total_byte_size(columns: &[ColumnStatistics]) -> Precision<usize> {
    let exact = Precision::Exact(0usize);
    columns
        .iter()
        .try_fold(exact, |total, column| match (total, column.byte_size) {
            (_, Precision::Absent) => None,
            (Precision::Exact(a), Precision::Exact(b)) => a.checked_add(b).map(Precision::Exact),
            (total, column) => {
                let (a, b) = (*total.get_value()?, *column.get_value()?);
                a.checked_add(b).map(Precision::Inexact)
            }
        })
        .unwrap_or(Precision::Absent)
}

fn count(value: u64, complete: bool) -> Precision<usize> {
    let Ok(value) = usize::try_from(value) else { return Precision::Absent };
    if complete { Precision::Exact(value) } else { Precision::Inexact(value) }
}

/// One end of a column's range, `Exact` only where it is the value the column
/// holds and the summary covers every group: a text or `bytea` extreme
/// clipped to `DICTIONARY_ENTRY_MAX_BYTES` says so, and is read rather than
/// answered. Where a float's extreme is a zero, it is the one `total_cmp`,
/// which DataFusion's `MIN`/`MAX` orders by, picks of those the column holds.
///
/// A bound is withheld where it is itself a value the Arrow type cannot hold:
/// it does not decode, and the module's note above says what that leaves.
fn bound(bound: Option<&pgdump_query::Bound>, complete: bool) -> Precision<ScalarValue> {
    let Some(bound) = bound else { return Precision::Absent };
    let Ok(value) = ScalarValue::try_from_array(&bound.value, 0) else {
        return Precision::Absent;
    };
    if value.is_null() {
        return Precision::Absent;
    }
    if complete && bound.exact { Precision::Exact(value) } else { Precision::Inexact(value) }
}

/// The orderings a scan declares, from the order the library proved each
/// projected column's partitions emit it in (`TablePartitions::orders`), so
/// a sort DataFusion would otherwise plan over the scan is dropped.
///
/// **Each proved column is its own ordering, with its NULLs where SQL puts
/// them by default** — last ascending, first descending. A proved column
/// holds none, so either placement is true of it; but DataFusion keeps one
/// order per expression and matches a nullable column's placement exactly,
/// so only one can be declared, and it is the one an unqualified `ORDER BY`
/// asks for.
pub(crate) fn output_orderings(schema: &Schema, orders: &[Sortedness]) -> Vec<LexOrdering> {
    orders
        .iter()
        .enumerate()
        .filter_map(|(i, order)| {
            let descending = match order {
                Sortedness::Ascending => false,
                Sortedness::Descending => true,
                Sortedness::Unsorted => return None,
            };
            let column = Arc::new(Column::new(schema.field(i).name(), i));
            let options = SortOptions { descending, nulls_first: descending };
            LexOrdering::new([PhysicalSortExpr::new(column, options)])
        })
        .collect()
}
