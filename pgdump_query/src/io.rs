use std::future::Future;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

use bytes::Bytes;

use crate::{Error, Result};

/// Minimal async byte-range read abstraction.
///
/// The three required methods are shaped to mirror `object_store`'s
/// `get_range`/`head` semantics so a real `object_store`-backed implementation
/// can be added later (behind a Cargo feature flag — see
/// `docs/design/architecture.md`, "Execution model and API surface") without
/// changing them. [`ByteRangeSource::hint_read_size`] sits outside that mirror
/// and is defaulted, so such an implementation need not know it exists.
///
/// **Dyn-compatible on purpose** (`docs/design/architecture.md`, "Execution
/// model and API surface"): each method returns a
/// boxed future rather than `impl Future` (RPITIT), so callers can hold
/// `&dyn ByteRangeSource` / `Arc<dyn ByteRangeSource>` instead of being
/// generic over the source. [`XzSource`] is the second implementation and the
/// case this was done for — a *wrapping* source composes with whatever it
/// wraps instead of multiplying the branch at every call site, and a remote
/// source would square that again. One allocation per `read_range` call is
/// noise beside a chunk-sized read.
pub trait ByteRangeSource: Send + Sync {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>>;
    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>>;
    /// Last-modified time, if the source exposes one — `object_store`'s
    /// `head` carries this too. `None` rather than an error for a source
    /// that genuinely has no notion of one; the structure cache
    /// (`docs/design/architecture.md`, "The cache") treats an absent mtime as
    /// nothing to compare against, never as a mismatch.
    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>>;
    /// Bytes as stored on the device — what a `stat` reports — as opposed to
    /// [`ByteRangeSource::size`]'s addressable (possibly decompressed) length
    /// (`docs/design/architecture.md`, "The cache"). This is the structure
    /// cache's staleness check now:
    /// checking `size()` on a decompressing source would cost that source's
    /// whole stream-index walk on every `pgdq info`/`pgdq parse` invocation,
    /// where `stored_size()` costs one `stat`.
    ///
    /// Defaults to [`ByteRangeSource::size`], which is exactly right for a
    /// source that does not decompress — [`LocalFileSource`] never overrides
    /// this. A decompressing source overrides it to the compressed file's own
    /// length.
    fn stored_size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        self.size()
    }
    /// Whether [`ByteRangeSource::size`] is this source's *exact* addressable
    /// length rather than a bound (`docs/design/architecture.md`, "Execution
    /// model and API surface"). Every source implemented so far answers `true`
    /// honestly, xz's own size coming from its stream index exactly; nothing
    /// reads this yet; it exists for the gzip and zstd sources (P15, P18),
    /// whose sizes cannot always be known exactly ahead of a full decode.
    fn size_is_exact(&self) -> bool {
        true
    }
    /// The read length this caller is about to ask for over and over — a
    /// read loop's `ScanOptions::chunk_size`, announced once before the loop
    /// starts.
    ///
    /// **Advisory, and it defaults to doing nothing.** It exists because
    /// one-off-ness is a property of the *caller* and nothing in a
    /// `read_range` call carries it: an implementation that recycles buffers
    /// has to tell a chunk read, which repeats for the whole scan, from
    /// `map::attach_text`'s coalesced span read, which happens once per map,
    /// and by length alone it cannot ([`LocalFileSource`], and
    /// `docs/design/architecture.md`, "Execution model and API surface").
    fn hint_read_size(&self, _len: usize) {}
    /// The xz seek table behind this source, for a caller building a cache
    /// envelope to persist alongside it
    /// (`docs/design/architecture.md`, "The cache" — the sibling field
    /// `crate::cache::CompressionIndex` wraps this at save time).
    ///
    /// `None` by default, which is exactly right for a source with no
    /// compression layer; [`LocalFileSource`] never overrides it.
    /// [`XzSource`] returns the table it already built while walking the
    /// file's stream footers at open time, so persisting a cache never
    /// re-walks a file it is about to record one for. A cloned value rather
    /// than a borrow: the table is a plain, cheaply-cloned value (`Vec`s of
    /// small `Copy` entries) and a caller building a `CacheFile` needs to own
    /// it, not hold a borrow across an `await`.
    fn seek_table(&self) -> Option<xz_seek::SeekTable> {
        None
    }
    /// How this source would like `range` split across concurrent readers,
    /// and what one of those readers costs it resident
    /// (`docs/design/architecture.md`, "Execution model and API surface").
    ///
    /// **Advisory, and the caller never learns what is underneath.** A
    /// scheduler asks the source how to split, runs the partitions, and names
    /// no source type; a local file answers "anywhere, one buffer each" and a
    /// compressed one answers "at these block boundaries, a block each". That
    /// is what keeps decode scheduling out of the query layer, and it is the
    /// question a remote source inherits with a different answer — a
    /// ranged-GET size.
    ///
    /// **The default declines to advise**: one partition, at no stated cost.
    /// A source that has not thought about being read concurrently must not be
    /// split by a caller that assumed it had, and a cost is meaningless for a
    /// split that is not happening.
    ///
    /// **It answers where and at what cost, never whether.** Whether a split
    /// pays depends on the work the caller is about to do, not on the source:
    /// the same [`LocalFileSource`] over the same range is worth cutting for
    /// extraction and not for discovery. A caller reads this for the shape of
    /// the cut and decides on its own whether to make one
    /// (`docs/design/architecture.md`, "Execution model and API surface").
    fn partitions(&self, _range: Range<u64>) -> Partitioning {
        Partitioning::single(0)
    }

    /// What reading this source's container a block at a time would cost at
    /// **one** reader, in bytes — the number a memory budget has to clear for
    /// that path to be taken at all, whether or not it was
    /// (`docs/design/architecture.md`, "The compressed source").
    ///
    /// **One reader's charge *plus the pool floor it leaves*, because that is
    /// the line the gate actually draws** ([`BlockCache::affordable`]): a pool
    /// serving one reader still holds [`POOL_DEPTH`] slots, so the recourse a
    /// declined source names would be short of the budget that buys the path
    /// back if it named the per-reader term alone.
    ///
    /// `None` for a source with no such path to take, which is every source
    /// but a compressed one.
    ///
    /// **It exists so that the decline has one statement of its rule.** A
    /// caller that meant to name what to raise a budget *to* would otherwise
    /// re-derive the source's own arithmetic — the block unit twice, the chunk
    /// buffer and the decoder's own retention — and the two copies would part
    /// company at the first change to any of the four. Whether the path was
    /// declined is still read off [`ByteRangeSource::partitions`], which is a
    /// comparison between two values already in hand; this is only the number
    /// the message needs.
    fn block_decode_bytes(&self) -> Option<u64> {
        None
    }

    /// How many concurrent readers this source recommends to a caller that has
    /// stated no count of its own — a **recommendation**, never a bound
    /// (`docs/design/architecture.md`, "Execution model and API surface").
    ///
    /// **The default is one, which is the serial path**, and it is the same
    /// convention [`ByteRangeSource::partitions`] follows for the same reason:
    /// a source that has not thought about being read concurrently must not be
    /// read concurrently by a caller who took its silence for consent. Nothing
    /// in the library reads this — a caller that states a count gets that
    /// count, and [`Parallelism`] is where a count is stated. It exists for the
    /// layer above, which has a person's flags to fill in
    /// (`pgdump_query-cli`'s `ParallelArgs::resolve`).
    ///
    /// **It is a raw count, not a budgeted one.** What the caller's byte budget
    /// affords is `crate::stream::worker_count`'s question and is asked of
    /// every count alike, stated or recommended, against the source's own
    /// per-partition footprint — so a source answering here reasons about the
    /// work rather than about the memory, and cannot make the budget bind
    /// twice.
    ///
    /// [`LocalFileSource`] inherits the default: a plain `parse` is slower than
    /// serial at every worker count measured, including with the pool-depth
    /// clamp lifted ("Where a scan's time goes"). [`XzSource`] overrides it,
    /// decode being the one shape that demonstrably scales.
    fn default_workers(&self) -> usize {
        1
    }

    /// What the workers of this source hold, where the caller has stated no
    /// budget of its own — a **recommendation**, never a bound, and the budget
    /// sibling of [`ByteRangeSource::default_workers`]
    /// (`docs/design/architecture.md`, "Execution model and API surface").
    ///
    /// **It is per worker rather than a total, and that is what makes the two
    /// recommendations a consistent pair.** A total would have to be a total
    /// *for some count*, and the only count a source knows is its own — so a
    /// caller that stated `--jobs 4` against a source recommending
    /// twenty-four would get a budget for twenty-four either way, and a
    /// composition trying to recover the per-worker cost from it would divide
    /// by the wrong number ([`Parallelism::discover_for`], which is where the
    /// two meet). Stated per worker, the source answers a question that has
    /// nothing to do with any count, and multiplying is the caller's.
    ///
    /// **The default is `None`: no recommendation at all**, which leaves a
    /// caller on whatever it would have used — [`DEFAULT_MEMORY_BUDGET`] where
    /// nothing was discovered, and the discovered allowance where something
    /// was. [`LocalFileSource`] inherits it, as it inherits the serial worker
    /// count, and for the same reason: a source that has not thought about
    /// being read concurrently asks for nothing to read it with.
    ///
    /// **It is what keeps "no limit found" from meaning "serial".** A
    /// compressed reader of an ordinary dump costs about 58 MiB, so the 64 MiB
    /// fallback admits exactly one of them — which would override
    /// [`XzSource`]'s `available_parallelism()` worker count to the serial
    /// path on an unlimited host, the machine most likely to run this. A count
    /// nothing can afford is not a recommendation, so a source that recommends
    /// a count recommends what each of those workers needs.
    ///
    /// **Nothing in the library reads it**, exactly as with the worker count:
    /// a caller that states a budget gets that budget, and this exists for the
    /// layer above, which has a person's flags to fill in.
    ///
    /// **It is a [`WorkerMemory`] rather than a scalar, because one source's
    /// cost is not linear in the count.** A block-decoding source holds a pool
    /// floor below [`POOL_DEPTH`] readers that no per-worker term can express,
    /// so the recommendation is a shape a budget is *solved* against
    /// ([`Parallelism::fit`]) rather than a number it is divided by. Every
    /// other source states its per-worker term and no floor, which is the same
    /// division it always was.
    fn default_worker_memory(&self) -> Option<WorkerMemory> {
        None
    }
    /// How much concurrency this caller allows, and how many bytes the source
    /// may hold while serving it — announced once before a read loop starts,
    /// exactly where [`ByteRangeSource::hint_read_size`] is
    /// (`docs/design/architecture.md`, "Execution model and API surface").
    ///
    /// **Advisory, and it defaults to doing nothing**, for the same reason the
    /// read-size hint is: the numbers are the *caller's*, stated on
    /// `ScanOptions`/`QueryOptions`, and nothing in a `read_range` call carries
    /// them. What a source makes of them is its own business —
    /// [`LocalFileSource`] sizes its one free list, [`XzSource`] divides the
    /// budget between its two read units and decides from it whether it can
    /// afford to decode a whole block at all.
    ///
    /// A [`Parallelism`] that states no byte count — [`Parallelism::default`]
    /// — leaves a source on [`DEFAULT_MEMORY_BUDGET`].
    fn hint_parallelism(&self, _parallelism: Parallelism) {}
    /// How long the read loop about to start will hold the bytes it gets back
    /// — announced once, beside the other two hints
    /// (`docs/design/architecture.md`, "Execution model and API surface").
    ///
    /// **This is what makes a bound on outstanding buffers safe**, and like
    /// one-off-ness it is a property of the *caller* that nothing in a
    /// `read_range` call carries. A source that recycles buffers can wait for
    /// a free slot instead of allocating past the budget it was given — but
    /// only for a holder that drops each read before it takes the next, since
    /// a holder that accumulates reads is the only thing that could free the
    /// slot it is waiting on. [`WaitPolicy`] is where a read loop says whether
    /// it may be made to wait.
    ///
    /// **A source applies it only where the loop is the holder.** The
    /// permission says what may be done to *this loop*, and it says nothing
    /// about the source's own caches: a pool whose buffers the source keeps
    /// past the read that took them has a holder the loop does not control, and
    /// a wait there blocks on a slot the loop could never free. [`XzSource`] is
    /// the case in hand — its block pool retains decoded blocks, so it is left
    /// at [`WaitPolicy::NeverWait`] however a loop announces itself.
    ///
    /// **Advisory, and it defaults to doing nothing** — which is
    /// [`WaitPolicy::NeverWait`]'s behaviour, so a source nobody announces to
    /// allocates exactly as it would without the method.
    fn hint_wait_policy(&self, _policy: WaitPolicy) {}
}

/// Whether a read loop's acquisitions may be made to **wait** for a pooled
/// slot (`docs/design/architecture.md`, "Execution model and API surface").
///
/// **It is a permission, not a description of the holder.** What the pool needs
/// to know is whether it may block this loop, and a loop grants that or
/// withholds it; how long the loop happens to keep its bytes is the *reason*
/// behind the answer rather than the answer itself. Naming it after the holder
/// made `scan` — which retains nothing — state something false about itself in
/// order to select the behaviour it wanted.
///
/// **The distinction is a deadlock, not a preference.** A loop that retains
/// into a batch holds every buffer it has taken a view into until that batch
/// flushes — `(max_source_span / chunk) + 1` chunk buffers for the serial
/// replay loop, and three or four whole decoded blocks against a pool of two
/// on a compressed source. If such a loop waited, the only task that could
/// free the slot would be the one waiting for it. A loop that reads, consumes
/// and drops holds exactly one buffer, so a wait is backpressure.
///
/// **No pair of option values stands in for this.** `TableStream` yields its
/// batches to the caller and a `Utf8View` batch carries the buffers its
/// columns were built on, so how many slots are outstanding is a property of
/// consumer code (`docs/design/architecture.md`, "Execution model and API
/// surface").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WaitPolicy {
    /// This loop is never to be blocked: a read allocates past the pool's
    /// budget rather than waiting for a slot.
    ///
    /// **The default, and what every read in this build did before the policy
    /// existed.** It is the safe answer in the direction that matters — a
    /// policy that fails to bind costs memory and shows up in `peak-rss`,
    /// where a wait that should not have been permitted is a hang with nothing
    /// to measure — so a source nobody announces to cannot block, and a loop
    /// whose discipline is in any doubt grants nothing.
    #[default]
    NeverWait,
    /// This loop may be blocked until a slot frees, which is what turns the
    /// pool's slot count into a bound on what is *outstanding* rather than
    /// only on what is idle.
    ///
    /// **Granting it is a promise about the loop**: that it consumes and drops
    /// each read before taking the next, so it holds **one** buffer at a time.
    /// A loop that grants this and then keeps two reads alive at once against
    /// a one-slot pool blocks forever. The promise binds the loop and nothing
    /// else, so a source that retains buffers of its own withholds the
    /// permission from that pool ([`ByteRangeSource::hint_wait_policy`]).
    MayWait,
}

/// Where a source is willing to be split
/// (`docs/design/architecture.md`, "Execution model and API surface").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartitionBoundaries {
    /// Anywhere in the range: no offset costs more to start reading at than
    /// another. A plain file's answer, and what a caller may cut into as many
    /// equal pieces as it has workers.
    ///
    /// **It is a fact about seeking, not a verdict on concurrency.** Whether
    /// seeking is the expensive part of the caller's work is a question this
    /// source was never asked, so a scheduler must not read this arm as
    /// "device-bound, stay serial" — a remote source answers `Anywhere` too,
    /// and it is the one with latency worth hiding.
    Anywhere,
    /// Only at these offsets — ascending, and strictly inside the range, so
    /// `n` of them describe `n + 1` partitions.
    ///
    /// **An empty list is "one partition", and it is a policy rather than an
    /// absence.** A source that could enumerate boundaries and still advises
    /// none is saying that splitting this range makes its readers worse, not
    /// that it failed to find a seam — which is exactly what a compressed
    /// source reading through a restart-and-discard decoder says
    /// (`docs/design/architecture.md`, "The compressed source").
    At(Vec<u64>),
}

/// What a caller's **retained** bytes are rounded out to on this source — the
/// unit a batch held across reads pins, as against the bytes one concurrent
/// reader costs while it is reading, which is
/// [`Partitioning::partition_bytes`]
/// (`docs/design/architecture.md`, "Execution model and API surface").
///
/// It exists because a caller budgeting sub-streams charges each one its held
/// batch's span *on top of* the decode footprint, and that second charge is
/// honest for one source shape and double-counts for the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RetainedUnit {
    /// The read chunk. A batch spanning `crate::batch::QueryOptions`'
    /// `max_source_span` bytes pins the chunk buffers those bytes were read
    /// into, and those are bytes [`Partitioning::partition_bytes`] has not
    /// charged for — so a caller adds the span. A plain file's answer.
    ///
    /// **The default, for the same reason [`WaitPolicy::NeverWait`] is one.**
    /// It is what every source was charged before the term existed, and it is
    /// the safe direction: over-charging plans fewer readers than the budget
    /// could hold, where under-charging plans readers it cannot.
    #[default]
    ReadChunk,
    /// The partition itself. A batch confined to a partition pins that
    /// partition whatever the span cap says
    /// (`docs/design/architecture.md`, "Three flush triggers, and only one of
    /// them bounds memory"), and [`Partitioning::partition_bytes`] has already
    /// charged for it — so adding the span would count the same bytes twice.
    /// A block-decoding source's answer, whose retained unit is the decoded
    /// block a partition is made of.
    Partition,
}

/// How a worker reads the piece it was handed
/// ([`crate::leader::scan_partition`]) — **stated by the source, because the
/// right answer differs by source and was measured to differ by a factor of
/// twenty-two**.
///
/// It is a separate statement from [`RetainedUnit`] and from
/// [`Partitioning::partition_bytes`] on purpose. Those two say what a reader
/// *holds*; this says what shape its reads are, and deriving one from another
/// is how the cut size came to follow the memory charge
/// ([`Partitioning::window_end`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PartitionRead {
    /// `ScanOptions::chunk_size` at a time, repeating until the piece is
    /// consumed. A plain file's answer, and **the default, for the reason
    /// [`RetainedUnit::ReadChunk`] is one**: a chunk-sized read is the
    /// announced length, so [`BufferPool::keeps`] pools every buffer a worker
    /// takes, and a source that has said nothing about itself is charged and
    /// read the conservative way.
    ///
    /// Measured on the plain control at `--jobs 24`: **9.4 MiB resident
    /// against 209.2**, and 1.30 s against 1.54 — the partition-length buffer
    /// that `keeps` refuses and the releasing thread's arena retains is simply
    /// not allocated. The extra read syscalls and `spawn_blocking` hops it was
    /// expected to cost are not visible.
    #[default]
    Chunked,
    /// One of the source's own `unit`s in a single call — the whole piece
    /// where the piece is one unit, which is what
    /// [`BOUNDARIED_PARTITION_UNITS`] makes it. A block-decoding source's
    /// answer, where that read is a **zero-copy slice** of a block the worker
    /// was going to decode anyway — so it allocates nothing at all, where
    /// reading the same block a chunk at a time is 24 lookups against one
    /// ([`BlockCache::lookup`], which takes the cache's lock).
    ///
    /// Measured on the 24 MiB-block control, flagless in 1 GiB, where both
    /// arrangements resolve thirteen readers: **854 MiB median resident
    /// against 931, none killed against one of three**. What a chunk-sized
    /// read costs there is unattributed — the candidate is that a block
    /// evicted while a view is out lives past the slot cap, and chunk reads
    /// multiply the eviction events by the chunk count — and it is not claimed
    /// as accounted for.
    ///
    /// **The unit is what bounds the buffer, and it is the source's number
    /// rather than the cut's.** `crate::leader::scan_partition` reads
    /// `min(piece, unit)`, so at the shipped width — where a piece *is* one
    /// unit — this is the piece exactly and the identical read, and above it
    /// the buffer is capped at one unit instead of growing with the width.
    /// That removes the unbounded case and **not** the copy: [`BufferPool::keeps`]
    /// admits only the announced chunk, so a one-unit read is un-poolable
    /// either way, and `xz_seek::SeekTable::max_block_uncompressed` is a
    /// file-wide maximum, so a read starting on a boundary can still cross
    /// into a smaller successor block and take the copying arm. Raising the
    /// cut width safely wants a read clipped to the next boundary, which is
    /// not this (`docs/design/architecture.md`, "cut-width").
    Whole {
        /// The source's own unit, in bytes — the decoded block on the
        /// block-decoding path, where [`BlockCache::unit`] is
        /// `xz_seek::SeekTable::max_block_uncompressed`. Never zero: a source
        /// with no unit has nothing to state and stays [`PartitionRead::Chunked`].
        unit: u64,
    },
}

/// How many of a boundaried source's own units one partition covers
/// ([`Partitioning::window_end`], [`PartitionBoundaries::At`]) — **the cut
/// width, chosen by measurement**.
///
/// **What it buys is the tail read's second decode, amortised.** A worker
/// finishes its piece by reading one chunk past the piece's end, and on a
/// block-decoding source those bytes are in the block its *successor* owns —
/// so both decode that block and nothing shares the result (`KD20`). The waste
/// is one unit's decode per **piece**, whatever the piece covers, so a piece of
/// `k` units pays `1/k` of a unit per unit read: at one it doubles the decode
/// work of the region, at two it adds half, at four a quarter.
///
/// **It does not widen what a reader holds.** Retention on this path is capped
/// by [`BufferPool::slots`] and not by the piece — [`BlockCache::slot`] drains
/// to one below the slot count before every decode — so a worker walking `k`
/// units holds the unit it views and the unit it is decoding exactly as it
/// does at one, which is the two units [`BlockCache::reader_bytes`] charges.
/// The cut width and the memory charge are therefore genuinely independent
/// numbers, which is the whole reason [`Partitioning::window_end`] exists
/// apart from [`Partitioning::partition_bytes`].
///
/// **What it costs is granularity.** A window is `want × k` units wide, so a
/// region shorter than that is cut into fewer pieces than there are workers
/// and the last window of every region is ragged. That is the trade the
/// measurement priced.
const BOUNDARIED_PARTITION_UNITS: usize = 1;

/// What a source holds resident while some number of concurrent workers read
/// it — **a shape rather than a scalar**, because the cost is not linear in
/// the count (`docs/design/architecture.md`, "Execution model and API
/// surface").
///
/// Two terms:
///
/// - **per worker**, paid once for each concurrent reader, which is what
///   [`Partitioning::partition_bytes`] states; and
/// - a **shared floor**, `floor_unit` bytes for every worker *below*
///   `floor_below` — buffers a pool holds whatever the count, and so the term
///   that is largest when the count is smallest.
///
/// **The floor is the block pool's, and it is a real quantity rather than a
/// safety margin.** [`BufferPool::slots`] clamps a pool at
/// `POOL_DEPTH.max(jobs)`, so below [`POOL_DEPTH`] readers the block pool
/// holds `(POOL_DEPTH − jobs)` units that a per-reader charge of two units
/// each never billed: the pool's free list and its retention together take
/// `slots` of them and each reader is decoding into one besides, which is
/// `slots + jobs` against a bill of `2 × jobs`. It is **unbounded in the block
/// size** — 96 MiB at koji's 24 MiB blocks, 384 MiB at 128 and 2 GiB at 512 —
/// which is why [`MEMORY_RESERVE`] cannot absorb it and why it is billed here
/// instead.
///
/// **A budget is solved against this, never divided by it**
/// ([`WorkerMemory::affords`]). `cap / per_worker` is the arithmetic this type
/// replaces, and it errs in the one direction that matters: it admits readers
/// whose share of the floor the allowance never granted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkerMemory {
    per_worker: u64,
    floor_unit: u64,
    floor_below: usize,
}

impl WorkerMemory {
    /// A cost that is `per_worker` bytes a worker and nothing else — every
    /// source's shape but the block-decoding one's.
    pub const fn per_worker(per_worker: u64) -> Self {
        Self { per_worker, floor_unit: 0, floor_below: 0 }
    }

    /// State that a pool holds `unit` bytes for each worker short of `below`,
    /// on top of the per-worker term.
    pub const fn flooring(mut self, unit: u64, below: usize) -> Self {
        self.floor_unit = unit;
        self.floor_below = below;
        self
    }

    /// The per-worker term alone — what one more concurrent reader adds once
    /// the floor is filled, and what a *cut* is sized by
    /// ([`Partitioning::window_end`]).
    pub const fn bytes_per_worker(self) -> u64 {
        self.per_worker
    }

    /// The shared floor at `workers` readers: zero at or above `floor_below`.
    pub fn floor_bytes(self, workers: usize) -> u64 {
        (self.floor_below.saturating_sub(workers) as u64).saturating_mul(self.floor_unit)
    }

    /// What `workers` concurrent readers cost, in bytes.
    pub fn at(self, workers: usize) -> u64 {
        self.per_worker.saturating_mul(workers as u64).saturating_add(self.floor_bytes(workers))
    }

    /// Whether this source states no cost at all, which is the declining
    /// default and every source before a cost was stated: such a source is
    /// bounded by the caller's own count and by nothing here.
    pub const fn is_zero(self) -> bool {
        self.per_worker == 0 && self.floor_unit == 0
    }

    /// The same shape with `extra` bytes added to the per-worker term — what a
    /// caller pinning a batch of its own on top of the source's charge pays
    /// (`crate::stream::plan_partitions`).
    pub const fn plus_per_worker(mut self, extra: u64) -> Self {
        self.per_worker = self.per_worker.saturating_add(extra);
        self
    }

    /// The largest count in `1..=most` that `cap` affords, **never zero**: a
    /// cap too small for even one worker is a real arrangement, and one worker
    /// is what the floors inside the mechanism deliver there.
    ///
    /// **Two regimes, because the cost need not be monotone.** At or above
    /// `floor_below` the floor is gone and the cost is one multiplication, so
    /// the largest affordable count there is a single division. Below it the
    /// floor decays as the count rises, so a larger count can cost *less*;
    /// those candidates are enumerated rather than divided for, and there are
    /// at most [`POOL_DEPTH`] of them.
    pub fn affords(self, cap: u64, most: usize) -> usize {
        let most = most.max(1);
        let above = match self.per_worker {
            0 => most,
            per_worker => usize::try_from(cap / per_worker).unwrap_or(usize::MAX).min(most),
        };
        if above >= self.floor_below {
            return above.max(1);
        }
        (1..=most.min(self.floor_below)).rev().find(|workers| self.at(*workers) <= cap).unwrap_or(1)
    }
}

