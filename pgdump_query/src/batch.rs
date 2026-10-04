//! Row/batch assembly: turns rows inside a `COPY` block into typed Arrow
//! `RecordBatch`es, one column builder per [`crate::resolve::ResolvedSchema`]
//! field (`docs/design/decisions.md`, "D46").
//!
//! A `Utf8View` field that needs no unescaping is appended as a zero-copy
//! view into the Arrow `Buffer` backing the read chunk it came from, rather
//! than copied into the builder's own storage
//! (`docs/design/decisions.md`, "D46"). Every other mapped type always
//! copies: its decoded value has its own representation (an `i32`, a
//! `[u8; 16]`, …), not a byte range of the original field.
//!
//! **The chunks those views point into are held here too**, in
//! [`RetainedChunks`]: a read loop says what it read and how far the scanner
//! has got, and this module decides when a chunk becomes an Arrow `Buffer`
//! and how long it is kept, and holds the cached builder block indices the
//! read loop invalidates at a flush (`docs/design/decisions.md`, "D46",
//! "D68").

use std::borrow::Cow;
use std::collections::VecDeque;
use std::ops::{ControlFlow, Range};
use std::sync::Arc;

use arrow::array::builder::{
    BinaryBuilder, BooleanBuilder, Date32Builder, Decimal128Builder, Decimal256Builder,
    FixedSizeBinaryBuilder, Float32Builder, Float64Builder, Int16Builder, Int32Builder,
    Int64Builder, IntervalMonthDayNanoBuilder, StringDictionaryBuilder, StringViewBuilder,
    Time64MicrosecondBuilder, TimestampMicrosecondBuilder, UInt32Builder,
};
use arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array, Decimal256Array,
    DictionaryArray, FixedSizeBinaryArray, Float32Array, Float64Array, Int16Array, Int32Array,
    Int64Array, IntervalMonthDayNanoArray, ListArray, RecordBatch, RecordBatchOptions, StringArray,
    StringViewArray, StructArray, Time64MicrosecondArray, TimestampMicrosecondArray, UInt32Array,
};
use arrow::buffer::{Buffer, NullBuffer, OffsetBuffer, ScalarBuffer};
use arrow::datatypes::{
    DataType, FieldRef, Fields, Int32Type, IntervalMonthDayNano, IntervalUnit, SchemaRef, TimeUnit,
};
use bytes::Bytes;

use crate::cache::CacheMode;
use crate::copy::{RawRow, RowSplit};
use crate::decode;
use crate::index::UnrepresentableTier;
use crate::io::{ByteRangeSource, Parallelism};
use crate::nested::{self, RangeLiteral};
use crate::pgtype::{ComparisonSemantics, NestedPlan};
use crate::scan::PostgresInvalidValues;
// L4, imported by L3: `QueryOptions::filter` is the query's filter tree. One
// of the two deviations `docs/design/decisions.md`, "D68" records.
use crate::predicate::Expr;
use crate::resolve::{ResolvedSchema, SchemaMode};
use crate::scan::ScanOptions;
use crate::statistics::StatisticsView;
use crate::unrepresentable::{UnrepresentableMode, UnrepresentableRead};
// L4, imported by L3: `read_table` is a push-mode entry point that belongs
// in `stream.rs`; the other recorded deviation, named here rather than
// reached for inline so `tests/layering.rs` sees it.
use crate::stream::{ResumeToken, table_stream};
use crate::{Error, Result};

/// What one table query asks for, and how its batches are cut.
///
/// Both halves of a query live here — the projection and the filter terms
/// beside the batching knobs — rather than the query half arriving as
/// positional arguments
/// (`docs/design/decisions.md`, "Batches, streams and the leader").
#[derive(Debug, Clone)]
pub struct QueryOptions {
    /// Which columns to materialize, by name, in the order given. `None`
    /// projects every column of the block; `Some(vec![])` projects none,
    /// which is the `COUNT(*)` shape — every batch then carries a row count
    /// and no arrays. Names are matched against the block's own column list
    /// exactly as [`crate::predicate::Predicate::column`] is, so a name the
    /// block does not carry is `Error::UnknownProjectionColumn` and a
    /// repeated name is `Error::DuplicateProjectionColumn`
    /// (`docs/design/decisions.md`, "D28").
    pub projection: Option<Vec<String>>,
    /// Post-parse row filter (`docs/design/decisions.md`, "D54"),
    /// as a boolean **expression** over single-column terms: a row is kept
    /// only if the root evaluates `Truth::True`. The default is the empty
    /// conjunction, which yields every row, so "no filter" needs no separate
    /// spelling.
    ///
    /// A term may name a column the projection does not: the projection
    /// decides what is *built*, never what may be tested.
    pub filter: Expr,
    /// Which order `filter`'s comparisons answer in: PostgreSQL's, by
    /// default, or DataFusion's order of the value each column emits, which a
    /// comparison this build cannot make that way refuses
    /// ([`ComparisonSemantics`]). Statistics prune a query in either, read
    /// only where they were gathered in its order.
    pub semantics: ComparisonSemantics,
    /// Rows per batch. A batch is flushed once it reaches this many rows.
    pub max_rows: usize,
    /// Optional cap on a batch's total field-byte count — counting only the
    /// fields a projection actually builds, since those are the bytes the
    /// batch holds. Whichever of this or `max_rows` is hit first flushes the
    /// batch.
    pub max_bytes: Option<usize>,
    /// Cap on the source byte span an in-flight batch covers — the distance
    /// from the start of its first selected row to the end of its latest, and
    /// the one trigger that bounds what a batch **pins** on a chunk-shaped
    /// source: the zero-copy `Utf8View` path holds a clone of every chunk it
    /// views until the batch flushes (`docs/design/decisions.md`, "D46").
    /// Defaults to 64 MiB, which no ordinary query reaches; `None` leaves a
    /// batch's span unbounded. What it bounds is the span rounded out to the
    /// retained unit, so on a block-shaped source, whose unit can exceed the
    /// cap, this is a batch-size knob and not the bound.
    ///
    /// **It is a ceiling, not the span a partitioned replay uses.**
    /// [`crate::table_stream_partitions`] derives the span it charges from the
    /// read-buffer budget and the requested worker count, so `jobs` readers
    /// are bought at the cost of batch size rather than declined
    /// (`docs/design/decisions.md`, "D84"). The derivation only moves down,
    /// and never narrows past **one announced read chunk**
    /// ([`crate::scan::ScanOptions::chunk_size_bytes`], which is what a
    /// chunk-shaped source retains) — a span stated below that floor is left
    /// where it was, the floor bounding the derivation rather than the
    /// result; a serial [`crate::table_stream`] does not derive at all.
    pub max_source_span: Option<usize>,
    /// Whether to resolve column types against the dump's DDL
    /// (`docs/design/decisions.md`, "D46"). Every `RecordBatch` this build
    /// produces carries the same schema as its query's
    /// [`crate::resolve::ResolvedSchema`] — a column this build has no
    /// mapping for stays `Utf8View`, as `SchemaMode::Strings` maps every
    /// column.
    pub schema_mode: SchemaMode,
    /// How a value PostgreSQL accepts for a column's declared type and the
    /// column's Arrow type cannot hold is read ([`UnrepresentableMode`]):
    /// NULL, by default, its column read as its text, or refused. Which values those are is the
    /// semantics' to say — Arrow's format spec under PostgreSQL's, and past
    /// the engine's calendar too under DataFusion's
    /// (`docs/design/decisions.md`, "D98"). Moot under
    /// [`SchemaMode::Strings`], which reads every value as its text.
    pub unrepresentable: UnrepresentableMode,
    /// What a read does with a field its type's `*_in` refuses: fails on it,
    /// by default, or decodes it as the decoders read it, which only a
    /// float's past its type's range does ([`PostgresInvalidValues`]). The
    /// typed read and every filter reading the field take it alike; a
    /// filter's literal is read as before.
    pub postgres_invalid_values: PostgresInvalidValues,
    /// Selects which database's table to query when the name alone is
    /// ambiguous — matched against `DatabaseMetadata::name`
    /// (`docs/design/decisions.md`, "D49"). `None` is the common case: a
    /// single-database dump, or an ambiguity a qualified name resolves.
    pub database: Option<String>,
    /// How far a query's mapping scan walks before it starts returning rows
    /// — see [`ScanExtent`].
    pub scan_extent: ScanExtent,
    /// How much concurrency this query may use, and what it may hold while
    /// it does — [`Parallelism::Serial`] by default, and stated here as well
    /// as on [`ScanOptions`] because a query runs two passes with different
    /// shapes: a mapping scan, and a replay split into partitions
    /// (`docs/design/decisions.md`, "D1"). The replay loop announces it to
    /// the source ([`crate::ByteRangeSource::hint_parallelism`]), so the two
    /// passes are bounded separately, and `jobs`, capped by what the bytes
    /// afford, is how many sub-streams [`crate::table_stream_partitions`]
    /// hands back. Nothing here spawns — the caller runs them.
    pub parallelism: Parallelism,
    /// Whether the replay skips the row groups whose statistics — gathered by
    /// a mapping pass ([`crate::stream::map_file`]) — prove no row satisfies
    /// `filter`, and stops reading a block sorted on a column `filter` bounds
    /// at its first row past the bound. On by default; the rows are the same
    /// either way, and `false` reads every row of every block, which is also
    /// how a value it would not read still raises its decode failure
    /// (`docs/design/decisions.md`, "D54"). What was skipped is a
    /// [`crate::stream::PlanNoteKind::StatisticsPruned`]; a stop is found only
    /// as rows are read, so what it left unread is reported after the fact
    /// ([`crate::stream::TableStream::early_stops`]).
    pub use_statistics: bool,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            projection: None,
            filter: Expr::default(),
            semantics: ComparisonSemantics::default(),
            max_rows: 8192,
            max_bytes: None,
            max_source_span: Some(64 << 20),
            schema_mode: SchemaMode::default(),
            unrepresentable: UnrepresentableMode::default(),
            postgres_invalid_values: PostgresInvalidValues::default(),
            database: None,
            scan_extent: ScanExtent::default(),
            parallelism: Parallelism::default(),
            use_statistics: true,
        }
    }
}

