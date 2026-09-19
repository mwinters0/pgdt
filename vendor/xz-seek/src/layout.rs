//! What is known about an `.xz` file without reading one of its bytes.
//!
//! [`Layout`] is the seek table, the memory limit, the verification state and
//! the backend, and every query those four answer between them: the file's
//! shape, the work one block is, what a decode holds, and what a range would
//! cost. It owns no source and borrows nothing, so it is the handle a caller
//! whose transport is its own business keeps — [`Layout::block_task`] hands out
//! the work and [`crate::BlockTask::decode_into`] takes the bytes back.
//!
//! [`crate::Reader`] is this handle plus a source plus one live decode, and
//! forwards all seven of those queries unchanged, so a caller who has a file
//! never has to hold both. [`Layout::verify`] and [`Layout::backend`] are not
//! forwarded and are read through [`crate::Reader::layout`] — `D70`.

use std::sync::Arc;

use crate::backend::Backend;
use crate::error::{Error, Result};
use crate::plan::{Bulk, RangePlan};
use crate::reader::Verify;
use crate::table::{Check, SeekTable};
use crate::task::BlockTask;

/// An `.xz` file's shape and decode configuration, with no source attached.
///
/// Built by [`Builder::layout`](crate::Builder::layout) from a table the caller
/// already holds and the compressed file's size, or taken from a reader that
/// walked one ([`Reader::layout`](crate::Reader::layout)). Cloning it is an
/// `Arc` bump and three words: the table exists once however many handles name
/// it, exactly as [`Reader::index_shared`](crate::Reader::index_shared)
/// promises.
///
/// **Nothing here reads a byte or can fail for lack of one.** Every answer is
/// arithmetic over the table, which is what lets a caller ask *what would this
/// cost* before deciding to fetch anything at all.
#[derive(Debug, Clone)]
pub struct Layout {
    /// Shared rather than owned, so a caller keeping the table beside the
    /// layout holds one table rather than two.
    pub(crate) table: Arc<SeekTable>,
    pub(crate) memlimit: u64,
    pub(crate) verify: Verify,
    pub(crate) backend: Backend,
}

impl Layout {
    /// The handle over `table`, with the configuration a [`Builder`] carries.
    ///
    /// [`Builder`] is the public route in, because the backend has to be
    /// checked against the build and the table against the file's size before
    /// either is worth anything.
    ///
    /// [`Builder`]: crate::Builder
    pub(crate) fn new(
        table: Arc<SeekTable>,
        memlimit: u64,
        verify: Verify,
        backend: Backend,
    ) -> Layout {
        Layout {
            table,
            memlimit,
            verify,
            backend,
        }
    }

    /// The seek table this layout describes the file by.
    ///
    /// The table is a plain value with public documented fields, so this is
    /// also the extraction half of persisting it: serialize what comes back —
    /// the `serde` feature derives `Serialize` and `Deserialize` on every type
    /// in it — and hand it to [`Builder::layout`](crate::Builder::layout) or
    /// [`Builder::open_with_table`](crate::Builder::open_with_table) next time.
    ///
    /// The shape and cost queries live on [`SeekTable`] and nothing forwards
    /// them: `layout.index().uncompressed_size()` is one keystroke more than a
    /// forwarding method and one fewer thing to keep in step. That is also why
    /// [`SeekTable::seek_cost`] is unambiguous — the table has no live position
    /// to confuse a cold-start cost with.
    pub fn index(&self) -> &SeekTable {
        &self.table
    }

    /// The seek table, as a handle that aliases it.
    ///
    /// **Not a clone of the table**: the `Arc` returned is the one this layout
    /// holds, so `&*layout.index_shared()` and [`Layout::index`] are the same
    /// address and the table exists once however many handles are out. Cloning
    /// out of [`Layout::index`] answers the same queries at the cost of a second
    /// table for the life of the process, 80 bytes a stream and 32 a block.
    ///
    /// [`Builder::layout`](crate::Builder::layout) and
    /// [`Builder::open_with_table`](crate::Builder::open_with_table) take an
    /// `Arc<SeekTable>` back the same way, so a table read from a caller's own
    /// storage can be shared from the start.
    pub fn index_shared(&self) -> Arc<SeekTable> {
        Arc::clone(&self.table)
    }

