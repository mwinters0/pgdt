//! The parsers and the cache: the SQL lexer, the scanner, the DDL statement
//! grammar, the map builder, the preamble's prepass, and persisting and
//! reloading the model (`docs/design/decisions.md`, "D68"). No Arrow (D74).

pub mod cache;
mod ddl;
mod lex;
mod map;
mod preamble;
mod scan;

pub use ddl::dump_metadata_from_spans;
pub use map::{attach_text, check_tiling};
pub use preamble::preamble_only;
pub use scan::{
    ChunkEvents, ChunkedScanner, CopyEnd, CopyScanner, CopyStart, DollarQuoteEnd, Event,
    LargeObjectEnd, LargeObjectStart, Line, Row, SCAN_LINE_DEFAULT_MAX_BYTES, ScanOptions, scan,
};

/// What a crate above reaches that is not this crate's API: public only
/// because a crate boundary forced it, and named here so a caller reaching
/// past the API names the word.
pub mod internal {
    #[cfg(any(test, feature = "test-support"))]
    pub use crate::cache::file::{
        CACHE_MAGIC, CacheFile, CacheFileRef, CompressionIndex, ContainerKind, read_cache_file,
        write_cache_file,
    };
    #[cfg(any(test, feature = "test-support"))]
    pub use crate::cache::identity::SourceIdentity;
    pub use crate::map::{
        Builder, non_seekable_compression_diagnostic, tiling_diagnostics, toc_coverage_diagnostic,
    };
    pub use crate::preamble::scan_preamble;
    pub use crate::scan::{CopyScannerExt, PieceEndings, RowEnding, announce_read_loop};
}