/// A source's answer to [`ByteRangeSource::partitions`]: where to split, what
/// one partition holds resident while it reads, and what a batch held across
/// reads pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partitioning {
    boundaries: PartitionBoundaries,
    /// What concurrent readers of this source cost — the per-worker term every
    /// constructor takes, plus whatever shared floor
    /// [`Partitioning::flooring`] has stated ([`WorkerMemory`]).
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

    /// State that this source holds `unit` bytes for every worker short of
    /// `below`, on top of the per-worker charge — the block pool's floor, and
    /// nothing every other source has ([`WorkerMemory`]).
    pub fn flooring(mut self, unit: u64, below: usize) -> Self {
        self.memory = self.memory.flooring(unit, below);
        self
    }

    /// What concurrent readers of this source cost, as a shape a budget is
    /// solved against — [`Partitioning::partition_bytes`] is its per-worker
    /// term, and `crate::stream::worker_count` is what reads it.
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
    /// For a plain file that is [`PLAIN_PARTITION_CHUNKS`] read chunks, which
    /// a worker takes in one buffer. For a block-decoding compressed one it is
    /// the block unit **twice** — the block being decoded and the one the
    /// reader retains beside it — plus the chunk buffer a read straddling a
    /// boundary is assembled into, plus the decoder's own retention: 58 MiB
    /// against the 24 MiB blocks koji's download carries.
    ///
    /// **The decoder's own retention is inside it, and it is a published
    /// number rather than a guess.** `xz_seek::Reader::decode_footprint()` —
    /// the LZMA2 dictionary, the compressed input buffer and the backend's own
    /// state — costs no source read and is the same whatever range is read, so
    /// the source charges it once at construction and never re-derives it. A
    /// charge that left it out was the whole of a 2.4× under-count
    /// (`docs/design/architecture.md`, "The compressed source"). The dictionary
    /// is written in each block's *header* and the table records the first
    /// block's per stream, so that number is a very good estimate and not a
    /// sound ceiling; what cannot understate is the reader's `memlimit`,
    /// compared against each block's own declared dictionary before any backend
    /// object is built.
    ///
    /// **A shared cost is not a per-reader one.** The streaming fallback keeps
    /// one decoder behind a mutex however many readers a caller runs, so that
    /// arm charges the chunk buffer alone and leaves the decoder to the fixed
    /// term a budget's reserve covers; the block path builds a decoder per
    /// concurrent decode, which is what makes the same number per-reader there.
    pub fn partition_bytes(&self) -> u64 {
        self.memory.bytes_per_worker()
    }

    /// What a batch this caller holds across reads pins on this source, which
    /// is what a sub-stream's span allowance is charged against — see
    /// [`RetainedUnit`], and `crate::stream::plan_partitions`, which is the
    /// one caller that reads it.
    pub fn retained_unit(&self) -> RetainedUnit {
        self.retained
    }

    /// How a worker should read one of these partitions — see
    /// [`PartitionRead`], and `crate::leader::scan_partition`, which is the
    /// one caller that reads it.
    pub fn partition_read(&self) -> PartitionRead {
        self.read
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
    /// past `limit` — the number `crate::leader::run_region` hands
    /// `crate::stream::cut`, and **the cut size, which is not the memory
    /// charge**.
    ///
    /// [`Partitioning::partition_bytes`] is what one reader *holds*;
    /// this is what one reader *covers*. They are different quantities and
    /// they answer to different pressures — the charge to a memory allowance,
    /// the coverage to the tail read's wasted decode
    /// ([`BOUNDARIED_PARTITION_UNITS`]) — so computing the second from the
    /// first is what let a change made for one silently move the other. It
    /// did: raising the charge to [`BlockCache::reader_bytes`] widened the
    /// window until [`crate::stream::cut`] had to thin the boundaries on
    /// offer, and a partition became 2.08–2.42 blocks with no decision
    /// recorded anywhere (`docs/design/architecture.md`, "Execution model and
    /// API surface").
    ///
    /// So the two are computed apart:
    ///
    /// - [`PartitionBoundaries::Anywhere`] has no seams to respect, so a
    ///   window is `want` charges wide — which is what a plain file has always
    ///   been cut into, unchanged;
    /// - [`PartitionBoundaries::At`] ends the window at the
    ///   **`want × `[`BOUNDARIED_PARTITION_UNITS`]-th boundary strictly past
    ///   `start`**, so the window holds `want` pieces of that many units each
    ///   and [`crate::stream::cut`]'s thinning picks every `k`-th boundary.
    ///   Fewer boundaries left than that is the region's tail: the window runs
    ///   to `limit`, and the thinning still cannot give a piece more than `k`
    ///   units, since the window never contains more than `want × k` of them.
    ///
    /// Asserted rather than asserted-in-prose:
    /// [`a_block_decoding_partition_spans_at_most_the_cut_width`].
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

/// How much concurrency a caller allows a scan or a query, and how much
/// memory that concurrency may hold
/// (`docs/design/architecture.md`, "Execution model and API surface").
///
/// **The library defaults to a [`Parallelism::Serial`] stating no budget**,
/// which is the serial
/// code path this build has and not a pool of one: an embeddable component
/// does not spawn threads by surprise, so parallelism is opted into. The CLI
/// makes the opposite default, being a program a person ran on purpose.
///
/// **Two numbers, and whichever binds first wins**, mirroring
/// `xz_seek::Bulk::new(workers, budget_bytes)` — which is the interface a
/// compressed source's decode is ultimately planned against, so the surface a
/// caller states it in is the same shape. Neither is defaulted inside
/// [`Parallelism::Workers`]: the right worker count is a property of the
/// caller's device and build, and bytes are the only one of the two that can
/// be promised to a memory cgroup.
///
/// **`Serial` is a state, not the number one.** One worker and the serial path
/// are the same execution, so [`Parallelism::workers`] answers `Serial` for a
/// count of one rather than building a degenerate `Workers` nobody can tell
/// from it — which is what keeps "is this parallel" a match on the value
/// instead of a comparison against a magic number.
///
/// **The two numbers are independent, so the collapse takes only one of them
/// down.** `Serial` carries the stated budget as an `Option`, because a caller
/// that asks for one worker inside 400 MiB has said something a source can act
/// on: how large a chunk pool it may hold, and whether it can afford to decode
/// a whole compressed block. `None` is the distinct fact that nobody stated a
/// budget at all, which is [`Parallelism::default`] and what
/// [`DEFAULT_MEMORY_BUDGET`] answers.
///
/// **Three mechanisms read it, and only the third spawns.** `memory_bytes` is
/// what both pools in a source are sized from, and it is what decides whether
/// a compressed source can afford to decode a whole block ("The compressed
/// source"); `jobs` is the block pool's depth, one retained block per
/// concurrent reader, and — capped by what the bytes afford — how many
/// sub-streams a partitioned replay is cut into
/// (`crate::table_stream_partitions`). The caller runs those sub-streams, so
/// what `jobs` states is a ceiling rather than a request. The third is
/// [`crate::leader::scan_region`], which both numbers size a window of fused
/// workers from, and which the mapping pass offers every open `COPY` region to
/// (`docs/design/architecture.md`, "Execution model and API surface").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parallelism {
    /// The serial code path: one thread, reading in file order — inside
    /// `memory_bytes` where the caller stated one.
    Serial {
        /// What that one thread's source may hold in pooled buffers, in
        /// bytes, or `None` where the caller stated no budget at all. The
        /// worker count collapsing to the serial path says nothing about the
        /// budget beside it, so the budget survives the collapse.
        memory_bytes: Option<u64>,
    },
    /// At most `jobs` concurrent workers, holding at most `memory_bytes`
    /// between them.
    Workers {
        /// The ceiling on concurrent workers — a ceiling rather than a
        /// request, since three input shapes admit no parallelism at all
        /// (`docs/design/architecture.md`, "Execution model and API
        /// surface").
        jobs: NonZeroUsize,
        /// What those workers may hold resident between them, in bytes. This
        /// is the number a memory cgroup is denominated in, and the one a
        /// worker count cannot be promised to: the same count is 192 MiB on a
        /// file of 24 MiB blocks and 1 GiB on a file of 128 MiB ones.
        memory_bytes: u64,
    },
}

/// The library's own default: the serial path, stating no budget — an
/// embeddable component does not spawn threads by surprise, and it does not
/// claim a byte count nobody gave it either.
impl Default for Parallelism {
    fn default() -> Self {
        Self::Serial { memory_bytes: None }
    }
}

impl Parallelism {
    /// `jobs` workers inside `memory_bytes`, or the serial path inside that
    /// same budget where `jobs` is one or zero.
    ///
    /// **The collapse is on the worker count alone.** A count of one is the
    /// serial path, but the bytes beside it were still stated, so they ride
    /// through into [`Parallelism::Serial`]'s own field rather than being
    /// dropped — which is what makes a stated budget worth stating at any
    /// worker count (`docs/design/architecture.md`, "Execution model and API
    /// surface").
    ///
    /// Zero reads as one rather than as an error, exactly as
    /// `xz_seek::Bulk::new` reads it: it is a shape a caller's own arithmetic
    /// produces, and an error would only make them do that arithmetic twice.
    pub fn workers(jobs: usize, memory_bytes: u64) -> Self {
        match NonZeroUsize::new(jobs) {
            Some(jobs) if jobs.get() > 1 => Self::Workers { jobs, memory_bytes },
            _ => Self::Serial { memory_bytes: Some(memory_bytes) },
        }
    }

    /// Whether this is the serial path — a match on the variant, so a stated
    /// budget does not change the answer.
    pub fn is_serial(&self) -> bool {
        matches!(self, Self::Serial { .. })
    }

    /// The bytes this caller allows, or `None` where it stated none — which is
    /// [`Parallelism::default`] and nothing else.
    ///
    /// `None` rather than [`DEFAULT_MEMORY_BUDGET`] because "the caller said
    /// nothing" and "the caller said 64 MiB" are different facts, and a source
    /// that already holds a budget of its own — one set by an earlier
    /// announcement — must be able to tell them apart.
    pub fn memory_bytes(&self) -> Option<u64> {
        match self {
            Self::Serial { memory_bytes } => *memory_bytes,
            Self::Workers { memory_bytes, .. } => Some(*memory_bytes),
        }
    }

    /// The ceiling on concurrent workers: one for [`Parallelism::Serial`],
    /// which *is* one worker rather than none.
    pub fn jobs(&self) -> usize {
        match self {
            Self::Serial { .. } => 1,
            Self::Workers { jobs, .. } => jobs.get(),
        }
    }

    /// The arrangement to run when the caller has stated neither number and
    /// has nothing open — the environment's own answer, composed from
    /// [`discover_memory_limit`] and `std::thread::available_parallelism`
    /// through [`MEMORY_RESERVE`]
    /// (`docs/design/architecture.md`, "Execution model and API surface").
    ///
    /// **It is a convenience over two primitives, and taking it is opting
    /// in.** [`Parallelism::default`] is still the serial path stating no
    /// budget, so a library caller that says nothing still spawns nothing;
    /// this is for the caller that wants the whole answer and would otherwise
    /// reimplement the reserve arithmetic, which is a measured finding rather
    /// than a detail.
    ///
    /// **An unlimited environment falls back to today's constant**, which is
    /// what keeps discovery strictly additive: with no limit found and no
    /// source to recommend otherwise, this is
    /// [`DEFAULT_MEMORY_BUDGET`] — or, at a serial count,
    /// [`Parallelism::default`] itself.
    pub fn discover() -> Self {
        Self::discover_for(std::thread::available_parallelism().map_or(1, NonZeroUsize::get), None)
    }

    /// [`Parallelism::discover`]'s rule against recommendations a caller has
    /// already obtained — a source's own worker count
    /// ([`ByteRangeSource::default_workers`]) and what one of those workers
    /// holds ([`ByteRangeSource::default_worker_memory`]).
    ///
    /// **It answers with both numbers, because they are a pair.** What the
    /// caller asks is "I would like `jobs` workers, each holding
    /// `per_worker`" — and where the environment's allowance affords fewer,
    /// the answer is the smaller count *and* the budget that count spends,
    /// never the full count beside a budget it cannot have
    /// (`docs/design/roadmap.md`, "A default runs as fast as the allocation
    /// permits"). `jobs` is therefore a recommendation this may lower; a
    /// caller holding a count somebody **stated** keeps that count and takes
    /// only the budget from here, which is what `ParallelArgs::resolve` does.
    ///
    /// **The composition is not a `min`, and the difference is one
    /// `Option`.** Written as `min(want, discovered)` it breaks the case it
    /// exists for: on an unlimited host `discover` falls back to today's
    /// 64 MiB constant, so the minimum would be 64 MiB and a compressed scan —
    /// whose reader costs about 58 MiB — would be serial again. What
    /// distinguishes *no limit found* from *a small limit* is that the first
    /// has no cap at all:
    ///
    /// - a discovered limit caps at `limit − MEMORY_RESERVE`, and
    ///   `jobs × per_worker` — or [`DEFAULT_MEMORY_BUDGET`] where the caller
    ///   has no recommendation — is taken no higher. That is "do not take what
    ///   you cannot use", and it is allowed to fall **below**
    ///   [`DEFAULT_MEMORY_BUDGET`], because reasserting that constant under a
    ///   small limit would put today's default back in the one case discovery
    ///   was built for. The **count** answers to a second condition beside
    ///   that cap: its predicted resident must leave
    ///   [`MEMORY_MARGIN_PERCENT`] of the limit unused
    ///   ([`margin_allowance`]), which is what stops the answer depending on
    ///   how many cores the host happens to have;
    /// - no limit found caps at half of [`available_memory`], which costs
    ///   nothing wherever there is room — resident saturates at
    ///   `jobs × per-reader`, so every byte above that is structurally inert —
    ///   and binds only on a machine too small to afford the recommended
    ///   worker count. Half rather than all because `MemAvailable` is an
    ///   estimate two processes reading at once each see the whole of.
    ///
    /// **A caller with no recommendation and no limit states nothing**, which
    /// is [`Parallelism::default`] at a serial count and
    /// [`DEFAULT_MEMORY_BUDGET`] above one — the distinction
    /// [`Parallelism::Workers`] has nowhere to record.
    pub fn discover_for(jobs: usize, memory: Option<WorkerMemory>) -> Self {
        Self::discover_in(Path::new("/"), jobs, memory)
    }

    /// [`Parallelism::discover_for`] against an arbitrary filesystem root, for
    /// the reason [`discover_memory_limit_in`] takes one — and public for the
    /// same reason it is: a caller's own resolution is pinned against
    /// environments this machine cannot be put into
    /// (`pgdump_query-cli/tests/data/runtime/`).
    pub fn discover_in(root: &Path, jobs: usize, memory: Option<WorkerMemory>) -> Self {
        let (cap, ceiling) = match discover_memory_limit_in(root) {
            Some(limit) => (
                Some(limit.bytes.saturating_sub(MEMORY_RESERVE)),
                Some(margin_allowance(limit.bytes)),
            ),
            // Nothing discovered and nothing recommended: no cap to state, and
            // no recommendation to cap. That is today's default, unchanged.
            None if memory.is_none() => (None, None),
            // `MemAvailable` unreadable is the one shape with a recommendation
            // and no ceiling to hold it under — and no limit to take a margin
            // of either, the halving being the margin there.
            None => {
                (Some(available_memory_in(root).map_or(u64::MAX, |available| available / 2)), None)
            }
        };
        match cap {
            Some(cap) => {
                let (jobs, budget) = Self::fit(jobs, memory, cap, ceiling);
                Self::workers(jobs, budget)
            }
            None if jobs > 1 => Self::workers(jobs, DEFAULT_MEMORY_BUDGET),
            None => Self::default(),
        }
    }

    /// [`Parallelism::discover_for`]'s lowering against a budget the caller
    /// already holds — a *recommended* `jobs` cut to what `memory_bytes`
    /// affords at `per_worker` each, with the budget kept exactly as stated.
    ///
    /// **A recommended count answers to the allowance however that allowance
    /// arrived.** `discover_for` reads the number off the environment and
    /// lowers the count to fit it; this is the same rule where the number was
    /// *typed* instead (`docs/design/roadmap.md`, "A default runs as fast as
    /// the allocation permits"). What the rule is scoped to is the absence of
    /// a **count**, never the absence of a budget: a caller holding a count
    /// somebody stated calls [`Parallelism::workers`] and keeps it.
    ///
    /// **No margin here**, unlike `discover_for`: [`MEMORY_MARGIN_PERCENT`] is
    /// a share of a *limit* the environment states, and a budget somebody
    /// typed is not one — an operator who states bytes has made the headroom
    /// decision themselves.
    ///
    /// **Only the count moves.** Where `discover_for` hands back the budget
    /// the lowered count spends — because it chose that budget and must not
    /// name one the allowance never granted — the budget here is the caller's
    /// own, and lowering it would be overruling a stated flag
    /// (`docs/design/architecture.md`, "Status output").
    pub fn recommended_within(
        jobs: usize,
        memory: Option<WorkerMemory>,
        memory_bytes: u64,
    ) -> Self {
        Self::workers(Self::fit(jobs, memory, memory_bytes, None).0, memory_bytes)
    }

    /// The largest pair `(count, budget)` that fits inside `cap`: as many of
    /// `jobs` workers as `cap` affords at `per_worker` each, and exactly what
    /// that many of them spend.
    ///
    /// **The floor is one worker at whatever `cap` is**, not one worker's
    /// worth of bytes. A cap too small for even a single reader is a real
    /// arrangement — a cgroup at or under [`MEMORY_RESERVE`] resolves to a budget
    /// of zero — and the
    /// three floors already inside the mechanism turn it into one reader on
    /// the streaming path. Handing back `per_worker` there would be the one
    /// thing this function exists to stop: a budget the allowance never
    /// granted.
    ///
    /// A caller recommending nothing is capped at [`DEFAULT_MEMORY_BUDGET`]
    /// and keeps its count, there being no cost to solve against.
    ///
    /// **It solves rather than divides** ([`WorkerMemory::affords`]), because
    /// one source's cost is not linear in the count: a block-decoding source
    /// holds a pool floor below [`POOL_DEPTH`] readers, so `cap / per_worker`
    /// hands back a count whose floor the allowance never granted. The budget
    /// it names is what that many workers actually spend, floor included,
    /// which is what keeps the number the run reports for itself true.
    ///
    /// **`charge_ceiling` is the margin, and it bounds the count alone.**
    /// Where the cap came from a discovered limit the caller also states the
    /// most this arrangement may be *charged* if its predicted resident is to
    /// leave [`MEMORY_MARGIN_PERCENT`] of that limit unused
    /// ([`margin_allowance`]) — predicted resident being `memory.at(n)` plus
    /// [`MEMORY_UNPOOLED_BOUND`], which is this crate's bound on what a scan
    /// holds outside its pools. A budget somebody **typed** carries no
    /// such ceiling: the margin is a statement about a limit, and a stated
    /// budget is not one. The floor is still one worker, so the ceiling can
    /// lower a count and never decline the one reader the mechanism's own
    /// floors deliver; and the budget stays `cap`-bounded rather than
    /// ceiling-bounded, so what a lowered count reports is still what it
    /// spends (`docs/design/architecture.md`, "Execution model and API
    /// surface").
    fn fit(
        jobs: usize,
        memory: Option<WorkerMemory>,
        cap: u64,
        charge_ceiling: Option<u64>,
    ) -> (usize, u64) {
        let jobs = jobs.max(1);
        let Some(memory) = memory.filter(|memory| !memory.is_zero()) else {
            return (jobs, DEFAULT_MEMORY_BUDGET.min(cap));
        };
        let affords = memory.affords(charge_ceiling.map_or(cap, |ceiling| cap.min(ceiling)), jobs);
        (affords, cap.min(memory.at(affords)))
    }
}

/// The byte budget actually governing reads under `p`, worded for a status
/// line rather than for code — [`Parallelism::memory_bytes`] itself, printed
/// bare, renders an unstated budget as `None`, which states nothing a
/// reader can act on. Deliberately a free function rather than a method on
/// [`Parallelism`]: that type's own `memory_bytes` exists precisely to keep
/// "the caller said nothing" apart from "the caller said 64 MiB" for a source
/// that must not have an already-announced budget silently overwritten, and a
/// status line has no such source to protect — it wants the number actually
/// in force either way, worded honestly about which one it got: the stated
/// byte count, or [`DEFAULT_MEMORY_BUDGET`] — what every pool falls back to —
/// marked `(default)` since nothing was asked for it
/// (`docs/design/architecture.md`, "Status output").
pub(crate) fn memory_budget_display(p: Parallelism) -> String {
    match p.memory_bytes() {
        Some(bytes) => bytes.to_string(),
        None => format!("{DEFAULT_MEMORY_BUDGET} (default)"),
    }
}

/// What a source may hold in pooled buffers when the caller has stated no
/// budget of its own — [`Parallelism::default`]'s number, and the CLI's default
/// for `--parallel-memory`.
///
/// **It is the serial path's budget and it is deliberately not the largest
/// block anyone might write.** 64 MiB is [`POOL_DEPTH`] slots at the largest
/// chunk size the read-chunk sweep measured
/// (`docs/design/measurements.md`, "What the read chunk size is worth"), so
/// nothing at or below 16 MiB loses a slot to it and the 1.83× a pool miss
/// costs cannot come back through this ceiling. It is also the line a
/// compressed source's whole-block decode is refused above, which is what
/// makes the number a *bound* rather than an aspiration — see
/// [`BlockCache::affordable`] and `docs/design/architecture.md`, "The
/// compressed source".
pub const DEFAULT_MEMORY_BUDGET: u64 = 64 << 20;

/// What [`Parallelism::discover`] holds back from a discovered memory limit,
/// in bytes: everything the process holds that a stated budget does not bound
/// — the runtime's threads, glibc's per-thread arenas, the decoder state a
/// compressed source keeps outside its pools, and the binary itself.
///
/// **It is a subtraction rather than a fraction, and the shape argument is
/// what chose it.** A percentage would under-reserve at a small limit and
/// over-reserve at a large one, which is backwards: the small cgroup is where
/// being wrong kills the process
/// (`docs/design/roadmap-P19-efficient-defaults.md`, "What is discovered, and
/// what the default makes of it"). It survives its old justification — that
/// resident above a stated budget is roughly constant *because*
/// [`BufferPool::slots`] clamps at `POOL_DEPTH.max(jobs)` — which is false and
/// inverted: that clamp is what makes resident concave in the reader count
/// (`docs/design/architecture.md`, "Execution model and API surface").
///
/// **What it must satisfy: the worst observed rep leaves at least 20% of the
/// limit**, the median being context rather than the gate, because a cgroup's
/// killer reads one run's peak. The criterion bounds this number rather than
/// describing it, and **384 MiB is the smallest of five candidates measured
/// against it** — 256, 320, 384, 448 and 512 MiB, each a build of its own, over
/// two block sizes and four container limits with nothing stated
/// (`docs/design/roadmap-P19.16-reserve-constant-notes.md`). What one reader
/// holds is billed to within 1.3% by [`BlockCache::reader_bytes`], so what
/// this covers is the excess above that charge and not a per-reader term.
///
/// **It is the cap, and it is not the bound on that excess.** Those were one
/// number until `19.26` and the doubling was invisible: this constant is the
/// smallest meeting the criterion *under the cap rule*, so it already contains
/// `0.2 × 1 GiB` and subtracting it again as a predicted flat term applied the
/// same criterion twice. What bounds the excess is
/// [`MEMORY_UNPOOLED_BOUND`], which is what [`margin_allowance`] predicts
/// with; this one still comes off the top of a discovered limit and still
/// decides what [`BlockCache::affordable`] sees.
/// **Raising this constant is also what declines the block path**, since
/// [`BlockCache::affordable`] reads off the budget this leaves: the two are one
/// knob, and the floor below which a compressed scan is serial is implicit in
/// it rather than stated separately.
///
/// **What it was validated over, and what now carries the rest.** It was
/// measured to a 2 GiB limit *on a 24-core host*, where
/// `std::thread::available_parallelism` clamps the count the allowance would
/// otherwise afford — so the criterion held at that limit by the host's width
/// rather than by this number. It is no longer asked to: the count answers to
/// the criterion directly ([`MEMORY_MARGIN_PERCENT`]), which is what makes the
/// resolved arrangement a property of the allocation and not of the machine's
/// core count. What this constant covers is the flat term below.
///
/// **What it does not cover, and never had to, is the block pool's floor.**
/// [`BufferPool::slots`] clamps that pool at `POOL_DEPTH.max(jobs)` while the
/// per-reader term bills `2 × unit`, so below four readers the pool holds
/// `(POOL_DEPTH − jobs) × unit` that no per-reader term carries — unbounded in
/// the block size, which is why it is billed by [`WorkerMemory`] rather than
/// reserved for here
/// (`docs/design/architecture.md`, "Execution model and API surface").
///
/// **One constant, taken from the compressed leg, over-reserving the plain
/// path by roughly the difference.** The two paths' fixed terms are almost two
/// orders of magnitude apart — a plain `parse` holds 5.86 MiB above its pool
/// where a block-decoding one holds a few hundred megabytes — and a per-source
/// reserve
/// is not available where the number is needed: [`Parallelism::discover`] is
/// the primitive a caller reaches with nothing open, and a source's own answer
/// is downstream of recognition, which is I/O. Over-reserving is the safe
/// direction, and a caller who wants a plain scan's real headroom states a
/// budget.
///
/// **Sizing it from the arena count is refused, and the count is not the free
/// variable it looks like.** glibc's own ceiling is `8 × ncores` — 192 on a
/// 24-core host — and a scan never approaches it: arenas run at `readers + 2`
/// (6/15/24/26 observed at 4/13/22/24 readers), so the thread count this crate
/// chooses is what binds and the maximum never is. There is no getter for
/// `M_ARENA_MAX` to read in any case, and *setting* it is refused separately
/// and for a mechanical reason
/// (`docs/design/roadmap-P19-efficient-defaults.md`, "The arena cap is a
/// deployment setting, not a mechanism this binary ships"). The deeper reason
/// the arithmetic would not help: retention is **already inside the per-reader
/// charge** — the measured worst-resident slope is 0.987 of what
/// [`BlockCache::reader_bytes`] bills — so a reserve computed from an arena
/// count would be reserving for a term that is billed twice
/// (`docs/design/roadmap-P19.16-reserve-constant-notes.md`, "Arena retention is
/// inside the charge, not above it").
///
/// **Below it the budget goes to zero rather than to a floor.** A limit under
/// this leaves nothing, and the three floors already in the mechanism —
/// `crate::stream::worker_count`'s `.max(1)`, [`BufferPool::slots`]' clamp to
/// one, and [`BlockCache::affordable`] refusing block decode — make that one
/// reader's worth on the streaming path. Reasserting
/// [`DEFAULT_MEMORY_BUDGET`] there would put today's constant back under a new
/// name in the one case discovery exists for.
pub const MEMORY_RESERVE: u64 = 384 << 20;

/// How much of a **discovered** memory limit a resolved arrangement must leave
/// unused, as a percentage of the limit — the criterion [`MEMORY_RESERVE`] was
/// chosen against, enforced on the worker count instead of being left to hold
/// by accident.
///
/// **Twenty percent, and it is this phase's judgement rather than a derived
/// number** (`docs/design/roadmap-P19-efficient-defaults.md`, "The margin is
/// now stated"). A cgroup's killer reads one run's peak, so the criterion is
/// about the worst rep and not the median; ten reps is the estimate of a tail
/// this project can make.
///
/// **A constant reserve leaves constant headroom, and the criterion is a
/// fraction — so the two only agree at one limit.** `limit − MEMORY_RESERVE`
/// aims resident *at* the limit by construction, leaving roughly
/// `MEMORY_RESERVE` minus what a scan holds outside its pools however large
/// the limit is; as a *share* of the limit that shrinks, so the criterion is
/// met at a 1 GiB limit and not at 2 GiB. It looked met at 2 GiB only because
/// `std::thread::available_parallelism` on a 24-core host clamped the count
/// below what the allowance afforded — a 64-core host resolves twenty-eight
/// readers of a 24 MiB-block file there and is predicted to breach at 13.3%.
/// Enforcing the criterion on the count is what makes the answer a property of
/// the allocation rather than of the host's width.
///
/// **Enforced once, against [`MEMORY_UNPOOLED_BOUND`].** The predicted
/// resident a count is held to is `WorkerMemory::at(n)` plus that bound, not
/// plus [`MEMORY_RESERVE`]: the reserve is the smallest constant meeting this
/// criterion under the *cap* rule, so it decomposes as `0.2 × 1 GiB +
/// 178.9 MiB` and predicting with it subtracted the margin a second time, at a
/// cost of exactly `0.2 × limit` per limit
/// (`docs/design/architecture.md`, "Execution model and API surface").
///
/// **So it binds above `5 × (MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND)` and
/// nowhere below**, which is 640 MiB at today's two constants: under that the
/// cap `limit − MEMORY_RESERVE` is the tighter of the two conditions and this
/// one is inert. That is the intended shape — a constant reserve leaves a
/// shrinking *share* as the limit grows, so the large end is the end that
/// needed a fraction.
///
/// **It bounds the count and never the budget, and it cannot decline a
/// reader.** [`Parallelism::fit`] keeps one worker at whatever the cap is —
/// the arrangement below the reserve is real and the mechanism's own floors
/// deliver it — and reports the budget that count spends, capped by
/// `limit − MEMORY_RESERVE` as before. So a margin the smallest arrangement
/// cannot meet costs nothing, and [`BlockCache::affordable`] reads the same
/// budget it always did.
///
/// *Rejected: a fractional **budget** ceiling*, which is a different rule that
/// this one must not be read as reinstating. That one asserted "do not take an
/// allocation we cannot show we use" and was dropped because
/// [`BufferPool::slots`]' clamp already proves it: resident saturates at
/// `jobs × C` and every byte above is inert
/// (`docs/design/architecture.md`, "Execution model and API surface"). This
/// asserts headroom against a killer, binds where that one never did — at
/// large limits, where the count is what grows — and is checked against a
/// stated criterion rather than against a hedge.
pub const MEMORY_MARGIN_PERCENT: u64 = 20;

