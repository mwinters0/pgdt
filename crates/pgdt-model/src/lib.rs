//! The dump's vocabulary: what a scan finds and the cache persists — the
//! spans tiling a file and the `COPY` blocks among them, the schema the
//! preamble declares, what a pass over a block's rows records, its
//! statistics, and the findings about the file a caller drains. Data, and the
//! contracts over a block's rows a crate above implements; no I/O, no Arrow
//! (`docs/design/decisions.md`, "D68", "D74").
//!
//! **This is the cache's format**: an edit here is the edit that asks whether
//! [`CACHE_FORMAT_VERSION`] bumps.

mod block;
mod catalog;
mod diagnostic;
mod span;
mod statistics;

pub use block::{
    ArrayShape, BlockCensus, PG_ARRAY_MAX_DIMS, Unrepresentable, UnrepresentableTier, calendar_end,
};
pub use catalog::{
    CheckConstraint, CollationDef, ColumnDef, DatabaseMetadata, DumpMetadata, Extension, NotNull,
    PartitionOf, TableDef, TableReference, TypeDef, TypeKind,
};
pub use diagnostic::{
    CompressedContainer, Diagnostic, DiagnosticKind, DiagnosticSink, Finding, Severity,
    TilingIssue, drain,
};
pub use span::{
    CopyBlock, DataBlock, DumpIndex, InsertRun, LargeObjectRegion, SPAN_STORED_TEXT_MAX_BYTES,
    Span, SpanBody, SpanText, TableName, TocHeader, union_census,
};
pub use statistics::{
    BlockGathered, BlockStatistics, Bounds, BoundsSet, ColumnDictionary, ColumnRefusals,
    ColumnStatistics, DICTIONARY_ENTRY_MAX_BYTES, DICTIONARY_MAX_ENTRIES, FieldRefusal,
    GroupSizing, IgnoredRefusals, PostgresInvalidValues, RowGroup, Sortedness, StatisticsView,
};

/// What a crate above reaches that is not this crate's API: public only
/// because a crate boundary forced it, and named here so a caller reaching
/// past the API names the word.
pub mod internal {
    pub use crate::block::{BlockCensusExt, FieldCount, UnrepresentableExt};
    pub use crate::catalog::{DatabaseMetadataExt, TableDefExt};
    pub use crate::diagnostic::DiagnosticExt;
    pub use crate::statistics::{
        BlockObserver, BoundsView, ColumnBounds, ColumnStatisticsExt, ColumnStatisticsParts,
        FieldRefusalExt, IgnoredRefusalsExt, text_heap, vec_heap,
    };
}

/// **Bump whenever a persisted field is added, removed or reshaped.** A cache
/// written under a different version is unusable
/// ([`pgdt_dump::cache::Unusable::UnsupportedVersion`]) rather than partially trusted
/// (`docs/design/roadmap.md`, "Four decisions that keep later phases
/// additive"); pre-1.0 a bump is free, nothing migrates (`CLAUDE.md`,
/// "Pre-1.0"), and nothing records it (`docs/design/decisions.md`, "D22").
/// A new way of *reading* an existing on-disk shape needs no bump:
/// [`pgdt_dump::cache::LoadedCache::is_complete`] reinterprets `scanned_through` against an extent
/// already stored. A new way of *choosing* what an unchanged field holds does:
/// a block recording the request that sized it is read back as already sized
/// under it, so a cache whose sizes today's rule would not choose is as
/// unusable as one of another shape. So does a parse fix that saves other
/// bytes for the same dump: `tests/cache.rs`'s
/// `persisted_index_is_pinned_to_the_format_version` pins what every fixture
/// persists beside this number, as `golden_order_is_pinned_to_the_format_version`
/// pins the comparison order, and each fails until it is bumped.
pub const CACHE_FORMAT_VERSION: u32 = 75;
