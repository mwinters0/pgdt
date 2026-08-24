//! The structural summary a scan produces: where every COPY block lives.
//!
//! This is what the eager `pgdq parse` scan yields today and what the
//! structure cache will persist once it exists.

use std::collections::BTreeSet;
use std::ops::ControlFlow;

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::cache::CacheMode;
use crate::copy::CopyHeader;
use crate::io::ByteRangeSource;
use crate::map::{Span, SpanBody};
use crate::scan::{Event, ScanOptions, scan};

/// A block's sparse row index: the byte offset of every `interval`-th data
/// row, letting a later reader seek into the middle of a large block instead
/// of scanning from its start. Reserved in the cache format from the first
/// release; not populated until roadmap Phase 7
/// (`docs/design/roadmap-phase7-scan-performance.md`, "Cache: a sparse row
/// index") — no code constructs one yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SparseRowIndex {
    /// Rows between checkpoints (matches the default batch size, 8192 — see
    /// `roadmap-phase7-scan-performance.md`).
    pub interval: u64,
    /// `checkpoints[i]` is the byte offset of data row `i * interval` within
    /// the block.
    pub checkpoints: Vec<u64>,
}

/// Per-row-group column statistics for one block, keyed to its
/// [`SparseRowIndex`] checkpoints. Reserved in the cache format from the
/// first release; not populated until roadmap Phase 5
/// (`docs/design/roadmap.md`, "Companion: per-row-group column statistics")
/// defines its real shape (null counts, sortedness, min/max, the type each
/// was computed as) — no code constructs one yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowGroupStats {}

/// Dump-level preamble: source server version, `pg_dump` version, extension
/// list, user-defined type definitions. Populated by [`build_index`] via
/// `crate::preamble` (roadmap Phase 2.2,
/// `docs/design/roadmap-phase2-typed-columns.md`, "The preamble pass").
pub use crate::preamble::DumpMetadata;

/// One located COPY block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyBlock {
    pub header: CopyHeader,
    /// The database this block belongs to — the name from the `\connect`
    /// governing it, read straight off the line as either scan pass sees it.
    /// `None` means the file had no `\connect` at all (a plain dump), which
    /// falls back to the single unnamed [`DatabaseMetadata`]. Not an ordinal
    /// into `metadata.databases`: an incremental scan's metadata can hold
    /// just one entry no matter how many databases the file contains, so an
    /// index would be unresolvable in exactly the case this field exists for
    /// (`docs/design/roadmap-phase2-typed-columns.md`, "One target per
    /// query").
    pub database: Option<String>,
    /// Absolute file offset of the `C` in `COPY`.
    pub header_offset: u64,
    /// Absolute file offset of the block's first data byte.
    pub data_offset: u64,
    /// Absolute file offset of the `\.` terminator line.
    pub terminator_offset: u64,
    /// Absolute file offset just past the terminator line.
    pub end_offset: u64,
    pub row_count: u64,
    /// The root table named by this block's `-- load via partition root
    /// <name>` marker, if it carried one (I2) — meaning `header` names that
    /// root rather than the partition whose rows follow, and **other blocks
    /// in this same dump carry the same header name**. Stored rather than
    /// concluded from: it is a line the dump wrote, which is what
    /// `layering.md` rule 5 asks L1 to keep.
    ///
    /// `crate::stream::table_stream` reads it to decide whether a cold query
    /// may stop once the queried table's block closes, or must run to EOF
    /// because more blocks can share the name — the blocks are *not*
    /// adjacent, so nothing cheaper than EOF enumerates them
    /// (`docs/design/roadmap-phase3-object-inventory.md`, "Mapping and
    /// streaming are separate passes").
    pub partition_root: Option<String>,
    /// Reserved — see [`SparseRowIndex`]. Always `None` in Phase 1.
    pub sparse_index: Option<SparseRowIndex>,
    /// Reserved — see [`RowGroupStats`]. Always `None` in Phase 1.
    pub column_stats: Option<RowGroupStats>,
}