/// What a scan holds resident **outside the pools the budget bills**, bounded:
/// the runtime's threads, glibc's per-thread arena retention, the decoder
/// state a compressed source keeps beyond [`BlockCache::reader_bytes`], and
/// the binary itself. [`margin_allowance`] predicts with it, and nothing else
/// reads it.
///
/// **A bound, not a term, and it is read off a grid rather than fitted.**
/// `19.16`'s four hundred runs — five reserve constants over two block sizes
/// and four container limits, ten reps each — are re-read under today's charge
/// ([`WorkerMemory::at`]), and the unnamed remainder `held − at(jobs)` over
/// every surviving block-path leg runs **83.5–214.6 MiB** on a 24 MiB-block
/// file and **10.9–13.8 MiB** on a 128 MiB-block one. 256 MiB is the worst of
/// those rounded up to a 64 MiB step
/// (`docs/design/roadmap-P19.26-margin-constant-notes.md`).
///
/// **What justifies the value is the criterion's slack, not the rounding.**
/// What this crate promises is [`MEMORY_MARGIN_PERCENT`]; the bound is an input
/// to it, so what matters is the remainder at which the promise breaks —
/// `0.8 × limit − at(n)`, which at the four 24 MiB-block allocations the margin
/// governs is 296.9, 290.1, 300.3 and 303.6 MiB. The true remainder would have
/// to run 75–89 MiB above the worst of 270 legs, 35–41% above anything
/// measured, before an arrangement leaves less than a fifth of its limit, and
/// 2.3–3.3× above it before the allocation is exhausted.
///
/// *Rejected: 320 MiB.* It buys 2.8 points of headroom against the worst
/// measured remainder at 2 GiB and makes its own prediction 0.3 points worse
/// there, the step being 64 MiB where the reader it removes returns 58.03. It
/// costs a reader at each of `1g`, `1088m`, `1536m` and `2g` at 24 MiB blocks
/// and at `1536m` and `2g` at 128, where the remainder is 13.8 MiB. And since
/// the bound is subtracted, its weight grows as the limit falls: the crossover
/// drops to 320 MiB, so the margin binds across the band it exists to leave
/// alone — 4 readers to 2 at `640m`, 3 to 1 at `600m`, 2 to 1 at `576m`.
///
/// *Rejected: 224 MiB.* It costs the 128 MiB family nothing and buys a reader
/// at every 24 MiB-block allocation, its 2 GiB leg landing on the 24 readers
/// `19.16` measured directly rather than the 23 that is only bracketed — but
/// the criterion's slack falls to 17–31 MiB, inside that apparatus's own
/// scatter, and its `1536m` leg resolves the seventeen readers at which the
/// 214.6 MiB worst remainder was observed.
///
/// **It does not scale with the reader count, and it is *smaller* where the
/// blocks are larger** — the opposite of a per-reader term, which is why the
/// whole of it is carried as a constant rather than divided by anything. The
/// remainder wanders inside its range with no trend across three to
/// twenty-four readers.
///
/// **What it is, is unattributed.** As far as any reading here goes it is
/// glibc's arena retention (`19.18`); no term table sums to it, and the
/// harness's own account says so in those words. `scripts/measure.py`'s
/// `charge_model_problem` faults a cell whose remainder exceeds this, which is
/// the finding that would move it — and separately at [`MEMORY_RESERVE`],
/// which is the rule failing rather than the bound being low.
///
/// **Why it is not [`MEMORY_RESERVE`], which used to stand here.** That
/// constant is the smallest meeting [`MEMORY_MARGIN_PERCENT`] under the *cap*
/// rule, so it already contains `0.2 × 1 GiB`; predicting with it enforced the
/// criterion twice and cost `0.2 × limit` at every limit — 11→7 readers at
/// 1 GiB where 9 is what the criterion alone asks for. The two numbers are
/// separate because they answer separate questions: the reserve is what a
/// discovered limit hands back before anything is spent, and this is what the
/// arrangement is predicted to hold on top of what it spends.
pub const MEMORY_UNPOOLED_BOUND: u64 = 256 << 20;

/// The most a resolved arrangement may be **charged** under a discovered
/// `limit` if its predicted resident is to leave [`MEMORY_MARGIN_PERCENT`] of
/// that limit unused: `(100 − margin)% of limit`, less
/// [`MEMORY_UNPOOLED_BOUND`].
///
/// **The predicted resident is `charge + MEMORY_UNPOOLED_BOUND`**, that
/// constant being what bounds the one term the charge does not bill. Using
/// [`MEMORY_RESERVE`] here instead applied [`MEMORY_MARGIN_PERCENT`] twice,
/// the reserve being the smallest constant meeting that criterion under the
/// cap rule (`docs/design/architecture.md`, "Execution model and API
/// surface").
///
/// Integer arithmetic, rounding **down** the fraction of the limit so the
/// allowance errs small.
fn margin_allowance(limit: u64) -> u64 {
    (limit / 100).saturating_mul(100 - MEMORY_MARGIN_PERCENT).saturating_sub(MEMORY_UNPOOLED_BOUND)
}

/// A cgroup v1 `memory.limit_in_bytes` at or above this reads as *no limit*
/// (`docs/design/runtime-invariants.md`, `RT4`).
///
/// **A threshold, never an equality test.** The unset value is
/// `PAGE_COUNTER_MAX × PAGE_SIZE`, which is a function of the page size and
/// the word width: 9223372036854771712 at 4 KiB pages, 9223372036854759424 at
/// 16 KiB, 9223372036854710272 at 64 KiB, and 8796093018112 on a 32-bit
/// kernel. A threshold is correct at every one of them, where a constant
/// matches one — and 4 TiB is under the smallest of the four while being more
/// memory than a cgroup on this century's hardware is given.
///
/// It is applied to the v2 files too, where it is very nearly unreachable:
/// `memory.max` spells "no limit" as the string `max` (`RT2`), so a *stated*
/// v2 limit above 4 TiB is the only thing this could misread, and reading one
/// as unlimited falls back to the [`available_memory`] cap, which on such a
/// machine is the smaller number anyway.
const NO_LIMIT_AT_OR_ABOVE: u64 = 1 << 42;

/// Which cgroup hierarchy states this process's memory limit, and where in it
/// the process sits (`docs/design/runtime-invariants.md`, `RT6`).
enum MemoryHierarchy<'a> {
    /// A v1 hierarchy carrying the `memory` controller, at this path within
    /// that hierarchy's own mount.
    V1(&'a str),
    /// The unified hierarchy, at this path under the v2 mount.
    V2(&'a str),
    /// `/proc/self/cgroup` named neither — no cgroup governs this process, or
    /// the file is not a Linux one.
    None,
}

/// Which of the two file shapes to read, from the body of
/// `/proc/self/cgroup`.
///
/// **A controller lives in exactly one hierarchy** (`RT6`), so a line whose
/// controller list contains `memory` settles it outright and the `0::` line is
/// only consulted when no such line exists. Three parse hazards from `RT1` are
/// handled here and nowhere else: the v2 line is selected **by its shape** —
/// an empty controller field, which is what `std` matches for the CPU quota —
/// and never by position or by hierarchy id, which `idr_alloc_cyclic` hands
/// out cyclically; the controller field is matched by **membership** in a
/// comma-separated list, since a v1 hierarchy carrying only a name reads
/// `name=memory` and holds no memory controller at all; and a dead cgroup's
/// path carries a ` (deleted)` suffix that would otherwise be joined onto the
/// mount point as part of a directory name.
fn memory_hierarchy(cgroups: &str) -> MemoryHierarchy<'_> {
    let mut unified = None;
    for line in cgroups.lines() {
        let mut fields = line.splitn(3, ':');
        let (Some(_id), Some(controllers), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let path = path.strip_suffix(" (deleted)").unwrap_or(path);
        if controllers.is_empty() {
            unified.get_or_insert(path);
        } else if controllers.split(',').any(|c| c == "memory") {
            return MemoryHierarchy::V1(path);
        }
    }
    unified.map_or(MemoryHierarchy::None, MemoryHierarchy::V2)
}

/// Where the v1 hierarchy carrying the `memory` controller is mounted, read
/// from `/proc/self/mountinfo` — the authority when the conventional
/// `/sys/fs/cgroup/memory` is not where it is (`RT1`).
///
/// A line's mount point is its fifth space-separated field; the filesystem
/// type and the super options follow the ` - ` separator, and the options are
/// where a v1 cgroup mount names its controllers. Falls back to the
/// convention when the file cannot be read or lists no such mount, which is
/// also what a fixture tree that states only the cgroup path gets.
///
/// **Mountinfo's `\040` escaping of spaces is not undone**, which is the same
/// limit `std` states for its own reading of this file: a cgroup mounted under
/// a path containing a space is not found, and the fallback applies.
fn v1_memory_mount(root: &Path) -> PathBuf {
    let conventional = root.join("sys/fs/cgroup/memory");
    let Ok(text) = std::fs::read_to_string(root.join("proc/self/mountinfo")) else {
        return conventional;
    };
    for line in text.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        let mut tail = after.split_whitespace();
        if tail.next() != Some("cgroup") {
            continue;
        }
        let _source = tail.next();
        if !tail.next().unwrap_or("").split(',').any(|o| o == "memory") {
            continue;
        }
        if let Some(point) = before.split_whitespace().nth(4) {
            return root.join(point.trim_start_matches('/'));
        }
    }
    conventional
}

/// One limit file's value in bytes, or `None` where it states no limit — the
/// literal `max` of a v2 file (`RT2`, `RT3`), a v1 value at
/// [`NO_LIMIT_AT_OR_ABOVE`] (`RT4`), or a file that is not there at all.
///
/// **Absent is "no limit here", not an error** (`RT5`): the hierarchy root
/// carries no `memory.max` at all, and a cgroup whose parent has not enabled
/// the controller in `cgroup.subtree_control` carries none either.
fn read_limit_file(path: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(path).ok()?;
    let value = text.trim().parse::<u64>().ok()?;
    (value < NO_LIMIT_AT_OR_ABOVE).then_some(value)
}

/// The smallest limit any of `files` states at `cgroup_path` or at any
/// ancestor of it up to `mount` (`docs/design/runtime-invariants.md`, `RT5`).
///
/// **Every ancestor, and the two v2 files minimised together.** A limit set on
/// an ancestor binds a process in a descendant and is invisible in the
/// descendant's own file, so a reader that consults only the leaf believes an
/// unlimited process is unlimited when it is not; and nothing orders
/// `memory.high` against `memory.max`, across levels or within one — a
/// `memory.high` two levels up may be the smallest number in the walk.
/// `memory.high` is read at all because it throttles where `memory.max` kills
/// (`RT3`), and sustained reclaim ends a scan's throughput as surely as an OOM
/// ends the run.
///
/// **The walk is bounded by the mount point, not by counting separators.**
/// Inside a cgroup namespace the namespace root is what is mounted, so walking
/// up from the `0::` path never leaves it; limits set outside the namespace
/// still bind and are simply not readable, which is a property of the
/// environment rather than a defect to work around.
fn smallest_limit(mount: &Path, cgroup_path: &str, files: &[&str]) -> Option<MemoryLimit> {
    let mut smallest: Option<MemoryLimit> = None;
    let mut rel = cgroup_path.trim_start_matches('/');
    loop {
        let dir = if rel.is_empty() { mount.to_path_buf() } else { mount.join(rel) };
        for file in files {
            let at = dir.join(file);
            if let Some(bytes) = read_limit_file(&at)
                && smallest.as_ref().is_none_or(|seen| bytes < seen.bytes)
            {
                smallest = Some(MemoryLimit { bytes, read_from: at });
            }
        }
        match rel.rfind('/') {
            Some(at) => rel = &rel[..at],
            None if rel.is_empty() => break,
            None => rel = "",
        }
    }
    smallest
}

/// A memory limit this process is running under, and the file that states it
/// (`docs/design/architecture.md`, "Execution model and API surface").
///
/// **The path is carried because the two v2 files do different things and the
/// walk minimises over both.** `memory.high` throttles where `memory.max`
/// kills (`RT3`), and either may be stated on an ancestor rather than on the
/// leaf (`RT5`) — so "your budget was cut to 280 MiB" is only actionable
/// beside the file whose number did the cutting. It is a display fact: nothing
/// in the library branches on it, and a status line is its one consumer
/// (`docs/design/architecture.md`, "Status output").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryLimit {
    /// The smallest limit that binds, in bytes — the minimum over every file
    /// and every ancestor cgroup.
    pub bytes: u64,
    /// The file that stated it, as reached: under
    /// [`discover_memory_limit_in`]'s root rather than absolute, so a fixture
    /// tree's answer names the fixture.
    pub read_from: PathBuf,
}

/// The memory limit this process is actually running under, in bytes, or
/// `None` where nothing limits it
/// (`docs/design/architecture.md`, "Execution model and API surface").
///
/// **A primitive, not a policy.** It reports what the environment states and
/// makes nothing of it: what a caller may usefully take from a limit is
/// [`Parallelism::discover`], which is this composed with
/// `std::thread::available_parallelism` through [`MEMORY_RESERVE`]. The split
/// is for the embedder who has already made half their allocation decision —
/// the case that keeps this library from spawning threads or claiming bytes
/// nobody asked it for.
///
/// **`None` means no limit is being *enforced*, which is a complete and
/// checkable statement** — unlike "are we in a container", which no reading of
/// `/.dockerenv` or `/proc/self/cgroup` answers on every runtime. What binds a
/// process is the limit, not the namespace.
///
/// The reading is `RT1`–`RT6` of
/// [`docs/design/runtime-invariants.md`](../../../docs/design/runtime-invariants.md):
/// which hierarchy owns the `memory` controller, that hierarchy's files
/// (`memory.max` and `memory.high` on v2, `memory.limit_in_bytes` on v1), and
/// the minimum over every ancestor cgroup rather than the nearest one.
pub fn discover_memory_limit() -> Option<MemoryLimit> {
    discover_memory_limit_in(Path::new("/"))
}

/// [`discover_memory_limit`] against an arbitrary filesystem root.
///
/// **Public because the arms a test must drive are the ones no machine has
/// both of.** This is the seam a fixture tree is handed — here, and through
/// [`Parallelism::discover_in`] and [`available_memory_in`], which take one
/// for the same reason — so a caller's own resolution can be pinned against a
/// v1 hierarchy, an unlimited one and a below-reserve one on a machine that is
/// none of the three (`pgdump_query-cli/tests/data/runtime/`). It is not a
/// chroot facility: paths are joined onto the root, so a root of `/` is the
/// real reading and anything else is a tree somebody built.
///
/// **The root is a parameter so that the v1 arm can be executed at all.** This
/// project's machines run a pure v2 unified hierarchy, and the memory
/// controller lives in exactly one hierarchy at a time (`RT6`), so a v1 shape
/// cannot be produced beside it — without the seam the v1 branch would ship
/// never having run. What a fixture tree establishes is that this reader
/// handles the shape `RT4` describes, not that a kernel still produces it,
/// which only a v1 host can say.
pub fn discover_memory_limit_in(root: &Path) -> Option<MemoryLimit> {
    let cgroups = std::fs::read_to_string(root.join("proc/self/cgroup")).ok()?;
    match memory_hierarchy(&cgroups) {
        MemoryHierarchy::V1(path) => {
            smallest_limit(&v1_memory_mount(root), path, &["memory.limit_in_bytes"])
        }
        MemoryHierarchy::V2(path) => {
            smallest_limit(&root.join("sys/fs/cgroup"), path, &["memory.max", "memory.high"])
        }
        MemoryHierarchy::None => None,
    }
}

/// The kernel's own estimate of the memory obtainable without swapping, in
/// bytes — `/proc/meminfo`'s `MemAvailable`
/// (`docs/design/runtime-invariants.md`, `RT8`).
///
/// **It is the host's number even inside a container**, so it is meaningful
/// only once [`discover_memory_limit`] has answered `None`: consulted first it
/// would read tens of gigabytes against a half-gigabyte allocation.
///
/// **`MemAvailable` and not `MemFree`.** `MemFree` excludes reclaimable page
/// cache and reads several times lower on any machine that has read a large
/// file, so planning against it would throttle a scan for memory the kernel
/// would hand straight back.
///
/// **A ceiling to plan under, never a reservation.** It is an estimate, it
/// moves second to second, and two processes reading it at once each see the
/// whole of it — which is why the caller takes half.
pub fn available_memory() -> Option<u64> {
    available_memory_in(Path::new("/"))
}

/// [`available_memory`] against an arbitrary filesystem root, for the same
/// reason [`discover_memory_limit_in`] takes one: the no-limit branch is
/// otherwise only exercisable on a machine that has no limit.
pub fn available_memory_in(root: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(root.join("proc/meminfo")).ok()?;
    let line = text.lines().find_map(|line| line.strip_prefix("MemAvailable:"))?;
    let mut fields = line.split_whitespace();
    let value = fields.next()?.parse::<u64>().ok()?;
    match fields.next() {
        // `meminfo_proc_show` prints every size in kB and the unit is part of
        // the line; a bare number is accepted so that a fixture need not
        // repeat it.
        Some("kB") => Some(value.saturating_mul(1024)),
        None => Some(value),
        Some(_) => None,
    }
}

/// How deep a [`BufferPool`] would like to be, in slots.
///
/// A scan holds one chunk at a time, so one slot would serve it; the query
/// replay path retains chunks past the read that produced them
/// (`crate::batch::RetainedChunks`), so the buffer of chunk *N* can still be
/// alive when chunk *N+1* is read. Four is that depth with room to spare.
///
/// **It is a wish, not the bound** — the stated budget is the bound, and the
/// two together are what make a slot able to hold a decoded xz block rather
/// than only a read chunk (`docs/design/architecture.md`, "Execution model and
/// API surface").
///
/// **It is also the floor under a stated worker count**, not a second number
/// beside it: a pool told to keep one block per concurrent reader still keeps
/// this many when the reader is serial, because a block pool of one makes
/// eviction drain before every decode and stops pooling exactly when a caller
/// is holding a block ("The compressed source").
const POOL_DEPTH: usize = 4;

/// The largest buffer worth keeping, in bytes, for a length nobody has
/// announced as a read size.
///
/// The read path's steady state is chunk-sized — 1 MiB by default, and
/// tunable through `ScanOptions::chunk_size`. What can be far larger is
/// `crate::map::attach_text`'s coalesced span read, which happens once per
/// map and never again; holding one of those for the rest of a process would
/// trade the ~5.9 MiB a scan holds resident (`docs/design/measurements.md`,
/// "What a scan holds resident") for an allocation nothing is going to ask for
/// twice.
///
/// **Size stands in for one-off-ness, and the caller is what overrides it.** A
/// chunk buffer at the configured size is asked for once per chunk for the
/// whole scan — the thing the pool is for — and by length alone it is
/// indistinguishable from a span read, so this ceiling on its own would turn
/// pooling off for any chunk above it. What tells the two apart is the caller
/// saying so: [`ByteRangeSource::hint_read_size`] names the length a read loop
/// is about to repeat, and *becomes* the pool's slot size
/// ([`BufferPool::slot_bytes`]), so a buffer of that length is kept however
/// large it is and this constant governs only a pool nobody has announced to
/// (`docs/design/architecture.md`, "Execution model and API surface").
const POOL_MAX_BYTES: usize = 8 << 20;

/// How many read chunks a plain file's partition holds
/// ([`LocalFileSource::partitions`]).
///
/// **A partition is not one chunk, because a worker reads its piece and then
/// one chunk more.** `crate::leader::scan_partition` reads `[start, end)` and
/// cannot finish there — the line ending at or past `end` needs bytes past
/// `end` — so a chunk-sized tail read follows, and those bytes are the next
/// partition's body read a second time. The waste is therefore
/// `chunk / partition`: a partition of exactly one chunk reads the whole file
/// **twice**, which is what a plain `--jobs 2` `parse` did, flat in the worker
/// count because it is charged per partition
/// (`docs/design/architecture.md`, "The interior split"). Eight caps it at
/// 12.5% and is where the returns flatten — the tail is one chunk however
/// large the partition is, so each doubling past this buys less than a
/// percent of the read while it halves how finely a region can be cut and how
/// many readers a stated budget affords.
///
/// **The product is capped at [`POOL_MAX_BYTES`], which bounds what one worker
/// allocates and no longer buys it any pooling.** [`BufferPool`] serves
/// exactly one read unit ([`ByteRangeSource::hint_read_size`]) and the parallel
/// plain path has two — this partition read and `scan_partition`'s chunk-sized
/// tail read — so the partition read is not the announced one and
/// [`BufferPool::keeps`] drops it on release whatever its size: a worker pays
/// a fresh `calloc` per partition, and the cap is what keeps that allocation
/// from following a raised `ScanOptions::chunk_size` to hundreds of megabytes.
/// It cost pooling before `19.7` too — the free list held these buffers only
/// because the pool was under-reporting them eightfold, which is the
/// accounting that change repaired. What the cap costs is that the multiple
/// shrinks as the announced chunk grows, reaching **one** at
/// [`POOL_MAX_BYTES`] and above — the double read this constant exists to
/// remove, returned to the caller who raised `ScanOptions::chunk_size`. The
/// fix for both is the two-unit arrangement [`XzSource`] already runs; see
/// `docs/design/architecture.md`, "The interior split".
///
/// **The shipped default sits exactly on the cap** — 1 MiB × 8 is
/// [`POOL_MAX_BYTES`] — and the two constants are justified independently, so
/// nothing but [`a_shipped_plain_partition_is_eight_whole_chunks`] stops a
/// later change to either from silently capping the default configuration.
const PLAIN_PARTITION_CHUNKS: usize = 8;

/// Read buffers, reused rather than allocated per chunk.
///
/// **What this is for is not allocator throughput.** A fresh `vec![0u8; len]`
/// per chunk is `calloc`, so the kernel or the allocator zeroes a megabyte
/// that `read_exact_at` immediately overwrites — 23.2% of a warm `parse`'s
/// user time on the 3.00 GiB control
/// (`docs/design/architecture.md`, "parse-profile"). It also hands the region
/// back to the allocator once per chunk, which is what put `jemalloc` at 3,161
/// `madvise` calls against glibc's 50 over the same file
/// (`docs/design/measurements.md`, "Which allocator a figure was taken
/// under"). Neither cost is work the problem requires: the same megabyte is
/// wanted again a moment later.
///
/// **A pooled buffer is fully initialized and stays at its own length.** It
/// is created once as `vec![0u8; len]` and thereafter only ever read into, so
/// no reuse memsets anything. A read shorter than the buffer takes the
/// buffer whole and the `Bytes` handed back is sliced down to the bytes that
/// were actually read, which is why a short final chunk does not shrink a
/// pooled buffer and force the next full chunk to grow one back.
///
/// **Whether an acquisition blocks is the caller's permission, not the pool's
/// policy.** [`BufferPool::obtain`] waits for a free slot where the read loop
/// granted [`WaitPolicy::MayWait`] — which is what turns
/// [`BufferPool::slots`] into a bound on what is *outstanding* rather than
/// only on what is idle — and allocates past the budget where it granted
/// [`WaitPolicy::NeverWait`], which is what a loop pinning buffers into
/// batches must grant, since a wait there deadlocks
/// (`docs/design/architecture.md`, "Execution model and API surface").
/// [`BufferPool::take`] is the unwaiting primitive underneath both.
#[derive(Debug)]
struct BufferPool {
    /// The free list and the count of charged buffers outstanding, under
    /// one lock because [`BufferPool::returned`] is what a waiting acquisition
    /// blocks on and a condvar needs the state it waits on beside it.
    ///
    /// A poisoned lock is not a corruption hazard here — the only thing under
    /// it is a list of scratch buffers and a counter — so every caller
    /// recovers the guard rather than propagating a panic from an unrelated
    /// task.
    state: Mutex<PoolState>,
    /// Signalled whenever a charged buffer comes back, which is the only event
    /// that can let a waiting acquisition through.
    returned: Condvar,
    /// What this pool may hold in free buffers at once, in bytes:
    /// [`DEFAULT_MEMORY_BUDGET`] until a caller states one through
    /// [`ByteRangeSource::hint_parallelism`].
    ///
    /// **The pool is bounded in bytes because a block pool cannot be bounded
    /// in slots.** [`POOL_DEPTH`] slots of a decoded 128 MiB xz block is
    /// 512 MiB — the whole cgroup the measurements run in — so a pool whose
    /// slot size is free and whose count is fixed states no bound at all. The
    /// byte budget makes the count the consequence ([`BufferPool::slots`]) and
    /// the memory the number a caller states, which is the currency an RSS
    /// claim is made in.
    ///
    /// **It bounds the free list at or below its own slot size, and not above
    /// it.** [`BufferPool::slots`] clamps to at least one, so a unit larger
    /// than the budget still gets a slot at that unit's size; a pool that can
    /// hold nothing is not a pool. What keeps that floor from making the
    /// budget a fiction is that the one unit which can exceed it — a decoded
    /// xz block — is refused *before* a slot is asked for
    /// ([`BlockCache::affordable`]).
    ///
    /// `Relaxed` for the same reason [`BufferPool::hinted`] is: it is written
    /// once per read loop and read once per sizing decision, nothing is
    /// published through it, and a value that arrives a buffer late costs one
    /// allocation.
    budget: AtomicUsize,
    /// The ceiling on the free list in slots, whatever the budget affords:
    /// [`POOL_DEPTH`] until a caller states a worker count, and then one slot
    /// per concurrent reader.
    depth: AtomicUsize,
    /// How many of [`BufferPool::slots`] are held by a holder outside the free
    /// list — `0` for every pool but [`BlockCache`]'s, whose retained blocks
    /// own slots of this pool's own unit ([`BufferPool::reserve`]).
    reserved: AtomicUsize,
    /// The read length a caller announced through
    /// [`ByteRangeSource::hint_read_size`], or `0` for none — the pool's slot
    /// size, and a buffer of exactly this length is kept past
    /// [`POOL_MAX_BYTES`].
    ///
    /// It is an atomic rather than part of the `Mutex` because it is written
    /// once per read loop and read once per released buffer, and because
    /// `hint_read_size` takes `&self`: a source is shared, and the announcement
    /// must not have to wait behind a `take` on another task. `Relaxed` is
    /// enough — nothing is published through it, and a hint that arrives a
    /// buffer late costs one allocation.
    ///
    /// **One value, so one pool describes one read unit.** It drives both
    /// [`BufferPool::keeps`] and [`BufferPool::slot_bytes`], and a pool asked
    /// to serve two units is wrong for one of them either way: at the smaller
    /// unit the larger buffer is dropped on release, and at the larger one the
    /// slot count collapses under the smaller reads. A source that acquires a
    /// second read unit takes a second pool rather than a second hint
    /// (`docs/design/architecture.md`, "The compressed source").
    hinted: AtomicUsize,
    /// What the read loop currently reading through this pool permits, as
    /// [`WaitPolicy`] encodes it — [`WaitPolicy::NeverWait`] until one is
    /// granted, so a pool nobody has spoken to never waits.
    ///
    /// `Relaxed` for the same reason [`BufferPool::hinted`] is: it is written
    /// once per read loop, nothing is published through it, and a value that
    /// arrives a buffer late costs one allocation. It is read *inside* the
    /// state lock by [`BufferPool::obtain`], so a policy that changes between
    /// the read and the wait cannot leave a charge unmatched — the charge is
    /// carried by the buffer, not re-derived at release.
    policy: AtomicUsize,
}

/// What a [`BufferPool`] holds under its lock.
#[derive(Debug, Default)]
struct PoolState {
    /// Buffers nobody is using, at most [`BufferPool::slots`] of them.
    free: Vec<Vec<u8>>,
    /// Buffers handed to a [`WaitPolicy::MayWait`] loop and not yet
    /// returned — the term a waiting acquisition is bounded by, and the only
    /// one of the two the library sets. What in-flight batches pin is the
    /// caller's and is deliberately not counted here
    /// (`docs/design/architecture.md`, "Execution model and API surface").
    charged: usize,
}

impl Default for BufferPool {
    fn default() -> Self {
        Self {
            state: Mutex::new(PoolState::default()),
            returned: Condvar::new(),
            budget: AtomicUsize::new(DEFAULT_MEMORY_BUDGET as usize),
            depth: AtomicUsize::new(POOL_DEPTH),
            reserved: AtomicUsize::new(0),
            hinted: AtomicUsize::new(0),
            policy: AtomicUsize::new(WaitPolicy::NeverWait as usize),
        }
    }
}

impl BufferPool {
    /// The smallest free buffer that fits `len`, or a fresh allocation —
    /// taken under the guard [`BufferPool::obtain`] is already holding, so
    /// that a waiting acquisition's wait and its take are one critical
    /// section.
    fn pick(state: &mut PoolState, len: usize) -> Vec<u8> {
        let pick = state
            .free
            .iter()
            .enumerate()
            .filter(|(_, buf)| buf.len() >= len)
            .min_by_key(|(_, buf)| buf.len())
            .map(|(i, _)| i);
        match pick {
            Some(i) => state.free.swap_remove(i),
            None => vec![0u8; len],
        }
    }

