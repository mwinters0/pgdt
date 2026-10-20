//! **Two values' order as DataFusion answers `a < b`**, for keys a codec
//! read in its semantics under two orders: by DataFusion's own comparison
//! coercion, cast and comparison kernel, which the codec cannot reproduce,
//! linking no arrow-cast (`docs/design/decisions.md`, "D74"). DataFusion is
//! its oracle (`tests/key_order.rs`). Not Arrow's `cmp` kernels alone, which
//! relate one type: DataFusion coerces first, as PostgreSQL's answers count
//! its casts.

use std::cmp::Ordering;

use arrow::array::{Array, BooleanArray};
use arrow::datatypes::{DataType, IntervalMonthDayNano, IntervalUnit, TimeUnit};
use datafusion::common::ScalarValue;
use datafusion::logical_expr::type_coercion::binary::BinaryTypeCoercer;
use datafusion::logical_expr::{ColumnarValue, Operator};
use datafusion::physical_expr_common::datum::apply_cmp;
use pgdt_codec::key::{EmittedValue, ValueKey};

/// **`a`'s order against `b`'s, as its engine answers**: every pair
/// [`ValueKey::compare`] relates, with its answer — two keys of one order,
/// and in PostgreSQL's semantics every pair the server relates — and in
/// DataFusion's semantics two keys of two orders its comparison coercion
/// relates, both values cast to the type it coerces them to and compared by
/// the kernel a query compares by. **`None` wherever DataFusion answers
/// none**: no coerced type, a cast it refuses — a text that does not read as
/// the number or time it is compared with, a date past a nanosecond
/// timestamp's range — and, against another order, a value its column's
/// type cannot hold, which a scan reads as NULL
/// (`docs/design/decisions.md`, "D98"); and for two keys read in two
/// semantics.
pub fn compare_value_keys(a: &ValueKey, b: &ValueKey) -> Option<Ordering> {
    if let Some(order) = a.compare(b) {
        return Some(order);
    }
    let (a_type, a_value) = a.emitted()?;
    let (b_type, b_value) = b.emitted()?;
    let (left, right) =
        BinaryTypeCoercer::new(a_type, &Operator::Lt, b_type).get_input_types().ok()?;
    // A comparison's cast is the query's: `safe: false`, so a value it
    // cannot cast refuses rather than reading as NULL.
    let left = value(a_type, a_value)?.cast_to(&left, None).ok()?;
    let right = value(b_type, b_value)?.cast_to(&right, None).ok()?;
    let holds = |op| {
        let ColumnarValue::Array(answer) = apply_cmp(op, &left, &right).ok()? else {
            return None;
        };
        let answer = answer.as_any().downcast_ref::<BooleanArray>()?;
        answer.is_valid(0).then(|| answer.value(0))
    };
    Some(if holds(Operator::Lt)? {
        Ordering::Less
    } else if holds(Operator::Eq)? {
        Ordering::Equal
    } else {
        Ordering::Greater
    })
}

/// A one-row column of `data_type` holding `value`, as a scan emits it:
/// `None` where the type cannot hold the value, which a literal read against
/// a narrower column may be.
fn value(data_type: &DataType, value: EmittedValue<'_>) -> Option<ColumnarValue> {
    let scalar = scalar(data_type, value)?;
    Some(ColumnarValue::Array(scalar.to_array().ok()?))
}

fn scalar(data_type: &DataType, value: EmittedValue<'_>) -> Option<ScalarValue> {
    use DataType as T;
    use EmittedValue as V;
    Some(match (data_type, value) {
        (T::Boolean, V::Boolean(value)) => ScalarValue::Boolean(Some(value)),
        (T::Int16, V::Integer(value)) => ScalarValue::Int16(Some(value.try_into().ok()?)),
        (T::Int32, V::Integer(value)) => ScalarValue::Int32(Some(value.try_into().ok()?)),
        (T::Int64, V::Integer(value)) => ScalarValue::Int64(Some(value)),
        (T::UInt32, V::Integer(value)) => ScalarValue::UInt32(Some(value.try_into().ok()?)),
        // A `real`'s key is its value widened, so narrowing it back is exact.
        (T::Float32, V::Float(value)) => ScalarValue::Float32(Some(value as f32)),
        (T::Float64, V::Float(value)) => ScalarValue::Float64(Some(value)),
        (T::Decimal128(precision, scale), V::Decimal(unscaled)) => {
            ScalarValue::Decimal128(Some(unscaled.to_i128()?), *precision, *scale)
        }
        (T::Decimal256(precision, scale), V::Decimal(unscaled)) => {
            ScalarValue::Decimal256(Some(unscaled), *precision, *scale)
        }
        (T::Date32, V::Integer(days)) => ScalarValue::Date32(Some(days.try_into().ok()?)),
        (T::Time64(TimeUnit::Microsecond), V::Integer(micros)) => {
            ScalarValue::Time64Microsecond(Some(micros))
        }
        (T::Timestamp(TimeUnit::Microsecond, zone), V::Integer(micros)) => {
            ScalarValue::TimestampMicrosecond(Some(micros), zone.clone())
        }
        (
            T::Interval(IntervalUnit::MonthDayNano),
            V::MonthDayNano { months, days, nanoseconds },
        ) => ScalarValue::IntervalMonthDayNano(Some(IntervalMonthDayNano::new(
            months,
            days,
            nanoseconds,
        ))),
        (T::Binary, V::Bytes(bytes)) => ScalarValue::Binary(Some(bytes.to_vec())),
        (T::FixedSizeBinary(width), V::Bytes(bytes)) => {
            ScalarValue::FixedSizeBinary(*width, Some(bytes.to_vec()))
        }
        (T::Utf8View, V::Text(text)) => ScalarValue::Utf8View(Some(text.to_owned())),
        (T::Dictionary(index, values), V::Text(_)) => {
            ScalarValue::Dictionary(index.clone(), Box::new(scalar(values, value)?))
        }
        (T::Utf8, V::Text(text)) => ScalarValue::Utf8(Some(text.to_owned())),
        _ => return None,
    })
}