/// The full file map discovered in a dump, in file order
/// (`docs/design/roadmap-phase3-object-inventory.md`, "The map is the
/// structure, not a description of it"). [`DumpIndex::blocks`] is a derived
/// filter over it, not a second stored structure.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpIndex {
    pub spans: Vec<Span>,
    /// How much of the file the scan covered. Equal to the file size after an
    /// eager scan.
    pub scanned_through: u64,
    /// Dump-level preamble metadata — see [`DumpMetadata`]. Always `None`
    /// for a `DumpIndex` a caller built by hand rather than through
    /// [`build_index`] (Phase 1's default), but every `build_index` scan now
    /// populates it. A derived view over `spans`
    /// (`crate::preamble::dump_metadata_from_spans`), computed once when the
    /// scan that produced `spans` finishes rather than stored twice — see
    /// `docs/design/roadmap-phase3-object-inventory.md`, "The span is the
    /// container".
    pub metadata: Option<DumpMetadata>,
    /// Roles referenced anywhere the scan has reached — the TOC `Owner:`
    /// field, `ALTER ... OWNER TO`, and `GRANT`/`REVOKE`/`ALTER DEFAULT
    /// PRIVILEGES FOR ROLE` (`docs/design/roadmap-phase3-object-inventory.md`,
    /// "What a span carries"). `PUBLIC` is never included. Flat and per-file
    /// — a per-database view is a filter over `Span::database`, not a second
    /// stored structure. Persisted: unlike `metadata`/`diagnostics`, there's
    /// no cheaper way to answer "which roles does this dump need" than
    /// keeping what the scan already found, since re-deriving it means
    /// re-parsing every span's raw text. Complete only once `scanned_through`
    /// reaches the file's size — a query that stops at its target (`crate::stream`)
    /// never sees a reference past the stopping point, the same partiality
    /// `metadata`'s `preamble_complete` flags for its own data.
    pub roles: BTreeSet<String>,
    /// Tablespaces referenced anywhere the scan has reached — the TOC
    /// `Tablespace:` field and `SET default_tablespace = ...;`. `pg_default`
    /// is never included. Same flatness, persistence and partial-scan caveat
    /// as [`roles`](Self::roles).
    pub tablespaces: BTreeSet<String>,
    /// Things worth telling the caller that have no `Result` to travel in —
    /// a tiling failure, a cache mtime mismatch. **Not persisted**
    /// (`#[serde(skip)]`): a cached diagnostic would replay a warning about a
    /// check *this* run performed successfully. Recomputed wherever an index
    /// is produced or loaded; see [`crate::diagnostic`].
    #[serde(skip)]
    pub diagnostics: Vec<crate::diagnostic::Diagnostic>,
}

impl DumpIndex {
    /// The `COPY` blocks among `spans`, in file order — a filtered view, not
    /// a stored field, so a block's byte offsets have exactly one owner
    /// (`docs/design/roadmap-phase3-object-inventory.md`, "The map is the
    /// structure, not a description of it").
    pub fn blocks(&self) -> impl Iterator<Item = &CopyBlock> {
        self.spans.iter().filter_map(|s| match &s.body {
            SpanBody::Data(block) => Some(block),
            _ => None,
        })
    }

    /// Blocks whose table matches `name`, given qualified (`schema.table`) or
    /// bare (`table`, any schema).
    pub fn blocks_for(&self, name: &str) -> impl Iterator<Item = &CopyBlock> {
        self.blocks().filter(move |b| b.header.matches(name))
    }

    pub fn total_rows(&self) -> u64 {
        self.blocks().map(|b| b.row_count).sum()
    }
}

