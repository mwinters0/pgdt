//! One column's builder, chosen from its field's Arrow type and nested plan,
//! and what appends into it: a decoded scalar, a nested literal walked into
//! its list and struct parts as the output grammars' walk reads each part,
//! a NULL, and the text a typmod refuses.

use std::sync::Arc;

use arrow::array::builder::{
    BooleanBuilder, Date32Builder, Decimal128Builder, Decimal256Builder, FixedSizeBinaryBuilder,
    Float32Builder, Float64Builder, Int16Builder, Int32Builder, Int64Builder,
    IntervalMonthDayNanoBuilder, StringDictionaryBuilder, StringViewBuilder,
    Time64MicrosecondBuilder, TimestampMicrosecondBuilder, UInt32Builder,
};
use arrow::array::{ArrayRef, ListArray, StructArray};
use arrow::buffer::{NullBuffer, OffsetBuffer, ScalarBuffer};
use arrow::datatypes::{
    DataType, FieldRef, Fields, Int32Type, IntervalMonthDayNano, IntervalUnit, TimeUnit,
};

use pgdt_codec::decode;
use pgdt_codec::internal::{
    FieldText, RangeShape, char_typmod_refuses, float_field, walk_array, walk_bracketed,
    walk_int2vector, walk_multirange, walk_range, walk_record,
};
use pgdt_codec::pgtype::{NestedPlan, Position};
use pgdt_model::PostgresInvalidValues;

use super::bytea::ByteaBuilder;

mod factor;

pub(crate) use self::factor::{Factor, column_factor};

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
    /// `bytea`, decoded into the column in place (`docs/design/decisions.md`, "D141").
    Binary(ByteaBuilder),
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

fn new_list_parts(
    data_type: &DataType,
    element: &Position<NestedPlan>,
    room: Option<Room>,
) -> ListParts {
    let DataType::List(field) = data_type else {
        unreachable!("a NestedPlan::Array/Multirange only ever accompanies DataType::List")
    };
    let child = |room| Box::new(column_builder(field.data_type(), &element.plan, room));
    let (child, lists) = match room {
        None => (child(None), 0),
        Some(Room { entries, len }) => {
            // As many parts as the text holds at their fewest bytes each.
            let parts = len.div_ceil(part_bytes(&child(None)));
            (child(Some(Room { entries: parts, len })), entries)
        }
    };
    let mut offsets = Vec::with_capacity(lists + 1);
    offsets.push(0);
    ListParts {
        field: field.clone(),
        child,
        not_null: element.not_null,
        offsets,
        validity: Vec::with_capacity(lists),
    }
}

fn new_struct_parts(
    data_type: &DataType,
    positions: &[Position<NestedPlan>],
    room: Option<Room>,
) -> StructParts {
    let DataType::Struct(fields) = data_type else {
        unreachable!("a NestedPlan::Record/Range only ever accompanies DataType::Struct")
    };
    assert_eq!(fields.len(), positions.len(), "a struct's plan has one entry per field");
    let children = fields
        .iter()
        .zip(positions)
        .map(|(f, position)| column_builder(f.data_type(), &position.plan, room))
        .collect();
    let not_null = positions.iter().map(|position| position.not_null).collect();
    let validity = Vec::with_capacity(room.map_or(0, |room| room.entries));
    StructParts { fields: fields.clone(), children, not_null, validity }
}

/// The fewest bytes an entry of `builder` is written in as a list's part, its
/// delimiter aside: a leaf's one byte, an unquoted part never being empty; a
/// list's `{`, its `}` delimiting its last part; an `int2vector`'s first
/// element; a record's parentheses and commas, each field NULL; a range's
/// `(,)`.
pub(super) fn shortest(builder: &ColumnBuilder) -> u64 {
    match builder {
        ColumnBuilder::Record(parts) => parts.children.len().max(1) as u64 + 1,
        ColumnBuilder::Range(_) => 3,
        _ => 1,
    }
}

/// The fewest bytes an entry of `builder` takes as a list's part, its
/// delimiter included: what a list's child is reserved by, one entry for
/// each of them its text holds.
fn part_bytes(builder: &ColumnBuilder) -> usize {
    shortest(builder) as usize + 1
}

/// The entries an Arrow leaf builder has room for unreserved: its default.
const DEFAULT_ENTRIES: usize = 1024;

/// What a builder is reserved for: `entries` of its own, and a text of `len`
/// bytes its repeated parts are read from.
#[derive(Debug, Clone, Copy)]
struct Room {
    entries: usize,
    len: usize,
}