    /// The buffer for one read, waiting or allocating according to what the
    /// read loop permitted.
    ///
    /// [`WaitPolicy::MayWait`] blocks while [`BufferPool::slots`] buffers
    /// taken under that permission are already out, so the pool's slot count
    /// bounds what is outstanding; the buffer carries the charge, so the
    /// matching release cannot be lost to a policy granted in between.
    /// [`WaitPolicy::NeverWait`] takes the same buffer without blocking and
    /// without charging.
    ///
    /// **The wait is safe only because a waiting loop holds one buffer.**
    /// A caller granting that permission and then keeping two reads alive at
    /// once against a one-slot pool blocks forever; that is the promise
    /// [`ByteRangeSource::hint_wait_policy`] documents.
    fn obtain(self: &Arc<Self>, len: usize) -> PooledBuffer {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let charged = self.policy() == WaitPolicy::MayWait;
        if charged {
            // Re-read the ceiling every pass: `set_limits` may raise it while
            // this holder waits, and a wait against a stale number would
            // outlast the condition that caused it.
            while state.charged >= self.slots() {
                state = self.returned.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            state.charged += 1;
        }
        let buf = Self::pick(&mut state, len);
        drop(state);
        PooledBuffer { buf: Some(buf), pool: Arc::clone(self), charged }
    }

    /// Announce the read length a loop is about to repeat, so a buffer of
    /// that length survives [`POOL_MAX_BYTES`] and the pool's slot size is
    /// the unit that loop retains.
    fn hint(&self, len: usize) {
        self.hinted.store(len, Ordering::Relaxed);
    }

    /// The pool's slot size: the unit a read loop announced, or
    /// [`POOL_MAX_BYTES`] where nothing has been announced — which is the
    /// largest buffer [`BufferPool::keeps`] will take in that case, so the
    /// two rules bound the same thing.
    fn slot_bytes(&self) -> usize {
        match self.hinted.load(Ordering::Relaxed) {
            0 => POOL_MAX_BYTES,
            len => len,
        }
    }

    /// The unit a read loop announced, or `None` where none has — the same
    /// state [`BufferPool::slot_bytes`] reads, with the absence still an
    /// absence.
    ///
    /// It exists because a caller pricing a *reader* and a pool sizing a
    /// *slot* want different answers to "nobody has said": the pool's own
    /// question is answered by [`POOL_MAX_BYTES`], the largest buffer it will
    /// keep, and a charge divided into a memory allowance wants the chunk the
    /// scan is about to announce ([`XzSource::charged_chunk_bytes`]).
    fn announced_bytes(&self) -> Option<usize> {
        match self.hinted.load(Ordering::Relaxed) {
            0 => None,
            len => Some(len),
        }
    }

    /// The budget this pool is sizing itself against, in bytes.
    fn budget(&self) -> usize {
        self.budget.load(Ordering::Relaxed)
    }

    /// What the loop currently reading through this pool permits.
    fn policy(&self) -> WaitPolicy {
        match self.policy.load(Ordering::Relaxed) {
            n if n == WaitPolicy::MayWait as usize => WaitPolicy::MayWait,
            _ => WaitPolicy::NeverWait,
        }
    }

    /// State what the read loop about to start permits.
    ///
    /// It governs acquisitions made *after* it: a buffer already out keeps the
    /// charge it was taken with, which is what makes the two policies safe to
    /// alternate across a query's mapping and replay passes.
    fn set_policy(&self, policy: WaitPolicy) {
        self.policy.store(policy as usize, Ordering::Relaxed);
    }

    /// State the budget and the slot ceiling this pool is to work inside.
    ///
    /// Both at once, because they are two halves of one sizing decision and a
    /// pool caught between an old depth and a new budget would report a count
    /// neither caller asked for.
    /// A raised ceiling wakes every waiter, since one of them may now fit
    /// under it — the only other thing that can release a waiting acquisition
    /// is a charged buffer coming back.
    fn set_limits(&self, budget: usize, depth: usize) {
        self.budget.store(budget, Ordering::Relaxed);
        self.depth.store(depth.max(1), Ordering::Relaxed);
        self.returned.notify_all();
    }

    /// How many free buffers this pool holds: its depth where the budget
    /// affords it, fewer where a slot is large, never zero.
    ///
    /// **This is what makes the pool block-capable.** At the 1 MiB default
    /// chunk it is 4, exactly the fixed count it once had; at a decoded
    /// 128 MiB xz block it is 1, where four would have been the whole cgroup.
    fn slots(&self) -> usize {
        let depth = self.depth.load(Ordering::Relaxed).max(1);
        (self.budget() / self.slot_bytes()).clamp(1, depth)
    }

    /// How many of those slots the free list may take: the rest are held by a
    /// reserving holder that accounts for them itself
    /// ([`BufferPool::reserve`]).
    fn free_slots(&self) -> usize {
        self.slots().saturating_sub(self.reserved.load(Ordering::Relaxed))
    }

    /// The most this pool can hold at once, in bytes: its slot count at its
    /// own slot size, counting the free list and whatever a reserving holder
    /// retains **together**.
    ///
    /// **It is a true bound because [`BufferPool::keeps`] refuses anything
    /// above a slot and [`BufferPool::reserve`] takes the retained blocks out
    /// of the free list's share.** Neither was so before `19.7`: `keeps`
    /// admitted every buffer under [`POOL_MAX_BYTES`], so a free list of four
    /// 8 MiB partition reads was 32 MiB this called 4, and the retained list
    /// was held at [`BufferPool::slots`] beside a free list held at the same
    /// count, so a block pool's real ceiling was twice this.
    ///
    /// This is what a source with **two** read units subtracts before handing
    /// the remainder to the second pool, so that one stated budget bounds the
    /// source rather than each of its pools separately
    /// (`docs/design/architecture.md`, "The compressed source").
    fn held_bytes(&self) -> usize {
        self.slots().saturating_mul(self.slot_bytes())
    }

    /// Whether a released buffer is worth keeping: **anything that fits a
    /// slot**, which is the announced read length or [`POOL_MAX_BYTES`] where
    /// nothing has been announced ([`BufferPool::slot_bytes`]).
    ///
    /// **One rule, so that [`BufferPool::held_bytes`] is true.** A pool
    /// describes one read unit, and a buffer larger than that unit occupying a
    /// slot counted at the unit is exactly the under-report the byte budget
    /// exists to prevent: the free list would hold `slots × POOL_MAX_BYTES`
    /// while `held_bytes` reported `slots × slot_bytes`, and that number is
    /// what [`XzSource::apportion`] divides a budget with.
    ///
    /// **What it costs is the second read unit a single-unit pool could never
    /// account for.** The parallel plain path releases a partition-sized body
    /// read into a pool whose slot is a *chunk* ([`PLAIN_PARTITION_CHUNKS`]),
    /// and that buffer is now dropped rather than pooled — the under-report
    /// was what made it look pooled. The fix is the two-unit arrangement
    /// [`XzSource`] already runs, filed as the roadmap Future item "A two-unit
    /// plain source"; a ceiling here would only hide it again
    /// (`docs/design/architecture.md`, "The interior split").
    fn keeps(&self, len: usize) -> bool {
        len <= self.slot_bytes()
    }

    /// State how many slots a holder outside the free list is accounting for.
    ///
    /// The only one is [`BlockCache`]'s retained list, whose blocks hold
    /// [`PooledBuffer`]s of this pool's own slot size: without the reservation
    /// the two lists are held at [`BufferPool::slots`] each with nothing
    /// shared between them, and the pool's ceiling is twice what its budget
    /// states (`docs/design/architecture.md`, "The compressed source").
    ///
    /// `Relaxed` for the same reason [`BufferPool::hinted`] is: the holder
    /// writes it whenever its list changes length and [`BufferPool::release`]
    /// reads it once per returned buffer, nothing is published through it, and
    /// a value that arrives a buffer late costs one keep or one drop.
    fn reserve(&self, slots: usize) {
        self.reserved.store(slots, Ordering::Relaxed);
    }

    /// Seed the free list with a buffer nobody took, which only a test has
    /// reason to do: every shipped path releases through [`PooledBuffer`],
    /// whose charge this shape cannot carry.
    #[cfg(test)]
    fn give(&self, buf: Vec<u8>) {
        self.release(buf, false);
    }

    /// How many buffers are on the free list.
    #[cfg(test)]
    fn free_len(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).free.len()
    }

    /// How many buffers taken under a granted wait are out — the term a
    /// waiting acquisition is bounded by.
    #[cfg(test)]
    fn charged(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).charged
    }

    /// Return a buffer, discharging its slot where it was charged one.
    ///
    /// **The charge is discharged whether or not the buffer is kept.** A
    /// buffer the ceiling refuses is dropped here exactly as before, and the
    /// slot it occupied is still free the moment its holder let go of it —
    /// counting *kept* buffers instead would leak a slot per refused release
    /// and eventually block every transient reader.
    fn release(&self, buf: Vec<u8>, charged: bool) {
        let keeps = self.keeps(buf.len());
        let slots = self.free_slots();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if keeps && state.free.len() < slots {
            state.free.push(buf);
        }
        if charged {
            state.charged = state.charged.saturating_sub(1);
            drop(state);
            self.returned.notify_one();
        }
    }
}

/// The owner behind a [`BufferPool`]-backed [`Bytes`]: it returns the buffer
/// when the last reference to those bytes is dropped, which is what lets the
/// query path keep a chunk alive for as long as a zero-copy view points into
/// it and still get the buffer back afterwards.
struct PooledBuffer {
    /// `None` only between [`Drop`] taking the buffer and the struct going
    /// away, which no `as_ref` can observe.
    buf: Option<Vec<u8>>,
    pool: Arc<BufferPool>,
    /// Whether this buffer counts against [`PoolState::charged`] — set by
    /// [`BufferPool::obtain`] from the [`WaitPolicy`] in force when it was
    /// taken, and carried here rather than re-derived at release so that a
    /// policy granted in between cannot leave the count wrong.
    charged: bool,
}

impl PooledBuffer {
    /// The bytes, for a caller that is about to fill them.
    fn as_mut(&mut self) -> &mut [u8] {
        self.buf.as_deref_mut().unwrap_or(&mut [])
    }
}

impl AsRef<[u8]> for PooledBuffer {
    fn as_ref(&self) -> &[u8] {
        self.buf.as_deref().unwrap_or(&[])
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        if let Some(buf) = self.buf.take() {
            self.pool.release(buf, self.charged);
        }
    }
}

/// The only `ByteRangeSource` implementation shipped in the MVP: a local
/// file, read via blocking positioned reads on a `spawn_blocking` task
/// (`tokio` has no native async positioned-read).
pub struct LocalFileSource {
    path: PathBuf,
    file: Arc<std::fs::File>,
    pool: Arc<BufferPool>,
}

impl LocalFileSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = std::fs::File::open(&path)?;
        Ok(Self { path, file: Arc::new(file), pool: Arc::new(BufferPool::default()) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl ByteRangeSource for LocalFileSource {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        Box::pin(async move {
            let file = Arc::clone(&self.file);
            let pool = Arc::clone(&self.pool);
            // A read that fails drops its buffer instead of returning it: the
            // pool is an optimization, and an error path is the one place where
            // re-allocating costs nothing anyone will measure.
            //
            // **The slot is taken inside the blocking task, not before it.**
            // `obtain` blocks for a loop that granted the wait, and blocking the
            // runtime thread that is meant to be driving the sibling reads which
            // would free a slot is the one way to turn that wait into a
            // deadlock.
            let slot = tokio::task::spawn_blocking(move || -> std::io::Result<PooledBuffer> {
                let mut slot = pool.obtain(len);
                file.read_exact_at(&mut slot.as_mut()[..len], offset)?;
                Ok(slot)
            })
            .await
            .map_err(Error::from)?
            .map_err(Error::from)?;
            Ok(Bytes::from_owner(slot).slice(..len))
        })
    }

    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        Box::pin(async move {
            let file = Arc::clone(&self.file);
            let len = tokio::task::spawn_blocking(move || file.metadata().map(|m| m.len()))
                .await
                .map_err(Error::from)?
                .map_err(Error::from)?;
            Ok(len)
        })
    }

    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        Box::pin(async move {
            let file = Arc::clone(&self.file);
            let mtime =
                tokio::task::spawn_blocking(move || file.metadata().and_then(|m| m.modified()))
                    .await
                    .map_err(Error::from)?
                    .map_err(Error::from)?;
            Ok(Some(mtime))
        })
    }

    /// **The pool's one non-size rule.** A read loop announcing its chunk size
    /// is what lets a buffer of that length be kept above [`POOL_MAX_BYTES`],
    /// so a raised `ScanOptions::chunk_size` keeps its pooling instead of
    /// paying a fresh `calloc` per chunk. The cost is bounded and is the
    /// caller's own number: [`BufferPool::slots`] buffers of the size it asked
    /// for.
    fn hint_read_size(&self, len: usize) {
        self.pool.hint(len);
    }

    /// **Anywhere, [`PLAIN_PARTITION_CHUNKS`] read chunks each.** A positioned
    /// read costs the same at every offset, so nothing about this source
    /// prefers one split point to another; the size is what
    /// [`PLAIN_PARTITION_CHUNKS`] documents.
    ///
    /// The unit it is a multiple of is the pool's own slot size — the length a
    /// read loop announced, or the ceiling where none has
    /// ([`BufferPool::slot_bytes`]) — so a caller that raised
    /// `crate::ScanOptions::chunk_size` scales this with it, and a chunk
    /// already at or past [`POOL_MAX_BYTES`] is a partition on its own.
    ///
    /// What a partition holds resident is that whole number of chunks in one
    /// buffer, not a chunk: `crate::leader::scan_partition` reads its piece in
    /// a single `read_range`, so `partition_bytes` is a length this source
    /// really does allocate.
    fn partitions(&self, _range: Range<u64>) -> Partitioning {
        let chunk = self.pool.slot_bytes();
        let bytes = chunk.saturating_mul(PLAIN_PARTITION_CHUNKS).min(POOL_MAX_BYTES).max(chunk);
        Partitioning::anywhere(bytes as u64)
    }

    /// **The budget sizes the free list; the worker count does not.** This
    /// source has one read unit, and a chunk buffer is taken and released
    /// inside a single `read_range` — so what the free list has to hold is the
    /// *replay* path's depth, which is what [`POOL_DEPTH`] is, and not a
    /// worker count. Raising the ceiling with `jobs` would grow a query's
    /// resident set by `(jobs - POOL_DEPTH)` chunks the moment
    /// `crate::batch::RetainedChunks` releases the buffers a flushed batch was
    /// pinning (`docs/design/architecture.md`, "Execution model and API
    /// surface").
    ///
    /// **What it means now that the leader schedules concurrent readers over
    /// this source**: a `parse` at `--jobs n` runs `n` fused workers against
    /// [`POOL_DEPTH`] chunk slots, so above four of them the extra workers
    /// block for a slot rather than allocating — which is the wait doing its
    /// job, and a ceiling on plain-file worker throughput that no figure has
    /// priced yet.
    fn hint_parallelism(&self, parallelism: Parallelism) {
        self.pool.set_limits(budget_bytes(parallelism), POOL_DEPTH);
    }

    /// **One read unit, so one policy to state.** A loop that permits a wait
    /// gets one: its reads block for a free chunk buffer rather than
    /// allocating past the stated budget. A loop that does not permit one
    /// allocates, because the buffers it is holding may be the only ones that
    /// could free a slot.
    fn hint_wait_policy(&self, policy: WaitPolicy) {
        self.pool.set_policy(policy);
    }
}

/// The byte budget a caller's [`Parallelism`] states, as a `usize`, falling
/// back to [`DEFAULT_MEMORY_BUDGET`] where it states none — which is
/// [`Parallelism::default`], and also a `u64` too large to be a length on this
/// target, where the default is the smaller and therefore the safe answer.
fn budget_bytes(parallelism: Parallelism) -> usize {
    parallelism
        .memory_bytes()
        .and_then(|bytes| usize::try_from(bytes).ok())
        .unwrap_or(DEFAULT_MEMORY_BUDGET as usize)
}

/// One block's plaintext, decoded whole into a pooled slot.
///
/// The slot returns to its pool when the last [`Bytes`] viewing it drops
/// together with the retention list's own reference, which is what lets a
/// read inside this block be a zero-copy slice rather than a copy
/// (`docs/design/architecture.md`, "The compressed source").
struct DecodedBlock {
    /// A slot sized to the file's *largest* block, so every block fits every
    /// slot; `len` is what this block actually holds.
    slot: PooledBuffer,
    len: usize,
}

impl DecodedBlock {
    /// This block's plaintext — the slot cut down to what the block holds.
    fn bytes(&self) -> &[u8] {
        &self.slot.as_ref()[..self.len]
    }
}

/// The owner behind a [`Bytes`] sliced out of a decoded block: a share of the
/// block, so the slot outlives every view into it.
struct BlockView(Arc<DecodedBlock>);

impl AsRef<[u8]> for BlockView {
    fn as_ref(&self) -> &[u8] {
        self.0.bytes()
    }
}

/// The block unit's pool, and the decoded blocks it is currently holding.
///
/// **This is the second pool a source with two read units takes**, not a
/// second unit announced into the chunk pool: [`BufferPool::hinted`] is one
/// value driving both [`BufferPool::keeps`] and [`BufferPool::slot_bytes`], so
/// one pool serving both units is wrong for one of them either way
/// (`docs/design/architecture.md`, "The compressed source").
///
/// **Retention is what makes per-call block decode affordable.** A read loop
/// asks for `chunk_size` bytes at a time — 1 MiB by default — and a 24 MiB
/// block decoded afresh per call would be 24× the decode work a live decode
/// does for the same walk. So a decoded block is kept, and the next read
/// inside it is a slice. The retained set is capped at
/// [`BufferPool::slots`], so raising the pool's budget raises how many blocks
/// may be in flight at once, which is what N concurrent readers need.
///
/// **A retained block occupies a slot rather than adding to the free list.**
/// Eviction happens *before* a slot is taken ([`BlockCache::slot`]), so the
/// evicted buffer is what the next decode reuses — a retained set bounded at
/// [`BufferPool::slots`] and its own budget again would be two numbers for one
/// bound, which is exactly what the byte budget replaced.
///
/// **The retained cap and the free-list cap are one count, so this pool's
/// ceiling is `slots * unit`.** This list reserves what it holds
/// ([`BufferPool::reserve`]), so [`BufferPool::release`] pools a returned
/// buffer only while free-plus-retained is below [`BufferPool::slots`] —
/// 48 MiB at the default budget's two 24 MiB slots, where two independent
/// counts made it 96. *Rejected: leaving them independent*, on the ground that
/// coupling touches [`BufferPool::release`]'s hot path for a window only a
/// serial forward scan's zero-or-one free list closes. Under N readers one
/// worker's release refills the free list while another retains, so the window
/// is open for the length of the run; and the sub-stream divisor cannot bound
/// it from outside, because [`BufferPool::slots`] is `budget / unit` once the
/// depth clamp is slack and so does not move with the admitted worker count
/// (`docs/design/architecture.md`, "The compressed source").
///
/// *Rejected: retaining one block on the serial path*, on the ground that one
/// reader walking forward needs exactly one and that the cap costs a 3.00 GiB
/// `.xz` `parse` 64.7 MiB resident against the streaming form's 16.2. Two
/// things answer it. `pgdq parse` is the shape that pins **least** — it builds
/// no batches, so `crate::batch::RetainedChunks` never runs — while on the
/// query path a retained chunk is a zero-copy view into a whole block, so a
/// batch bounded by `max_source_span`'s 64 MiB spans several blocks and pins
/// every one of them; that is more than this cap holds and is where a
/// compressed scan's resident cost actually comes from. And a cap of one is
/// not merely smaller: it makes [`BlockCache::slot`] keep zero and drain
/// before every decode, so any outstanding view forces a fresh allocation of
/// the block unit instead of a reuse — the pool stops pooling exactly when a
/// caller is holding a block.
struct BlockCache {
    /// Slot size: the file's largest block. Every slot fits every block, which
    /// is what `BlockTask::decode_into` is documented to allow, and it is what
    /// the pool is hinted with so a slot survives [`POOL_MAX_BYTES`].
    unit: usize,
    pool: Arc<BufferPool>,
    /// Retained blocks by index, least-recently-used first.
    retained: Mutex<Vec<(usize, Arc<DecodedBlock>)>>,
}

impl BlockCache {
    /// The cache for `table`, or `None` where there is no block unit to build
    /// one around: a file with no blocks at all, or a block longer than a
    /// length on this target.
    ///
    /// **Whether the unit is *affordable* is a separate question and a later
    /// one** ([`BlockCache::affordable`]), because the answer depends on a
    /// budget the caller has not stated yet at the moment a source is
    /// constructed.
    fn for_table(table: &xz_seek::SeekTable) -> Option<BlockCache> {
        let unit = table.max_block_uncompressed();
        if unit == 0 {
            return None;
        }
        let unit = usize::try_from(unit).ok()?;
        let pool = Arc::new(BufferPool::default());
        pool.hint(unit);
        Some(BlockCache { unit, pool, retained: Mutex::new(Vec::new()) })
    }

    /// What **one concurrent reader** of this file holds while it decodes
    /// whole blocks, in bytes: the block unit twice, the chunk buffer a
    /// straddling read is assembled into, and the decoder's own retention
    /// (`docs/design/architecture.md`, "The compressed source").
    ///
    /// **Two units, because a coupled count of one is the shape this pool
    /// rejects by name.** [`BlockCache::slot`] drains the retention list to
    /// `slots() - 1` before taking a slot, so a one-slot pool retains nothing
    /// and drains before every decode — it stops pooling exactly when a caller
    /// is holding a block. A reader therefore holds the block it is decoding
    /// and the one it kept, and that is the same sentence as "the coupled
    /// count is at least two" rather than a constant of its own.
    ///
    /// It is one number with two consumers — [`BlockCache::affordable`], which
    /// asks whether the budget admits a single such reader, and
    /// [`XzSource::partition_advice`], which is what a caller's budget is
    /// divided by to reach a worker count. They were two statements of one
    /// cost until the divisor was found charging a *single* unit and nothing
    /// for the decoder, 25 MiB against a measured 59.4.
    fn reader_bytes(&self, chunk_bytes: u64, decode_bytes: u64) -> u64 {
        (self.unit as u64)
            .saturating_mul(2)
            .saturating_add(chunk_bytes)
            .saturating_add(decode_bytes)
    }

    /// What **the block path costs at `workers` concurrent readers**: the
    /// per-reader charge above, plus the pool floor those readers leave
    /// unfilled ([`WorkerMemory`]).
    ///
    /// **The floor is `(POOL_DEPTH − workers) × unit`, and it is arithmetic
    /// from this pool rather than a measured term.** [`BufferPool::slots`]
    /// clamps the block pool at `POOL_DEPTH.max(jobs)`; the free list and the
    /// retention list share those slots ([`BufferPool::reserve`]) and each
    /// reader is decoding into a buffer besides, so the pool holds
    /// `slots + workers` units against a bill of `2 × workers`. Above
    /// [`POOL_DEPTH`] readers the two agree exactly and the floor is zero;
    /// below it the difference is what nobody paid for. It is confirmed
    /// against five cells of `19.16`'s readings to 1.4 MiB
    /// (`docs/design/architecture.md`, "Execution model and API surface").
    fn worker_memory(&self, chunk_bytes: u64, decode_bytes: u64) -> WorkerMemory {
        WorkerMemory::per_worker(self.reader_bytes(chunk_bytes, decode_bytes))
            .flooring(self.unit as u64, POOL_DEPTH)
    }

    /// Whether `budget` admits one such reader — the line that decides between
    /// block decode and the streaming reader
    /// (`docs/design/architecture.md`, "The compressed source").
    ///
    /// **It is asked of *one* reader and charges that reader's pool floor with
    /// it**, which is what makes the stated budget true rather than nearly
    /// true: a pool serving a single reader still holds [`POOL_DEPTH`] slots,
    /// so admitting the path on the per-reader charge alone allows
    /// `(POOL_DEPTH − 1)` units the caller never granted. The decline it
    /// widens is accepted rather than worked around — at 128 MiB blocks the
    /// line moves from 266 MiB to 650 — because it is the first arrangement in
    /// which a large-block file's stated number holds, the decline is reported
    /// (`crate::stream::compressed_block_path_declined`), and a caller who
    /// wants the path back states a budget.
    ///
    /// **The chunk and the decoder come off the top, and what is left must
    /// hold two blocks.** Both are paid on the streaming path too — that
    /// fallback assembles reads into a chunk buffer and keeps a decoder of its
    /// own — so they are not what the decline saves; the two block slots are.
    /// Stating it as one comparison against the whole per-reader cost is what
    /// keeps the decline and the divisor from being two sentences that can
    /// drift apart.
    ///
    /// **This is where a constant used to be.** The refusal was a fixed
    /// 256 MiB beside a fixed 64 MiB budget, two unrelated numbers of which
    /// the smaller was not a bound above its own slot size — a 128 MiB block
    /// was 128 MiB resident against a 64 MiB budget, because
    /// [`BufferPool::slots`] floors at one. Reading it off the budget makes
    /// the stated number true: a unit the caller did not allow room for is
    /// never decoded whole, so the floor can never be reached with a slot
    /// bigger than the budget.
    ///
    /// **What it declines is memory, not seekability.** Any file with more
    /// than one block is seekable, and decodable block-wise, for a client
    /// willing to allocate a block — so a file written with large blocks
    /// (`xz -9 -T0`, whose blocks are ~192 MiB; `xz -T8
    /// --block-size=512MiB`) is declined under the default budget and does
    /// have parallelism to lose. That is now the caller's to reverse, with
    /// `--parallel-memory` or `Parallelism::Workers`, rather than a constant's
    /// to permit.
    fn affordable(&self, chunk_bytes: u64, decode_bytes: u64, budget: u64) -> bool {
        self.worker_memory(chunk_bytes, decode_bytes).at(1) <= budget
    }

    /// The retained block at `index`, promoted to most-recently-used.
    fn lookup(&self, index: usize) -> Option<Arc<DecodedBlock>> {
        let mut retained = self.retained.lock().unwrap_or_else(|e| e.into_inner());
        let at = retained.iter().position(|(i, _)| *i == index)?;
        let entry = retained.remove(at);
        let block = Arc::clone(&entry.1);
        retained.push(entry);
        Some(block)
    }

    /// A slot to decode into, after making room for it: least-recently-used
    /// blocks are dropped down to one below the slot count *first*, so the
    /// buffer this take reuses is usually the one that eviction just released.
    ///
    /// **Eviction before acquisition is a reuse rule, not a progress
    /// guarantee.** It is what makes the buffer this take reuses the one the
    /// drain just released, and nothing more: a retained block is normally
    /// also the block some reader is holding a view into, so the drain frees
    /// no slot at all for the caller that is waiting on one. That is why the
    /// block pool is never granted a wait ([`XzSource::hint_wait_policy`], and
    /// `docs/design/architecture.md`, "Execution model and API surface").
    ///
    /// A block evicted while a [`Bytes`] still views it stays alive until that
    /// view drops.
    ///
    /// **The reservation is lowered before the evicted blocks drop**, and the
    /// order is load-bearing rather than tidy: a dropped block releases its
    /// buffer straight into the pool, and a release seeing the pre-eviction
    /// reservation would find no room and discard the very buffer this take is
    /// about to reuse.
    fn slot(&self) -> PooledBuffer {
        let keep = self.pool.slots().saturating_sub(1);
        let evicted = {
            let mut retained = self.retained.lock().unwrap_or_else(|e| e.into_inner());
            let over = retained.len().saturating_sub(keep);
            let evicted: Vec<_> = retained.drain(..over).collect();
            self.pool.reserve(retained.len());
            evicted
        };
        drop(evicted);
        self.pool.obtain(self.unit)
    }

    /// Retain `block` as most-recently-used. [`BlockCache::slot`] has already
    /// made room, so nothing is evicted here.
    fn retain(&self, index: usize, block: Arc<DecodedBlock>) {
        let mut retained = self.retained.lock().unwrap_or_else(|e| e.into_inner());
        retained.retain(|(i, _)| *i != index);
        retained.push((index, block));
        self.pool.reserve(retained.len());
    }
}

/// A `ByteRangeSource` decoding an `.xz`-compressed local file on the fly
/// (`docs/design/architecture.md`, "The compressed source").
///
/// **Three file handles, deliberately.** `open` walks the file's stream
/// footers once (`xz_seek::SeekTable::from_source`'s cost — one read per
/// stream plus one for the file's tail) and hands one handle to the
/// `xz_seek::Reader`, which owns it for decoding; `stat_file` answers
/// [`ByteRangeSource::stored_size`]/[`ByteRangeSource::modified`] with a plain
/// `stat` and must never disturb the decoder's live position to do it; and
/// `data_file` is what block decodes read their compressed bytes through,
/// positioned reads only, so a decode running outside the mutex shares no
/// cursor with anything.
///
/// **A read decodes the blocks it lands in, and the reader behind the mutex is
/// the fallback.** `xz_seek::BlockTask` is `Copy` and owns everything a
/// decode needs, so `read_range` takes a task, decodes the block into a slot
/// of its own [`BlockCache`], and slices the answer out of it — with the
/// mutex held only long enough to *name* the task, never across the decode.
/// That is what lets concurrent `read_range` calls genuinely run concurrently.
/// The streaming `xz_seek::Reader::read_at` path is kept for the one shape
/// block decode refuses, a file whose largest block does not fit the caller's
/// stated budget ([`BlockCache::affordable`]).
///
/// `Verify::Full` — completing a partly-decoded block's check before the
/// decoder leaves it — is `xz_seek::Reader::new`'s own default, so nothing
/// here has to ask for it; a whole-block decode is stronger still, comparing
/// the check before it returns.
pub struct XzSource {
    path: PathBuf,
    stat_file: Arc<std::fs::File>,
    data_file: Arc<std::fs::File>,
    /// The seek table, held beside the reader so that `size()`, `seek_table()`
    /// and the per-read `blocks_in` lookup take no lock at all.
    table: Arc<xz_seek::SeekTable>,
    /// The streaming reader: the fallback decode path, and the only thing that
    /// can hand out an `xz_seek::BlockTask`. Behind a `Mutex` because
    /// `read_at` takes `&mut self`; the block path holds it for a table lookup
    /// and no I/O.
    reader: Arc<Mutex<xz_seek::Reader<std::fs::File>>>,
    /// The chunk unit: what a read spanning more than one block is assembled
    /// into, and what the fallback path reads into.
    pool: Arc<BufferPool>,
    /// The block unit, or `None` where this file has no blocks to decode.
    /// Present does not mean *taken*: [`XzSource::block_path`] is what decides
    /// per read, since a block the caller's budget cannot hold is streamed.
    blocks: Option<Arc<BlockCache>>,
    /// What one decode of this file retains beyond the slot it writes into —
    /// the LZMA2 dictionary, the compressed input chunk and the backend's own
    /// state (`xz_seek::Reader::decode_footprint`).
    ///
    /// **Read once, at construction.** It is a property of the file and of the
    /// backend, not of a range, so nothing about a read moves it; asking the
    /// reader for it per call would take the mutex the block path exists to
    /// stay off.
    decode_bytes: u64,
    /// What the caller stated it may hold, in bytes, across **both** pools —
    /// [`DEFAULT_MEMORY_BUDGET`] until one is announced.
    ///
    /// Held on the source rather than pushed straight into the pools because
    /// the split between them is derived from it and from the announced chunk
    /// size, which arrive in either order: [`XzSource::apportion`] recomputes
    /// from this on both announcements, so neither hint has to come first.
    budget: AtomicUsize,
    /// The worker ceiling the caller stated — 1 until one is announced. It is
    /// the **block** pool's depth: one retained block per concurrent reader is
    /// what keeps N workers off each other's decodes, where a chunk buffer is
    /// taken and released inside one read and wants the replay path's depth
    /// instead ([`LocalFileSource::hint_parallelism`]).
    jobs: AtomicUsize,
}