/// Scan `source` end to end and build its full file map — [`crate::map::Builder`]
/// is fed the same [`Event`] stream as `CopyBlock` discovery, so this is one
/// pass, not two. `DumpIndex::metadata` is then [`crate::preamble::dump_metadata_from_spans`]
/// over the result, and [`DumpIndex::blocks`] a filter over it — neither is a
/// second scan (`docs/design/roadmap-phase3-object-inventory.md`, "The map is
/// the structure, not a description of it").
pub async fn build_index<S: ByteRangeSource>(
    source: &S,
    options: &ScanOptions,
) -> Result<DumpIndex> {
    let mut spans = crate::map::Builder::new();

    scan(source, options, |event| {
        match event {
            Event::CopyStart(start) => spans.on_copy_start(start),
            Event::Row(_) => {}
            Event::CopyEnd(end) => spans.on_copy_end(end),
            Event::Line(line) => spans.feed_line(line.offset, line.raw),
            Event::DollarQuoteEnd(end) => spans.on_dollar_quote_end(end.offset),
        }
        ControlFlow::Continue(())
    })
    .await?;

    let size = source.size().await?;
    let roles = spans.roles().clone();
    let tablespaces = spans.tablespaces().clone();
    let mut spans = spans.finish(size);
    let metadata = Some(crate::preamble::dump_metadata_from_spans(&spans));
    crate::map::attach_text(source, &mut spans).await?;
    let mut diagnostics = tiling_diagnostics(&spans, size);
    diagnostics.push(toc_coverage_diagnostic(&spans));
    Ok(DumpIndex { spans, scanned_through: size, metadata, roles, tablespaces, diagnostics })
}

/// Run the tiling check over a finished map and turn any failure into a
/// diagnostic (`docs/design/roadmap-phase3-object-inventory.md`, "Tiling is
/// verified at runtime and reported as a diagnostic").
///
/// A hole means *we* have a bug, not that the dump is bad, so the map is
/// still returned: refusing to answer "which roles does this file need" over
/// an accounting discrepancy serves nobody. The check is O(spans) against a
/// scan that just read the whole region, so it is free — and what it guards
/// is a silently dropped region on a dump shape no fixture covers, which is
/// exactly what a test cannot catch.
pub(crate) fn tiling_diagnostics(
    spans: &[Span],
    expected_end: u64,
) -> Vec<crate::diagnostic::Diagnostic> {
    let issues = crate::map::check_tiling(spans, expected_end);
    if issues.is_empty() {
        Vec::new()
    } else {
        vec![crate::diagnostic::Diagnostic::tiling_broken(issues)]
    }
}

/// The TOC-coverage figure for a finished map: how many `spans` carry a
/// parsed [`crate::map::Span::toc`] against how many spans exist at all
/// (`docs/design/roadmap-phase3-object-inventory.md`, "TOC coverage is
/// recorded per file"). Always produced, never conditionally — a
/// `pg_dump`-compatible file with zero TOC comments is a normal, reported
/// state (the map running in header-less degraded mode), not an error, so
/// `headers == 0` is a legitimate value here rather than something this
/// function special-cases away.
pub(crate) fn toc_coverage_diagnostic(spans: &[Span]) -> crate::diagnostic::Diagnostic {
    let headers = spans.iter().filter(|s| s.toc.is_some()).count();
    crate::diagnostic::Diagnostic::toc_coverage(headers, spans.len())
}

