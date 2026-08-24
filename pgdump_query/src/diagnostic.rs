//! The file-level diagnostic channel
//! (`docs/design/roadmap-phase3-object-inventory.md`, "Diagnostics: a
//! file-level channel on `DumpIndex`").
//!
//! Two things have no good home in a `Result`. A tiling failure means *our*
//! parser dropped a region — evidence of a bug, never a reason to refuse the
//! file, since a map with a hole still answers "which roles does this dump
//! need". A cache mtime mismatch is weak evidence that the source changed,
//! too weak to invalidate on and with no column or schema to hang off. The
//! library also cannot `eprintln!`: it is destined to sit inside DataFusion.
//! So both are collected on [`crate::index::DumpIndex`] and left for a
//! caller to drain.
//!
//! **Not persisted.** `DumpIndex::diagnostics` is `#[serde(skip)]`, because a
//! cached diagnostic would replay a warning about a check *this* run
//! performed successfully — the mtime a cache was saved with is not the mtime
//! the next run observes. Recomputing on load is O(spans) against a scan that
//! just read the file, so it is free.
//!
//! ## Why [`Severity`] is here but the per-column outcome is not
//!
//! The phase doc asks for one vocabulary across Phase 2's per-column
//! resolution outcomes, the cache-identity warning, the TOC-coverage figure
//! and an unrecognized span. What it did not account for is
//! [`layering.md`](../../docs/design/layering.md): `DumpIndex` is **L1**, and
//! `crate::resolve::ColumnResolution` is an **L2** conclusion about
//! PostgreSQL type semantics — a `DiagnosticKind` variant carrying one would
//! make L1 name an L2 type, which rule 1 forbids and which L1's whole
//! "parses a declared type as an opaque string and never interprets it"
//! premise rules out.
//!
//! So the shared vocabulary is [`Severity`] and the `{severity, kind}` shape,
//! not a single enum: L1 owns this file-level channel, and
//! `crate::resolve::Diagnostic` keeps its structured per-column payload at L2
//! while carrying the same `Severity`. A caller draining both sees one scale.

use crate::map::TilingIssue;

/// How much a diagnostic matters. One scale for every producer, so a caller
/// draining several channels can filter uniformly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// The map does not tile its file: a gap, an overlap, an empty span, or
    /// a span list that doesn't reach the scanned end. Always a bug in
    /// `crate::map`, never a property of the input
    /// (`roadmap-phase3-object-inventory.md`, "Tiling is verified at runtime
    /// and reported as a diagnostic").
    TilingBroken { issues: Vec<TilingIssue> },
    /// A loaded cache recorded a different mtime than the source now has.
    /// Deliberately not an invalidation: mtime granularity and preservation
    /// vary too much across filesystems, copies and restores to be
    /// conclusive (`roadmap-phase3-object-inventory.md`, "Cache: the dump
    /// file's identity is checked, not assumed"). A *size* mismatch is an
    /// invalidation instead, and never reaches this channel because the
    /// cache is discarded outright.
    CacheMtimeChanged,
}

/// One thing worth telling the caller about a file, with no `Result` to carry
/// it — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub kind: DiagnosticKind,
}

impl Diagnostic {
    pub(crate) fn tiling_broken(issues: Vec<TilingIssue>) -> Self {
        Self { severity: Severity::Error, kind: DiagnosticKind::TilingBroken { issues } }
    }

    pub(crate) fn cache_mtime_changed() -> Self {
        Self { severity: Severity::Warning, kind: DiagnosticKind::CacheMtimeChanged }
    }
}