    /// How much verification a decode through this layout does.
    ///
    /// What [`Builder::verify`](crate::Builder::verify) was given, or
    /// [`Verify::Full`] where it was not — so a caller that took the defaults,
    /// through [`Reader::new`](crate::Reader::new) or a bare
    /// [`Builder::new`](crate::Builder::new), can **assert** what it got rather
    /// than comment it. Every [`BlockTask`] this layout hands out carries this
    /// value, so it is also what a block decoded off-thread verifies under.
    ///
    /// The memory limit is not readable beside it: `D70`.
    pub fn verify(&self) -> Verify {
        self.verify
    }

    /// Which implementation decodes a block's payload here.
    ///
    /// Resolved against the build: a [`Backend`] this build does not carry is
    /// [`Error::BackendUnavailable`] from the constructor, so a layout that
    /// exists names one that is compiled. Together with
    /// [`check_implementation`](crate::check_implementation) — the other half
    /// of the crate's build-resolved configuration — it is what a log, a
    /// measurement or a bug report records the decode's provenance from.
    ///
    /// The memory limit is not readable beside it: `D70`.
    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// The work of decoding one block, as a value a caller can schedule.
    ///
    /// `index` is an index into [`SeekTable::blocks`](crate::SeekTable::blocks);
    /// [`SeekTable::blocks_in`](crate::SeekTable::blocks_in) is what turns an
    /// uncompressed range into the indices covering it. The task carries this
    /// layout's memory limit, verification state and backend, and the block's
    /// *resolved* check — so a block decoded through it decodes exactly as
    /// [`Reader::read_at`](crate::Reader::read_at) would decode it.
    ///
    /// **`None` past the last block.** The one other way it is `None` is a table
    /// that places a block outside every stream, so that no check can be
    /// resolved for it; [`SeekTable::validate`](crate::SeekTable) refuses such a
    /// table and a walk cannot produce one, so a layout that exists has no such
    /// block.
    ///
    /// **This layout is not borrowed by the task.** `&self` ends with the call:
    /// a [`BlockTask`] is `Copy` and owns everything it needs, so tasks may be
    /// collected, sent to threads, and outlive the layout that named them.
    pub fn block_task(&self, index: usize) -> Option<BlockTask> {
        let block = *self.table.blocks.get(index)?;
        let check = self.check_of(index).ok()?;
        Some(BlockTask::new(
            block,
            check,
            self.verify,
            self.memlimit,
            self.backend,
        ))
    }