/// Scan only far enough to recover the first database's preamble — up to
/// (not including) the first `COPY` block header in the file, or to EOF if
/// none exists. Per I1 (`docs/design/postgres-invariants.md`), nothing
/// `crate::preamble` cares about can follow that point for whichever
/// database is open when it's reached, and no database earlier in the file
/// (in a multi-`\connect` dump) can have a `COPY` block of its own before it
/// either — so this one offset always closes out the *first* database's
/// preamble, incidentally finishing any earlier, table-less database's too.
///
/// Phase 2.2.1 (`docs/design/roadmap-phase2-typed-columns-notes.md`,
/// "Preamble parsing"): exists so an incremental scan
/// (`crate::stream::table_stream`) can
/// guarantee this metadata gets captured even when the query's own target
/// table starts later in the file (or never appears at all) — see also
/// `docs/design/roadmap-phase2-typed-columns.md`, "Companion: dump-level
/// metadata".
///
/// Returns the recovered metadata, the spans tiling `[0, preamble_end)` (per
/// `crate::map::Builder` — no `Data` span among them, since the scan stops at
/// the first `COPY` header rather than walking into the block), that offset
/// itself — a safe watermark for a later scan to continue from, since no
/// `COPY` block starts before it — and whatever roles/tablespaces the
/// preamble region referenced (`DumpIndex::roles`/`tablespaces`'s own
/// partial-scan caveat applies here too).
pub(crate) async fn scan_preamble<S: ByteRangeSource>(
    source: &S,
    options: &ScanOptions,
) -> Result<(DumpMetadata, Vec<Span>, u64, BTreeSet<String>, BTreeSet<String>)> {
    let mut spans = crate::map::Builder::new();
    let mut end = source.size().await?;
    scan(source, options, |event| match event {
        Event::CopyStart(start) => {
            // The map builder is deliberately not fed this event: it would
            // open a `Data` span this scan never closes (it stops here
            // rather than walking the block). If a TOC comment precedes the
            // header, `finish` below must not swallow it either — since
            // Phase 3.3, `map::Builder` absorbs such a comment straight into
            // the `Data` span a later, unfed-truncated scan produces
            // (`roadmap-phase3-object-inventory.md`, "COPY blocks are the
            // one exception"), so retreating to the comment's own start
            // leaves it for that scan rather than guessing it here as its
            // own `Framing`/`Unparsed` span.
            end = spans.pending_comment_start().unwrap_or(start.header_offset);
            ControlFlow::Break(())
        }
        Event::Line(line) => {
            spans.feed_line(line.offset, line.raw);
            ControlFlow::Continue(())
        }
        Event::DollarQuoteEnd(end) => {
            spans.on_dollar_quote_end(end.offset);
            ControlFlow::Continue(())
        }
        _ => ControlFlow::Continue(()),
    })
    .await?;
    let roles = spans.roles().clone();
    let tablespaces = spans.tablespaces().clone();
    let spans = spans.finish(end);
    let metadata = crate::preamble::dump_metadata_from_spans(&spans);
    Ok((metadata, spans, end, roles, tablespaces))
}

/// Answer from the preamble alone (`docs/design/roadmap-phase2-typed-columns.md`,
/// "CLI", `--preamble-only`): reuse a cache's already-known metadata when
/// present, falling back to a fresh [`scan_preamble`] otherwise and
/// persisting the result when the cache is enabled (a no-op when it isn't —
/// see [`CacheMode::save`]). Bounded to the file's first `COPY` block
/// regardless of dump size (I1), independent of `build_index`'s full
/// structural scan.
///
/// A fresh scan's spans are persisted alongside the metadata, with a
/// trailing [`SpanBody::Unscanned`] span covering the rest of the file — this
/// is a genuinely partial scan (unlike `build_index`, which always reaches
/// EOF), so it's the one place today that produces that variant for real
/// rather than only in `crate::map`'s own unit tests
/// (`docs/design/roadmap-phase3-object-inventory.md`, "Scan coverage is a
/// prefix, expressed as a span").
pub async fn preamble_only<S: ByteRangeSource>(
    source: &S,
    options: &ScanOptions,
    cache: &CacheMode,
) -> Result<DumpMetadata> {
    let mut base_index = cache.load(source).await?.unwrap_or_default();
    let known = base_index
        .metadata
        .as_ref()
        .and_then(|m| m.databases.first())
        .is_some_and(|db| db.preamble_complete);
    if !known {
        let (metadata, mut spans, preamble_end, roles, tablespaces) =
            scan_preamble(source, options).await?;
        base_index.metadata = Some(metadata);
        base_index.roles.extend(roles);
        base_index.tablespaces.extend(tablespaces);
        base_index.scanned_through = base_index.scanned_through.max(preamble_end);
        let file_size = source.size().await?;
        if preamble_end < file_size {
            spans.push(Span {
                start: preamble_end,
                end: file_size,
                database: None,
                text: None,
                toc: None,
                body: SpanBody::Unscanned,
            });
        }
        base_index.spans.extend(spans);
        crate::map::attach_text(source, &mut base_index.spans).await?;
        cache.save(source, &base_index).await?;
    }
    Ok(base_index.metadata.unwrap_or_default())
}
