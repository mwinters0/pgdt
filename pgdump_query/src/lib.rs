//! Reads `pg_dump` plain-format output into Arrow batches. Each module
//! belongs to one of six crates-to-be whose dependencies point one way —
//! core, model, io, then the dump's parsers and the codec side by side, then
//! query (`docs/design/decisions.md`, "D68"; `tests/layering.rs`).

pub mod batch;
pub mod cache;
pub mod copy;
mod datetime_in;
pub mod decode;
mod error;
mod gather;
pub mod instrument;
mod io;
mod key;
mod leader;
mod lex;
pub mod load;
pub mod map;
pub mod model;
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
pub use error::{Error, RowEndingRefusal};
pub use gather::{
    StrictUnchecked, UncheckedBound, UncheckedCheck, UncheckedColumn, strict_unchecked,
};
pub use io::{
    ByteRangeSource, Cancellation, DEFAULT_MEMORY_BUDGET, FetchedXzSource, KnownCompression,
    LocalFileSource, MEMORY_MARGIN_PERCENT, MEMORY_RESERVE, MEMORY_UNPOOLED_BOUND, MemoryLimit,
    Origin, OriginProbe, Parallelism, PartitionBoundaries, PartitionRead, Partitioning, Recognized,
    RemoteIdentity, RetainedUnit, SCAN_CHUNK_DEFAULT_SIZE_BYTES, WaitPolicy, WorkerMemory,
    XzSource, available_memory, available_memory_in, discover_memory_limit,
    discover_memory_limit_in, open, open_local, statistics_allowance, walk_seek_table,
};
#[cfg(feature = "http")]
pub use io::{REMOTE_READ_TIMEOUT, RemoteSource, open_remote};
pub use load::preamble_only;
pub use map::{attach_text, check_tiling};
pub use model::{
    ArrayShape, BlockCensus, BlockGathered, BlockStatistics, Bounds, BoundsSet, BoundsView,
    CheckConstraint, CollationDef, ColumnBounds, ColumnDef, ColumnDictionary, ColumnRefusals,
    ColumnStatistics, CopyBlock, DICTIONARY_ENTRY_MAX_BYTES, DICTIONARY_MAX_ENTRIES, DataBlock,
    DatabaseMetadata, Diagnostic, DiagnosticKind, DiagnosticSink, DumpIndex, DumpMetadata,
    Extension, FieldRefusal, Finding, GroupSizing, IgnoredRefusals, InsertRun, LargeObjectRegion,
    PG_ARRAY_MAX_DIMS, PartitionOf, PostgresInvalidValues, RowGroup, SPAN_STORED_TEXT_MAX_BYTES,
    Severity, Sortedness, Span, SpanBody, SpanText, StatisticsView, TableDef, TableName,
    TableReference, TilingIssue, TocHeader, TypeDef, TypeKind, Unrepresentable,
    UnrepresentableTier, calendar_end, union_census,
};
pub use pgtype::{
    CanonicalExtension, CompareKind, ComparisonDivergence, ComparisonPlan, ComparisonSemantics,
    IntervalQualifier, NestedCompare, NestedPlan, NumericTypmod, Position, TypeOutcome,
    UnanswerableReason, Unchecked, bounds_set_keyed_by, comparison_for, extension_for,
    resolve_declared_type,
};
pub use preamble::dump_metadata_from_spans;
pub use predicate::{
    ComparisonNote, Expr, Membership, Predicate, PredicateOp, Truth, column_divergences,
};
pub use resolve::{ColumnNote, ColumnResolution, ResolvedSchema, SchemaMode, resolve_columns};
pub use scan::{
    ChunkCarry, ChunkPass, CopyEnd, CopyScanner, CopyStart, Event, LargeObjectEnd,
    LargeObjectStart, Line, Row, SCAN_LINE_DEFAULT_MAX_BYTES, ScanOptions, scan,
};
pub use statistics::{
    BLOCK_MAX_ROW_GROUPS, ROW_GROUP_DEFAULT_MIN_ROWS, ROW_GROUP_DEFAULT_SIZE_BYTES,
    StatisticsBackfill, StatisticsHeld, StatisticsLevel, StatisticsRequest, StatisticsSelection,
    StatisticsTarget, StatisticsTerms,
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
