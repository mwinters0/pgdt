//! The structural summary a scan produces: where every COPY block lives.
//!
//! This is what the eager `pgdq parse` scan yields today and what the
//! structure cache will persist once it exists.

use std::ops::ControlFlow;

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::cache::CacheMode;
use crate::copy::CopyHeader;
use crate::io::ByteRangeSource;
use crate::map::{Span, SpanBody};
use crate::preamble::PreambleBuilder;
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
    /// populates it.
    ///
    /// Not yet a derived view over `spans` (`docs/design/roadmap-phase3-object-inventory.md`'s
    /// "The span is the container" describes the eventual shape) — this is
    /// still populated by its own [`PreambleBuilder`] pass run alongside the
    /// span builder, per `docs/design/roadmap-phase3.2.1-span-wiring-notes.md`'s
    /// "What this slice does not do".
    pub metadata: Option<DumpMetadata>,
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

/// Scan `source` end to end and build its full file map, plus its dump-level
/// metadata (`docs/design/roadmap-phase2-typed-columns.md`, "The preamble
/// pass") — one pass serves both: [`crate::map::Builder`] and
/// [`PreambleBuilder`] are fed the same [`Event`] stream, since both only
/// ever look at the outside-block lines this scan already walks.
/// [`DumpIndex::blocks`] is then a filter over the resulting spans, not a
/// second scan (`docs/design/roadmap-phase3-object-inventory.md`, "The map is
/// the structure, not a description of it").
pub async fn build_index<S: ByteRangeSource>(
    source: &S,
    options: &ScanOptions,
) -> Result<DumpIndex> {
    let mut spans = crate::map::Builder::new();
    let mut preamble = PreambleBuilder::new();

    scan(source, options, |event| {
        match event {
            Event::CopyStart(start) => {
                preamble.on_copy_start();
                spans.on_copy_start(start);
            }
            Event::Row(_) => {}
            Event::CopyEnd(end) => spans.on_copy_end(end),
            Event::Line(line) => {
                preamble.feed_line(line.raw);
                spans.feed_line(line.offset, line.raw);
            }
        }
        ControlFlow::Continue(())
    })
    .await?;

    let size = source.size().await?;
    Ok(DumpIndex {
        spans: spans.finish(size),
        scanned_through: size,
        metadata: Some(preamble.finish()),
    })
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
/// the first `COPY` header rather than walking into the block), and that
/// offset itself — a safe watermark for a later scan to continue from, since
/// no `COPY` block starts before it.
pub(crate) async fn scan_preamble<S: ByteRangeSource>(
    source: &S,
    options: &ScanOptions,
) -> Result<(DumpMetadata, Vec<Span>, u64)> {
    let mut preamble = PreambleBuilder::new();
    let mut spans = crate::map::Builder::new();
    let mut end = source.size().await?;
    scan(source, options, |event| match event {
        Event::CopyStart(start) => {
            // The map builder is deliberately not fed this event: it would
            // open a `Data` span this scan never closes (it stops here
            // rather than walking the block), and whatever TOC comment
            // precedes the header is exactly what should flush as its own
            // trailing span instead — see `finish` below.
            preamble.on_copy_start();
            end = start.header_offset;
            ControlFlow::Break(())
        }
        Event::Line(line) => {
            preamble.feed_line(line.raw);
            spans.feed_line(line.offset, line.raw);
            ControlFlow::Continue(())
        }
        _ => ControlFlow::Continue(()),
    })
    .await?;
    Ok((preamble.finish(), spans.finish(end), end))
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
        let (metadata, mut spans, preamble_end) = scan_preamble(source, options).await?;
        base_index.metadata = Some(metadata);
        base_index.scanned_through = base_index.scanned_through.max(preamble_end);
        let file_size = source.size().await?;
        if preamble_end < file_size {
            spans.push(Span {
                start: preamble_end,
                end: file_size,
                database: None,
                body: SpanBody::Unscanned,
            });
        }
        base_index.spans.extend(spans);
        cache.save(source, &base_index).await?;
    }
    Ok(base_index.metadata.unwrap_or_default())
}