    /// What one decode of this file holds besides the bytes it produces, in
    /// bytes.
    ///
    /// The charge a caller sizing a worker pool against a memory budget divides
    /// by: everything one [`BlockTask::decode_into`](crate::BlockTask::decode_into)
    /// — or one live [`Reader::read_at`](crate::Reader::read_at) — retains
    /// beyond the output buffer the caller owns. Three terms, and only the first
    /// varies per block:
    ///
    /// | Held | What it is |
    /// |---|---|
    /// | the LZMA2 dictionary | the largest any stream's first block header declares |
    /// | the compressed input chunk | 1 MiB, capped by the largest block's whole extent |
    /// | the backend's own decoder state | a constant, per backend |
    ///
    /// **It is [`Layout::decoder_bytes`] plus [`Layout::input_chunk_bytes`]**,
    /// and a caller whose source lends its bytes wants the first of those alone:
    /// the chunk is never allocated over such a source, so this sum over-charges
    /// it by exactly that term.
    ///
    /// It costs **no source read and no decode**: the seek table carries the
    /// first two and this layout is the third. **The dictionary is the whole
    /// file's**, so *this* number does not vary with what is read and a caller
    /// asks once, at construction, and stores it.
    ///
    /// That is also why it is not the charge for one *range*.
    /// [`Layout::plan_range`] takes the same three terms over the streams a
    /// range touches, so its
    /// [`RangePlan::decoder_bytes`](crate::RangePlan::decoder_bytes) is the
    /// smaller wherever a range misses the largest-dictionary stream — or a
    /// stream whose first block header did not parse, which this method charges
    /// `memlimit` for across the whole file. A caller sizing a pool for a read
    /// it is about to make asks there and not here.
    ///
    /// **It is a footprint, not a dictionary, and `memlimit` is neither.**
    /// [`Builder::memlimit`](crate::Builder::memlimit) is a refusal threshold
    /// about the *file* — *this file declares a dictionary larger than you
    /// allowed* — and stays in dictionary bytes; this is a charge for *our own*
    /// decode. The dictionary term is separately readable as
    /// [`StreamEntry::first_block_dict_size`](crate::StreamEntry::first_block_dict_size),
    /// which is a public field of the table.
    ///
    /// # What it is not
    ///
    /// **Not a sound ceiling.** Within one stream every block declares the same
    /// filter chain unless the producer deliberately changed it, which `xz`
    /// does on demand (`docs/design/xz-invariants.md`, `I23`), so this is exact
    /// for every file written without that and **understates** for the rest.
    /// The backstop is unconditional: `memlimit` is compared against each
    /// block's own declared dictionary before any backend object is built, so
    /// an understating charge surfaces as a clean
    /// [`Error::MemoryLimitExceeded`] and never as an overrun. A caller that
    /// needs a number that cannot understate has one already —
    /// `memlimit` itself, which is the ceiling this can never exceed.
    ///
    /// Deficiency register: `deficiency: KD10` — that understatement is
    /// [`RangePlan::footprint`](crate::RangePlan::footprint)'s too, both
    /// reading the same per-stream field, so a caller dividing a budget by it
    /// admits more workers than its memory allows, and
    /// [`RangePlan::fits`](crate::RangePlan::fits) can call a range affordable
    /// whose decodes then exceed the budget. **(a) deliberate tradeoff**: the
    /// exact answer is a block-header read per block, which gives up *a plan
    /// makes no source read*, and raising `memlimit` does not restore the
    /// number — only the backstop above keeps the understatement from being an
    /// overrun. Promoted by a real file of that shape, which `xz` writes given
    /// two `--filtersN=` chains (`I23`).
    ///
    /// **A stream whose first block header did not parse is charged
    /// `memlimit`**, since nothing is known about the rest of it and
    /// over-charging is the cheap direction.
    ///
    /// **Never zero**, so it is safe as a divisor: a file with no blocks still
    /// reports the backend's own state.
    ///
    /// ```no_run
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use xz_seek::{Bulk, Reader};
    ///
    /// let reader = Reader::new(std::fs::File::open("dump.xz")?)?;
    /// let layout = reader.layout();
    /// let budget = 1 << 30;
    ///
    /// // What one decode of this file holds, whichever range it is over.
    /// let per_decode = layout.decode_footprint();
    ///
    /// // How many decodes that budget admits over the range actually being
    /// // read is `plan_range`'s answer rather than a division: the plan charges
    /// // that range's own dictionary, and the buffers a bulk read holds beside
    /// // the decoders.
    /// let whole = 0..layout.index().uncompressed_size();
    /// let workers = layout.plan_range(whole, Bulk::new(16, budget)).workers();
    /// # let _ = (per_decode, workers);
    /// # Ok(())
    /// # }
    /// ```
    pub fn decode_footprint(&self) -> u64 {
        self.table.decode_footprint(self.backend, self.memlimit)
    }

    /// What one decode of this file holds when the source lends its bytes, in
    /// bytes: the LZMA2 dictionary plus the backend's own decoder state.
    ///
    /// [`Layout::decode_footprint`] less
    /// [`Layout::input_chunk_bytes`] — the charge for a decode that never
    /// allocates an input buffer, because a source that lends is read through
    /// [`CompressedSource::slice_at`](crate::CompressedSource::slice_at) and the
    /// chunk is never built. A caller handing
    /// [`BlockTask::decode_into`](crate::BlockTask::decode_into) a
    /// [`Window`](crate::Window) cut to that task's own
    /// [`compressed_range`](crate::BlockTask::compressed_range) is exactly that
    /// caller — as is one driving a [`BlockRead`](crate::BlockRead) over such a
    /// window — and divides a budget by this rather than by the whole sum.
    ///
    /// **It is stated rather than left to the subtraction** so that a caller is
    /// not the one deciding, silently, which side a fourth term of the footprint
    /// would fall on.
    ///
    /// **The dictionary is the whole file's**, so this does not vary with what
    /// is read.
    /// [`RangePlan::decoder_bytes`](crate::RangePlan::decoder_bytes) is the same
    /// quantity over the streams one *range* touches, and is the smaller
    /// wherever that range misses the largest-dictionary stream; a caller sizing
    /// a pool for a read it is about to make asks there and not here.
    ///
    /// **Not a sound ceiling**, on the same terms as the sum that contains it: a
    /// stream whose *later* block declares a larger dictionary than its first is
    /// understated — `KD10`, stated at [`Layout::decode_footprint`].
    /// `memlimit` remains the backstop, compared against each block's own
    /// declared dictionary before any backend object is built.
    ///
    /// **Never zero**, so it is safe as a divisor: the backend term is
    /// unconditional, so even a file with no blocks reports it.
    ///
    /// ```no_run
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use xz_seek::Reader;
    ///
    /// let reader = Reader::new(std::fs::File::open("dump.xz")?)?;
    /// let layout = reader.layout();
    /// assert_eq!(
    ///     layout.decode_footprint(),
    ///     layout.decoder_bytes() + layout.input_chunk_bytes(),
    /// );
    /// # Ok(())
    /// # }
    /// ```
    pub fn decoder_bytes(&self) -> u64 {
        self.table.decoder_bytes(self.backend, self.memlimit)
    }

