//! One column's builder, chosen from its field's Arrow type and nested plan,
//! and what appends into it: a decoded scalar, a nested literal walked into
//! its list and struct parts, a NULL, and the text a typmod refuses.

use std::sync::Arc;

use arrow::array::builder::{
    BinaryBuilder, BooleanBuilder, Date32Builder, Decimal128Builder, Decimal256Builder,
    FixedSizeBinaryBuilder, Float32Builder, Float64Builder, Int16Builder, Int32Builder,
    Int64Builder, IntervalMonthDayNanoBuilder, StringDictionaryBuilder, StringViewBuilder,
    Time64MicrosecondBuilder, TimestampMicrosecondBuilder, UInt32Builder,
};
use arrow::array::{ArrayRef, ListArray, StructArray};
use arrow::buffer::{NullBuffer, OffsetBuffer, ScalarBuffer};
use arrow::datatypes::{
    DataType, FieldRef, Fields, Int32Type, IntervalMonthDayNano, IntervalUnit, TimeUnit,
};

use pgdt_codec::decode;
use pgdt_codec::internal::{char_typmod_refuses, float_field};
use pgdt_codec::nested::{
    RangeLiteral, decode_array, decode_int2vector, decode_multirange, decode_range, decode_record,
};
use pgdt_codec::pgtype::{NestedPlan, Position};
use pgdt_model::PostgresInvalidValues;

/// One column's typed builder, chosen from a [`pgdt_codec::ResolvedSchema`]
/// field's [`DataType`] — the complete set [`pgdt_codec::pgtype::resolve_declared_type`]
/// and [`pgdt_codec::resolve_columns`] can ever produce.
/// `with_precision_and_scale`/`with_timezone_opt` tag the three builders
/// whose default type is not the schema's — the two decimals and the
/// timestamp — every other arm's default already matching, so a `finish()`ed array's type matches the schema exactly, which
/// `RecordBatch::try_new` checks.
pub(crate) enum ColumnBuilder {
    /// A text, and a `character varying(n)`'s or `character(n)`'s length,
    /// past which a field is refused ([`char_typmod_refuses`]).
    Utf8View(StringViewBuilder, Option<u32>),
    Bool(BooleanBuilder),
    Int16(Int16Builder),
    Int32(Int32Builder),
    Int64(Int64Builder),
    /// `oid`, and nothing else: PostgreSQL's only unsigned integer type.
    UInt32(UInt32Builder),
    Float32(Float32Builder),
    Float64(Float64Builder),
    Date32(Date32Builder),
    TimestampMicro {
        builder: TimestampMicrosecondBuilder,
        has_tz: bool,
    },
    Time64Micro(Time64MicrosecondBuilder),
    /// `interval`: PostgreSQL's three independent fields, narrowed from its
    /// `int64` microseconds to Arrow's `int64` nanoseconds.
    IntervalMonthDayNano(IntervalMonthDayNanoBuilder),
    /// A `numeric(p,s)`: `scale` is the Arrow type's and `precision` the
    /// typmod's, which a field is put through ([`decode::typmod_unscaled_digits`]).
    Decimal128 {
        builder: Decimal128Builder,
        scale: i8,
        precision: u16,
    },
    Decimal256 {
        builder: Decimal256Builder,
        scale: i8,
        precision: u16,
    },
    FixedSizeBinary16(FixedSizeBinaryBuilder),
    Binary(BinaryBuilder),
    Dictionary(StringDictionaryBuilder<Int32Type>),
    /// `List<T>` filled from an `array_out` literal. Nested `Array`s are the
    /// multi-dimensional case.
    Array(ListParts),
    /// `List<` range struct `>` filled from a `multirange_out` literal —
    /// structurally identical to an array of ranges and written differently
    /// (`docs/design/decisions.md`, "D43").
    Multirange(ListParts),
    /// `List<Int16>` filled from an `int2vectorout` literal — the same Arrow
    /// type a `smallint[]` column gets, written in a grammar of its own:
    /// [`NestedPlan`]'s second collision, not a special case of the first.
    Int2Vector(ListParts),
    /// `Struct<…>` filled from a `record_out` literal, one child per declared
    /// field of the composite.
    Record(StructParts),
    /// The five-field range struct filled from a `range_out` literal:
    /// children are `lower`, `upper`, and the three `Boolean` flags, in
    /// [`pgdt_codec::pgtype::RANGE_STRUCT_FIELDS`] order.
    Range(StructParts),
}

