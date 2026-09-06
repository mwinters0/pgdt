//! What a bulk read will hold, answered before a byte of it is read.
//!
//! A bulk read is bounded by two numbers the caller supplies — a worker count
//! and a byte budget, [`Bulk`] — and **whichever binds first wins**. The worker
//! count is the caller's because the right one is a property of their device and
//! their build, which this crate cannot see; the byte budget is there because a
//! worker count cannot be promised to a memory cgroup, where the same count is
//! 192 MiB on a file of 24 MiB blocks and 1 GiB on a file of 128 MiB ones.
//!
//! [`RangePlan`] is the answer, and it is readable **before the read starts**:
//! how many workers the two numbers admit over *this* range, and exactly how
//! many bytes that will hold. It costs no source read and no decode — the seek
//! table already carries every block size the arithmetic needs. See
//! `docs/design/architecture.md`, "The plan: what a bulk read will hold, before
//! it reads".
//!
//! **A budget too small even for one worker clamps to one and reports.** It
//! never refuses: a caller told *"1 worker, 135.4 MiB, which exceeds the budget
//! you set"* can fall back to its own streaming decoder, which holds a
//! dictionary and a chunk and never materializes a block at all, where a caller
//! handed an error has a number it must recover from the block sizes itself.

use core::ops::Range;

use crate::decode::INPUT_CHUNK;
use crate::table::SeekTable;

/// The two numbers a bulk read is bounded by: a worker count and a byte budget.
///
/// **Neither is defaulted.** The worker count is required because the right
/// value is a property of the device and the build — the same file saturates at
/// about 8 workers in the default build and about 17 in the unsafe-free one, and
/// on tmpfs or NVMe there is no such cap at all — so a value derived from the
/// machine would be this crate making the caller's device-awareness decision
/// from underneath it. The budget is required because bytes are the unit a
/// memory cgroup is denominated in, and it is the only one of the two that can
/// be promised to one.
///
/// A caller with no memory bound passes [`u64::MAX`], which is a budget that
/// binds on nothing; a caller with no opinion on the worker count has one to
/// form, and this type is where they are told so.
///
/// ```
/// use xz_seek::Bulk;
///
/// // Eight workers, inside a 512 MiB cgroup.
/// let bulk = Bulk::new(8, 512 << 20);
/// assert_eq!(bulk.workers(), 8);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bulk {
    workers: usize,
    budget: u64,
}

impl Bulk {
    /// `workers` decoders at most, inside `budget_bytes` of memory.
    ///
    /// A worker count of zero is read as one — the floor every plan clamps to —
    /// rather than being an error, for the same reason a range past the end of
    /// the file clamps: it is a shape a caller's own arithmetic produces, and an
    /// error would only make them do that arithmetic twice.
    pub const fn new(workers: usize, budget_bytes: u64) -> Bulk {
        Bulk {
            workers,
            budget: budget_bytes,
        }
    }

    /// The worker count asked for, as it was passed.
    ///
    /// [`RangePlan::workers`] is the count a range actually admits, which is
    /// this one clamped by the budget and by how many blocks there are to work
    /// on.
    pub fn workers(self) -> usize {
        self.workers
    }

    /// The byte budget, as it was passed.
    pub fn budget(self) -> u64 {
        self.budget
    }
}

/// What a bulk read over one range will hold, and how many workers it admits.
///
/// Produced by [`Reader::plan_range`](crate::Reader::plan_range) from a
/// [`Bulk`] and a range, and carried by the read that
/// [`Reader::read_range`](crate::Reader::read_range) hands back
/// ([`RangeRead::plan`](crate::RangeRead::plan)). Every field is arithmetic over
/// the seek table: no byte of the source is read to produce one, so a caller can
/// plan a read it then decides not to make.
///
/// **The footprint is what the read holds at once, not what it allocates over
/// its life.** Per worker: one decoded block, one compressed window and the
/// decode's own input chunk. Beyond one worker there are two spare compressed
/// windows besides, so that the stage fetching them always has somewhere to put
/// the next block while every worker is busy.
///
/// ```no_run
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use std::sync::Arc;
/// use xz_seek::{Bulk, Reader};
///
/// let reader = Reader::new(Arc::new(std::fs::File::open("dump.xz")?))?;
/// let plan = reader.plan_range(0..40 << 30, Bulk::new(8, 512 << 20));
/// if !plan.fits() {
///     eprintln!(
///         "{} worker(s), {} bytes, which exceeds the budget",
///         plan.workers(),
///         plan.footprint()
///     );
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangePlan {
    /// [`Bulk::workers`], unclamped, so a caller can see that it was clamped.
    requested_workers: usize,
    /// The count the budget and the work admit. Never zero.
    workers: usize,
    /// How many blocks the range covers.
    blocks: usize,
    /// [`Bulk::budget`].
    budget: u64,
    /// The largest block the range covers, in uncompressed bytes.
    slot: u64,
    /// The largest block the range covers, in compressed bytes — header, payload,
    /// padding and check, which is what a fetch of that block asks for.
    window: u64,
    /// The compressed input buffer one decode holds, which is the input chunk
    /// capped by the largest block: a block smaller than the chunk is read whole.
    chunk: u64,
}

