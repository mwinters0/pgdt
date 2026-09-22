//! Reads `pg_dump` plain-format output into Arrow batches. The modules are
//! four layers whose dependencies point one way — bytes and structure, then
//! PostgreSQL semantics, then Arrow assembly, then query — with this file,
//! `error` and `instrument` in none (`docs/design/decisions.md`, "D68"; `tests/layering.rs`).

pub mod batch;
pub mod cache;
pub mod copy;
pub mod decode;
pub mod diagnostic;
mod error;
mod gather;
pub mod index;
pub mod instrument;
mod io;
mod leader;
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

pub use batch::{QueryOptions, ScanExtent, read_table, render_field, render_field_into};
pub use copy::CopyHeader;
pub use diagnostic::{Diagnostic, DiagnosticKind, DiagnosticSink, Finding, Severity};
pub use error::Error;
pub use index::{
    ArrayShape, CopyBlock, DumpIndex, PG_ARRAY_MAX_DIMS, TableName, build_index, preamble_only,
    union_census,
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
    TilingIssue, TocHeader, attach_text, build_map, check_tiling,
};
pub use pgtype::{
    CanonicalExtension, CompareKind, ComparisonDivergence, ComparisonPlan, ComparisonSemantics,
    NestedCompare, NestedPlan, TypeOutcome, UnanswerableReason, comparison_for, extension_for,
    resolve_declared_type,
};
pub use preamble::{
    CollationDef, ColumnDef, DatabaseMetadata, DumpMetadata, Extension, TypeDef, TypeKind,
    dump_metadata_from_spans,
};
pub use predicate::{ComparisonNote, Expr, Predicate, PredicateOp, Truth, column_divergences};
pub use resolve::{ColumnNote, ColumnResolution, ResolvedSchema, SchemaMode, resolve_columns};
pub use scan::{
    Cancellation, ChunkCarry, ChunkPass, CopyEnd, CopyScanner, CopyStart, Event, LargeObjectEnd,
    LargeObjectStart, Line, Row, SCAN_CHUNK_DEFAULT_SIZE_BYTES, SCAN_LINE_DEFAULT_MAX_BYTES,
    ScanOptions, scan,
};
pub use statistics::{
    BLOCK_MAX_STATISTICS_GROUPS, BlockGathered, BlockStatistics, Bounds, ColumnBounds,
    ColumnDictionary, ColumnStatistics, DICTIONARY_ENTRY_MAX_BYTES, DICTIONARY_MAX_ENTRIES,
    GroupSizing, RowGroup, STATISTICS_GROUP_DEFAULT_MIN_ROWS, STATISTICS_GROUP_DEFAULT_SIZE_BYTES,
    Sortedness, StatisticsBackfill, StatisticsHeld, StatisticsRequest, StatisticsSelection,
    StatisticsTarget, StatisticsTerms,
};
pub use stream::{
    BlockingTableIter, EarlyStop, MapRun, PlanNote, PlanNoteKind, ResumeToken, TablePartitions,
    TableStream, bounded_columns, gather_block_statistics, map_file, table_schema, table_stream,
    table_stream_partitions,
};

pub type Result<T> = std::result::Result<T, Error>;
