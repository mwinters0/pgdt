//! Reads `pg_dump` plain-format output into Arrow batches. The modules are
//! four layers whose dependencies point one way — bytes and structure, then
//! PostgreSQL semantics, then Arrow assembly, then query — with this file,
//! `error` and `instrument` in none (`docs/design/decisions.md`, "D68"; `tests/layering.rs`).

pub mod batch;
pub mod cache;
pub mod copy;
mod datetime_in;
pub mod decode;
pub mod diagnostic;
mod error;
mod gather;
pub mod index;
pub mod instrument;
mod io;
mod leader;
mod lex;
pub mod map;
pub mod nested;
pub mod pgtype;
pub mod preamble;
pub mod predicate;
mod prune;
pub mod resolve;
pub mod scan;
pub mod statistics;
pub mod stream;
pub mod summary;
mod unrepresentable;

pub use batch::{
    QueryOptions, ScanExtent, decode_field, read_table, render_field, render_field_into,
};
pub use copy::CopyHeader;
pub use diagnostic::{Diagnostic, DiagnosticKind, DiagnosticSink, Finding, Severity};
pub use error::Error;
pub use gather::{StrictUnchecked, UncheckedCheck, UncheckedColumn, strict_unchecked};
pub use index::{
    ArrayShape, BlockCensus, CopyBlock, DumpIndex, PG_ARRAY_MAX_DIMS, TableName, Unrepresentable,
    UnrepresentableTier, calendar_end, preamble_only, union_census,
};
pub use io::{
    ByteRangeSource, DEFAULT_MEMORY_BUDGET, FetchedXzSource, KnownCompression, LocalFileSource,
    MEMORY_MARGIN_PERCENT, MEMORY_RESERVE, MEMORY_UNPOOLED_BOUND, MemoryLimit, Origin, OriginProbe,
    Parallelism, PartitionBoundaries, PartitionRead, Partitioning, Recognized, RemoteIdentity,
    RetainedUnit, WaitPolicy, WorkerMemory, XzSource, available_memory, available_memory_in,
    discover_memory_limit, discover_memory_limit_in, open, open_local, statistics_allowance,
    walk_seek_table,
};
#[cfg(feature = "http")]
pub use io::{REMOTE_READ_TIMEOUT, RemoteSource, open_remote};
pub use map::{
    DataBlock, InsertRun, LargeObjectRegion, SPAN_STORED_TEXT_MAX_BYTES, Span, SpanBody, SpanText,
    TilingIssue, TocHeader, attach_text, check_tiling,
};
pub use pgtype::{
    CanonicalExtension, CompareKind, ComparisonDivergence, ComparisonPlan, ComparisonSemantics,
    NestedCompare, NestedPlan, NumericTypmod, TypeOutcome, UnanswerableReason, Unchecked,
    bounds_set_keyed_by, comparison_for, extension_for, resolve_declared_type,
};
pub use preamble::{
    CheckConstraint, CollationDef, ColumnDef, DatabaseMetadata, DumpMetadata, Extension, TableDef,
    TableReference, TypeDef, TypeKind, dump_metadata_from_spans,
};
pub use predicate::{
    ComparisonNote, Expr, Membership, Predicate, PredicateOp, Truth, column_divergences,
};
pub use resolve::{ColumnNote, ColumnResolution, ResolvedSchema, SchemaMode, resolve_columns};
pub use scan::PostgresInvalidValues;
pub use scan::{
    Cancellation, ChunkCarry, ChunkPass, CopyEnd, CopyScanner, CopyStart, Event, LargeObjectEnd,
    LargeObjectStart, Line, Row, SCAN_CHUNK_DEFAULT_SIZE_BYTES, SCAN_LINE_DEFAULT_MAX_BYTES,
    ScanOptions, scan,
};
pub use statistics::{
    BLOCK_MAX_ROW_GROUPS, BlockGathered, BlockStatistics, Bounds, BoundsSet, BoundsView,
    ColumnBounds, ColumnDictionary, ColumnRefusals, ColumnStatistics, DICTIONARY_ENTRY_MAX_BYTES,
    DICTIONARY_MAX_ENTRIES, FieldRefusal, GroupSizing, IgnoredRefusals, ROW_GROUP_DEFAULT_MIN_ROWS,
    ROW_GROUP_DEFAULT_SIZE_BYTES, RowGroup, Sortedness, StatisticsBackfill, StatisticsHeld,
    StatisticsLevel, StatisticsRequest, StatisticsSelection, StatisticsTarget, StatisticsTerms,
    StatisticsView,
};
pub use stream::{
    BlockReread, BlockingTableIter, DynamicFilter, DynamicPartitions, EarlyStop, MapRun, PlanLever,
    PlanNote, PlanNoteKind, ResumeToken, RowEvaluation, TablePartitions, TableStream,
    bounded_columns, build_index, build_map, gather_block_statistics, map_file, table_schema,
    table_stream, table_stream_partitions,
};
pub use summary::{Bound, ColumnSummary, TableSummary, table_summary};
pub use unrepresentable::UnrepresentableMode;

pub type Result<T> = std::result::Result<T, Error>;
