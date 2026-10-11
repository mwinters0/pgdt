//! The parsers and the cache: the SQL lexer, the scanner, the preamble grammar, the map
//! builder, the interior split, and persisting and reloading the model
//! (`docs/design/decisions.md`, "D68"). No Arrow (D74).

pub mod cache;
mod leader;
mod lex;
mod load;
mod map;
mod preamble;
mod scan;

pub use load::preamble_only;
pub use map::{attach_text, check_tiling};
pub use preamble::dump_metadata_from_spans;
pub use scan::{
    ChunkCarry, ChunkPass, CopyEnd, CopyScanner, CopyStart, DollarQuoteEnd, Event, LargeObjectEnd,
    LargeObjectStart, Line, Row, SCAN_LINE_DEFAULT_MAX_BYTES, ScanOptions, scan,
};

/// What a crate above reaches that is not this crate's API: public only
/// because a crate boundary forced it, and named here so a caller reaching
/// past the API names the word.
pub mod internal {
    #[cfg(any(test, feature = "test-support"))]
    pub use crate::cache::file::{
        CACHE_MAGIC, CacheFile, CacheFileRef, CompressionIndex, ContainerKind, read_cache_file,
        write_beside,
    };
    #[cfg(any(test, feature = "test-support"))]
    pub use crate::cache::identity::SourceIdentity;
    pub use crate::leader::{
        BoundBy, CensusPlan, Interior, RegionOutcome, RegionScan, Shortfall, scan_region,
    };
    pub use crate::load::{
        non_seekable_compression_diagnostic, scan_preamble, tiling_diagnostics,
        toc_coverage_diagnostic,
    };
    pub use crate::map::Builder;
    pub use crate::scan::announce_cancellation;
}
