pub mod batch;
pub mod cache;
pub mod copy;
pub mod decode;
pub mod diagnostic;
mod error;
pub mod index;
mod io;
mod leader;
pub mod map;
pub mod nested;
pub mod pgtype;
pub mod preamble;
pub mod predicate;
pub mod resolve;
pub mod scan;
pub mod stream;

pub use batch::{QueryOptions, ScanExtent, read_table, render_field, render_field_into};
pub use copy::CopyHeader;
pub use diagnostic::{Diagnostic, DiagnosticKind, Severity};
pub use error::Error;
pub use index::{
    ArrayShape, CopyBlock, DumpIndex, MAX_ARRAY_DIMS, RowGroupStats, SparseRowIndex, build_index,
    preamble_only, union_census,
};
pub use io::{
    ByteRangeSource, DEFAULT_MEMORY_BUDGET, KnownCompression, LocalFileSource, MEMORY_RESERVE,
    MemoryLimit, Parallelism, PartitionBoundaries, Partitioning, Recognized, RetainedUnit,
    WaitPolicy, XzSource, available_memory, available_memory_in, discover_memory_limit,
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
    ChunkCarry, ChunkPass, CopyEnd, CopyScanner, CopyStart, DEFAULT_CHUNK_SIZE, Event,
    LargeObjectEnd, LargeObjectStart, Line, Row, ScanOptions, scan,
};
pub use stream::{
    BlockingTableIter, MapRun, PlanNote, PlanNoteKind, ResumeToken, TableStream, map_file,
    table_stream, table_stream_partitions,
};

pub type Result<T> = std::result::Result<T, Error>;