/// A list column under construction. `offsets` starts at `[0]` and gains one
/// entry per appended row (including a null one, which contributes an empty
/// range); `validity` is parallel to those rows.
pub(crate) struct ListParts {
    /// The child `Field` the `DataType::List` names — reused verbatim when
    /// finishing, so the built array's type matches the schema exactly.
    field: FieldRef,
    child: Box<ColumnBuilder>,
    /// Whether a NULL element is refused ([`Position::not_null`]).
    not_null: bool,
    offsets: Vec<i32>,
    validity: Vec<bool>,
}

/// A struct column under construction. Arrow requires every child to have one
/// entry per struct row, **including for a null row**, so `append_null` fills
/// the children too.
pub(crate) struct StructParts {
    fields: Fields,
    children: Vec<ColumnBuilder>,
    /// Whether each child refuses a NULL ([`Position::not_null`]).
    not_null: Vec<bool>,
    validity: Vec<bool>,
}

/// How many values a builder holds — the offset a list's next entry starts at.
fn builder_len(builder: &ColumnBuilder) -> usize {
    use arrow::array::ArrayBuilder;
    match builder {
        ColumnBuilder::Utf8View(b, _) => b.len(),
        ColumnBuilder::Bool(b) => b.len(),
        ColumnBuilder::Int16(b) => b.len(),
        ColumnBuilder::Int32(b) => b.len(),
        ColumnBuilder::Int64(b) => b.len(),
        ColumnBuilder::UInt32(b) => b.len(),
        ColumnBuilder::Float32(b) => b.len(),
        ColumnBuilder::Float64(b) => b.len(),
        ColumnBuilder::Date32(b) => b.len(),
        ColumnBuilder::TimestampMicro { builder, .. } => builder.len(),
        ColumnBuilder::Time64Micro(b) => b.len(),
        ColumnBuilder::IntervalMonthDayNano(b) => b.len(),
        ColumnBuilder::Decimal128 { builder, .. } => builder.len(),
        ColumnBuilder::Decimal256 { builder, .. } => builder.len(),
        ColumnBuilder::FixedSizeBinary16(b) => b.len(),
        ColumnBuilder::Binary(b) => b.len(),
        ColumnBuilder::Dictionary(b) => b.len(),
        ColumnBuilder::Array(parts)
        | ColumnBuilder::Multirange(parts)
        | ColumnBuilder::Int2Vector(parts) => parts.validity.len(),
        ColumnBuilder::Record(parts) | ColumnBuilder::Range(parts) => parts.validity.len(),
    }
}

/// How many `List` levels a builder chain has, i.e. the dimensionality an
/// `array_out` value must have to fit it. An empty array (`{}`) fits any
/// depth and is exempt.
fn array_depth(parts: &ListParts) -> usize {
    match &*parts.child {
        ColumnBuilder::Array(inner) => 1 + array_depth(inner),
        _ => 1,
    }
}

fn new_list_parts(data_type: &DataType, element: &Position<NestedPlan>) -> ListParts {
    let DataType::List(field) = data_type else {
        unreachable!("a NestedPlan::Array/Multirange only ever accompanies DataType::List")
    };
    ListParts {
        field: field.clone(),
        child: Box::new(new_column_builder(field.data_type(), &element.plan)),
        not_null: element.not_null,
        offsets: vec![0],
        validity: Vec::new(),
    }
}

fn new_struct_parts(data_type: &DataType, positions: &[Position<NestedPlan>]) -> StructParts {
    let DataType::Struct(fields) = data_type else {
        unreachable!("a NestedPlan::Record/Range only ever accompanies DataType::Struct")
    };
    assert_eq!(fields.len(), positions.len(), "a struct's plan has one entry per field");
    let children = fields
        .iter()
        .zip(positions)
        .map(|(f, position)| new_column_builder(f.data_type(), &position.plan))
        .collect();
    let not_null = positions.iter().map(|position| position.not_null).collect();
    StructParts { fields: fields.clone(), children, not_null, validity: Vec::new() }
}

