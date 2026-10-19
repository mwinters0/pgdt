//! An Arrow value read back as PostgreSQL text, and one field's text read
//! into the array a batch of its column would hold
//! (`docs/design/decisions.md`, "D44", "D66").

use arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array, Decimal256Array,
    DictionaryArray, FixedSizeBinaryArray, Float32Array, Float64Array, Int16Array, Int32Array,
    Int64Array, IntervalMonthDayNanoArray, ListArray, StringArray, StringViewArray, StructArray,
    Time64MicrosecondArray, TimestampMicrosecondArray, UInt32Array,
};
use arrow::datatypes::{DataType, Int32Type, IntervalUnit, TimeUnit};

use pgdt_codec::ResolvedColumn;
use pgdt_codec::decode;
use pgdt_codec::internal::{push_array_null, push_integer, quote_array_element};
use pgdt_codec::nested::{
    RangeLiteral, RecordLiteral, render_int2vector, render_range, render_record,
};
use pgdt_codec::pgtype::NestedPlan;
use pgdt_core::{Error, Result};
use pgdt_model::PostgresInvalidValues;

use crate::batch::{append_typed, finish_column, new_column_builder};

/// Render one row of `array`, a batch's array of `column`, back to the same
/// PostgreSQL text form `pgdt_core::unescape_field` would have produced for
/// it (`docs/design/decisions.md`, "D66"), and the round-trip tests' oracle.
/// `Ok(None)` for SQL NULL. Covers exactly the [`DataType`]s
/// [`pgdt_codec::resolve_columns`] can ever produce.
///
/// **The `Result` is for a value with no text form at all**, which is a third
/// outcome and not a NULL: [`Error::FieldRender`] says which, and a value is
/// never rounded into range (`docs/design/decisions.md`, "D44"). Nothing this
/// crate builds can reach it, so it is a statement about arrays a caller
/// assembled itself.
///
/// `column` is needed for the same reason
/// [`ColumnBuilder`](crate::batch::ColumnBuilder) needs it: the Arrow type
/// does not say which literal form a nested value is written in, and
/// `int4range[]` and `int4multirange` share one. Its
/// [`ResolvedColumn::plan`] says; there is no plan-less entry point
/// (`docs/design/decisions.md`, "D43").
pub fn render_field(
    array: &dyn Array,
    row: usize,
    column: &ResolvedColumn,
) -> Result<Option<String>> {
    render_plan(array, row, column.plan())
}

/// [`render_field`] over a plan, which each nested level recurses through.
pub(crate) fn render_plan(
    column: &dyn Array,
    row: usize,
    plan: &NestedPlan,
) -> Result<Option<String>> {
    // Checked here rather than left to the sink so a SQL NULL costs no
    // allocation at all.
    if column.is_null(row) {
        return Ok(None);
    }
    // Sized rather than empty: a zero-capacity `String` is grown by whichever
    // `push_str` writes into it first, where one allocation suffices for a
    // value of 16 bytes or fewer.
    let mut out = String::with_capacity(16);
    if render_plan_into(column, row, plan, &mut out)? { Ok(Some(out)) } else { Ok(None) }
}

/// [`render_field`]'s inverse for one field of `column`: `text`, unescaped as
/// a row's field arrives, as the one-element array a batch of the column
/// would hold — or `None` where the text is not a value of the column's
/// type, which is [`Error::FieldDecode`]'s condition without the row that
/// raises it, or is one its Arrow type cannot hold, which a scan reads as
/// NULL.
///
/// It exists so that a caller holding field text out of band — a stored bound
/// ([`pgdt_model::Bounds`]) — reads it back through the one decoder,
/// rather than a second one that could disagree with what a scan emits.
pub fn decode_field(column: &ResolvedColumn, text: &str) -> Option<ArrayRef> {
    let mut builder = new_column_builder(column.data_type(), column.plan());
    append_typed(&mut builder, text, PostgresInvalidValues::Default).ok()?;
    Some(finish_column(&mut builder))
}

/// [`render_field`] appending to a caller's buffer instead of returning one.
/// `Ok(false)` is SQL NULL and appends nothing; `Ok(true)` says the value was
/// written. This is the form that does the work — [`render_field`] is a
/// wrapper over it, so the two cannot drift.
///
/// It exists so that a consumer printing a whole row builds it in one buffer:
/// a scalar column of an integer, a boolean, a text or a date/time type other
/// than `interval` is written straight into that buffer and allocates nothing
/// (`docs/design/decisions.md`, "D44").
///
/// **An error may leave a partial value behind**, so the discipline this asks
/// for is not to publish a buffer an error came out of.
pub fn render_field_into(
    array: &dyn Array,
    row: usize,
    column: &ResolvedColumn,
    out: &mut String,
) -> Result<bool> {
    render_plan_into(array, row, column.plan(), out)
}

