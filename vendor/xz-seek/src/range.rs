//! An ordered bulk read over a caller-named uncompressed range.
//!
//! [`RangeRead`] is the handle [`Reader::read_range`](crate::Reader::read_range)
//! returns: the caller names a half-open range of the uncompressed stream, and
//! then fills its own buffer from the handle, repeatedly, until a short return
//! says the range ended. The range may span gigabytes, so nothing here assumes a
//! buffer the whole of it fits in — see `docs/design/architecture.md`, "The bulk
//! range read: ordered delivery into the caller's buffer".
//!
//! **The delivery contract does not change with the worker count.** At one
//! worker there is no pool at all: the calling thread decodes each block the
//! range covers, in order, into a slot of its own and serves the caller's fills
//! out of it. At two workers and above the same fills are served out of slots a
//! [`Pool`](crate::pool::Pool) filled — one fetch stage reading ascending, N
//! decoders drawing single blocks from a shared queue, and this module putting
//! the results back in order at the delivery head. Which of the two runs is
//! [`RangePlan::workers`], and nothing else about the read moves with it.
//!
//! **The handle carries the plan it was built under**
//! ([`RangeRead::plan`]), so what the read holds — and how many workers its
//! range admits under the caller's byte budget — is readable off the handle as
//! well as before it exists. See [`RangePlan`].
//!
//! **The slot is unconditional, even for a caller whose buffer could take a
//! whole block** — deficiency: KD8, whose detail is in
//! `docs/design/architecture.md`, "Every worker holds one slot, and the copy
//! is what buys verification". A caller that wants a block decoded straight into
//! its own memory has [`BlockTask::decode_into`](crate::BlockTask::decode_into),
//! which is that shape without this module's ordering or clamping.
//!
//! Three properties are the whole of it:
//!
//! * **A short return means the range ended, and means nothing else.** It never
//!   means a block boundary, a slot boundary or a short read from the source —
//!   the same fill-or-EOF rule [`Reader::read_at`](crate::Reader::read_at) ships,
//!   and for the same reason: a caller's loop that treats a short read as
//!   survivable would otherwise spin.
//! * **Every delivered byte was verified first.** A block is decoded whole and
//!   its check compared before any of its bytes leave the slot, so *"the bytes
//!   you have may fail their check two calls from now"* — true of the seeking
//!   path, and stated there — is not true here.
//! * **A fault leaves the bytes before it good.** The error surfaces from the
//!   call whose delivery head reaches the damaged block, naming that block's
//!   uncompressed range; everything an earlier call returned was verified and
//!   stays good. With workers running ahead the fault may be *found* several
//!   blocks early, and it waits its turn. A call that returns an error ends the
//!   range: the handle is spent, [`RangeRead::failed`] says so, and every later
//!   call returns zero. Recovery is a new range, not another call — unlike
//!   [`Reader::read_at`](crate::Reader::read_at), which survives its errors
//!   because a reader is long-lived and addressed by offset.

use core::ops::Range;
use std::sync::Arc;

use crate::error::Result;
use crate::plan::RangePlan;
use crate::pool::Pool;
use crate::source::CompressedSource;
use crate::task::BlockTask;