pub(crate) fn new_column_builder(data_type: &DataType, plan: &NestedPlan) -> ColumnBuilder {
    match plan {
        NestedPlan::Scalar | NestedPlan::Decimal { .. } | NestedPlan::Text { .. } => {}
        NestedPlan::Array(child) => {
            return ColumnBuilder::Array(new_list_parts(data_type, child));
        }
        NestedPlan::Multirange(bound) => {
            let range = Position::nullable(NestedPlan::Range(bound.clone()));
            return ColumnBuilder::Multirange(new_list_parts(data_type, &range));
        }
        NestedPlan::Int2Vector => {
            let element = Position::nullable(NestedPlan::Scalar);
            return ColumnBuilder::Int2Vector(new_list_parts(data_type, &element));
        }
        NestedPlan::Record(field_plans) => {
            return ColumnBuilder::Record(new_struct_parts(data_type, field_plans));
        }
        NestedPlan::Range(bound) => {
            let plans = [
                (**bound).clone(),
                (**bound).clone(),
                NestedPlan::Scalar,
                NestedPlan::Scalar,
                NestedPlan::Scalar,
            ]
            .map(Position::nullable);
            return ColumnBuilder::Range(new_struct_parts(data_type, &plans));
        }
    }
    // A decimal's typmod precision, which the Arrow type carries only where
    // the scale does not exceed it; a plan stating none, as a caller's own
    // schema does, is bounded by the Arrow type's.
    let precision = |arrow: u8| match plan {
        NestedPlan::Decimal { precision } => *precision,
        _ => u16::from(arrow),
    };
    let length = match plan {
        NestedPlan::Text { length } => Some(*length),
        _ => None,
    };
    match data_type {
        DataType::Utf8View => ColumnBuilder::Utf8View(StringViewBuilder::new(), length),
        DataType::Boolean => ColumnBuilder::Bool(BooleanBuilder::new()),
        DataType::Int16 => ColumnBuilder::Int16(Int16Builder::new()),
        DataType::Int32 => ColumnBuilder::Int32(Int32Builder::new()),
        DataType::Int64 => ColumnBuilder::Int64(Int64Builder::new()),
        DataType::UInt32 => ColumnBuilder::UInt32(UInt32Builder::new()),
        DataType::Float32 => ColumnBuilder::Float32(Float32Builder::new()),
        DataType::Float64 => ColumnBuilder::Float64(Float64Builder::new()),
        DataType::Date32 => ColumnBuilder::Date32(Date32Builder::new()),
        DataType::Timestamp(TimeUnit::Microsecond, tz) => ColumnBuilder::TimestampMicro {
            builder: TimestampMicrosecondBuilder::new().with_timezone_opt(tz.clone()),
            has_tz: tz.is_some(),
        },
        DataType::Time64(TimeUnit::Microsecond) => {
            ColumnBuilder::Time64Micro(Time64MicrosecondBuilder::new())
        }
        DataType::Interval(IntervalUnit::MonthDayNano) => {
            ColumnBuilder::IntervalMonthDayNano(IntervalMonthDayNanoBuilder::new())
        }
        DataType::Decimal128(p, s) => ColumnBuilder::Decimal128 {
            builder: Decimal128Builder::new().with_precision_and_scale(*p, *s).expect(
                "pgtype::map_numeric only ever produces a valid Decimal128 precision/scale",
            ),
            scale: *s,
            precision: precision(*p),
        },
        DataType::Decimal256(p, s) => ColumnBuilder::Decimal256 {
            builder: Decimal256Builder::new().with_precision_and_scale(*p, *s).expect(
                "pgtype::map_numeric only ever produces a valid Decimal256 precision/scale",
            ),
            scale: *s,
            precision: precision(*p),
        },
        DataType::FixedSizeBinary(16) => {
            ColumnBuilder::FixedSizeBinary16(FixedSizeBinaryBuilder::new(16))
        }
        DataType::Binary => ColumnBuilder::Binary(BinaryBuilder::new()),
        DataType::Dictionary(k, v) if **k == DataType::Int32 && **v == DataType::Utf8 => {
            ColumnBuilder::Dictionary(StringDictionaryBuilder::new())
        }
        other => unreachable!("resolve_columns never resolves a column to {other:?}"),
    }
}