pub(crate) fn new_column_builder(data_type: &DataType, plan: &NestedPlan) -> ColumnBuilder {
    column_builder(data_type, plan, None)
}

/// **A nested column's builder reserved for one value of `len` bytes of
/// text**: each buffer at that buffer's density — as many of each repeated
/// part as the text holds at their fewest bytes, a `bytea` part's bytes as
/// many as the text's — so decoding the value into it never grows a buffer
/// by doubling, and holds what [`Factor`]'s reservation states
/// (`docs/design/decisions.md`, "D154").
pub(crate) fn reserved_column_builder(
    data_type: &DataType,
    plan: &NestedPlan,
    len: usize,
) -> ColumnBuilder {
    column_builder(data_type, plan, Some(Room { entries: 1, len }))
}

/// A builder of `data_type` filled as `plan` says, with room as `room`
/// states, or with none an Arrow leaf's default room, a `bytea` and a list's
/// and struct's own entries none.
fn column_builder(data_type: &DataType, plan: &NestedPlan, room: Option<Room>) -> ColumnBuilder {
    match plan {
        NestedPlan::Scalar | NestedPlan::Decimal { .. } | NestedPlan::Text { .. } => {}
        NestedPlan::Array(child) => {
            return ColumnBuilder::Array(new_list_parts(data_type, child, room));
        }
        NestedPlan::Multirange(bound) => {
            let range = Position::nullable(NestedPlan::Range(bound.clone()));
            return ColumnBuilder::Multirange(new_list_parts(data_type, &range, room));
        }
        NestedPlan::Int2Vector => {
            let element = Position::nullable(NestedPlan::Scalar);
            return ColumnBuilder::Int2Vector(new_list_parts(data_type, &element, room));
        }
        NestedPlan::Record(field_plans) => {
            return ColumnBuilder::Record(new_struct_parts(data_type, field_plans, room));
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
            return ColumnBuilder::Range(new_struct_parts(data_type, &plans, room));
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
    // Arrow's default room, or the entries `room` reserves.
    let n = room.map_or(DEFAULT_ENTRIES, |room| room.entries);
    match data_type {
        DataType::Utf8View => ColumnBuilder::Utf8View(StringViewBuilder::with_capacity(n), length),
        DataType::Boolean => ColumnBuilder::Bool(BooleanBuilder::with_capacity(n)),
        DataType::Int16 => ColumnBuilder::Int16(Int16Builder::with_capacity(n)),
        DataType::Int32 => ColumnBuilder::Int32(Int32Builder::with_capacity(n)),
        DataType::Int64 => ColumnBuilder::Int64(Int64Builder::with_capacity(n)),
        DataType::UInt32 => ColumnBuilder::UInt32(UInt32Builder::with_capacity(n)),
        DataType::Float32 => ColumnBuilder::Float32(Float32Builder::with_capacity(n)),
        DataType::Float64 => ColumnBuilder::Float64(Float64Builder::with_capacity(n)),
        DataType::Date32 => ColumnBuilder::Date32(Date32Builder::with_capacity(n)),
        DataType::Timestamp(TimeUnit::Microsecond, tz) => ColumnBuilder::TimestampMicro {
            builder: TimestampMicrosecondBuilder::with_capacity(n).with_timezone_opt(tz.clone()),
            has_tz: tz.is_some(),
        },
        DataType::Time64(TimeUnit::Microsecond) => {
            ColumnBuilder::Time64Micro(Time64MicrosecondBuilder::with_capacity(n))
        }
        DataType::Interval(IntervalUnit::MonthDayNano) => {
            ColumnBuilder::IntervalMonthDayNano(IntervalMonthDayNanoBuilder::with_capacity(n))
        }
        DataType::Decimal128(p, s) => ColumnBuilder::Decimal128 {
            builder: Decimal128Builder::with_capacity(n).with_precision_and_scale(*p, *s).expect(
                "pgtype::map_numeric only ever produces a valid Decimal128 precision/scale",
            ),
            scale: *s,
            precision: precision(*p),
        },
        DataType::Decimal256(p, s) => ColumnBuilder::Decimal256 {
            builder: Decimal256Builder::with_capacity(n).with_precision_and_scale(*p, *s).expect(
                "pgtype::map_numeric only ever produces a valid Decimal256 precision/scale",
            ),
            scale: *s,
            precision: precision(*p),
        },
        DataType::FixedSizeBinary(16) => {
            ColumnBuilder::FixedSizeBinary16(FixedSizeBinaryBuilder::with_capacity(n, 16))
        }
        // A part's bytes are never more than its text's.
        DataType::Binary => ColumnBuilder::Binary(match room {
            None => ByteaBuilder::new(),
            Some(Room { entries, len }) => ByteaBuilder::with_capacity(entries, len),
        }),
        // An enum's labels are its type's, so only its keys are reserved.
        DataType::Dictionary(k, v) if **k == DataType::Int32 && **v == DataType::Utf8 => {
            ColumnBuilder::Dictionary(match room {
                None => StringDictionaryBuilder::new(),
                Some(Room { entries, .. }) => StringDictionaryBuilder::with_capacity(entries, 0, 0),
            })
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
/// sitting *inside* a nested value. Unlike the scan's top level, a
/// `Utf8View` here copies (`docs/design/decisions.md`, "D29";
/// `measurements.md`, `nested-decode-micro`).
///
/// The error is unit, as [`append_typed`]'s is: `Error::FieldDecode`
/// quotes the *field*, so a failure deep inside a nested literal is the
/// whole literal's. `not_null` is the
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
        Some(text) => append_typed(builder, text, invalid),
    }
}

/// The builder an array's elements land in, below every dimension's list,
/// and whether a NULL there is refused: the innermost list's position's.
fn array_leaf(parts: &mut ListParts) -> (&mut ColumnBuilder, bool) {
    let not_null = parts.not_null;
    match &mut *parts.child {
        ColumnBuilder::Array(inner) => array_leaf(inner),
        leaf => (leaf, not_null),
    }
}

/// Close `lists` lists at `parts`' level, each of `dims[0]` entries of its
/// child, and each of those lists' own below them: a rectangular value's
/// offsets, every one of a level's lists as long as the next.
fn close_dimensions(parts: &mut ListParts, dims: &[usize], lists: usize) {
    if let [len, inner @ ..] = dims {
        if !inner.is_empty()
            && let ColumnBuilder::Array(child) = &mut *parts.child
        {
            close_dimensions(child, inner, lists * len);
        }
        let base = *parts.offsets.last().expect("a list's offsets start at [0]") as usize;
        parts.offsets.extend((1..=lists).map(|list| (base + list * len) as i32));
        parts.validity.extend(std::iter::repeat_n(true, lists));
    }
}

/// Append one `array_out` value read off `text`, each element appended to
/// the innermost list's child as the walk reads it, and each dimension's
/// lists closed once the value has ended and its shape is known.
fn append_array(
    parts: &mut ListParts,
    text: &mut FieldText<'_>,
    invalid: PostgresInvalidValues,
) -> std::result::Result<(), ()> {
    let (leaf, not_null) = array_leaf(parts);
    let shape = walk_array(text, |element| append_nested(leaf, element, not_null, invalid).ok())
        .ok_or(())?;
    if shape.dims.is_empty() {
        // `{}` is what `array_out` emits for a zero-element array of *any*
        // dimensionality, so it fits this column whatever its resolved
        // depth.
        parts.offsets.push(builder_len(&parts.child) as i32);
        parts.validity.push(true);
        return Ok(());
    }
    // Arrow lists are 0-based and have no lower bound, so an `[lb:ub]=`
    // value's index origin has nowhere to go. Failing is the recoverable
    // outcome; dropping the prefix is not.
    if shape.lower_bounds.iter().any(|lb| *lb != 1) || shape.dims.len() != array_depth(parts) {
        return Err(());
    }
    close_dimensions(parts, &shape.dims, 1);
    Ok(())
}

/// Append one range whose bounds `read` walks, each handed to the bound's
/// builder as it is read, and the three flags of the shape it returns: an
/// `empty` range's bounds, which it never hands over, are NULL.
fn append_range(
    parts: &mut StructParts,
    invalid: PostgresInvalidValues,
    read: impl FnOnce(&mut dyn FnMut(bool, Option<&str>) -> Option<()>) -> Option<RangeShape>,
) -> std::result::Result<(), ()> {
    let [lower, upper, lower_inclusive, upper_inclusive, empty] = &mut parts.children[..] else {
        unreachable!("a range struct always has exactly five children")
    };
    // An infinite bound, which `range_in` hands no input function, is NULL.
    let shape = read(&mut |is_lower, value| {
        let bound = if is_lower { &mut *lower } else { &mut *upper };
        append_nested(bound, value, false, invalid).ok()
    })
    .ok_or(())?;
    if shape.empty {
        append_null(lower);
        append_null(upper);
    }
    for (flag, value) in [
        (lower_inclusive, shape.lower_inclusive),
        (upper_inclusive, shape.upper_inclusive),
        (empty, shape.empty),
    ] {
        let ColumnBuilder::Bool(b) = flag else {
            unreachable!("a range struct's three flags are always Boolean")
        };
        b.append_value(value);
    }
    parts.validity.push(true);
    Ok(())
}

/// Append one `record_out` value read off `text`, each field appended to
/// its child as the walk reads it.
fn append_record(
    parts: &mut StructParts,
    text: &mut FieldText<'_>,
    invalid: PostgresInvalidValues,
) -> std::result::Result<(), ()> {
    let StructParts { children, not_null, validity, .. } = parts;
    let mut at = 0;
    let count = walk_record(text, |field| {
        let appended = if children.is_empty() {
            // A zero-field composite is written `()`, and so is a one-field
            // composite holding NULL — the literal cannot tell them apart,
            // so the walk reads the only thing the text supports and the
            // declared field list decides (I23).
            (at == 0 && field.is_none()).then_some(())
        } else {
            append_nested(children.get_mut(at)?, field, not_null[at], invalid).ok()
        };
        at += 1;
        appended
    })
    .ok_or(())?;
    // A composite's field count comes from its `CREATE TYPE`; a literal that
    // disagrees is not a value of this type.
    if count != children.len().max(1) {
        return Err(());
    }
    validity.push(true);
    Ok(())
}

/// Append one nested value read off `text` by the output grammars' walk
/// (`pgdt_codec::internal::walk_array` and its siblings), each part handed
/// to its builder as it is read, so no element of it is staged
/// (`docs/design/decisions.md`, "D153"): over a text in place, or a field
/// as `COPY` wrote it, unescaped as it is read. `Err` as [`append_typed`]'s
/// is, the builders then holding the parts read before the refusal: a
/// field refused ends its query, which reads none of them.
pub(crate) fn append_walked(
    builder: &mut ColumnBuilder,
    mut text: FieldText<'_>,
    invalid: PostgresInvalidValues,
) -> std::result::Result<(), ()> {
    let text = &mut text;
    match builder {
        ColumnBuilder::Array(parts) => append_array(parts, text, invalid)?,
        ColumnBuilder::Multirange(parts) => {
            let ColumnBuilder::Range(range) = &mut *parts.child else {
                unreachable!("a multirange's child is always the range struct")
            };
            walk_multirange(text, |member| {
                append_range(range, invalid, |bound| walk_bracketed(member, bound)).ok()
            })
            .ok_or(())?;
            parts.offsets.push(builder_len(&parts.child) as i32);
            parts.validity.push(true);
        }
        // An empty vector is the empty *field*, a value and not a NULL: `\N`
        // is the only NULL in COPY TEXT, so `''` fills a zero-length list
        // rather than collapsing into the null beside it.
        ColumnBuilder::Int2Vector(parts) => {
            let ColumnBuilder::Int16(child) = &mut *parts.child else {
                unreachable!("an int2vector's child is always Int16")
            };
            walk_int2vector(text, |value| {
                child.append_value(value);
                Some(())
            })
            .ok_or(())?;
            parts.offsets.push(builder_len(&parts.child) as i32);
            parts.validity.push(true);
        }
        ColumnBuilder::Record(parts) => append_record(parts, text, invalid)?,
        ColumnBuilder::Range(parts) => {
            append_range(parts, invalid, |bound| walk_range(text, bound))?
        }
        _ => unreachable!("only a nested builder is walked (`is_walked`)"),
    }
    Ok(())
}

/// Whether `builder` takes a nested value, which [`append_walked`] reads.
pub(crate) fn is_walked(builder: &ColumnBuilder) -> bool {
    matches!(
        builder,
        ColumnBuilder::Array(_)
            | ColumnBuilder::Multirange(_)
            | ColumnBuilder::Int2Vector(_)
            | ColumnBuilder::Record(_)
            | ColumnBuilder::Range(_)
    )
}

/// Whether a text held to `length` — a `character varying(n)`'s or
/// `character(n)`'s — is refused, as `COPY` refuses it (I75), unless `invalid`
/// says to read it as it is.
pub(super) fn text_refused(
    text: &str,
    length: Option<u32>,
    invalid: PostgresInvalidValues,
) -> bool {
    invalid != PostgresInvalidValues::Ignore
        && length.is_some_and(|length| char_typmod_refuses(text, length))
}

/// Decode `text` (already COPY-unescaped) per `builder`'s type and append it,
/// via `pgdt_codec::decode`'s per-type decoders, a nested value walked
/// ([`append_walked`]). `Err` on a decode failure, unit as [`append_nested`]'s is — the
/// caller wraps it into `Error::FieldDecode` with the table/column/row
/// context and the field's quote, so no copy of `text` is made here. A
/// `ColumnBuilder::Utf8View` copies `text`, refused where its length typmod
/// refuses it ([`text_refused`]); the scan's `push_field` takes its zero-copy
/// path ahead of this one, which needs the raw field's byte offset
/// (`docs/design/decisions.md`, "D29"). `invalid` is how a field its type's
/// `*_in` refuses is read, at any depth.
pub(crate) fn append_typed(
    builder: &mut ColumnBuilder,
    text: &str,
    invalid: PostgresInvalidValues,
) -> std::result::Result<(), ()> {
    match builder {
        ColumnBuilder::Utf8View(b, length) => {
            if text_refused(text, *length, invalid) {
                return Err(());
            }
            b.append_value(text);
        }
        ColumnBuilder::Array(_)
        | ColumnBuilder::Multirange(_)
        | ColumnBuilder::Int2Vector(_)
        | ColumnBuilder::Record(_)
        | ColumnBuilder::Range(_) => append_walked(builder, FieldText::plain(text), invalid)?,
        ColumnBuilder::Bool(b) => b.append_value(decode::decode_bool(text).ok_or(())?),
        ColumnBuilder::Int16(b) => b.append_value(text.parse::<i16>().map_err(drop)?),
        ColumnBuilder::Int32(b) => b.append_value(text.parse::<i32>().map_err(drop)?),
        ColumnBuilder::Int64(b) => b.append_value(text.parse::<i64>().map_err(drop)?),
        // `u32`: `oidout` writes `%u`, so a field carrying a sign is the file
        // contradicting its own DDL, same as any other undecodable field.
        ColumnBuilder::UInt32(b) => b.append_value(text.parse::<u32>().map_err(drop)?),
        ColumnBuilder::Float32(b) => {
            b.append_value(float_field(text, invalid).ok_or(())?);
        }
        ColumnBuilder::Float64(b) => {
            b.append_value(float_field(text, invalid).ok_or(())?);
        }
        ColumnBuilder::Date32(b) => b.append_value(decode::decode_date32(text).ok_or(())?),
        ColumnBuilder::TimestampMicro { builder, has_tz } => {
            builder.append_value(decode::decode_timestamp_micros(text, *has_tz).ok_or(())?);
        }
        ColumnBuilder::Time64Micro(b) => {
            b.append_value(decode::decode_time64_micros(text).ok_or(())?);
        }
        // The two value classes with no `MonthDayNano` encoding — v17's
        // infinities and a time part past `2562047:47:16.854775807` — fail
        // here, the same way `date`'s infinities and `numeric(p,s)`'s `NaN`
        // do.
        ColumnBuilder::IntervalMonthDayNano(b) => {
            let (months, days, nanoseconds) = decode::decode_interval(text).ok_or(())?;
            b.append_value(IntervalMonthDayNano { months, days, nanoseconds });
        }
        ColumnBuilder::Decimal128 { builder, scale, precision } => {
            let unscaled = decode::typmod_unscaled_digits(text, *precision, i16::from(*scale))
                .map_err(drop)?;
            builder.append_value(unscaled.parse::<i128>().map_err(drop)?);
        }
        ColumnBuilder::Decimal256 { builder, scale, precision } => {
            let unscaled = decode::typmod_unscaled_digits(text, *precision, i16::from(*scale))
                .map_err(drop)?;
            builder.append_value(arrow::datatypes::i256::from_string(&unscaled).ok_or(())?);
        }
        ColumnBuilder::FixedSizeBinary16(b) => {
            let bytes = decode::decode_uuid(text).ok_or(())?;
            b.append_value(bytes).expect("decode_uuid always produces exactly 16 bytes");
        }
        ColumnBuilder::Binary(b) => {
            b.append_value(&decode::decode_bytea(text).ok_or(())?);
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
        append_typed(&mut builder, text, PostgresInvalidValues::Default)
            .map_err(|()| text.to_string())?;
    }
    Ok(finish_column(&mut builder))
}

#[cfg(test)]
mod tests;
