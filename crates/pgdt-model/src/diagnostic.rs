//! The file-level diagnostic channel
//! (`docs/design/decisions.md`, "The file map and the preamble").
//!
//! The founding two had no good home in a `Result`: a tiling failure, which
//! is evidence of a bug in our parser but never a reason to refuse the file,
//! and a cache mtime mismatch, too weak by default to invalidate on and with
//! no column or schema to hang off. [`DiagnosticKind`] has grown past them —
//! its variants are the list. All are collected on
//! [`crate::DumpIndex`] and left for a caller to drain, but
//! `pgdt_dump::preamble_only`'s, which it returns beside the metadata.
//!
//! **This is not the library's only channel, and the division is by kind, not
//! by necessity.** A `tracing` line carries cost and progress as a run meets
//! them — wired to stderr for every command, on by default
//! (`docs/design/decisions.md`, "D64") — and can therefore speak *before* the
//! thing it describes happens. A [`Diagnostic`] carries a structured finding
//! **about the file**, reaches a caller as data rather than as text, and can
//! only be raised once there is a result to hang it on. A finding that is a
//! property of the file belongs here even when a status line already said it;
//! a cost a run is about to pay cannot be here at all.
//!
//! **Not persisted.** `DumpIndex::diagnostics` is `#[serde(skip)]`
//! (`docs/design/decisions.md`, "D34") and recomputed on load.
//!
//! [`Severity`] and the `{severity, kind}` shape are the vocabulary shared
//! across producers, not one enum: `DumpIndex` is the model's and
//! `pgdt_codec::ColumnResolution` is a codec conclusion, so a
//! [`DiagnosticKind`] variant carrying one would make the model name a codec
//! type (`docs/design/decisions.md`, "D68"). `pgdt_codec::ResolvedColumn` is
//! the per-column record in the codec — one per column, always present — and
//! `pgdt_query::ComparisonNote` the comparison one in query, per term
//! or, from `pgdt_query::Table::divergences`, per column, and
//! `pgdt_query::PlanNote` what a query's plan settled, also in query.
//!
//! **The four are unified at the drain, not at the storage type**: each
//! implements [`Finding`], as does what the provider reports over them, defined here in the model so that every crate can, and a
//! caller hands any of them to one [`DiagnosticSink`] of its own through
//! [`drain`]. Each channel is still read where it is produced — a
//! `DumpIndex`, a `ResolvedSchema`, a `TableStream` — so what the sink adds is
//! one scale and one sentence per finding, not one place they are stored.

use std::any::Any;
use std::fmt::Debug;

use serde::Serialize;

/// One finding from any of the library's diagnostic channels, as a
/// [`DiagnosticSink`] receives it: where it sits on the shared [`Severity`]
/// scale, and the sentence a human reads. Implemented by [`Diagnostic`] (the
/// model, about the file), `pgdt_codec::ResolvedColumn` (the codec, about one
/// column's type), `pgdt_query::ComparisonNote` (query, about one term of
/// a query, or one column in a query's semantics) and `pgdt_query::PlanNote`
/// (query, about one query's plan), and by what `datafusion-pgdump` reports
/// over them — `Located`, `RefusedTable`, `UncollatedColumns` and
/// `BudgetedPlanNote`.
///
/// A sink wanting the structured record rather than the sentence recovers it
/// through [`Finding::as_any`], downcasting to one of those.
pub trait Finding: Debug + Send + Sync {
    fn severity(&self) -> Severity;
    /// One sentence, with no severity label and no trailing newline: the
    /// label is the sink's to render, from [`Finding::severity`].
    fn message(&self) -> String;
    fn as_any(&self) -> &dyn Any;
}

/// Where a caller wants findings to go — a caller-supplied object the
/// library's channels drain into ([`drain`]). Shared, so a sink handed to
/// several concurrent readers is `Send + Sync`; any
/// `Fn(&dyn Finding) + Send + Sync` closure is one.
pub trait DiagnosticSink: Send + Sync {
    fn report(&self, finding: &dyn Finding);
}

impl<F: Fn(&dyn Finding) + Send + Sync> DiagnosticSink for F {
    fn report(&self, finding: &dyn Finding) {
        self(finding)
    }
}

