//! **A key's value as its column emits it**, for an engine relating two of
//! DataFusion's orders by its own coercion and cast: the codec links no
//! arrow-cast and builds no array (`docs/design/decisions.md`, "D74"), so it
//! hands over the value in its emitted type's units and the engine rebuilds
//! it (`datafusion_pgdump::compare_value_keys`).

use arrow_buffer::i256;

use super::order::{FINITE, OrderKey};
use super::relate::ScalarOrder;
use crate::value::datetime::POSTGRES_EPOCH_UNIX_MICROS;

/// A finite value in its column's emitted Arrow type's own units: what
/// [`ValueKey::emitted`](super::ValueKey::emitted) pairs with that type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EmittedValue<'k> {
    Boolean(bool),
    /// An integer or an `oid`; a `Date32`'s days, a `Time64`'s microseconds
    /// and a `Timestamp`'s microseconds, from 1970 for the two that count
    /// from an epoch.
    Integer(i64),
    /// A `Float32`'s value widened, or a `Float64`'s.
    Float(f64),
    /// A decimal's unscaled value at its type's scale.
    Decimal(i256),
    /// An `Interval(MonthDayNano)`'s three fields.
    MonthDayNano {
        months: i32,
        days: i32,
        nanoseconds: i64,
    },
    /// A `Binary`'s or a `FixedSizeBinary`'s bytes.
    Bytes(&'k [u8]),
    /// A `Utf8View`'s text, or a `Dictionary`'s value.
    Text(&'k str),
}

/// `key`, read in DataFusion's `order`, as its column emits it: `None` for a
/// special value, which no emitted type holds, and for a value the units
/// cannot count — a timestamp past `i64` microseconds from 1970, an
/// `interval` time part past `i64` nanoseconds.
pub(super) fn emitted_value<'k>(
    order: &ScalarOrder,
    key: &'k OrderKey,
) -> Option<EmittedValue<'k>> {
    if key.rank() != FINITE {
        return None;
    }
    use ScalarOrder as O;
    Some(match (order, key) {
        (O::Bool, OrderKey::Bool(value)) => EmittedValue::Boolean(*value),
        (O::Timestamp { .. }, OrderKey::Int(micros)) => EmittedValue::Integer(
            i64::try_from(i128::from(*micros) + POSTGRES_EPOCH_UNIX_MICROS).ok()?,
        ),
        (O::Int { .. } | O::Oid | O::Date | O::Time, OrderKey::Int(value)) => {
            EmittedValue::Integer(*value)
        }
        (O::Float32 | O::Float64, OrderKey::Float(value)) => EmittedValue::Float(*value),
        (O::Decimal { .. }, OrderKey::Decimal(unscaled)) => EmittedValue::Decimal(*unscaled),
        (O::IntervalFields, OrderKey::IntervalFields(months, days, micros)) => {
            EmittedValue::MonthDayNano {
                months: *months,
                days: *days,
                nanoseconds: micros.checked_mul(1_000)?,
            }
        }
        (O::Uuid | O::Bytea, OrderKey::Bytes(bytes)) => EmittedValue::Bytes(bytes),
        (O::Text, OrderKey::Text(text)) => EmittedValue::Text(text),
        _ => return None,
    })
}