/// An ordered bulk read in flight.
///
/// Built by [`Reader::read_range`](crate::Reader::read_range), which is where
/// the bound on `S` and the reasoning for it live. The handle owns its own
/// clone of the source and its own copy of the work, so it borrows the reader
/// for no longer than the call that made it and several may be open at once.
///
/// ```no_run
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use std::sync::Arc;
/// use xz_seek::{Bulk, Reader};
///
/// let reader = Reader::new(Arc::new(std::fs::File::open("dump.xz")?))?;
/// let bulk = Bulk::new(8, 512 << 20);
/// let mut range = reader.read_range(1_000_000..9_000_000, bulk)?;
///
/// let mut buf = vec![0u8; 64 << 10];
/// loop {
///     let n = range.read(&mut buf)?;
///     if n == 0 {
///         break;
///     }
///     // ... consume `buf[..n]` ...
///     if n < buf.len() {
///         break; // a short return means the range ended
///     }
/// }
/// # Ok(())
/// # }
/// ```
pub struct RangeRead<S: CompressedSource> {
    source: S,
    /// The blocks the range covers, in order — the work queue the pool draws
    /// from, shared with its fetch stage rather than copied for it.
    tasks: Arc<[BlockTask]>,
    /// The next block to bring to the delivery head. Past the end once the
    /// range's last block is in the slot.
    next: usize,
    /// The next uncompressed offset to deliver.
    pos: u64,
    /// One past the range's last byte, already clamped to the file's plaintext.
    end: u64,
    /// The decoded block. Allocated at the first block actually decoded, so an
    /// empty range and a range past the end of the file allocate nothing.
    slot: Vec<u8>,
    /// What the slot is sized to: the largest block *this range covers*, not the
    /// largest in the file.
    slot_len: usize,
    /// The uncompressed range the slot currently holds. Empty before the first
    /// decode, and an empty range never contains anything, so no sentinel is
    /// needed.
    held: Range<u64>,
    /// What this read will hold, derived before it started. The slot above is
    /// its `decoded_slot_bytes`, and the rest of it is the pool's.
    plan: RangePlan,
    /// The threads, above one worker. `None` is the one-worker path, where the
    /// calling thread fetches and decodes at the delivery head — and also what
    /// a machine that would not start a thread falls back to.
    pool: Option<Pool>,
    /// Set once a call returned an error, which ends the range.
    ///
    /// Retrying is not what this replaces: a damaged block stays damaged, and
    /// above one worker the fetch stage has already stopped at the fault. What
    /// the flag buys is the *answer* — without it the pooled path's second call
    /// waits on threads that have exited and reports `Pool::lost`, an `Io`
    /// error blaming the caller's source for a damaged file. The bar that
    /// the contract may not move with the worker count is why the serial path
    /// adopts it too. See `docs/design/architecture.md`, "The bulk range read".
    ///
    /// [`RangeRead::failed`] is this field, published: a spent handle and a
    /// delivered one both read zero, so nothing else distinguishes them.
    failed: bool,
}

impl<S: CompressedSource> RangeRead<S> {
    /// The handle a reader hands out, over the blocks its range covers.
    ///
    /// `start..requested_end` is what the caller asked for; the delivered end is
    /// clamped to the last covered block's own end, which is the file's
    /// plaintext end whenever the range runs past it. Both clamps are
    /// `SeekTable::blocks_in`'s, seen from the byte side.
    pub(crate) fn new(
        source: S,
        tasks: Vec<BlockTask>,
        plan: RangePlan,
        start: u64,
        requested_end: u64,
    ) -> RangeRead<S>
    where
        S: Clone + Send + 'static,
    {
        let end = match tasks.last() {
            Some(last) => requested_end.min(last.uncompressed_range().end).max(start),
            // No block covers the range: an empty range, an inverted one, or one
            // wholly past the end of the file. All three deliver nothing.
            None => start,
        };
        // The plan's own decoded-block slot, which is the largest block this
        // range covers. A block larger than this platform's address space cannot
        // be decoded into memory at all, and saying so is `BufferTooSmall`'s job
        // rather than an allocation that aborts: a zero-length slot reaches
        // `decode_into`, which names both lengths.
        let slot_len = usize::try_from(plan.decoded_slot_bytes()).unwrap_or(0);
        let tasks: Arc<[BlockTask]> = tasks.into();
        // **The plan decides which path runs**, and it has already clamped the
        // count by the blocks the range covers — so more than one worker means
        // more than one block, and the pool is never started for work one
        // thread would finish first. The source clone is the fetch stage's; the
        // handle keeps its own for the one-worker path and for the fallback.
        let pool = if plan.workers() > 1 {
            Pool::spawn(source.clone(), Arc::clone(&tasks), plan.workers(), slot_len)
        } else {
            None
        };
        RangeRead {
            source,
            tasks,
            next: 0,
            pos: start,
            end,
            slot: Vec::new(),
            slot_len,
            held: 0..0,
            plan,
            pool,
            failed: false,
        }
    }