impl QueryOptions {
    /// **How this query's statistics are read where a column holds a value
    /// its type cannot**, as its reads and its filter take one: as NULL under
    /// [`UnrepresentableMode::Null`], in the tiers its semantics cannot hold
    /// ([`Self::unrepresentable_reach`]), and otherwise every value in
    /// PostgreSQL's order — under the refuse mode; under the untyped mode,
    /// whose columns holding one are text and compare in that order in
    /// PostgreSQL's semantics, and read no bounds in DataFusion's; and under
    /// [`SchemaMode::Strings`], whose text holds every value
    /// (`docs/design/decisions.md`, "D97", "D98").
    pub fn statistics_view(&self) -> StatisticsView {
        match (self.schema_mode, self.unrepresentable, self.unrepresentable_reach()) {
            (SchemaMode::Typed, UnrepresentableMode::Null, UnrepresentableTier::Format) => {
                StatisticsView::Representable
            }
            (SchemaMode::Typed, UnrepresentableMode::Null, UnrepresentableTier::Engine) => {
                StatisticsView::Displayable
            }
            _ => StatisticsView::Every,
        }
    }

    /// **The tiers of value this query's front end cannot hold**, named by the
    /// wider: every layer holds a value to Arrow's format spec, and DataFusion
    /// — whose comparison [`ComparisonSemantics::DataFusion`] is — cannot display
    /// a `date` or timestamp past [`crate::calendar_end`] either
    /// (`docs/design/decisions.md`, "D98").
    pub(crate) fn unrepresentable_reach(&self) -> UnrepresentableTier {
        match self.semantics {
            ComparisonSemantics::Postgres => UnrepresentableTier::Format,
            ComparisonSemantics::DataFusion => UnrepresentableTier::Engine,
        }
    }
}

/// How far [`crate::stream::table_stream`]'s mapping scan walks
/// (`docs/design/decisions.md`, "D48"). Rows are always replayed from blocks the map
/// already holds, so this controls how much of the file a query pays to map
/// before any row comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanExtent {
    /// Stop as soon as the queried table is settled: at least one matching
    /// block has closed, none of the matching blocks carried a partition-root
    /// marker (I2 — a marked block's name owns further blocks that are *not*
    /// adjacent, so only EOF enumerates them), and the map holds no
    /// `\connect`, after which a name can be defined again. This is what
    /// keeps a query against an early table in a huge dump from costing a
    /// full scan.
    ///
    /// What it gives up: a second, conflicting candidate past the stopping
    /// point is never seen, so `Error::AmbiguousTable` reports only what the
    /// scan reached. A file whose *first* segment has no `\connect` is the
    /// case with no early signal at all.
    ///
    /// Deficiency register: `deficiency: KD6` — so a second, conflicting
    /// table past the stopping point is never seen, `Error::AmbiguousTable` is
    /// not raised for it, and the query answers with the candidate it found.
    /// **(c) unowned**; promoted by a concatenated or `pg_dumpall`-style file
    /// reaching a user through a cold query. [`Self::Full`], or a query after
    /// `pgdt parse`, gives exact detection today, which is why the DataFusion
    /// provider, reading only a complete map, never shows it; closing it by
    /// default means giving up the early stop.
    #[default]
    UntilTargetSettled,
    /// Map the whole file before returning anything. Costs a full scan and
    /// gives exact ambiguity detection and a complete, reusable cache — the
    /// same map `pgdt parse` builds.
    Full,
}

/// A read chunk retained only long enough for zero-copy views to be taken
/// into it. Dropped once the scanner has moved past it for good.
struct SourceChunk {
    /// Absolute file offset of `buffer[0]`.
    start: u64,
    buffer: Buffer,
    /// Cached `StringViewBuilder::append_block` index per column, filled in
    /// the first time a column takes a view into this chunk. Grown lazily,
    /// since more than one schema can reference the same chunk.
    column_blocks: Vec<Option<u32>>,
}

