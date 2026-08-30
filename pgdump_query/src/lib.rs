pub mod batch;
pub mod cache;
pub mod copy;
pub mod decode;
pub mod diagnostic;
mod error;
pub mod index;
mod io;
pub mod map;
pub mod nested;
pub mod pgtype;
pub mod preamble;
pub mod predicate;
pub mod resolve;
pub mod scan;
pub mod stream;

pub use batch::{QueryOptions, ScanExtent, read_table, render_field};
pub use copy::CopyHeader;
pub use diagnostic::{Diagnostic, DiagnosticKind, Severity};
pub use error::Error;
pub use index::{
    ArrayShape, CopyBlock, DumpIndex, MAX_ARRAY_DIMS, RowGroupStats, SparseRowIndex, build_index,
    preamble_only, union_census,
};
pub use io::{ByteRangeSource, LocalFileSource};
pub use map::{
    DataBlock, InsertRun, LargeObjectRegion, Span, SpanBody, SpanText, TEXT_CAP, TilingIssue,
    TocHeader, attach_text, build_map, check_tiling,
};
pub use pgtype::{NestedPlan, TypeOutcome, resolve_declared_type};
pub use preamble::{
    DatabaseMetadata, DumpMetadata, Extension, TypeDef, TypeKind, dump_metadata_from_spans,
};
pub use predicate::{OrderingDivergence, OrderingNote, Predicate, PredicateOp};
pub use resolve::{ColumnNote, ColumnResolution, ResolvedSchema, SchemaMode, resolve_columns};
pub use scan::{
    CopyEnd, CopyScanner, CopyStart, Event, LargeObjectEnd, LargeObjectStart, Line, Row,
    ScanOptions, scan,
};
pub use stream::{BlockingTableIter, MapRun, ResumeToken, TableStream, map_file, table_stream};

pub type Result<T> = std::result::Result<T, Error>;