impl XzSource {
    /// Open `path` as `.xz`-compressed input, walking its stream footers to
    /// build the seek table before this call returns.
    ///
    /// This does not sniff the magic bytes — a caller that already knows it
    /// has an `.xz` file constructs this directly; content-sniffing
    /// recognition across both source kinds is [`open_local`], a
    /// library-level convenience deliberately outside the trait.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        // The walk this names is the one [`XzSource::with_table`] exists to
        // skip, so only this constructor emits it — a cached table costs no
        // footer read and earns no line
        // (`docs/design/architecture.md`, "Status output").
        tracing::info!(path = %path.display(), "seek table build started");
        let file = std::fs::File::open(&path)?;
        let stat_file = Arc::new(file.try_clone()?);
        let data_file = Arc::new(file.try_clone()?);
        let reader = xz_seek::Reader::new(file)?;
        let table = reader.index();
        tracing::info!(
            path = %path.display(),
            streams = table.stream_count(),
            blocks = table.block_count(),
            "seek table build complete",
        );
        Ok(Self::assembled(path, stat_file, data_file, reader))
    }

    /// Open `path` from a seek table a previous walk of the *same* file
    /// produced, **without walking it again**
    /// (`docs/design/architecture.md`, "The compressed source").
    ///
    /// This is what turns the table a cache persists into a saving: the
    /// footer walk [`XzSource::open`] pays is one read per stream, which is
    /// 85 s on the 31,150-stream koji download and is otherwise paid by every
    /// command against it however complete the cache is.
    ///
    /// `xz_seek` validates the table for internal consistency and against the
    /// file's own length before trusting it, reading **no bytes** — so a
    /// table that does not describe this file costs nothing to reject, and a
    /// block header's own CRC32 is what catches one that survives validation.
    /// A rejected table is `Error::Xz(xz_seek::Error::InvalidTable { .. })`,
    /// which is the error [`open_local`] turns into [`Recognized::Mismatch`]
    /// rather than a walk.
    pub fn with_table(path: impl AsRef<Path>, table: xz_seek::SeekTable) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = std::fs::File::open(&path)?;
        let stat_file = Arc::new(file.try_clone()?);
        let data_file = Arc::new(file.try_clone()?);
        // `Builder::new()` rather than a configured one: `Reader::new` is the
        // shortcut through exactly these defaults, so the two constructors
        // decode identically — `Verify::Full` included.
        let reader = xz_seek::Builder::new().open_with_table(file, table)?;
        Ok(Self::assembled(path, stat_file, data_file, reader))
    }

    /// The one place the two constructors agree: the table is lifted out of
    /// the reader so nothing but the fallback path ever locks it, and the
    /// block pool is sized from that table or refused.
    fn assembled(
        path: PathBuf,
        stat_file: Arc<std::fs::File>,
        data_file: Arc<std::fs::File>,
        reader: xz_seek::Reader<std::fs::File>,
    ) -> Self {
        let table = Arc::new(reader.index().clone());
        let blocks = BlockCache::for_table(&table).map(Arc::new);
        let decode_bytes = reader.decode_footprint();
        let source = Self {
            path,
            stat_file,
            data_file,
            table,
            reader: Arc::new(Mutex::new(reader)),
            pool: Arc::new(BufferPool::default()),
            blocks,
            decode_bytes,
            budget: AtomicUsize::new(DEFAULT_MEMORY_BUDGET as usize),
            jobs: AtomicUsize::new(1),
        };
        // A source is readable before anything is announced to it, so the two
        // pools are divided at construction rather than at the first hint.
        source.apportion();
        source
    }

    /// Divide the stated budget between the two pools, **chunks first**.
    ///
    /// One stated number bounds the source, not each of its pools, so the
    /// block pool is given what is left after the chunk pool's own ceiling
    /// (`BufferPool::held_bytes`) — 4 MiB at the 1 MiB default chunk, so a
    /// 64 MiB budget leaves 60 for blocks. Chunks come first because that pool
    /// is the one every read path uses and the one the 1.83× pool-miss figure
    /// was measured through; the block pool is what a large budget is
    /// *for*, and it is the term that actually grows.
    ///
    /// Recomputed from `self.budget` on **both** announcements rather than
    /// composed incrementally, so `hint_read_size` and `hint_parallelism` may
    /// arrive in either order and a source that is never told either still
    /// holds a coherent split.
    fn apportion(&self) {
        let budget = self.budget.load(Ordering::Relaxed);
        let jobs = self.jobs.load(Ordering::Relaxed).max(1);
        self.pool.set_limits(budget, POOL_DEPTH);
        if let Some(blocks) = &self.blocks {
            // One retained block per concurrent reader, and never fewer than
            // the pool's own depth: a block pool of one drains before every
            // decode, so it stops pooling exactly when a caller is holding a
            // block.
            //
            // **`jobs` here is the count the caller *announced*, not the count
            // `crate::stream::worker_count` then delivers.** They agree under
            // discovery and diverge where a stated `--jobs` outruns a stated
            // budget, which is `KD21`
            // (`docs/design/architecture.md`, "Execution model and API
            // surface").
            // deficiency: KD21
            blocks
                .pool
                .set_limits(budget.saturating_sub(self.pool.held_bytes()), POOL_DEPTH.max(jobs));
        }
    }

    /// The chunk slot every one of this source's charges is stated against:
    /// the length a read loop announced ([`ByteRangeSource::hint_read_size`]),
    /// or [`crate::DEFAULT_CHUNK_SIZE`] where none has yet.
    ///
    /// **The fallback is the chunk a scan settles at, not
    /// [`POOL_MAX_BYTES`]**, which is what [`BufferPool::slot_bytes`] answers
    /// an unannounced pool and is the right answer to *its* question — the
    /// largest buffer that pool will keep. It is the wrong one here, because
    /// this number is divided into an allowance by
    /// [`Parallelism::fit`] at a moment when the file is not yet open for
    /// reading, while the gate the count then meets
    /// ([`BlockCache::affordable`]) is compared at the steady-state slot. The
    /// gap is exactly `POOL_MAX_BYTES - DEFAULT_CHUNK_SIZE`, and it cost a
    /// reader at every allocation: a 512 MiB cgroup resolved three readers
    /// where four fit (`docs/design/architecture.md`, "Execution model and API
    /// surface").
    ///
    /// *Rejected:* moving the gate to the recommendation instead. The gate is
    /// compared against what a reader really holds while it reads, which is a
    /// chunk and not a pool ceiling, so raising it would decline the block path
    /// on files that fit.
    fn charged_chunk_bytes(&self) -> u64 {
        match self.pool.announced_bytes() {
            Some(len) => len as u64,
            None => crate::DEFAULT_CHUNK_SIZE as u64,
        }
    }

    /// What concurrent block-decoding readers of this file would cost, whether
    /// or not the budget admits one: [`BlockCache::worker_memory`] over this
    /// source's own chunk size ([`XzSource::charged_chunk_bytes`]) and decoder
    /// charge — the per-reader charge and the pool floor below it, as one
    /// shape.
    ///
    /// **It is the one composition site**, which is what keeps the
    /// recommendation ([`ByteRangeSource::default_worker_memory`]), the gate
    /// ([`BlockCache::affordable`]) and the advice
    /// ([`Partitioning::worker_memory`]) from being three statements of one
    /// cost that can part company. They were two 7 MiB apart once already
    /// ([`XzSource::charged_chunk_bytes`]).
    fn block_worker_memory(&self) -> Option<WorkerMemory> {
        let cache = self.blocks.as_ref()?;
        Some(cache.worker_memory(self.charged_chunk_bytes(), self.decode_bytes))
    }

    /// The block-decode path, or `None` where this read goes through the
    /// streaming reader: no blocks at all, or a reader the caller's budget
    /// cannot afford ([`BlockCache::affordable`]).
    fn block_path(&self) -> Option<&Arc<BlockCache>> {
        let budget = self.budget.load(Ordering::Relaxed) as u64;
        let chunk_bytes = self.charged_chunk_bytes();
        self.blocks
            .as_ref()
            .filter(|cache| cache.affordable(chunk_bytes, self.decode_bytes, budget))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The error a read past the end of the uncompressed stream is — the same
    /// `UnexpectedEof` a short `read_exact_at` raises on the plain source, so
    /// both sources report a caller/source disagreement identically.
    fn short_read() -> Error {
        Error::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "xz stream ended before the requested range",
        ))
    }

    /// The block at `index`, from the retention list or freshly decoded into a
    /// slot of the block pool.
    ///
    /// **Two concurrent misses on one block decode it twice**, and on the
    /// parallel scan path that is the common case rather than the rare one:
    /// every fused worker's chunk-sized tail read lands in the block its
    /// *successor* owns, so each block is decoded about twice and a scan's
    /// speedup is capped near half the reader count — `KD20`, the argument and
    /// the readings being beside the mechanism
    /// (`docs/design/architecture.md`, "Execution model and API surface").
    /// An in-flight map is the fix and is not taken here: it puts a second
    /// lock in front of the case that does *not* collide, and this is a
    /// throughput bound rather than a correctness one. **Widening the cut so
    /// that fewer pieces each waste one block has been measured and refused**
    /// ([`BOUNDARIED_PARTITION_UNITS`]), so the map is the fix left rather
    /// than one of two.
    // deficiency: KD20
    fn block(
        index: usize,
        cache: &BlockCache,
        reader: &Mutex<xz_seek::Reader<std::fs::File>>,
        file: &std::fs::File,
    ) -> Result<Arc<DecodedBlock>> {
        if let Some(hit) = cache.lookup(index) {
            return Ok(hit);
        }
        // The lock covers a lookup in the seek table and nothing else: a
        // `BlockTask` is `Copy` and owns its block, its check and the reader's
        // decode settings, so the decode below runs outside it.
        let task = {
            let reader = reader.lock().unwrap_or_else(|e| e.into_inner());
            reader.block_task(index)
        }
        .ok_or_else(Self::short_read)?;
        let len = usize::try_from(task.uncompressed_len()).map_err(|_| Self::short_read())?;
        // A slot is the file's largest block, so `len` is never above it and a
        // released slot is exactly the length the pool was hinted with.
        let mut slot = cache.slot();
        // A decode that fails drops its slot rather than returning it, as the
        // plain source's failed read does.
        task.decode_into(file, &mut slot.as_mut()[..len])?;
        let block = Arc::new(DecodedBlock { slot, len });
        cache.retain(index, Arc::clone(&block));
        Ok(block)
    }

    /// A read served by decoding the blocks it lands in.
    ///
    /// A read inside one block is a **slice of that block**, so the common
    /// case copies nothing; one that straddles a boundary is assembled into a
    /// chunk-pool buffer, which is what `read_at` did for every read.
    fn read_by_blocks(
        offset: u64,
        len: usize,
        table: &xz_seek::SeekTable,
        cache: &BlockCache,
        reader: &Mutex<xz_seek::Reader<std::fs::File>>,
        file: &std::fs::File,
        chunks: &Arc<BufferPool>,
    ) -> Result<Bytes> {
        let end = offset.checked_add(len as u64).ok_or_else(Self::short_read)?;
        let covering = table.blocks_in(offset..end);
        if covering.is_empty() {
            return Err(Self::short_read());
        }
        if covering.len() == 1 {
            let index = covering.start;
            let block = Self::block(index, cache, reader, file)?;
            let base = table.blocks[index].uncompressed_offset;
            // `blocks_in` answers the block *covering* the start, so `base` is
            // at or below `offset`; the checked form is what keeps a table
            // that says otherwise a refusal rather than a panic.
            let start = offset
                .checked_sub(base)
                .and_then(|at| usize::try_from(at).ok())
                .ok_or_else(Self::short_read)?;
            if start + len > block.len {
                return Err(Self::short_read());
            }
            return Ok(Bytes::from_owner(BlockView(block)).slice(start..start + len));
        }
        let mut out = chunks.obtain(len);
        let mut covered = 0usize;
        for index in covering {
            let block = Self::block(index, cache, reader, file)?;
            let base = table.blocks[index].uncompressed_offset;
            let from = base.max(offset);
            let to = (base + block.len as u64).min(end);
            if to <= from {
                continue;
            }
            let at = (from - offset) as usize;
            let within = (from - base) as usize;
            let n = (to - from) as usize;
            out.as_mut()[at..at + n].copy_from_slice(&block.bytes()[within..within + n]);
            covered += n;
        }
        if covered != len {
            return Err(Self::short_read());
        }
        Ok(Bytes::from_owner(out).slice(..len))
    }

    /// This source's partitioning advice, read off the **read path it took**
    /// rather than off the seek table.
    ///
    /// Two answers, and which one applies is `blocks`:
    ///
    /// **Block-decoding** — the boundaries are the block starts inside
    /// `range`, so a partition is a whole number of blocks and two workers
    /// never decode the same block twice
    /// (`docs/design/architecture.md`, "The compressed source"). A partition
    /// costs what one concurrent reader holds — [`BlockCache::reader_bytes`],
    /// the same number [`BlockCache::affordable`] compares a budget against,
    /// so the path a budget affords and the readers it admits are one
    /// statement.
    ///
    /// **Streaming fallback** — one partition, whatever the table says. Such a
    /// file still has block boundaries, but reaching an offset inside a block
    /// has one route through `xz_seek::Reader::read_at`: restart at that
    /// block's start and decode forward, discarding. Two workers on different
    /// partitions would each force the other's restart, so parallel mode over
    /// it is **worse than serial** rather than merely unaccelerated. Nothing is
    /// given up by it — LZMA2's dictionary runs the length of a block, so a
    /// block is the parallel unit entire, and the shape this reaches in
    /// practice is a single-block file, which has no second worker to give at
    /// any budget.
    ///
    /// Taken as a free function over the two pieces of state it reads, so the
    /// fallback arm is assertable against a table that *does* have blocks —
    /// which a fixture reaches only by stating a budget smaller than one of
    /// them, since a real file the default budget declines carries blocks of
    /// tens of megabytes each.
    fn partition_advice(
        table: &xz_seek::SeekTable,
        blocks: Option<&BlockCache>,
        chunk_bytes: u64,
        decode_bytes: u64,
        range: Range<u64>,
    ) -> Partitioning {
        let Some(cache) = blocks else {
            // **The streaming arm charges the chunk buffer and not the
            // decoder**, because that path keeps one `xz_seek::Reader` behind
            // a mutex however many readers a caller runs: the decoder is a
            // fixed cost of the source rather than a cost of a concurrent
            // reader, and charging it per partition would over-charge every
            // reader but the first ([`Partitioning::partition_bytes`]).
            return Partitioning::single(chunk_bytes);
        };
        let covering = table.blocks_in(range.clone());
        let at = covering
            .filter_map(|i| {
                let start = table.blocks[i].uncompressed_offset;
                (start > range.start && start < range.end).then_some(start)
            })
            .collect();
        // **And the retained unit is the partition, not the chunk.** A read
        // inside a decoded block is a zero-copy slice of it, so a batch that
        // holds views into this partition pins the block the line above has
        // already charged for; a caller adding its span allowance on top would
        // count those bytes twice ([`RetainedUnit`]). The streaming arm above
        // keeps the default: there a read is assembled *into* a chunk buffer,
        // so what a batch pins is chunks.
        // **And a worker reads one of this source's units in one call**,
        // which at the shipped cut width is the whole piece and so a zero-copy
        // slice of the block it lands in ([`PartitionRead::Whole`]). The unit
        // is `cache.unit`, which `BlockCache::for_table` sets from
        // `xz_seek::SeekTable::max_block_uncompressed` — the same number, read
        // off the cache that already holds it rather than recomputed, and
        // non-zero by that constructor's own guard. The streaming arm above
        // keeps the default for the same reason it keeps the retained unit:
        // there a read is assembled into a chunk buffer, and nothing is gained
        // by asking for a longer one.
        // **And the block pool's floor is stated beside the per-reader
        // charge.** `BufferPool::slots` clamps that pool at
        // `POOL_DEPTH.max(jobs)`, so below four readers it holds units the
        // per-reader charge never billed; stating it here is what lets
        // `crate::stream::worker_count` solve for a count rather than divide
        // by one ([`BlockCache::worker_memory`]).
        Partitioning::at(at, cache.reader_bytes(chunk_bytes, decode_bytes))
            .flooring(cache.unit as u64, POOL_DEPTH)
            .retaining(RetainedUnit::Partition)
            .reading(PartitionRead::Whole { unit: cache.unit as u64 })
    }

    /// The fallback: one live decode, restarted on a backward seek, serialized
    /// by the reader's mutex. It is what a file whose largest block does not
    /// fit the stated budget is read through ([`BlockCache::affordable`]),
    /// where decoding a block whole would allocate more than the caller
    /// allowed — up to the whole file, on a single-block one.
    fn read_streaming(
        offset: u64,
        len: usize,
        reader: &Mutex<xz_seek::Reader<std::fs::File>>,
        chunks: &Arc<BufferPool>,
    ) -> Result<Bytes> {
        let mut buf = chunks.obtain(len);
        let mut reader = reader.lock().unwrap_or_else(|e| e.into_inner());
        // Mirrors `LocalFileSource::read_range`'s `read_exact_at` contract:
        // `xz_seek::Reader::read_at` is fill-or-EOF, and a short return here
        // means the file ended before the range this caller asked for, which
        // every read loop already clamps `len` to avoid.
        let n = reader.read_at(offset, &mut buf.as_mut()[..len])?;
        drop(reader);
        if n != len {
            return Err(Self::short_read());
        }
        Ok(Bytes::from_owner(buf).slice(..len))
    }
}

impl ByteRangeSource for XzSource {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        Box::pin(async move {
            // A zero-length read covers no blocks, so it is answered here
            // rather than left to look like a read past the end.
            if len == 0 {
                return Ok(Bytes::new());
            }
            let reader = Arc::clone(&self.reader);
            let chunks = Arc::clone(&self.pool);
            let table = Arc::clone(&self.table);
            let file = Arc::clone(&self.data_file);
            let blocks = self.block_path().cloned();
            tokio::task::spawn_blocking(move || match blocks {
                Some(cache) => {
                    Self::read_by_blocks(offset, len, &table, &cache, &reader, &file, &chunks)
                }
                None => Self::read_streaming(offset, len, &reader, &chunks),
            })
            .await
            .map_err(Error::from)?
        })
    }

    /// The uncompressed length, from the seek table built at `open` — no
    /// further decode, no I/O, and no lock.
    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        Box::pin(async move { Ok(self.table.uncompressed_size()) })
    }

    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        Box::pin(async move {
            let file = Arc::clone(&self.stat_file);
            let mtime =
                tokio::task::spawn_blocking(move || file.metadata().and_then(|m| m.modified()))
                    .await
                    .map_err(Error::from)?
                    .map_err(Error::from)?;
            Ok(Some(mtime))
        })
    }

    /// The compressed file's own on-disk length (D4) — a `stat` on the
    /// second handle, never the stream-index walk [`XzSource::size`]
    /// answers from memory.
    fn stored_size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        Box::pin(async move {
            let file = Arc::clone(&self.stat_file);
            let len = tokio::task::spawn_blocking(move || file.metadata().map(|m| m.len()))
                .await
                .map_err(Error::from)?
                .map_err(Error::from)?;
            Ok(len)
        })
    }

    /// The chunk unit is announced to the chunk pool, and the split between
    /// the two pools is re-derived: the chunk pool's share is its own ceiling,
    /// which this number is what sizes ([`XzSource::apportion`]).
    fn hint_read_size(&self, len: usize) {
        self.pool.hint(len);
        self.apportion();
    }

    /// **One stated budget, divided between two read units.** The chunk pool
    /// takes its own ceiling and the block pool takes the rest, so the number
    /// a caller states bounds this source rather than each of its pools; the
    /// worker count is the block pool's depth, one retained block per
    /// concurrent reader (`docs/design/architecture.md`, "The compressed
    /// source").
    ///
    /// A budget too small for a whole block sends every read through the
    /// streaming reader ([`BlockCache::affordable`]) — the budget is honoured
    /// rather than exceeded by a slot the pool floors at one.
    fn hint_parallelism(&self, parallelism: Parallelism) {
        self.budget.store(budget_bytes(parallelism), Ordering::Relaxed);
        self.jobs.store(parallelism.jobs(), Ordering::Relaxed);
        self.apportion();
    }

    /// **The permission reaches the chunk pool only.** A chunk buffer is taken
    /// and released inside one `read_range`, so the loop that granted the wait
    /// holds exactly one of them — which is the discipline
    /// [`BufferPool::obtain`] documents. The **block** pool's holder is
    /// [`BlockCache`], which *retains*: it is the exempt class, exactly as the
    /// replay loop is, so it is left at [`WaitPolicy::NeverWait`] whatever a
    /// loop permits (`docs/design/architecture.md`, "Execution model and API
    /// surface").
    ///
    /// A read that needs both takes the **chunk** slot first and the block
    /// slot inside it ([`XzSource::read_by_blocks`]), never the other way
    /// round, so a waiting reader holds no block slot at all.
    fn hint_wait_policy(&self, policy: WaitPolicy) {
        self.pool.set_policy(policy);
    }

    fn seek_table(&self) -> Option<xz_seek::SeekTable> {
        Some((*self.table).clone())
    }

    fn partitions(&self, range: Range<u64>) -> Partitioning {
        Self::partition_advice(
            &self.table,
            self.block_path().map(|cache| &**cache),
            self.charged_chunk_bytes(),
            self.decode_bytes,
            range,
        )
    }

    fn block_decode_bytes(&self) -> Option<u64> {
        Some(self.block_worker_memory()?.at(1))
    }

    /// **The cores this process was given** — `available_parallelism()`, which
    /// is already the minimum of the affinity mask and every ancestor cgroup's
    /// CPU quota, rounded down and floored at one
    /// (`docs/design/runtime-invariants.md`, `RT7`). So a container told
    /// `--cpus=3.5` recommends three, and nothing here re-derives what `std`
    /// already reads.
    ///
    /// **Decode is the one shape that demonstrably scales** — a compressed
    /// `parse` reaches 5.82× at twenty-four workers and is still climbing
    /// (`docs/design/measurements.md`, "What a second scan worker buys") — so
    /// a small constant such as four would leave the machine's own answer
    /// unspent on the only path that can use it. What the caller's budget
    /// affords still binds afterwards, through the divisor every count passes
    /// (`crate::stream::worker_count`).
    ///
    /// **Capped at this file's own block count**, because `crate::stream::cut`
    /// cuts at block boundaries and there is no seam past the last one: a
    /// four-block file can run four readers however many cores this process
    /// was given. It is capped *here*, where the count is recommended, rather
    /// than left to bind downstream — the recommendation is multiplied into a
    /// budget request ([`ByteRangeSource::default_worker_memory`]) and
    /// printed beside it, so a count the file cannot supply work for becomes
    /// an over-ask and a self-contradicting status line
    /// ([`Parallelism::discover_for`]).
    ///
    /// A failure to read the count answers **one** rather than propagating: the
    /// caller asked what this source would like, and "the serial path" is a
    /// usable answer where an error is not.
    fn default_workers(&self) -> usize {
        let cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        cores.min(self.table.block_count().max(1))
    }

    /// **What readers of this file hold**: [`XzSource::block_worker_memory`] —
    /// the per-reader charge plus the block pool's floor, which is the shape
    /// `crate::stream::worker_count` and [`Parallelism::fit`] both solve
    /// against to hand back a count. `None` where there is no block path to
    /// buy — a file with no blocks, or one whose largest block is not a length
    /// on this target — since the streaming reader has no second worker to
    /// give at any budget.
    ///
    /// **It is charged at the chunk a scan settles at**
    /// ([`XzSource::charged_chunk_bytes`]), which is what
    /// [`BlockCache::affordable`] compares a budget against once the file is
    /// open — so the shape handed to [`Parallelism::fit`] before it is open
    /// and the number the resulting count then meets are the same. Charging
    /// the unannounced pool's [`POOL_MAX_BYTES`] ceiling instead ran the
    /// recommendation a tenth high, which was inert as bytes and not as a
    /// count: the allowance was divided by it, so a 512 MiB cgroup resolved
    /// three readers where four fit. [`ByteRangeSource::block_decode_bytes`]
    /// is this same shape evaluated at one reader, deliberately: one statement
    /// of what the block path costs.
    fn default_worker_memory(&self) -> Option<WorkerMemory> {
        self.block_worker_memory()
    }
}

/// The six bytes every `.xz` stream opens with
/// (`docs/design/architecture.md`, "The compressed source").
const XZ_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];

/// What a caller already knows about a file's compression layer before
/// [`open_local`] has looked at it — normally read out of a cache written
/// from that same file (`crate::cache::claim`), and the whole
/// reason an `.xz` source need not re-walk its stream footers
/// (`docs/design/architecture.md`, "The compressed source").
///
/// **A bare [`xz_seek::SeekTable`], not a cache.** Recognition is the layer
/// that decides which source to build, so it is the layer the table is handed
/// to; what loads it is the caller's business, which is what keeps `io.rs`
/// naming nothing in `crate::cache`.
///
/// Three states, not an `Option`: "the cache says this file is plain" is a
/// claim recognition can *contradict*, and folding it together with "nothing
/// is known" would lose the one case where a cache describes a different file
/// than the one at the path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum KnownCompression {
    /// Nothing is known — recognition reads the file's own bytes and, for
    /// `.xz`, pays the walk. The only state that can never produce a
    /// [`Recognized::Mismatch`], since it claims nothing.
    #[default]
    Unknown,
    /// Known to have no compression layer at all.
    Plain,
    /// Known to be `.xz`, with the seek table a previous walk of this file
    /// produced.
    Xz(xz_seek::SeekTable),
}

/// What [`open_local`] made of a file, given what its caller claimed to know.
pub enum Recognized {
    /// The source, ready to read.
    Source(Arc<dyn ByteRangeSource>),
    /// The [`KnownCompression`] handed in does not describe this file: its
    /// compression layer disagrees with the file's own first bytes, or its
    /// seek table was refused by `xz_seek`'s validation. Nothing beyond the
    /// magic was read and no source was built, so the caller still has both
    /// choices — pay the walk with [`KnownCompression::Unknown`], or report
    /// the cache it came from as unusable without spending a footer walk to
    /// reach an error it was always going to reach
    /// (`docs/design/architecture.md`, "The compressed source").
    ///
    /// The claim it contradicts and the cache's span index were written by
    /// one `save` from one file, so this condemns that index too: a caller
    /// that goes on to scan must not also resume from it.
    Mismatch,
}

/// Open `path` as a [`ByteRangeSource`], choosing between [`LocalFileSource`]
/// and [`XzSource`] by **content**, not by name (D8): the first six bytes are
/// checked against `.xz`'s magic, whatever `path` is called. A file that
/// really is `.xz`-compressed is recognised however it is named or
/// extensionless; a file merely *named* `.xz` whose bytes don't match opens
/// as plain — content sniffing means the two are never confused in either
/// direction, which a path-extension check cannot promise.
///
/// This is a caller convenience layered on top of two sources that stay
/// agnostic of it — an embedder that already knows what it has can construct
/// either directly and skip the read this does. The magic check costs one
/// small read ahead of the source construction that was about to happen
/// anyway.
///
/// `known` is what a caller read out of a cache for this same file, and it is
/// checked rather than believed: recognition still reads the magic, and a
/// claim the file contradicts is [`Recognized::Mismatch`] rather than a
/// silent fallback, because the cache that made the claim is thereby known
/// not to describe this file at all. [`KnownCompression::Unknown`] is the
/// no-knowledge case and always yields a source.
pub fn open_local(path: impl AsRef<Path>, known: KnownCompression) -> Result<Recognized> {
    let path = path.as_ref();
    let is_xz = is_xz_by_magic(path)?;
    match (is_xz, known) {
        // The saving: a table from a previous walk of this file, validated
        // against the file's length and its own internal consistency without
        // reading a byte, and never walked for.
        (true, KnownCompression::Xz(table)) => match XzSource::with_table(path, table) {
            Ok(source) => Ok(Recognized::Source(Arc::new(source))),
            Err(Error::Xz(xz_seek::Error::InvalidTable { .. })) => Ok(Recognized::Mismatch),
            Err(e) => Err(e),
        },
        (true, KnownCompression::Unknown) => {
            Ok(Recognized::Source(Arc::new(XzSource::open(path)?)))
        }
        // Both directions of "the cache describes a different file": a
        // compression index for a file whose bytes are plain, and a plain
        // cache for a file whose bytes are `.xz`.
        (true, KnownCompression::Plain) | (false, KnownCompression::Xz(_)) => {
            Ok(Recognized::Mismatch)
        }
        (false, KnownCompression::Unknown | KnownCompression::Plain) => {
            Ok(Recognized::Source(Arc::new(LocalFileSource::open(path)?)))
        }
    }
}