impl RangePlan {
    /// The plan for `range` under `bulk`, from the table alone.
    pub(crate) fn new(table: &SeekTable, range: Range<u64>, bulk: Bulk) -> RangePlan {
        let indices = table.blocks_in(range);
        let blocks = indices.len();
        let mut slot = 0u64;
        let mut window = 0u64;
        for block in &table.blocks[indices] {
            slot = slot.max(block.uncompressed_size);
            window = window.max(block.total_size());
        }
        RangePlan::derive(blocks, slot, window, bulk)
    }

    /// The arithmetic, over the four numbers it is arithmetic in.
    ///
    /// Split out from [`RangePlan::new`] so that the sizes a real file has —
    /// 24 MiB blocks against 1.29 MB windows, 128 MiB against 6.7 MB — can be
    /// pinned without a fixture that shape, which no generated corpus will ever
    /// hold.
    fn derive(blocks: usize, slot: u64, window: u64, bulk: Bulk) -> RangePlan {
        let chunk = window.min(INPUT_CHUNK as u64);
        let unit = slot.saturating_add(chunk).saturating_add(window);
        // A range covering no blocks holds nothing, so nothing binds; one
        // worker is still what is reported, because the floor is one worker and
        // not zero.
        let admitted = if unit == 0 {
            usize::MAX
        } else {
            admitted_workers(unit, window, bulk.budget)
        };
        let workers = bulk.workers.max(1).min(blocks.max(1)).min(admitted).max(1);
        RangePlan {
            requested_workers: bulk.workers,
            workers,
            blocks,
            budget: bulk.budget,
            slot,
            window,
            chunk,
        }
    }

    /// How many workers this range admits under the budget it was planned with.
    ///
    /// **Never zero, and never more than the range has blocks for.** It is
    /// [`Bulk::workers`] clamped three ways: by the budget, by the block count —
    /// a two-block range cannot occupy eight workers however much memory there
    /// is — and by the floor of one, which is what a budget too small for a
    /// single worker clamps to.
    pub fn workers(self) -> usize {
        self.workers
    }

    /// The worker count that was asked for, whether or not it was admitted.
    ///
    /// `plan.workers() < plan.requested_workers()` is the whole of "you were
    /// clamped"; comparing [`RangePlan::workers`] with [`RangePlan::blocks`]
    /// says whether it was the work that bound rather than the budget, and
    /// [`RangePlan::fits`] is false only where the budget bound below one
    /// worker.
    pub fn requested_workers(self) -> usize {
        self.requested_workers
    }

    /// How many blocks the range covers.
    ///
    /// The same number as
    /// [`SeekTable::blocks_in`](crate::SeekTable::blocks_in)'s, and the reason a
    /// caller may be given fewer workers than it asked for even under an
    /// unbounded budget.
    pub fn blocks(self) -> usize {
        self.blocks
    }

    /// The byte budget this plan was made under.
    pub fn budget(self) -> u64 {
        self.budget
    }

    /// The bytes the read holds at once, at [`RangePlan::workers`] workers.
    ///
    /// `workers × (decoded slot + input chunk + compressed window)`, plus two
    /// spare compressed windows above one worker. Zero for a range covering no
    /// blocks, which allocates nothing at all.
    ///
    /// **The decoder's dictionary is not in it, and cannot be**: a block's
    /// dictionary size is written in its own header, which the seek table does
    /// not carry and which would cost a source read per block to learn. What
    /// bounds it is [`Builder::memlimit`](crate::Builder::memlimit), which is a
    /// statement about the file and therefore per worker — the aggregate is
    /// yours to compute, and it is 8 MiB a worker on the files this crate was
    /// built for.
    pub fn footprint(self) -> u64 {
        let unit = self
            .slot
            .saturating_add(self.chunk)
            .saturating_add(self.window);
        let spares = if self.workers > 1 {
            self.window.saturating_mul(2)
        } else {
            0
        };
        (self.workers as u64)
            .saturating_mul(unit)
            .saturating_add(spares)
    }

    /// Whether [`RangePlan::footprint`] is inside [`RangePlan::budget`].
    ///
    /// **False is not an error and does not stop the read.** It is the
    /// clamp-and-report case: the plan is already down to one worker and one
    /// worker still costs more than the budget allows, which is a number the
    /// caller acts on — usually by falling back to a decoder that never
    /// materializes a whole block.
    pub fn fits(self) -> bool {
        self.footprint() <= self.budget
    }

