//! The facade over the six `pgdt-*` crates while they are carved: every name
//! this crate's root exported before the carve that is still its crate's API,
//! and the namespaces an embedder names, re-exported by its API path, so its
//! dependents keep building until the facade is retired
//! (`docs/design/roadmap-P35-seams.md`, "Naming and layout").

pub use pgdt_codec::{decode, pgtype};
pub use pgdt_core::instrument;
pub use pgdt_dump::cache;
pub use pgdt_model as model;

pub use pgdt_codec::pgtype::{
    CanonicalExtension, CompareKind, ComparisonDivergence, ComparisonPlan, ComparisonSemantics,
    IntervalQualifier, NestedCompare, NestedPlan, NumericTypmod, Position, TypeOutcome,
    UnanswerableReason, Unchecked, bounds_set_keyed_by, comparison_for, extension_for,
    resolve_declared_type,
};
pub use pgdt_codec::{
    ColumnNote, ColumnResolution, ResolvedSchema, SchemaMode, UnrepresentableMode, resolve_columns,
};
pub use pgdt_core::{CopyHeader, Error, Result, RowEndingRefusal};
pub use pgdt_dump::{
    ChunkCarry, ChunkPass, CopyEnd, CopyScanner, CopyStart, Event, LargeObjectEnd,
    LargeObjectStart, Line, Row, SCAN_LINE_DEFAULT_MAX_BYTES, ScanOptions, attach_text,
    check_tiling, dump_metadata_from_spans, preamble_only, scan,
};
pub use pgdt_io::{
    ByteRangeSource, Cancellation, DEFAULT_MEMORY_BUDGET, FetchedXzSource, KnownCompression,
    LocalFileSource, MEMORY_MARGIN_PERCENT, MEMORY_RESERVE, MEMORY_UNPOOLED_BOUND, MemoryLimit,
    Origin, OriginProbe, Parallelism, PartitionBoundaries, PartitionRead, Partitioning, Recognized,
    RemoteIdentity, RetainedUnit, SCAN_CHUNK_DEFAULT_SIZE_BYTES, WaitPolicy, WorkerMemory,
    XzSource, available_memory, available_memory_in, discover_memory_limit,
    discover_memory_limit_in, open, open_local, statistics_allowance, walk_seek_table,
};
#[cfg(feature = "http")]
pub use pgdt_io::{REMOTE_READ_TIMEOUT, RemoteSource, open_remote};
pub use pgdt_model::{
    ArrayShape, BlockCensus, BlockStatistics, Bounds, BoundsSet, BoundsView, CheckConstraint,
    CollationDef, ColumnBounds, ColumnDef, ColumnDictionary, ColumnRefusals, ColumnStatistics,
    CopyBlock, DICTIONARY_ENTRY_MAX_BYTES, DICTIONARY_MAX_ENTRIES, DataBlock, DatabaseMetadata,
    Diagnostic, DiagnosticKind, DiagnosticSink, DumpIndex, DumpMetadata, Extension, FieldRefusal,
    Finding, GroupSizing, IgnoredRefusals, InsertRun, LargeObjectRegion, PG_ARRAY_MAX_DIMS,
    PartitionOf, PostgresInvalidValues, RowGroup, SPAN_STORED_TEXT_MAX_BYTES, Severity, Sortedness,
    Span, SpanBody, SpanText, StatisticsView, TableDef, TableName, TableReference, TilingIssue,
    TocHeader, TypeDef, TypeKind, Unrepresentable, UnrepresentableTier, calendar_end, union_census,
};
pub use pgdt_query::{
    BLOCK_MAX_ROW_GROUPS, BlockingTableIter, Bound, ColumnSummary, ComparisonNote, DynamicFilter,
    DynamicPartitions, EarlyStop, Expr, MapRun, Membership, PlanLever, PlanNote, PlanNoteKind,
    Predicate, PredicateOp, QueryOptions, ROW_GROUP_DEFAULT_MIN_ROWS, ROW_GROUP_DEFAULT_SIZE_BYTES,
    ResumeToken, RowEvaluation, ScanExtent, StatisticsHeld, StatisticsLevel, StatisticsRequest,
    StatisticsSelection, StatisticsTarget, StatisticsTerms, StrictUnchecked, TablePartitions,
    TableStream, TableSummary, Truth, UncheckedBound, UncheckedCheck, UncheckedColumn, build_index,
    build_map, column_divergences, decode_field, map_file, read_table, render_field,
    render_field_into, strict_unchecked, table_schema, table_stream, table_stream_partitions,
    table_summary,
};