    /// What this read will hold, and how many workers its range admits.
    ///
    /// The same [`RangePlan`]
    /// [`Reader::plan_range`](crate::Reader::plan_range) answers for this range
    /// under the same [`Bulk`](crate::Bulk) — carried here so that a caller who
    /// went straight to the read still has the number, and so that a report of
    /// what a read is costing comes off the read itself rather than off a
    /// separate query that could have been asked with different parameters.
    pub fn plan(&self) -> RangePlan {
        self.plan
    }

    /// Whether a fault ended this range.
    ///
    /// **This is the only thing that tells a spent handle from a delivered
    /// one.** Both answer `0` to every further [`RangeRead::read`], because an
    /// error ends the range and so does running out of it, so a caller holding a
    /// handle it did not read itself — one passed between frames, or parked in a
    /// struct — cannot otherwise say which happened. The caller that saw the
    /// `Err` already knows.
    ///
    /// `true` from the moment [`RangeRead::read`] returns an error, and never
    /// after any other outcome: a fully delivered range, an empty one and a
    /// range past the end of the file all answer `false`. It reports the fault
    /// rather than re-raising it — the error was raised once, at the position it
    /// happened, and recovering means opening a new range.
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// Fill `buf` with the range's next bytes.
    ///
    /// **Fills `buf` completely.** A return shorter than `buf.len()` means the
    /// range ended, and means nothing else — never a block boundary, a partial
    /// decode or a short read from the source. Once the range is delivered every
    /// further call returns zero.
    ///
    /// **Every byte is copied once on its way to `buf`**, out of the slot the
    /// block was decoded and verified in — including when `buf` is large enough
    /// to have held the block itself, which this method does not check for
    /// (deficiency: KD8). [`Reader::read_range`](crate::Reader::read_range) says
    /// what to reach for instead where that copy matters.
    ///
    /// # Errors
    ///
    /// Whatever decoding the block at the delivery head raises, including
    /// [`Error::BlockCheckFailed`](crate::Error::BlockCheckFailed) — which
    /// arrives *here*, before any of that block's bytes are returned, because a
    /// block is decoded whole and checked before it is delivered.
    ///
    /// The error names the damaged block by its uncompressed range, so a caller
    /// resuming knows exactly which bytes it has: everything an earlier call
    /// returned. Bytes this call wrote into `buf` before the fault are not
    /// reported and must not be used — a short count would be indistinguishable
    /// from the range ending, which is the one thing a short return is allowed
    /// to mean.
    ///
    /// **An error ends the range.** The handle is spent afterwards and every
    /// further call returns zero; the fault is not raised a second time, and
    /// [`RangeRead::failed`] is what says a zero came from that rather than from
    /// the range ending. Open a new range if you want to try the same bytes
    /// again.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match self.deliver(buf) {
            Ok(n) => Ok(n),
            Err(e) => {
                self.failed = true;
                Err(e)
            }
        }
    }

    /// [`RangeRead::read`], before the error is made terminal.
    fn deliver(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.failed {
            return Ok(0);
        }
        let mut done = 0usize;
        while done < buf.len() && self.pos < self.end {
            if !(self.held.start <= self.pos && self.pos < self.held.end) && !self.fill_slot()? {
                break;
            }
            // The slot holds `pos`, and the range ends no later than the file's
            // plaintext does, so there is at least one byte to copy.
            let from = (self.pos - self.held.start) as usize;
            let available = (self.held.end.min(self.end) - self.pos) as usize;
            let n = available.min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&self.slot[from..from + n]);
            done += n;
            self.pos += n as u64;
        }
        Ok(done)
    }

    /// Bring the block covering the delivery head into the slot.
    ///
    /// Returns `false` where no block covers it, which the range's own clamp
    /// makes unreachable for a walked table and which a hand-built one could
    /// still produce.
    fn fill_slot(&mut self) -> Result<bool> {
        if self.pool.is_some() {
            self.take_from_pool()
        } else {
            self.decode_here()
        }
    }

    /// The one-worker path: fetch and decode at the delivery head.
    fn decode_here(&mut self) -> Result<bool> {
        // A block that ends at or before the delivery head has nothing to give.
        // Only a zero-length block can be one — `SeekTable::validate` admits
        // those and `blocks_in` includes them — and skipping is what keeps one
        // from stalling the loop.
        while self
            .tasks
            .get(self.next)
            .is_some_and(|t| t.uncompressed_range().end <= self.pos)
        {
            self.next += 1;
        }
        let Some(task) = self.tasks.get(self.next).copied() else {
            return Ok(false);
        };
        if self.slot.len() < self.slot_len {
            // `vec![0u8; n]` reaches `alloc_zeroed`; growing an empty `Vec` with
            // `resize` writes the zeros a byte at a time in an unoptimized
            // build, which is the difference `reader::DISCARD_CHUNK` records.
            self.slot = vec![0u8; self.slot_len];
        }
        // Decoded into the slot even where `buf` could have taken the block
        // whole — deficiency: KD8, whose detail is in
        // `docs/design/architecture.md`.

        task.decode_into(&self.source, &mut self.slot)?;
        self.held = task.uncompressed_range();
        self.next += 1;
        Ok(true)
    }

    /// The pooled path: take the next block **in order** from the workers.
    ///
    /// **The spent slot goes back to the free list before the wait, not after.**
    /// The head has no further use for its contents once it is about to replace
    /// them, and with exactly N slots a head that held one across the wait would
    /// leave the pool N − 1 to circulate for most of the read — N workers
    /// delivering N − 1 workers' throughput.
    ///
    /// **The order is load-bearing and its soundness is not local.** For the
    /// length of the wait `self.slot` is empty while `self.held` still names the
    /// block just delivered, and that stale `held` is never read only because
    /// [`RangeRead::deliver`] calls `fill_slot` **only** when `pos` is already
    /// outside `held`, and because [`RangeRead::read`] sets `failed` on every
    /// error, after which `deliver` returns `Ok(0)` without touching either
    /// field. A change to either makes this order unsound.
    ///
    /// **Every block the fetcher issued is taken, including one the delivery
    /// head has already run past.** A zero-length block gives nothing, but its
    /// result still has to be consumed and its slot still has to go back to the
    /// free list, or the sequence the head is waiting on never advances. That is
    /// the one place the pooled path cannot simply skip where the serial one
    /// does.
    fn take_from_pool(&mut self) -> Result<bool> {
        while self.next < self.tasks.len() {
            let seq = self.next;
            let task = self.tasks[seq];
            // The handle's own first buffer is not one of the pool's N and is
            // dropped rather than recycled: handing it to the free list would
            // grow the pool by one and make the plan's footprint understate.
            // `seq` is the block count taken so far, so it is what tells the
            // two apart.
            let spent = core::mem::take(&mut self.slot);
            if seq > 0 {
                self.pool_mut().recycle(spent);
            }
            let decoded = self.pool_mut().take(seq, &task)?;
            self.next += 1;
            self.slot = decoded;
            if task.uncompressed_range().end <= self.pos {
                continue;
            }
            self.held = task.uncompressed_range();
            return Ok(true);
        }
        Ok(false)
    }

    /// The pool, on the path that only runs with one.
    fn pool_mut(&mut self) -> &mut Pool {
        self.pool
            .as_mut()
            .expect("the pooled path runs only with a pool")
    }
}

