//! Row/batch assembly: turns rows inside a `COPY` block into typed Arrow
//! `RecordBatch`es, one column builder per [`crate::resolve::ResolvedSchema`]
//! field (`docs/design/architecture.md`, "Arrow assembly and the zero-copy
//! path").
//!
//! Per `docs/design/roadmap-phase7-scan-performance.md`, a `Utf8View` field
//! that needs no unescaping is appended as a zero-copy view into the Arrow
//! `Buffer` backing the read chunk it came from, rather than copied into the
//! builder's own storage — retrofitting that later would be expensive, so
//! it's built in even though the rest of the performance work is not. Every other mapped type always copies: its decoded value has
//! its own representation (an `i32`, a `[u8; 16]`, …), not a byte range of
//! the original field.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::ops::ControlFlow;
use std::sync::Arc;

use arrow::array::builder::{
    BinaryBuilder, BooleanBuilder, Date32Builder, Decimal128Builder, Decimal256Builder,
    FixedSizeBinaryBuilder, Float32Builder, Float64Builder, Int16Builder, Int32Builder,
    Int64Builder, StringDictionaryBuilder, StringViewBuilder, Time64MicrosecondBuilder,
    TimestampMicrosecondBuilder,
};
use arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array, Decimal256Array,
    DictionaryArray, FixedSizeBinaryArray, Float32Array, Float64Array, Int16Array, Int32Array,
    Int64Array, ListArray, RecordBatch, StringArray, StringViewArray, StructArray,
    Time64MicrosecondArray, TimestampMicrosecondArray,
};
use arrow::buffer::{Buffer, NullBuffer, OffsetBuffer, ScalarBuffer};
use arrow::datatypes::{DataType, FieldRef, Fields, Int32Type, SchemaRef, TimeUnit};

use crate::cache::CacheMode;
use crate::copy::{CopyHeader, DELIMITER, decode_field};
use crate::decode;
use crate::io::ByteRangeSource;
use crate::nested::{self, RangeLiteral};
use crate::pgtype::NestedPlan;
use crate::predicate::Predicate;
use crate::resolve::{ResolvedSchema, SchemaMode};
use crate::scan::ScanOptions;
use crate::{Error, Result};

/// Tuning knobs for batch assembly.
#[derive(Debug, Clone)]
pub struct BatchOptions {
    /// Rows per batch. A batch is flushed once it reaches this many rows.
    pub max_rows: usize,
    /// Optional cap on a batch's total field-byte count. Whichever of this
    /// or `max_rows` is hit first flushes the batch.
    pub max_bytes: Option<usize>,
    /// Whether to resolve column types against the dump's DDL — see
    /// `docs/design/architecture.md`, "Arrow assembly and the zero-copy path". Every
    /// `RecordBatch` this build produces carries the same schema as its
    /// query's [`crate::resolve::ResolvedSchema`] — a column this build has
    /// no mapping for stays `Utf8View`, same as `SchemaMode::Strings` maps
    /// every column.
    pub schema_mode: SchemaMode,
    /// Selects which database's table to query when the name alone is
    /// ambiguous — matched against `DatabaseMetadata::name`
    /// (`docs/design/architecture.md`, "One target per query"). `None` is the common case: a single-database dump, or a
    /// cross-schema ambiguity a qualified name already resolves on its own.
    pub database: Option<String>,
    /// How far a query's mapping scan walks before it starts returning rows
    /// — see [`ScanExtent`].
    pub scan_extent: ScanExtent,
}

impl Default for BatchOptions {
    fn default() -> Self {
        Self {
            max_rows: 8192,
            max_bytes: None,
            schema_mode: SchemaMode::default(),
            database: None,
            scan_extent: ScanExtent::default(),
        }
    }
}

/// How far [`crate::stream::table_stream`]'s mapping scan walks
/// (`docs/design/architecture.md`, "Query: mapping and streaming are separate passes"). Rows are always replayed from blocks the map
/// already holds, so this controls how much of the file a query pays to map
/// before any row comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanExtent {
    /// Stop as soon as the queried table is settled: at least one matching
    /// block has closed, and none of the matching blocks carried a
    /// partition-root marker (I2 — a marked block's name owns further blocks
    /// that are *not* adjacent, so only EOF enumerates them). This is what
    /// keeps a query against an early table in a huge dump from costing a
    /// full scan.
    ///
    /// What it gives up is stated in `STATUS.md`'s "Known gaps": a second,
    /// conflicting candidate past the stopping point is never seen, so
    /// `Error::AmbiguousTable` reports only what the scan reached. A file
    /// concatenating two dumps of the *same* database name is the case with
    /// no early signal at all.
    #[default]
    UntilTargetSettled,
    /// Map the whole file before returning anything. Costs a full scan and
    /// gives exact ambiguity detection and a complete, reusable cache — the
    /// same map `pgdq parse` builds.
    Full,
}

/// A read chunk retained only long enough for zero-copy views to be taken
/// into it. Dropped once the scanner has moved past it for good.
pub(crate) struct SourceChunk {
    /// Absolute file offset of `buffer[0]`.
    pub(crate) start: u64,
    pub(crate) buffer: Buffer,
    /// Cached `StringViewBuilder::append_block` index per column, filled in
    /// the first time a column takes a view into this chunk. Grown lazily
    /// rather than sized up front, since more than one schema (from
    /// sequential or same-name-different-schema blocks) can reference the
    /// same chunk.
    pub(crate) column_blocks: Vec<Option<u32>>,
}

impl SourceChunk {
    pub(crate) fn end(&self) -> u64 {
        self.start + self.buffer.len() as u64
    }

    /// If `[offset, offset + len)` lies entirely within this chunk, the
    /// `(local_offset, len)` view coordinates into it.
    fn contains(&self, offset: u64, len: usize) -> Option<(u32, u32)> {
        let local = offset.checked_sub(self.start)?;
        let local_end = local.checked_add(len as u64)?;
        (local_end <= self.buffer.len() as u64).then_some((local as u32, len as u32))
    }