/// [`render_field_into`] over a plan, which each nested level recurses
/// through.
pub(crate) fn render_plan_into(
    column: &dyn Array,
    row: usize,
    plan: &NestedPlan,
    out: &mut String,
) -> Result<bool> {
    if column.is_null(row) {
        return Ok(false);
    }
    match plan {
        NestedPlan::Scalar | NestedPlan::Decimal { .. } | NestedPlan::Text { .. } => {}
        NestedPlan::Array(child) => {
            // Both are empty and neither allocates until it is used: `scratch`
            // only where an element needs quoting, `dims` only where the value
            // has a second dimension.
            let mut scratch = String::new();
            let mut dims = Vec::new();
            render_array_into(column, row, &child.plan, out, &mut scratch, &mut dims)?;
            return Ok(true);
        }
        NestedPlan::Multirange(bound) => {
            let list = column.as_any().downcast_ref::<ListArray>().unwrap();
            let members = list.value(row);
            let range = NestedPlan::Range(bound.clone());
            let rendered: Vec<String> = (0..members.len())
                .map(|i| {
                    Ok(render_plan(members.as_ref(), i, &range)?
                        .expect("a multirange's members are never SQL NULL"))
                })
                .collect::<Result<_>>()?;
            out.push('{');
            out.push_str(&rendered.join(","));
            out.push('}');
            return Ok(true);
        }
        // The second nested form whose elements cannot be NULL — a
        // multirange's members are the other, and panic above rather than
        // faulting. `int2vector` has no encoding for one, so a `List<Int16>`
        // holding a null element is an Arrow value with no PostgreSQL text
        // form — `Error::FieldRender`, exactly as a sub-microsecond
        // `interval` is, and reachable only from an array a caller assembled.
        NestedPlan::Int2Vector => {
            let list = column.as_any().downcast_ref::<ListArray>().unwrap();
            let values = list.value(row);
            let values = values.as_any().downcast_ref::<Int16Array>().unwrap();
            if values.null_count() != 0 {
                return Err(Error::FieldRender {
                    declared_type: "int2vector",
                    reason: "an int2vector has no encoding for a NULL element".to_string(),
                });
            }
            out.push_str(&render_int2vector(values.values()));
            return Ok(true);
        }
        NestedPlan::Record(field_plans) => {
            let s = column.as_any().downcast_ref::<StructArray>().unwrap();
            let fields: Vec<Option<String>> = s
                .columns()
                .iter()
                .zip(field_plans)
                .map(|(child, p)| render_plan(child.as_ref(), row, &p.plan))
                .collect::<Result<_>>()?;
            out.push_str(&render_record(&RecordLiteral { fields }));
            return Ok(true);
        }
        NestedPlan::Range(bound) => {
            let s = column.as_any().downcast_ref::<StructArray>().unwrap();
            let flag =
                |i: usize| s.column(i).as_any().downcast_ref::<BooleanArray>().unwrap().value(row);
            out.push_str(&render_range(&RangeLiteral {
                empty: flag(4),
                lower: render_plan(s.column(0).as_ref(), row, bound)?,
                upper: render_plan(s.column(1).as_ref(), row, bound)?,
                lower_inclusive: flag(2),
                upper_inclusive: flag(3),
            }));
            return Ok(true);
        }
    }
    match column.data_type() {
        DataType::Utf8View => {
            out.push_str(column.as_any().downcast_ref::<StringViewArray>().unwrap().value(row))
        }
        DataType::Boolean => out.push_str(decode::render_bool(
            column.as_any().downcast_ref::<BooleanArray>().unwrap().value(row),
        )),
        DataType::Int16 => {
            push_integer(
                out,
                i64::from(column.as_any().downcast_ref::<Int16Array>().unwrap().value(row)),
            );
        }
        DataType::Int32 => {
            push_integer(
                out,
                i64::from(column.as_any().downcast_ref::<Int32Array>().unwrap().value(row)),
            );
        }
        DataType::Int64 => {
            push_integer(out, column.as_any().downcast_ref::<Int64Array>().unwrap().value(row));
        }
        DataType::UInt32 => {
            push_integer(
                out,
                i64::from(column.as_any().downcast_ref::<UInt32Array>().unwrap().value(row)),
            );
        }
        DataType::Float32 => out.push_str(&decode::render_f32(
            column.as_any().downcast_ref::<Float32Array>().unwrap().value(row),
        )),
        DataType::Float64 => out.push_str(&decode::render_f64(
            column.as_any().downcast_ref::<Float64Array>().unwrap().value(row),
        )),
        DataType::Date32 => decode::render_date32_into(
            column.as_any().downcast_ref::<Date32Array>().unwrap().value(row),
            out,
        ),
        DataType::Timestamp(TimeUnit::Microsecond, tz) => {
            let v = column.as_any().downcast_ref::<TimestampMicrosecondArray>().unwrap().value(row);
            decode::render_timestamp_micros_into(v, tz.is_some(), out);
        }
        DataType::Time64(TimeUnit::Microsecond) => decode::render_time64_micros_into(
            column.as_any().downcast_ref::<Time64MicrosecondArray>().unwrap().value(row),
            out,
        ),
        DataType::Interval(IntervalUnit::MonthDayNano) => {
            let v = column.as_any().downcast_ref::<IntervalMonthDayNanoArray>().unwrap().value(row);
            let rendered = decode::render_interval(v.months, v.days, v.nanoseconds).ok_or_else(|| {
                Error::FieldRender {
                    declared_type: "interval",
                    reason: format!(
                        "a time part of {} ns is not a whole number of microseconds, which is the unit PostgreSQL's own field counts in",
                        v.nanoseconds
                    ),
                }
            })?;
            out.push_str(&rendered);
        }
        DataType::Decimal128(_, scale) => {
            let v = column.as_any().downcast_ref::<Decimal128Array>().unwrap().value(row);
            out.push_str(&decode::render_decimal(&v.to_string(), *scale));
        }
        DataType::Decimal256(_, scale) => {
            let v = column.as_any().downcast_ref::<Decimal256Array>().unwrap().value(row);
            out.push_str(&decode::render_decimal(&v.to_string(), *scale));
        }
        DataType::FixedSizeBinary(16) => {
            let bytes = column.as_any().downcast_ref::<FixedSizeBinaryArray>().unwrap().value(row);
            let bytes: &[u8; 16] =
                bytes.try_into().expect("FixedSizeBinary(16) is always 16 bytes");
            out.push_str(&decode::render_uuid(bytes));
        }
        DataType::Binary => out.push_str(&decode::render_bytea(
            column.as_any().downcast_ref::<BinaryArray>().unwrap().value(row),
        )),
        DataType::Dictionary(k, v) if **k == DataType::Int32 && **v == DataType::Utf8 => {
            let dict = column.as_any().downcast_ref::<DictionaryArray<Int32Type>>().unwrap();
            let values = dict.values().as_any().downcast_ref::<StringArray>().unwrap();
            out.push_str(values.value(dict.keys().value(row) as usize));
        }
        other => unreachable!("resolve_columns never resolves a column to {other:?}"),
    }
    Ok(true)
}

