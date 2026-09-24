//! The map's statistics as DataFusion's, so `COUNT(*)`, `COUNT(<column>)`,
//! `MIN` and `MAX` can be answered without reading a row
//! (`docs/design/decisions.md`, "D89").
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

use arrow::datatypes::DataType;
use datafusion::common::stats::Precision;
use datafusion::common::{ColumnStatistics, ScalarValue, Statistics};
use pgdump_query::{ComparisonSemantics, DumpIndex, ResolvedSchema, TableName};

/// What the dump's map says about `table`, in the schema `resolved` states and
/// in DataFusion's own comparison semantics — the order the emitted values
/// carry, which is the order a `MIN` or `MAX` of them would be taken in.
pub(crate) fn table_statistics(
    index: &DumpIndex,
    table: &TableName,
    resolved: &ResolvedSchema,
) -> Statistics {
    let summary = pgdump_query::table_summary(index, table, resolved, ComparisonSemantics::Arrow);
    let column_statistics = summary
        .columns
        .iter()
        .enumerate()
        .map(|(i, column)| {
            let mut statistics = ColumnStatistics::new_unknown();
            statistics.null_count = count(column.nulls, column.nulls_complete);
            // deficiency: KD45 — an enum is emitted `Dictionary`, and
            // DataFusion types a `MIN` or `MAX` of one as the dictionary's value
            // type, not the column's (`get_min_max_result_type`), so a statistic
            // in the column's own type would be a literal of the wrong type in
            // a plan that no longer checks its schema; and it orders the labels
            // as text, where the stored bounds are in declaration order, so no
            // bound here is DataFusion's extreme. **(b) owned by P27**, whose
            // dynamic filters compare an enum in that label order too; the fix
            // is bounds kept in it, handed over in the value type.
            if matches!(resolved.schema.field(i).data_type(), DataType::Dictionary(..)) {
                return statistics;
            }
            // deficiency: KD42 — a float's stored bound is gathered in
            // PostgreSQL's order, where `-0` and `0` tie and the first seen
            // stands, while DataFusion's `MIN`/`MAX` over a float column orders
            // by `total_cmp`, `-0` below `0`. So a column holding both zeros
            // can be handed an `Exact` minimum of `0` where reading it answers
            // `-0`, or a maximum of `-0` where it answers `0`: a wrong answer
            // with no error. The same tie lets a float block be recorded
            // sorted where `total_cmp` would not call it so, so no ordering
            // can be declared from it. **(b) owned by P25**, the fix being the
            // zero's sign kept as `total_cmp` would at the extremes and in the
            // order recorded.
            statistics.min_value = bound(column.min.as_ref(), column.bounds_complete);
            statistics.max_value = bound(column.max.as_ref(), column.bounds_complete);
            statistics
        })
        .collect();
    Statistics {
        // Every block of a complete map carries its row count, whether or not
        // it gathered statistics.
        num_rows: Precision::Exact(summary.rows as usize),
        // The Arrow bytes a scan produces, which nothing here measures.
        total_byte_size: Precision::Absent,
        column_statistics,
    }
}

fn count(value: u64, complete: bool) -> Precision<usize> {
    let Ok(value) = usize::try_from(value) else { return Precision::Absent };
    if complete { Precision::Exact(value) } else { Precision::Inexact(value) }
}

/// One end of a column's range, `Exact` only where it is the value the column
/// holds and the summary covers every group.
///
/// deficiency: KD39 — two cases never reach `Exact`, so `MIN` or `MAX` over
/// them is read rather than answered. **(b) owned by P25.** **A text-ordered column's lower bound**
/// is stored clipped to `DICTIONARY_ENTRY_MAX_BYTES` where the value is
/// longer, and the stored shape records that of the upper bound alone
/// (`statistics::Bounds::max_exact`), so a lower bound is never known to be
/// the value: the fix is a flag beside it and a `CACHE_FORMAT_VERSION` bump,
/// and it costs `MIN` on every `Utf8View` and `Binary` column. **A
/// `character(n)`'s** own set is stored unpadded, so under PostgreSQL's
/// semantics — which the provider does not ask for — it is not a value the
/// column emits; its Arrow set is the padded text and is unaffected. An
/// enum's is refused above (`KD45`).
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
