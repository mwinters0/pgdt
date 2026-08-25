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
    Int64Array, RecordBatch, StringArray, StringViewArray, Time64MicrosecondArray,
    TimestampMicrosecondArray,
};
use arrow::buffer::Buffer;
use arrow::datatypes::{DataType, Int32Type, SchemaRef, TimeUnit};

use crate::cache::CacheMode;
use crate::copy::{CopyHeader, DELIMITER, decode_field};
use crate::decode;
use crate::io::ByteRangeSource;
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
    TimestampMicro { builder: TimestampMicrosecondBuilder, has_tz: bool },
    Time64Micro(Time64MicrosecondBuilder),
    Decimal128 { builder: Decimal128Builder, scale: i8 },
    Decimal256 { builder: Decimal256Builder, scale: i8 },
    FixedSizeBinary16(FixedSizeBinaryBuilder),
    Binary(BinaryBuilder),
    Dictionary(StringDictionaryBuilder<Int32Type>),
}

fn new_column_builder(data_type: &DataType) -> ColumnBuilder {
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
    }
}

/// Decode `text` (already COPY-unescaped) per `builder`'s type and append it,
/// via `crate::decode`'s per-type decoders. `Err(text)` on a decode failure
/// — the caller wraps it into `Error::FieldDecode` with the table/column/row
/// context this function doesn't have. Never called for `ColumnBuilder::Utf8View`,
/// which the caller handles itself (its zero-copy path needs the raw field's
/// byte offset, which this function never sees).
fn append_typed(builder: &mut ColumnBuilder, text: &str) -> std::result::Result<(), String> {
    let fail = || text.to_string();
    match builder {
        ColumnBuilder::Utf8View(_) => unreachable!("caller handles Utf8View directly"),
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
        let columns = schema.fields().iter().map(|f| new_column_builder(f.data_type())).collect();
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
pub fn render_field(column: &dyn Array, row: usize) -> Option<String> {
    if column.is_null(row) {
        return None;
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
