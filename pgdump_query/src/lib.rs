pub mod batch;
pub mod cache;
pub mod copy;
pub mod decode;
pub mod diagnostic;
mod error;
pub mod index;
mod io;
pub mod map;
pub mod pgtype;
pub mod preamble;
pub mod predicate;
pub mod resolve;
pub mod scan;
pub mod stream;

pub use batch::{BatchOptions, ScanExtent, read_table, render_field};
pub use copy::CopyHeader;
pub use diagnostic::{Diagnostic as FileDiagnostic, DiagnosticKind, Severity};
pub use error::Error;
pub use index::{CopyBlock, DumpIndex, RowGroupStats, SparseRowIndex, build_index, preamble_only};
pub use io::{ByteRangeSource, LocalFileSource};
pub use map::{
    Span, SpanBody, SpanText, TEXT_CAP, TilingIssue, attach_text, build_map, check_tiling,
};
pub use pgtype::{DeferredKind, TypeOutcome, resolve_declared_type};
pub use preamble::{
    DatabaseMetadata, DumpMetadata, Extension, TypeDef, TypeKind, dump_metadata_from_spans,
};
pub use predicate::{Predicate, PredicateOp};
pub use resolve::{ColumnResolution, Diagnostic, ResolvedSchema, SchemaMode, resolve_columns};
pub use scan::{CopyEnd, CopyScanner, CopyStart, Event, Line, Row, ScanOptions, scan};
pub use stream::{BlockingTableIter, ResumeToken, TableStream, table_stream};

pub type Result<T> = std::result::Result<T, Error>;