pub(super) fn append_null(builder: &mut ColumnBuilder) {
    match builder {
        ColumnBuilder::Utf8View(b, _) => b.append_null(),
        ColumnBuilder::Bool(b) => b.append_null(),
        ColumnBuilder::Int16(b) => b.append_null(),
        ColumnBuilder::Int32(b) => b.append_null(),
        ColumnBuilder::Int64(b) => b.append_null(),
        ColumnBuilder::UInt32(b) => b.append_null(),
        ColumnBuilder::Float32(b) => b.append_null(),
        ColumnBuilder::Float64(b) => b.append_null(),
        ColumnBuilder::Date32(b) => b.append_null(),
        ColumnBuilder::TimestampMicro { builder, .. } => builder.append_null(),
        ColumnBuilder::Time64Micro(b) => b.append_null(),
        ColumnBuilder::IntervalMonthDayNano(b) => b.append_null(),
        ColumnBuilder::Decimal128 { builder, .. } => builder.append_null(),
        ColumnBuilder::Decimal256 { builder, .. } => builder.append_null(),
        ColumnBuilder::FixedSizeBinary16(b) => b.append_null(),
        ColumnBuilder::Binary(b) => b.append_null(),
        ColumnBuilder::Dictionary(b) => b.append_null(),
        ColumnBuilder::Array(parts)
        | ColumnBuilder::Multirange(parts)
        | ColumnBuilder::Int2Vector(parts) => {
            // A null list still needs an offset entry; it spans zero children.
            parts.offsets.push(builder_len(&parts.child) as i32);
            parts.validity.push(false);
        }
        ColumnBuilder::Record(parts) | ColumnBuilder::Range(parts) => {
            // Arrow gives a struct's children one entry per struct row, null
            // row included — a shorter child is an invalid array, not a
            // compact one.
            for child in &mut parts.children {
                append_null(child);
            }
            parts.validity.push(false);
        }
    }
}

/// Append one already-COPY-unescaped value — or SQL NULL — into a builder
/// sitting *inside* a nested value. Unlike the top level, a `Utf8View` here
/// copies (`docs/design/decisions.md`, "D29"; `measurements.md`,
/// `nested-decode-micro`).
///
/// The error is unit rather than the offending text: `Error::FieldDecode`
/// reports the *field*'s value, so [`append_typed`] attributes a failure deep
/// inside a nested literal to the whole literal. `not_null` is the
/// position's ([`Position::not_null`]): a NULL there is refused but under
/// `ignore`, as `array_in` and `record_in` refuse it (I76).
fn append_nested(
    builder: &mut ColumnBuilder,
    value: Option<&str>,
    not_null: bool,
    invalid: PostgresInvalidValues,
) -> std::result::Result<(), ()> {
    match value {
        None if not_null && invalid != PostgresInvalidValues::Ignore => Err(()),
        None => {
            append_null(builder);
            Ok(())
        }
        Some(text) => match builder {
            ColumnBuilder::Utf8View(b, length) => {
                if text_refused(text, *length, invalid) {
                    return Err(());
                }
                b.append_value(text);
                Ok(())
            }
            _ => append_typed(builder, text, invalid).map_err(|_| ()),
        },
    }
}

