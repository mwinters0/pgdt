//! Where a source may be cut and what one piece costs: the partition advice
//! a source gives ([`ByteRangeSource::partitions`](crate::ByteRangeSource::partitions)),
//! what a retained batch pins, how a worker reads its piece, and the cut the
//! scan and the leader both apply.

use std::ops::Range;

use crate::parallelism::WorkerMemory;

/// How many of a boundaried source's own units one partition covers
/// ([`Partitioning::window_end`], [`PartitionBoundaries::At`]) — **the cut
/// width**. See `docs/design/decisions.md`, "D8".
///
/// A worker reads one chunk past its piece's end, into the block its successor
/// owns, and a block-decoding source decodes that block once for both
/// ([`BlockCache::claim`](crate::source::compressed::blocks::BlockCache::claim)), so a wider cut amortises nothing. It does not
/// widen what a reader holds — retention is capped by [`BufferPool::slots`](crate::source::pool::BufferPool::slots), not by
/// the piece — and it costs granularity, a window being `want × k` units wide.
const BOUNDARIED_PARTITION_UNITS: usize = 1;

/// Where a source is willing to be split. See `docs/design/decisions.md`,
/// "D7".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartitionBoundaries {
    /// Anywhere in the range: no offset costs more to start reading at than
    /// another. A plain file's answer, and what a caller may cut into as many
    /// equal pieces as it has workers. **A fact about seeking, not a verdict
    /// on concurrency** — a scheduler must not read this arm as
    /// "device-bound, stay serial" (`docs/design/decisions.md`, "D7").
    Anywhere,
    /// Only at these offsets — ascending, and strictly inside the range, so
    /// `n` of them describe `n + 1` partitions.
    ///
    /// **An empty list is "one partition", and it is a policy rather than an
    /// absence**: a source that could enumerate boundaries and still advises
    /// none is saying that splitting this range makes its readers worse
    /// (`docs/design/decisions.md`, "D15").
    At(Vec<u64>),
}

/// What a caller's **retained** bytes are rounded out to on this source — the
/// unit a batch held across reads pins, as against the bytes one concurrent
/// reader costs while it is reading, which is
/// [`Partitioning::partition_bytes`].
///
/// A caller budgeting sub-streams charges each one its held batch's span *on
/// top of* the decode footprint, and that second charge is honest for one
/// source shape and double-counts for the other
/// (`docs/design/decisions.md`, "D46").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RetainedUnit {
    /// The read chunk. A batch spanning `pgdt_query::QueryOptions`'
    /// `max_source_span` bytes pins the chunk buffers those bytes were read
    /// into, and those are bytes [`Partitioning::partition_bytes`] has not
    /// charged for — so a caller adds the span. A plain file's answer.
    ///
    /// **The default, for the same reason [`WaitPolicy::NeverWait`](crate::WaitPolicy::NeverWait) is one**:
    /// over-charging plans fewer readers than the budget could hold, where
    /// under-charging plans readers it cannot.
    #[default]
    ReadChunk,
    /// The partition itself. A batch confined to a partition pins that
    /// partition whatever the span cap says
    /// (`docs/design/decisions.md`, "D46"), and
    /// [`Partitioning::partition_bytes`] has already charged for it — so
    /// adding the span would count the same bytes twice. A block-decoding
    /// source's answer.
    ///
    /// **That rationale holds only where a partition is one unit wide, which
    /// the leader's window guarantees and a query's cut does not** — `KD23`.
    /// `pgdt_mapping::interior::scan_region` sizes its window with
    /// [`Partitioning::window_end`] and is held to
    /// [`BOUNDARIED_PARTITION_UNITS`](crate::partition::BOUNDARIED_PARTITION_UNITS); `pgdt_query`'s `plan_partitions` cuts a
    /// whole `CopyBlock` into `min(workers, max_partitions)` pieces, so a
    /// piece may span more units than `partition_bytes` charged for.
    Partition,
}

