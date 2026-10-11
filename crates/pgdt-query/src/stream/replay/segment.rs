//! One contiguous piece of one `COPY` block a sub-stream replays, and where
//! a piece's first row starts.

use std::ops::Range;

use pgdt_core::{Error, Result};
use pgdt_dump::ScanOptions;
use pgdt_io::ByteRangeSource;
use pgdt_model::CopyBlock;

/// What state a [`Segment`]'s scanner starts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::stream) enum SegmentEntry {
    /// Outside a block, at [`Segment::start`] — the `COPY` header line for a
    /// whole block, so the header the scanner reads is what resolves the
    /// schema. Also how a [`ResumeToken`](crate::stream::resume::ResumeToken) that paused *between* blocks
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
pub(in crate::stream) struct Segment {
    /// The block this piece is of, **without its statistics**
    /// ([`Segment::new`]).
    pub(in crate::stream) block: CopyBlock,
    pub(in crate::stream) start: u64,
    pub(in crate::stream) limit: u64,
    pub(in crate::stream) entry: SegmentEntry,
}

impl Segment {
    /// The piece of `block` from `start` to `limit`, entered as `entry`.
    ///
    /// **The copy drops the block's statistics.** Nothing reading a segment
    /// needs them once [`ReplayPlan::new`](super::plan::ReplayPlan::new) has pruned — a replay under a
    /// [`DynamicFilter`](crate::stream::replay::dynamic::DynamicFilter) reads them from the [`TablePartitions`](crate::stream::partitions::TablePartitions) holding them
    /// — and a copy keeping the `Arc` would hold every matched block's
    /// resident for as long as the replay runs, after the map that loaded
    /// them is gone, which is what lets a caller of [`table_stream`](crate::stream::table_stream) or
    /// [`table_stream_partitions`](crate::stream::partitions::table_stream_partitions) carve a replay with nothing held for them.
    pub(in crate::stream) fn new(
        block: &CopyBlock,
        start: u64,
        limit: u64,
        entry: SegmentEntry,
    ) -> Self {
        let block = CopyBlock { statistics: None, ..block.clone() };
        Segment { block, start, limit, entry }
    }

    /// The piece of `block` replaying `run`, a range in segment terms: the
    /// run that begins before the block's first row starts at its header, as
    /// a whole block's first piece does, and any other inside its data.
    pub(in crate::stream) fn over(block: &CopyBlock, run: Range<u64>) -> Self {
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
    pub(in crate::stream) fn resumed_at(mut self, offset: u64) -> Option<Self> {
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
    pub(in crate::stream) fn rest_from(&self, start: u64) -> Self {
        Segment::new(&self.block, start, self.limit, SegmentEntry::Interior)
    }

    /// The bytes this piece is responsible for, for balancing sub-streams
    /// against each other. Approximate at both ends by exactly one row, which
    /// is why nothing reads it as an extent. Measured from the block's
    /// **data**, so the first piece is not charged for the header line it also
    /// covers.
    pub(in crate::stream) fn weight(&self) -> u64 {
        self.limit.saturating_sub(self.start.max(self.block.data_offset))
    }
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
pub(in crate::stream) async fn first_row_start(
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