/// Whether `path`'s first six bytes are `.xz`'s magic. A file shorter than
/// six bytes is answered `false` rather than an error — it cannot be a valid
/// `.xz` file either way, and the plain path already handles an empty or
/// tiny file correctly.
fn is_xz_by_magic(path: &Path) -> Result<bool> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buf = [0u8; XZ_MAGIC.len()];
    match file.read_exact(&mut buf) {
        Ok(()) => Ok(buf == XZ_MAGIC),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(Error::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn source_of(bytes: &[u8]) -> (tempfile::NamedTempFile, LocalFileSource) {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(bytes).unwrap();
        file.flush().unwrap();
        let source = LocalFileSource::open(file.path()).unwrap();
        (file, source)
    }

    /// `LocalFileSource` never overrides [`ByteRangeSource::stored_size`], so
    /// its default — mirroring [`ByteRangeSource::size`] — is what a
    /// non-decompressing source is meant to answer (D4).
    #[tokio::test]
    async fn stored_size_defaults_to_size() {
        let (_file, source) = source_of(b"0123456789abcdef");
        assert_eq!(source.stored_size().await.unwrap(), source.size().await.unwrap());
    }

    /// Every source implemented so far answers its addressable length
    /// exactly (D7); nothing overrides the default yet.
    #[test]
    fn size_is_exact_defaults_true() {
        let (_file, source) = source_of(b"0123456789abcdef");
        assert!(source.size_is_exact());
    }

    /// The pool's whole risk: a reused buffer still holding the previous
    /// read's bytes past the length of the current one. Reading a shorter
    /// range after a longer one from the same source must see the file's
    /// bytes and nothing else.
    #[tokio::test]
    async fn short_read_after_long_read_sees_only_its_own_bytes() {
        let (_file, source) = source_of(b"0123456789abcdef");
        let long = source.read_range(0, 16).await.unwrap();
        assert_eq!(&long[..], b"0123456789abcdef");
        drop(long);
        let short = source.read_range(0, 4).await.unwrap();
        assert_eq!(&short[..], b"0123");
        let elsewhere = source.read_range(10, 3).await.unwrap();
        assert_eq!(&elsewhere[..], b"abc");
    }

    /// A retained chunk must not be handed back out while it is still
    /// referenced — the query path holds chunks past the read that produced
    /// them, and a zero-copy view into a recycled buffer would silently
    /// return another chunk's bytes.
    #[tokio::test]
    async fn a_retained_chunk_is_not_recycled_under_it() {
        let (_file, source) = source_of(b"0123456789abcdef");
        let held = source.read_range(0, 8).await.unwrap();
        let next = source.read_range(8, 8).await.unwrap();
        assert_eq!(&held[..], b"01234567");
        assert_eq!(&next[..], b"89abcdef");
    }

    /// A buffer above the ceiling that no caller announced as its read length
    /// is dropped rather than pooled, so one oversized `attach_text` read
    /// cannot hold megabytes for the life of the process.
    #[test]
    fn the_pool_refuses_a_buffer_above_the_ceiling() {
        let pool = BufferPool::default();
        pool.give(vec![0u8; POOL_MAX_BYTES + 1]);
        assert_eq!(pool.free_len(), 0);
        pool.give(vec![0u8; 64]);
        assert_eq!(pool.free_len(), 1);
    }

    /// The announced read length is what raises the ceiling: a scan configured
    /// with a large chunk keeps its pooling, and a span read longer than that
    /// chunk is still dropped.
    #[test]
    fn the_pool_keeps_a_buffer_of_the_announced_read_length() {
        let chunk = 16 << 20;
        let pool = Arc::new(BufferPool::default());
        pool.give(vec![0u8; chunk]);
        assert_eq!(pool.free_len(), 0);

        pool.hint(chunk);
        pool.give(vec![0u8; chunk]);
        assert_eq!(pool.free_len(), 1);
        // The same read asked for again gets that buffer back rather than a
        // fresh `calloc`, which is the whole point.
        let taken = pool.obtain(chunk);
        assert_eq!(taken.as_ref().len(), chunk);
        assert_eq!(pool.free_len(), 0);
        drop(taken);

        // A buffer that fits a slot is kept and occupies one; one larger than
        // a slot is dropped, which is what makes `held_bytes` a bound.
        pool.give(vec![0u8; chunk - 1]);
        assert_eq!(pool.free_len(), 2);
        pool.give(vec![0u8; chunk + 1]);
        assert_eq!(pool.free_len(), 2);
    }

    /// **The free list holds no more bytes than the pool reports.** Every
    /// buffer it keeps fits a slot and every slot is counted, so
    /// `held_bytes()` is an upper bound rather than the eightfold under-report
    /// it was — which matters because that number is what divides one stated
    /// budget between a source's two pools (`XzSource::apportion`).
    ///
    /// The buffer it now refuses is the parallel plain path's *second* read
    /// unit, a whole partition against a pool whose slot is a chunk. Nothing
    /// but a second pool can hold that honestly ("A two-unit plain source"),
    /// and the under-report was what made it look held.
    #[test]
    fn the_free_list_holds_no_more_than_it_reports() {
        let chunk = 1 << 20;
        let pool = Arc::new(BufferPool::default());
        pool.hint(chunk);
        for _ in 0..8 {
            pool.give(vec![0u8; chunk * PLAIN_PARTITION_CHUNKS]);
        }
        assert_eq!(pool.free_len(), 0, "a partition read is larger than this pool's slot");
        for _ in 0..8 {
            pool.give(vec![0u8; chunk]);
        }
        assert_eq!(pool.free_len(), POOL_DEPTH);
        assert_eq!(pool.free_len() * chunk, pool.held_bytes());
    }

    /// **A reservation comes out of the free list's share, not beside it.**
    /// The one holder that takes slots without leaving them on the free list
    /// is `BlockCache`'s retained list; without this the two are held at
    /// `slots()` each and the pool's ceiling is twice what its budget states.
    #[test]
    fn a_reservation_takes_the_free_list_share() {
        let chunk = 1 << 20;
        let pool = Arc::new(BufferPool::default());
        pool.hint(chunk);
        assert_eq!(pool.slots(), POOL_DEPTH);

        pool.reserve(POOL_DEPTH - 1);
        for _ in 0..POOL_DEPTH {
            pool.give(vec![0u8; chunk]);
        }
        assert_eq!(pool.free_len(), 1, "free plus reserved is one slot count, not two");

        pool.reserve(0);
        pool.give(vec![0u8; chunk]);
        assert_eq!(pool.free_len(), 2, "a released reservation is the free list's again");
    }

    /// The hint reaches the pool through the trait method, which is the only
    /// way a read loop has of announcing anything.
    #[test]
    fn a_source_passes_the_hint_to_its_pool() {
        let (_file, source) = source_of(b"0123456789abcdef");
        let chunk = POOL_MAX_BYTES + 1;
        source.hint_read_size(chunk);
        source.pool.give(vec![0u8; chunk]);
        assert_eq!(source.pool.free_len(), 1);
    }

    /// The library's own default is the serial path, and it is that on both
    /// option structs — an embeddable component does not spawn threads by
    /// surprise, so parallelism is opted into
    /// (`docs/design/architecture.md`, "Execution model and API surface").
    #[test]
    fn the_library_defaults_to_serial() {
        assert_eq!(Parallelism::default(), Parallelism::Serial { memory_bytes: None });
        assert!(crate::scan::ScanOptions::default().parallelism.is_serial());
        assert!(crate::batch::QueryOptions::default().parallelism.is_serial());
        // And the default states no budget, which is not the same fact as
        // stating the number every pool falls back to.
        assert_eq!(Parallelism::default().memory_bytes(), None);
    }

    /// One worker **is** the serial path, so it is spelled that way rather
    /// than as a `Workers` of one nobody can tell from it; zero is a caller's
    /// own arithmetic and reads as one, exactly as `xz_seek::Bulk::new` reads
    /// it.
    ///
    /// **The collapse takes the worker count down and not the budget.** The
    /// two numbers are independent, so a caller asking for one worker inside
    /// a stated budget has still stated it, and every pool sized from
    /// `memory_bytes` sees it — which is the whole of that ask on the serial
    /// path (`docs/design/architecture.md`, "Execution model and API
    /// surface").
    #[test]
    fn one_worker_and_none_are_both_the_serial_state() {
        let serial_in_budget = Parallelism::Serial { memory_bytes: Some(512 << 20) };
        assert_eq!(Parallelism::workers(0, 512 << 20), serial_in_budget);
        assert_eq!(Parallelism::workers(1, 512 << 20), serial_in_budget);
        assert!(serial_in_budget.is_serial());
        assert_eq!(serial_in_budget.jobs(), 1);
        assert_eq!(serial_in_budget.memory_bytes(), Some(512 << 20));
        let eight = Parallelism::workers(8, 512 << 20);
        assert!(!eight.is_serial());
        assert_eq!(
            eight,
            Parallelism::Workers { jobs: NonZeroUsize::new(8).unwrap(), memory_bytes: 512 << 20 }
        );
    }

    /// **A one-worker budget reaches the pool it is a budget for.** The value
    /// carrying it buys nothing unless the source is sized from it, so this
    /// drives the announcement a serial read loop makes rather than the value
    /// alone: 12 KiB of 4 KiB chunks is three slots, where the same source
    /// told nothing keeps `DEFAULT_MEMORY_BUDGET`'s depth.
    #[test]
    fn a_serial_budget_sizes_the_source_it_is_announced_to() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.sql");
        std::fs::write(&path, b"x").unwrap();
        let source = LocalFileSource::open(&path).unwrap();
        source.hint_read_size(4 << 10);

        source.hint_parallelism(Parallelism::workers(1, 12 << 10));
        assert_eq!(source.pool.slots(), 3, "the serial path's stated budget sizes the pool");

        source.hint_parallelism(Parallelism::default());
        assert_eq!(source.pool.slots(), POOL_DEPTH, "stating nothing keeps the default depth");
    }

    /// The pool holds `POOL_DEPTH` buffers and no more, and a take picks the
    /// smallest that fits rather than the first.
    #[test]
    fn the_pool_is_bounded_and_takes_the_smallest_that_fits() {
        let pool = Arc::new(BufferPool::default());
        for len in [8usize, 64, 32, 16, 128] {
            pool.give(vec![0u8; len]);
        }
        assert_eq!(pool.free_len(), POOL_DEPTH);
        let sixteen = pool.obtain(16);
        assert_eq!(sixteen.as_ref().len(), 16);
        let thirty_two = pool.obtain(16);
        assert_eq!(thirty_two.as_ref().len(), 32);
        // Nothing left that fits: a fresh allocation, exactly as long as asked.
        assert_eq!(pool.obtain(1024).as_ref().len(), 1024);
    }

    /// The slot count is a consequence of the announced unit, which is what
    /// makes a decoded xz block a slot the pool can state a bound for: four
    /// 128 MiB slots would be the whole 512 MB cgroup the measurements run
    /// in, so the count falls rather than the budget rising.
    #[test]
    fn the_slot_count_falls_out_of_the_announced_unit() {
        let pool = BufferPool::default();
        // Nothing announced: the ceiling stands in for the unit.
        assert_eq!(pool.slot_bytes(), POOL_MAX_BYTES);
        assert_eq!(pool.slots(), POOL_DEPTH);

        // The two chunk sizes the sweep brackets keep the depth the fixed
        // count gave; a 24 MiB block halves it and a 128 MiB block — larger
        // than the whole budget — gets one slot rather than none, since a
        // pool that refused to keep a block at all would hand back the fresh
        // `calloc` it exists to remove.
        for (unit, slots) in
            [(1 << 20, POOL_DEPTH), (16 << 20, POOL_DEPTH), (24 << 20, 2), (128 << 20, 1)]
        {
            pool.hint(unit);
            assert_eq!(pool.slot_bytes(), unit);
            assert_eq!(pool.slots(), slots, "unit {unit}");
        }
    }

    /// The budget and the depth are both the caller's to state, and each binds
    /// on its own: whichever is smaller decides the count, and the floor of
    /// one survives both.
    #[test]
    fn the_stated_budget_and_depth_each_bound_the_slot_count() {
        let pool = BufferPool::default();
        pool.hint(24 << 20);
        assert_eq!(pool.slots(), 2, "the default budget affords two 24 MiB slots");

        // A larger budget with the same depth: the depth binds.
        pool.set_limits(512 << 20, POOL_DEPTH);
        assert_eq!(pool.slots(), POOL_DEPTH);
        // A larger budget and a larger depth: the budget binds.
        pool.set_limits(512 << 20, 32);
        assert_eq!(pool.slots(), 21);
        // A budget below one slot still keeps one, since a pool that holds
        // nothing hands every reader back the `calloc` it exists to remove.
        pool.set_limits(1 << 20, 8);
        assert_eq!(pool.slots(), 1);
        assert_eq!(pool.held_bytes(), 24 << 20);
    }

    /// A plain file has one read unit, so the stated budget sizes its free
    /// list and the worker count does not — a chunk buffer is taken and
    /// released inside one read, where a retained block is not.
    #[test]
    fn a_plain_source_takes_the_budget_and_not_the_worker_count() {
        let (_file, source) = source_of(b"0123456789abcdef");
        source.hint_read_size(1 << 20);
        assert_eq!(source.pool.slots(), POOL_DEPTH);

        source.hint_parallelism(Parallelism::workers(16, 2 << 20));
        assert_eq!(source.pool.slots(), 2, "the stated budget binds");
        source.hint_parallelism(Parallelism::default());
        assert_eq!(source.pool.slots(), POOL_DEPTH, "the default states no number");
    }

    /// **Backpressure**: a loop that permits a wait gets one instead of
    /// allocating past the stated budget, which is what turns the slot count
    /// into a bound on what is *outstanding* rather than on what is idle.
    ///
    /// Two threads, because a wait has no behaviour except its interaction
    /// with another holder: with one it can only be asserted not to have
    /// blocked, which the unwaiting take already guaranteed. The one loop that
    /// grants this permission is the leader's fused worker, which the mapping
    /// pass now reaches — so this is the wait's *contract*, and
    /// `crate::leader`'s scheduler tests are where it is exercised against a
    /// real source.
    #[test]
    fn a_permitted_wait_takes_a_slot_rather_than_allocating() {
        let unit = 1 << 20;
        let pool = Arc::new(BufferPool::default());
        pool.hint(unit);
        pool.set_limits(unit, POOL_DEPTH);
        pool.set_policy(WaitPolicy::MayWait);
        assert_eq!(pool.slots(), 1, "one slot, so the second holder must wait for the first");

        let held = pool.obtain(unit);
        assert_eq!(pool.charged(), 1);

        let (tx, rx) = std::sync::mpsc::channel();
        let waiting = Arc::clone(&pool);
        let second = std::thread::spawn(move || {
            let slot = waiting.obtain(unit);
            tx.send(()).unwrap();
            drop(slot);
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(150)).is_err(),
            "the only slot is out, so the second acquisition must not complete"
        );

        drop(held);
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("the released slot lets the waiting holder through");
        second.join().unwrap();
        assert_eq!(pool.charged(), 0, "every charge is discharged when its buffer returns");
    }

    /// The exemption, and it is stated by permission rather than by any
    /// setting: a loop that grants no wait allocates past the budget instead.
    /// Three buffers at once against a one-slot pool on **one** thread — a
    /// wait here would be the deadlock the exemption exists to prevent, so
    /// this test hangs rather than fails if the policy is ignored.
    #[test]
    fn an_unpermitted_wait_allocates_past_the_budget_instead() {
        let unit = 1 << 20;
        let pool = Arc::new(BufferPool::default());
        pool.hint(unit);
        pool.set_limits(unit, POOL_DEPTH);
        assert_eq!(pool.slots(), 1);

        // The default, and what all three shipped read loops state.
        assert_eq!(pool.policy(), WaitPolicy::NeverWait);
        let held: Vec<_> = (0..3).map(|_| pool.obtain(unit)).collect();
        assert_eq!(held.len(), 3);
        assert_eq!(pool.charged(), 0, "an exempt read charges no slot");

        drop(held);
        assert_eq!(pool.free_len(), 1, "the slot count still bounds what is kept");
    }

    /// The charge belongs to the buffer, not to the pool's current policy, so
    /// a source may be handed different permissions by successive loops while
    /// an earlier one's buffers are still out. And it is discharged whether
    /// or not the buffer is kept — counting only the kept ones would leak a
    /// slot per refused release until every waiting reader blocked.
    #[test]
    fn a_charge_outlives_the_policy_that_took_it_and_survives_a_refused_release() {
        let pool = Arc::new(BufferPool::default());
        pool.set_policy(WaitPolicy::MayWait);
        let held = pool.obtain(POOL_MAX_BYTES + 1);
        assert_eq!(pool.charged(), 1);

        pool.set_policy(WaitPolicy::NeverWait);
        drop(held);
        assert_eq!(pool.charged(), 0);
        assert_eq!(pool.free_len(), 0, "a buffer above the ceiling is still dropped");
    }

    /// The policy reaches the pool through the trait method, which is the only
    /// way a read loop has of granting one — and the default is to permit no
    /// wait, so a source nobody announces to allocates exactly as it did
    /// before the policy existed.
    #[test]
    fn a_source_states_the_wait_policy_to_its_pool() {
        let (_file, source) = source_of(b"0123456789abcdef");
        assert_eq!(source.pool.policy(), WaitPolicy::NeverWait);
        source.hint_wait_policy(WaitPolicy::MayWait);
        assert_eq!(source.pool.policy(), WaitPolicy::MayWait);
        source.hint_wait_policy(WaitPolicy::NeverWait);
        assert_eq!(source.pool.policy(), WaitPolicy::NeverWait);
    }

    /// A source implementing nothing but the three required methods, to pin
    /// what the *defaults* answer — the only way to read a defaulted body,
    /// since both shipped sources override this one.
    struct BareSource;

    impl ByteRangeSource for BareSource {
        fn read_range(
            &self,
            _offset: u64,
            _len: usize,
        ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
            Box::pin(async { Ok(Bytes::new()) })
        }
        fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
            Box::pin(async { Ok(0) })
        }
        fn modified(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
            Box::pin(async { Ok(None) })
        }
    }

    /// A source that has not thought about being read concurrently advises
    /// one partition, so a scheduler cannot split it on the assumption that
    /// silence means consent.
    #[test]
    fn a_source_that_does_not_advise_gets_one_partition() {
        let advice = BareSource.partitions(0..1 << 30);
        assert_eq!(advice.max_partitions(), Some(1));
        assert_eq!(advice.boundaries(), &PartitionBoundaries::At(Vec::new()));
        assert_eq!(advice.partition_bytes(), 0);
    }

    /// **Silence recommends the serial path**, the same convention
    /// `partitions` above answers with — and [`LocalFileSource`] inherits it
    /// rather than overriding, because a plain `parse` is slower than serial at
    /// every worker count measured ("Where a scan's time goes"). Only a reading
    /// showing a plain parallel `parse` beating serial reopens that, and this
    /// is where it would be reopened.
    #[test]
    fn a_source_that_does_not_advise_recommends_the_serial_path() {
        assert_eq!(BareSource.default_workers(), 1);
        let (_file, source) = source_of(b"0123456789abcdef");
        assert_eq!(source.default_workers(), 1);
    }

    /// A plain file prefers no split point to another, and a partition costs
    /// [`PLAIN_PARTITION_CHUNKS`] read chunks — the caller's own announced
    /// number where it announced one, so the size scales with the unit the
    /// pool sizes a slot by, and never past the ceiling above which a released
    /// buffer stops being kept.
    ///
    /// **What a partition is a multiple of, and where the multiple stops, are
    /// two rules and both are asserted**: a hint under the ceiling multiplies,
    /// one that would carry the product past it is capped, and one already at
    /// or past it is a partition on its own.
    #[test]
    fn a_plain_file_advises_anywhere_at_several_chunks_each() {
        let (_file, source) = source_of(b"0123456789abcdef");
        let advice = source.partitions(0..16);
        assert_eq!(advice.boundaries(), &PartitionBoundaries::Anywhere);
        assert_eq!(advice.max_partitions(), None);
        assert_eq!(
            advice.retained_unit(),
            RetainedUnit::ReadChunk,
            "a batch pins the chunks it was read into, which partition_bytes has not charged"
        );
        // Nothing announced: the slot size is the ceiling, and the multiple is
        // capped back to it.
        assert_eq!(advice.partition_bytes(), POOL_MAX_BYTES as u64);

        source.hint_read_size(64 << 10);
        assert_eq!(
            source.partitions(0..16).partition_bytes(),
            (PLAIN_PARTITION_CHUNKS as u64) * (64 << 10)
        );

        source.hint_read_size(4 << 20);
        assert_eq!(source.partitions(0..16).partition_bytes(), POOL_MAX_BYTES as u64);

        source.hint_read_size(16 << 20);
        assert_eq!(source.partitions(0..16).partition_bytes(), 16 << 20);
    }

    /// **The shipped configuration must sit *inside* the cap, not on it.**
    /// `DEFAULT_CHUNK_SIZE * PLAIN_PARTITION_CHUNKS` is exactly
    /// [`POOL_MAX_BYTES`] today, and the three constants are justified
    /// independently — the chunk by the read-chunk sweep, the multiple by
    /// where the tail's returns flatten, the ceiling by one-off-ness. So
    /// nothing but this assertion stops a later change to any one of them from
    /// silently converting the default from "eight whole chunks" to "capped",
    /// which is the case [`PLAIN_PARTITION_CHUNKS`] exists to prevent and the
    /// one no other test would notice.
    #[test]
    fn a_shipped_plain_partition_is_eight_whole_chunks() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let source = LocalFileSource::open(file.path()).unwrap();
        source.hint_read_size(crate::DEFAULT_CHUNK_SIZE);
        assert_eq!(
            source.partitions(0..16).partition_bytes(),
            (PLAIN_PARTITION_CHUNKS as u64) * (crate::DEFAULT_CHUNK_SIZE as u64),
            "the shipped chunk size must yield a whole multiple, uncapped"
        );
    }

    /// A block-decoding `.xz` advises its block boundaries, so a partition is
    /// a whole number of blocks and two workers never decode one block twice.
    /// The boundaries are ascending and strictly inside the range: the block
    /// containing `range.start` begins at or before it, and is not a split.
    #[tokio::test]
    async fn an_xz_source_advises_its_block_boundaries() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let source = XzSource::open(compressed.path()).unwrap();
        let table = source.seek_table().unwrap();
        let starts: Vec<u64> = table.blocks.iter().map(|b| b.uncompressed_offset).collect();
        assert!(starts.len() > 2, "this test needs several blocks, got {}", starts.len());

        let whole = source.partitions(0..payload.len() as u64);
        assert_eq!(whole.boundaries(), &PartitionBoundaries::At(starts[1..].to_vec()));
        assert_eq!(whole.max_partitions(), Some(starts.len()));

        // A range opening inside a block: that block's own start is behind
        // `range.start` and is not offered as a split.
        let from = starts[1] + 10;
        let inner = source.partitions(from..payload.len() as u64);
        assert_eq!(inner.boundaries(), &PartitionBoundaries::At(starts[2..].to_vec()));

        // A range inside one block has nothing to split at.
        let one = source.partitions(from..from + 10);
        assert_eq!(one.max_partitions(), Some(1));

        // A partition costs what one concurrent reader holds: two block slots
        // — the one being decoded and the one retained beside it — the chunk
        // buffer a straddling read is assembled into, and the decoder's own
        // retention, which is a published number rather than a guess. The
        // chunk term is the one a scan settles at, no read loop having
        // announced one here ([`XzSource::charged_chunk_bytes`]).
        let unit = source.blocks.as_ref().unwrap().unit as u64;
        assert_eq!(
            whole.partition_bytes(),
            2 * unit + crate::DEFAULT_CHUNK_SIZE as u64 + source.decode_bytes
        );
        assert_eq!(whole.worker_memory().bytes_per_worker(), whole.partition_bytes());

        // **And the gate is that same charge at one reader**, which is the
        // per-reader term plus the pool floor a single reader leaves unfilled
        // — `(POOL_DEPTH - 1)` units nobody else is there to take
        // ([`WorkerMemory`]). That is what a budget has to clear for the block
        // path to be taken at all, so it is what a declined source names as
        // the recourse.
        assert_eq!(
            source.block_decode_bytes(),
            Some(whole.partition_bytes() + (POOL_DEPTH as u64 - 1) * unit)
        );
        assert_eq!(source.block_decode_bytes(), Some(whole.worker_memory().at(1)));

        // And what a retained batch pins is that same block, which the line
        // above has already charged for — so a caller must not add its span
        // allowance on top ([`RetainedUnit`]).
        assert_eq!(whole.retained_unit(), RetainedUnit::Partition);
    }

    /// **A streaming-fallback source advises one partition even though its
    /// table has boundaries.** Reaching an offset inside a block there means
    /// restarting that block and discarding forward, so two workers each force
    /// the other's restart and parallel mode is worse than serial. Asserted
    /// against a synthetic multi-block table, `partition_advice` being a free
    /// function precisely so that the fallback arm can be handed a table that
    /// *does* have boundaries.
    #[test]
    fn a_streaming_fallback_source_advises_one_partition() {
        let table = four_block_table();
        assert!(table.is_seekable(), "the table has boundaries to advise");

        let cache = BlockCache::for_table(&table).expect("4 KiB blocks decode whole");
        let decoding =
            XzSource::partition_advice(&table, Some(&cache), 1 << 20, 9 << 20, 0..4 * 4096);
        assert_eq!(decoding.max_partitions(), Some(4));
        assert_eq!(decoding.retained_unit(), RetainedUnit::Partition);
        assert_eq!(
            decoding.partition_bytes(),
            2 * 4096 + (1 << 20) + (9 << 20),
            "two block slots, the chunk buffer, and the decoder's own retention"
        );

        let streaming = XzSource::partition_advice(&table, None, 1 << 20, 9 << 20, 0..4 * 4096);
        assert_eq!(streaming.max_partitions(), Some(1));
        assert_eq!(
            streaming.partition_bytes(),
            1 << 20,
            "one chunk buffer a reader; the fallback's single decoder is a cost of the source"
        );
        assert_eq!(
            streaming.retained_unit(),
            RetainedUnit::ReadChunk,
            "a streaming read is assembled into a chunk buffer, so chunks are what a batch pins"
        );
        // **The read shape is stated apart from both of those**, and the two
        // arms differ in it: a block-decoding piece is one block and reading it
        // whole copies nothing, while a streaming read is assembled into a
        // chunk buffer whatever length is asked for. The unit the block arm
        // states is the table's own largest block — what bounds the body read
        // at a cut width the measurement may yet raise, rather than the piece.
        assert_eq!(decoding.partition_read(), PartitionRead::Whole { unit: 4096 });
        assert_eq!(
            table.max_block_uncompressed(),
            4096,
            "the stated unit is the table's largest block, not the piece"
        );
        assert_eq!(streaming.partition_read(), PartitionRead::Chunked);
    }

    /// A four-block seek table over 4 KiB blocks, built by hand rather than by
    /// `xz`: the block *cache* only ever reads a table's block lengths, so a
    /// synthetic one lets a slot be small enough for a budget to be stated in
    /// whole slots.
    fn four_block_table() -> xz_seek::SeekTable {
        block_table(4)
    }

    /// The same table at `count` blocks, for the one test that needs more
    /// boundaries than workers: a bound on how many units a piece spans is
    /// vacuous on a file whose every window runs out of boundaries first.
    fn block_table(count: u64) -> xz_seek::SeekTable {
        let block = |i: u64| xz_seek::BlockEntry {
            compressed_offset: 12 + i * 128,
            uncompressed_offset: i * 4096,
            unpadded_size: 64,
            uncompressed_size: 4096,
        };
        xz_seek::SeekTable {
            compressed_file_size: 1 << 20,
            streams: vec![xz_seek::StreamEntry {
                compressed_offset: 0,
                uncompressed_offset: 0,
                compressed_size: 1 << 20,
                uncompressed_size: count * 4096,
                check: xz_seek::Check::Crc64,
                first_block_dict_size: Some(8 << 20),
                padding: 0,
                first_block: 0,
                block_count: count as usize,
            }],
            blocks: (0..count).map(block).collect(),
        }
    }

    /// **Every partition the leader hands a worker covers at most
    /// [`BOUNDARIED_PARTITION_UNITS`] of this source's units**, whatever the
    /// charge says and whatever the worker count is — which is what makes the
    /// cut width a number somebody chose rather than a consequence of the
    /// memory charge.
    ///
    /// This is the invariant three doc comments and `architecture.md` asserted
    /// in prose while the code had stopped honouring it: `partition_bytes` was
    /// both the memory charge and `run_region`'s cut size, so raising the
    /// charge to `BlockCache::reader_bytes` widened the window until
    /// `crate::stream::cut` had to thin the boundaries on offer, and a piece
    /// became 2.08–2.42 blocks. The repair is
    /// [`Partitioning::window_end`], which sizes the window in the source's own
    /// units; this test is what stops the two consumers being confused again.
    ///
    /// It walks the window loop rather than one window: a frontier lands
    /// wherever the last window's final row ended, and a *mid-block* start is
    /// the case a single aligned window would never show. The bound holds at
    /// the ragged end too — a window with fewer boundaries left than it asked
    /// for runs to the limit, and thinning a shorter list cannot give a piece
    /// more units than a full one would.
    #[test]
    fn a_block_decoding_partition_spans_at_most_the_cut_width() {
        // Enough blocks that a window of `workers × BOUNDARIED_PARTITION_UNITS`
        // is satisfied out of the boundary list rather than running off the end
        // of the file, which is what makes the bound below bind.
        let blocks = 8 * BOUNDARIED_PARTITION_UNITS as u64 + 4;
        let table = block_table(blocks);
        let size = blocks * 4096;
        let cache = BlockCache::for_table(&table).expect("4 KiB blocks decode whole");
        let advice = XzSource::partition_advice(&table, Some(&cache), 1 << 20, 9 << 20, 0..size);
        // The charge is two blocks, a chunk and the decoder — three orders of
        // magnitude above the 4 KiB unit here, which is what made a window
        // sized by it swallow the whole file.
        assert!(advice.partition_bytes() > size, "the charge is not the cut size");

        for workers in [2usize, 3, 4, 8] {
            for start in [0u64, 1, 4095, 4096, 5000, 8192, 12287] {
                assert!(start < size, "a frontier inside the file");
                let end = advice.window_end(start, workers, size);
                assert!(end > start, "a window must advance: {workers} workers from {start}");
                let ranges = crate::stream::cut(start..end, &advice, workers);
                assert!(
                    ranges.len() <= workers,
                    "{workers} workers were handed {} pieces from {start}",
                    ranges.len()
                );
                for piece in ranges {
                    let covering = table.blocks_in(piece.clone());
                    assert!(
                        covering.len() <= BOUNDARIED_PARTITION_UNITS,
                        "{piece:?} spans {} blocks at {workers} workers from {start}, \
                         against a cut width of {BOUNDARIED_PARTITION_UNITS}",
                        covering.len()
                    );
                }
            }
        }
    }

    /// **A plain file's window is unchanged by the split above**: with no
    /// boundaries to respect the charge *is* the cut size, so a window is
    /// `workers` charges wide exactly as it always was.
    #[test]
    fn a_plain_window_is_still_the_charge_times_the_worker_count() {
        let (_file, source) = source_of(&[0u8; 64]);
        source.hint_read_size(8);
        let advice = source.partitions(0..64);
        assert_eq!(
            advice.partition_read(),
            PartitionRead::Chunked,
            "a plain partition is read a chunk at a time, so every buffer is a pooled one"
        );
        assert_eq!(
            BareSource.partitions(0..1 << 30).partition_read(),
            PartitionRead::Chunked,
            "and a source that has said nothing is read the conservative way"
        );
        let charge = advice.partition_bytes();
        assert_eq!(advice.window_end(0, 3, u64::MAX), 3 * charge);
        assert_eq!(advice.window_end(charge, 3, u64::MAX), 4 * charge);
        assert_eq!(advice.window_end(0, 3, 17), 17, "the limit binds");
    }

    /// **Whole-block decode is declined unless the budget affords the two
    /// blocks a reader holds *and* the pool floor beside them.** A pool of one
    /// slot retains nothing — `BlockCache::slot` drains to `slots() - 1`
    /// before every decode — so it stops pooling exactly when a caller is
    /// holding a block, which is why a reader is charged two units; and
    /// `BufferPool::slots` clamps this pool at `POOL_DEPTH.max(jobs)`, so a
    /// single reader leaves `(POOL_DEPTH - 1)` units standing that nothing
    /// else is there to fill. [`POOL_DEPTH`] units is therefore the line, not
    /// two.
    #[test]
    fn the_block_path_wants_room_for_two_blocks_and_the_pool_floor() {
        let table = four_block_table();
        let cache = BlockCache::for_table(&table).expect("4 KiB blocks have a unit");
        // The two terms a decline does not save, held at zero so that this
        // test is about the block slots alone; the test above is where they
        // bind.
        let unit = cache.unit as u64;
        let floor = POOL_DEPTH as u64 - 1;

        assert!(
            cache.affordable(0, 0, unit * (2 + floor)),
            "two units for the reader, and the floor the pool holds beside it"
        );
        cache.pool.set_limits(cache.unit * 2, POOL_DEPTH);
        assert_eq!(cache.pool.slots(), 2);

        assert!(
            !cache.affordable(0, 0, unit * (2 + floor) - 1),
            "a byte short of the floor is the un-poolable shape, so it is declined"
        );
        // Two units alone were the whole of the old line, and the floor is
        // what it left unbilled.
        assert!(!cache.affordable(0, 0, unit * 2));
        // And the reader's other two terms are inside the same number: the
        // same budget that afforded the blocks declines them once a chunk
        // buffer and a decoder are charged beside it.
        assert!(!cache.affordable(1, 1, unit * (2 + floor)));
    }

    /// **One slot count covers the retained blocks and the free ones
    /// together**, so a block pool's ceiling is its stated budget rather than
    /// twice it. Driven through the two calls the decode path makes — a slot,
    /// then a retention — with the most recent block held live, which is what a
    /// reader does with the `Bytes` it sliced out of it.
    #[test]
    fn a_block_pool_holds_one_slot_count_across_both_lists() {
        let table = four_block_table();
        let cache = BlockCache::for_table(&table).expect("4 KiB blocks have a unit");
        cache.pool.set_limits(cache.unit * 3, POOL_DEPTH);
        assert_eq!(cache.pool.slots(), 3);

        let mut live = None;
        for index in 0..12 {
            let slot = cache.slot();
            let block = Arc::new(DecodedBlock { slot, len: cache.unit });
            cache.retain(index, Arc::clone(&block));
            live = Some(block);
            let retained = cache.retained.lock().unwrap().len();
            let free = cache.pool.free_len();
            assert!(
                retained + free <= cache.pool.slots(),
                "read {index}: {retained} retained + {free} free exceeds {} slots",
                cache.pool.slots(),
            );
        }
        drop(live);
    }

    /// `xz` is not `mise`-pinned (`docs/design/architecture.md`, "Testing
    /// philosophy"), so a
    /// missing binary fails with the remedy rather than skipping silently
    /// (`docs/design/roadmap.md`, "A test may assume the tools `mise`
    /// pins").
    fn require_xz() {
        let ok = std::process::Command::new("xz")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
        assert!(ok, "`xz` is not runnable, so this test cannot build its fixture; install it.");
    }

    /// Compress `bytes` with `xz`, passing `extra_args` before the input path
    /// (e.g. `&["--block-size=4096"]` for a seekable multi-block file, or
    /// `&[]` for a single-stream, single-block one), and return the temp
    /// file holding the compressed bytes.
    fn xz_compress(bytes: &[u8], extra_args: &[&str]) -> tempfile::NamedTempFile {
        require_xz();
        let mut input = tempfile::NamedTempFile::new().unwrap();
        input.write_all(bytes).unwrap();
        input.flush().unwrap();
        let out = std::process::Command::new("xz")
            .args(extra_args)
            .arg("-c")
            .arg(input.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "xz {extra_args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let mut compressed = tempfile::NamedTempFile::new().unwrap();
        compressed.write_all(&out.stdout).unwrap();
        compressed.flush().unwrap();
        compressed
    }

    /// A payload long enough that a small `--block-size` reliably splits it
    /// into several blocks — not `edge_cases.sql`, which the CLI-level
    /// differential tests use; this is a throwaway pattern for pinning
    /// `XzSource`'s own wiring.
    fn xz_test_payload() -> Vec<u8> {
        (0..20_000u32).map(|i| (i % 251) as u8).collect()
    }

    /// The multi-block shape: `size()` is the exact uncompressed length from
    /// the seek table, with no read yet.
    #[tokio::test]
    async fn xz_source_size_is_the_uncompressed_length() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let source = XzSource::open(compressed.path()).unwrap();
        assert_eq!(source.size().await.unwrap(), payload.len() as u64);
        assert!(source.size_is_exact());
    }

    /// `stored_size()` is a `stat` on the compressed file itself (D4), not
    /// the uncompressed length `size()` answers — the two must disagree for
    /// a payload that actually compresses.
    #[tokio::test]
    async fn xz_source_stored_size_is_the_compressed_files_on_disk_length() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let on_disk = std::fs::metadata(compressed.path()).unwrap().len();
        let source = XzSource::open(compressed.path()).unwrap();
        assert_eq!(source.stored_size().await.unwrap(), on_disk);
        assert_ne!(source.stored_size().await.unwrap(), source.size().await.unwrap());
    }

    /// `seek_table()` is what `cache::save` wraps into
    /// `CompressionIndex::Xz` — a multi-block file must actually report more
    /// than one block, or the "seekable" half of this test proves nothing.
    #[tokio::test]
    async fn xz_source_seek_table_reports_every_block() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let source = XzSource::open(compressed.path()).unwrap();
        let table = source.seek_table().expect("an XzSource always has a table");
        assert!(table.is_seekable(), "block_count = {}", table.block_count());
        assert_eq!(table.uncompressed_size(), payload.len() as u64);
    }

    /// **A compressed source recommends the cores it was given, capped at its
    /// own block count** — one recommendation with two halves, since
    /// `crate::stream::cut` cuts at block boundaries and a file with fewer
    /// blocks than the machine has cores offers no seam for the rest. The cap
    /// is applied where the count is recommended because the count is
    /// multiplied into a budget request and printed beside it.
    ///
    /// The core count is asserted against `available_parallelism()` rather
    /// than against a literal: it is the machine's, and the property being
    /// pinned is that this source asks `std` rather than carrying a constant
    /// of its own — which is what makes a container's CPU quota reach the
    /// default (`docs/design/runtime-invariants.md`, `RT7`).
    #[test]
    fn an_xz_source_recommends_the_cores_it_was_given_capped_at_its_block_count() {
        let cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);

        // More blocks than any machine has cores, so `std`'s answer is what
        // binds and the cap is inert.
        let wide: Vec<u8> = (0..4096u32 * 512).map(|i| (i % 251) as u8).collect();
        let compressed = xz_compress(&wide, &["--block-size=4096"]);
        let source = XzSource::open(compressed.path()).unwrap();
        assert!(source.table.block_count() > cores, "{} blocks", source.table.block_count());
        assert_eq!(source.default_workers(), cores);

        // A handful of blocks: the *file* binds on any machine wider than it,
        // which is the half that was missing.
        let narrow = xz_compress(&xz_test_payload(), &["--block-size=4096"]);
        let source = XzSource::open(narrow.path()).unwrap();
        let blocks = source.table.block_count();
        assert!((2..64).contains(&blocks), "{blocks} blocks");
        assert_eq!(source.default_workers(), cores.min(blocks));
        if cores > blocks {
            assert_eq!(source.default_workers(), blocks, "the file is what binds here");
        }
    }

    /// Reading forward in chunks that each land inside one block, straddle a
    /// block boundary, or span several — the shape every real read loop in
    /// this crate produces — must reproduce the plaintext exactly.
    #[tokio::test]
    async fn xz_source_reads_forward_matching_plain_bytes() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let source = XzSource::open(compressed.path()).unwrap();

        let mut offset = 0u64;
        for len in [100usize, 4096, 3000, 5000] {
            let got = source.read_range(offset, len).await.unwrap();
            assert_eq!(&got[..], &payload[offset as usize..offset as usize + len]);
            offset += len as u64;
        }
    }

    /// A read at an offset behind the live decode's position must restart
    /// the covering block rather than return the wrong bytes — this is the
    /// only path `LocalFileSource` never has, and it is D6's whole reason to
    /// exist.
    #[tokio::test]
    async fn xz_source_seeks_backward_across_a_block_boundary() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let source = XzSource::open(compressed.path()).unwrap();

        // Walk forward past several block boundaries first, exactly like the
        // multi-chunk test above.
        let far = source.read_range(15_000, 2000).await.unwrap();
        assert_eq!(&far[..], &payload[15_000..17_000]);

        // Then jump backward into an earlier block.
        let near = source.read_range(500, 1000).await.unwrap();
        assert_eq!(&near[..], &payload[500..1500]);

        // And forward again, past where the first read left off — the live
        // decode was rebuilt by the backward read, so this is a second
        // restart, not a continuation of the first pass.
        let later = source.read_range(18_000, 2000).await.unwrap();
        assert_eq!(&later[..], &payload[18_000..20_000]);
    }

    /// The block a read landed in is retained, so the next read inside it
    /// costs a slice rather than a second decode — which is what makes
    /// per-call block decode affordable under a chunked read loop.
    #[tokio::test]
    async fn a_read_retains_the_block_it_landed_in() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let source = XzSource::open(compressed.path()).unwrap();
        let cache = source.blocks.clone().expect("4 KiB blocks are decoded whole");
        assert!(cache.retained.lock().unwrap().is_empty());

        let first = source.read_range(100, 200).await.unwrap();
        assert_eq!(&first[..], &payload[100..300]);
        let retained = cache.retained.lock().unwrap().clone();
        assert_eq!(retained.len(), 1, "one read, one block");
        let (index, block) = retained.into_iter().next().unwrap();
        assert_eq!(index, 0);

        // A second read inside the same block is served from that same
        // decoded block: the view and the retention list are the only two
        // holders after the first read's `Bytes` are dropped.
        drop(first);
        assert_eq!(Arc::strong_count(&block), 2, "the retention list and this handle");
        let second = source.read_range(1000, 500).await.unwrap();
        assert_eq!(&second[..], &payload[1000..1500]);
        assert_eq!(cache.retained.lock().unwrap().len(), 1);
        assert_eq!(Arc::strong_count(&block), 3, "plus the view the second read handed back");
    }

    /// The retention list holds no more than the pool's slot count, so a walk
    /// across many blocks does not accumulate decoded plaintext.
    #[tokio::test]
    async fn retention_is_bounded_by_the_pools_slot_count() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let source = XzSource::open(compressed.path()).unwrap();
        let cache = source.blocks.clone().unwrap();
        let slots = cache.pool.slots();

        let mut offset = 0u64;
        while offset < payload.len() as u64 {
            let len = 100.min(payload.len() as u64 - offset) as usize;
            source.read_range(offset, len).await.unwrap();
            assert!(cache.retained.lock().unwrap().len() <= slots);
            offset += 4096;
        }
        assert!(cache.retained.lock().unwrap().len() <= slots);
    }

    /// **A read served from a retained block never touches the reader**,
    /// which is the serialization point this source is losing: with the
    /// reader's mutex held by another thread outright, the read still answers.
    /// The lock is taken only to name a block's task, and a block already
    /// decoded needs no task.
    #[tokio::test]
    async fn a_retained_block_is_read_without_the_reader() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let source = XzSource::open(compressed.path()).unwrap();

        // Warm the block this read lands in: 9_000..10_000 is inside the
        // 8_192..12_288 block, so one decode covers both reads.
        source.read_range(9_000, 100).await.unwrap();

        // Held on a thread of its own rather than across the await, so the
        // guard's lifetime is not entangled with the future being tested.
        let (locked, is_locked) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let reader = Arc::clone(&source.reader);
        let holder = std::thread::spawn(move || {
            let guard = reader.lock().unwrap_or_else(|e| e.into_inner());
            locked.send(()).unwrap();
            released.recv().unwrap();
            drop(guard);
        });
        is_locked.recv().unwrap();

        let got = source.read_range(9_000, 1_000).await.unwrap();
        assert_eq!(&got[..], &payload[9_000..10_000]);

        release.send(()).unwrap();
        holder.join().unwrap();
    }

    /// Concurrent reads over different blocks all answer their own bytes —
    /// the property `XzSource` could not have before, since every read ran
    /// under one mutex.
    #[tokio::test]
    async fn concurrent_reads_each_answer_their_own_bytes() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let source = Arc::new(XzSource::open(compressed.path()).unwrap());

        let mut tasks = Vec::new();
        for offset in (0..16_000u64).step_by(997) {
            let source = Arc::clone(&source);
            tasks.push(tokio::spawn(async move {
                (offset, source.read_range(offset, 900).await.unwrap())
            }));
        }
        for task in tasks {
            let (offset, got) = task.await.unwrap();
            let at = offset as usize;
            assert_eq!(&got[..], &payload[at..at + 900], "at {offset}");
        }
    }

    /// **A block the stated budget cannot hold is not decoded whole**, and the
    /// line is the budget rather than a constant beside it: a single block
    /// holding the whole plaintext is what a bare `xz` writes, and the
    /// streaming reader is what such a file is read through. Asserted against
    /// a synthetic table, that being the only way to have a block this size
    /// without writing one.
    #[test]
    fn a_block_too_large_to_hold_refuses_the_block_path() {
        let block = |uncompressed_size| xz_seek::BlockEntry {
            compressed_offset: 12,
            uncompressed_offset: 0,
            unpadded_size: 64,
            uncompressed_size,
        };
        let table = |uncompressed_size| xz_seek::SeekTable {
            compressed_file_size: 1 << 20,
            streams: vec![xz_seek::StreamEntry {
                compressed_offset: 0,
                uncompressed_offset: 0,
                compressed_size: 1 << 20,
                uncompressed_size,
                check: xz_seek::Check::Crc64,
                first_block_dict_size: Some(8 << 20),
                padding: 0,
                first_block: 0,
                block_count: 1,
            }],
            blocks: vec![block(uncompressed_size)],
        };
        // A cache exists for any file with blocks; whether the block path is
        // *taken* is the budget's answer, asked again on every read. The
        // decoder's charge is stated here rather than read off a reader — a
        // synthetic table has none — and stands for what
        // `xz_seek::Reader::decode_footprint` answers on an 8 MiB-dictionary
        // file: the dictionary, a 1 MiB input chunk and the backend's own
        // state, 9,471,776 bytes on koji's shape.
        const DECODE: u64 = 9_471_776;
        let affordable = |unit: u64, budget: u64| {
            let cache = BlockCache::for_table(&table(unit)).expect("a block to build a unit from");
            cache.affordable(crate::DEFAULT_CHUNK_SIZE as u64, DECODE, budget)
        };
        // **`POOL_DEPTH` units, not two.** A single reader is charged the two
        // blocks it holds *and* the pool floor beside them
        // ([`BlockCache::worker_memory`]), so koji's 24 MiB blocks want
        // 5 x 24 MiB plus the chunk and the decoder — 131 MiB — where the
        // per-reader term alone was 58 and fitted the default. The default
        // budget therefore declines an ordinary compressed dump, which is the
        // first arrangement in which that stated number is true: the pool held
        // four units under it either way (`docs/design/architecture.md`,
        // "Execution model and API surface").
        assert!(!affordable(24 << 20, DEFAULT_MEMORY_BUDGET));
        assert!(affordable(24 << 20, 131 << 20));
        // 128 and 192 MiB blocks — `xz --block-size=128MiB`, and `xz -9 -T0`,
        // whose block is three times its 64 MiB dictionary — are declined at
        // the default and taken once the caller allows the room.
        assert!(!affordable(128 << 20, DEFAULT_MEMORY_BUDGET));
        assert!(!affordable(192 << 20, DEFAULT_MEMORY_BUDGET));
        assert!(!affordable(192 << 20, 512 << 20), "five units of 192 MiB is 960");
        assert!(affordable(192 << 20, 971 << 20));
        // **The unavoidable terms come off the top.** Five 24 MiB blocks fit a
        // 120 MiB budget on their own and the file is declined all the same,
        // because a reader of it also holds the chunk buffer and the decoder —
        // both of which the streaming fallback holds too, which is why they
        // bound the decision rather than being saved by it.
        assert!(!affordable(24 << 20, 120 << 20));
        // A file with no blocks at all — `xz -c /dev/null` writes one — has no
        // unit to size a slot with, so there is no cache to ask.
        let empty = xz_seek::SeekTable {
            compressed_file_size: 32,
            streams: Vec::new(),
            blocks: Vec::new(),
        };
        assert!(BlockCache::for_table(&empty).is_none());
    }

    /// The stated budget bounds the **source**, not each of its pools: the
    /// chunk pool takes its own ceiling and the block pool takes what is left,
    /// and the two hints may arrive in either order because the split is
    /// re-derived from the stated number rather than composed incrementally.
    #[tokio::test]
    async fn a_stated_budget_is_divided_between_the_two_pools() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let held = |source: &XzSource| {
            let blocks = source.blocks.as_ref().unwrap();
            (source.pool.held_bytes(), blocks.pool.held_bytes(), blocks.pool.slots())
        };

        // Budget then chunk size, and chunk size then budget: same split.
        let one = XzSource::open(compressed.path()).unwrap();
        one.hint_parallelism(Parallelism::workers(8, 32 << 20));
        one.hint_read_size(1 << 20);
        let two = XzSource::open(compressed.path()).unwrap();
        two.hint_read_size(1 << 20);
        two.hint_parallelism(Parallelism::workers(8, 32 << 20));
        assert_eq!(held(&one), held(&two));

        let (chunks, blocks, slots) = held(&one);
        assert_eq!(chunks, POOL_DEPTH * (1 << 20), "four slots of the announced chunk");
        assert_eq!(blocks, slots * 4096);
        assert!(chunks + blocks <= 32 << 20, "the two pools sum inside the stated budget");
        // Eight workers, so eight retained blocks where the budget affords
        // them — the depth is the caller's number and the floor is the pool's.
        assert_eq!(slots, 8);
        one.hint_parallelism(Parallelism::default());
        assert_eq!(held(&one).2, POOL_DEPTH, "stating nothing keeps the pool's own depth");
    }

    /// **A retained block is storage, not a waiting holder, so no read loop's
    /// permission reaches the block pool.** Every retained block is also the
    /// one some reader is holding a view into — that is what zero-copy means
    /// here — so `BlockCache::slot`'s drain to `slots() - 1` can free nothing
    /// the waiting reader itself is not holding, and a charged block pool
    /// reaches `slots()` permanently the moment a caller holds one view per
    /// slot.
    ///
    /// The third read below is what deadlocked: it runs on a detached thread
    /// against a bounded receive, so a pool that starts charging block slots
    /// again fails this test rather than hanging the suite.
    #[test]
    fn a_permitted_wait_never_reaches_the_block_pool() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let source = XzSource::open(compressed.path()).unwrap();
        // 12 KiB: four chunk slots of the announced 1 KiB, and the 8 KiB left
        // over is two 4 KiB block slots.
        source.hint_read_size(1 << 10);
        source.hint_parallelism(Parallelism::workers(2, 12 << 10));
        source.hint_wait_policy(WaitPolicy::MayWait);
        let cache = Arc::clone(source.blocks.as_ref().expect("4 KiB blocks are decoded whole"));
        assert_eq!(cache.pool.slots(), 2, "two block slots inside the stated budget");

        let read = |offset: u64| {
            XzSource::read_by_blocks(
                offset,
                100,
                &source.table,
                &cache,
                &source.reader,
                &source.data_file,
                &source.pool,
            )
        };
        // One live view per slot, and both blocks are retained as well.
        let held = vec![read(0).unwrap(), read(4096).unwrap()];
        assert_eq!(cache.retained.lock().unwrap().len(), 2);

        let (tx, rx) = std::sync::mpsc::channel();
        let (table, blocks, reader, file, chunks) = (
            Arc::clone(&source.table),
            Arc::clone(&cache),
            Arc::clone(&source.reader),
            Arc::clone(&source.data_file),
            Arc::clone(&source.pool),
        );
        std::thread::spawn(move || {
            let got = XzSource::read_by_blocks(8192, 100, &table, &blocks, &reader, &file, &chunks);
            let _ = tx.send(got.map(|bytes| bytes.to_vec()));
        });
        let third = rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("a third block decoded while two are held")
            .unwrap();
        assert_eq!(&third[..], &payload[8192..8292]);
        drop(held);
    }

    /// Two read units, one permission, and it reaches one of them: the chunk
    /// pool takes what the loop granted and the block pool stays at
    /// `NeverWait`, its holder being the retention list rather than the loop.
    #[tokio::test]
    async fn a_compressed_source_states_the_policy_to_its_chunk_pool_only() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let source = XzSource::open(compressed.path()).unwrap();
        let policies = |source: &XzSource| {
            (source.pool.policy(), source.blocks.as_ref().unwrap().pool.policy())
        };
        assert_eq!(policies(&source), (WaitPolicy::NeverWait, WaitPolicy::NeverWait));
        source.hint_wait_policy(WaitPolicy::MayWait);
        assert_eq!(policies(&source), (WaitPolicy::MayWait, WaitPolicy::NeverWait));
        source.hint_wait_policy(WaitPolicy::NeverWait);
        assert_eq!(policies(&source), (WaitPolicy::NeverWait, WaitPolicy::NeverWait));

        // And the block path still reads correctly while a wait is permitted.
        source.hint_wait_policy(WaitPolicy::MayWait);
        source.hint_read_size(1024);
        let got = source.read_range(0, 4096).await.unwrap();
        assert_eq!(&got[..], &payload[..4096]);
    }

    /// A budget too small for a whole block sends every read through the
    /// streaming reader, and the bytes are the same either way — the budget
    /// changes the path, never the answer.
    #[tokio::test]
    async fn a_budget_below_the_block_unit_streams_and_still_reads_the_same_bytes() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let source = XzSource::open(compressed.path()).unwrap();
        assert!(source.block_path().is_some(), "4 KiB blocks fit the default budget");

        // Under a budget of one chunk buffer there is nothing left for a
        // block, so the source falls back — and says so through its own
        // partitioning advice as well.
        source.hint_read_size(1 << 20);
        source.hint_parallelism(Parallelism::workers(4, 1 << 20));
        assert!(source.block_path().is_none());
        assert_eq!(source.partitions(0..payload.len() as u64).max_partitions(), Some(1));

        let got = source.read_range(4000, 5000).await.unwrap();
        assert_eq!(&got[..], &payload[4000..9000]);

        source.hint_parallelism(Parallelism::default());
        assert!(source.block_path().is_some(), "the default budget takes it back");
        let again = source.read_range(4000, 5000).await.unwrap();
        assert_eq!(&again[..], &payload[4000..9000]);
    }

    /// The non-seekable shape (D2): one stream, one block, produced by a bare
    /// `xz` invocation with no `-T`/`--block-size`. Every read still decodes
    /// to the right bytes, backward ones included; a file this small has its
    /// one block decoded whole and retained, so the backward read below is a
    /// slice rather than a second decode from zero.
    #[tokio::test]
    async fn xz_source_reads_a_single_block_non_seekable_file() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &[]);
        let source = XzSource::open(compressed.path()).unwrap();

        let table = source.seek_table().unwrap();
        assert_eq!(table.block_count(), 1);
        assert!(!table.is_seekable());

        let forward = source.read_range(10_000, 5000).await.unwrap();
        assert_eq!(&forward[..], &payload[10_000..15_000]);

        // Backward, forcing a decode from byte zero of the sole block.
        let backward = source.read_range(0, 3000).await.unwrap();
        assert_eq!(&backward[..], &payload[0..3000]);
    }

    /// Recognition with nothing claimed, unwrapped — every test below that is
    /// not about a mismatch wants the source and nothing else, and
    /// `KnownCompression::Unknown` claims nothing recognition could
    /// contradict.
    fn recognize(path: &Path) -> Arc<dyn ByteRangeSource> {
        match open_local(path, KnownCompression::Unknown).unwrap() {
            Recognized::Source(source) => source,
            Recognized::Mismatch => panic!("`Unknown` claims nothing to contradict"),
        }
    }

    /// D8: a genuinely `.xz`-compressed file is recognised whatever it is
    /// named — the temp file `xz_compress` returns carries no `.xz` suffix at
    /// all, and `open_local` still hands back a source whose `seek_table()`
    /// answers `Some`, which only `XzSource` ever does.
    #[tokio::test]
    async fn open_local_recognizes_xz_content_with_no_xz_name() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        assert_ne!(compressed.path().extension(), Some(std::ffi::OsStr::new("xz")));

        let source = recognize(compressed.path());
        assert!(source.seek_table().is_some(), "content-sniffed as .xz");
        assert_eq!(source.size().await.unwrap(), payload.len() as u64);
        let got = source.read_range(0, payload.len()).await.unwrap();
        assert_eq!(&got[..], &payload[..]);
    }

    /// D8's other direction: a file *named* `.xz` whose bytes are not — the
    /// rejected extension-based dispatch would have handed this to
    /// `XzSource` and failed on `xz_seek::Error::NotXz`. Content sniffing
    /// opens it plain instead, correctly.
    #[tokio::test]
    async fn open_local_opens_a_dot_xz_named_file_with_plain_content_as_plain() {
        let mut file = tempfile::Builder::new().suffix(".xz").tempfile().unwrap();
        file.write_all(b"not actually compressed").unwrap();
        file.flush().unwrap();

        let source = recognize(file.path());
        assert!(source.seek_table().is_none(), "no compression layer — read as plain");
        let got = source.read_range(0, 23).await.unwrap();
        assert_eq!(&got[..], b"not actually compressed");
    }

    /// A file too short to hold the six-byte magic must not be mistaken for
    /// `.xz`, and must not error just for being short.
    #[tokio::test]
    async fn open_local_treats_a_file_shorter_than_the_magic_as_plain() {
        let (_file, plain) = source_of(b"ab");
        let path = plain.path().to_path_buf();
        let source = recognize(&path);
        assert!(source.seek_table().is_none());
        assert_eq!(source.size().await.unwrap(), 2);
    }

    /// A table a previous walk produced is handed back and used: the source
    /// reads correctly and answers the same table it was given.
    #[tokio::test]
    async fn open_local_builds_an_xz_source_from_a_handed_back_table() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let table = XzSource::open(compressed.path()).unwrap().seek_table().unwrap();

        let source =
            match open_local(compressed.path(), KnownCompression::Xz(table.clone())).unwrap() {
                Recognized::Source(source) => source,
                Recognized::Mismatch => panic!("the file's own table must describe it"),
            };
        assert_eq!(source.seek_table().as_ref(), Some(&table));
        assert_eq!(source.size().await.unwrap(), payload.len() as u64);
        let got = source.read_range(9_000, 4000).await.unwrap();
        assert_eq!(&got[..], &payload[9_000..13_000]);
    }

    /// **The walk is actually skipped**, tested behaviourally rather than by
    /// instrumentation (`docs/design/architecture.md`, "The compressed
    /// source"): the table handed in names a check algorithm this file's
    /// streams do not use, which `validate` does not police and a walk of
    /// this file would never produce. The check's size moves the payload's
    /// end, so a decode from the handed table fails — where a source that had
    /// silently re-walked would read the payload back happily.
    #[tokio::test]
    async fn a_handed_back_table_is_used_rather_than_re_walked() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let mut table = XzSource::open(compressed.path()).unwrap().seek_table().unwrap();
        let real = table.streams[0].check;
        let wrong = if real == xz_seek::Check::Crc32 {
            xz_seek::Check::Crc64
        } else {
            xz_seek::Check::Crc32
        };
        for stream in &mut table.streams {
            stream.check = wrong;
        }

        let source = match open_local(compressed.path(), KnownCompression::Xz(table)).unwrap() {
            Recognized::Source(source) => source,
            Recognized::Mismatch => panic!("`validate` does not police the check algorithm"),
        };
        assert!(
            source.read_range(0, 4000).await.is_err(),
            "a decode under the wrong check must fail — a silent re-walk would have succeeded"
        );
    }

    /// A table that does not describe this file is refused, and the refusal
    /// is an outcome rather than an error: nothing was read and the caller
    /// still chooses whether to pay the walk.
    #[tokio::test]
    async fn open_local_reports_a_table_that_does_not_describe_the_file() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        // A second, shorter file's table: `validate` compares the table's
        // recorded compressed length against this source's.
        let other = xz_compress(&payload[..5_000], &["--block-size=4096"]);
        let other_table = XzSource::open(other.path()).unwrap().seek_table().unwrap();

        assert!(matches!(
            open_local(compressed.path(), KnownCompression::Xz(other_table)).unwrap(),
            Recognized::Mismatch
        ));
    }

    /// Both directions of "the cache describes a different file": a
    /// compression index for a file whose bytes are plain, and a plain cache
    /// for a file whose bytes are `.xz`.
    #[tokio::test]
    async fn open_local_reports_a_compression_claim_the_file_contradicts() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let table = XzSource::open(compressed.path()).unwrap().seek_table().unwrap();
        let (plain_file, _plain) = source_of(&payload);

        assert!(matches!(
            open_local(plain_file.path(), KnownCompression::Xz(table)).unwrap(),
            Recognized::Mismatch
        ));
        assert!(matches!(
            open_local(compressed.path(), KnownCompression::Plain).unwrap(),
            Recognized::Mismatch
        ));
    }

    /// A plain claim about a plain file is simply right, and costs nothing.
    #[tokio::test]
    async fn open_local_accepts_a_plain_claim_about_a_plain_file() {
        let (file, _plain) = source_of(b"0123456789abcdef");
        let source = match open_local(file.path(), KnownCompression::Plain).unwrap() {
            Recognized::Source(source) => source,
            Recognized::Mismatch => panic!("a plain file is what the claim said"),
        };
        assert!(source.seek_table().is_none());
        assert_eq!(source.size().await.unwrap(), 16);
    }

    /// A filesystem root shaped like a Linux one, for driving discovery
    /// against something other than the machine the test is running on.
    ///
    /// **Everything the reader consults is under the root** — `proc/self/cgroup`,
    /// `proc/meminfo`, and the cgroup mount — which is what makes the v1 arm
    /// executable at all: the memory controller lives in exactly one hierarchy
    /// (`RT6`), so no v1 shape can exist beside this machine's v2 one.
    struct FakeRoot(tempfile::TempDir);

    impl FakeRoot {
        fn new() -> FakeRoot {
            FakeRoot(tempfile::tempdir().unwrap())
        }

        fn path(&self) -> &Path {
            self.0.path()
        }

        /// Write `body` at `rel`, creating the directories above it.
        fn write(&self, rel: &str, body: &str) -> &FakeRoot {
            let at = self.0.path().join(rel);
            std::fs::create_dir_all(at.parent().unwrap()).unwrap();
            std::fs::write(&at, body).unwrap();
            self
        }

        /// The `0::` line for a v2 process at `cgroup`.
        fn v2(&self, cgroup: &str) -> &FakeRoot {
            self.write("proc/self/cgroup", &format!("0::{cgroup}\n"))
        }

        /// A v2 cgroup directory's two limit files, each `None` for absent.
        fn v2_limits(&self, cgroup: &str, max: Option<&str>, high: Option<&str>) -> &FakeRoot {
            let dir = format!("sys/fs/cgroup{cgroup}");
            let dir = dir.trim_end_matches('/');
            if let Some(max) = max {
                self.write(&format!("{dir}/memory.max"), &format!("{max}\n"));
            }
            if let Some(high) = high {
                self.write(&format!("{dir}/memory.high"), &format!("{high}\n"));
            }
            self
        }
    }

    /// **The `0::` line is selected by its shape, and a dead cgroup's path is
    /// not a directory name** (`RT1`). The hierarchy id is whatever
    /// `idr_alloc_cyclic` handed out and the line order is unspecified, so
    /// neither may be read; the ` (deleted)` suffix would otherwise be joined
    /// onto the mount point.
    #[test]
    fn the_unified_line_is_found_by_shape_and_stripped_of_a_deleted_suffix() {
        let root = FakeRoot::new();
        root.write("proc/self/cgroup", "7:name=probe:/somewhere\n0::/leaf (deleted)\n").v2_limits(
            "/leaf",
            Some("536870912"),
            None,
        );
        assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(536870912));
    }

    /// **The controller field is a comma-separated list matched by
    /// membership** (`RT6`): a v1 hierarchy carrying only a *name* reads
    /// `name=memory` and holds no memory controller at all, so a substring
    /// test would send the reader to a hierarchy with no limit files in it.
    #[test]
    fn a_hierarchy_merely_named_memory_is_not_the_memory_controller() {
        let root = FakeRoot::new();
        root.write("proc/self/cgroup", "3:name=memory:/decoy\n0::/leaf\n").v2_limits(
            "/leaf",
            Some("268435456"),
            None,
        );
        assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(268435456));
    }

    /// **`max` is a sentinel for one level, not a statement that the process is
    /// unlimited** (`RT2`), and an absent file is "no limit here" rather than an
    /// error (`RT5`) — the hierarchy root carries neither file at all. So the
    /// walk climbs past both and finds the ancestor that binds.
    #[test]
    fn an_ancestors_limit_binds_where_the_leaf_states_max() {
        let root = FakeRoot::new();
        root.v2("/pgdq-probe/child")
            .v2_limits("/pgdq-probe/child", Some("max"), Some("max"))
            .v2_limits("/pgdq-probe", Some("268435456"), None);
        assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(268435456));
    }

    /// **`memory.high` and `memory.max` are minimised together, across levels
    /// and across the two files** (`RT3`, `RT5`): nothing orders them, and the
    /// throttle two levels up may be the smallest number in the walk. It is
    /// read at all because sustained reclaim ends a scan's throughput as surely
    /// as an OOM ends the run.
    #[test]
    fn the_smallest_of_every_limit_at_every_level_is_what_binds() {
        let root = FakeRoot::new();
        root.v2("/a/b/c")
            .v2_limits("/a/b/c", Some("2147483648"), None)
            .v2_limits("/a/b", None, Some("134217728"))
            .v2_limits("/a", Some("1073741824"), None);
        assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(134217728));
    }

    /// **The file that stated the smallest limit comes back with it**, because
    /// `memory.high` throttles where `memory.max` kills (`RT3`) and either may
    /// be an ancestor's (`RT5`) — so a status line saying a budget was cut is
    /// only actionable beside the file that cut it
    /// (`docs/design/architecture.md`, "Status output").
    #[test]
    fn a_discovered_limit_names_the_file_that_stated_it() {
        let root = FakeRoot::new();
        root.v2("/a/b").v2_limits("/a/b", Some("2147483648"), None).v2_limits(
            "/a",
            None,
            Some("134217728"),
        );
        let limit = discover_memory_limit_in(root.path()).expect("a limit binds here");
        assert_eq!(limit.bytes, 134217728);
        assert_eq!(limit.read_from, root.path().join("sys/fs/cgroup/a/memory.high"));
    }

    /// **Nothing anywhere is `None`, and that is a complete statement**: no
    /// limit found means no limit is being enforced, however the process was
    /// started. A tree with no cgroup file at all reads the same way, which is
    /// the non-Linux case.
    #[test]
    fn an_unlimited_hierarchy_and_a_missing_one_both_read_as_no_limit() {
        let root = FakeRoot::new();
        root.v2("/leaf").v2_limits("/leaf", Some("max"), Some("max"));
        assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), None);
        assert_eq!(discover_memory_limit_in(FakeRoot::new().path()).map(|l| l.bytes), None);
    }

    /// **A v1 hierarchy is read from its own mount, located through
    /// `mountinfo`** (`RT4`, `RT6`), and "unlimited" there is a *threshold*
    /// near `LONG_MAX` rather than a sentinel string — the value being a
    /// function of the page size and the word width, so an equality test would
    /// be right on one kernel configuration and wrong on three.
    ///
    /// This establishes that the reader handles the shape `RT4` describes. It
    /// observes no kernel: this machine runs a pure v2 hierarchy and cannot
    /// produce a v1 memory controller at all.
    #[test]
    fn the_v1_arm_reads_its_own_mount_and_treats_a_huge_value_as_no_limit() {
        let root = FakeRoot::new();
        root.write("proc/self/cgroup", "5:memory:/svc\n0::/leaf\n")
            .write(
                "proc/self/mountinfo",
                "31 24 0:27 / /sys/fs/cgroup/memory rw,nosuid - cgroup cgroup rw,memory\n",
            )
            .write("sys/fs/cgroup/memory/svc/memory.limit_in_bytes", "536870912\n")
            // The v2 files are there and must not be consulted: a controller
            // lives in exactly one hierarchy, and this process's memory
            // controller is the v1 one.
            .v2_limits("/leaf", Some("104857600"), None);
        assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(536870912));

        let unset = FakeRoot::new();
        unset
            .write("proc/self/cgroup", "5:memory:/\n")
            .write(
                "proc/self/mountinfo",
                "31 24 0:27 / /sys/fs/cgroup/memory rw - cgroup cgroup rw,memory\n",
            )
            // 32-bit `PAGE_COUNTER_MAX × PAGE_SIZE`, the smallest of the four
            // shapes "unlimited" takes and therefore the one a threshold has
            // to clear.
            .write("sys/fs/cgroup/memory/memory.limit_in_bytes", "8796093018112\n");
        assert_eq!(discover_memory_limit_in(unset.path()).map(|l| l.bytes), None);
    }

    /// **The conventional mount is the fallback, not the authority** — a tree
    /// that lists no cgroup mount still reads `/sys/fs/cgroup/memory`, which is
    /// where `file-hierarchy(7)` puts it.
    #[test]
    fn the_v1_arm_falls_back_to_the_conventional_mount() {
        let root = FakeRoot::new();
        root.write("proc/self/cgroup", "5:cpu,memory,cpuacct:/svc\n")
            .write("sys/fs/cgroup/memory/svc/memory.limit_in_bytes", "268435456\n");
        assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(268435456));
    }

    /// **`MemAvailable`, in kB, and nothing else** (`RT8`). `MemFree` is on the
    /// same file and is not read: it excludes reclaimable page cache, so
    /// planning against it would throttle a scan for memory the kernel would
    /// hand straight back.
    #[test]
    fn available_memory_reads_memavailable_and_converts_from_kb() {
        let root = FakeRoot::new();
        root.write(
            "proc/meminfo",
            "MemTotal:       32774304 kB\nMemFree:         1489828 kB\n\
             MemAvailable:   20002184 kB\n",
        );
        assert_eq!(available_memory_in(root.path()), Some(20002184 * 1024));
        assert_eq!(available_memory_in(FakeRoot::new().path()), None);
    }

    /// **A discovered limit caps what a source asked for; it does not become
    /// it.** The reserve comes off the top and the source's recommendation is
    /// taken no higher — "do not take what you cannot use" — while a
    /// recommendation that already fits is left alone.
    ///
    /// The count is held to [`MEMORY_MARGIN_PERCENT`] besides, which is the
    /// tighter of the two conditions at every limit; the budget is still what
    /// the resolved count spends.
    #[test]
    fn a_discovered_limit_caps_the_recommendation_at_the_limit_less_the_reserve() {
        let root = FakeRoot::new();
        root.v2("/leaf").v2_limits("/leaf", Some("1073741824"), None);
        let cap = (1024 << 20) - MEMORY_RESERVE;

        // Eight readers of 512 MiB each is 4 GiB against a cap of 640 MiB, so
        // the count comes down with the budget rather than being printed
        // beside one it cannot spend: one reader is what fits. The budget is
        // still `cap`-bounded and not margin-bounded, so the floor arrangement
        // reports what one reader holds.
        let squeezed =
            Parallelism::discover_in(root.path(), 8, Some(WorkerMemory::per_worker(512 << 20)));
        assert_eq!(squeezed.memory_bytes(), Some(512 << 20));
        assert_eq!(squeezed.jobs(), 1);

        // Eight readers of 16 MiB is 128 MiB, inside the 563.2 MiB the margin
        // allows as well as inside the cap, so both numbers stand.
        let roomy =
            Parallelism::discover_in(root.path(), 8, Some(WorkerMemory::per_worker(16 << 20)));
        assert_eq!(roomy.memory_bytes(), Some(128 << 20));
        assert_eq!(roomy.jobs(), 8);

        // And in between, the count is what the margin affords and the budget
        // is exactly what that many readers spend — never the cap itself,
        // which is the over-ask this pairing exists to remove. Five readers of
        // 100 MiB is 500, against a margin allowance of 563.2 and a cap of
        // 640: six would fit the cap and is refused.
        let fitted =
            Parallelism::discover_in(root.path(), 8, Some(WorkerMemory::per_worker(100 << 20)));
        assert_eq!(fitted.jobs(), 5);
        assert_eq!(fitted.memory_bytes(), Some(500 << 20));
        assert!(fitted.memory_bytes() < Some(cap), "the cap itself would be the over-ask");
    }

    /// **The resolved count answers to the criterion rather than to the host's
    /// width.** Before this the margin at a large limit was bought by
    /// `available_parallelism` clamping the count below what the allowance
    /// afforded, so the same allocation on a wider host resolved more readers
    /// and left less headroom — the one thing `MEMORY_RESERVE` was never shown
    /// to do. A recommendation above what the margin allows now resolves the
    /// same arrangement whatever the recommendation is.
    #[test]
    fn the_count_answers_to_the_margin_and_not_to_the_recommendation() {
        let root = FakeRoot::new();
        root.v2("/leaf").v2_limits("/leaf", Some(&(2u64 << 30).to_string()), None);
        // What one reader of an ordinary 24 MiB-block `.xz` holds, and the
        // pool floor below four readers of it.
        let reader = 58 << 20;
        let memory = WorkerMemory::per_worker(reader).flooring(24 << 20, POOL_DEPTH);

        let narrow = Parallelism::discover_in(root.path(), 24, Some(memory));
        let wide = Parallelism::discover_in(root.path(), 64, Some(memory));
        assert_eq!(narrow.jobs(), wide.jobs(), "the host's core count is not the answer");
        assert_eq!(narrow.memory_bytes(), wide.memory_bytes());

        // And what it resolves to leaves the margin: the charge plus the
        // unpooled bound is under four fifths of the limit, where the cap
        // alone would have admitted twenty-eight readers and left 13%.
        let limit = 2u64 << 30;
        let charge = wide.memory_bytes().unwrap();
        assert!(
            charge + MEMORY_UNPOOLED_BOUND <= limit / 100 * (100 - MEMORY_MARGIN_PERCENT),
            "predicted resident {} breaches the margin",
            charge + MEMORY_UNPOOLED_BOUND
        );
        assert!(
            memory.at(wide.jobs() + 1) + MEMORY_UNPOOLED_BOUND
                > limit / 100 * (100 - MEMORY_MARGIN_PERCENT),
            "one more reader would still have fitted, so the margin is not what bound it"
        );
        assert!(
            memory.at(wide.jobs() + 1) <= limit - MEMORY_RESERVE,
            "the cap alone would have afforded more, which is what the margin is for"
        );
    }

    /// **A margin the smallest arrangement cannot meet costs nothing.** The
    /// floor is one worker at whatever the cap is, so a limit whose margin
    /// allowance is under one reader's charge still resolves that reader and
    /// still reports the budget it spends — which is what
    /// `BlockCache::affordable` reads, so the compressed block path is not
    /// declined by the margin at any limit.
    ///
    /// The arrangement is a 128 MiB-block file at a 1088 MiB limit, which is
    /// where that window sits now that the prediction uses
    /// [`MEMORY_UNPOOLED_BOUND`]: the allowance must fall under one reader's
    /// charge while the cap stays above it, and at 24 MiB blocks no limit does
    /// both. It is also the acceptance leg the `reserve` figure registers for
    /// the pool floor, so the cell this pins is one a sitting reads.
    #[test]
    fn the_margin_never_takes_the_last_reader_or_the_budget_it_spends() {
        let root = FakeRoot::new();
        // 1088 MiB: the cap is 704 MiB and the margin allowance 614.4, against
        // the 650 MiB one block-decoding reader of a 128 MiB-block file is
        // charged.
        root.v2("/leaf").v2_limits("/leaf", Some(&(1088u64 << 20).to_string()), None);
        let memory = WorkerMemory::per_worker(266 << 20).flooring(128 << 20, POOL_DEPTH);
        let tight = Parallelism::discover_in(root.path(), 24, Some(memory));
        assert_eq!(tight.jobs(), 1);
        assert_eq!(tight.memory_bytes(), Some(memory.at(1)));
        assert!(memory.at(1) > margin_allowance(1088 << 20), "the margin cannot afford it");
    }

    /// **The margin binds at a large limit and is inert at a small one**, which
    /// is the shape it exists for: a constant reserve leaves a shrinking
    /// *share* as the limit grows, so the fraction is needed at the top end and
    /// the subtraction already covers the bottom. The crossover is arithmetic
    /// rather than a reading — `limit − MEMORY_RESERVE` and
    /// `(100 − margin)% × limit − MEMORY_UNPOOLED_BOUND` are equal at
    /// `5 × (MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND)` — and it is pinned here
    /// because nothing else would notice either constant moving past the other.
    #[test]
    fn the_margin_is_the_tighter_condition_only_above_the_crossover() {
        let crossover = 5 * (MEMORY_RESERVE - MEMORY_UNPOOLED_BOUND);
        assert_eq!(crossover, 640 << 20);
        // Equal there but for the rounding: `limit / 100` discards under a
        // hundred bytes, so the allowance errs small by that much and never
        // the other way.
        assert!(crossover - MEMORY_RESERVE - margin_allowance(crossover) < 100);
        // Below it the cap is what a count is solved against, so the margin
        // can lower nothing; above it the margin is, at every limit.
        assert!(
            margin_allowance(crossover - (64 << 20)) > (crossover - (64 << 20)) - MEMORY_RESERVE
        );
        assert!(
            margin_allowance(crossover + (64 << 20)) < (crossover + (64 << 20)) - MEMORY_RESERVE
        );
        assert!(margin_allowance(4 << 30) < (4 << 30) - MEMORY_RESERVE);
    }

    /// **Below the reserve the budget goes to zero rather than to a floor.**
    /// Reasserting [`DEFAULT_MEMORY_BUDGET`] here would hand a cgroup too small
    /// to clear the reserve exactly what an unlimited host gets, in the one case
    /// discovery was built for. What zero produces is one reader's worth on the streaming path,
    /// through the three floors already in the mechanism.
    #[test]
    fn a_limit_at_or_under_the_reserve_leaves_no_budget_at_all() {
        let root = FakeRoot::new();
        root.v2("/leaf").v2_limits("/leaf", Some(&MEMORY_RESERVE.to_string()), None);
        let starved =
            Parallelism::discover_in(root.path(), 4, Some(WorkerMemory::per_worker(1 << 30)));
        assert_eq!(starved.memory_bytes(), Some(0));
        // One worker at whatever the cap is, not one worker's worth of bytes:
        // the floor is on the count, and a budget the allowance never granted
        // is the one thing the fit must not hand back.
        assert_eq!(starved.jobs(), 1);

        let tighter = FakeRoot::new();
        tighter.v2("/leaf").v2_limits("/leaf", Some("134217728"), None);
        assert_eq!(Parallelism::discover_in(tighter.path(), 4, None).memory_bytes(), Some(0));
    }

    /// **A source with no recommendation is still capped by a discovered
    /// limit**, at [`DEFAULT_MEMORY_BUDGET`] where the limit leaves room — the
    /// plain path, which asks for nothing and is left where it was.
    #[test]
    fn a_source_that_recommends_nothing_is_capped_at_the_existing_default() {
        let root = FakeRoot::new();
        root.v2("/leaf").v2_limits("/leaf", Some("3221225472"), None);
        assert_eq!(
            Parallelism::discover_in(root.path(), 1, None).memory_bytes(),
            Some(DEFAULT_MEMORY_BUDGET)
        );
    }

    /// **The composition is not a `min`, and the difference is one `Option`.**
    /// With no limit found there is no cap from the environment at all, so a
    /// source's recommendation stands — where a `min` against the fallback
    /// constant would have handed a compressed scan 64 MiB, which affords one
    /// reader, and made the source's own worker count unreachable on the
    /// machine most likely to run it.
    #[test]
    fn no_limit_found_leaves_the_recommendation_uncapped_but_for_memavailable() {
        let root = FakeRoot::new();
        root.v2("/leaf")
            .v2_limits("/leaf", Some("max"), None)
            .write("proc/meminfo", "MemAvailable:   20002184 kB\n");
        // Twenty-four readers of an ordinary 24 MiB-block dump: ~1.4 GiB in
        // total, well under half of a 19 GiB `MemAvailable`.
        let per_worker = 58 << 20;
        let roomy =
            Parallelism::discover_in(root.path(), 24, Some(WorkerMemory::per_worker(per_worker)));
        assert_eq!(roomy.memory_bytes(), Some(per_worker * 24));
        assert_eq!(roomy.jobs(), 24);

        // Half of `MemAvailable`, because it is an estimate two processes
        // reading at once each see the whole of — and it binds only on a
        // machine too small to afford the recommended count, which is then
        // reduced to what half of it buys rather than left standing.
        let small = FakeRoot::new();
        small.v2("/leaf").write("proc/meminfo", "MemAvailable:     262144 kB\n");
        let squeezed =
            Parallelism::discover_in(small.path(), 24, Some(WorkerMemory::per_worker(per_worker)));
        assert_eq!(squeezed.jobs(), 2);
        assert_eq!(squeezed.memory_bytes(), Some(per_worker * 2));

        // `MemAvailable` unreadable is the one shape with a recommendation and
        // no ceiling over it: the whole of what the count asks for.
        let blind = FakeRoot::new();
        blind.v2("/leaf");
        assert_eq!(
            Parallelism::discover_in(blind.path(), 24, Some(WorkerMemory::per_worker(per_worker)))
                .memory_bytes(),
            Some(per_worker * 24)
        );
    }

    /// **A recommended count is lowered by a stated budget exactly as it is by
    /// a discovered one, and only the count moves.** Where
    /// [`Parallelism::discover_in`] hands back the bytes the lowered count
    /// spends — it chose them, and must not name a budget the allowance never
    /// granted — the budget here was stated by the caller and is taken whole.
    #[test]
    fn a_stated_budget_lowers_a_recommendation_without_being_lowered_itself() {
        let per_worker = 58 << 20;

        // Six readers fit inside 400 MiB; the seventh does not.
        let cut = Parallelism::recommended_within(
            24,
            Some(WorkerMemory::per_worker(per_worker)),
            400 << 20,
        );
        assert_eq!(cut.jobs(), 6);
        assert_eq!(cut.memory_bytes(), Some(400 << 20), "the stated bytes, not 6 × per_worker");

        // Room for the whole recommendation leaves it standing.
        let roomy = Parallelism::recommended_within(
            24,
            Some(WorkerMemory::per_worker(per_worker)),
            64 << 30,
        );
        assert_eq!(roomy.jobs(), 24);
        assert_eq!(roomy.memory_bytes(), Some(64 << 30));

        // The floor is one worker at whatever was stated, which is
        // `Parallelism::workers`' serial arrangement carrying the budget.
        let tight = Parallelism::recommended_within(
            24,
            Some(WorkerMemory::per_worker(per_worker)),
            32 << 20,
        );
        assert_eq!(tight, Parallelism::Serial { memory_bytes: Some(32 << 20) });

        // Nothing to divide by leaves the count where it is.
        assert_eq!(Parallelism::recommended_within(8, None, 32 << 20).jobs(), 8);
    }

    /// **An unlimited environment falls back to today's constant**, which is
    /// what keeps discovery strictly additive — and at a serial count it falls
    /// all the way back to [`Parallelism::default`], the state that says nobody
    /// asked for a budget at all. A `Workers` arrangement has nowhere to record
    /// that, so it carries the constant bare.
    #[test]
    fn nothing_discovered_and_nothing_recommended_is_todays_default() {
        let root = FakeRoot::new();
        root.v2("/leaf").write("proc/meminfo", "MemAvailable:   20002184 kB\n");
        assert_eq!(Parallelism::discover_in(root.path(), 1, None), Parallelism::default());
        assert_eq!(
            Parallelism::discover_in(root.path(), 8, None),
            Parallelism::workers(8, DEFAULT_MEMORY_BUDGET)
        );
    }

    /// **A source recommends what its workers hold**, so that "no limit found"
    /// cannot mean "serial": one reader of an ordinary compressed dump costs
    /// more than the 64 MiB fallback, and a count nothing can afford is not a
    /// recommendation. The plain source inherits the silence it inherits for
    /// the worker count.
    ///
    /// **A shape rather than a scalar**, because the block pool's floor is not
    /// per worker: `block_decode_bytes` is that same shape evaluated at one
    /// reader — the gate's own number — and is strictly above the per-worker
    /// term, while at [`POOL_DEPTH`] readers the floor is gone and the cost is
    /// the per-worker term times the count.
    #[test]
    fn a_compressed_source_recommends_what_its_workers_hold() {
        assert_eq!(BareSource.default_worker_memory(), None);
        let (_file, plain) = source_of(b"0123456789abcdef");
        assert_eq!(plain.default_worker_memory(), None);

        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let source = XzSource::open(compressed.path()).unwrap();
        let memory = source.default_worker_memory().expect("a block path to price");
        assert_eq!(source.block_decode_bytes(), Some(memory.at(1)));
        assert!(
            memory.at(1) > memory.bytes_per_worker(),
            "one reader leaves a pool floor nobody else fills"
        );
        assert_eq!(
            memory.at(POOL_DEPTH),
            memory.bytes_per_worker() * POOL_DEPTH as u64,
            "at the pool's own depth the floor is filled and the cost is linear"
        );
    }

    /// **The charge an allowance is solved against before the file is open is
    /// the charge the resulting count then meets.** `Parallelism::fit` solves
    /// a cap against `default_worker_memory` with nothing announced;
    /// `crate::stream::worker_count` solves the budget that produced against
    /// `partitions().worker_memory()` once the scan has announced its chunk.
    /// Those were two different numbers — the second consumer of a value
    /// changed for the first — and the gap was `POOL_MAX_BYTES -
    /// DEFAULT_CHUNK_SIZE` a reader, so a 512 MiB allocation resolved three
    /// readers where four fit.
    ///
    /// Asserted end to end rather than by comparing two accessors, because
    /// what has to agree is the *resolution* and the *scan*, and only the
    /// round trip through `Parallelism` can say so.
    #[test]
    fn the_recommendation_an_allowance_is_solved_against_is_the_scans_own_charge() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let source = XzSource::open(compressed.path()).unwrap();
        let memory = source.default_worker_memory().expect("a block path to price");

        // Four readers' worth of allowance, as `Parallelism::fit` would spend
        // it, then the scan's own announcement and its charge.
        let (jobs, budget) = Parallelism::fit(24, Some(memory), memory.at(4), None);
        assert_eq!(jobs, 4);
        let parallelism = Parallelism::workers(jobs, budget);
        source.hint_read_size(crate::DEFAULT_CHUNK_SIZE);
        source.hint_parallelism(parallelism);
        let advice = source.partitions(0..payload.len() as u64);
        assert_eq!(
            advice.worker_memory(),
            memory,
            "the recommendation and the charge are one shape"
        );
        assert_eq!(
            crate::stream::worker_count(parallelism, advice.worker_memory()),
            jobs,
            "the count the allowance bought is the count the scan runs"
        );
    }
}