/// How a worker reads the piece it was handed
/// (`pgdt_mapping::interior::scan_region`) — **stated by the source, because the
/// right answer differs by source**.
///
/// Separate from [`RetainedUnit`] and [`Partitioning::partition_bytes`], which
/// say what a reader *holds*: this says what shape its reads are, and the cut
/// size must not follow the memory charge ([`Partitioning::window_end`], and
/// `docs/design/decisions.md`, "D8").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PartitionRead {
    /// `ReadOptions::chunk_size_bytes` at a time, repeating until the piece is
    /// consumed. A plain file's answer, and **the default, for the reason
    /// [`RetainedUnit::ReadChunk`] is one**: a chunk-sized read is the
    /// announced length, so [`BufferPool::keeps`](crate::source::pool::BufferPool::keeps) pools every buffer a worker
    /// takes, where a partition-length read would allocate a buffer `keeps`
    /// refuses to pool.
    #[default]
    Chunked,
    /// One of the source's own `unit`s in a single call — the whole piece
    /// where the piece is one unit, which is what
    /// [`BOUNDARIED_PARTITION_UNITS`](crate::partition::BOUNDARIED_PARTITION_UNITS) makes it. A block-decoding source's
    /// answer, where that read is a **zero-copy slice** of a block the worker
    /// was going to decode anyway, against one [`BlockCache::claim`](crate::source::compressed::blocks::BlockCache::claim) per
    /// chunk on the chunked arm.
    ///
    /// **The unit is what bounds the buffer, and it is the source's number
    /// rather than the cut's.** `pgdt_mapping::interior::scan_partition` reads
    /// `min(piece, unit)`, so at the shipped width — where a piece *is* one
    /// unit — this is the piece exactly, and above it the buffer is capped at
    /// one unit instead of growing with the width. It does not remove the
    /// copy: the largest unit is a file-wide maximum, so a read starting on a
    /// boundary can still cross into a smaller successor unit
    /// (`docs/design/decisions.md`, "D8").
    Whole {
        /// The source's own unit, in bytes — the decoded unit on the
        /// block-decoding path, where [`BlockCache::unit`](crate::source::compressed::blocks::BlockCache::unit)
        /// is the codec's largest unit. Never zero: a source with no unit has
        /// nothing to state and stays [`PartitionRead::Chunked`].
        unit: u64,
    },
}

/// A source's answer to [`ByteRangeSource::partitions`](crate::ByteRangeSource::partitions): where to split, what
/// one partition holds resident while it reads, and what a batch held across
/// reads pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partitioning {
    boundaries: PartitionBoundaries,
    /// What concurrent readers of this source cost — the per-worker term every
    /// constructor takes, plus whatever shared pool
    /// [`Partitioning::pooling`] has stated ([`WorkerMemory`]).
    memory: WorkerMemory,
    retained: RetainedUnit,
    read: PartitionRead,
}

impl Partitioning {
    /// Split anywhere; each partition holds `partition_bytes`.
    pub fn anywhere(partition_bytes: u64) -> Self {
        Self {
            boundaries: PartitionBoundaries::Anywhere,
            memory: WorkerMemory::per_worker(partition_bytes),
            retained: RetainedUnit::default(),
            read: PartitionRead::default(),
        }
    }

    /// Split only at `offsets`, which are sorted and deduplicated here so the
    /// ascending order the accessor promises is a property of the value rather
    /// than of every source that builds one.
    pub fn at(mut offsets: Vec<u64>, partition_bytes: u64) -> Self {
        offsets.sort_unstable();
        offsets.dedup();
        Self {
            boundaries: PartitionBoundaries::At(offsets),
            memory: WorkerMemory::per_worker(partition_bytes),
            retained: RetainedUnit::default(),
            read: PartitionRead::default(),
        }
    }

    /// One partition: this range is not to be split at all.
    pub fn single(partition_bytes: u64) -> Self {
        Self::at(Vec::new(), partition_bytes)
    }

    /// State that what a retained batch pins on this source is `unit`, rather
    /// than the [`RetainedUnit::ReadChunk`] every constructor starts at.
    pub fn retaining(mut self, unit: RetainedUnit) -> Self {
        self.retained = unit;
        self
    }

    /// State how a worker should read one of these partitions, rather than the
    /// [`PartitionRead::Chunked`] every constructor starts at.
    pub fn reading(mut self, read: PartitionRead) -> Self {
        self.read = read;
        self
    }