    /// The compressed input buffer a positioned read of this file may hold, in
    /// bytes: the second term of [`Layout::decode_footprint`], on its own.
    ///
    /// A fixed 1 MiB chunk, capped by the largest block's whole compressed
    /// extent, since a block shorter than the chunk is read in one. It is here
    /// because the memory is: [`Reader::read_at`](crate::Reader::read_at) pulls
    /// a block through this buffer, and so does a
    /// [`BlockTask::decode_into`](crate::BlockTask::decode_into) over a source
    /// that does not lend.
    ///
    /// **It is an upper bound, not an allocation that always happens.** A source
    /// that lends its bytes — a [`Window`](crate::Window), a `&[u8]`, a caller's
    /// memory map — is read through
    /// [`CompressedSource::slice_at`](crate::CompressedSource::slice_at) and the
    /// decode holds no chunk at all, so over such a source this term and the
    /// footprint that contains it over-charge by exactly this much. That is the
    /// cheap direction, and it is the same direction
    /// [`Layout::decode_footprint`] is conservative in elsewhere.
    ///
    /// **It is not a term of [`RangePlan::footprint`](crate::RangePlan::footprint).**
    /// A bulk range read fetches every block whole into a compressed window and
    /// decodes out of it at every worker count, so no decoder on that path owns
    /// an input buffer. A caller sizing a pool asks
    /// [`Layout::plan_range`]; this is what a *positioned* read holds.
    ///
    /// ```no_run
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use xz_seek::Reader;
    ///
    /// let reader = Reader::new(std::fs::File::open("dump.xz")?)?;
    /// let layout = reader.layout();
    /// // The published charge is the dictionary, this chunk, and the backend's
    /// // own state.
    /// assert!(layout.input_chunk_bytes() <= layout.decode_footprint());
    /// # Ok(())
    /// # }
    /// ```
    pub fn input_chunk_bytes(&self) -> u64 {
        self.table.input_chunk()
    }

    /// What a bulk read over `range` under `bulk` would hold, and how many
    /// workers it admits.
    ///
    /// **Readable before the read starts**, which is the point of it: it costs
    /// no source read and no decode, so a caller can ask *"does this file's
    /// blocks admit any parallelism inside my memory budget?"* and act on the
    /// answer — including by not making the read at all. See [`RangePlan`], and
    /// [`Bulk`] for the two numbers it is planned under.
    ///
    /// [`Reader::read_range`](crate::Reader::read_range) plans the same range
    /// under the same [`Bulk`] and gets the same answer; a caller who wants the
    /// number without the read asks here, and one who wants it afterwards asks
    /// [`RangeRead::plan`](crate::RangeRead::plan).
    ///
    /// **It never fails.** A budget too small for one worker is not an error but
    /// a plan whose [`RangePlan::fits`] is false, reporting the footprint one
    /// worker will use anyway.
    pub fn plan_range(&self, range: core::ops::Range<u64>, bulk: Bulk) -> RangePlan {
        RangePlan::new(&self.table, range, bulk, self.backend, self.memlimit)
    }

    /// The check declared by the stream holding block `block`.
    ///
    /// A block's check type is written in its stream's flags and nowhere else,
    /// and streams are ordered by compressed offset like the blocks inside
    /// them, so this is one binary search. It reads only fields that mean
    /// something for every stream — `first_block` does not, for a stream with no
    /// blocks.
    pub(crate) fn check_of(&self, block: usize) -> Result<Check> {
        let at = self.table.blocks[block].compressed_offset;
        let i = self
            .table
            .streams
            .partition_point(|s| s.compressed_offset <= at)
            .checked_sub(1);
        match i {
            Some(i) => Ok(self.table.streams[i].check),
            // A walked table cannot put a block outside every stream.
            None => Err(Error::IndexInconsistent {
                compressed_offset: at,
            }),
        }
    }
}