    fn block_for(&mut self, col: usize, builder: &mut StringViewBuilder) -> u32 {
        if self.column_blocks.len() <= col {
            self.column_blocks.resize(col + 1, None);
        }
        *self.column_blocks[col].get_or_insert_with(|| builder.append_block(self.buffer.clone()))
    }
}

/// `StringViewBuilder::finish()` resets its internal block list to build the
/// next batch, which invalidates every cached block index in `chunks` — a
/// flush must clear them all, or a later reference to an already-seen chunk
/// would resolve to the wrong (or out-of-bounds) block in the new batch.
pub(crate) fn invalidate_block_cache(chunks: &mut VecDeque<SourceChunk>) {
    for chunk in chunks.iter_mut() {
        chunk.column_blocks.clear();
    }
}

/// The column names a `COPY` header implies: its own list, or — when it
/// carried none, meaning "all columns, in table order" — placeholder names
/// sized to `field_count` (the first row's field count) — there is no DDL
/// to name them from; this is also what a headerless block's
/// [`crate::resolve::resolve_columns`] lookup is keyed against.
pub(crate) fn column_names(header: &CopyHeader, field_count: usize) -> Vec<String> {
    if header.columns.is_empty() {
        (1..=field_count).map(|i| format!("column{i}")).collect()
    } else {
        header.columns.clone()
    }
}

/// One column's typed builder, chosen from a [`crate::resolve::ResolvedSchema`]
/// field's [`DataType`] — the complete set [`crate::pgtype::resolve_declared_type`]
/// and [`crate::resolve::resolve_columns`] can ever produce. `with_data_type`/
/// `with_precision_and_scale`/`with_timezone_opt` tag each builder so its
/// `finish()`ed array's type matches the schema exactly (`RecordBatch::try_new`
/// checks this), rather than the builder's own default `DataType`.
enum ColumnBuilder {
    Utf8View(StringViewBuilder),
    Bool(BooleanBuilder),
    Int16(Int16Builder),
    Int32(Int32Builder),
    Int64(Int64Builder),
    Float32(Float32Builder),
    Float64(Float64Builder),
    Date32(Date32Builder),
    TimestampMicro {
        builder: TimestampMicrosecondBuilder,
        has_tz: bool,
    },
    Time64Micro(Time64MicrosecondBuilder),
    Decimal128 {
        builder: Decimal128Builder,
        scale: i8,
    },
    Decimal256 {
        builder: Decimal256Builder,
        scale: i8,
    },
    FixedSizeBinary16(FixedSizeBinaryBuilder),
    Binary(BinaryBuilder),
    Dictionary(StringDictionaryBuilder<Int32Type>),
    /// `List<T>` filled from an `array_out` literal. Nested `Array`s are the
    /// multi-dimensional case.
    Array(ListParts),
    /// `List<` range struct `>` filled from a `multirange_out` literal —
    /// structurally identical to an array of ranges and written differently,
    /// which is why [`NestedPlan`] exists.
    Multirange(ListParts),
    /// `Struct<…>` filled from a `record_out` literal, one child per declared
    /// field of the composite.
    Record(StructParts),
    /// The five-field range struct filled from a `range_out` literal:
    /// children are `lower`, `upper`, and the three `Boolean` flags, in
    /// [`crate::pgtype::RANGE_STRUCT_FIELDS`] order.
    Range(StructParts),
}

/// A list column under construction. `offsets` starts at `[0]` and gains one
/// entry per appended row (including a null one, which contributes an empty
/// range); `validity` is parallel to those rows.
struct ListParts {
    /// The child `Field` the `DataType::List` names — reused verbatim when
    /// finishing, so the built array's type matches the schema exactly.
    field: FieldRef,
    child: Box<ColumnBuilder>,
    offsets: Vec<i32>,
    validity: Vec<bool>,
}

/// A struct column under construction. Arrow requires every child to have one
/// entry per struct row, **including for a null row**, so `append_null` fills
/// the children too.
struct StructParts {
    fields: Fields,
    children: Vec<ColumnBuilder>,
    validity: Vec<bool>,
}

