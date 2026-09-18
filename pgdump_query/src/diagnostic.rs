//! The file-level diagnostic channel
//! (`docs/design/decisions.md`, "The file map and the preamble").
//!
//! Two things have no good home in a `Result`: a tiling failure, which is
//! evidence of a bug in our parser but never a reason to refuse the file, and
//! a cache mtime mismatch, too weak by default to invalidate on and with no
//! column or schema to hang off. The library cannot `eprintln!`, so both are
//! collected
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
    /// A loaded cache recorded a different modification *time* than the source
    /// now has — an mtime locally, a `Last-Modified` remotely. Not an
    /// invalidation by default (`docs/design/decisions.md`, "D21"); a caller
    /// that asked for `crate::cache::StrictIdentity::time` is refused instead
    /// of being handed this. A *size* mismatch is an invalidation either way,
    /// and never reaches this channel because the cache is discarded outright.
    CacheMtimeChanged,
    /// The stronger half of the same signal: both sides carry a server's
    /// **entity tag** and the two differ, which is the server's own statement
    /// that this is a different version of the object. Its own kind rather
    /// than [`DiagnosticKind::CacheMtimeChanged`]'s wording widened, because a
    /// local dump has no such tag and a message about one would be noise
    /// there. Bound by the same selector
    /// (`docs/design/roadmap-P14-remote-input.md`, "D5").
    CacheEntityTagChanged,
    /// A loaded cache was written for a different **origin** than this run
    /// reads — where the object was fetched from, which a local source has
    /// none of. Advisory by the same rule the signal above is
    /// (`docs/design/roadmap-P14-remote-input.md`, "D4"); a caller that asked
    /// for `crate::cache::StrictIdentity::location` is refused instead.
    ///
    /// It is what makes the working-directory default cache path's one
    /// collision visible: two same-named dumps of equal stored size from
    /// different hosts read in one directory.
    CacheOriginChanged,
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
    /// [`crate::cache::CacheMode::load`] still has to turn a
    /// [`crate::cache::WeakIdentity::Differs`] into this warning. `pgdq info`
    /// is that caller: it reports what the cache holds, unless the selection
    /// it asked [`crate::cache::CacheMode::strict_identity_refusal`] about
    /// binds the signal.
    pub fn cache_mtime_changed() -> Self {
        Self { severity: Severity::Warning, kind: DiagnosticKind::CacheMtimeChanged }
    }

    /// Public for [`Diagnostic::cache_mtime_changed`]'s reason, and pushed by
    /// the same callers: both sides' entity tags differ
    /// (`crate::cache::WeakIdentity::TagDiffers`).
    pub fn cache_entity_tag_changed() -> Self {
        Self { severity: Severity::Warning, kind: DiagnosticKind::CacheEntityTagChanged }
    }

    /// Public for [`Diagnostic::cache_mtime_changed`]'s reason, and pushed by
    /// the same callers: a cache written for another origin than this run
    /// reads (`crate::cache::OriginMatch`).
    pub fn cache_origin_changed() -> Self {
        Self { severity: Severity::Warning, kind: DiagnosticKind::CacheOriginChanged }
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
