//! The structural summary a scan produces: where every COPY block lives.
//!
//! This is what the eager `pgdq parse` scan yields today and what the
//! structure cache will persist once it exists.

use std::ops::ControlFlow;

use serde::{Deserialize, Serialize};

use crate::cache::CacheMode;
use crate::copy::CopyHeader;
use crate::io::ByteRangeSource;
use crate::preamble::PreambleBuilder;
use crate::scan::{Event, ScanOptions, scan};
use crate::{CopyStart, Result};

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

/// The COPY blocks discovered in a dump, in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpIndex {
    pub blocks: Vec<CopyBlock>,
    /// How much of the file the scan covered. Equal to the file size after an
    /// eager scan.
    pub scanned_through: u64,
    /// Dump-level preamble metadata — see [`DumpMetadata`]. Always `None`
    /// for a `DumpIndex` a caller built by hand rather than through
    /// [`build_index`] (Phase 1's default), but every `build_index` scan now
    /// populates it.
    pub metadata: Option<DumpMetadata>,
}

impl DumpIndex {
    /// Blocks whose table matches `name`, given qualified (`schema.table`) or
    /// bare (`table`, any schema).
    pub fn blocks_for(&self, name: &str) -> impl Iterator<Item = &CopyBlock> {
        self.blocks.iter().filter(move |b| b.header.matches(name))
    }

    pub fn total_rows(&self) -> u64 {
        self.blocks.iter().map(|b| b.row_count).sum()
    }
}

/// Scan `source` end to end and collect its COPY block structure, plus its
/// dump-level metadata (`docs/design/roadmap-phase2-typed-columns.md`, "The
/// preamble pass") — a single pass serves both, since the preamble builder
/// only ever looks at the same outside-block lines this scan already walks.
pub async fn build_index<S: ByteRangeSource>(
    source: &S,
    options: &ScanOptions,
) -> Result<DumpIndex> {
    let mut index = DumpIndex::default();
    let mut pending: Option<CopyStart> = None;
    let mut preamble = PreambleBuilder::new();

    scan(source, options, |event| {
        match event {
            Event::CopyStart(start) => {
                preamble.on_copy_start();
                pending = Some(start);
            }
            Event::Row(_) => {}
            Event::CopyEnd(end) => {
                // A `CopyEnd` is only ever emitted after a `CopyStart`.
                if let Some(start) = pending.take() {
                    index.blocks.push(CopyBlock {
                        header: start.header,
                        database: preamble.current_database_name(),
                        header_offset: start.header_offset,
                        data_offset: start.data_offset,
                        terminator_offset: end.terminator_offset,
                        end_offset: end.end_offset,
                        row_count: end.row_count,
                        sparse_index: None,
                        column_stats: None,
                    });
                }
            }
            Event::Line(line) => preamble.feed_line(line.raw),
        }
        ControlFlow::Continue(())
    })
    .await?;

    index.scanned_through = source.size().await?;
    index.metadata = Some(preamble.finish());
    Ok(index)
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
/// Phase 2.2.1 (`docs/design/roadmap-phase2.2.1-incremental-preamble-notes.md`):
/// exists so an incremental scan (`crate::stream::table_stream`) can
/// guarantee this metadata gets captured even when the query's own target
/// table starts later in the file (or never appears at all) — see also
/// `docs/design/roadmap-phase2-typed-columns.md`, "Companion: dump-level
/// metadata".
///
/// Returns the recovered metadata and the offset just past the scanned
/// prefix — a safe watermark for a later scan to continue from, since no
/// `COPY` block starts before it.
pub(crate) async fn scan_preamble<S: ByteRangeSource>(
    source: &S,
    options: &ScanOptions,
) -> Result<(DumpMetadata, u64)> {
    let mut preamble = PreambleBuilder::new();
    let mut end = source.size().await?;
    scan(source, options, |event| match event {
        Event::CopyStart(start) => {
            preamble.on_copy_start();
            end = start.header_offset;
            ControlFlow::Break(())
        }
        Event::Line(line) => {
            preamble.feed_line(line.raw);
            ControlFlow::Continue(())
        }
        _ => ControlFlow::Continue(()),
    })
    .await?;
    Ok((preamble.finish(), end))
}

/// Answer from the preamble alone (`docs/design/roadmap-phase2-typed-columns.md`,
/// "CLI", `--preamble-only`): reuse a cache's already-known metadata when
/// present, falling back to a fresh [`scan_preamble`] otherwise and
/// persisting the result when the cache is enabled (a no-op when it isn't —
/// see [`CacheMode::save`]). Bounded to the file's first `COPY` block
/// regardless of dump size (I1), independent of `build_index`'s full
/// structural scan.
pub async fn preamble_only<S: ByteRangeSource>(
    source: &S,
    options: &ScanOptions,
    cache: &CacheMode,
) -> Result<DumpMetadata> {
    let mut base_index = cache.load()?.unwrap_or_default();
    let known = base_index
        .metadata
        .as_ref()
        .and_then(|m| m.databases.first())
        .is_some_and(|db| db.preamble_complete);
    if !known {
        let (metadata, preamble_end) = scan_preamble(source, options).await?;
        base_index.metadata = Some(metadata);
        base_index.scanned_through = base_index.scanned_through.max(preamble_end);
        cache.save(&base_index)?;
    }
    Ok(base_index.metadata.unwrap_or_default())
}
