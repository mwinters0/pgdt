//! The structural summary a scan produces: where every COPY block lives.
//!
//! This is what the eager `pgdq parse` scan yields today and what the
//! structure cache will persist once it exists.

use std::ops::ControlFlow;

use serde::{Deserialize, Serialize};

use crate::copy::CopyHeader;
use crate::io::ByteRangeSource;
use crate::scan::{Event, ScanOptions, scan};
use crate::{CopyStart, Result};

/// A block's sparse row index: the byte offset of every `interval`-th data
/// row, letting a later reader seek into the middle of a large block instead
/// of scanning from its start. Reserved in the cache format from the first
/// release; not populated until roadmap Phase 5
/// (`docs/design/roadmap-phase5-scan-performance.md`, "Cache: a sparse row
/// index") — no code constructs one yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SparseRowIndex {
    /// Rows between checkpoints (matches the default batch size, 8192 — see
    /// `roadmap-phase5-scan-performance.md`).
    pub interval: u64,
    /// `checkpoints[i]` is the byte offset of data row `i * interval` within
    /// the block.
    pub checkpoints: Vec<u64>,
}

/// Per-row-group column statistics for one block, keyed to its
/// [`SparseRowIndex`] checkpoints. Reserved in the cache format from the
/// first release; not populated until roadmap Phase 3
/// (`docs/design/roadmap.md`, "Companion: per-row-group column statistics")
/// defines its real shape (null counts, sortedness, min/max, the type each
/// was computed as) — no code constructs one yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowGroupStats {}

/// Dump-level preamble: source server version, `pg_dump` version, extension
/// list, user-defined type definitions. Reserved in the cache format from
/// the first release; not populated until roadmap Phase 2
/// (`docs/design/roadmap.md`, "Companion: dump-level metadata") defines its
/// real shape — no code constructs one yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpMetadata {}

/// One located COPY block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyBlock {
    pub header: CopyHeader,
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
    /// Reserved — see [`DumpMetadata`]. Always `None` in Phase 1.
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

/// Scan `source` end to end and collect its COPY block structure.
pub async fn build_index<S: ByteRangeSource>(
    source: &S,
    options: &ScanOptions,
) -> Result<DumpIndex> {
    let mut index = DumpIndex::default();
    let mut pending: Option<CopyStart> = None;

    scan(source, options, |event| {
        match event {
            Event::CopyStart(start) => pending = Some(start),
            Event::Row(_) => {}
            Event::CopyEnd(end) => {
                // A `CopyEnd` is only ever emitted after a `CopyStart`.
                if let Some(start) = pending.take() {
                    index.blocks.push(CopyBlock {
                        header: start.header,
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
        }
        ControlFlow::Continue(())
    })
    .await?;

    index.scanned_through = source.size().await?;
    Ok(index)
}