impl SourceChunk {
    fn end(&self) -> u64 {
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

/// The read chunks a batch's zero-copy views may still point into, in file
/// order. A replay loop hands each chunk it reads to [`Self::retain`] and
/// tells it where the scanner has got to; everything else about the
/// arrangement — that a chunk becomes an Arrow `Buffer` at all, that the
/// buffer is what a view is taken against, that a flush leaves a cached
/// block index stale ([`Self::invalidate_block_cache`], which the loop calls)
/// — is Arrow assembly's business, which is why this type lives
/// beside the builders (`docs/design/decisions.md`, "D68").
pub(crate) struct RetainedChunks {
    chunks: VecDeque<SourceChunk>,
}

impl RetainedChunks {
    pub(crate) fn new() -> Self {
        Self { chunks: VecDeque::new() }
    }

    /// Retain one read chunk, `start` being the absolute file offset of its
    /// first byte.
    ///
    /// **This is the one place a read chunk becomes an `arrow::Buffer`.** The
    /// `Bytes` is cloned rather than consumed because the caller still scans
    /// it; both refer to the same allocation, which the buffer pool reclaims
    /// only when the last reference dies (`docs/design/decisions.md`, "D9").
    pub(crate) fn retain(&mut self, start: u64, bytes: &Bytes) {
        self.chunks.push_back(SourceChunk {
            start,
            buffer: Buffer::from(bytes.clone()),
            column_blocks: Vec::new(),
        });
    }

    /// Drop every chunk the scanner has walked entirely past — viewing
    /// happens synchronously as rows are pushed, so everything before `floor`
    /// has had its chance.
    ///
    /// **The chunk the scanner is inside is retained**: `end() <= floor`
    /// holds only past a chunk's last byte, and the row straddling the next
    /// boundary is carried rather than scanned, so it cannot arrive needing a
    /// chunk that has gone.
    pub(crate) fn release_through(&mut self, floor: u64) {
        while self.chunks.front().is_some_and(|c| c.end() <= floor) {
            self.chunks.pop_front();
        }
    }

    /// `StringViewBuilder::finish()` resets its internal block list, which
    /// invalidates every cached block index held here: a flush must clear
    /// them all, or a later reference to an already-seen chunk resolves to
    /// the wrong block in the new batch.
    pub(crate) fn invalidate_block_cache(&mut self) {
        for chunk in self.chunks.iter_mut() {
            chunk.column_blocks.clear();
        }
    }
}

/// One column's typed builder, chosen from a [`crate::resolve::ResolvedSchema`]
/// field's [`DataType`] — the complete set [`crate::pgtype::resolve_declared_type`]
/// and [`crate::resolve::resolve_columns`] can ever produce.
/// `with_precision_and_scale`/`with_timezone_opt` tag the three builders
/// whose default type is not the schema's — the two decimals and the
/// timestamp — every other arm's default already matching, so a `finish()`ed array's type matches the schema exactly, which
/// `RecordBatch::try_new` checks.
enum ColumnBuilder {
    /// A text, and a `character varying(n)`'s or `character(n)`'s length,
    /// past which a field is refused ([`decode::char_typmod_refuses`]).
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
    /// (`docs/design/decisions.md`, "D39").
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
        NestedPlan::Scalar | NestedPlan::Decimal { .. } | NestedPlan::Text { .. } => {}
        NestedPlan::Array(child) => {
            return ColumnBuilder::Array(new_list_parts(data_type, child));
        }
        NestedPlan::Multirange(bound) => {
            let range = NestedPlan::Range(bound.clone());
            return ColumnBuilder::Multirange(new_list_parts(data_type, &range));
        }
        NestedPlan::Int2Vector => {
            return ColumnBuilder::Int2Vector(new_list_parts(data_type, &NestedPlan::Scalar));
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

fn append_null(builder: &mut ColumnBuilder) {
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
/// inside a nested literal to the whole literal.
fn append_nested(
    builder: &mut ColumnBuilder,
    value: Option<&str>,
    invalid: PostgresInvalidValues,
) -> std::result::Result<(), ()> {
    match value {
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
            append_nested(&mut parts.child, elements.next().ok_or(())?.as_deref(), invalid)?;
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
    append_nested(lower, range.lower.as_deref(), invalid)?;
    append_nested(upper, range.upper.as_deref(), invalid)?;
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
fn text_refused(text: &str, length: Option<u32>, invalid: PostgresInvalidValues) -> bool {
    invalid != PostgresInvalidValues::Ignore
        && length.is_some_and(|length| decode::char_typmod_refuses(text, length))
}

/// Decode `text` (already COPY-unescaped) per `builder`'s type and append it,
/// via `crate::decode`'s per-type decoders and `crate::nested`'s literal
/// codecs. `Err(text)` on a decode failure — the caller wraps it into
/// `Error::FieldDecode` with the table/column/row context. Never called for a
/// *top-level* `ColumnBuilder::Utf8View`, which `push_field` handles itself
/// (its zero-copy path needs the raw field's byte offset); a nested one
/// reaches [`append_nested`] instead and copies. `invalid` is how a field its
/// type's `*_in` refuses is read, at any depth.
fn append_typed(
    builder: &mut ColumnBuilder,
    text: &str,
    invalid: PostgresInvalidValues,
) -> std::result::Result<(), String> {
    let fail = || text.to_string();
    match builder {
        ColumnBuilder::Utf8View(..) => unreachable!("caller handles Utf8View directly"),
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
            append_array_level(parts, &literal.dims, &mut literal.elements.iter(), invalid)
                .map_err(|()| fail())?;
        }
        ColumnBuilder::Multirange(parts) => {
            let members = nested::decode_multirange(text).ok_or_else(fail)?;
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
            let values = nested::decode_int2vector(text).ok_or_else(fail)?;
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
                append_nested(child, field.as_deref(), invalid).map_err(|()| fail())?;
            }
            parts.validity.push(true);
        }
        ColumnBuilder::Range(parts) => {
            let literal = nested::decode_range(text).ok_or_else(fail)?;
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
            b.append_value(decode::float_field(text, invalid).ok_or_else(fail)?);
        }
        ColumnBuilder::Float64(b) => {
            b.append_value(decode::float_field(text, invalid).ok_or_else(fail)?);
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
        // one whose ordering term keys it, though a strict parse refuses it
        // wherever it sits. Refusing it here needs the labels and their
        // exactness, which this builder does not carry.
        ColumnBuilder::Dictionary(b) => b.append_value(text),
    }
    Ok(())
}

fn finish_column(builder: &mut ColumnBuilder) -> ArrayRef {
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

/// Accumulates rows from a single `COPY` block into typed `RecordBatch`es,
/// one [`ColumnBuilder`] per field of the query's
/// [`crate::resolve::ResolvedSchema`].
pub(crate) struct RowBatcher {
    schema: SchemaRef,
    /// Qualified table name, for `Error::FieldDecode`'s context.
    table: String,
    /// One entry per field of the **block's own** column list, in file
    /// order: the `columns` index that field feeds, or `None` for a field no
    /// projection asked for. Its length — not `columns.len()` — is the field
    /// count a row is checked against.
    field_targets: Vec<Option<usize>>,
    /// Parallel to `schema.fields()` — the declared PostgreSQL type string
    /// behind each `Mapped` column, for the same error.
    declared_types: Vec<Option<String>>,
    columns: Vec<ColumnBuilder>,
    rows_in_batch: usize,
    bytes_in_batch: usize,
    /// The source byte span the in-flight batch covers: `Some((start, end))`
    /// once a row has been pushed, where `start` is the first pushed row's
    /// offset and `end` is one past the latest pushed row's last byte. A row
    /// a predicate rejected never reaches `push_row`, so it neither opens a
    /// span nor extends one, which is what stops an empty batch from flushing
    /// across a long stretch that matches nothing.
    span: Option<(u64, u64)>,
    /// Whether any field of this block feeds a projected column. When
    /// nothing does — `COUNT(*)`, `pgdt query --no-columns` — the batcher
    /// decodes no field, and where the filter reads none either the read loop
    /// skips the bulk UTF-8 validation (`docs/design/decisions.md`, "D27").
    decodes_fields: bool,
    /// Parallel to `columns`: how each projected column's values its type
    /// cannot hold are read, where it can hold one
    /// ([`crate::unrepresentable::unrepresentable_reads`]).
    unrepresentable: Vec<Option<UnrepresentableRead>>,
    options: QueryOptions,
}

impl RowBatcher {
    /// `resolved` is the **projected** schema — what this batcher's
    /// `RecordBatch`es carry — and `field_targets` maps the block's own
    /// fields onto it. Both come from `crate::stream::project`;
    /// `unrepresentable` is parallel to `field_targets`, `None` for each
    /// field that is not tested.
    pub(crate) fn new(
        resolved: &ResolvedSchema,
        table: String,
        options: QueryOptions,
        field_targets: Vec<Option<usize>>,
        unrepresentable: Vec<Option<UnrepresentableRead>>,
    ) -> Self {
        let schema = resolved.schema.clone();
        let mut projected = vec![None; schema.fields().len()];
        for (field, target) in field_targets.iter().enumerate() {
            if let Some(target) = *target {
                projected[target] = unrepresentable.get(field).cloned().flatten();
            }
        }
        let declared_types = resolved.notes.iter().map(|n| n.declared.clone()).collect();
        // `plans` is positional and parallel to `schema.fields()`:
        // `resolve_columns` fills one entry per column, `NestedPlan::Scalar`
        // included.
        let columns = schema
            .fields()
            .iter()
            .zip(&resolved.plans)
            .map(|(f, plan)| new_column_builder(f.data_type(), plan))
            .collect();
        Self {
            schema,
            table,
            decodes_fields: field_targets.iter().any(Option::is_some),
            field_targets,
            declared_types,
            columns,
            rows_in_batch: 0,
            bytes_in_batch: 0,
            span: None,
            unrepresentable: projected,
            options,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rows_in_batch == 0
    }

    /// Where in the source the in-flight batch begins — the offset of its
    /// first pushed row — or `None` while no row has landed in it. It is the
    /// `span`'s lower bound, so read it **before** [`Self::flush`], which
    /// clears the span. It is the key a caller merging several partitions of
    /// one replay back into file order sorts on
    /// (`docs/design/decisions.md`, "D51").
    pub(crate) fn batch_start(&self) -> Option<u64> {
        self.span.map(|(start, _)| start)
    }

    /// The block's qualified table name — context for the
    /// `Error::FieldDecode` an ordering predicate raises on a value that is
    /// not of its mapped type, worded as this batcher words its own.
    pub(crate) fn table(&self) -> &str {
        &self.table
    }

    /// Whether pushing a row here decodes anything — see the field.
    pub(crate) fn decodes_fields(&self) -> bool {
        self.decodes_fields
    }

    /// Whether any of the three flush triggers has fired. All three are
    /// evaluated after a row has been appended, so each may be overshot by at
    /// most one row. The span trigger cannot fire on an empty batch whatever
    /// its cap, `span` staying `None` until a row lands.
    pub(crate) fn should_flush(&self) -> bool {
        self.rows_in_batch >= self.options.max_rows
            || self.options.max_bytes.is_some_and(|max| self.bytes_in_batch >= max)
            || self.source_span_reached()
    }

    fn source_span_reached(&self) -> bool {
        let (Some(max), Some((start, end))) = (self.options.max_source_span, self.span) else {
            return false;
        };
        end.saturating_sub(start) >= max as u64
    }

    /// Append one raw (still-escaped) COPY TEXT data row.
    ///
    /// **The whole row is walked whatever the projection is.** Skipping is
    /// per column — `decode_field` and the builder append — never an early
    /// stop at the last projected field, because this is the system's only
    /// field-count check: the sole site that raises
    /// `Error::ColumnCountMismatch`, the mapping pass never erroring on a
    /// count (`docs/design/decisions.md`, "D28").
    ///
    /// **The walk is `split`'s**, which the caller has already offered to the
    /// filter, so a boundary a term crossed is not crossed again here and one
    /// nothing read is found now. There is no second entry point walking the
    /// row directly for the unfiltered case
    /// (`docs/design/decisions.md`, "D28"; `measurements.md`,
    /// `predicate-terms`).
    ///
    /// **A block of zero fields takes only empty lines** — one listing no
    /// columns (I5) — where `split` reads an empty line as one empty field,
    /// so any other line is the mismatch it is against a wider block.
    pub(crate) fn push_row(
        &mut self,
        header_offset: u64,
        row_offset: u64,
        row: RawRow<'_>,
        split: &mut RowSplit,
        chunks: &mut RetainedChunks,
    ) -> Result<()> {
        let raw = row.bytes();
        let expected = self.field_targets.len();
        let ends: &[usize] =
            if expected == 0 && raw.is_empty() { &[] } else { split.complete(raw) };
        let found = ends.len();
        if found != expected {
            return Err(Error::ColumnCountMismatch {
                header_offset,
                row_offset,
                expected,
                // Saturated one past the width, though the split counted
                // every field of the row.
                found: if found > expected { expected + 1 } else { found },
            });
        }
        let mut pos = 0usize;
        for (col, &end) in ends.iter().enumerate() {
            if let Some(target) = self.field_targets[col] {
                self.push_field(
                    target,
                    row_offset,
                    row_offset + pos as u64,
                    row,
                    pos..end,
                    chunks,
                )?;
                self.bytes_in_batch += end - pos;
            }
            pos = end + 1;
        }
        self.rows_in_batch += 1;
        let row_end = row_offset + raw.len() as u64;
        self.span = Some(match self.span {
            Some((start, _)) => (start, row_end),
            None => (row_offset, row_end),
        });
        Ok(())
    }

    /// Append one field's text to projected column `col`.
    ///
    /// **A value the query reads as NULL is NULL before it is decoded**, as a
    /// whole: an array, range or composite holding one leaf its type cannot
    /// hold is the NULL, a NULL range bound meaning unbounded
    /// (`docs/design/decisions.md`, "D98"). The refuse mode's plan refused
    /// any column holding one before a row was read, so a value that does not
    /// decode here does not parse: [`Error::FieldDecode`].
    fn push_field(
        &mut self,
        col: usize,
        row_offset: u64,
        field_offset: u64,
        row: RawRow<'_>,
        field: Range<usize>,
        chunks: &mut RetainedChunks,
    ) -> Result<()> {
        let field_len = field.len();
        let decoded = row.decode(field)?;
        // Disjoint-field borrow: `columns[col]` is mutated below while
        // `schema`/`table`/`declared_types` are only ever read, on the
        // (rare) error path.
        let Self { schema, table, declared_types, columns, unrepresentable, options, .. } = self;
        let builder = &mut columns[col];
        let Some(text) = decoded else {
            append_null(builder);
            return Ok(());
        };
        let read = unrepresentable[col].as_ref();
        if read.is_some_and(|read| read.cannot_hold(&text)) {
            append_null(builder);
            return Ok(());
        }
        let appended = match builder {
            ColumnBuilder::Utf8View(_, length)
                if text_refused(&text, *length, options.postgres_invalid_values) =>
            {
                Err(text.into_owned())
            }
            ColumnBuilder::Utf8View(b, _) => {
                push_utf8view_field(b, col, field_offset, field_len, text, chunks);
                Ok(())
            }
            _ => append_typed(builder, &text, options.postgres_invalid_values),
        };
        appended.map_err(|value| Error::FieldDecode {
            table: table.clone(),
            column: schema.field(col).name().clone(),
            line_offset: row_offset,
            declared_type: declared_types[col].clone().unwrap_or_default(),
            value,
        })
    }

    /// Finish the in-flight batch. The row count is passed explicitly rather
    /// than inferred from the arrays, since a zero-column projection has none
    /// to infer it from; stating it unconditionally keeps one path for both
    /// widths, and `try_new_with_options` still checks every array against
    /// it.
    pub(crate) fn flush(&mut self) -> Result<RecordBatch> {
        let rows = self.rows_in_batch;
        self.rows_in_batch = 0;
        self.bytes_in_batch = 0;
        self.span = None;
        let arrays: Vec<ArrayRef> = self.columns.iter_mut().map(finish_column).collect();
        Ok(RecordBatch::try_new_with_options(
            self.schema.clone(),
            arrays,
            &RecordBatchOptions::new().with_row_count(Some(rows)),
        )?)
    }
}

/// Append one already-decoded field to a `Utf8View` column, beside the raw
/// field's offset and length. The caller decodes, so the escaping rules live
/// in one place; a `Cow::Borrowed` result (no escapes present, already UTF-8
/// checked) is what makes the field eligible for a zero-copy view —
/// everything else is copied.
fn push_utf8view_field(
    builder: &mut StringViewBuilder,
    col: usize,
    field_offset: u64,
    field_len: usize,
    text: Cow<'_, str>,
    chunks: &mut RetainedChunks,
) {
    match text {
        Cow::Owned(s) => builder.append_value(s),
        Cow::Borrowed(s) => {
            let view = chunks
                .chunks
                .iter_mut()
                .find_map(|c| c.contains(field_offset, field_len).map(|coords| (c, coords)));
            match view {
                Some((chunk, (local_offset, len))) => {
                    let block = chunk.block_for(col, builder);
                    // SAFETY: `contains` confirmed `local_offset..local_offset+len`
                    // is in bounds for `block`, and the `Borrowed` case above
                    // means the decode already validated this exact byte
                    // range as UTF-8. `contains` narrows to `u32`, so this
                    // holds while a retained chunk stays under 4 GiB, which
                    // every shipped chunk and block size is by orders of
                    // magnitude; nothing clamps `ScanOptions::chunk_size_bytes` to
                    // enforce it.
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
/// `crate::copy::decode_field` would have produced for it
/// (`docs/design/decisions.md`, "D66"), and the round-trip tests' oracle.
/// `Ok(None)` for SQL NULL. Covers exactly the [`DataType`]s
/// [`crate::resolve::resolve_columns`] can ever produce.
///
/// **The `Result` is for a value with no text form at all**, which is a third
/// outcome and not a NULL: [`Error::FieldRender`] says which, and a value is
/// never rounded into range (`docs/design/decisions.md`, "D44"). Nothing this
/// crate builds can reach it, so it is a statement about arrays a caller
/// assembled itself.
///
/// `plan` is needed for the same reason [`ColumnBuilder`] needs it: the Arrow
/// type does not say which literal form a nested value is written in, and
/// `int4range[]` and `int4multirange` share one. It comes from
/// [`crate::resolve::ResolvedSchema::plans`], positionally; a caller that
/// knows its column is scalar passes `&NestedPlan::Scalar`, which is
/// [`NestedPlan`]'s `Default`. There is no plan-less entry point
/// (`docs/design/decisions.md`, "D39").
pub fn render_field(column: &dyn Array, row: usize, plan: &NestedPlan) -> Result<Option<String>> {
    // Checked here rather than left to the sink so a SQL NULL costs no
    // allocation at all.
    if column.is_null(row) {
        return Ok(None);
    }
    // Sized rather than empty: a zero-capacity `String` is grown by whichever
    // `push_str` writes into it first, where one allocation suffices for a
    // value of 16 bytes or fewer.
    let mut out = String::with_capacity(16);
    if render_field_into(column, row, plan, &mut out)? { Ok(Some(out)) } else { Ok(None) }
}

/// [`render_field`]'s inverse for one field: `text`, unescaped as a row's
/// field arrives, as the one-element array a batch of this column would hold —
/// or `None` where the text is not a value of the column's type, which is
/// [`Error::FieldDecode`]'s condition without the row that raises it.
///
/// It exists so that a caller holding field text out of band — a stored bound
/// ([`crate::statistics::Bounds`]) — reads it back through the one decoder,
/// rather than a second one that could disagree with what a scan emits.
/// `plan` travels beside the Arrow type as it does for [`render_field`].
pub fn decode_field(data_type: &DataType, plan: &NestedPlan, text: &str) -> Option<ArrayRef> {
    let mut builder = new_column_builder(data_type, plan);
    match &mut builder {
        // `append_typed` leaves this one to its caller, the scan's path being
        // the borrowing one.
        ColumnBuilder::Utf8View(_, length)
            if text_refused(text, *length, PostgresInvalidValues::Default) =>
        {
            return None;
        }
        ColumnBuilder::Utf8View(b, _) => b.append_value(text),
        _ => append_typed(&mut builder, text, PostgresInvalidValues::Default).ok()?,
    }
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
            render_array_into(column, row, child, out, &mut scratch, &mut dims)?;
            return Ok(true);
        }
        NestedPlan::Multirange(bound) => {
            let list = column.as_any().downcast_ref::<ListArray>().unwrap();
            let members = list.value(row);
            let range = NestedPlan::Range(bound.clone());
            let rendered: Vec<String> = (0..members.len())
                .map(|i| {
                    Ok(render_field(members.as_ref(), i, &range)?
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
            out.push_str(&nested::render_int2vector(values.values()));
            return Ok(true);
        }
        NestedPlan::Record(field_plans) => {
            let s = column.as_any().downcast_ref::<StructArray>().unwrap();
            let fields: Vec<Option<String>> = s
                .columns()
                .iter()
                .zip(field_plans)
                .map(|(child, p)| render_field(child.as_ref(), row, p))
                .collect::<Result<_>>()?;
            out.push_str(&nested::render_record(&nested::RecordLiteral { fields }));
            return Ok(true);
        }
        NestedPlan::Range(bound) => {
            let s = column.as_any().downcast_ref::<StructArray>().unwrap();
            let flag =
                |i: usize| s.column(i).as_any().downcast_ref::<BooleanArray>().unwrap().value(row);
            out.push_str(&nested::render_range(&RangeLiteral {
                empty: flag(4),
                lower: render_field(s.column(0).as_ref(), row, bound)?,
                upper: render_field(s.column(1).as_ref(), row, bound)?,
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
            decode::push_integer(
                out,
                i64::from(column.as_any().downcast_ref::<Int16Array>().unwrap().value(row)),
            );
        }
        DataType::Int32 => {
            decode::push_integer(
                out,
                i64::from(column.as_any().downcast_ref::<Int32Array>().unwrap().value(row)),
            );
        }
        DataType::Int64 => {
            decode::push_integer(
                out,
                column.as_any().downcast_ref::<Int64Array>().unwrap().value(row),
            );
        }
        DataType::UInt32 => {
            decode::push_integer(
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
/// walking the Arrow list rather than building a [`nested::ArrayLiteral`].
///
/// Each element is rendered where it will be read, and
/// [`nested::quote_array_element`] moves it aside only if the grammar wants
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
            render_list_level(values.as_ref(), i, inner, depth + 1, dims, out, scratch, leaves)?;
            continue;
        }
        let mark = out.len();
        if render_field_into(values.as_ref(), i, child_plan, out)? {
            nested::quote_array_element(out, mark, scratch);
        } else {
            nested::push_array_null(out);
        }
        *leaves += 1;
    }
    out.push('}');
    Ok(())
}

/// Scan `source` as far as `query_options.scan_extent` says, assembling typed
/// `RecordBatch`es for every row
/// of every `COPY` block whose table matches `table` (qualified or bare — see
/// [`CopyHeader::matches`]). A table with zero rows produces no batches.
///
/// Push-mode entry point (`docs/design/decisions.md`, "D68"): internally
/// drains the pull-mode [`crate::stream::table_stream`], so the two share one
/// scan loop. The callback may return [`ControlFlow::Break`] to stop early,
/// in which case the returned token resumes from just past the last batch
/// delivered to it — see [`crate::stream::TableStream::resume_token`].
/// `query_options` and `cache` mean what they do on `table_stream`. Rejects
/// `CacheMode::Offline` up front: `source` is mandatory here, and `Span::text`
/// is `None` for every `Data` span regardless, so `query` could never answer
/// from a cache alone (`docs/design/decisions.md`, "D30").
pub async fn read_table<F>(
    source: &dyn ByteRangeSource,
    table: &str,
    scan_options: &ScanOptions,
    query_options: &QueryOptions,
    cache: CacheMode,
    mut on_batch: F,
) -> Result<(ResolvedSchema, Option<ResumeToken>)>
where
    F: FnMut(RecordBatch) -> ControlFlow<()>,
{
    use futures::StreamExt;

    if matches!(cache, CacheMode::Offline(_)) {
        return Err(Error::CacheModeMismatch(
            "query requires a live dump source; CacheMode::Offline is cache-only",
        ));
    }

    let mut stream =
        table_stream(source, table, scan_options.clone(), query_options.clone(), None, cache);
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
                Some(text) => append_typed(&mut builder, text, PostgresInvalidValues::Default)?,
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
        let rendered: Vec<Option<String>> = (0..array.len())
            .map(|i| render_field(array.as_ref(), i, &plan).expect("renders back"))
            .collect();
        let expected: Vec<Option<String>> = values.iter().map(|v| v.map(str::to_string)).collect();
        assert_eq!(rendered, expected);
        array
    }

    /// An `Interval(MonthDayNano)` array holding a value no `interval` has.
    /// Built by hand because nothing in this crate can produce one —
    /// `append_typed` fills the column from `decode_interval`, which
    /// multiplies microseconds by a thousand — so the refusal it pins is the
    /// contract for an array a caller assembled. It travels out of a nested
    /// column too, the nested walk being the same function.
    #[test]
    fn render_refuses_an_interval_with_no_postgresql_text_form() {
        let mut b = IntervalMonthDayNanoBuilder::new();
        b.append_value(IntervalMonthDayNano { months: 0, days: 0, nanoseconds: 1_500 });
        b.append_value(IntervalMonthDayNano { months: 0, days: 0, nanoseconds: 1_000 });
        let array = Arc::new(b.finish()) as ArrayRef;

        let err = render_field(array.as_ref(), 0, &NestedPlan::Scalar).unwrap_err();
        assert!(
            matches!(&err, Error::FieldRender { declared_type: "interval", reason } if reason.contains("1500 ns")),
            "{err}"
        );
        assert_eq!(
            render_field(array.as_ref(), 1, &NestedPlan::Scalar).unwrap().as_deref(),
            Some("00:00:00.000001"),
        );

        let list = ListArray::new(
            Arc::new(Field::new("item", DataType::Interval(IntervalUnit::MonthDayNano), true)),
            OffsetBuffer::from_lengths([2usize]),
            array,
            None,
        );
        let plan = NestedPlan::Array(Box::new(NestedPlan::Scalar));
        assert!(matches!(
            render_field(&list, 0, &plan).unwrap_err(),
            Error::FieldRender { declared_type: "interval", .. }
        ));
    }

    /// `int2vector` and `smallint[]` are one Arrow type and two literal
    /// forms — [`NestedPlan`]'s collision — so the two are built side by
    /// side, from text neither could read as the other.
    #[test]
    fn an_int2vector_column_round_trips_and_is_not_the_array_of_the_same_type() {
        let array = round_trips(
            list_of(DataType::Int16),
            NestedPlan::Int2Vector,
            &[Some("1 2 3"), Some(""), Some("-32768 32767"), Some("0"), None],
        );
        let list = array.as_any().downcast_ref::<ListArray>().unwrap();
        // The empty vector is the empty *field*, and `\N` is the NULL beside
        // it: an empty list, and no list.
        assert_eq!(list.value(1).len(), 0);
        assert!(!list.is_null(1));
        assert!(list.is_null(4));
        assert_eq!(list.value(0).len(), 3);

        // Neither grammar reads the other's text.
        let array_plan = NestedPlan::Array(Box::new(NestedPlan::Scalar));
        assert!(build(&list_of(DataType::Int16), &array_plan, &[Some("1 2 3")]).is_err());
        assert!(
            build(&list_of(DataType::Int16), &NestedPlan::Int2Vector, &[Some("{1,2}")]).is_err()
        );
    }

    /// The one value class an `int2vector` column cannot hold: PostgreSQL's
    /// `int2vector` has no NULL element, so a `List<Int16>` carrying one has
    /// no `int2vectorout` text at all. Built by hand for the reason the
    /// `interval` refusal above is — `append_typed` can never produce one.
    #[test]
    fn render_refuses_an_int2vector_holding_a_null_element() {
        let mut values = Int16Builder::new();
        values.append_value(1);
        values.append_null();
        let list = ListArray::new(
            Arc::new(Field::new("item", DataType::Int16, true)),
            OffsetBuffer::from_lengths([2usize]),
            Arc::new(values.finish()) as ArrayRef,
            None,
        );
        let err = render_field(&list, 0, &NestedPlan::Int2Vector).unwrap_err();
        assert!(
            matches!(&err, Error::FieldRender { declared_type: "int2vector", reason }
                if reason.contains("NULL element")),
            "{err}"
        );
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

    /// A [`RowBatcher`] over one nullable `data_type` column, fed by the field
    /// `field_targets` says — for driving `push_row` at chosen file offsets,
    /// the flush triggers being arithmetic over offsets and row lengths.
    fn one_column_batcher_fed_by(
        options: QueryOptions,
        field_targets: Vec<Option<usize>>,
        data_type: DataType,
    ) -> RowBatcher {
        use arrow::datatypes::Schema;

        use crate::pgtype::comparison_for;
        use crate::resolve::{ColumnNote, ColumnResolution};

        let declared = if data_type == DataType::Int32 { "integer" } else { "text" };
        let resolved = ResolvedSchema {
            schema: Arc::new(Schema::new(vec![Field::new("v", data_type, true)])),
            columns: vec![ColumnResolution::Mapped],
            notes: vec![ColumnNote {
                column: "v".into(),
                declared: Some(declared.into()),
                resolution: ColumnResolution::Mapped,
            }],
            plans: vec![NestedPlan::Scalar],
            comparisons: vec![comparison_for(declared, None, &[], &[])],
        };
        RowBatcher::new(&resolved, "public.t".into(), options, field_targets, Vec::new())
    }

    /// The unprojected case: one `Utf8View` column, fed by the row's only
    /// field.
    fn one_column_batcher(options: QueryOptions) -> RowBatcher {
        one_column_batcher_fed_by(options, vec![Some(0)], DataType::Utf8View)
    }

    /// A field no projection asked for is walked and skipped: never decoded
    /// — the first field here would be a hard `Int32` decode failure if it
    /// were — and not counted towards `max_bytes`.
    #[test]
    fn an_unprojected_field_is_walked_but_never_decoded() {
        let options = QueryOptions { max_bytes: Some(4), ..Default::default() };
        let mut batcher = one_column_batcher_fed_by(options, vec![None, Some(0)], DataType::Int32);
        let mut chunks = RetainedChunks::new();
        batcher
            .push_row(
                0,
                0,
                RawRow::unchecked(b"not an integer\t77"),
                &mut RowSplit::default(),
                &mut chunks,
            )
            .unwrap();
        assert!(!batcher.should_flush(), "only the projected field's bytes count");
        batcher
            .push_row(
                0,
                20,
                RawRow::unchecked(b"nor is this\t88"),
                &mut RowSplit::default(),
                &mut chunks,
            )
            .unwrap();
        assert!(batcher.should_flush(), "two projected bytes, then four");
        let batch = batcher.flush().unwrap();
        assert_eq!(batch.num_columns(), 1);
        assert_eq!(rendered(&batch), vec![Some("77".to_string()), Some("88".to_string())]);
    }

    /// A row is checked against the **block's** field count, not the
    /// projection's width — `push_row` is the system's only field-count
    /// check, and a projection must not weaken it.
    #[test]
    fn the_field_count_check_is_against_the_block_not_the_projection() {
        let mut batcher = one_column_batcher_fed_by(
            QueryOptions::default(),
            vec![None, Some(0), None],
            DataType::Utf8View,
        );
        let mut chunks = RetainedChunks::new();
        let err = batcher
            .push_row(0, 0, RawRow::unchecked(b"a\t1"), &mut RowSplit::default(), &mut chunks)
            .unwrap_err();
        match err {
            Error::ColumnCountMismatch { expected, found, .. } => {
                assert_eq!((expected, found), (3, 2));
            }
            other => panic!("expected ColumnCountMismatch, got {other:?}"),
        }
    }

    /// A row a term has already begun splitting is finished by the batcher,
    /// not re-walked, and gives the same answer as a split nothing touched.
    #[test]
    fn a_partly_filled_split_gives_the_same_row_as_a_fresh_one() {
        let rows: [&[u8]; 3] = [b"a\t1\tz", b"x\t\\N\ty", b"\t2\t"];
        for row in rows {
            let targets = vec![None, Some(0), None];
            let mut fresh = one_column_batcher_fed_by(
                QueryOptions::default(),
                targets.clone(),
                DataType::Utf8View,
            );
            let mut shared =
                one_column_batcher_fed_by(QueryOptions::default(), targets, DataType::Utf8View);
            let mut chunks = RetainedChunks::new();
            let mut split = RowSplit::default();
            fresh.push_row(0, 0, RawRow::unchecked(row), &mut split, &mut chunks).unwrap();
            split.restart();
            // What a `--filter` on the middle column reads first.
            assert!(split.field(row, 1).is_some(), "{row:?}");
            shared.push_row(0, 0, RawRow::unchecked(row), &mut split, &mut chunks).unwrap();
            assert_eq!(rendered(&fresh.flush().unwrap()), rendered(&shared.flush().unwrap()));
        }
    }

    /// A row wider than the block is refused with the count saturated one
    /// past the width, not the row's real one.
    #[test]
    fn a_row_wider_than_the_block_reports_one_past_the_width() {
        let mut batcher = one_column_batcher_fed_by(
            QueryOptions::default(),
            vec![None, Some(0)],
            DataType::Utf8View,
        );
        let mut chunks = RetainedChunks::new();
        let err = batcher
            .push_row(
                0,
                0,
                RawRow::unchecked(b"a\t1\tz\textra"),
                &mut RowSplit::default(),
                &mut chunks,
            )
            .unwrap_err();
        match err {
            Error::ColumnCountMismatch { expected, found, .. } => {
                assert_eq!((expected, found), (2, 3));
            }
            other => panic!("expected ColumnCountMismatch, got {other:?}"),
        }
    }

    /// A zero-column projection builds nothing and still reports its rows —
    /// what `RecordBatch::try_new` cannot express.
    #[test]
    fn a_zero_column_batch_carries_only_its_row_count() {
        use arrow::datatypes::Schema;

        let resolved = ResolvedSchema {
            schema: Arc::new(Schema::new(Vec::<Field>::new())),
            ..ResolvedSchema::default()
        };
        let mut batcher = RowBatcher::new(
            &resolved,
            "public.t".into(),
            QueryOptions::default(),
            vec![None, None],
            Vec::new(),
        );
        let mut chunks = RetainedChunks::new();
        batcher
            .push_row(0, 0, RawRow::unchecked(b"a\tb"), &mut RowSplit::default(), &mut chunks)
            .unwrap();
        batcher
            .push_row(0, 4, RawRow::unchecked(b"c\td"), &mut RowSplit::default(), &mut chunks)
            .unwrap();
        let batch = batcher.flush().unwrap();
        assert_eq!(batch.num_columns(), 0);
        assert_eq!(batch.num_rows(), 2);
    }

    /// Every row of a one-column batch, rendered back to text.
    fn rendered(batch: &RecordBatch) -> Vec<Option<String>> {
        (0..batch.num_rows())
            .map(|row| {
                render_field(batch.column(0).as_ref(), row, &NestedPlan::Scalar)
                    .expect("renders back")
            })
            .collect()
    }

    /// The span trigger measures from the first selected row's offset to one
    /// past the latest selected row's last byte and fires the moment that
    /// reaches the cap, so it is overshot by at most one row. The rows here
    /// are four bytes apart, with `max_rows` unbounded and `max_bytes` off.
    #[test]
    fn the_source_span_trigger_fires_on_the_distance_between_selected_rows() {
        let options = QueryOptions {
            max_rows: usize::MAX,
            max_bytes: None,
            max_source_span: Some(100),
            ..Default::default()
        };
        let mut batcher = one_column_batcher(options);
        let mut chunks = RetainedChunks::new();
        assert!(!batcher.should_flush(), "an empty batch never flushes");

        // Offsets 0, 4, 8, … each three bytes of row: the span after the row
        // at offset `n` is `n + 3`, so the first `>= 100` is at offset 100.
        for offset in (0..100).step_by(4) {
            batcher
                .push_row(
                    0,
                    offset,
                    RawRow::unchecked(b"abc"),
                    &mut RowSplit::default(),
                    &mut chunks,
                )
                .unwrap();
            assert!(!batcher.should_flush(), "span {} is under the cap", offset + 3);
        }
        batcher
            .push_row(0, 100, RawRow::unchecked(b"abc"), &mut RowSplit::default(), &mut chunks)
            .unwrap();
        assert!(batcher.should_flush(), "span 103 has reached the cap");

        // Flushing reopens the span at the next row rather than at the
        // flushed batch's start: the row at 104 opens a span of three bytes,
        // not one from 100.
        assert_eq!(batcher.flush().unwrap().num_rows(), 26);
        assert!(!batcher.should_flush());
        batcher
            .push_row(0, 104, RawRow::unchecked(b"abc"), &mut RowSplit::default(), &mut chunks)
            .unwrap();
        assert!(!batcher.should_flush(), "the span restarts at the first row after a flush");
    }

    /// A stretch of rows the predicate rejects moves the scanner but not the
    /// span, a rejected row never reaching `push_row`.
    #[test]
    fn rows_that_were_never_pushed_do_not_widen_the_span() {
        let options = QueryOptions {
            max_rows: usize::MAX,
            max_bytes: None,
            max_source_span: Some(100),
            ..Default::default()
        };
        let mut batcher = one_column_batcher(options);
        let mut chunks = RetainedChunks::new();
        batcher
            .push_row(
                0,
                1_000_000,
                RawRow::unchecked(b"abc"),
                &mut RowSplit::default(),
                &mut chunks,
            )
            .unwrap();
        assert!(!batcher.should_flush(), "a span opens at the first selected row, not at zero");
        batcher
            .push_row(
                0,
                1_000_040,
                RawRow::unchecked(b"abc"),
                &mut RowSplit::default(),
                &mut chunks,
            )
            .unwrap();
        assert!(!batcher.should_flush(), "43 bytes of span, whatever lay between");
    }

    /// `None` opts out of the trigger entirely: no span, however wide,
    /// flushes on its own.
    #[test]
    fn a_none_source_span_leaves_a_batch_unbounded() {
        let options = QueryOptions {
            max_rows: usize::MAX,
            max_bytes: None,
            max_source_span: None,
            ..Default::default()
        };
        let mut batcher = one_column_batcher(options);
        let mut chunks = RetainedChunks::new();
        batcher
            .push_row(0, 0, RawRow::unchecked(b"abc"), &mut RowSplit::default(), &mut chunks)
            .unwrap();
        batcher
            .push_row(0, 1 << 40, RawRow::unchecked(b"abc"), &mut RowSplit::default(), &mut chunks)
            .unwrap();
        assert!(!batcher.should_flush());
    }
}

/// The `ArrayLiteral`-building render, kept as the oracle
/// [`render_array_into`]'s direct walk is checked against, over a generated
/// corpus rather than a listed one.
#[cfg(test)]
mod prior_shape {
    use super::*;

    /// Walk one `List` level, appending its lengths to `dims` and its leaves
    /// to `elements` in row-major order — the flattened shape
    /// [`nested::ArrayLiteral`] wants.
    fn collect_array(
        column: &dyn Array,
        row: usize,
        child_plan: &NestedPlan,
        depth: usize,
        dims: &mut Vec<usize>,
        elements: &mut Vec<Option<std::borrow::Cow<'static, str>>>,
    ) -> Result<()> {
        let values = column.as_any().downcast_ref::<ListArray>().unwrap().value(row);
        if dims.len() == depth {
            dims.push(values.len());
        }
        for i in 0..values.len() {
            match child_plan {
                NestedPlan::Array(inner) => {
                    collect_array(values.as_ref(), i, inner, depth + 1, dims, elements)?;
                }
                _ => elements.push(
                    render_field(values.as_ref(), i, child_plan)?.map(std::borrow::Cow::Owned),
                ),
            }
        }
        Ok(())
    }

    /// Rebuild an `array_out` literal from a `List` value.
    pub(super) fn render_array(
        column: &dyn Array,
        row: usize,
        child_plan: &NestedPlan,
    ) -> Result<String> {
        let mut dims = Vec::new();
        let mut elements = Vec::new();
        collect_array(column, row, child_plan, 0, &mut dims, &mut elements)?;
        if elements.is_empty() {
            return Ok("{}".to_string());
        }
        let lower_bounds = vec![1; dims.len()];
        Ok(nested::render_array(&nested::ArrayLiteral { elements, dims, lower_bounds }))
    }
}

/// [`render_array_into`] against the literal-building oracle, over generated
/// element text and generated shapes. The corpus is built as Arrow values
/// directly rather than through [`append_typed`]: the quoting rule is what is
/// under test, and `decode_array` only hands back text `array_out` would have
/// written, so a corpus routed through it could not reach an element holding
/// a brace, a bare `NULL` or a lone backslash.
#[cfg(test)]
mod differential {
    use arrow::datatypes::Field;

    use super::*;

    /// The same deterministic 64-bit LCG `decode.rs`'s differential tests use,
    /// so a failure is reproducible from the seed alone and the corpus does
    /// not have to be committed.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 11
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    /// Every byte class the array grammar branches on: the force-quote set,
    /// each whitespace character `array_isspace` knows, and ordinary text.
    const ALPHABET: &[&str] = &[
        "a", "Z", "0", ",", "{", "}", "\"", "\\", " ", "\t", "\n", "\r", "\x0b", "\x0c", "é", "N",
        "U", "L",
    ];

    fn element(rng: &mut Rng) -> Option<String> {
        match rng.below(12) {
            0 => return None,
            1 => return Some(String::new()),
            2 => return Some("NULL".to_string()),
            3 => return Some("null".to_string()),
            _ => {}
        }
        let len = rng.below(6);
        Some((0..len).map(|_| ALPHABET[rng.below(ALPHABET.len())]).collect())
    }

    fn utf8_list(rows: &[Vec<Option<String>>]) -> ArrayRef {
        let mut values = StringViewBuilder::new();
        for row in rows {
            for v in row {
                match v {
                    Some(s) => values.append_value(s),
                    None => values.append_null(),
                }
            }
        }
        Arc::new(ListArray::new(
            Arc::new(Field::new("item", DataType::Utf8View, true)),
            OffsetBuffer::from_lengths(rows.iter().map(Vec::len)),
            Arc::new(values.finish()) as ArrayRef,
            None,
        ))
    }

    /// Wrap a `List` in another `List`, `width` inner lists to each outer one
    /// — the rectangular shape `append_typed` is the only thing that produces.
    fn nest(inner: ArrayRef, width: usize) -> ArrayRef {
        let outer = inner.len() / width;
        let field = Arc::new(Field::new("item", inner.data_type().clone(), true));
        Arc::new(ListArray::new(
            field,
            OffsetBuffer::from_lengths((0..outer).map(|_| width)),
            inner,
            None,
        ))
    }

    #[track_caller]
    fn agrees(column: &ArrayRef, plan: &NestedPlan, child: &NestedPlan) {
        for row in 0..column.len() {
            let expected = prior_shape::render_array(column.as_ref(), row, child)
                .expect("the oracle renders every corpus row");
            let actual = render_field(column.as_ref(), row, plan)
                .expect("the walk renders every corpus row")
                .expect("no corpus row is a SQL NULL array");
            assert_eq!(actual, expected, "row {row}");
        }
    }

    /// One dimension, generated element text: the shape every registered
    /// figure's array columns are, and where the quoting decision lives.
    #[test]
    fn a_one_dimensional_array_renders_as_the_literal_builder_did() {
        let mut rng = Rng(0x5eed_0717);
        let rows: Vec<Vec<Option<String>>> =
            (0..400).map(|_| (0..rng.below(7)).map(|_| element(&mut rng)).collect()).collect();
        assert!(rows.iter().any(Vec::is_empty), "the corpus must reach the empty array");
        let column = utf8_list(&rows);
        let child = NestedPlan::Scalar;
        agrees(&column, &NestedPlan::Array(Box::new(child.clone())), &child);
    }

    /// Two dimensions, including the all-empty shape `array_out` writes as
    /// `{}` whatever its dimensionality (I20) — the one case where the walk
    /// has to unwind the braces it has already written.
    #[test]
    fn a_two_dimensional_array_renders_as_the_literal_builder_did() {
        let mut rng = Rng(0x5eed_0718);
        for width in [1usize, 2, 3] {
            for inner_len in [0usize, 1, 4] {
                let rows: Vec<Vec<Option<String>>> = (0..(6 * width))
                    .map(|_| (0..inner_len).map(|_| element(&mut rng)).collect())
                    .collect();
                let column = nest(utf8_list(&rows), width);
                let child = NestedPlan::Array(Box::new(NestedPlan::Scalar));
                agrees(&column, &NestedPlan::Array(Box::new(child.clone())), &child);
            }
        }
    }

    /// Three dimensions, so the depth-keyed rectangularity check is exercised
    /// below the level a two-dimensional value reaches.
    #[test]
    fn a_three_dimensional_array_renders_as_the_literal_builder_did() {
        let mut rng = Rng(0x5eed_0719);
        let rows: Vec<Vec<Option<String>>> =
            (0..24).map(|_| (0..2).map(|_| element(&mut rng)).collect()).collect();
        let column = nest(nest(utf8_list(&rows), 3), 2);
        let child = NestedPlan::Array(Box::new(NestedPlan::Array(Box::new(NestedPlan::Scalar))));
        agrees(&column, &NestedPlan::Array(Box::new(child.clone())), &child);
    }

    /// A nested element that is not a scalar: the composite arm builds its
    /// own literal and goes through the same quoting decision.
    #[test]
    fn an_array_of_composites_renders_as_the_literal_builder_did() {
        let mut rng = Rng(0x5eed_071a);
        let mut a = Int32Builder::new();
        let mut b = StringViewBuilder::new();
        let mut lengths = Vec::new();
        for _ in 0..60 {
            let len = rng.below(4);
            lengths.push(len);
            for _ in 0..len {
                match rng.below(5) {
                    0 => a.append_null(),
                    _ => a.append_value(rng.below(1000) as i32 - 500),
                }
                match element(&mut rng) {
                    Some(s) => b.append_value(&s),
                    None => b.append_null(),
                }
            }
        }
        let fields = Fields::from(vec![
            Field::new("a", DataType::Int32, true),
            Field::new("b", DataType::Utf8View, true),
        ]);
        let values = Arc::new(StructArray::new(
            fields.clone(),
            vec![Arc::new(a.finish()) as ArrayRef, Arc::new(b.finish()) as ArrayRef],
            None,
        )) as ArrayRef;
        let column = Arc::new(ListArray::new(
            Arc::new(Field::new("item", DataType::Struct(fields), true)),
            OffsetBuffer::from_lengths(lengths),
            values,
            None,
        )) as ArrayRef;
        let child = NestedPlan::Record(vec![NestedPlan::Scalar, NestedPlan::Scalar]);
        agrees(&column, &NestedPlan::Array(Box::new(child.clone())), &child);
    }
}