/// The position and the work, never the slot: a slot is routinely megabytes.
impl<S: CompressedSource> std::fmt::Debug for RangeRead<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RangeRead")
            .field("position", &self.pos)
            .field("end", &self.end)
            .field("blocks", &self.tasks.len())
            .field("decoded", &self.next)
            .field("slot_len", &self.slot_len)
            .field("workers", &self.plan.workers())
            .field(
                "threads",
                &self.pool.as_ref().map_or(0, |pool| pool.workers()),
            )
            .field("failed", &self.failed)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::Builder;
    use std::sync::Arc;

    /// A handle carries itself to a thread, and owns everything it needs.
    ///
    /// `Send` and `'static` are what the pool needs of the handle it builds
    /// threads underneath, and both are inferred from the fields — so nothing in
    /// the type's own text would notice a field arriving that broke either. The
    /// assertion is what notices, in the shape `src/window.rs` and
    /// `src/backend.rs` already set.
    #[test]
    fn a_range_read_is_send_and_owns_nothing_borrowed() {
        fn assert_send_static<T: Send + 'static>() {}
        assert_send_static::<RangeRead<Arc<std::fs::File>>>();
        assert_send_static::<RangeRead<crate::Window<Vec<u8>>>>();
    }

    /// The plan parameters these tests read under.
    ///
    /// Neither number binds — four workers over 256 blocks with no byte budget —
    /// so what is asserted here is the delivery and never the clamp. The
    /// arithmetic that clamps is `src/plan.rs`, which needs no fixture.
    fn bulk() -> crate::Bulk {
        crate::Bulk::new(4, u64::MAX)
    }

    fn corpus_reader(name: &str) -> crate::Reader<Arc<std::fs::File>> {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let file = std::fs::File::open(dir.join(name)).expect("the fixture opens");
        Builder::new()
            .open(Arc::new(file))
            .expect("the fixture walks")
    }

    /// The three edges of an empty delivery, and none of them allocates.
    ///
    /// `blocks_in`'s empty answer read from the byte side: an empty range, an
    /// inverted one, and one wholly past the end of the file all deliver zero
    /// bytes with no error and no slot. A caller's span arithmetic produces all
    /// three, which is why none of them is an error.
    #[test]
    fn an_empty_delivery_reads_zero_and_allocates_nothing() {
        let reader = corpus_reader("many-blocks.xz");
        let size = reader.index().uncompressed_size();
        let mut buf = [0u8; 64];

        #[allow(clippy::reversed_empty_ranges)]
        for range in [0..0, size / 2..size / 2, 900..100, size..size + 4_096] {
            let mut read = reader
                .read_range(range.clone(), bulk())
                .expect("a range opens");
            assert_eq!(read.read(&mut buf).unwrap(), 0, "{range:?}");
            assert_eq!(read.read(&mut buf).unwrap(), 0, "{range:?}, twice");
            assert!(read.slot.is_empty(), "{range:?} allocated a slot");
            assert_eq!(read.slot_len, 0, "{range:?} sized a slot");
            assert!(!read.failed(), "{range:?} reported a fault it never had");
        }
    }

    /// The slot is one block, sized to the largest block the *range* covers, and
    /// it is decoded once however many fills it serves.
    ///
    /// The count is what makes "ordered delivery costs one copy per byte and one
    /// decode per block" checkable: a fill smaller than a block must not
    /// re-decode, and a fill larger than one must not skip. `next` counts blocks
    /// brought to the delivery head, which is the same number on either path —
    /// the two-block range here runs on the pool and the one-block range does
    /// not.
    #[test]
    fn a_block_is_decoded_once_however_many_fills_it_serves() {
        let reader = corpus_reader("many-blocks.xz");
        let block = reader.index().blocks[0].uncompressed_size;
        assert_eq!(reader.index().max_block_uncompressed(), block);

        // Two blocks, delivered a sixteenth of a block at a time.
        let mut read = reader
            .read_range(0..2 * block, bulk())
            .expect("a range opens");
        assert_eq!(read.slot_len, block as usize);
        let mut buf = vec![0u8; block as usize / 16];
        let mut fills = 0;
        while read.read(&mut buf).expect("the range delivers") == buf.len() {
            fills += 1;
            assert!(fills <= 32, "the range delivered more than it holds");
        }
        assert_eq!(fills, 32);
        assert_eq!(read.next, 2, "two blocks, decoded once each");
        assert!(
            !read.failed(),
            "a range that delivered everything reported a fault"
        );

        // And a range inside one block sizes its slot to that block alone.
        let one = reader
            .read_range(block + 1..block + 2, bulk())
            .expect("opens");
        assert_eq!(one.slot_len, block as usize);
        assert_eq!(one.tasks.len(), 1);
    }

    /// The handle's plan is the reader's own answer for that range, and the slot
    /// it allocates is the plan's.
    ///
    /// The two could drift — the handle sizes its slot from the plan, and the
    /// plan is derived from the table — and the failure would be silent: a read
    /// that reports one footprint and holds another is exactly the number a
    /// caller promised its cgroup.
    #[test]
    fn the_handle_carries_the_plan_the_reader_answers_for_that_range() {
        let reader = corpus_reader("many-blocks.xz");
        let block = reader.index().blocks[0].uncompressed_size;

        let range = 0..4 * block;
        let read = reader.read_range(range.clone(), bulk()).expect("opens");
        assert_eq!(read.plan(), reader.plan_range(range, bulk()));
        assert_eq!(read.plan().blocks(), 4);
        assert_eq!(read.plan().workers(), 4);
        assert_eq!(read.plan().decoded_slot_bytes(), read.slot_len as u64);

        // And a range covering two blocks admits two workers, whatever was asked
        // for — the work binds where the budget does not.
        let read = reader.read_range(0..block + 1, bulk()).expect("opens");
        assert_eq!(read.plan().workers(), 2);
        assert_eq!(read.plan().requested_workers(), 4);
    }

    /// The delivery head leaves every slot to the pool while it waits.
    ///
    /// **This is what a worker count buys, asserted as state rather than as a
    /// rate.** The pool circulates exactly N slots, so a head that held its
    /// spent one across the wait would leave N − 1 for the workers and N workers
    /// would deliver N − 1 workers' throughput — which every byte property the
    /// pool passes is green under, and always would be. `slots_while_waiting`
    /// is the minimum over every wait of what the head left behind, so the
    /// answer is N or it is not, with no scheduling in it.
    ///
    /// The fixture is the 256-block one: the stash is bounded by the slot count,
    /// so over that many blocks the head goes to the decoders hundreds of times
    /// and the minimum is taken over all of them.
    #[test]
    fn the_head_leaves_every_slot_to_the_pool_while_it_waits() {
        let reader = corpus_reader("bulk-blocks.xz");
        let size = reader.index().uncompressed_size();

        for workers in [2usize, 3, 8] {
            let mut read = reader
                .read_range(0..size, crate::Bulk::new(workers, u64::MAX))
                .expect("a range opens");
            let mut buf = vec![0u8; 4_093];
            while read.read(&mut buf).expect("the range delivers") == buf.len() {}
            let pool = read.pool.as_ref().expect("this range runs on the pool");
            assert_eq!(pool.workers(), workers, "at {workers} workers");
            assert_eq!(
                pool.slots_while_waiting(),
                Some(workers),
                "at {workers} workers the head kept a slot back while it waited, so \
                 the pool overlapped one fewer decode than it has workers"
            );
        }
    }

    /// The plan's worker count is what decides whether threads are started, and
    /// how many.
    ///
    /// It is the one thing about the pool that is not visible from the bytes:
    /// `tests/range.rs` asserts that every count delivers the same answer, which
    /// is exactly what would still pass if the pool were never built at all. The
    /// handle's private fields are the only place the two can be compared.
    ///
    /// The lower boundary is the interesting one. A count of one and a range of
    /// one block must both take the path with no threads in it, and the
    /// clamp that makes it automatic is `RangePlan`'s own cap by the block count
    /// rather than a second condition here.
    #[test]
    fn the_plan_s_worker_count_decides_whether_threads_are_started() {
        let reader = corpus_reader("many-blocks.xz");
        let block = reader.index().blocks[0].uncompressed_size;

        for (range, workers, threads) in [
            // No blocks, so nothing to thread.
            (0..0, 8, 0),
            // One block, however many workers were asked for.
            (0..block, 8, 0),
            // One worker, however many blocks there are.
            (0..16 * block, 1, 0),
            // Above one of each, the pool, at the count the plan derived.
            (0..2 * block, 8, 2),
            (0..16 * block, 3, 3),
            (0..16 * block, 8, 8),
        ] {
            let read = reader
                .read_range(range.clone(), crate::Bulk::new(workers, u64::MAX))
                .expect("a range opens");
            assert_eq!(
                read.pool.as_ref().map_or(0, |pool| pool.workers()),
                threads,
                "{range:?} at {workers} workers"
            );
            assert_eq!(
                read.plan().workers(),
                threads.max(1),
                "{range:?} at {workers} workers: the plan and the pool disagree"
            );
        }
    }
}