/// How many values a builder holds — the offset a list's next entry starts at.
fn builder_len(builder: &ColumnBuilder) -> usize {
    use arrow::array::ArrayBuilder;
    match builder {
        ColumnBuilder::Utf8View(b) => b.len(),
        ColumnBuilder::Bool(b) => b.len(),
        ColumnBuilder::Int16(b) => b.len(),
        ColumnBuilder::Int32(b) => b.len(),
        ColumnBuilder::Int64(b) => b.len(),
        ColumnBuilder::Float32(b) => b.len(),
        ColumnBuilder::Float64(b) => b.len(),
        ColumnBuilder::Date32(b) => b.len(),
        ColumnBuilder::TimestampMicro { builder, .. } => builder.len(),
        ColumnBuilder::Time64Micro(b) => b.len(),
        ColumnBuilder::Decimal128 { builder, .. } => builder.len(),
        ColumnBuilder::Decimal256 { builder, .. } => builder.len(),
        ColumnBuilder::FixedSizeBinary16(b) => b.len(),
        ColumnBuilder::Binary(b) => b.len(),
        ColumnBuilder::Dictionary(b) => b.len(),
        ColumnBuilder::Array(parts) | ColumnBuilder::Multirange(parts) => parts.validity.len(),
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

fn new_list_parts(data_type: &DataType, child_plan: &NestedPlan) -> ListParts {
    let DataType::List(field) = data_type else {
        unreachable!("a NestedPlan::Array/Multirange only ever accompanies DataType::List")
    };
    ListParts {
        field: field.clone(),
        child: Box::new(new_column_builder(field.data_type(), child_plan)),
        offsets: vec![0],
        validity: Vec::new(),
    }
}

fn new_struct_parts(data_type: &DataType, plans: &[NestedPlan]) -> StructParts {
    let DataType::Struct(fields) = data_type else {
        unreachable!("a NestedPlan::Record/Range only ever accompanies DataType::Struct")
    };
    assert_eq!(fields.len(), plans.len(), "a struct's plan has one entry per field");
    let children =
        fields.iter().zip(plans).map(|(f, plan)| new_column_builder(f.data_type(), plan)).collect();
    StructParts { fields: fields.clone(), children, validity: Vec::new() }
}

fn new_column_builder(data_type: &DataType, plan: &NestedPlan) -> ColumnBuilder {
    match plan {
        NestedPlan::Scalar => {}
        NestedPlan::Array(child) => {
            return ColumnBuilder::Array(new_list_parts(data_type, child));
        }
        NestedPlan::Multirange(bound) => {
            let range = NestedPlan::Range(bound.clone());
            return ColumnBuilder::Multirange(new_list_parts(data_type, &range));
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
            ];
            return ColumnBuilder::Range(new_struct_parts(data_type, &plans));
        }
    }
    match data_type {
        DataType::Utf8View => ColumnBuilder::Utf8View(StringViewBuilder::new()),
        DataType::Boolean => ColumnBuilder::Bool(BooleanBuilder::new()),
        DataType::Int16 => ColumnBuilder::Int16(Int16Builder::new()),
        DataType::Int32 => ColumnBuilder::Int32(Int32Builder::new()),
        DataType::Int64 => ColumnBuilder::Int64(Int64Builder::new()),
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
        DataType::Decimal128(p, s) => ColumnBuilder::Decimal128 {
            builder: Decimal128Builder::new().with_precision_and_scale(*p, *s).expect(
                "pgtype::map_numeric only ever produces a valid Decimal128 precision/scale",
            ),
            scale: *s,
        },
        DataType::Decimal256(p, s) => ColumnBuilder::Decimal256 {
            builder: Decimal256Builder::new().with_precision_and_scale(*p, *s).expect(
                "pgtype::map_numeric only ever produces a valid Decimal256 precision/scale",
            ),
            scale: *s,
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

fn append_null(builder: &mut ColumnBuilder) {
    match builder {
        ColumnBuilder::Utf8View(b) => b.append_null(),
        ColumnBuilder::Bool(b) => b.append_null(),
        ColumnBuilder::Int16(b) => b.append_null(),
        ColumnBuilder::Int32(b) => b.append_null(),
        ColumnBuilder::Int64(b) => b.append_null(),
        ColumnBuilder::Float32(b) => b.append_null(),
        ColumnBuilder::Float64(b) => b.append_null(),
        ColumnBuilder::Date32(b) => b.append_null(),
        ColumnBuilder::TimestampMicro { builder, .. } => builder.append_null(),
        ColumnBuilder::Time64Micro(b) => b.append_null(),
        ColumnBuilder::Decimal128 { builder, .. } => builder.append_null(),
        ColumnBuilder::Decimal256 { builder, .. } => builder.append_null(),
        ColumnBuilder::FixedSizeBinary16(b) => b.append_null(),
        ColumnBuilder::Binary(b) => b.append_null(),
        ColumnBuilder::Dictionary(b) => b.append_null(),
        ColumnBuilder::Array(parts) | ColumnBuilder::Multirange(parts) => {
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
/// copies: widening the zero-copy view path into a recursive builder means
/// honouring its chunk-retention and block-invalidation edges at every level,
/// which is a scan-performance change Phase 7 owns
/// (`docs/design/roadmap-phase7-inbox.md`).
///
/// The error is unit rather than the offending text: `Error::FieldDecode`
/// reports the *field*'s value, so a failure deep inside a nested literal is
/// attributed to the whole literal by [`append_typed`], not to the fragment
/// that tripped it.
fn append_nested(builder: &mut ColumnBuilder, value: Option<&str>) -> std::result::Result<(), ()> {
    match value {
        None => {
            append_null(builder);
            Ok(())
        }
        Some(text) => match builder {
            ColumnBuilder::Utf8View(b) => {
                b.append_value(text);
                Ok(())
            }
            _ => append_typed(builder, text).map_err(|_| ()),
        },
    }
}

/// Append one level of an `array_out` value, taking `dims[0]` entries from
/// `elements`. Recurses for a multi-dimensional value, one `List` level per
/// dimension.
fn append_array_level(
    parts: &mut ListParts,
    dims: &[usize],
    elements: &mut std::slice::Iter<'_, Option<String>>,
) -> std::result::Result<(), ()> {
    if dims.len() == 1 {
        for _ in 0..dims[0] {
            append_nested(&mut parts.child, elements.next().ok_or(())?.as_deref())?;
        }
    } else {
        for _ in 0..dims[0] {
            let ColumnBuilder::Array(inner) = &mut *parts.child else {
                return Err(());
            };
            append_array_level(inner, &dims[1..], elements)?;
        }
    }
    parts.offsets.push(builder_len(&parts.child) as i32);
    parts.validity.push(true);
    Ok(())
}

fn append_range(parts: &mut StructParts, range: &RangeLiteral) -> std::result::Result<(), ()> {
    let [lower, upper, lower_inclusive, upper_inclusive, empty] = &mut parts.children[..] else {
        unreachable!("a range struct always has exactly five children")
    };
    append_nested(lower, range.lower.as_deref())?;
    append_nested(upper, range.upper.as_deref())?;
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

/// Decode `text` (already COPY-unescaped) per `builder`'s type and append it,
/// via `crate::decode`'s per-type decoders and `crate::nested`'s literal
/// codecs. `Err(text)` on a decode failure — the caller wraps it into
/// `Error::FieldDecode` with the table/column/row context this function
/// doesn't have. Never called for a *top-level* `ColumnBuilder::Utf8View`,
/// which `push_field` handles itself (its zero-copy path needs the raw
/// field's byte offset, which this function never sees); a nested one reaches
/// [`append_nested`] instead and copies.
fn append_typed(builder: &mut ColumnBuilder, text: &str) -> std::result::Result<(), String> {
    let fail = || text.to_string();
    match builder {
        ColumnBuilder::Utf8View(_) => unreachable!("caller handles Utf8View directly"),
        ColumnBuilder::Array(parts) => {
            let literal = nested::decode_array(text).ok_or_else(fail)?;
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
            append_array_level(parts, &literal.dims, &mut literal.elements.iter())
                .map_err(|()| fail())?;
        }
        ColumnBuilder::Multirange(parts) => {
            let members = nested::decode_multirange(text).ok_or_else(fail)?;
            for member in &members {
                let ColumnBuilder::Range(range) = &mut *parts.child else {
                    unreachable!("a multirange's child is always the range struct")
                };
                append_range(range, member).map_err(|()| fail())?;
            }
            parts.offsets.push(builder_len(&parts.child) as i32);
            parts.validity.push(true);
        }
        ColumnBuilder::Record(parts) => {
            let literal = nested::decode_record(text).ok_or_else(fail)?;
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
            for (child, field) in parts.children.iter_mut().zip(&literal.fields) {
                append_nested(child, field.as_deref()).map_err(|()| fail())?;
            }
            parts.validity.push(true);
        }
        ColumnBuilder::Range(parts) => {
            let literal = nested::decode_range(text).ok_or_else(fail)?;
            append_range(parts, &literal).map_err(|()| fail())?;
        }
        ColumnBuilder::Bool(b) => b.append_value(decode::decode_bool(text).ok_or_else(fail)?),
        ColumnBuilder::Int16(b) => b.append_value(text.parse::<i16>().map_err(|_| fail())?),
        ColumnBuilder::Int32(b) => b.append_value(text.parse::<i32>().map_err(|_| fail())?),
        ColumnBuilder::Int64(b) => b.append_value(text.parse::<i64>().map_err(|_| fail())?),
        ColumnBuilder::Float32(b) => b.append_value(decode::decode_f32(text).ok_or_else(fail)?),
        ColumnBuilder::Float64(b) => b.append_value(decode::decode_f64(text).ok_or_else(fail)?),
        ColumnBuilder::Date32(b) => b.append_value(decode::decode_date32(text).ok_or_else(fail)?),
        ColumnBuilder::TimestampMicro { builder, has_tz } => {
            builder.append_value(decode::decode_timestamp_micros(text, *has_tz).ok_or_else(fail)?);
        }
        ColumnBuilder::Time64Micro(b) => {
            b.append_value(decode::decode_time64_micros(text).ok_or_else(fail)?);
        }
        ColumnBuilder::Decimal128 { builder, scale } => {
            let unscaled = decode::decimal_unscaled_digits(text, *scale).ok_or_else(fail)?;
            builder.append_value(unscaled.parse::<i128>().map_err(|_| fail())?);
        }
        ColumnBuilder::Decimal256 { builder, scale } => {
            let unscaled = decode::decimal_unscaled_digits(text, *scale).ok_or_else(fail)?;
            builder.append_value(arrow::datatypes::i256::from_string(&unscaled).ok_or_else(fail)?);
        }
        ColumnBuilder::FixedSizeBinary16(b) => {
            let bytes = decode::decode_uuid(text).ok_or_else(fail)?;
            b.append_value(bytes).expect("decode_uuid always produces exactly 16 bytes");
        }
        ColumnBuilder::Binary(b) => {
            b.append_value(decode::decode_bytea(text).ok_or_else(fail)?);
        }
        ColumnBuilder::Dictionary(b) => b.append_value(text),
    }
    Ok(())
}

fn finish_column(builder: &mut ColumnBuilder) -> ArrayRef {
    match builder {
        ColumnBuilder::Utf8View(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Bool(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Int16(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Int32(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Int64(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Float32(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Float64(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Date32(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::TimestampMicro { builder, .. } => Arc::new(builder.finish()) as ArrayRef,
        ColumnBuilder::Time64Micro(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Decimal128 { builder, .. } => Arc::new(builder.finish()) as ArrayRef,
        ColumnBuilder::Decimal256 { builder, .. } => Arc::new(builder.finish()) as ArrayRef,
        ColumnBuilder::FixedSizeBinary16(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Binary(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Dictionary(b) => Arc::new(b.finish()) as ArrayRef,
        ColumnBuilder::Array(parts) | ColumnBuilder::Multirange(parts) => {
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

/// Accumulates rows from a single `COPY` block into typed `RecordBatch`es,
/// one [`ColumnBuilder`] per field of the query's
/// [`crate::resolve::ResolvedSchema`].
pub(crate) struct RowBatcher {
    schema: SchemaRef,
    /// Qualified table name, for `Error::FieldDecode`'s context.
    table: String,
    /// Parallel to `schema.fields()` — the declared PostgreSQL type string
    /// behind each `Mapped` column, for the same error.
    declared_types: Vec<Option<String>>,
    columns: Vec<ColumnBuilder>,
    rows_in_batch: usize,
    bytes_in_batch: usize,
    options: BatchOptions,
}

impl RowBatcher {
    pub(crate) fn new(resolved: &ResolvedSchema, table: String, options: BatchOptions) -> Self {
        let schema = resolved.schema.clone();
        let declared_types = resolved.notes.iter().map(|n| n.declared.clone()).collect();
        // `plans` is positional and parallel to `schema.fields()`, from the
        // same producer — `resolve_columns` fills one entry per column,
        // `NestedPlan::Scalar` included, so the two can only disagree if
        // something built a `ResolvedSchema` by hand.
        let columns = schema
            .fields()
            .iter()
            .zip(&resolved.plans)
            .map(|(f, plan)| new_column_builder(f.data_type(), plan))
            .collect();
        Self {
            schema,
            table,
            declared_types,
            columns,
            rows_in_batch: 0,
            bytes_in_batch: 0,
            options,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rows_in_batch == 0
    }

    /// Columns in this block's schema — the field count a resumed stream
    /// needs to rebuild the same schema without re-reading the header.
    pub(crate) fn field_count(&self) -> usize {
        self.schema.fields().len()
    }

    pub(crate) fn should_flush(&self) -> bool {
        self.rows_in_batch >= self.options.max_rows
            || self.options.max_bytes.is_some_and(|max| self.bytes_in_batch >= max)
    }

    /// Append one raw (still-escaped) COPY TEXT data row.
    pub(crate) fn push_row(
        &mut self,
        header_offset: u64,
        row_offset: u64,
        raw: &[u8],
        chunks: &mut VecDeque<SourceChunk>,
    ) -> Result<()> {
        let mut col = 0;
        let mut pos = 0usize;
        loop {
            let end = memchr::memchr(DELIMITER, &raw[pos..]).map_or(raw.len(), |i| pos + i);
            let field = &raw[pos..end];
            let expected = self.columns.len();
            if col >= expected {
                return Err(Error::ColumnCountMismatch {
                    header_offset,
                    row_offset,
                    expected,
                    found: col + 1,
                });
            }
            self.push_field(col, row_offset, row_offset + pos as u64, field, chunks)?;
            self.bytes_in_batch += field.len();
            col += 1;
            if end == raw.len() {
                break;
            }
            pos = end + 1;
        }
        if col != self.columns.len() {
            return Err(Error::ColumnCountMismatch {
                header_offset,
                row_offset,
                expected: self.columns.len(),
                found: col,
            });
        }
        self.rows_in_batch += 1;
        Ok(())
    }

    fn push_field(
        &mut self,
        col: usize,
        row_offset: u64,
        field_offset: u64,
        field: &[u8],
        chunks: &mut VecDeque<SourceChunk>,
    ) -> Result<()> {
        let decoded = decode_field(field)?;
        // Disjoint-field borrow: `columns[col]` is mutated below while
        // `schema`/`table`/`declared_types` are only ever read, on the
        // (rare) error path.
        let Self { schema, table, declared_types, columns, .. } = self;
        let builder = &mut columns[col];
        let Some(text) = decoded else {
            append_null(builder);
            return Ok(());
        };
        match builder {
            ColumnBuilder::Utf8View(b) => {
                push_utf8view_field(b, col, field_offset, field, text, chunks)
            }
            _ => {
                if let Err(value) = append_typed(builder, &text) {
                    return Err(Error::FieldDecode {
                        table: table.clone(),
                        column: schema.field(col).name().clone(),
                        row_offset,
                        declared_type: declared_types[col].clone().unwrap_or_default(),
                        value,
                    });
                }
            }
        }
        Ok(())
    }

    pub(crate) fn flush(&mut self) -> Result<RecordBatch> {
        self.rows_in_batch = 0;
        self.bytes_in_batch = 0;
        let arrays: Vec<ArrayRef> = self.columns.iter_mut().map(finish_column).collect();
        Ok(RecordBatch::try_new(self.schema.clone(), arrays)?)
    }
}

/// Append one still-escaped field to a `Utf8View` column. Reuses
/// [`decode_field`] so the escaping rules live in exactly one place; a
/// `Cow::Borrowed` result (no escapes present, already UTF-8 checked) is what
/// makes the field eligible for a zero-copy view — everything else is
/// copied.
fn push_utf8view_field(
    builder: &mut StringViewBuilder,
    col: usize,
    field_offset: u64,
    field: &[u8],
    text: Cow<'_, str>,
    chunks: &mut VecDeque<SourceChunk>,
) {
    match text {
        Cow::Owned(s) => builder.append_value(s),
        Cow::Borrowed(s) => {
            let view = chunks
                .iter_mut()
                .find_map(|c| c.contains(field_offset, field.len()).map(|coords| (c, coords)));
            match view {
                Some((chunk, (local_offset, len))) => {
                    let block = chunk.block_for(col, builder);
                    // SAFETY: `contains` confirmed `local_offset..local_offset+len`
                    // is in bounds for `block`, and the `Borrowed` case above
                    // means `decode_field` already validated this exact byte
                    // range as UTF-8.
                    unsafe { builder.append_view_unchecked(block, local_offset, len) };
                }
                // The field's bytes straddle two read chunks (rare: at most
                // one field per chunk boundary) or its chunk was already
                // evicted. Either way, a copy is unavoidable.
                None => builder.append_value(s),
            }
        }
    }
}

/// Render one row of `column` back to the same PostgreSQL text form
/// `crate::copy::decode_field` would have produced for it — `pgdq query`'s
/// job (`docs/design/architecture.md`, "CLI surface": output must be
/// byte-identical whether typing is on or off) and the round-trip tests'
/// oracle. `None` for SQL NULL. Covers exactly the [`DataType`]s
/// [`crate::resolve::resolve_columns`] can ever produce.
///
/// `plan` is needed for the same reason [`ColumnBuilder`] needs it: the Arrow
/// type does not say which literal form a nested value is written in, and
/// `int4range[]` and `int4multirange` share one. It comes from
/// [`crate::resolve::ResolvedSchema::plans`], positionally; a caller that
/// knows its column is scalar passes `&NestedPlan::Scalar`, which is
/// [`NestedPlan`]'s `Default`.
///
/// **There is deliberately no plan-less entry point.** One that panicked on a
/// nested column would make "did every caller switch?" a review question
/// rather than a compile error.
pub fn render_field(column: &dyn Array, row: usize, plan: &NestedPlan) -> Option<String> {
    if column.is_null(row) {
        return None;
    }
    match plan {
        NestedPlan::Scalar => {}
        NestedPlan::Array(child) => return Some(render_array(column, row, child)),
        NestedPlan::Multirange(bound) => {
            let list = column.as_any().downcast_ref::<ListArray>().unwrap();
            let members = list.value(row);
            let range = NestedPlan::Range(bound.clone());
            let rendered: Vec<String> = (0..members.len())
                .map(|i| {
                    render_field(members.as_ref(), i, &range)
                        .expect("a multirange's members are never SQL NULL")
                })
                .collect();
            return Some(format!("{{{}}}", rendered.join(",")));
        }
        NestedPlan::Record(field_plans) => {
            let s = column.as_any().downcast_ref::<StructArray>().unwrap();
            let fields: Vec<Option<String>> = s
                .columns()
                .iter()
                .zip(field_plans)
                .map(|(child, p)| render_field(child.as_ref(), row, p))
                .collect();
            return Some(nested::render_record(&nested::RecordLiteral { fields }));
        }
        NestedPlan::Range(bound) => {
            let s = column.as_any().downcast_ref::<StructArray>().unwrap();
            let flag =
                |i: usize| s.column(i).as_any().downcast_ref::<BooleanArray>().unwrap().value(row);
            return Some(nested::render_range(&RangeLiteral {
                empty: flag(4),
                lower: render_field(s.column(0).as_ref(), row, bound),
                upper: render_field(s.column(1).as_ref(), row, bound),
                lower_inclusive: flag(2),
                upper_inclusive: flag(3),
            }));
        }
    }
    Some(match column.data_type() {
        DataType::Utf8View => {
            column.as_any().downcast_ref::<StringViewArray>().unwrap().value(row).to_string()
        }
        DataType::Boolean => {
            decode::render_bool(column.as_any().downcast_ref::<BooleanArray>().unwrap().value(row))
                .to_string()
        }
        DataType::Int16 => {
            column.as_any().downcast_ref::<Int16Array>().unwrap().value(row).to_string()
        }
        DataType::Int32 => {
            column.as_any().downcast_ref::<Int32Array>().unwrap().value(row).to_string()
        }
        DataType::Int64 => {
            column.as_any().downcast_ref::<Int64Array>().unwrap().value(row).to_string()
        }
        DataType::Float32 => {
            decode::render_f32(column.as_any().downcast_ref::<Float32Array>().unwrap().value(row))
        }
        DataType::Float64 => {
            decode::render_f64(column.as_any().downcast_ref::<Float64Array>().unwrap().value(row))
        }
        DataType::Date32 => {
            decode::render_date32(column.as_any().downcast_ref::<Date32Array>().unwrap().value(row))
        }
        DataType::Timestamp(TimeUnit::Microsecond, tz) => {
            let v = column.as_any().downcast_ref::<TimestampMicrosecondArray>().unwrap().value(row);
            decode::render_timestamp_micros(v, tz.is_some())
        }
        DataType::Time64(TimeUnit::Microsecond) => decode::render_time64_micros(
            column.as_any().downcast_ref::<Time64MicrosecondArray>().unwrap().value(row),
        ),
        DataType::Decimal128(_, scale) => {
            let v = column.as_any().downcast_ref::<Decimal128Array>().unwrap().value(row);
            decode::render_decimal(&v.to_string(), *scale)
        }
        DataType::Decimal256(_, scale) => {
            let v = column.as_any().downcast_ref::<Decimal256Array>().unwrap().value(row);
            decode::render_decimal(&v.to_string(), *scale)
        }
        DataType::FixedSizeBinary(16) => {
            let bytes = column.as_any().downcast_ref::<FixedSizeBinaryArray>().unwrap().value(row);
            let bytes: &[u8; 16] =
                bytes.try_into().expect("FixedSizeBinary(16) is always 16 bytes");
            decode::render_uuid(bytes)
        }
        DataType::Binary => {
            decode::render_bytea(column.as_any().downcast_ref::<BinaryArray>().unwrap().value(row))
        }
        DataType::Dictionary(k, v) if **k == DataType::Int32 && **v == DataType::Utf8 => {
            let dict = column.as_any().downcast_ref::<DictionaryArray<Int32Type>>().unwrap();
            let values = dict.values().as_any().downcast_ref::<StringArray>().unwrap();
            values.value(dict.keys().value(row) as usize).to_string()
        }
        other => unreachable!("resolve_columns never resolves a column to {other:?}"),
    })
}

/// Walk one `List` level, appending its lengths to `dims` and its leaves to
/// `elements` in row-major order — the flattened shape [`nested::ArrayLiteral`]
/// wants. Rectangularity is not re-checked: only [`append_typed`] fills these
/// columns, and it rejects a value whose shape does not fit.
fn collect_array(
    column: &dyn Array,
    row: usize,
    child_plan: &NestedPlan,
    depth: usize,
    dims: &mut Vec<usize>,
    elements: &mut Vec<Option<String>>,
) {
    let values = column.as_any().downcast_ref::<ListArray>().unwrap().value(row);
    if dims.len() == depth {
        dims.push(values.len());
    }
    for i in 0..values.len() {
        match child_plan {
            NestedPlan::Array(inner) => {
                collect_array(values.as_ref(), i, inner, depth + 1, dims, elements);
            }
            _ => elements.push(render_field(values.as_ref(), i, child_plan)),
        }
    }
}

/// Rebuild an `array_out` literal from a `List` value. Lower bounds are
/// always 1: a decorated value is refused at append time, so no column ever
/// holds one to render back.
fn render_array(column: &dyn Array, row: usize, child_plan: &NestedPlan) -> String {
    let mut dims = Vec::new();
    let mut elements = Vec::new();
    collect_array(column, row, child_plan, 0, &mut dims, &mut elements);
    if elements.is_empty() {
        return "{}".to_string();
    }
    let lower_bounds = vec![1; dims.len()];
    nested::render_array(&nested::ArrayLiteral { elements, dims, lower_bounds })
}

/// Scan `source` end to end, assembling typed `RecordBatch`es for every row
/// of every `COPY` block whose table matches `table` (qualified or bare — see
/// [`CopyHeader::matches`]). A table with zero rows produces no batches.
///
/// Push-mode entry point (`docs/design/architecture.md`, "Execution model and API surface"):
/// internally drains the pull-mode [`crate::stream::table_stream`], so the two
/// share one scan loop. The callback may return [`ControlFlow::Break`] to stop
/// early, in which case the returned token resumes from just past the last
/// batch delivered to it — see [`crate::stream::TableStream::resume_token`].
/// `predicate` applies a post-parse row filter — see `table_stream`'s docs.
/// `cache` controls structure-cache consulting — see `table_stream`'s docs.
/// Rejects `CacheMode::Offline` up front: `source` is mandatory here, and a
/// cache-only mode paired with a live source in hand is a caller contract
/// violation (`docs/design/architecture.md`, "The cache" — `Span::text` is `None` for every `Data` span regardless, so
/// `query` could never answer from a cache alone even if this were allowed).
pub async fn read_table<S, F>(
    source: &S,
    table: &str,
    scan_options: &ScanOptions,
    batch_options: &BatchOptions,
    predicate: Option<Predicate>,
    cache: CacheMode,
    mut on_batch: F,
) -> Result<(crate::resolve::ResolvedSchema, Option<crate::stream::ResumeToken>)>
where
    S: ByteRangeSource,
    F: FnMut(RecordBatch) -> ControlFlow<()>,
{
    use futures::StreamExt;

    if matches!(cache, CacheMode::Offline(_)) {
        return Err(crate::Error::CacheModeMismatch(
            "query requires a live dump source; CacheMode::Offline is cache-only",
        ));
    }

    let mut stream = crate::stream::table_stream(
        source,
        table,
        scan_options.clone(),
        batch_options.clone(),
        predicate,
        None,
        cache,
    );
    while let Some(batch) = stream.next().await.transpose()? {
        if on_batch(batch).is_break() {
            return Ok((stream.resolved_schema(), Some(stream.resume_token())));
        }
    }
    Ok((stream.resolved_schema(), None))
}

#[cfg(test)]
mod tests {
    use arrow::datatypes::Field;

    use super::*;
    use crate::pgtype::RANGE_STRUCT_FIELDS;

    fn list_of(child: DataType) -> DataType {
        DataType::List(Arc::new(Field::new("item", child, true)))
    }

    fn point2d() -> DataType {
        DataType::Struct(Fields::from(vec![
            Field::new("x", DataType::Int32, true),
            Field::new("y", DataType::Utf8View, true),
        ]))
    }

    fn range_struct(bound: DataType) -> DataType {
        DataType::Struct(Fields::from(vec![
            Field::new(RANGE_STRUCT_FIELDS[0], bound.clone(), true),
            Field::new(RANGE_STRUCT_FIELDS[1], bound, true),
            Field::new(RANGE_STRUCT_FIELDS[2], DataType::Boolean, false),
            Field::new(RANGE_STRUCT_FIELDS[3], DataType::Boolean, false),
            Field::new(RANGE_STRUCT_FIELDS[4], DataType::Boolean, false),
        ]))
    }

    fn record(plans: &[NestedPlan]) -> NestedPlan {
        NestedPlan::Record(plans.to_vec())
    }

    fn build(
        data_type: &DataType,
        plan: &NestedPlan,
        values: &[Option<&str>],
    ) -> std::result::Result<ArrayRef, String> {
        let mut builder = new_column_builder(data_type, plan);
        for value in values {
            match value {
                None => append_null(&mut builder),
                Some(text) => append_typed(&mut builder, text)?,
            }
        }
        Ok(finish_column(&mut builder))
    }

    /// Build a column from literals, then render every row back. The array's
    /// own `DataType` must match what the schema promised — `RecordBatch::try_new`
    /// checks exactly this — and the text must survive unchanged.
    #[track_caller]
    fn round_trips(data_type: DataType, plan: NestedPlan, values: &[Option<&str>]) -> ArrayRef {
        let array = build(&data_type, &plan, values).expect("every value here is well-formed");
        assert_eq!(array.data_type(), &data_type, "built array's type must match the schema");
        assert_eq!(array.len(), values.len());
        let rendered: Vec<Option<String>> =
            (0..array.len()).map(|i| render_field(array.as_ref(), i, &plan)).collect();
        let expected: Vec<Option<String>> = values.iter().map(|v| v.map(str::to_string)).collect();
        assert_eq!(rendered, expected);
        array
    }

    #[test]
    fn a_one_dimensional_list_column_round_trips_every_null_and_quoting_case() {
        let array = round_trips(
            list_of(DataType::Utf8View),
            NestedPlan::Array(Box::new(NestedPlan::Scalar)),
            &[Some("{a,b}"), Some("{}"), Some("{NULL}"), Some(r#"{"a,b","has space"}"#), None],
        );
        let list = array.as_any().downcast_ref::<ListArray>().unwrap();
        // `{}` and a NULL array are different: an empty list, and no list.
        assert_eq!(list.value(1).len(), 0);
        assert!(!list.is_null(1));
        assert!(list.is_null(4));
        // `{NULL}` is a one-element list whose element is null.
        assert_eq!(list.value(2).len(), 1);
        assert!(list.value(2).is_null(0));
    }

    #[test]
    fn a_list_of_a_decoded_scalar_type_decodes_its_elements() {
        let array = round_trips(
            list_of(DataType::Int32),
            NestedPlan::Array(Box::new(NestedPlan::Scalar)),
            &[Some("{1,2,3}"), Some("{-1,NULL}"), None],
        );
        let list = array.as_any().downcast_ref::<ListArray>().unwrap();
        let first = list.value(0);
        let ints = first.as_any().downcast_ref::<Int32Array>().unwrap();
        assert_eq!(ints.values(), &[1, 2, 3]);
    }

    #[test]
    fn a_two_dimensional_value_fills_two_list_levels() {
        let plan = NestedPlan::Array(Box::new(NestedPlan::Array(Box::new(NestedPlan::Scalar))));
        let array = round_trips(
            list_of(list_of(DataType::Int32)),
            plan,
            // `{}` fits any depth — `array_out` collapses a zero-element
            // array of every dimensionality to it.
            &[Some("{{1,2},{3,4}}"), Some("{{5},{6},{7}}"), Some("{}"), None],
        );
        let outer = array.as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(outer.value(0).len(), 2);
        assert_eq!(outer.value(1).len(), 3);
        assert_eq!(outer.value(2).len(), 0);
    }

    #[test]
    fn a_value_whose_shape_does_not_fit_the_column_is_refused_rather_than_reshaped() {
        let flat = NestedPlan::Array(Box::new(NestedPlan::Scalar));
        let nested = NestedPlan::Array(Box::new(NestedPlan::Array(Box::new(NestedPlan::Scalar))));
        for (data_type, plan, value) in [
            // A multi-dimensional value in a 1-D column.
            (list_of(DataType::Int32), flat.clone(), "{{1,2},{3,4}}"),
            // A 1-D value in a column resolved as 2-D.
            (list_of(list_of(DataType::Int32)), nested, "{1,2}"),
            // An `[lb:ub]=` prefix: Arrow lists are 0-based with no lower
            // bound, so the index origin has nowhere to go.
            (list_of(DataType::Int32), flat.clone(), "[0:2]={7,8,9}"),
            // Not an array literal at all.
            (list_of(DataType::Int32), flat.clone(), "1,2"),
            // An element that is not a value of the element type.
            (list_of(DataType::Int32), flat, "{x}"),
        ] {
            assert_eq!(
                build(&data_type, &plan, &[Some(value)]).unwrap_err(),
                value,
                "should have been refused: {value}"
            );
        }
    }

    #[test]
    fn a_struct_column_round_trips_a_composite_including_its_null_and_empty_fields() {
        let array = round_trips(
            point2d(),
            record(&[NestedPlan::Scalar, NestedPlan::Scalar]),
            &[Some(r#"(1,"a,b""c")"#), Some(r#"(,"")"#), Some("(3,)"), None],
        );
        let s = array.as_any().downcast_ref::<StructArray>().unwrap();
        assert!(s.is_null(3));
        // A null struct row still gives every child an entry, or the array
        // is malformed.
        assert_eq!(s.column(0).len(), 4);
        assert!(s.column(0).is_null(1), "`(,\"\")` has a NULL first field");
        assert!(!s.column(1).is_null(1), "and an empty-string second one");
    }

    #[test]
    fn a_composite_literal_with_the_wrong_field_count_is_refused() {
        let plan = record(&[NestedPlan::Scalar, NestedPlan::Scalar]);
        for value in ["(1)", "(1,2,3)"] {
            assert_eq!(build(&point2d(), &plan, &[Some(value)]).unwrap_err(), value);
        }
    }

    #[test]
    fn the_range_struct_keeps_empty_apart_from_an_unbounded_range() {
        let array = round_trips(
            range_struct(DataType::Int32),
            NestedPlan::Range(Box::new(NestedPlan::Scalar)),
            &[Some("[1,10)"), Some("empty"), Some("(,5)"), Some("(,)"), None],
        );
        let s = array.as_any().downcast_ref::<StructArray>().unwrap();
        let empty = s.column(4).as_any().downcast_ref::<BooleanArray>().unwrap();
        // `empty` and `(,)` both have absent bounds; only the flag separates
        // them, which is why the fifth field is not redundant.
        assert!(s.column(0).is_null(1) && s.column(0).is_null(3));
        assert!(empty.value(1) && !empty.value(3));
    }

    #[test]
    fn a_range_over_a_text_subtype_keeps_its_quoted_bounds() {
        round_trips(
            range_struct(DataType::Utf8View),
            NestedPlan::Range(Box::new(NestedPlan::Scalar)),
            &[Some(r#"["a,b","c""d")"#), Some(r#"["",a)"#), Some("(,z)")],
        );
    }

    /// The collision `NestedPlan` exists for: one Arrow type, two literal
    /// forms, and nothing in the type to tell them apart.
    #[test]
    fn an_array_of_ranges_and_a_multirange_share_a_type_and_not_a_literal() {
        let bound = Box::new(NestedPlan::Scalar);
        let data_type = list_of(range_struct(DataType::Int32));

        let as_array = round_trips(
            data_type.clone(),
            NestedPlan::Array(Box::new(NestedPlan::Range(bound.clone()))),
            &[Some(r#"{"[1,10)","[2,3)"}"#), Some("{}"), None],
        );
        let as_multirange = round_trips(
            data_type,
            NestedPlan::Multirange(bound),
            &[Some("{[1,10),[2,3)}"), Some("{}"), None],
        );
        assert_eq!(as_array.data_type(), as_multirange.data_type());
        // Same Arrow values, too — only the text differs.
        assert_eq!(as_array.to_data(), as_multirange.to_data());
    }

    #[test]
    fn both_nesting_orders_carry_both_escape_conventions_through_the_builders() {
        // `public.point2d[]` — array quoting outside, record doubling inside.
        round_trips(
            list_of(point2d()),
            NestedPlan::Array(Box::new(record(&[NestedPlan::Scalar, NestedPlan::Scalar]))),
            &[Some(r#"{"(1,\"a,b\"\"c\")","(2,plain)"}"#), Some("{NULL,\"(3,)\"}")],
        );

        // `public.tagged` — the other way up.
        let tagged = DataType::Struct(Fields::from(vec![
            Field::new("label", DataType::Utf8View, true),
            Field::new("tags", list_of(DataType::Utf8View), true),
        ]));
        round_trips(
            tagged,
            record(&[NestedPlan::Scalar, NestedPlan::Array(Box::new(NestedPlan::Scalar))]),
            &[Some(r#"("a,b","{""x\\""y"",""p q"",NULL}")"#), Some("(\"\",{})")],
        );
    }

    /// A batch is only valid if every column's array type matches the schema
    /// field it was built from — the check `RecordBatch::try_new` performs
    /// and the reason the builders reuse the schema's own `Field`s.
    #[test]
    fn nested_columns_assemble_into_a_record_batch() {
        use arrow::datatypes::Schema;

        let fields = vec![
            Field::new("tags", list_of(DataType::Utf8View), true),
            Field::new("pt", point2d(), true),
            Field::new("span", range_struct(DataType::Int32), true),
        ];
        let plans = [
            NestedPlan::Array(Box::new(NestedPlan::Scalar)),
            record(&[NestedPlan::Scalar, NestedPlan::Scalar]),
            NestedPlan::Range(Box::new(NestedPlan::Scalar)),
        ];
        let values: [&[Option<&str>]; 3] =
            [&[Some("{a,b}"), None], &[Some("(1,x)"), None], &[Some("[1,10)"), None]];
        let arrays: Vec<ArrayRef> = fields
            .iter()
            .zip(&plans)
            .zip(values)
            .map(|((f, plan), v)| build(f.data_type(), plan, v).unwrap())
            .collect();
        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap();
        assert_eq!(batch.num_rows(), 2);
    }
}