    /// The decoded-block slot, in bytes: the largest block **this range
    /// covers**, not the largest in the file.
    ///
    /// There is one per worker. A three-block range in a file whose largest
    /// block is 128 MiB is planned — and read — against its own three blocks.
    pub fn decoded_slot_bytes(self) -> u64 {
        self.slot
    }

    /// One compressed window, in bytes: the largest block the range covers,
    /// measured as
    /// [`BlockTask::compressed_range`](crate::BlockTask::compressed_range) is —
    /// header, payload, padding and check.
    pub fn compressed_window_bytes(self) -> u64 {
        self.window
    }

    /// How many compressed windows are held at once.
    ///
    /// [`RangePlan::workers`] plus two above one worker, and exactly one at one
    /// worker, where nothing is fetching ahead. The two spares are what let a
    /// fetch overlap a decode: without them there would be nowhere to put block
    /// *k+1* while every worker is busy, so each worker's next block would begin
    /// with a cold source read.
    pub fn compressed_windows(self) -> usize {
        if self.workers > 1 {
            self.workers + 2
        } else {
            1
        }
    }

    /// The compressed input buffer one decode holds, in bytes.
    ///
    /// The third term of the per-worker cost, and the smallest: a fixed chunk,
    /// capped by the largest block the range covers, since a block shorter than
    /// the chunk is read in one.
    pub fn input_chunk_bytes(self) -> u64 {
        self.chunk
    }
}

