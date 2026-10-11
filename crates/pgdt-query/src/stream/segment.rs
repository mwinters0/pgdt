//! One contiguous piece of one `COPY` block a sub-stream replays, what a
//! stream publishes while it runs, and opening a block's state at a piece.

use std::ops::Range;
use std::pin::Pin;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use arrow::array::RecordBatch;
use futures::Stream;

use pgdt_codec::ResolvedSchema;
use pgdt_core::CopyHeader;
use pgdt_core::{Error, Result};
use pgdt_dump::ScanOptions;
use pgdt_io::ByteRangeSource;
use pgdt_model::CopyBlock;

use super::note::{EarlyStop, PlanNote};
use super::plan::{ReplayPlan, resolve_for_query};
use super::resolve::Opened;
use super::table::{ResumeToken, TableStream};
use crate::batch::RowBatcher;
use crate::predicate::ComparisonNote;

/// What state a [`Segment`]'s scanner starts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SegmentEntry {
    /// Outside a block, at [`Segment::start`] — the `COPY` header line for a
    /// whole block, so the header the scanner reads is what resolves the
    /// schema. Also how a [`ResumeToken`] that paused *between* blocks
    /// re-enters.
    Header,
    /// Inside the block's data: reading begins at the first row boundary
    /// after [`Segment::start`], and the schema comes from the map's own
    /// copy of the header, no header line being in range.
    Interior,
}

/// One contiguous piece of one `COPY` block that a sub-stream replays.
///
/// **The two offsets are not a byte range, and the difference is what makes
/// the pieces tile** (`docs/design/decisions.md`, "D51"). `start` is where the
/// *search* for this piece's first row begins: its first row is the one just
/// past the first LF at or after it. `limit` is not where reading stops: the
/// piece runs through the line that *ends* at the first LF at or after
/// `limit`, which is the row the next piece's search then skips. So a row
/// straddling a cut belongs to the piece before it, once.
#[derive(Debug, Clone)]
pub(super) struct Segment {
    /// The block this piece is of, **without its statistics**
    /// ([`Segment::new`]).
    pub(super) block: CopyBlock,
    pub(super) start: u64,
    pub(super) limit: u64,
    pub(super) entry: SegmentEntry,
}

impl Segment {
    /// The piece of `block` from `start` to `limit`, entered as `entry`.
    ///
    /// **The copy drops the block's statistics.** Nothing reading a segment
    /// needs them once [`ReplayPlan::new`] has pruned — a replay under a
    /// [`DynamicFilter`](super::dynamic::DynamicFilter) reads them from the [`TablePartitions`](super::table_partitions::TablePartitions) holding them
    /// — and a copy keeping the `Arc` would hold every matched block's
    /// resident for as long as the replay runs, after the map that loaded
    /// them is gone, which is what lets a caller of [`table_stream`](super::table::table_stream) or
    /// [`table_stream_partitions`](super::table::table_stream_partitions) carve a replay with nothing held for them.
    pub(super) fn new(block: &CopyBlock, start: u64, limit: u64, entry: SegmentEntry) -> Self {
        let block = CopyBlock { statistics: None, ..block.clone() };
        Segment { block, start, limit, entry }
    }

    /// The piece of `block` replaying `run`, a range in segment terms: the
    /// run that begins before the block's first row starts at its header, as
    /// a whole block's first piece does, and any other inside its data.
    pub(super) fn over(block: &CopyBlock, run: Range<u64>) -> Self {
        let (start, entry) = if run.start < block.data_offset {
            (block.header_offset, SegmentEntry::Header)
        } else {
            (run.start, SegmentEntry::Interior)
        };
        Segment::new(block, start, run.end, entry)
    }

    /// This piece as a stream resuming at `offset` — a row's start, or 0 —
    /// replays it: `None` where every row it owns lies before `offset`, and
    /// starting at `offset`'s row where the piece began before it. A piece
    /// begun at its block's header is then entered inside the data.
    pub(super) fn resumed_at(mut self, offset: u64) -> Option<Self> {
        let Some(search) = offset.checked_sub(1) else { return Some(self) };
        if self.limit < offset {
            return None;
        }
        if self.start < search {
            self.start = search;
            self.entry = SegmentEntry::Interior;
        }
        Some(self)
    }

    /// The rest of this piece from `start`, the search start of a group
    /// inside its data ([`DynamicRead::at_row`](super::dynamic::DynamicRead::at_row)): entered there, its limit
    /// unchanged.
    pub(super) fn rest_from(&self, start: u64) -> Self {
        Segment::new(&self.block, start, self.limit, SegmentEntry::Interior)
    }

    /// The bytes this piece is responsible for, for balancing sub-streams
    /// against each other. Approximate at both ends by exactly one row, which
    /// is why nothing reads it as an extent. Measured from the block's
    /// **data**, so the first piece is not charged for the header line it also
    /// covers.
    pub(super) fn weight(&self) -> u64 {
        self.limit.saturating_sub(self.start.max(self.block.data_offset))
    }
}