/// Append one level of an `array_out` value, taking `dims[0]` entries from
/// `elements`. Recurses for a multi-dimensional value, one `List` level per
/// dimension.
fn append_array_level(
    parts: &mut ListParts,
    dims: &[usize],
    elements: &mut std::slice::Iter<'_, Option<std::borrow::Cow<'_, str>>>,
    invalid: PostgresInvalidValues,
) -> std::result::Result<(), ()> {
    if dims.len() == 1 {
        for _ in 0..dims[0] {
            let element = elements.next().ok_or(())?.as_deref();
            append_nested(&mut parts.child, element, parts.not_null, invalid)?;
        }
    } else {
        for _ in 0..dims[0] {
            let ColumnBuilder::Array(inner) = &mut *parts.child else {
                return Err(());
            };
            append_array_level(inner, &dims[1..], elements, invalid)?;
        }
    }
    parts.offsets.push(builder_len(&parts.child) as i32);
    parts.validity.push(true);
    Ok(())
}

fn append_range(
    parts: &mut StructParts,
    range: &RangeLiteral,
    invalid: PostgresInvalidValues,
) -> std::result::Result<(), ()> {
    let [lower, upper, lower_inclusive, upper_inclusive, empty] = &mut parts.children[..] else {
        unreachable!("a range struct always has exactly five children")
    };
    // An infinite bound, which `range_in` hands no input function.
    append_nested(lower, range.lower.as_deref(), false, invalid)?;
    append_nested(upper, range.upper.as_deref(), false, invalid)?;
    for (flag, value) in [
        (lower_inclusive, range.lower_inclusive),
        (upper_inclusive, range.upper_inclusive),
        (empty, range.empty),
    ] {
        let ColumnBuilder::Bool(b) = flag else {
            unreachable!("a range struct's three flags are always Boolean")
        };
        b.append_value(value);
    }
    parts.validity.push(true);
    Ok(())
}

/// Whether a text held to `length` — a `character varying(n)`'s or
/// `character(n)`'s — is refused, as `COPY` refuses it (I75), unless `invalid`
/// says to read it as it is.
pub(crate) fn text_refused(
    text: &str,
    length: Option<u32>,
    invalid: PostgresInvalidValues,
) -> bool {
    invalid != PostgresInvalidValues::Ignore
        && length.is_some_and(|length| char_typmod_refuses(text, length))
}