/// The largest worker count whose footprint fits `budget`, or zero.
///
/// Piecewise because the two spare windows are the fetch stage's and one worker
/// has no fetch stage: the general term is `n × unit + 2 × window`, and the
/// one-worker term is `unit` alone.
fn admitted_workers(unit: u64, window: u64, budget: u64) -> usize {
    let with_spares = budget
        .checked_sub(window.saturating_mul(2))
        .map_or(0, |left| left / unit);
    if with_spares >= 2 {
        usize::try_from(with_spares).unwrap_or(usize::MAX)
    } else if budget >= unit {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The koji multistream file's shape: 24 MiB blocks, 1.29 MB compressed.
    const KOJI_24: (u64, u64) = (24 << 20, 1_290_000);
    /// The recompressed one's: 128 MiB blocks, 6.7 MB compressed.
    const KOJI_128: (u64, u64) = (128 << 20, 6_700_000);
    /// A budget of 512 MiB, which is the cgroup the first downstream runs in.
    const CGROUP: u64 = 512 << 20;

    fn plan_for((slot, window): (u64, u64), workers: usize, budget: u64) -> RangePlan {
        RangePlan::derive(1 << 20, slot, window, Bulk::new(workers, budget))
    }

    /// The two counts the spec derived, from the two real files' shapes.
    ///
    /// This is the arithmetic's headline claim — *"this file's 128 MiB blocks
    /// admit 3 workers under your 512 MB budget"* — and the corpus has no file
    /// of either shape, so the numbers are pinned here or nowhere.
    #[test]
    fn the_budget_admits_nineteen_workers_on_one_real_file_and_three_on_the_other() {
        let plan = plan_for(KOJI_24, 64, CGROUP);
        assert_eq!(plan.workers(), 19);
        assert!(plan.fits());
        assert!(plan.footprint() <= CGROUP);
        // And one more would not.
        assert!(
            20 * (plan.decoded_slot_bytes() + plan.input_chunk_bytes() + KOJI_24.1) + 2 * KOJI_24.1
                > CGROUP
        );

        let plan = plan_for(KOJI_128, 64, CGROUP);
        assert_eq!(plan.workers(), 3);
        assert!(plan.fits());
        assert_eq!(plan.compressed_windows(), 5);
        assert_eq!(
            plan.footprint(),
            3 * (KOJI_128.0 + (1 << 20) + KOJI_128.1) + 2 * KOJI_128.1
        );
    }

    /// The worker count binds where it is the smaller of the two.
    #[test]
    fn a_worker_count_under_what_the_budget_admits_is_what_runs() {
        let plan = plan_for(KOJI_24, 4, CGROUP);
        assert_eq!(plan.workers(), 4);
        assert_eq!(plan.requested_workers(), 4);
        assert_eq!(plan.compressed_windows(), 6);
        assert!(plan.fits());
        assert_eq!(
            plan.footprint(),
            4 * (KOJI_24.0 + (1 << 20) + KOJI_24.1) + 2 * KOJI_24.1
        );
    }

    /// A budget too small for one worker clamps to one and reports the number.
    ///
    /// The reason `fits()` exists at all: the plan is a sentence with a number
    /// in it, not a refusal.
    #[test]
    fn a_budget_too_small_for_one_worker_clamps_to_one_and_says_so() {
        let plan = plan_for(KOJI_128, 8, 100 << 20);
        assert_eq!(plan.workers(), 1);
        assert_eq!(plan.requested_workers(), 8);
        assert!(!plan.fits());
        // One worker: one slot, one chunk, one window, and no spares.
        assert_eq!(plan.compressed_windows(), 1);
        assert_eq!(plan.footprint(), KOJI_128.0 + (1 << 20) + KOJI_128.1);

        // The floor holds however small the budget is, including zero.
        let plan = plan_for(KOJI_128, 8, 0);
        assert_eq!(plan.workers(), 1);
        assert!(!plan.fits());
    }

    /// Two workers are admitted only with their spare windows paid for.
    ///
    /// The step from one worker to two costs a unit *and* two spare windows, so
    /// a budget between the two is a one-worker budget — which is the piecewise
    /// half of the arithmetic and the easiest thing to get wrong.
    #[test]
    fn the_step_to_two_workers_pays_for_the_spares() {
        let (slot, window) = KOJI_24;
        let unit = slot + (1 << 20) + window;
        let two = 2 * unit + 2 * window;

        assert_eq!(plan_for(KOJI_24, 8, two).workers(), 2);
        assert_eq!(plan_for(KOJI_24, 8, two - 1).workers(), 1);
        // ... and one worker fits comfortably inside that, so the report at
        // `two - 1` is a fit rather than the clamp-and-report case.
        assert!(plan_for(KOJI_24, 8, two - 1).fits());
    }

    /// The work binds too: a range cannot use more workers than it has blocks.
    #[test]
    fn a_range_admits_no_more_workers_than_it_covers_blocks() {
        let bulk = Bulk::new(8, u64::MAX);
        assert_eq!(
            RangePlan::derive(2, KOJI_24.0, KOJI_24.1, bulk).workers(),
            2
        );
        assert_eq!(
            RangePlan::derive(1, KOJI_24.0, KOJI_24.1, bulk).workers(),
            1
        );
        assert_eq!(
            RangePlan::derive(9, KOJI_24.0, KOJI_24.1, bulk).workers(),
            8
        );
    }

    /// A range covering no blocks holds nothing, and still reports one worker.
    ///
    /// The empty, inverted and past-the-end ranges all arrive here as zero
    /// blocks, and `RangeRead` allocates nothing for any of them — so a
    /// footprint of zero is the true number rather than a floor.
    #[test]
    fn an_empty_range_costs_nothing_and_fits_every_budget() {
        for budget in [0, 1, CGROUP, u64::MAX] {
            let plan = RangePlan::derive(0, 0, 0, Bulk::new(8, budget));
            assert_eq!(plan.blocks(), 0);
            assert_eq!(plan.workers(), 1);
            assert_eq!(plan.footprint(), 0);
            assert!(plan.fits(), "an empty range does not fit {budget}");
        }
    }

    /// A worker count of zero is one, not zero and not an error.
    #[test]
    fn a_worker_count_of_zero_is_read_as_one() {
        let plan = plan_for(KOJI_24, 0, CGROUP);
        assert_eq!(plan.workers(), 1);
        assert_eq!(plan.requested_workers(), 0);
        assert!(plan.fits());
    }

    /// A block smaller than the input chunk is not charged a whole chunk.
    ///
    /// Every fixture in the corpus is this shape — 64 KiB blocks compressing to
    /// a few kilobytes — so without the cap every plan the test suite makes
    /// would be a megabyte of imaginary input buffer per worker.
    #[test]
    fn a_block_smaller_than_the_input_chunk_is_charged_what_it_is() {
        let plan = RangePlan::derive(256, 64 << 10, 4_000, Bulk::new(4, u64::MAX));
        assert_eq!(plan.input_chunk_bytes(), 4_000);
        assert_eq!(plan.workers(), 4);
        assert_eq!(
            plan.footprint(),
            4 * ((64 << 10) + 4_000 + 4_000) + 2 * 4_000
        );
    }

    /// A hand-built table's absurd block sizes saturate rather than wrap.
    ///
    /// The table is a plain value with public fields, so nothing stops a caller
    /// handing back a block claiming `u64::MAX` uncompressed bytes; the plan is
    /// arithmetic over exactly those fields and must not overflow on one.
    #[test]
    fn absurd_block_sizes_saturate() {
        let plan = RangePlan::derive(4, u64::MAX, u64::MAX, Bulk::new(8, u64::MAX));
        assert_eq!(plan.workers(), 1);
        assert_eq!(plan.footprint(), u64::MAX);
        assert!(plan.fits());
    }
}
