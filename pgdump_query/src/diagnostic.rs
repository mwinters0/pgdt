//! The file-level diagnostic channel
//! (`docs/design/decisions.md`, "The file map and the preamble").
//!
//! Two things have no good home in a `Result`: a tiling failure, which is
//! evidence of a bug in our parser but never a reason to refuse the file, and
//! a cache mtime mismatch, too weak to invalidate on and with no column or
//! schema to hang off. The library cannot `eprintln!`, so both are collected
//! on [`crate::index::DumpIndex`] and left for a caller to drain.
//!
//! **Not persisted.** `DumpIndex::diagnostics` is `#[serde(skip)]`
//! (`docs/design/decisions.md`, "D34") and recomputed on load.
//!
//! [`Severity`] and the `{severity, kind}` shape are the vocabulary shared
//! across producers, not one enum: `DumpIndex` is L1 and
//! `crate::resolve::ColumnResolution` is an L2 conclusion, so a
//! [`DiagnosticKind`] variant carrying one would make L1 name an L2 type
//! (`docs/design/decisions.md`, "D68"). `crate::resolve::ColumnNote` is the
//! per-column record at L2 — one per column, always present — reporting on
//! this same scale through `ColumnNote::severity`, so a caller reading both
//! filters uniformly. Unifying at the *drain* point stays open
//! (`docs/design/roadmap.md`).

use serde::Serialize;

use crate::map::TilingIssue;

/// How much a diagnostic matters. One scale for every producer, so a caller
/// draining several channels can filter uniformly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Severity {
    /// Worth reporting, nothing is wrong (a fully-mapped column, a
    /// TOC-coverage figure).
    Info,
    /// Something a user should see and may need to act on, but the result is
    /// still usable as-is.
    Warning,
    /// Evidence of a bug or of data we could not account for. The result is
    /// still returned — see the module docs.
    Error,
}

/// What a [`Diagnostic`] is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum DiagnosticKind {
    /// The map does not tile its file: a gap, an overlap, an empty span, or
    /// a span list that doesn't reach the scanned end. Always a bug in
    /// `crate::map`, never a property of the input
    /// (`docs/design/decisions.md`, "D30").
    TilingBroken { issues: Vec<TilingIssue> },
    /// A loaded cache recorded a different mtime than the source now has.
    /// Not an invalidation (`docs/design/decisions.md`, "D21"). A *size*
    /// mismatch is one instead, and never reaches this channel because the
    /// cache is discarded outright.
    CacheMtimeChanged,
    /// How much of the map is attributed to a TOC entry: `attributed` spans
    /// out of `spans` total — a follow-on statement that inherited its
    /// governing entry's header (`crate::map::Span::toc_owned` is `false`)
    /// counts the same as one whose own comment carried it
    /// (`docs/design/decisions.md`, "D31"). Always `Info` — zero is a normal, reported state (the map
    /// running in header-less degraded mode), not an error.
    TocCoverage { attributed: usize, spans: usize },
    /// The index was loaded from a retained `.dqcache` with no live dump file
    /// to check it against (`docs/design/decisions.md`,
    /// "The compressed source and the cache") — unverified, there being no
    /// recorded size/mtime to compare. Pushed unconditionally by
    /// [`crate::cache::CacheMode::load_offline`] on every successful
    /// cache-only load, `Incomplete` included.
    CacheOffline,
    /// A `.xz` source has no usable seek structure — one stream, one block —
    /// so every read (forward included) decodes from byte zero. Never a
    /// reason to refuse the file (`docs/design/decisions.md`, "D19").
    /// `block_count` is `SeekTable::block_count()` — 0 or 1 for a
    /// non-seekable table — pushed once per index, whether the table was just
    /// walked (`crate::index::build_index`, `crate::index::preamble_only`) or
    /// read back from a persisted cache (`crate::cache::status_from_file`).
    NonSeekableCompressedSource { block_count: usize },
}

/// One thing worth telling the caller about a file, with no `Result` to carry
/// it — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub kind: DiagnosticKind,
}

impl Diagnostic {
    pub(crate) fn tiling_broken(issues: Vec<TilingIssue>) -> Self {
        Self { severity: Severity::Error, kind: DiagnosticKind::TilingBroken { issues } }
    }

    /// Public, unlike its siblings: a caller matching on
    /// [`crate::cache::CacheStatus`] itself rather than going through
    /// [`crate::cache::CacheMode::load`] still has to turn the
    /// `mtime_changed` bit into this warning. `pgdq info` is that caller.
    pub fn cache_mtime_changed() -> Self {
        Self { severity: Severity::Warning, kind: DiagnosticKind::CacheMtimeChanged }
    }

    pub(crate) fn toc_coverage(attributed: usize, spans: usize) -> Self {
        Self { severity: Severity::Info, kind: DiagnosticKind::TocCoverage { attributed, spans } }
    }

    pub(crate) fn cache_offline() -> Self {
        Self { severity: Severity::Warning, kind: DiagnosticKind::CacheOffline }
    }

    /// D19's warning: usable as-is, so `Warning` rather than `Error`.
    pub(crate) fn non_seekable_compressed_source(block_count: usize) -> Self {
        Self {
            severity: Severity::Warning,
            kind: DiagnosticKind::NonSeekableCompressedSource { block_count },
        }
    }
}