/// Decode `text` (already COPY-unescaped) per `builder`'s type and append it,
/// via `pgdt_codec::decode`'s per-type decoders and `pgdt_codec::nested`'s literal
/// codecs. `Err(text)` on a decode failure — the caller wraps it into
/// `Error::FieldDecode` with the table/column/row context. Never called for a
/// *top-level* `ColumnBuilder::Utf8View`, which `push_field` handles itself
/// (its zero-copy path needs the raw field's byte offset); a nested one
/// reaches [`append_nested`] instead and copies. `invalid` is how a field its
/// type's `*_in` refuses is read, at any depth.
pub(crate) fn append_typed(
    builder: &mut ColumnBuilder,
    text: &str,
    invalid: PostgresInvalidValues,
) -> std::result::Result<(), String> {
    let fail = || text.to_string();
    match builder {
        ColumnBuilder::Utf8View(..) => unreachable!("caller handles Utf8View directly"),
        ColumnBuilder::Array(parts) => {
            let literal = decode_array(text).ok_or_else(fail)?;
            if literal.is_decorated() {
                // Arrow lists are 0-based and have no lower bound, so an
                // `[lb:ub]=` value's index origin has nowhere to go. Failing
                // is the recoverable outcome; dropping the prefix is not.
                return Err(fail());
            }
            if literal.elements.is_empty() {
                // `{}` is what `array_out` emits for a zero-element array of
                // *any* dimensionality, so it fits this column whatever its
                // resolved depth.
                parts.offsets.push(builder_len(&parts.child) as i32);
                parts.validity.push(true);
                return Ok(());
            }
            if literal.ndim() != array_depth(parts) {
                return Err(fail());
            }
            append_array_level(parts, &literal.dims, &mut literal.elements.iter(), invalid)
                .map_err(|()| fail())?;
        }
        ColumnBuilder::Multirange(parts) => {
            let members = decode_multirange(text).ok_or_else(fail)?;
            for member in &members {
                let ColumnBuilder::Range(range) = &mut *parts.child else {
                    unreachable!("a multirange's child is always the range struct")
                };
                append_range(range, member, invalid).map_err(|()| fail())?;
            }
            parts.offsets.push(builder_len(&parts.child) as i32);
            parts.validity.push(true);
        }
        // An empty vector is the empty *field*, a value and not a NULL: `\N`
        // is the only NULL in COPY TEXT, so `''` fills a zero-length list
        // rather than collapsing into the null beside it.
        ColumnBuilder::Int2Vector(parts) => {
            let values = decode_int2vector(text).ok_or_else(fail)?;
            let ColumnBuilder::Int16(child) = &mut *parts.child else {
                unreachable!("an int2vector's child is always Int16")
            };
            for value in values {
                child.append_value(value);
            }
            parts.offsets.push(builder_len(&parts.child) as i32);
            parts.validity.push(true);
        }
        ColumnBuilder::Record(parts) => {
            let literal = decode_record(text).ok_or_else(fail)?;
            if parts.children.is_empty() {
                // A zero-field composite is written `()`, and so is a
                // one-field composite holding NULL — the literal cannot tell
                // them apart, so `decode_record` reports the only thing the
                // text supports and the declared field list decides (I23).
                if literal.fields != [None] {
                    return Err(fail());
                }
                parts.validity.push(true);
                return Ok(());
            }
            // A composite's field count comes from its `CREATE TYPE`; a
            // literal that disagrees is not a value of this type.
            if literal.fields.len() != parts.children.len() {
                return Err(fail());
            }
            for ((child, &not_null), field) in
                parts.children.iter_mut().zip(&parts.not_null).zip(&literal.fields)
            {
                append_nested(child, field.as_deref(), not_null, invalid).map_err(|()| fail())?;
            }
            parts.validity.push(true);
        }
        ColumnBuilder::Range(parts) => {
            let literal = decode_range(text).ok_or_else(fail)?;
            append_range(parts, &literal, invalid).map_err(|()| fail())?;
        }
        ColumnBuilder::Bool(b) => b.append_value(decode::decode_bool(text).ok_or_else(fail)?),
        ColumnBuilder::Int16(b) => b.append_value(text.parse::<i16>().map_err(|_| fail())?),
        ColumnBuilder::Int32(b) => b.append_value(text.parse::<i32>().map_err(|_| fail())?),
        ColumnBuilder::Int64(b) => b.append_value(text.parse::<i64>().map_err(|_| fail())?),
        // `u32`: `oidout` writes `%u`, so a field carrying a sign is the file
        // contradicting its own DDL, same as any other undecodable field.
        ColumnBuilder::UInt32(b) => b.append_value(text.parse::<u32>().map_err(|_| fail())?),
        ColumnBuilder::Float32(b) => {
            b.append_value(float_field(text, invalid).ok_or_else(fail)?);
        }
        ColumnBuilder::Float64(b) => {
            b.append_value(float_field(text, invalid).ok_or_else(fail)?);
        }
        ColumnBuilder::Date32(b) => b.append_value(decode::decode_date32(text).ok_or_else(fail)?),
        ColumnBuilder::TimestampMicro { builder, has_tz } => {
            builder.append_value(decode::decode_timestamp_micros(text, *has_tz).ok_or_else(fail)?);
        }
        ColumnBuilder::Time64Micro(b) => {
            b.append_value(decode::decode_time64_micros(text).ok_or_else(fail)?);
        }
        // The two value classes with no `MonthDayNano` encoding — v17's
        // infinities and a time part past `2562047:47:16.854775807` — fail
        // here, the same way `date`'s infinities and `numeric(p,s)`'s `NaN`
        // do.
        ColumnBuilder::IntervalMonthDayNano(b) => {
            let (months, days, nanoseconds) = decode::decode_interval(text).ok_or_else(fail)?;
            b.append_value(IntervalMonthDayNano { months, days, nanoseconds });
        }
        ColumnBuilder::Decimal128 { builder, scale, precision } => {
            let unscaled = decode::typmod_unscaled_digits(text, *precision, i16::from(*scale))
                .map_err(|_| fail())?;
            builder.append_value(unscaled.parse::<i128>().map_err(|_| fail())?);
        }
        ColumnBuilder::Decimal256 { builder, scale, precision } => {
            let unscaled = decode::typmod_unscaled_digits(text, *precision, i16::from(*scale))
                .map_err(|_| fail())?;
            builder.append_value(arrow::datatypes::i256::from_string(&unscaled).ok_or_else(fail)?);
        }
        ColumnBuilder::FixedSizeBinary16(b) => {
            let bytes = decode::decode_uuid(text).ok_or_else(fail)?;
            b.append_value(bytes).expect("decode_uuid always produces exactly 16 bytes");
        }
        ColumnBuilder::Binary(b) => {
            b.append_value(decode::decode_bytea(text).ok_or_else(fail)?);
        }
        // deficiency: KD88 — the text is appended whatever it is, so a label
        // the column's enum does not declare, which `enum_in` refuses (I70),
        // reaches the batch: a field no default parse keyed — its column at
        // the metadata level, its block declined — is refused by no query but
        // one whose ordering term keys it or whose term compares an array or
        // composite holding it, though a strict parse refuses it
        // wherever it sits. Refusing it here needs the labels and their
        // exactness, which this builder does not carry.
        ColumnBuilder::Dictionary(b) => b.append_value(text),
    }
    Ok(())
}