/// Write an `array_out` literal for a `List` value straight into `out`,
/// walking the Arrow list rather than building a [`pgdt_codec::nested::ArrayLiteral`].
///
/// Each element is rendered where it will be read, and
/// [`quote_array_element`] moves it aside only if the grammar wants
/// it quoted, so an `integer[]` of fifty is fifty appends and no allocation
/// (`docs/design/decisions.md`, "D44").
///
/// Lower bounds are always 1, so no `[lb:ub]=` prefix is ever written: a
/// decorated value is refused at append time (I21), and no column holds one to
/// render back.
fn render_array_into(
    column: &dyn Array,
    row: usize,
    child_plan: &NestedPlan,
    out: &mut String,
    scratch: &mut String,
    dims: &mut Vec<usize>,
) -> Result<()> {
    let mark = out.len();
    let mut leaves = 0usize;
    render_list_level(column, row, child_plan, 0, dims, out, scratch, &mut leaves)?;
    // `array_out` writes `{}` for a zero-element array whatever its
    // dimensionality (I20), so a `List<List<T>>` whose inner lists are all
    // empty collapses to it rather than keeping its outer braces.
    if leaves == 0 {
        out.truncate(mark);
        out.push_str("{}");
    }
    Ok(())
}

/// One `List` level of [`render_array_into`]: braces, separators, and either
/// a recursion or the leaf elements.
///
/// `dims` records the length of the first list seen at each depth **below the
/// outermost**, which every later list at that depth is asserted against —
/// the rectangularity guard. It is untouched for a one-dimensional array, so
/// the common case allocates nothing.
#[allow(clippy::too_many_arguments)]
fn render_list_level(
    column: &dyn Array,
    row: usize,
    child_plan: &NestedPlan,
    depth: usize,
    dims: &mut Vec<usize>,
    out: &mut String,
    scratch: &mut String,
    leaves: &mut usize,
) -> Result<()> {
    let values = column.as_any().downcast_ref::<ListArray>().unwrap().value(row);
    if let Some(d) = depth.checked_sub(1) {
        if dims.len() == d {
            dims.push(values.len());
        } else {
            assert_eq!(
                dims[d],
                values.len(),
                "a List column's sub-lists must all be the same length at one depth; \
                 append_typed rejects a value whose shape does not fit"
            );
        }
    }
    out.push('{');
    for i in 0..values.len() {
        if i > 0 {
            out.push(',');
        }
        if let NestedPlan::Array(inner) = child_plan {
            let inner = &inner.plan;
            render_list_level(values.as_ref(), i, inner, depth + 1, dims, out, scratch, leaves)?;
            continue;
        }
        let mark = out.len();
        if render_plan_into(values.as_ref(), i, child_plan, out)? {
            quote_array_element(out, mark, scratch);
        } else {
            push_array_null(out);
        }
        *leaves += 1;
    }
    out.push('}');
    Ok(())
}

/// The `ArrayLiteral`-building render, kept as the oracle
/// [`render_array_into`]'s direct walk is checked against, over a generated
/// corpus rather than a listed one.
#[cfg(test)]
mod prior_shape;

/// [`render_array_into`] against the literal-building oracle, over generated
/// element text and generated shapes. The corpus is built as Arrow values
/// directly rather than through [`append_typed`]: the quoting rule is what is
/// under test, and a corpus routed through `decode_array` would need its
/// input literals written by that same rule.
#[cfg(test)]
mod differential;