/// The values a [`TableStream`] publishes to its owner while it runs, plus
/// `plan_notes` — settled once, with the plan and before any row, unlike the
/// rest.
#[derive(Clone)]
pub(super) struct StreamShared {
    pub(super) position: Arc<Mutex<ResumeToken>>,
    pub(super) resolved_schema: Arc<Mutex<ResolvedSchema>>,
    pub(super) comparison_notes: Arc<Mutex<Vec<ComparisonNote>>>,
    pub(super) batch_offset: Arc<Mutex<u64>>,
    pub(super) plan_notes: Arc<Mutex<Vec<PlanNote>>>,
    pub(super) early_stops: Arc<Mutex<Vec<EarlyStop>>>,
    pub(super) dynamic_pruned: Arc<AtomicU64>,
    pub(super) dynamic_rows: Arc<AtomicU64>,
}

impl StreamShared {
    pub(super) fn new(token: ResumeToken) -> Self {
        Self {
            position: Arc::new(Mutex::new(token)),
            resolved_schema: Arc::new(Mutex::new(ResolvedSchema::default())),
            comparison_notes: Arc::new(Mutex::new(Vec::new())),
            batch_offset: Arc::new(Mutex::new(0)),
            plan_notes: Arc::new(Mutex::new(Vec::new())),
            early_stops: Arc::new(Mutex::new(Vec::new())),
            dynamic_pruned: Arc::new(AtomicU64::new(0)),
            dynamic_rows: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Attach the plan-level notes, known in full before any row is read and
    /// never changed after.
    pub(super) fn with_plan_notes(self, plan_notes: Vec<PlanNote>) -> Self {
        *self.plan_notes.lock().unwrap() = plan_notes;
        self
    }

    pub(super) fn into_stream<'a>(
        self,
        inner: Pin<Box<dyn Stream<Item = Result<RecordBatch>> + Send + 'a>>,
    ) -> TableStream<'a> {
        TableStream {
            inner,
            position: self.position,
            resolved_schema: self.resolved_schema,
            comparison_notes: self.comparison_notes,
            batch_offset: self.batch_offset,
            plan_notes: self.plan_notes,
            early_stops: self.early_stops,
            dynamic_pruned: self.dynamic_pruned,
            dynamic_rows: self.dynamic_rows,
        }
    }
}

/// Take one block's resolution from `plan` — resolving it here where the
/// plan holds none ([`plan_blocks`](super::plan::plan_blocks)) — and build the [`RowBatcher`] that will
/// hold its rows.
///
/// The three places a block becomes active — a `COPY` header the scanner
/// read, a partition that started inside a block, and a resumed token —
/// differ only in where the header comes from.
pub(super) fn activate(
    header: CopyHeader,
    header_offset: u64,
    database: Option<String>,
    plan: &ReplayPlan,
) -> Result<Opened> {
    let resolved_here;
    let block = match plan
        .blocks
        .get(&header_offset)
        .filter(|planned| planned.database == database && planned.header == header)
    {
        Some(planned) => planned,
        None => {
            resolved_here = resolve_for_query(
                &header,
                header_offset,
                database.as_deref(),
                &plan.query_options,
                plan.metadata.as_ref(),
                &plan.table,
                None,
            )?;
            &resolved_here
        }
    };
    let batcher = RowBatcher::new(
        &block.resolved,
        header.qualified_name(),
        plan.query_options.clone(),
        block.field_targets.clone(),
        block.unrepresentable.clone(),
    );
    let (resolved, notes) = (block.resolved.clone(), block.notes.clone());
    Ok(((header_offset, header, batcher, Arc::clone(&block.filter), database), resolved, notes))
}

/// The offset of the first row boundary after `from`, searching no
/// further than `end` — the byte just past the first LF in `[from, end)`, or
/// `None` when there is none.
///
/// **A partition never resyncs by handing the scanner a mid-row byte**
/// (`docs/design/decisions.md`, "D51"). The scanner in its `InCopy` state
/// treats every line as a row, and a row's tail can be the two bytes `\.`,
/// which it would read as the block's terminator; I7's guarantee is about a
/// *line start*, so it covers a scanner started here and not one started
/// mid-row.
pub(super) async fn first_row_start(
    source: &dyn ByteRangeSource,
    from: u64,
    end: u64,
    options: &ScanOptions,
) -> Result<Option<u64>> {
    let mut pos = from;
    while pos < end {
        let want = options.chunk_size_bytes.min((end - pos) as usize);
        let bytes = source.read_range(pos, want).await?;
        if bytes.is_empty() {
            return Ok(None);
        }
        if let Some(nl) = memchr::memchr(b'\n', &bytes) {
            return Ok(Some(pos + nl as u64 + 1));
        }
        pos += bytes.len() as u64;
        if pos - from > options.max_line_bytes as u64 {
            return Err(Error::LineTooLong { offset: from, limit: options.max_line_bytes });
        }
    }
    Ok(None)
}