pub(crate) fn finish_column(builder: &mut ColumnBuilder) -> ArrayRef {
    match builder {
        ColumnBuilder::Utf8View(b, _) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Bool(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Int16(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Int32(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Int64(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::UInt32(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Float32(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Float64(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Date32(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::TimestampMicro { builder, .. } => Arc::new(builder.finish()) as ArrayRef,
        ColumnBuilder::Time64Micro(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::IntervalMonthDayNano(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Decimal128 { builder, .. } => Arc::new(builder.finish()) as ArrayRef,
        ColumnBuilder::Decimal256 { builder, .. } => Arc::new(builder.finish()) as ArrayRef,
        ColumnBuilder::FixedSizeBinary16(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Binary(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Dictionary(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Array(parts)
        | ColumnBuilder::Multirange(parts)
        | ColumnBuilder::Int2Vector(parts) => {
            let values = finish_column(&mut parts.child);
            let offsets = std::mem::replace(&mut parts.offsets, vec![0]);
            let nulls = NullBuffer::from(std::mem::take(&mut parts.validity));
            Arc::new(ListArray::new(
                parts.field.clone(),
                OffsetBuffer::new(ScalarBuffer::from(offsets)),
                values,
                Some(nulls),
            )) as ArrayRef
        }
        ColumnBuilder::Record(parts) | ColumnBuilder::Range(parts) => {
            let children: Vec<ArrayRef> = parts.children.iter_mut().map(finish_column).collect();
            let nulls = NullBuffer::from(std::mem::take(&mut parts.validity));
            // `try_new_with_length`, not `new`: a zero-field composite (I23)
            // has no child to read the row count off, and Arrow refuses to
            // guess one.
            let len = nulls.len();
            Arc::new(
                StructArray::try_new_with_length(parts.fields.clone(), children, Some(nulls), len)
                    .expect("children are built one per struct row, from the schema's own fields"),
            ) as ArrayRef
        }
    }
}

/// One column of `data_type` holding `values`, each appended as
/// [`RowBatcher`] appends a non-NULL field, for a test outside this module
/// that needs the array a batch emits beside the text it was filled from.
/// `Err` is the first value the column's builder refused.
#[cfg(test)]
pub(crate) fn column_of(
    data_type: &DataType,
    plan: &NestedPlan,
    values: &[&str],
) -> std::result::Result<ArrayRef, String> {
    let mut builder = new_column_builder(data_type, plan);
    for text in values {
        match &mut builder {
            ColumnBuilder::Utf8View(_, length)
                if text_refused(text, *length, PostgresInvalidValues::Default) =>
            {
                return Err(text.to_string());
            }
            ColumnBuilder::Utf8View(b, _) => b.append_value(text),
            typed => append_typed(typed, text, PostgresInvalidValues::Default)?,
        }
    }
    Ok(finish_column(&mut builder))
}

#[cfg(test)]
mod tests;
