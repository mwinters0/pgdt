//! Reads `pg_dump` plain-format output into Arrow batches. The modules are
//! four layers whose dependencies point one way — bytes and structure, then
//! PostgreSQL semantics, then Arrow assembly, then query — with this file and
//! `error` in none (`docs/design/decisions.md`, "D68"; `tests/layering.rs`).

pub mod batch;
pub mod cache;
pub mod copy;
pub mod decode;
pub mod diagnostic;
mod error;
mod gather;
pub mod index;
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
pub use diagnostic::{Diagnostic, DiagnosticKind, Severity};
pub use error::Error;
pub use index::{
    ArrayShape, CopyBlock, DumpIndex, MAX_ARRAY_DIMS, build_index, preamble_only, union_census,
};
pub use io::{
    ByteRangeSource, DEFAULT_MEMORY_BUDGET, KnownCompression, LocalFileSource,
    MEMORY_MARGIN_PERCENT, MEMORY_RESERVE, MEMORY_UNPOOLED_BOUND, MemoryLimit, Parallelism,
    PartitionBoundaries, PartitionRead, Partitioning, Recognized, RetainedUnit, WaitPolicy,
    WorkerMemory, XzSource, available_memory, available_memory_in, discover_memory_limit,
    discover_memory_limit_in, open_local,
};
pub use map::{
    DataBlock, InsertRun, LargeObjectRegion, Span, SpanBody, SpanText, TEXT_CAP, TilingIssue,
    TocHeader, attach_text, build_map, check_tiling,
};
pub use pgtype::{
    CanonicalExtension, CompareKind, ComparisonDivergence, ComparisonPlan, NestedCompare,
    NestedPlan, TypeOutcome, UnanswerableReason, comparison_for, extension_for,
    resolve_declared_type,
};
pub use preamble::{
    CollationDef, ColumnDef, DatabaseMetadata, DumpMetadata, Extension, TypeDef, TypeKind,
    dump_metadata_from_spans,
};
pub use predicate::{ComparisonNote, Expr, Predicate, PredicateOp, Truth};
pub use resolve::{ColumnNote, ColumnResolution, ResolvedSchema, SchemaMode, resolve_columns};
pub use scan::{
    ChunkCarry, ChunkPass, CopyEnd, CopyScanner, CopyStart, DEFAULT_CHUNK_SIZE,
    DEFAULT_MAX_LINE_BYTES, Event, LargeObjectEnd, LargeObjectStart, Line, Row, ScanOptions, scan,
};
pub use statistics::{
    BlockStatistics, Bounds, ColumnBounds, ColumnDictionary, ColumnStatistics,
    DEFAULT_STATISTICS_GROUP_SIZE, DICTIONARY_CAP, RowGroup, STORED_VALUE_CAP, Sortedness,
    StatisticsBackfill, StatisticsRequest, StatisticsSelection, StatisticsTarget,
};
pub use stream::{
    BlockingTableIter, EarlyStop, MapRun, PlanNote, PlanNoteKind, ResumeToken, TableStream,
    gather_block_statistics, map_file, table_stream, table_stream_partitions,
};

pub type Result<T> = std::result::Result<T, Error>;