    /// State that this source's pool retains `unit` bytes for every slot but
    /// the one a worker is filling, over a pool `depth` slots deep or one per
    /// worker — the block pool's retention list, and nothing every other
    /// source has ([`WorkerMemory`]).
    pub fn pooling(mut self, unit: u64, depth: usize) -> Self {
        self.memory = self.memory.pooling(unit, depth);
        self
    }

    /// What concurrent readers of this source cost, as a shape a budget is
    /// solved against — [`Partitioning::partition_bytes`] is its per-worker
    /// term. `worker_count` solves against it,
    /// `pgdt_query`'s `plan_partitions` builds a query's charge from it, and
    /// `pgdt_mapping::interior` reads it for the `scan arrangement` note.
    pub fn worker_memory(&self) -> WorkerMemory {
        self.memory
    }

    /// Where this source is willing to be split. Any offsets are ascending
    /// and deduplicated, [`Partitioning::at`] having made them so.
    pub fn boundaries(&self) -> &PartitionBoundaries {
        &self.boundaries
    }

    /// What one concurrent reader costs this source resident, in bytes:
    /// **the buffers the source itself allocates per partition**, and nothing
    /// else.
    ///
    /// For a plain file that is [`PLAIN_PARTITION_CHUNKS`](crate::source::local::PLAIN_PARTITION_CHUNKS) read chunks, capped
    /// at [`POOL_MAX_BYTES`](crate::source::pool::POOL_MAX_BYTES) and floored at one chunk, which is the **cut**
    /// size and not what a worker of it holds, so this over-bills that source
    /// (`KD25`). For a block-decoding compressed one it
    /// is the block unit **once** — the block being decoded is the block the
    /// reader then retains — plus the chunk buffer a straddling read is
    /// assembled into, plus the decoder's own retention. The rest of the
    /// retention list is shared and is [`WorkerMemory`]'s second term.
    ///
    /// **The decoder's own retention is a declared number rather than a
    /// guess**, the same whatever range is read and so charged once at
    /// construction: `xz_seek::Layout::decode_footprint()` for a local file,
    /// the decoder's bytes and the widest compressed window for a fetched one. The table
    /// records only the first block's dictionary per stream, so it is an
    /// estimate and not a ceiling; what cannot understate is the layout's
    /// `memlimit`, compared against each block's own declared dictionary
    /// before any backend object is built (`docs/design/decisions.md`, "D15").
    pub fn partition_bytes(&self) -> u64 {
        self.memory.bytes_per_worker()
    }

    /// What a batch this caller holds across reads pins on this source, which
    /// is what a sub-stream's span allowance is charged against — see
    /// [`RetainedUnit`], and `pgdt_query`'s `plan_partitions`, which is the
    /// one caller that reads it.
    pub fn retained_unit(&self) -> RetainedUnit {
        self.retained
    }

    /// How a worker should read one of these partitions — see
    /// [`PartitionRead`], and `pgdt_mapping::interior::run_region`, which is the
    /// one caller that reads it.
    pub fn partition_read(&self) -> PartitionRead {
        self.read
    }

    /// Whether a smaller read chunk makes one reader of this source cheaper —
    /// the question a plan note's [`pgdt_query::PlanLever::SmallerReadChunk`]
    /// answers of [`Partitioning::partition_bytes`].
    ///
    /// **Read off how a reader reads**: a [`PartitionRead::Chunked`] reader
    /// holds chunk buffers, so what it is charged is a count of chunks — a
    /// plain file's [`PLAIN_PARTITION_CHUNKS`](crate::source::local::PLAIN_PARTITION_CHUNKS), and the one chunk a compressed
    /// source's piecewise arm charges. A [`PartitionRead::Whole`] reader holds
    /// the source's own unit, a decoded block the chunk does not size, and a
    /// source charging nothing has no reader cost to lower.
    pub fn sized_by_read_chunk(&self) -> bool {
        matches!(self.read, PartitionRead::Chunked) && self.partition_bytes() > 0
    }

    /// How many partitions this advice describes, or `None` for
    /// [`PartitionBoundaries::Anywhere`], which is bounded by the caller's
    /// worker count rather than by the source.
    pub fn max_partitions(&self) -> Option<usize> {
        match &self.boundaries {
            PartitionBoundaries::Anywhere => None,
            PartitionBoundaries::At(offsets) => Some(offsets.len() + 1),
        }
    }