/// Hand every finding of one channel to `sink`, in the channel's own order —
/// `sink` taking `&index.diagnostics`, `resolved.columns()`,
/// `&stream.comparison_notes()`, `&table.divergences(…)?` or
/// `&stream.plan_notes()` alike.
pub fn drain<'a, F: Finding + 'a>(
    sink: &dyn DiagnosticSink,
    findings: impl IntoIterator<Item = &'a F>,
) {
    for finding in findings {
        sink.report(finding);
    }
}

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

/// What [`pgdt_dump::check_tiling`] found wrong, if anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum TilingIssue {
    /// `spans[i]` and `spans[i + 1]` overlap or leave a gap between them.
    Discontinuity { after_index: usize, span_end: u64, next_start: u64 },
    /// The spans aren't sorted by `start` — every other check assumes they
    /// are, so this is reported instead of a misleading discontinuity.
    OutOfOrder { index: usize },
    /// The first span doesn't start at byte 0.
    DoesNotStartAtZero { first_start: u64 },
    /// The last span's `end` doesn't reach `expected_end`: the file's
    /// addressable size, which a partial map's trailing `Unscanned` span
    /// reaches too, or the map's watermark where that size is not known yet.
    DoesNotReachEnd { last_end: u64, expected_end: u64 },
    /// A span's own `start >= end`.
    Empty { index: usize },
}

/// What a [`Diagnostic`] is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum DiagnosticKind {
    /// The map does not tile its file: one or more [`TilingIssue`]s. Always a
    /// bug in
    /// `pgdt_dump::map` (`KD106` names one a hand-written input reaches),
    /// never a property of the input; `every_fixture_tiles_exactly`
    /// (`pgdt-mapping`'s `tests/map.rs`) checks a parse's map over the
    /// fixtures, and nothing checks other input
    /// (`docs/design/decisions.md`, "D30").
    TilingBroken { issues: Vec<TilingIssue> },
    /// A loaded cache recorded a different modification *time* than the source
    /// now has — an mtime locally, a `Last-Modified` remotely. Not an
    /// invalidation by default (`docs/design/decisions.md`, "D21"); a caller
    /// that asked for `pgdt_dump::cache::StrictIdentity::time` is refused instead
    /// of being handed this. A *size* mismatch never reaches this channel: the
    /// run is refused, the cache left as it is unless the mode may overwrite
    /// an unusable one (`docs/design/decisions.md`, "D20").
    CacheMtimeChanged,
    /// The stronger half of the same signal: both sides carry a server's
    /// **entity tag** and the two differ, which is the server's own statement
    /// that this is a different version of the object. Its own kind rather
    /// than [`DiagnosticKind::CacheMtimeChanged`]'s wording widened, because a
    /// local dump has no such tag and a message about one would be noise
    /// there. Bound by the same selector
    /// (`docs/design/decisions.md`, "D21").
    CacheEntityTagChanged,
    /// A loaded cache was written for a different **origin** than this run
    /// reads — where the object was fetched from, which a local source has
    /// none of. Advisory by the same rule the signal above is
    /// (`docs/design/decisions.md`, "D87"); a caller that asked
    /// for `pgdt_dump::cache::StrictIdentity::location` is refused instead.
    ///
    /// It is what makes the working-directory default cache path's one
    /// collision visible: two same-named dumps of equal stored size from
    /// different hosts read in one directory.
    CacheOriginChanged,
    /// How much of the map is attributed to a TOC entry: `attributed` spans
    /// out of `spans` total — a follow-on statement that inherited its
    /// governing entry's header (`crate::Span::toc_owned` is `false`)
    /// counts the same as one whose own comment carried it
    /// (`docs/design/decisions.md`, "D31"). Always `Info` — zero is a normal, reported state (the map
    /// running in header-less degraded mode), not an error.
    TocCoverage { attributed: usize, spans: usize },
    /// The index was loaded from a retained `.dtcache` with no live dump file
    /// to check it against (`docs/design/decisions.md`,
    /// "The compressed source and the cache") — unverified, there being no
    /// live source to compare its recorded size and mtime with. Pushed unconditionally by
    /// [`pgdt_dump::cache::OfflineCache::load`] on every successful
    /// cache-only load, an incomplete one included.
    CacheOffline,
    /// A compressed source has no usable seek structure, so a read inside its
    /// widest unit is reached by decoding from that unit's start — once for a
    /// forward scan, and again for each read behind where it stands. Never a
    /// reason to refuse the file (`docs/design/decisions.md`, "D19"). What
    /// counts as none is the container's rule ([`CompressedContainer`]): one
    /// `.xz` block, or a gzip unit wider than twice its checkpoint spacing.
    ///
    /// `block_count` is `CompressionIndex::unit_count()` and `largest_unit`
    /// `CompressionIndex::largest_unit()`, plaintext bytes — the number a gzip
    /// warning is raised on, and an `.xz` file's one block. Pushed once per
    /// complete index, by `pgdt_dump`'s `derived_diagnostics` wherever a whole
    /// map is finished or loaded, and by `pgdt_dump::preamble_only` for a
    /// preamble alone.
    NonSeekableCompressedSource {
        container: CompressedContainer,
        block_count: usize,
        largest_unit: u64,
    },
}

/// The compressed container a [`DiagnosticKind::NonSeekableCompressedSource`]
/// is about, with what its rule and its remedy read: the model's word for it,
/// the index being `pgdt_io`'s, which this crate sits beneath.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CompressedContainer {
    /// `.xz`, whose blocks are its writer's: one block is no seek structure
    /// whatever its size.
    Xz,
    /// gzip, whose units are the spans between checkpoints a reader placed at
    /// `spacing` plaintext bytes, each at the first place past it a decode
    /// resumes at — a deflate block boundary, or a static block's symbol: a
    /// unit wider than twice the spacing is a writer that coded a whole
    /// spacing in one dynamic block.
    Gzip { spacing: u64 },
}

/// One thing worth telling the caller about a file, with no `Result` to carry
/// it — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    severity: Severity,
    kind: DiagnosticKind,
}

impl Finding for Diagnostic {
    fn severity(&self) -> Severity {
        self.severity
    }

    fn message(&self) -> String {
        match &self.kind {
            DiagnosticKind::TilingBroken { issues } => format!(
                "the file map does not tile the file ({}) — this is a pgdt bug, please report it",
                tiling_tally(issues)
            ),
            DiagnosticKind::CacheMtimeChanged => "the dump file's mtime has changed since the \
                cache was saved (size still matches, so the cache was kept)"
                .to_string(),
            DiagnosticKind::CacheEntityTagChanged => "the server's entity tag for the dump has \
                changed since the cache was saved (size still matches, so the cache was kept)"
                .to_string(),
            DiagnosticKind::CacheOriginChanged => "the cache was written for a dump fetched from \
                somewhere else (size still matches, so the cache was kept)"
                .to_string(),
            DiagnosticKind::TocCoverage { attributed, spans } => {
                format!("TOC coverage: {attributed}/{spans} span(s) attributed to a TOC entry")
            }
            DiagnosticKind::CacheOffline => "answering from a cache with no source dump file to \
                check it against — unverified, historical as of whenever the cache was last saved"
                .to_string(),
            DiagnosticKind::NonSeekableCompressedSource {
                container: CompressedContainer::Xz,
                block_count,
                ..
            } => format!(
                "this .xz source has no seek structure ({block_count} block(s), one stream) — \
                 a read behind another decodes it again from byte 0 unless its one block is \
                 held; recompress with `xz -T0` or `--block-size=<size>` for random access"
            ),
            DiagnosticKind::NonSeekableCompressedSource {
                container: CompressedContainer::Gzip { spacing },
                block_count,
                largest_unit,
            } => format!(
                "this gzip source has no seek structure: its largest unit holds {largest_unit} \
                 byte(s) of plaintext, more than twice the {spacing}-byte spacing its \
                 checkpoints are placed at, its writer having coded them in one dynamic deflate \
                 block ({block_count} unit(s)) — every read inside that unit decodes it from its \
                 start; recompress with `bgzip` or `gzip` for random access"
            ),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `issues` counted by kind, a [`TilingIssue::Discontinuity`] a gap where the
/// next span starts past this one's end and an overlap where it starts before
/// it, in [`TilingIssue`]'s order and with every kind that has none left out:
/// `"2 gap(s), 1 overlap(s)"`. A kind the check raises at most once per map is
/// named without a count. The issues themselves are the kind's data.
fn tiling_tally(issues: &[TilingIssue]) -> String {
    let (mut gaps, mut overlaps, mut out_of_order, mut empty) = (0, 0, 0, 0);
    let (mut not_at_zero, mut short) = (false, false);
    for issue in issues {
        match issue {
            TilingIssue::Discontinuity { span_end, next_start, .. } => {
                if next_start > span_end {
                    gaps += 1;
                } else {
                    overlaps += 1;
                }
            }
            TilingIssue::OutOfOrder { .. } => out_of_order += 1,
            TilingIssue::DoesNotStartAtZero { .. } => not_at_zero = true,
            TilingIssue::DoesNotReachEnd { .. } => short = true,
            TilingIssue::Empty { .. } => empty += 1,
        }
    }
    let counted =
        [(gaps, "gap(s)"), (overlaps, "overlap(s)"), (out_of_order, "span(s) out of order")];
    let mut named: Vec<String> =
        counted.iter().filter(|(n, _)| *n > 0).map(|(n, kind)| format!("{n} {kind}")).collect();
    if not_at_zero {
        named.push("not starting at byte 0".to_string());
    }
    if short {
        named.push("not reaching the end of the file".to_string());
    }
    if empty > 0 {
        named.push(format!("{empty} empty span(s)"));
    }
    named.join(", ")
}

impl Diagnostic {
    /// How much it matters — the same answer as [`Finding::severity`], which
    /// a caller holding the concrete type need not import.
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// What was found. Its constructors are the crate's own (`DiagnosticExt`),
    /// so a severity always matches its kind.
    pub fn kind(&self) -> &DiagnosticKind {
        &self.kind
    }
}

/// What a crate above calls on [`Diagnostic`] that is not its API: public only
/// because a crate boundary forced it, and reached through `internal`.
pub trait DiagnosticExt {
    fn tiling_broken(issues: Vec<TilingIssue>) -> Self;
    fn toc_coverage(attributed: usize, spans: usize) -> Self;
    fn cache_offline() -> Self;
    fn non_seekable_compressed_source(
        container: CompressedContainer,
        block_count: usize,
        largest_unit: u64,
    ) -> Self;
    fn cache_mtime_changed() -> Self;
    fn cache_entity_tag_changed() -> Self;
    fn cache_origin_changed() -> Self;
}

impl DiagnosticExt for Diagnostic {
    fn tiling_broken(issues: Vec<TilingIssue>) -> Self {
        Self { severity: Severity::Error, kind: DiagnosticKind::TilingBroken { issues } }
    }

    fn toc_coverage(attributed: usize, spans: usize) -> Self {
        Self { severity: Severity::Info, kind: DiagnosticKind::TocCoverage { attributed, spans } }
    }

    fn cache_offline() -> Self {
        Self { severity: Severity::Warning, kind: DiagnosticKind::CacheOffline }
    }

    /// D19's warning: usable as-is, so `Warning` rather than `Error`.
    fn non_seekable_compressed_source(
        container: CompressedContainer,
        block_count: usize,
        largest_unit: u64,
    ) -> Self {
        Self {
            severity: Severity::Warning,
            kind: DiagnosticKind::NonSeekableCompressedSource {
                container,
                block_count,
                largest_unit,
            },
        }
    }

    /// A weak signal that moved, too weak to invalidate the cache
    /// (`pgdt_dump::cache::WeakIdentity::Differs`).
    fn cache_mtime_changed() -> Self {
        Self { severity: Severity::Warning, kind: DiagnosticKind::CacheMtimeChanged }
    }

    /// Both sides' entity tags differ (`pgdt_dump::cache::WeakIdentity::TagDiffers`).
    fn cache_entity_tag_changed() -> Self {
        Self { severity: Severity::Warning, kind: DiagnosticKind::CacheEntityTagChanged }
    }

    /// A cache written for another origin than this run reads
    /// (`pgdt_dump::cache::OriginMatch`).
    fn cache_origin_changed() -> Self {
        Self { severity: Severity::Warning, kind: DiagnosticKind::CacheOriginChanged }
    }
}

#[cfg(test)]
mod tests;