    /// Where a window of `want` partitions starting at `start` ends, never
    /// past `limit` — the number `pgdt_mapping::interior::run_region` hands
    /// `cut`, and **the cut size, which is not the memory
    /// charge**.
    ///
    /// [`Partitioning::partition_bytes`] is what one reader *holds*; this is
    /// what one reader *covers*, computed apart so a change made for one
    /// cannot silently move the other (`docs/design/decisions.md`, "D8"):
    ///
    /// - [`PartitionBoundaries::Anywhere`] has no seams to respect, so a
    ///   window is `want` charges wide;
    /// - [`PartitionBoundaries::At`] ends the window at the
    ///   **`want × `[`BOUNDARIED_PARTITION_UNITS`]-th boundary strictly past
    ///   `start`**, so the window holds `want` pieces of that many units each
    ///   and [`cut`]'s thinning picks every `k`-th boundary.
    ///   Fewer boundaries left is the region's tail: the window runs to
    ///   `limit`, and thinning still cannot give a piece more than `k` units.
    ///
    /// **Do not size the window at `want × unit`.** From a mid-block frontier
    /// that window contains `want` boundaries, [`cut`] is asked
    /// for `want` pieces, and the thinning drops the first — putting two
    /// blocks in the first piece. Hence "strictly past `start`" and the `× k`.
    /// Asserted by `partition/tests.rs`'s
    /// `a_boundaried_partition_spans_at_most_the_cut_width`.
    pub fn window_end(&self, start: u64, want: usize, limit: u64) -> u64 {
        let want = want.max(1);
        match &self.boundaries {
            PartitionBoundaries::Anywhere => start
                .saturating_add((want as u64).saturating_mul(self.partition_bytes()))
                .min(limit),
            PartitionBoundaries::At(offsets) => offsets
                .iter()
                .copied()
                .filter(|&at| at > start && at < limit)
                .nth(want.saturating_mul(BOUNDARIED_PARTITION_UNITS) - 1)
                .unwrap_or(limit),
        }
    }
}

/// Cut `range` into at most `want` pieces where `advice` permits, in ascending
/// order and tiling it exactly. `Anywhere` is cut evenly; `At(offsets)` at the
/// source's own boundaries, thinned to `want - 1` of them and evenly spaced
/// through the list when it offers more than the caller can use
/// (`docs/design/decisions.md`, "D15"). An empty `At` is the source declining
/// to be split, and it yields the range whole.
///
/// **Shared with the leader** (`pgdt_mapping::interior::scan_region`), which cuts an
/// open block's interior window with it: one cut rule for the two
/// arrangements (`docs/design/decisions.md`, "D52").
pub fn cut(range: Range<u64>, advice: &Partitioning, want: usize) -> Vec<Range<u64>> {
    if want <= 1 || range.start >= range.end {
        return vec![range];
    }
    let mut cuts: Vec<u64> = match advice.boundaries() {
        PartitionBoundaries::Anywhere => {
            let len = range.end - range.start;
            let want = want.min(usize::try_from(len).unwrap_or(usize::MAX)).max(1);
            (1..want as u64).map(|i| range.start + len * i / want as u64).collect()
        }
        PartitionBoundaries::At(offsets) => {
            let inside: Vec<u64> =
                offsets.iter().copied().filter(|&o| o > range.start && o < range.end).collect();
            let take = (want - 1).min(inside.len());
            // `n` offers describe `n + 1` pieces, and what is spread evenly
            // is the **pieces**, not the offers: with four offers and three
            // wanted groups the cuts fall after the second and fourth piece.
            // `ceil` rounds that the right way and keeps the picks strictly
            // increasing, so `cuts.dedup()` below is a no-op on this arm — as
            // it is on the other, once `want` is clamped to the length.
            (1..=take).map(|j| inside[(j * (inside.len() + 1)).div_ceil(take + 1) - 1]).collect()
        }
    };
    cuts.dedup();
    let mut out = Vec::with_capacity(cuts.len() + 1);
    let mut start = range.start;
    for cut in cuts {
        out.push(start..cut);
        start = cut;
    }
    out.push(start..range.end);
    out
}

#[cfg(test)]
mod tests;
