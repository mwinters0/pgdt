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

#[cfg(feature = "http")]
use std::sync::atomic::AtomicBool;
#[cfg(feature = "http")]
use std::time::{Duration, UNIX_EPOCH};

#[cfg(feature = "http")]
use object_store::ObjectStore as _;

use crate::scan::{Cancellation, SCAN_CHUNK_DEFAULT_SIZE_BYTES};
use crate::{Error, Result};

/// Minimal async byte-range read abstraction.
///
/// The three required methods mirror `object_store`'s `get_range`/`head`
/// semantics. [`ByteRangeSource::hint_read_size`] sits outside that mirror and
/// is defaulted, so an implementation need not know it exists.
///
/// **Dyn-compatible on purpose**: each method returns a boxed future rather
/// than `impl Future` (RPITIT), so callers can hold `&dyn ByteRangeSource` /
/// `Arc<dyn ByteRangeSource>` instead of being generic over the source. See
/// `docs/design/decisions.md`, "D6".
pub trait ByteRangeSource: Send + Sync {
    /// Exactly `len` bytes at `offset`, or an error — **never a short
    /// answer.** A range running past the end of the source is
    /// `std::io::ErrorKind::UnexpectedEof`, not the prefix that does exist,
    /// on every implementation here: `read_exact_at` on a file, a length
    /// check on a fetched body, and a fill-or-fault decode on the compressed
    /// ones.
    ///
    /// **A driver that is told what to fetch relies on that.**
    /// `xz_seek::Walk` derives each request from the bytes of the one before
    /// it and reads a short supply as the file being shorter than the size it
    /// was constructed with, so a source answering a prefix would make a
    /// truncated file and a truncated *read* the same event
    /// ([`walk_seek_table`]).
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>>;
    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>>;
    /// Last-modified time, if the source exposes one — `object_store`'s
    /// `head` carries this too. `None` rather than an error for a source that
    /// genuinely has no notion of one; the structure cache treats an absent
    /// mtime as nothing to compare against rather than as a mismatch, except
    /// under `crate::cache::StrictIdentity::time`, where a source that can
    /// offer no such signal is refused. See
    /// `docs/design/decisions.md`, "D21".
    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>>;
    /// Bytes as stored on the device — what a `stat` reports — as opposed to
    /// [`ByteRangeSource::size`]'s addressable (possibly decompressed) length.
    /// This is the structure cache's staleness check. See
    /// `docs/design/decisions.md`, "D21".
    ///
    /// Defaults to [`ByteRangeSource::size`], right for a source that does not
    /// decompress; a decompressing source overrides it to the compressed
    /// file's own length.
    fn stored_size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        self.size()
    }
    /// Whether [`ByteRangeSource::size`] is this source's *exact* addressable
    /// length rather than a bound. Every source implemented so far answers
    /// `true` honestly; nothing reads it yet. See
    /// `docs/design/decisions.md`, "D6".
    fn size_is_exact(&self) -> bool {
        true
    }
    /// The read length this caller is about to ask for over and over — a
    /// read loop's `ScanOptions::chunk_size`, announced once before the loop
    /// starts.
    ///
    /// **Advisory, and it defaults to doing nothing.** One-off-ness is a
    /// property of the *caller* and nothing in a `read_range` call carries it,
    /// so a source that recycles buffers cannot otherwise tell a repeating
    /// chunk read from `map::attach_text`'s one coalesced span read. See
    /// `docs/design/decisions.md`, "D9".
    fn hint_read_size(&self, _len: usize) {}
    /// The xz seek table behind this source, for a caller building a cache
    /// envelope to persist alongside it (`crate::cache::CompressionIndex`
    /// wraps this at save time). See `docs/design/decisions.md`, "D18".
    ///
    /// `None` by default, right for a source with no compression layer;
    /// [`XzSource`] returns the table it built while walking the file's stream
    /// footers at open time. A cloned value rather than a borrow: a caller
    /// building a `CacheFile` needs to own it across an `await`.
    fn seek_table(&self) -> Option<xz_seek::SeekTable> {
        None
    }
    /// How this source would like `range` split across concurrent readers,
    /// and what one of those readers costs it resident.
    ///
    /// **Advisory, and the caller never learns what is underneath**: a local
    /// file answers "anywhere, one buffer each" and a compressed one "at these
    /// block boundaries, a block each".
    ///
    /// **The default declines to advise**: one partition, at no stated cost.
    /// It answers where and at what cost, never whether — the caller decides
    /// on its own whether to make a cut (`docs/design/decisions.md`, "D7").
    fn partitions(&self, _range: Range<u64>) -> Partitioning {
        Partitioning::single(0)
    }

    /// What reading this source's container a block at a time would cost at
    /// **one** reader, in bytes — the number a memory budget has to clear for
    /// that path to be taken at all, whether or not it was.
    ///
    /// One reader's charge *plus the retention list it leaves behind*, which
    /// is the line [`BlockCache::affordable`] draws: a pool serving one reader
    /// still holds [`POOL_DEPTH`] slots. `None` for a source with no such path
    /// to take, which is every source but a compressed one.
    ///
    /// Whether the path was declined is read off
    /// [`ByteRangeSource::partitions`]; this is only the number the message
    /// needs. See `docs/design/decisions.md`, "D16".
    fn block_decode_bytes(&self) -> Option<u64> {
        None
    }

    /// How many concurrent readers this source recommends to a caller that has
    /// stated no count of its own — a **recommendation**, never a bound.
    ///
    /// **The default is one, which is the serial path**, the same convention
    /// [`ByteRangeSource::partitions`] follows. Nothing in the library reads
    /// this — a caller that states a count gets that count — and it exists for
    /// the layer above, which has a person's flags to fill in
    /// (`pgdump_query-cli`'s `Discovered::resolve`).
    ///
    /// **It is a raw count, not a budgeted one**: what the byte budget affords
    /// is `crate::stream::worker_count`'s question, asked of every count
    /// alike. [`LocalFileSource`] inherits the default; [`XzSource`] overrides
    /// it. See `docs/design/decisions.md`, "D1" and "D2".
    fn default_workers(&self) -> usize {
        1
    }

    /// What the workers of this source hold, where the caller has stated no
    /// budget of its own — a **recommendation**, never a bound, and the budget
    /// sibling of [`ByteRangeSource::default_workers`].
    ///
    /// **It is per worker rather than a total**: the only count a source knows
    /// is its own, so multiplying is the caller's
    /// ([`Parallelism::discover_for`], where the two meet).
    ///
    /// **The default is `None`: no recommendation at all**, which leaves a
    /// caller on [`DEFAULT_MEMORY_BUDGET`] where nothing was discovered and on
    /// the discovered allowance where something was. It is what keeps "no
    /// limit found" from meaning "serial", that constant affording no
    /// block-decoding reader (`docs/design/decisions.md`, "D3").
    ///
    /// **Nothing in the library reads it**, exactly as with the worker count.
    /// It is a [`WorkerMemory`] rather than a scalar because one source's cost
    /// is not linear in the count (`docs/design/decisions.md`, "D4").
    fn default_worker_memory(&self) -> Option<WorkerMemory> {
        None
    }
    /// How much concurrency this caller allows, and how many bytes the source
    /// may hold while serving it — announced once before a read loop starts,
    /// beside [`ByteRangeSource::hint_read_size`].
    ///
    /// **Advisory, and it defaults to doing nothing.** What a source makes of
    /// the numbers is its own business — [`LocalFileSource`] sizes its one
    /// free list, [`XzSource`] divides the budget between its two read units
    /// and decides from it whether it can afford to decode a whole block at
    /// all. A [`Parallelism`] that states no byte count leaves a source on
    /// [`DEFAULT_MEMORY_BUDGET`].
    fn hint_parallelism(&self, _parallelism: Parallelism) {}
    /// How long the read loop about to start will hold the bytes it gets back
    /// — announced once, beside the other two hints, and only for a holder
    /// that drops each read before it takes the next
    /// (`docs/design/decisions.md`, "D5").
    ///
    /// **A source applies it only where the loop is the holder**: a wait on a
    /// pool the source retains from blocks on a slot the loop could never
    /// free, so [`XzSource`]'s block pool is left at
    /// [`WaitPolicy::NeverWait`] however a loop announces itself.
    ///
    /// **Advisory, and it defaults to doing nothing** — which is
    /// [`WaitPolicy::NeverWait`]'s behaviour.
    fn hint_wait_policy(&self, _policy: WaitPolicy) {}
    /// The caller's ask that reading stop, announced beside the other hints
    /// for a source whose own wait needs it.
    ///
    /// **Advisory, and it defaults to doing nothing**, which is right for
    /// every source whose wait is a `pread`: the read loop's own poll of
    /// `crate::ScanOptions::cancelled` bounds a stop by tens of milliseconds
    /// and nothing in the source has to know. A source whose wait is a network
    /// request has no such bound and takes the signal so it can drop the
    /// request in flight (`docs/design/decisions.md`, "D26").
    fn hint_cancellation(&self, _cancel: Arc<Cancellation>) {}
    /// Where this source was fetched from and which version of the object it
    /// is reading — the half of a cache's identity a source reached over a
    /// network has and a file does not
    /// (`docs/design/roadmap-P14-remote-input.md`, "D18").
    ///
    /// **`None` is a statement, not a gap**: a local cache records no origin,
    /// because its default path sits *beside* the dump, so the pairing is the
    /// filesystem's rather than a name we derived, and
    /// `crate::cache::StrictIdentity::location` binds nothing there
    /// (`docs/design/roadmap-P14-remote-input.md`, "D19").
    fn remote_identity(&self) -> Option<RemoteIdentity> {
        None
    }
    /// Whether a source that changes under an in-flight read must fail this
    /// run — announced by `crate::cache::SourceWatch` beside the other hints,
    /// for a source that enforces it at a finer cadence than the watch's own.
    ///
    /// **Advisory, and it defaults to doing nothing**: a local file is
    /// re-checked by the watch itself, which holds the same answer. A source
    /// whose server can do the comparing pins the object on every request
    /// instead, and this is the only thing that turns that off
    /// (`docs/design/roadmap-P14-remote-input.md`, "D11").
    ///
    /// **It is announced late, so the default must be the binding one.** A
    /// read taken before the watch opens — an origin probe, a cache claim —
    /// has never heard it.
    fn hint_in_flight_identity(&self, _binds: bool) {}
}

/// Where a source was fetched from and which version of it is being read:
/// [`ByteRangeSource::remote_identity`]'s answer, and the two fields a
/// remote cache records that a local one has no equivalent of
/// (`docs/design/roadmap-P14-remote-input.md`, "D18").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteIdentity {
    origin: String,
    etag: Option<String>,
}

impl RemoteIdentity {
    /// Build one from where the object is and what the server called this
    /// version of it.
    pub fn new(origin: impl Into<String>, etag: Option<String>) -> Self {
        Self { origin: origin.into(), etag }
    }

    /// Where the object was fetched from, as a message names it — the URL,
    /// with no credential in it, [`Origin`]'s own display form.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// The server's entity tag for this version of the object, `None` where
    /// it sent none. Opaque: it is compared, never parsed.
    pub fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }
}

/// Whether a read loop's acquisitions may be made to **wait** for a pooled
/// slot. See `docs/design/decisions.md`, "D5".
///
/// **It is a permission, not a description of the holder**: the pool needs to
/// know whether it may block this loop, and a loop grants that or withholds
/// it.
///
/// **The distinction is a deadlock, not a preference.** A loop that retains
/// into a batch holds every buffer it has taken a view into until that batch
/// flushes, so if it waited the only task that could free the slot would be
/// the one waiting for it. No pair of option values stands in for this: how
/// many slots are outstanding is a property of consumer code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WaitPolicy {
    /// This loop is never to be blocked: a read allocates past the pool's
    /// budget rather than waiting for a slot.
    ///
    /// **The default**, and safe in the direction that matters: a policy that
    /// fails to bind costs memory (`peak-rss`), where a wait that should not
    /// have been permitted is a hang with nothing to measure.
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
    /// else ([`ByteRangeSource::hint_wait_policy`]).
    MayWait,
}

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
/// (`docs/design/decisions.md`, "D47").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RetainedUnit {
    /// The read chunk. A batch spanning `crate::batch::QueryOptions`'
    /// `max_source_span` bytes pins the chunk buffers those bytes were read
    /// into, and those are bytes [`Partitioning::partition_bytes`] has not
    /// charged for — so a caller adds the span. A plain file's answer.
    ///
    /// **The default, for the same reason [`WaitPolicy::NeverWait`] is one**:
    /// over-charging plans fewer readers than the budget could hold, where
    /// under-charging plans readers it cannot.
    #[default]
    ReadChunk,
    /// The partition itself. A batch confined to a partition pins that
    /// partition whatever the span cap says
    /// (`docs/design/decisions.md`, "D47"), and
    /// [`Partitioning::partition_bytes`] has already charged for it — so
    /// adding the span would count the same bytes twice. A block-decoding
    /// source's answer.
    ///
    /// **That rationale holds only where a partition is one unit wide, which
    /// the leader's window guarantees and a query's cut does not** — `KD23`.
    /// `crate::leader::scan_region` sizes its window with
    /// [`Partitioning::window_end`] and is held to
    /// [`BOUNDARIED_PARTITION_UNITS`]; `crate::stream::plan_partitions` cuts a
    /// whole `CopyBlock` into `min(workers, max_partitions)` pieces, so a
    /// piece may span more units than `partition_bytes` charged for.
    Partition,
}

/// How a worker reads the piece it was handed
/// (`crate::leader::scan_region`) — **stated by the source, because the
/// right answer differs by source**.
///
/// Separate from [`RetainedUnit`] and [`Partitioning::partition_bytes`], which
/// say what a reader *holds*: this says what shape its reads are, and the cut
/// size must not follow the memory charge ([`Partitioning::window_end`], and
/// `docs/design/decisions.md`, "D8").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PartitionRead {
    /// `ScanOptions::chunk_size` at a time, repeating until the piece is
    /// consumed. A plain file's answer, and **the default, for the reason
    /// [`RetainedUnit::ReadChunk`] is one**: a chunk-sized read is the
    /// announced length, so [`BufferPool::keeps`] pools every buffer a worker
    /// takes, where a partition-length read would allocate a buffer `keeps`
    /// refuses to pool.
    #[default]
    Chunked,
    /// One of the source's own `unit`s in a single call — the whole piece
    /// where the piece is one unit, which is what
    /// [`BOUNDARIED_PARTITION_UNITS`] makes it. A block-decoding source's
    /// answer, where that read is a **zero-copy slice** of a block the worker
    /// was going to decode anyway, against one [`BlockCache::lookup`] per
    /// chunk on the chunked arm.
    ///
    /// **The unit is what bounds the buffer, and it is the source's number
    /// rather than the cut's.** `crate::leader::scan_region` reads
    /// `min(piece, unit)`, so at the shipped width — where a piece *is* one
    /// unit — this is the piece exactly, and above it the buffer is capped at
    /// one unit instead of growing with the width. It does not remove the
    /// copy: `xz_seek::SeekTable::max_block_uncompressed` is a file-wide
    /// maximum, so a read starting on a boundary can still cross into a
    /// smaller successor block (`docs/design/decisions.md`, "D8").
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
/// width**. See `docs/design/decisions.md`, "D8".
///
/// What a wider cut would buy is the tail read's second decode, amortised: a
/// worker reads one chunk past its piece's end, and on a block-decoding source
/// the successor decodes that block too (`KD20`). It does not widen what a
/// reader holds — retention is capped by [`BufferPool::slots`], not by the
/// piece — and it costs granularity, a window being `want × k` units wide.
const BOUNDARIED_PARTITION_UNITS: usize = 1;

/// What a source holds resident while some number of concurrent workers read
/// it — **a shape rather than a scalar**, because the cost is not linear in
/// the count. See `docs/design/decisions.md`, "D4".
///
/// Two terms:
///
/// - **per worker**, paid once for each concurrent reader, which is what
///   [`Partitioning::partition_bytes`] states; and
/// - a **shared pool**, `(pool_depth.max(workers) − 1) × pool_unit` — the
///   buffers a pool retains beside the one each worker is filling, which is
///   the block pool's retention list and nothing every other source has.
///
/// **The pool term is arithmetic from [`BufferPool`], not a safety margin.**
/// [`BufferPool::slots`] clamps a pool at `POOL_DEPTH.max(jobs)` and
/// [`BlockCache::slot`] drains the retention list to `slots − 1` *before*
/// obtaining the buffer [`BlockCache::retain`] then pushes back onto it, so
/// the pool holds `slots − 1` retained units beside the one unit each worker
/// has in flight — unbounded in the block size, which is why
/// [`MEMORY_RESERVE`] cannot absorb it.
///
/// **A budget is solved against this, never divided by it**
/// ([`WorkerMemory::affords`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkerMemory {
    per_worker: u64,
    pool_unit: u64,
    pool_depth: usize,
}

impl WorkerMemory {
    /// A cost that is `per_worker` bytes a worker and nothing else — every
    /// source's shape but the block-decoding one's.
    pub const fn per_worker(per_worker: u64) -> Self {
        Self { per_worker, pool_unit: 0, pool_depth: 0 }
    }

    /// State that a pool retains `unit` bytes for every slot but the one the
    /// worker in flight is filling, over a pool `depth` slots deep or one slot
    /// per worker, whichever is more — on top of the per-worker term.
    pub const fn pooling(mut self, unit: u64, depth: usize) -> Self {
        self.pool_unit = unit;
        self.pool_depth = depth;
        self
    }

    /// The per-worker term alone — **what one concurrent reader holds on its
    /// own**, and what a *cut* is sized by ([`Partitioning::window_end`]).
    ///
    /// It is deliberately not "what one more reader adds", which is not one
    /// number: the shared pool grows by a unit for every reader past
    /// `pool_depth` and by nothing below it. What stays true at every count is
    /// that a reader holds this much of its own — [`WorkerMemory::at`] is what
    /// a caller sizing an allowance asks.
    pub const fn bytes_per_worker(self) -> u64 {
        self.per_worker
    }

    /// The shared pool at `workers` readers: `pool_depth.max(workers) − 1`
    /// units, which is [`BufferPool::slots`] less the slot a decode is writing
    /// into — flat up to the depth and rising with the count above it, and
    /// zero only for a depth of one at one reader.
    pub fn pool_bytes(self, workers: usize) -> u64 {
        (self.pool_depth.max(workers).saturating_sub(1) as u64).saturating_mul(self.pool_unit)
    }

    /// What `workers` concurrent readers cost, in bytes.
    pub fn at(self, workers: usize) -> u64 {
        self.per_worker.saturating_mul(workers as u64).saturating_add(self.pool_bytes(workers))
    }

    /// Whether this source states no cost at all, which is the declining
    /// default: such a source is bounded by the caller's own count and by
    /// nothing here.
    pub const fn is_zero(self) -> bool {
        self.per_worker == 0 && self.pool_unit == 0
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
    /// **Two regimes, because the cost is piecewise linear.** At or above
    /// `pool_depth` every reader adds its own term *and* a pool slot, so the
    /// cost is `n × (per_worker + pool_unit) − pool_unit` and the largest
    /// affordable count there is a single division. Below the depth the shared
    /// pool is a constant `(pool_depth − 1)` units, so the cost is linear again
    /// with a different intercept; those candidates are enumerated rather than
    /// divided for, and there are at most `pool_depth` of them — which every
    /// shipped shape sets to [`POOL_DEPTH`], though the type accepts any.
    ///
    /// The cost is non-decreasing in the count on both pieces, so the first
    /// regime's answer is final whenever it reaches the depth at all.
    pub fn affords(self, cap: u64, most: usize) -> usize {
        let most = most.max(1);
        let above = match self.per_worker.saturating_add(self.pool_unit) {
            0 => most,
            slope => usize::try_from(cap.saturating_add(self.pool_unit) / slope)
                .unwrap_or(usize::MAX)
                .min(most),
        };
        if above >= self.pool_depth {
            return above.max(1);
        }
        (1..=most.min(self.pool_depth)).rev().find(|workers| self.at(*workers) <= cap).unwrap_or(1)
    }
}

/// A source's answer to [`ByteRangeSource::partitions`]: where to split, what
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
    /// term. `crate::stream::worker_count` solves against it,
    /// `crate::stream::plan_partitions` builds a query's charge from it, and
    /// `crate::leader` reads it for the `scan arrangement` note.
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
    /// For a plain file that is [`PLAIN_PARTITION_CHUNKS`] read chunks, capped
    /// at [`POOL_MAX_BYTES`] and floored at one chunk, which is the **cut**
    /// size and not what a worker of it holds, so this over-bills that source
    /// (`KD25`). For a block-decoding compressed one it
    /// is the block unit **once** — the block being decoded is the block the
    /// reader then retains — plus the chunk buffer a straddling read is
    /// assembled into, plus the decoder's own retention. The rest of the
    /// retention list is shared and is [`WorkerMemory`]'s second term.
    ///
    /// **The decoder's own retention is a declared number rather than a
    /// guess**: `xz_seek::Reader::decode_footprint()` is the same whatever
    /// range is read, so the source charges it once at construction. The table
    /// records only the first block's dictionary per stream, so it is an
    /// estimate and not a ceiling; what cannot understate is the reader's
    /// `memlimit`, compared against each block's own declared dictionary
    /// before any backend object is built (`docs/design/decisions.md`, "D16").
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
    /// [`PartitionRead`], and `crate::leader::scan_region`, which is the
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
    /// past `limit` — the number `crate::leader::scan_region` hands
    /// `crate::stream::cut`, and **the cut size, which is not the memory
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
    ///   and [`crate::stream::cut`]'s thinning picks every `k`-th boundary.
    ///   Fewer boundaries left is the region's tail: the window runs to
    ///   `limit`, and thinning still cannot give a piece more than `k` units.
    ///
    /// **Do not size the window at `want × unit`.** From a mid-block frontier
    /// that window contains `want` boundaries, [`crate::stream::cut`] is asked
    /// for `want` pieces, and the thinning drops the first — putting two
    /// blocks in the first piece. Hence "strictly past `start`" and the `× k`.
    /// Asserted by
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
/// memory that concurrency may hold.
///
/// **The library defaults to a [`Parallelism::Serial`] stating no budget**,
/// which is the serial code path and not a pool of one. See
/// `docs/design/decisions.md`, "D1".
///
/// **Two numbers, and whichever binds first wins**, mirroring
/// `xz_seek::Bulk::new(workers, budget_bytes)`. Neither is defaulted inside
/// [`Parallelism::Workers`].
///
/// **The bytes here are a resolved pool budget, not the allowance a person
/// states.** A caller that means "this is all the memory the process may have"
/// says so through [`Parallelism::within`], which carves the budget out of it;
/// what survives into this type is the carved number, because that is the one
/// every pool and every affordability test reads
/// (`docs/design/decisions.md`, "D83").
///
/// **`Serial` is a state, not the number one.** One worker and the serial path
/// are the same execution, so [`Parallelism::workers`] answers `Serial` for a
/// count of one rather than a degenerate `Workers`, which keeps "is this
/// parallel" a match on the value. The two numbers are independent, so the
/// collapse takes only the count down: `Serial` carries the stated budget as
/// an `Option`, and `None` is the distinct fact that nobody stated a budget at
/// all ([`Parallelism::default`], answered by [`DEFAULT_MEMORY_BUDGET`]).
///
/// **Three mechanisms read it, and only the third spawns.** `memory_bytes`
/// sizes both pools in a source and decides whether a compressed source can
/// afford to decode a whole block (`docs/design/decisions.md`, "D16"); `jobs`
/// is the block pool's depth and — capped by what the bytes afford — how many
/// sub-streams a partitioned replay is cut into
/// (`crate::table_stream_partitions`), a ceiling rather than a request. The
/// third is `crate::leader::scan_region`
/// (`docs/design/decisions.md`, "D48").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parallelism {
    /// The serial code path: one thread, reading in file order — inside
    /// `memory_bytes` where the caller stated one.
    Serial {
        /// What that one thread's source may hold in pooled buffers, in
        /// bytes, or `None` where the caller stated no allowance at all. The
        /// worker count collapsing to the serial path says nothing about the
        /// budget beside it, so the budget survives the collapse.
        memory_bytes: Option<u64>,
    },
    /// At most `jobs` concurrent workers, holding at most `memory_bytes`
    /// between them.
    Workers {
        /// The ceiling on concurrent workers — a ceiling rather than a
        /// request, since some input shapes admit no parallelism at all.
        jobs: NonZeroUsize,
        /// What those workers may hold in their source's **pools**, in bytes —
        /// a resolved budget and not the resident allowance it was carved from
        /// ([`Parallelism::within`]), which is the number a memory cgroup is
        /// denominated in. A worker count cannot be promised to either: what a
        /// given count costs scales with the source's block size
        /// ([`WorkerMemory::at`]).
        memory_bytes: u64,
    },
}

/// The library's own default: the serial path, stating no budget
/// (`docs/design/decisions.md`, "D1").
impl Default for Parallelism {
    fn default() -> Self {
        Self::Serial { memory_bytes: None }
    }
}

impl Parallelism {
    /// `jobs` workers inside `memory_bytes`, or the serial path inside that
    /// same budget where `jobs` is one or zero.
    ///
    /// **The collapse is on the worker count alone**: the bytes beside a count
    /// of one were still stated, so they ride through into
    /// [`Parallelism::Serial`]'s own field.
    ///
    /// Zero reads as one rather than as an error, exactly as
    /// `xz_seek::Bulk::new` reads it: it is a shape a caller's own arithmetic
    /// produces.
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
    /// `None` rather than [`DEFAULT_MEMORY_BUDGET`]: a source that already
    /// holds a budget from an earlier announcement must be able to tell "the
    /// caller said nothing" from "the caller stated that same number".
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
    /// through [`MEMORY_RESERVE`].
    ///
    /// **It is a convenience over two primitives, and taking it is opting
    /// in**: [`Parallelism::default`] is still the serial path stating no
    /// budget. See `docs/design/decisions.md`, "D1" and "D11".
    ///
    /// **An unlimited environment falls back to the shipped constant**: with
    /// no limit found and no source to recommend otherwise, this is
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
    /// **It answers with both numbers, because they are a pair**: where the
    /// allowance affords fewer than `jobs`, the answer is the smaller count
    /// *and* the budget that count spends (`docs/design/roadmap.md`, "A
    /// default runs as fast as the allocation permits"). A caller holding a
    /// count somebody **stated** keeps that count and takes only the budget
    /// from here (`Discovered::resolve`).
    ///
    /// **The composition is not a `min`**, which would collapse a compressed
    /// scan to serial on an unlimited host (`docs/design/decisions.md`, "D3").
    /// What distinguishes *no limit found* from *a small limit* is that the
    /// first has no cap at all:
    ///
    /// - a discovered limit is carved by [`Parallelism::within`], the same
    ///   arithmetic a stated allowance gets: it caps at `limit −
    ///   MEMORY_RESERVE`, and `jobs × per_worker` — or
    ///   [`DEFAULT_MEMORY_BUDGET`] where the caller has no recommendation — is
    ///   taken no higher, and may fall **below**
    ///   [`DEFAULT_MEMORY_BUDGET`]. The **count** answers to a second
    ///   condition: its predicted resident must leave
    ///   [`MEMORY_MARGIN_PERCENT`] of the limit unused ([`margin_allowance`]);
    /// - no limit found caps at half of [`available_memory`], an estimate two
    ///   processes reading at once each see the whole of.
    ///
    /// **A caller with no recommendation and no limit states nothing**, which
    /// is [`Parallelism::default`] at a serial count and
    /// [`DEFAULT_MEMORY_BUDGET`] above one.
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
            // The one carving, shared with a stated allowance
            // ([`Parallelism::within`]).
            Some(limit) => return Self::within(jobs, memory, limit.bytes),
            // Nothing discovered and nothing recommended: no cap to state, and
            // no recommendation to cap, which is the library's own default.
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

    /// The arrangement `jobs` workers take inside a **resident allowance** the
    /// caller states — `pgdq --memory`, and an embedder that means by its
    /// number what a container means by one.
    ///
    /// **It is [`Parallelism::discover_in`]'s own carving, called with a
    /// stated number in place of the discovered limit**: [`MEMORY_RESERVE`]
    /// comes off the top for everything the pools do not bill, the count is
    /// held to [`margin_allowance`], and the budget it answers with is what
    /// that many workers spend. Neither number the caller gets back is the one
    /// it typed, and that is the point — the typed number bounds the
    /// **process**, where the budget bounds the pools
    /// (`docs/design/decisions.md`, "D83").
    ///
    /// **The count it returns is a recommendation lowered to fit**. A caller
    /// holding a count somebody *stated* keeps that count and takes only the
    /// budget from here, exactly as against a discovered limit
    /// (`docs/design/roadmap.md`, "A default runs as fast as the allocation
    /// permits").
    pub fn within(jobs: usize, memory: Option<WorkerMemory>, allowance: u64) -> Self {
        let (jobs, budget) = Self::fit(
            jobs,
            memory,
            allowance.saturating_sub(MEMORY_RESERVE),
            Some(margin_allowance(allowance)),
        );
        Self::workers(jobs, budget)
    }

    /// The largest pair `(count, budget)` that fits inside `cap`: as many of
    /// `jobs` workers as `cap` affords at `per_worker` each, and exactly what
    /// that many of them spend.
    ///
    /// **The floor is one worker at whatever `cap` is**, not one worker's
    /// worth of bytes: a cgroup at or under [`MEMORY_RESERVE`] resolves to a
    /// budget of zero, and the floors already in the mechanism turn that into
    /// one reader on the streaming path. A caller recommending nothing is
    /// capped at [`DEFAULT_MEMORY_BUDGET`] and keeps its count.
    ///
    /// **It solves rather than divides** ([`WorkerMemory::affords`]), and the
    /// budget it names is what that many workers actually spend, pool included
    /// (`docs/design/decisions.md`, "D4").
    ///
    /// **`charge_ceiling` is the margin, and it bounds the count alone**: the
    /// most this arrangement may be *charged* if its predicted resident —
    /// `memory.at(n)` plus [`MEMORY_UNPOOLED_BOUND`] — is to leave
    /// [`MEMORY_MARGIN_PERCENT`] of the allowance unused
    /// ([`margin_allowance`]), whether that allowance was discovered or typed
    /// (`docs/design/decisions.md`, "D83"). The budget stays `cap`-bounded
    /// rather than ceiling-bounded, so what a lowered count reports is still
    /// what it spends (`docs/design/decisions.md`, "D3").
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
/// line: the stated byte count, or [`DEFAULT_MEMORY_BUDGET`] — what every pool
/// falls back to — marked `(default)`. Deliberately a free function rather
/// than a method on [`Parallelism`], whose own `memory_bytes` must keep "the
/// caller said nothing" apart from "the caller stated that number"
/// (`docs/design/decisions.md`, "D64").
pub(crate) fn memory_budget_display(p: Parallelism) -> String {
    match p.memory_bytes() {
        Some(bytes) => bytes.to_string(),
        None => format!("{DEFAULT_MEMORY_BUDGET} (default)"),
    }
}

/// What a source may hold in pooled buffers when the caller has stated no
/// budget of its own — [`Parallelism::default`]'s number, and what a `pgdq` run
/// falls back to where neither `--memory` nor a limit says otherwise.
///
/// **It is the serial path's budget and deliberately not the largest block
/// anyone might write**: [`POOL_DEPTH`] slots at the largest chunk size the
/// `chunk-size` figure measured, so no chunk size in that range loses a slot
/// to it. It is also the line a compressed source's whole-block decode is
/// refused above ([`BlockCache::affordable`]). See
/// `docs/design/decisions.md`, "D3".
pub const DEFAULT_MEMORY_BUDGET: u64 = 64 << 20;

/// What [`Parallelism::within`] holds back from a memory allowance, discovered
/// or stated, in bytes: everything the process holds that the pools' budget
/// does not bound — the runtime's threads, glibc's per-thread arenas, the
/// decoder state a compressed source keeps outside its pools, and the binary
/// itself.
///
/// **It is a subtraction rather than a fraction**
/// (`docs/design/decisions.md`, "D3").
///
/// **What it must satisfy: the worst observed rep leaves
/// [`MEMORY_MARGIN_PERCENT`] of the limit**, a cgroup's killer reading one
/// run's peak. This is the smallest candidate measured against that criterion
/// (`reserve`); what it covers is the remainder above
/// [`BlockCache::reader_bytes`] and not a per-reader term, the block pool's
/// retention list being billed by [`WorkerMemory`].
///
/// **It is the cap, and it is not the bound on that excess** — that is
/// [`MEMORY_UNPOOLED_BOUND`], which [`margin_allowance`] predicts with. This
/// one comes off the top of the allowance and decides what
/// [`BlockCache::affordable`] sees, so **raising it is also what declines the
/// block path**. One constant, taken from the compressed leg, a source's own
/// answer being downstream of recognition and so of I/O.
///
/// *Rejected: sizing it from glibc's arena count.* There is no getter for
/// `M_ARENA_MAX` and this crate does not set a cap
/// (`docs/design/decisions.md`, "D13").
///
/// **Below it the budget goes to zero rather than to a floor**, which the
/// floors already in the mechanism turn into one reader's worth on the
/// streaming path.
///
/// deficiency: KD34 — this value was fitted before statistics existed, and
/// what a run holds above its charge and its statistics account has since been
/// attributed at a worst 544 MiB on a compressed `query` — well past the
/// 384 MiB held back here, with every wide-text compressed `query` leg from a
/// 1 GiB limit up killed in every rep. That attribution was taken on the
/// `introspect` build, so it sizes the gap without standing in for the blind
/// gate a constant has to pass; P23 owns both.
pub const MEMORY_RESERVE: u64 = 384 << 20;

/// How much of a memory allowance a resolved arrangement must leave unused, as
/// a percentage of that allowance — the criterion [`MEMORY_RESERVE`] was chosen
/// against, enforced on the worker count instead of being left to hold by
/// accident.
///
/// **A judgement rather than a derived number**, about the worst rep and not
/// the median (`docs/design/decisions.md`, "D3"). A constant reserve leaves a
/// shrinking *share* of the allowance as the allowance grows, so the cap alone
/// stops meeting the criterion at the top end; enforcing it on the count is
/// what makes the answer a property of the allocation rather than of the
/// host's width.
///
/// **Enforced once, against [`MEMORY_UNPOOLED_BOUND`]**: the predicted
/// resident a count is held to is `WorkerMemory::at(n)` plus that bound, not
/// plus [`MEMORY_RESERVE`], which would subtract the margin a second time. So
/// it binds above `5 × (MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND)` and nowhere
/// below.
///
/// **It bounds the count and never the budget, and it cannot decline a
/// reader**: [`Parallelism::fit`] keeps one worker at whatever the cap is and
/// reports the budget that count spends, and [`BlockCache::affordable`] reads
/// the budget the cap leaves, untouched by the margin.
///
/// **What a gathering pass holds is inside it too**: the statistics account is
/// billed against this same margin, [`statistics_allowance`] handing a mapping
/// pass what the resolved arrangement leaves under it and a block that does not
/// fit declining (`docs/design/decisions.md`, "D85").
pub const MEMORY_MARGIN_PERCENT: u64 = 20;

/// What a scan holds resident **outside the pools the budget bills**, bounded:
/// the runtime's threads, glibc's per-thread arena retention, the decoder
/// state a compressed source keeps beyond [`BlockCache::reader_bytes`], and
/// the binary itself. [`margin_allowance`] predicts with it, and nothing else
/// reads it.
///
/// **A bound, not a term, and it is read off a grid rather than fitted**: the
/// smallest step covering the unnamed remainder `held − at(jobs)` over the
/// grid behind `reserve` (`docs/design/decisions.md`, "D3").
///
/// *Rejected: the next step down*, which halves the margin's crossover and
/// starts taking readers across the band the margin exists to leave alone.
/// *Rejected: a bound per block size*, which the margin cannot state before
/// the file is open.
///
/// **What it is, is attributed only in bulk**: glibc's retained free memory at
/// exit (`fordblks`) is most of it, and no term table sums to it.
/// `scripts/measure.py`'s `charge_model_problem` faults a cell whose remainder
/// exceeds this, which is the finding that would move it. It is not
/// [`MEMORY_RESERVE`] because the reserve is what an allowance hands back
/// before anything is spent, where this is what the arrangement is predicted
/// to hold on top of what it spends.
pub const MEMORY_UNPOOLED_BOUND: u64 = 256 << 20;

/// The most a resolved arrangement may be **charged** under an `allowance` —
/// a discovered limit or a stated `--memory` — if its predicted resident is to
/// leave [`MEMORY_MARGIN_PERCENT`] of it unused: `(100 − margin)% of
/// allowance`, less [`MEMORY_UNPOOLED_BOUND`].
///
/// **The predicted resident is `charge + MEMORY_UNPOOLED_BOUND`**, that
/// constant being what bounds the one term the charge does not bill. Using
/// [`MEMORY_RESERVE`] here instead would apply [`MEMORY_MARGIN_PERCENT`] twice,
/// the reserve being the smallest constant meeting that criterion under the
/// cap rule (`docs/design/decisions.md`, "D3").
///
/// Integer arithmetic, rounding **down** the fraction of the allowance so the
/// ceiling errs small.
fn margin_allowance(allowance: u64) -> u64 {
    (allowance / 100)
        .saturating_mul(100 - MEMORY_MARGIN_PERCENT)
        .saturating_sub(MEMORY_UNPOOLED_BOUND)
}

/// **What a resolved arrangement leaves for a mapping pass's statistics**: the
/// bytes a `crate::statistics::StatisticsAccount` may hold under `allowance` —
/// a discovered limit or a stated `--memory` — once `budget`, the read-buffer
/// budget that arrangement resolved to, comes off [`margin_allowance`]'s
/// ceiling, which already stands [`MEMORY_UNPOOLED_BOUND`] below its fraction
/// of the allowance (`docs/design/decisions.md`, "D85").
///
/// **It is carved after the workers, never before them**
/// (`docs/design/decisions.md`, "D85"): the count is fixed before a byte is
/// read, while statistics are known only as they accumulate.
///
/// **[`MEMORY_RESERVE`] is not subtracted again here.** The reserve comes off
/// the top for the *cap* a count is solved against, where this is the *margin*
/// ceiling, which already stands [`MEMORY_UNPOOLED_BOUND`] below its fraction
/// of the allowance — subtracting both would take the margin twice, exactly as
/// [`margin_allowance`] refuses to.
///
/// **Which of the two bounds binds decides what is left, and the bands run the
/// other way round from the intuition.** [`Parallelism::fit`] solves the count
/// against `cap.min(ceiling)`, so below `5 × (MEMORY_RESERVE −
/// MEMORY_UNPOOLED_BOUND)` the *cap* binds and what is left here is at least
/// `ceiling − cap`, strictly positive — an allowance in that band cannot starve
/// statistics. At and above it the *ceiling* binds, the count is solved right up
/// against the number this subtracts from, and statistics get only the slack one
/// worker's step leaves: a wide host at a high `--jobs` is where every block
/// declines, not a narrow one. At or below `MEMORY_UNPOOLED_BOUND × 100 /
/// (100 − MEMORY_MARGIN_PERCENT)` the ceiling is zero on its own and nothing is
/// ever gathered. Both lines are arithmetic between the constants and both are
/// pinned by this module's tests.
///
/// Saturating, so an arrangement whose budget already fills the margin leaves
/// zero rather than wrapping, and every block declines.
pub fn statistics_allowance(allowance: u64, budget: u64) -> u64 {
    margin_allowance(allowance).saturating_sub(budget)
}

/// A cgroup v1 `memory.limit_in_bytes` at or above this reads as *no limit*
/// (`docs/design/runtime-invariants.md`, `RT4`).
///
/// **A threshold, never an equality test.** The unset value is
/// `PAGE_COUNTER_MAX × PAGE_SIZE`, a function of the page size and the word
/// width, so it differs between 4 KiB, 16 KiB and 64 KiB pages and on a
/// 32-bit kernel; 4 TiB is under all of them while being more memory than a
/// cgroup is given.
///
/// It is applied to the v2 files too, where it is very nearly unreachable:
/// `memory.max` spells "no limit" as the string `max` (`RT2`), so a *stated*
/// v2 limit above 4 TiB is the only thing this could misread, and reading one
/// as unlimited falls back to the [`available_memory`] cap.
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
/// descendant's own file, and nothing orders `memory.high` against
/// `memory.max`, across levels or within one. `memory.high` is read at all
/// because it throttles where `memory.max` kills (`RT3`).
///
/// **The walk is bounded by the mount point, not by counting separators**:
/// inside a cgroup namespace the namespace root is what is mounted, so walking
/// up from the `0::` path never leaves it. Limits set outside the namespace
/// still bind and are simply not readable.
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
/// (`docs/design/decisions.md`, "D11").
///
/// **The path is carried because the two v2 files do different things and the
/// walk minimises over both**: `memory.high` throttles where `memory.max`
/// kills (`RT3`), and either may be stated on an ancestor rather than on the
/// leaf (`RT5`), so a cut budget is only actionable beside the file whose
/// number did the cutting. It is a display fact: nothing in the library
/// branches on it, and a status line is its one consumer
/// (`docs/design/decisions.md`, "D64").
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
/// `None` where nothing limits it (`docs/design/decisions.md`, "D11").
///
/// **A primitive, not a policy**: what a caller may usefully take from a
/// limit is [`Parallelism::discover`] (`docs/design/decisions.md`, "D11").
///
/// **`None` means no limit is being *enforced*, which is a complete and
/// checkable statement** — unlike "are we in a container", which no reading of
/// `/.dockerenv` or `/proc/self/cgroup` answers on every runtime.
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
/// both of** (`docs/design/decisions.md`, "D11"). This is the seam a fixture
/// tree is handed — here, and through [`Parallelism::discover_in`] and
/// [`available_memory_in`] — so a caller's own resolution can be pinned
/// against a v1 hierarchy, an unlimited one and a below-reserve one
/// (`pgdump_query-cli/tests/data/runtime/`). The memory controller lives in
/// exactly one hierarchy at a time (`RT6`), so a v1 shape cannot be produced
/// beside a v2 one on the same host.
///
/// It is not a chroot facility: paths are joined onto the root, so a root of
/// `/` is the real reading and anything else is a tree somebody built. What a
/// fixture tree establishes is that this reader handles the shape `RT4`
/// describes, not that a kernel still produces it.
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
/// only once [`discover_memory_limit`] has answered `None`.
///
/// **`MemAvailable` and not `MemFree`**, which excludes reclaimable page cache
/// and would throttle a scan for memory the kernel would hand straight back.
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
/// alive when chunk *N+1* is read. This is that depth with room to spare.
///
/// **It is a wish, not the bound** — the stated budget is the bound, and the
/// two together are what make a slot able to hold a decoded xz block rather
/// than only a read chunk (`docs/design/decisions.md`, "D9").
///
/// **It is also the floor under a stated worker count**, not a second number
/// beside it: a block pool of one makes eviction drain before every decode and
/// stops pooling exactly when a caller is holding a block. Neither depth is
/// measured against what block retention buys a seeking query.
const POOL_DEPTH: usize = 4;

/// The largest buffer worth keeping, in bytes, for a length nobody has
/// announced as a read size.
///
/// The read path's steady state is chunk-sized — [`crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES`],
/// tunable through `ScanOptions::chunk_size`. What can be far larger is
/// `crate::map::attach_text`'s coalesced span read, which happens once per map
/// and never again; holding one of those for the rest of a process would trade
/// what a scan holds resident (`peak-rss`) for an allocation nothing asks for
/// twice.
///
/// **Size stands in for one-off-ness, and the caller is what overrides it**:
/// [`ByteRangeSource::hint_read_size`] names the length a read loop is about
/// to repeat, and *becomes* the pool's slot size
/// ([`BufferPool::slot_bytes`]), so a buffer of that length is kept however
/// large it is and this constant governs only a pool nobody has announced to
/// (`docs/design/decisions.md`, "D9").
const POOL_MAX_BYTES: usize = 8 << 20;

/// How many read chunks a plain file's partition holds
/// ([`LocalFileSource::partitions`]).
///
/// **A partition is not one chunk, because a worker reads its piece and then
/// one chunk more**: `crate::leader::scan_region` reads `[start, end)` and
/// a chunk-sized tail read follows to finish the line ending at or past `end`,
/// so those bytes are the next partition's body read a second time
/// (`docs/design/decisions.md`, "D52"). The waste is `chunk / partition`, paid
/// per partition, so this multiple caps it at the multiple's reciprocal
/// (`docs/design/decisions.md`, "D9").
///
/// **The product is capped at [`POOL_MAX_BYTES`].** Nothing allocates a
/// partition-length buffer, so what the cap bounds is the **cut** — and,
/// because this number is also [`Partitioning::partition_bytes`], what a
/// budget is solved against (`KD25`). The multiple therefore shrinks as the
/// announced chunk grows, reaching **one** at [`POOL_MAX_BYTES`] and above.
///
/// **The shipped default sits exactly on the cap** — this multiple of
/// [`crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES`] is [`POOL_MAX_BYTES`] — and the two constants
/// are justified independently, so
/// [`a_shipped_plain_partition_is_eight_whole_chunks`] is what stops a later
/// change to either from silently capping the default configuration.
/// *Rejected:* deriving this from [`POOL_MAX_BYTES`].
const PLAIN_PARTITION_CHUNKS: usize = 8;

/// Read buffers, reused rather than allocated per chunk.
///
/// **What this is for is not allocator throughput.** A fresh `vec![0u8; len]`
/// per chunk is `calloc`, so the kernel or the allocator zeroes a chunk that
/// `read_exact_at` immediately overwrites (`chunk-size`), and hands the region
/// back once per chunk, driving `madvise` traffic (`allocator`). See
/// `docs/design/decisions.md`, "D10".
///
/// **A pooled buffer is fully initialized and stays at its own length.** It is
/// created once as `vec![0u8; len]` and thereafter only ever read into, so no
/// reuse memsets anything. A read shorter than the buffer takes the buffer
/// whole and the `Bytes` handed back is sliced down to what was read, so a
/// short final chunk does not shrink a pooled buffer.
///
/// **Whether an acquisition blocks is the caller's permission, not the pool's
/// policy.** [`BufferPool::obtain`] waits for a free slot where the read loop
/// granted [`WaitPolicy::MayWait`] and allocates past the budget where it
/// granted [`WaitPolicy::NeverWait`] (`docs/design/decisions.md`, "D5").
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
    /// in slots** ([`POOL_DEPTH`] slots of a decoded xz block can be the whole
    /// of a small cgroup). The byte budget makes the count the consequence
    /// ([`BufferPool::slots`]) and the memory the number a caller states.
    ///
    /// **It bounds the free list at or below its own slot size, and not above
    /// it.** [`BufferPool::slots`] clamps to at least one, so a unit larger
    /// than the budget still gets a slot at that unit's size; what keeps that
    /// floor from making the budget a fiction is that a decoded xz block is
    /// refused *before* a slot is asked for ([`BlockCache::affordable`]).
    ///
    /// `Relaxed` for the same reason [`BufferPool::hinted`] is.
    budget: AtomicUsize,
    /// The ceiling on the free list in slots, whatever the budget affords:
    /// [`POOL_DEPTH`], which every chunk pool keeps whatever count is
    /// announced, and which [`XzSource::apportion`] raises on the block pool
    /// alone to one slot per announced reader above it.
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
    /// It is an atomic rather than part of the `Mutex` because
    /// `hint_read_size` takes `&self`: a source is shared, and the
    /// announcement must not have to wait behind a `take` on another task.
    /// `Relaxed` is enough — nothing is published through it, and a hint that
    /// arrives a buffer late costs one allocation.
    ///
    /// **One value, so one pool describes one read unit.** It drives both
    /// [`BufferPool::keeps`] and [`BufferPool::slot_bytes`], and a pool asked
    /// to serve two units is wrong for one of them either way, so a source
    /// with a second read unit takes a second pool rather than a second hint
    /// (`docs/design/decisions.md`, "D17").
    hinted: AtomicUsize,
    /// What the read loop currently reading through this pool permits, as
    /// [`WaitPolicy`] encodes it — [`WaitPolicy::NeverWait`] until one is
    /// granted, so a pool nobody has spoken to never waits.
    ///
    /// `Relaxed` for the same reason [`BufferPool::hinted`] is. It is read
    /// *inside* the state lock by [`BufferPool::obtain`], so a policy that
    /// changes between the read and the wait cannot leave a charge unmatched —
    /// the charge is carried by the buffer, not re-derived at release.
    policy: AtomicUsize,
}

/// What a [`BufferPool`] holds under its lock.
#[derive(Debug, Default)]
struct PoolState {
    /// Buffers nobody is using, at most [`BufferPool::slots`] of them as of
    /// the last release: a lowered limit trims nothing until then.
    free: Vec<Vec<u8>>,
    /// Buffers handed to a [`WaitPolicy::MayWait`] loop and not yet
    /// returned — the term a waiting acquisition is bounded by, and the only
    /// one of the two the library sets. What in-flight batches pin is the
    /// caller's and is deliberately not counted here
    /// (`docs/design/decisions.md`, "D5").
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
    /// **The wait is safe only because a waiting loop holds one buffer** —
    /// the promise [`ByteRangeSource::hint_wait_policy`] documents.
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
    /// A caller pricing a *reader* and a pool sizing a *slot* want different
    /// answers to "nobody has said": the pool's own is [`POOL_MAX_BYTES`], and
    /// a charge divided into a memory allowance wants the chunk the scan is
    /// about to announce ([`XzSource::charged_chunk_bytes`]).
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
    /// **This is what makes the pool block-capable.** At the default chunk it
    /// is [`POOL_DEPTH`]; at a decoded xz block large enough that the depth
    /// would be the whole allowance, it is one.
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
    /// **It is a true bound only because [`BufferPool::keeps`] refuses
    /// anything above a slot and [`BufferPool::reserve`] takes the retained
    /// blocks out of the free list's share — and only for one reader**: with
    /// several decoding at once the retention list outgrows its share by up to
    /// their number ([`BlockCache::worker_memory`]). This is what a source with
    /// **two** read units subtracts before handing the remainder to the second
    /// pool (`docs/design/decisions.md`, "D17").
    ///
    /// **It bounds the two lists and not a buffer a caller is still holding.**
    /// A [`PooledBuffer`] taken by [`BufferPool::obtain`] is on neither list
    /// until it is released, and a [`BlockCache`] block evicted while a
    /// [`Bytes`] still views it is on neither list at all — so what this
    /// answers is the pool's own retention, the quantity
    /// [`XzSource::apportion`] divides (`docs/design/decisions.md`, "D4").
    fn held_bytes(&self) -> usize {
        self.slots().saturating_mul(self.slot_bytes())
    }

    /// Whether a released buffer is worth keeping: **anything that fits a
    /// slot**, which is the announced read length or [`POOL_MAX_BYTES`] where
    /// nothing has been announced ([`BufferPool::slot_bytes`]).
    ///
    /// **One rule, so that [`BufferPool::held_bytes`] is true.** A pool
    /// describes one read unit, and a buffer larger than that unit occupying a
    /// slot counted at the unit would make the free list hold
    /// `slots × POOL_MAX_BYTES` while `held_bytes` reported
    /// `slots × slot_bytes`.
    ///
    /// **What it costs is a second read unit**, which the plain path does not
    /// have: it reads [`PartitionRead::Chunked`], so every buffer it takes is
    /// the announced length. The one two-unit source is [`XzSource`], which
    /// takes a second pool rather than a second hint
    /// (`docs/design/decisions.md`, "D17").
    fn keeps(&self, len: usize) -> bool {
        len <= self.slot_bytes()
    }

    /// State how many slots a holder outside the free list is accounting for.
    ///
    /// The only one is [`BlockCache`]'s retained list, whose blocks hold
    /// [`PooledBuffer`]s of this pool's own slot size
    /// (`docs/design/decisions.md`, "D17").
    ///
    /// `Relaxed` for the same reason [`BufferPool::hinted`] is: nothing is
    /// published through it, and a value that arrives a buffer late costs one
    /// keep or one drop.
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
    /// **The charge is discharged whether or not the buffer is kept**: the
    /// slot is free the moment its holder let go of it, and counting *kept*
    /// buffers would leak a slot per refused release.
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

/// A local file, read via blocking positioned reads on a `spawn_blocking`
/// task (`tokio` has no native async positioned-read). See
/// `docs/design/decisions.md`, "D10".
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
    /// prefers one split point to another.
    ///
    /// The unit it is a multiple of is the pool's own slot size — the length a
    /// read loop announced, or the ceiling where none has
    /// ([`BufferPool::slot_bytes`]) — so a chunk already at or past
    /// [`POOL_MAX_BYTES`] is a partition on its own.
    ///
    /// **It is the cut size, and not what a partition holds.**
    /// `crate::leader::scan_region` reads a [`PartitionRead::Chunked`]
    /// piece one announced chunk at a time and the interior split caps what is
    /// outstanding at [`BufferPool::slots`], so this source holds
    /// `POOL_DEPTH` chunks flat in the reader count while
    /// [`Partitioning::partition_bytes`] bills [`PLAIN_PARTITION_CHUNKS`] of
    /// them per reader (`docs/design/decisions.md`, "D4").
    // Deficiency register: `deficiency: KD25` — so plain readers are bounded
    // by a charge describing nothing the path holds: the bill grows with the
    // count while what is held is flat at `POOL_DEPTH` chunks, and above a
    // budget of `PLAIN_PARTITION_CHUNKS × chunk × jobs` nothing bounds them at
    // all. **(c) unowned.** Closing it means billing the pool's real depth and
    // recommending what a reader costs; a reading of a parallel plain scan on a real
    // device is what reopens it.
    fn partitions(&self, _range: Range<u64>) -> Partitioning {
        let chunk = self.pool.slot_bytes();
        let bytes = chunk.saturating_mul(PLAIN_PARTITION_CHUNKS).min(POOL_MAX_BYTES).max(chunk);
        Partitioning::anywhere(bytes as u64)
    }

    /// **The budget sizes the free list; the worker count does not.** This
    /// source has one read unit, and a chunk buffer is taken and released
    /// inside a single `read_range` — so what the free list has to hold is the
    /// *replay* path's depth, which is what [`POOL_DEPTH`] is, and not a
    /// worker count (`docs/design/decisions.md`, "D9").
    ///
    /// Where the leader schedules concurrent readers over this source, a
    /// `parse` at `--jobs n` runs `n` fused workers against [`POOL_DEPTH`]
    /// chunk slots, so above that many the extra workers block for a slot
    /// rather than allocating.
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
/// (`docs/design/decisions.md`, "D15").
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
/// (`docs/design/decisions.md`, "D17").
///
/// **Retention is what makes per-call block decode affordable**: a block
/// decoded afresh per call would repeat the whole block's decode once per
/// chunk inside it, where a retained block makes the next read a slice. The
/// retained set is drained to [`BufferPool::slots`] less one before each
/// decode takes its slot.
///
/// **A retained block occupies a slot rather than adding to the free list**,
/// eviction happening *before* a slot is taken ([`BlockCache::slot`]) so the
/// evicted buffer is what the next decode reuses. **The retained list and the
/// free list share one count** — this list reserves what it holds
/// ([`BufferPool::reserve`]) — so one reader's ceiling is `slots * unit`, and
/// `workers` readers decoding at once hold up to `slots − 1 + workers` units
/// (`docs/design/decisions.md`, "D17").
///
/// *Rejected: retaining one block on the serial path.* A cap of one makes
/// [`BlockCache::slot`] keep zero and drain before every decode, so any
/// outstanding view forces a fresh allocation of the block unit.
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
    /// one** ([`BlockCache::affordable`]): the answer depends on a budget the
    /// caller has not stated yet at construction.
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
    /// whole blocks, in bytes: one block unit, the chunk buffer a straddling
    /// read is assembled into, and the decoder's own retention
    /// (`docs/design/decisions.md`, "D16").
    ///
    /// **One unit, because the block a reader decodes *becomes* the block it
    /// retains**: [`BlockCache::slot`] drains the retention list to
    /// `slots() - 1` before taking a slot, and [`BlockCache::retain`] pushes
    /// that same buffer's block onto the list it just drained. The rest of the
    /// list is `slots() - 1` units shared by every reader, and is
    /// [`WorkerMemory`]'s second term.
    ///
    /// It is one number with two consumers — [`BlockCache::affordable`] and
    /// [`xz_partition_advice`]. *Rejected:* two statements of the one
    /// cost, which is how a divisor comes to charge nothing for the decoder.
    fn reader_bytes(&self, chunk_bytes: u64, decode_bytes: u64) -> u64 {
        (self.unit as u64).saturating_add(chunk_bytes).saturating_add(decode_bytes)
    }

    /// What **the block path costs at `workers` concurrent readers**: the
    /// per-reader charge above, plus the retention list those readers share
    /// ([`WorkerMemory`]).
    ///
    /// **The shared term is `(POOL_DEPTH.max(workers) − 1) × unit`, and it is
    /// arithmetic from this pool rather than a measured term**: the free list
    /// and the retention list share the pool's slots
    /// ([`BufferPool::reserve`]), and [`BlockCache::slot`] drains to
    /// `slots - 1` before obtaining the buffer [`BlockCache::retain`] then
    /// pushes onto that same list, so the pool holds `slots - 1 + workers`
    /// units while each worker holds one unit at a time. No test asserts that
    /// bound, and the case it has to survive is [`BlockCache::retain`]
    /// replacing an entry for the same block while the reader that decoded it
    /// still views it (`KD20`'s double decode). *Rejected:* charging
    /// `2 × workers` plus a decaying floor
    /// (`docs/design/decisions.md`, "D4"; `parallel-peak-rss`).
    fn worker_memory(&self, chunk_bytes: u64, decode_bytes: u64) -> WorkerMemory {
        WorkerMemory::per_worker(self.reader_bytes(chunk_bytes, decode_bytes))
            .pooling(self.unit as u64, POOL_DEPTH)
    }

    /// Whether `budget` admits one such reader — the line that decides between
    /// block decode and the streaming reader
    /// (`docs/design/decisions.md`, "D16").
    ///
    /// **It is asked of *one* reader and charges that reader's share of the
    /// retention list with it**: a pool serving a single reader still holds
    /// [`POOL_DEPTH`] slots, so admitting the path on the per-reader charge
    /// alone would allow `(POOL_DEPTH − 1)` units the caller never granted.
    /// The chunk and the decoder come off the top, both being paid on the
    /// streaming path too, so what a decline saves is the block slots.
    ///
    /// **It does not hold on the query path, and `KD23` is that defect.** A
    /// query partition is cut over a whole `CopyBlock` rather than through
    /// [`Partitioning::window_end`]'s window, so a held batch can pin several
    /// units where this charges one. The decline it draws on a large-block
    /// file is accepted rather than worked around: it is reported
    /// (`crate::stream::compressed_block_path_declined`), and a caller who
    /// wants the path back states a budget.
    ///
    /// *Rejected: a fixed refusal line beside a fixed budget*
    /// (`docs/design/decisions.md`, "D16"). Reading the line off the budget
    /// makes the stated number true: a unit the caller did not allow room for
    /// is never decoded whole.
    ///
    /// **What it declines is memory, not seekability**: a file written with
    /// large blocks (`xz -9 -T0`, or an explicit `--block-size`) is declined
    /// under the default budget and does have parallelism to lose, which
    /// `--memory` or `Parallelism::Workers` reverses.
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
    /// guarantee**: a retained block is normally also the block some reader is
    /// holding a view into, so the drain may free no slot at all. That is why
    /// the block pool is never granted a wait
    /// ([`XzSource::hint_wait_policy`], and `docs/design/decisions.md`, "D5").
    /// A block evicted while a [`Bytes`] still views it stays alive until that
    /// view drops.
    ///
    /// **The reservation is lowered before the evicted blocks drop**, and the
    /// order is load-bearing: a release seeing the pre-eviction reservation
    /// would find no room and discard the very buffer this take is about to
    /// reuse.
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
/// (`docs/design/decisions.md`, "D14", "D15").
///
/// **Three file handles, deliberately.** `open` walks the file's stream
/// footers once and hands one handle to the `xz_seek::Reader`, which owns it
/// for decoding; `stat_file` answers
/// [`ByteRangeSource::stored_size`]/[`ByteRangeSource::modified`] with a plain
/// `stat` and must never disturb the decoder's live position to do it; and
/// `data_file` is what block decodes read their compressed bytes through,
/// positioned reads only, so a decode running outside the mutex shares no
/// cursor with anything.
///
/// **A read decodes the blocks it lands in, and the reader behind the mutex is
/// the fallback.** `xz_seek::BlockTask` is `Copy` and owns everything a decode
/// needs, so `read_range` takes a task, decodes the block into a slot of its
/// own [`BlockCache`], and slices the answer out of it — with the mutex held
/// only long enough to *name* the task, never across the decode. The streaming
/// `xz_seek::Reader::read_at` path is kept for the one shape block decode
/// refuses ([`BlockCache::affordable`]).
///
/// `Verify::Full` is `xz_seek::Reader::new`'s own default, so nothing here has
/// to ask for it; a whole-block decode is stronger still, comparing the check
/// before it returns.
pub struct XzSource {
    path: PathBuf,
    stat_file: Arc<std::fs::File>,
    data_file: Arc<std::fs::File>,
    /// The seek table, aliased beside the reader so that `size()`,
    /// `seek_table()` and the per-read `blocks_in` lookup take no lock at all.
    /// The `Arc` is the reader's own ([`XzSource::assembled`]), so this is a
    /// handle onto one table rather than a second copy of it.
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
    /// Present does not mean *taken*: [`XzSource::block_path`] decides per
    /// read, a block the caller's budget cannot hold being streamed.
    blocks: Option<Arc<BlockCache>>,
    /// What one decode of this file retains beyond the slot it writes into —
    /// the LZMA2 dictionary, the compressed input chunk and the backend's own
    /// state (`xz_seek::Reader::decode_footprint`).
    ///
    /// **Read once, at construction**: it is a property of the file and of the
    /// backend, not of a range, and asking the reader per call would take the
    /// mutex the block path exists to stay off.
    decode_bytes: u64,
    /// What the caller stated it may hold, in bytes, across **both** pools —
    /// [`DEFAULT_MEMORY_BUDGET`] until one is announced.
    ///
    /// Held on the source rather than pushed straight into the pools: the
    /// split is derived from it and from the announced chunk size, which
    /// arrive in either order, and [`XzSource::apportion`] recomputes from
    /// this on both announcements.
    budget: AtomicUsize,
    /// The worker ceiling the caller stated — 1 until one is announced. It is
    /// the **block** pool's depth: one retained block per concurrent reader,
    /// where a chunk buffer is taken and released inside one read and wants
    /// the replay path's depth instead
    /// ([`LocalFileSource::hint_parallelism`]).
    jobs: AtomicUsize,
}

impl XzSource {
    /// Open `path` as `.xz`-compressed input, walking its stream footers to
    /// build the seek table before this call returns.
    ///
    /// This does not sniff the magic bytes; content-sniffing recognition
    /// across both source kinds is [`open_local`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        // The walk this names is the one [`XzSource::with_table`] exists to
        // skip, so only this constructor emits it — a cached table costs no
        // footer read and earns no line
        // (`docs/design/decisions.md`, "D64").
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
    /// (`docs/design/decisions.md`, "D18").
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

    /// The one place the two constructors agree: the reader's table is aliased
    /// so the source's own lookups never take its mutex, and the block pool is
    /// sized from that table or refused.
    ///
    /// **What the alias buys is every table read outside the reader's lock** —
    /// [`ByteRangeSource::size`], [`XzSource::partitions`],
    /// [`XzSource::default_workers`], and the `blocks_in` span arithmetic
    /// [`XzSource::read_by_blocks`] does on every read. The block path still
    /// locks once per cache **miss**, to build a `BlockTask` out of the
    /// reader's own decode settings ([`XzSource::block`]); a hit takes no
    /// reader lock, only the retention list's. It costs no second copy:
    /// `xz_seek::Reader::index_shared` hands
    /// out the `Arc` the reader itself holds.
    ///
    /// **Nothing bills the one copy that is left.** It is unbilled on an
    /// ordering rather than a size: the walk that builds it runs inside
    /// [`XzSource::open`], before any budget is announced
    /// (`docs/design/decisions.md`, "D4").
    // Deficiency register: `deficiency: KD26` — this table is the only term in
    // the memory account that grows with the *file* rather than with the
    // worker count, and a source whose index is not small beside
    // [`MEMORY_UNPOOLED_BOUND`] therefore holds bytes no charge names.
    // **(c) unowned**; closing it means a per-source term, which
    // [`WorkerMemory`] does not have. Billing it buys accuracy and no
    // protection, the walk having already happened.
    fn assembled(
        path: PathBuf,
        stat_file: Arc<std::fs::File>,
        data_file: Arc<std::fs::File>,
        reader: xz_seek::Reader<std::fs::File>,
    ) -> Self {
        let table = reader.index_shared();
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
    /// (`BufferPool::held_bytes`), which is [`POOL_DEPTH`] chunk slots. Chunks
    /// come first because that pool is the one every read path uses and the
    /// one the pool-miss cost was measured through (`chunk-size`)
    /// (`docs/design/decisions.md`, "D17").
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
            // block. That argues for a shallower floor; this is
            // `POOL_DEPTH`'s, inherited.
            //
            // **`jobs` here is the count the caller *announced*, not the count
            // `crate::stream::worker_count` then delivers.** They agree under
            // discovery and diverge where a stated `--jobs` outruns a stated
            // budget (`docs/design/decisions.md`, "D17").
            //
            // Deficiency register: `deficiency: KD21` — the slot count is then
            // bounded by the pool's byte budget rather than by the delivered
            // count, and the lists fill to it while each delivered reader
            // decodes into a buffer besides, so the source holds more than the
            // stated budget. **(c) unowned**; closing it means telling the
            // pool the *delivered* count, or [`BufferPool::slots`] reserving
            // for the decodes in flight — either reworks a pool's sizing rule.
            blocks
                .pool
                .set_limits(budget.saturating_sub(self.pool.held_bytes()), POOL_DEPTH.max(jobs));
        }
    }

    /// The chunk slot every one of this source's charges is stated against:
    /// the length a read loop announced ([`ByteRangeSource::hint_read_size`]),
    /// or [`crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES`] where none has yet.
    ///
    /// **The fallback is the chunk a scan settles at, not
    /// [`POOL_MAX_BYTES`]**: an allowance is solved against this number by
    /// [`Parallelism::fit`] before the file is open, while the gate the count
    /// then meets ([`BlockCache::affordable`]) is compared at the steady-state
    /// slot (`docs/design/decisions.md`, "D4"). *Rejected:* moving the gate to
    /// the recommendation instead, which would decline the block path on files
    /// that fit.
    ///
    /// **It is charged once a reader, and the chunk pool's free list beside it
    /// is charged nowhere** (`docs/design/decisions.md`, "D4").
    // Deficiency register: `deficiency: KD24` — that free list is a different
    // population from the per-reader buffer this bills, so a stated budget is
    // short by `⌊budget/chunk⌋.clamp(1, POOL_DEPTH)` chunks: flat in the
    // count, since the depth is a constant, and linear in whatever chunk the
    // caller announced. It is never *above* the stated budget unless one
    // chunk is. **(c)
    // unowned**; closing it means a count-independent term, which
    // [`WorkerMemory`] does not have.
    fn charged_chunk_bytes(&self) -> u64 {
        match self.pool.announced_bytes() {
            Some(len) => len as u64,
            None => SCAN_CHUNK_DEFAULT_SIZE_BYTES as u64,
        }
    }

    /// What concurrent block-decoding readers of this file would cost, whether
    /// or not the budget admits one: [`BlockCache::worker_memory`] over this
    /// source's own chunk size ([`XzSource::charged_chunk_bytes`]) and decoder
    /// charge — the per-reader charge and the retention list shared beside
    /// it, as one shape.
    ///
    /// **It is the one composition site**, keeping the recommendation
    /// ([`ByteRangeSource::default_worker_memory`]), the gate
    /// ([`BlockCache::affordable`]) and the advice
    /// ([`Partitioning::worker_memory`]) from being three statements of one
    /// cost ([`XzSource::charged_chunk_bytes`]).
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

    /// The block at `index`, from the retention list or freshly decoded into a
    /// slot of the block pool.
    ///
    /// Deficiency register: `deficiency: KD20` — two concurrent misses on one
    /// block decode it twice, and on the parallel scan path that is the common
    /// case: every fused worker's chunk-sized tail read lands in the block its
    /// *successor* owns, so a scan does about twice the decode work and its
    /// speedup is capped near half the reader count (`measurements.md`,
    /// `parallel-scan-throughput`). **(c) unowned.** The fix left is an
    /// in-flight map here — not taken, since it puts a second lock in front of
    /// the case that does *not* collide — a wider cut having been measured and
    /// refused ([`BOUNDARIED_PARTITION_UNITS`]).
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
        .ok_or_else(xz_short_read)?;
        let len = usize::try_from(task.uncompressed_len()).map_err(|_| xz_short_read())?;
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
    /// chunk-pool buffer.
    fn read_by_blocks(
        offset: u64,
        len: usize,
        table: &xz_seek::SeekTable,
        cache: &BlockCache,
        reader: &Mutex<xz_seek::Reader<std::fs::File>>,
        file: &std::fs::File,
        chunks: &Arc<BufferPool>,
    ) -> Result<Bytes> {
        let end = offset.checked_add(len as u64).ok_or_else(xz_short_read)?;
        let covering = table.blocks_in(offset..end);
        if covering.is_empty() {
            return Err(xz_short_read());
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
                .ok_or_else(xz_short_read)?;
            if start + len > block.len {
                return Err(xz_short_read());
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
            return Err(xz_short_read());
        }
        Ok(Bytes::from_owner(out).slice(..len))
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
            return Err(xz_short_read());
        }
        Ok(Bytes::from_owner(buf).slice(..len))
    }
}

/// The error a read past the end of the uncompressed stream is — the same
/// `UnexpectedEof` a short `read_exact_at` raises on the plain source, so
/// both sources report a caller/source disagreement identically.
fn xz_short_read() -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "xz stream ended before the requested range",
    ))
}

/// An `.xz` source's partitioning advice, read off the **read path it took**
/// rather than off the seek table.
///
/// Two answers, and which one applies is `blocks`:
///
/// **Block-decoding** — the boundaries are the block starts inside
/// `range`, so a partition is a whole number of blocks. A partition costs
/// what one concurrent reader holds — [`BlockCache::reader_bytes`], the
/// per-reader term of the [`BlockCache::worker_memory`] whose `at(1)`
/// [`BlockCache::affordable`] compares a budget against.
///
/// **Streaming fallback** — one partition, whatever the table says: two
/// workers on different partitions would each force the other's restart
/// through `xz_seek::Reader::read_at`
/// (`docs/design/decisions.md`, "D15").
///
/// Taken as a free function over the two pieces of state it reads, so the
/// fallback arm is assertable against a table that *does* have blocks.
fn xz_partition_advice(
    table: &xz_seek::SeekTable,
    blocks: Option<&BlockCache>,
    chunk_bytes: u64,
    decode_bytes: u64,
    range: Range<u64>,
) -> Partitioning {
    let Some(cache) = blocks else {
        // **The streaming arm charges the chunk buffer and not the
        // decoder**: that path keeps one `xz_seek::Reader` behind a mutex
        // however many readers a caller runs, so the decoder is a fixed
        // cost of the source rather than of a concurrent reader
        // ([`Partitioning::partition_bytes`]).
        //
        // Deficiency register: `deficiency: KD35` — one partition is what
        // the mutex admits, not what the budget affords: a decoder retains
        // far less than a decoded block, so several would fit where the
        // blocks they decode do not, and the budget that sends a file down
        // this arm is exactly the one that would benefit. **(c) unowned**;
        // closing it means a per-reader decode handle here and a figure
        // over the count, and the figure is the half this cannot skip.
        return Partitioning::single(chunk_bytes);
    };
    let covering = table.blocks_in(range.clone());
    let at = covering
        .filter_map(|i| {
            let start = table.blocks[i].uncompressed_offset;
            (start > range.start && start < range.end).then_some(start)
        })
        .collect();
    // Three statements the streaming arm above does not make, each
    // because a read inside a decoded block is a zero-copy slice of it:
    //
    // - **the retained unit is the partition, not the chunk**, so a caller
    //   adding its span allowance on top would count the partition's
    //   already-charged bytes twice ([`RetainedUnit`]);
    // - **a worker reads one of this source's units in one call**, which
    //   at the shipped cut width is the whole piece
    //   ([`PartitionRead::Whole`]). The unit is `cache.unit`, read off the
    //   cache rather than recomputed and non-zero by
    //   `BlockCache::for_table`'s own guard;
    // - **the block pool's retention list is stated beside the per-reader
    //   charge**, which is what lets `crate::stream::worker_count` solve
    //   for a count rather than divide by one
    //   ([`BlockCache::worker_memory`]).
    Partitioning::at(at, cache.reader_bytes(chunk_bytes, decode_bytes))
        .pooling(cache.unit as u64, POOL_DEPTH)
        .retaining(RetainedUnit::Partition)
        .reading(PartitionRead::Whole { unit: cache.unit as u64 })
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

    /// The compressed file's own on-disk length — a `stat` on the second
    /// handle, never the stream-index walk [`XzSource::size`] answers from
    /// memory (`docs/design/decisions.md`, "D21").
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
    /// concurrent reader (`docs/design/decisions.md`, "D17").
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
    /// loop permits (`docs/design/decisions.md`, "D5").
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
        xz_partition_advice(
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
    /// **Decode is the one shape that demonstrably scales**
    /// (`parallel-scan-throughput`; `docs/design/decisions.md`, "D2"). What
    /// the caller's budget affords still binds afterwards
    /// (`crate::stream::worker_count`).
    ///
    /// **Capped at this file's own block count**, `crate::stream::cut` cutting
    /// at block boundaries — and capped *here*, where the count is
    /// recommended, because the recommendation is multiplied into a budget
    /// request ([`ByteRangeSource::default_worker_memory`]) and printed beside
    /// it ([`Parallelism::discover_for`]).
    ///
    /// A failure to read the count answers **one** rather than propagating.
    fn default_workers(&self) -> usize {
        let cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        cores.min(self.table.block_count().max(1))
    }

    /// **What readers of this file hold**: [`XzSource::block_worker_memory`] —
    /// the per-reader charge plus the block pool's retention list, the shape
    /// `crate::stream::worker_count` and [`Parallelism::fit`] both solve
    /// against to hand back a count. `None` where there is no block path to
    /// buy — a file with no blocks, or one whose largest block is not a length
    /// on this target — since the streaming reader has no second worker to
    /// give at any budget.
    ///
    /// **It is charged at the chunk a scan settles at**
    /// ([`XzSource::charged_chunk_bytes`]), which is what
    /// [`BlockCache::affordable`] compares a budget against once the file is
    /// open. *Rejected:* charging the unannounced pool's [`POOL_MAX_BYTES`]
    /// ceiling instead, which runs the recommendation high and resolves fewer
    /// readers than fit. [`ByteRangeSource::block_decode_bytes`] is this same
    /// shape evaluated at one reader: one statement of what the block path
    /// costs.
    fn default_worker_memory(&self) -> Option<WorkerMemory> {
        self.block_worker_memory()
    }
}

/// How many uncompressed bytes [`FetchedXzSource`] throws away per decoder
/// call while skipping to a read's first byte, and again while completing the
/// block behind it.
///
/// On the stack, so a read that holds no whole block allocates nothing beyond
/// the buffer it is filling — which is the budget the piecewise arm exists to
/// respect — and large enough that skipping a 24 MiB block is thousands of
/// decoder calls rather than millions. It is `xz_seek`'s own drain chunk,
/// which sizes the same skip on the other side of `complete`.
const FETCHED_SKIP_CHUNK: usize = 4096;

/// Build an `.xz` file's seek table by driving `xz_seek`'s footer walk over an
/// **asynchronous** source, and say how many fetches it took.
///
/// `xz_seek::Walk` is the footer parse with the reads taken out: it hands out
/// a range and takes the bytes back, holding no borrow and no lifetime, so the
/// fetch is ours and may `await`. That is the whole reason a dump reached over
/// the network can be `.xz` at all —
/// `xz_seek::Reader` pulls through a *synchronous* positional trait, and
/// nothing can `await` inside it
/// (`docs/design/roadmap-P14-remote-input.md`, "D13").
///
/// `leading` is what the caller already holds of the file's first bytes — an
/// [`Origin`] probe's, normally. The walk's first request is the six magic
/// bytes at offset zero, which is exactly what that probe fetched, so a caller
/// passing them spends no round trip on them. Passing an empty slice is
/// correct and costs one.
///
/// **The count it returns is the file's, not this driver's.** The walk asks for
/// a footer, an index, a header and at least one padding probe per stream, in a
/// strictly backward chain where each request's position comes out of the bytes
/// of the one before it — so neither coalescing nor concurrency buys anything
/// on its own (`docs/design/roadmap-P14-remote-input.md`, "D1").
///
/// Deficiency register: `deficiency: KD36` — this driver fetches exactly what
/// it is asked for, so a cold walk over a dump compressed as many small streams
/// is that many round trips before a row is read. The remedy is a **straddling
/// window**: one read per stream positioned backward from the pending request
/// and sized to cover what the walk asks for next, which answers three of every
/// stream's four requests because a stream's header is adjacent to its
/// predecessor's padding, footer and index. **One fetch a stream is then a
/// floor, not a cost still looking for a remedy** — the distance to the next
/// boundary is knowable only from the index just read, and this file family has
/// no stream stride to predict it from. What would beat the floor is not a
/// better driver but a forward table build, which stops paying separately for
/// the walk at all. **(c) unowned**; promoted by a phase that tunes the network,
/// which is where the fetch policy this source defers belongs, and which is also
/// what would take the figure.
pub async fn walk_seek_table(
    source: &dyn ByteRangeSource,
    stored_size: u64,
    leading: &[u8],
) -> Result<(xz_seek::SeekTable, usize)> {
    let (mut walk, mut request) = xz_seek::Walk::begin(stored_size);
    let mut fetches = 0usize;
    loop {
        let asked = usize::try_from(request.len()).map_err(|_| xz_short_read())?;
        let step = if request.start() == 0 && asked <= leading.len() {
            walk.supply(request, &leading[..asked])?
        } else {
            fetches += 1;
            let bytes = source.read_range(request.start(), asked).await?;
            walk.supply(request, &bytes)?
        };
        match step {
            xz_seek::Step::Need(next) => request = next,
            xz_seek::Step::Done(table) => return Ok((table, fetches)),
        }
    }
}

/// A `ByteRangeSource` decoding an `.xz`-compressed dump the bytes of which
/// have to be **fetched** rather than pulled
/// (`docs/design/roadmap-P14-remote-input.md`, "D1", "D13").
///
/// **The difference from [`XzSource`] is the transport and nothing else.**
/// There the crate holds a `std::fs::File` and reads through it as it decodes;
/// here nothing can `await` inside a decode, so this source fetches a block's
/// whole compressed extent first — `xz_seek::BlockTask::compressed_range`
/// states it before the fetch — wraps it in an `xz_seek::Window` and decodes
/// out of that. The uncompressed side is unchanged: the same [`BlockCache`],
/// the same slot, the same zero-copy slice of a decoded block.
///
/// **Two arms, and the budget picks between them.**
///
/// - Where the stated budget holds a whole decoded block, a read is served
///   exactly as [`XzSource`]'s is: decode the block into a pooled slot, retain
///   it, slice the answer out of it.
/// - Where it does not, the block is read **in pieces** through
///   `xz_seek::BlockRead` — the handle skips forward to the first wanted byte,
///   fills the caller's buffer and compares the block's check when we complete
///   it — so what is held is the compressed window and the decoder, never the
///   plaintext. That is the arm with no local twin: a file can be pulled from
///   and so has `xz_seek::Reader::read_at` to fall back on, and a fetched
///   source has nothing (D20).
///
/// **A block is always completed**, which is what compares its check. Reading
/// a few kilobytes out of a large block therefore pays a decode of the rest —
/// today's guarantee on the path this replaces, carried over rather than a new
/// cost.
pub struct FetchedXzSource {
    /// The compressed bytes' transport: every fetch this source makes goes
    /// through it, so a cancellation, an identity precondition or a network
    /// failure arrives here in the words that source already uses.
    source: Arc<dyn ByteRangeSource>,
    /// The table, the memory limit, the verification state and the backend as
    /// one value — everything a decode needs that is not bytes. It holds no
    /// source, which is what lets this type exist (D13).
    layout: xz_seek::Layout,
    /// The layout's own table, aliased rather than copied, so `size()`,
    /// `seek_table()` and the per-read `blocks_in` lookup read one table.
    table: Arc<xz_seek::SeekTable>,
    /// The chunk unit: what a read straddling a block boundary is assembled
    /// into, and what the piecewise arm fills.
    pool: Arc<BufferPool>,
    /// The block unit, or `None` where this file has no blocks to decode.
    blocks: Option<Arc<BlockCache>>,
    /// What one decode here retains besides the plaintext: the decoder over a
    /// source that **lends** — an `xz_seek::Window` is read through
    /// `slice_at`, so no compressed input chunk is built — plus the largest
    /// window this file's blocks would make us fetch.
    ///
    /// **The window is charged rather than booked as unpooled.** Its length is
    /// known before the fetch, and a term that can be priced and is not is the
    /// falsification of [`MEMORY_UNPOOLED_BOUND`] rather than an instance of it
    /// (`docs/design/roadmap-P14-remote-input.md`, "D20").
    decode_bytes: u64,
    /// What the caller stated it may hold, across both pools —
    /// [`DEFAULT_MEMORY_BUDGET`] until one is announced.
    budget: AtomicUsize,
    /// The worker ceiling the caller stated, which is the block pool's depth.
    jobs: AtomicUsize,
}

impl FetchedXzSource {
    /// Read `source` as `.xz` from a seek table a previous walk of the *same*
    /// object produced, **without walking it again**
    /// (`docs/design/decisions.md`, "D18").
    ///
    /// `stored_size` is the compressed object's own length, which the caller
    /// has from its [`Origin`] probe; `xz_seek` validates the table against it
    /// and for internal consistency, reading **no bytes**. A table that does
    /// not describe this object is `Error::Xz(xz_seek::Error::InvalidTable)`,
    /// which is what [`open_remote`] turns into [`Recognized::Mismatch`]
    /// rather than a walk.
    pub fn with_table(
        source: Arc<dyn ByteRangeSource>,
        stored_size: u64,
        table: xz_seek::SeekTable,
    ) -> Result<Self> {
        // `Builder::new()` rather than a configured one, for `XzSource`'s
        // reason: it is the shortcut through exactly the defaults
        // `Reader::new` takes, `Verify::Full` included, so the two sources
        // decode identically.
        let layout = xz_seek::Builder::new().layout(table, stored_size)?;
        Ok(Self::assembled(source, layout))
    }

    /// The one place a constructor's pieces are put together: the table is
    /// aliased out of the layout, the block pool is sized from it or refused,
    /// and the budget is divided before the first read.
    fn assembled(source: Arc<dyn ByteRangeSource>, layout: xz_seek::Layout) -> Self {
        let table = layout.index_shared();
        let blocks = BlockCache::for_table(&table).map(Arc::new);
        // The largest window a decode here will hold, beside the decoder the
        // crate charges over a source that lends. `total_size()` rather than
        // `unpadded_size` because that is what `compressed_range()` asks for:
        // a window cut shorter would stop before the block's check.
        let widest_window = table.blocks.iter().map(|block| block.total_size()).max().unwrap_or(0);
        let decode_bytes = layout.decoder_bytes().saturating_add(widest_window);
        let source = Self {
            source,
            layout,
            table,
            pool: Arc::new(BufferPool::default()),
            blocks,
            decode_bytes,
            budget: AtomicUsize::new(DEFAULT_MEMORY_BUDGET as usize),
            jobs: AtomicUsize::new(1),
        };
        source.apportion();
        source
    }

    /// Divide the stated budget between the two pools, chunks first —
    /// [`XzSource::apportion`]'s rule, and the same one for the same reason.
    fn apportion(&self) {
        let budget = self.budget.load(Ordering::Relaxed);
        let jobs = self.jobs.load(Ordering::Relaxed).max(1);
        self.pool.set_limits(budget, POOL_DEPTH);
        if let Some(blocks) = &self.blocks {
            blocks
                .pool
                .set_limits(budget.saturating_sub(self.pool.held_bytes()), POOL_DEPTH.max(jobs));
        }
    }

    /// The chunk slot this source's charges are stated against
    /// ([`XzSource::charged_chunk_bytes`]).
    fn charged_chunk_bytes(&self) -> u64 {
        match self.pool.announced_bytes() {
            Some(len) => len as u64,
            None => SCAN_CHUNK_DEFAULT_SIZE_BYTES as u64,
        }
    }

    /// What concurrent block-decoding readers of this file would cost, whether
    /// or not the budget admits one — the one composition site, exactly as
    /// [`XzSource::block_worker_memory`] is.
    fn block_worker_memory(&self) -> Option<WorkerMemory> {
        let cache = self.blocks.as_ref()?;
        Some(cache.worker_memory(self.charged_chunk_bytes(), self.decode_bytes))
    }

    /// The whole-block arm, or `None` where a read is served in pieces: no
    /// blocks at all, or a budget that cannot hold one decoded
    /// ([`BlockCache::affordable`]).
    ///
    /// **The piecewise arm is not free of the budget either** — it holds the
    /// compressed window, the decoder and the caller's chunk, which is
    /// [`BlockCache::reader_bytes`] less the block unit. So what declining
    /// saves is the plaintext, and a budget below even that remainder is
    /// exceeded exactly as the local streaming fallback's is.
    fn block_path(&self) -> Option<&Arc<BlockCache>> {
        let budget = self.budget.load(Ordering::Relaxed) as u64;
        let chunk_bytes = self.charged_chunk_bytes();
        self.blocks
            .as_ref()
            .filter(|cache| cache.affordable(chunk_bytes, self.decode_bytes, budget))
    }

    /// One block's whole compressed extent, fetched and wrapped in the window
    /// a decode reads through.
    ///
    /// The window refuses every byte outside that range, so a mis-sized fetch
    /// is an error rather than a byte served silently from somewhere else.
    async fn window(&self, task: &xz_seek::BlockTask) -> Result<xz_seek::Window<Bytes>> {
        let extent = task.compressed_range();
        let len = usize::try_from(extent.end - extent.start).map_err(|_| xz_short_read())?;
        let bytes = self.source.read_range(extent.start, len).await?;
        Ok(xz_seek::Window::new(extent.start, self.table.compressed_file_size, bytes))
    }

    /// The task for block `index`, or the read-past-the-end error.
    ///
    /// **No lock is taken.** An `xz_seek::Layout` answers this from the table
    /// alone, where a `Reader` has to be borrowed mutably for it — which is
    /// the mutex [`XzSource::block`] holds for a lookup and no I/O.
    fn task(&self, index: usize) -> Result<xz_seek::BlockTask> {
        self.layout.block_task(index).ok_or_else(xz_short_read)
    }

    /// Block `index`, from the retention list or fetched and decoded whole
    /// into a slot of the block pool.
    ///
    /// Deficiency register: two concurrent misses on one block fetch and
    /// decode it twice, exactly as `KD20` describes for the local source; the
    /// marker sits there, this being the same shape and not a second one.
    async fn block(&self, index: usize, cache: &Arc<BlockCache>) -> Result<Arc<DecodedBlock>> {
        if let Some(hit) = cache.lookup(index) {
            return Ok(hit);
        }
        let task = self.task(index)?;
        let len = usize::try_from(task.uncompressed_len()).map_err(|_| xz_short_read())?;
        let window = self.window(&task).await?;
        // A slot is the file's largest block, so `len` is never above it.
        let slot = cache.slot();
        let slot = tokio::task::spawn_blocking(move || -> Result<PooledBuffer> {
            let mut slot = slot;
            task.decode_into(&window, &mut slot.as_mut()[..len])?;
            Ok(slot)
        })
        .await
        .map_err(Error::from)??;
        let block = Arc::new(DecodedBlock { slot, len });
        cache.retain(index, Arc::clone(&block));
        Ok(block)
    }

    /// A read served by decoding whole blocks — [`XzSource::read_by_blocks`]'s
    /// arithmetic over fetched windows.
    async fn read_by_blocks(
        &self,
        offset: u64,
        len: usize,
        cache: &Arc<BlockCache>,
    ) -> Result<Bytes> {
        let end = offset.checked_add(len as u64).ok_or_else(xz_short_read)?;
        let covering = self.table.blocks_in(offset..end);
        if covering.is_empty() {
            return Err(xz_short_read());
        }
        if covering.len() == 1 {
            let index = covering.start;
            let block = self.block(index, cache).await?;
            let base = self.table.blocks[index].uncompressed_offset;
            let start = offset
                .checked_sub(base)
                .and_then(|at| usize::try_from(at).ok())
                .ok_or_else(xz_short_read)?;
            if start + len > block.len {
                return Err(xz_short_read());
            }
            return Ok(Bytes::from_owner(BlockView(block)).slice(start..start + len));
        }
        let mut out = self.pool.obtain(len);
        let mut covered = 0usize;
        for index in covering {
            let block = self.block(index, cache).await?;
            let base = self.table.blocks[index].uncompressed_offset;
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
            return Err(xz_short_read());
        }
        Ok(Bytes::from_owner(out).slice(..len))
    }

    /// A read served **without holding a whole decoded block**: each covering
    /// block is fetched, skipped forward to the first wanted byte, read into
    /// the caller's buffer and completed.
    ///
    /// This is the arm the local source answers with `xz_seek::Reader::read_at`
    /// and a fetched source cannot: that path pulls, and pulling is what has no
    /// transport here (D20). What it holds is the compressed window, the
    /// decoder and one chunk buffer — never the block's plaintext, which is the
    /// point.
    async fn read_in_pieces(&self, offset: u64, len: usize) -> Result<Bytes> {
        let end = offset.checked_add(len as u64).ok_or_else(xz_short_read)?;
        let covering = self.table.blocks_in(offset..end);
        if covering.is_empty() {
            return Err(xz_short_read());
        }
        let mut out = self.pool.obtain(len);
        let mut covered = 0usize;
        for index in covering {
            let task = self.task(index)?;
            let block = task.uncompressed_range();
            let from = block.start.max(offset);
            let to = block.end.min(end);
            if to <= from {
                continue;
            }
            let at = (from - offset) as usize;
            let n = (to - from) as usize;
            let window = self.window(&task).await?;
            let (returned, filled) =
                tokio::task::spawn_blocking(move || -> Result<(PooledBuffer, usize)> {
                    let mut out = out;
                    let mut handle = task.begin(&window)?;
                    let mut skip = [0u8; FETCHED_SKIP_CHUNK];
                    handle.advance_to(&window, from, &mut skip)?;
                    let mut filled = 0usize;
                    while filled < n {
                        let got = handle.read(&window, &mut out.as_mut()[at + filled..at + n])?;
                        if got == 0 {
                            break;
                        }
                        filled += got;
                    }
                    // The check covers the whole block and is compared here,
                    // over a block this reader never held whole. It is not a
                    // knob: skipping it would make verification silently
                    // weaker than the path it replaces.
                    handle.complete(&window)?;
                    Ok((out, filled))
                })
                .await
                .map_err(Error::from)??;
            out = returned;
            covered += filled;
        }
        if covered != len {
            return Err(xz_short_read());
        }
        Ok(Bytes::from_owner(out).slice(..len))
    }
}

impl std::fmt::Debug for FetchedXzSource {
    /// The transport behind it is `dyn`, so what is printable is this file's
    /// shape and what it may hold.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchedXzSource")
            .field("streams", &self.table.stream_count())
            .field("blocks", &self.table.block_count())
            .field("decode_bytes", &self.decode_bytes)
            .finish_non_exhaustive()
    }
}

impl ByteRangeSource for FetchedXzSource {
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
            match self.block_path().cloned() {
                Some(cache) => self.read_by_blocks(offset, len, &cache).await,
                None => self.read_in_pieces(offset, len).await,
            }
        })
    }

    /// The uncompressed length, from the seek table — no decode, no fetch.
    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        Box::pin(async move { Ok(self.table.uncompressed_size()) })
    }

    /// The transport's, unchanged: the compression layer moves no file and
    /// changes no identity (`docs/design/decisions.md`, "D21").
    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        self.source.modified()
    }

    /// The compressed object's own stored length — the transport's `size`,
    /// never the stream-index total [`FetchedXzSource::size`] answers.
    fn stored_size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        self.source.stored_size()
    }

    fn hint_read_size(&self, len: usize) {
        self.pool.hint(len);
        self.apportion();
    }

    fn hint_parallelism(&self, parallelism: Parallelism) {
        self.budget.store(budget_bytes(parallelism), Ordering::Relaxed);
        self.jobs.store(parallelism.jobs(), Ordering::Relaxed);
        self.apportion();
    }

    /// **Ignored: this source never takes a wait**, and the reason is where its
    /// chunk buffer is obtained rather than what the loop promises.
    ///
    /// Every other source takes its buffer inside a `spawn_blocking` closure,
    /// so a `WaitPolicy::MayWait` holder blocks a blocking-pool thread while a
    /// sibling releases. Here the fetch has to `await`, so the buffer is
    /// obtained on the runtime's own task — and `BufferPool::obtain` waits on a
    /// `Condvar`, which on the `current_thread` runtime this project dispatches
    /// from (`docs/design/decisions.md`, "D12") would block the very thread the
    /// releasing sibling needs. A wait that should not have been permitted is a
    /// hang with nothing to measure ([`WaitPolicy::NeverWait`]), so it is
    /// refused here rather than granted and hoped for.
    ///
    /// The cost is the direction that constant is already safe in: the pool
    /// allocates past its budget rather than blocking, which is memory instead
    /// of a deadlock. Making the wait available again means obtaining the
    /// buffer inside the decode's closure, which is a rearrangement this
    /// phase's "correctness only" scope does not buy anything by.
    fn hint_wait_policy(&self, _policy: WaitPolicy) {}

    /// Announced to the transport rather than kept here: what a cancellation
    /// interrupts is the fetch, and a decode between two fetches is bounded by
    /// one block.
    fn hint_cancellation(&self, cancel: Arc<Cancellation>) {
        self.source.hint_cancellation(cancel);
    }

    /// The transport's, unchanged — the object's identity is the object's,
    /// whatever is compressed inside it.
    fn remote_identity(&self) -> Option<RemoteIdentity> {
        self.source.remote_identity()
    }

    /// Announced to the transport, which is what carries the precondition on
    /// every fetch — so every window a walk or a block decode asks for is
    /// pinned to the version this run opened on, without either knowing
    /// (`docs/design/roadmap-P14-remote-input.md`, "D10", "D11").
    fn hint_in_flight_identity(&self, binds: bool) {
        self.source.hint_in_flight_identity(binds);
    }

    fn seek_table(&self) -> Option<xz_seek::SeekTable> {
        Some((*self.table).clone())
    }

    /// [`xz_partition_advice`], as the local source's is: the cut points are a
    /// property of the file rather than of the transport, and reading them off
    /// the arm actually taken is what keeps
    /// `crate::stream::compressed_block_path_declined` honest here.
    fn partitions(&self, range: Range<u64>) -> Partitioning {
        xz_partition_advice(
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

    /// **One**, where the local twin recommends `min(cores, block_count)` over
    /// the same bytes ([`XzSource::default_workers`]) — this is the one source
    /// whose recommendation differs from its local twin's, and the file's
    /// block count says what may be *cut*, not what a network should be asked
    /// for at once.
    ///
    /// **The reason is not that this phase defers network tuning**, though it
    /// does (`docs/design/roadmap-P14-remote-input.md`, "D7"): it is that the
    /// two errors are not symmetric. Recommending too few costs throughput on
    /// a link, and the caller types `--jobs` to take it back. Recommending too
    /// many opens `min(cores, block_count)` concurrent connections to a *third
    /// party's* server for a command that carries no flag saying so. So a
    /// later reading showing the link underused does not raise this number —
    /// it argues for raising `--jobs`, which is the caller's to type.
    fn default_workers(&self) -> usize {
        1
    }

    /// What a reader of *this file* holds, which the transport does not change
    /// — the same shape [`XzSource::default_worker_memory`] answers, with the
    /// compressed window folded into the decoder term. It is a recommendation
    /// about memory rather than about concurrency, so D7's silence on the
    /// plain remote source — which has no block structure to charge for — does
    /// not reach it.
    fn default_worker_memory(&self) -> Option<WorkerMemory> {
        self.block_worker_memory()
    }
}

/// The six bytes every `.xz` stream opens with
/// (`docs/design/decisions.md`, "D14").
const XZ_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];

/// How many leading bytes an [`Origin`] probe brings back: the longest magic
/// recognition compares, which is [`XZ_MAGIC`]'s. A source whose whole
/// content is shorter brings back fewer, which is not an error — it simply
/// matches no magic.
const ORIGIN_LEADING_BYTES: usize = XZ_MAGIC.len();

/// Where a dump is, as a caller's `--source` argument resolved it, together
/// with the one cheap probe of it that everything before the source reads
/// from.
///
/// **It exists because two things are settled before a source is built, and
/// both were reading a `&Path`.** A cache recorded against another stored
/// size is refused before a byte of the dump is read
/// (`docs/design/decisions.md`, "D20"), and recognition has to know what the
/// file's first bytes are in order to choose which source to build
/// (`docs/design/decisions.md`, "D14"). Doing either through the filesystem
/// directly makes both of them statements about local files, which is the
/// assumption a source reached over the network does not meet.
///
/// So the origin answers exactly what is needed that early — **stored size,
/// weak identity, and the leading magic bytes** — and nothing else. It never
/// opens a source, never walks an `.xz` file's stream footers, and never
/// reads past the longest magic recognition compares.
///
/// **The probe runs at most once.** Locally that saves nothing worth naming;
/// the reason it is cached is that a source whose probe is a round trip must
/// answer all three from one. [`Origin::probe`] is therefore the only way to
/// an answer, and a caller reads whichever of them it needs.
///
/// A probe that fails is **not** cached: the failure belongs to whoever is in
/// a position to say something about it, which is the open that follows
/// (see [`crate::cache::claim`]).
#[derive(Debug)]
pub struct Origin {
    location: Location,
    probed: tokio::sync::OnceCell<OriginProbe>,
}

/// The kinds of place a dump can be, and private: what a caller reads is
/// [`OriginProbe`], which is the same three answers whichever kind produced
/// them.
#[derive(Debug)]
enum Location {
    LocalFile(PathBuf),
    /// An object a server answers ranged GETs for. It carries the client as
    /// well as the URL, so the probe's round trip and every later read share
    /// one connection pool and one configuration.
    #[cfg(feature = "http")]
    Remote(Arc<RemoteObject>),
}

/// What one probe of an [`Origin`] answered.
///
/// The weak identity is the modification time, which is the local source's —
/// the half of identity a mismatch in only warns about
/// (`docs/design/decisions.md`, "D21"). The stored size is the half that
/// refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginProbe {
    stored_size: u64,
    modified: Option<SystemTime>,
    leading: Vec<u8>,
}

impl OriginProbe {
    /// Bytes as stored, the number `cache::claim` compares against what a
    /// cache recorded — [`ByteRangeSource::stored_size`] without a source.
    pub fn stored_size(&self) -> u64 {
        self.stored_size
    }

    /// The weak identity, `None` where the origin exposes none.
    pub fn modified(&self) -> Option<SystemTime> {
        self.modified
    }

    /// The source's first bytes, as far as the longest magic recognition
    /// compares — fewer for a source shorter than that.
    pub fn leading(&self) -> &[u8] {
        &self.leading
    }
}

impl Origin {
    /// The dump at `path` on this filesystem.
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self { location: Location::LocalFile(path.into()), probed: tokio::sync::OnceCell::new() }
    }

    /// The dump an HTTP server holds at `url`, read over ranged GETs.
    ///
    /// **This is where "HTTP only" is enforced rather than merely stated**
    /// (`docs/design/roadmap-P14-remote-input.md`, "D3"): the scheme must be
    /// `http` or `https`, and a URL carrying a username or password is refused
    /// by name rather than silently stripped, so nobody believes a credential
    /// was sent (D17). A presigned URL needs none — the signature rides in the
    /// query string, which this preserves.
    ///
    /// The client is built here and reading nothing: the first round trip is
    /// [`Origin::probe`]'s.
    #[cfg(feature = "http")]
    pub fn remote(url: &url::Url) -> Result<Self> {
        Self::remote_with_read_timeout(url, REMOTE_READ_TIMEOUT)
    }

    /// [`Origin::remote`] with the liveness deadline stated rather than taken
    /// from [`REMOTE_READ_TIMEOUT`].
    ///
    /// **A knob the library offers and the CLI does not**: what a person at a
    /// terminal must state is governed, and an embedder on a link this project
    /// has never seen is not that person
    /// (`docs/design/roadmap-P14-remote-input.md`, "D9"). The stalled-origin
    /// assertion is its first user rather than its reason.
    #[cfg(feature = "http")]
    pub fn remote_with_read_timeout(url: &url::Url, read_timeout: Duration) -> Result<Self> {
        let refuse = |why: &str| Error::SourceNotReadable {
            origin: without_credentials(url),
            why: why.to_string(),
        };
        if !matches!(url.scheme(), "http" | "https") {
            return Err(refuse("only `http:` and `https:` URLs are fetched over the network"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(refuse(
                "it carries a username or password, and this build sends no credentials — use a \
                 presigned URL, whose signature rides in the query string",
            ));
        }
        if remote_name_of(url).is_none() {
            return Err(refuse(
                "it names no object — a bare host, or a path ending in `/`, is a place rather \
                 than a dump, and there is nothing there to read or to name a cache after",
            ));
        }
        let object = RemoteObject::open(url.clone(), read_timeout)?;
        Ok(Self {
            location: Location::Remote(Arc::new(object)),
            probed: tokio::sync::OnceCell::new(),
        })
    }

    /// What a `--source` argument names: **a URL first and a path second**
    /// (`docs/design/roadmap-P14-remote-input.md`, "D3").
    ///
    /// `http` and `https` select the network; `file:` is a local path, because
    /// a user who has seen `--source https://…` work may reasonably conclude
    /// that a local file now needs `file://`, and being right about how the
    /// tool works should not be a way to get an error (D17). An empty or
    /// `localhost` host is this filesystem; any other host is refused by name,
    /// this build speaking no network file protocol. Every other scheme is
    /// refused by name, which is what a mistyped one deserves.
    ///
    /// **Anything that is not a URL is a path**, exactly as before this phase.
    #[cfg(feature = "http")]
    pub fn resolve(argument: &str) -> Result<Self> {
        let refuse = |why: String| Error::SourceNotReadable { origin: argument.to_string(), why };
        let Ok(url) = url::Url::parse(argument) else {
            return Ok(Self::local(argument));
        };
        match url.scheme() {
            "http" | "https" => Self::remote(&url),
            // `to_file_path` refuses two things, and they are different
            // mistakes: a host naming another machine, and a path that is not
            // absolute.
            "file" => url.to_file_path().map(Self::local).map_err(|()| {
                refuse(match url.host_str() {
                    Some(host) if host != "localhost" => format!(
                        "a `file:` URL names a path on this machine, so its host must be empty \
                         or `localhost`, not `{host}`"
                    ),
                    _ => "a `file:` URL names an absolute path, so its path must begin with `/`"
                        .to_string(),
                })
            }),
            scheme => Err(refuse(format!(
                "the `{scheme}:` scheme is not one this build reads — `http:`, `https:` and \
                 `file:` are, and a path whose first segment holds a colon is \
                 written `./{argument}`"
            ))),
        }
    }

    /// The path this origin names on this filesystem, or `None` for one that
    /// is not a file — what a caller deriving a name *beside* the dump needs,
    /// there being no such place for an object fetched over the network.
    pub fn local_path(&self) -> Option<&Path> {
        match &self.location {
            Location::LocalFile(path) => Some(path),
            #[cfg(feature = "http")]
            Location::Remote(_) => None,
        }
    }

    /// What the object this origin names is called, for a caller deriving a
    /// file name from it: a URL's **last path segment**, exactly as written
    /// there — the working-directory cache name a remote dump gets, there
    /// being no place beside it for one to sit
    /// (`docs/design/roadmap-P14-remote-input.md`, "D4").
    ///
    /// `None` for a local file, whose caller has [`Origin::local_path`] and
    /// the colocated rule. A remote origin always has one: a URL that names
    /// no object is refused when the origin is built.
    pub fn remote_name(&self) -> Option<&str> {
        match &self.location {
            Location::LocalFile(_) => None,
            #[cfg(feature = "http")]
            Location::Remote(object) => remote_name_of(&object.url),
        }
    }

    /// Probe this origin, or hand back the answer a previous call got.
    pub async fn probe(&self) -> Result<&OriginProbe> {
        self.probed
            .get_or_try_init(|| async {
                match &self.location {
                    Location::LocalFile(path) => probe_local_file(path).await,
                    #[cfg(feature = "http")]
                    Location::Remote(object) => object.probe().await,
                }
            })
            .await
    }
}

impl std::fmt::Display for Origin {
    /// How a message to the user names this source.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.location {
            Location::LocalFile(path) => write!(f, "{}", path.display()),
            #[cfg(feature = "http")]
            Location::Remote(object) => write!(f, "{}", object.url),
        }
    }
}

/// One `stat` and one short read, on a blocking thread as every other local
/// read in this module is.
async fn probe_local_file(path: &Path) -> Result<OriginProbe> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<OriginProbe> {
        use std::io::Read;
        let mut file = std::fs::File::open(&path)?;
        let metadata = file.metadata()?;
        let mut buf = [0u8; ORIGIN_LEADING_BYTES];
        let mut filled = 0;
        while filled < buf.len() {
            match file.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::Io(e)),
            }
        }
        Ok(OriginProbe {
            stored_size: metadata.len(),
            modified: Some(metadata.modified()?),
            leading: buf[..filled].to_vec(),
        })
    })
    .await
    .map_err(Error::from)?
}

/// What a caller already knows about a file's compression layer before
/// [`open_local`] has looked at it — normally read out of a cache written from
/// that same file (`crate::cache::claim`), and the reason an `.xz` source need
/// not re-walk its stream footers (`docs/design/decisions.md`, "D18").
///
/// **A bare [`xz_seek::SeekTable`], not a cache.** What loads it is the
/// caller's business, which keeps `io.rs` naming nothing in `crate::cache`.
///
/// Three states, not an `Option`: "the cache says this file is plain" is a
/// claim recognition can *contradict* (`docs/design/decisions.md`, "D18").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum KnownCompression {
    /// Nothing is known — recognition reads the file's own bytes and, for
    /// `.xz`, pays the walk. The only state that can never produce a
    /// [`Recognized::Mismatch`].
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
    /// the cache it came from as unusable
    /// (`docs/design/decisions.md`, "D18").
    ///
    /// The claim it contradicts and the cache's span index were written by
    /// one `save` from one file, so this condemns that index too: a caller
    /// that goes on to scan must not also resume from it.
    Mismatch,
}

/// Open whatever `origin` names, choosing the source kind by where the dump
/// is and its compression layer by what its first bytes are.
///
/// **The entry point above [`open_local`] and [`open_remote`]**, and the one a
/// caller with an origin in hand wants: the two below it each answer for one
/// kind of place and refuse the other, so dispatching here is what keeps a
/// caller from having to know which it has. Every argument and every answer is
/// the same whichever it dispatches to.
pub async fn open(origin: &Origin, known: KnownCompression) -> Result<Recognized> {
    match &origin.location {
        Location::LocalFile(_) => open_local(origin, known).await,
        #[cfg(feature = "http")]
        Location::Remote(_) => open_remote(origin, known).await,
    }
}

/// Open `origin` as a [`ByteRangeSource`], choosing between
/// [`LocalFileSource`] and [`XzSource`] by **content**, not by name
/// (`docs/design/decisions.md`, "D14"): the first six bytes are checked
/// against `.xz`'s magic, whatever the file is called. A file that really is
/// `.xz`-compressed is recognised however it is named or extensionless; a file
/// merely *named* `.xz` whose bytes don't match opens as plain.
///
/// **The magic comes from [`Origin::probe`], not from a read of its own.**
/// Recognition is told what the source holds, which is what keeps it from
/// being a statement about files; by the time a caller gets here the probe
/// has normally already run, for the cache claim.
///
/// `known` is what a caller read out of a cache for this same file, and it is
/// checked rather than believed: recognition still compares the magic, and a
/// claim the file contradicts is [`Recognized::Mismatch`] rather than a silent
/// fallback. [`KnownCompression::Unknown`] is the no-knowledge case and always
/// yields a source.
pub async fn open_local(origin: &Origin, known: KnownCompression) -> Result<Recognized> {
    // The kind is settled before the probe, so an origin this cannot open is
    // refused without a round trip being spent on it. The match has a second
    // arm only where the remote source is compiled in; without it there is one
    // kind of place and the refusal is unreachable.
    #[allow(clippy::infallible_destructuring_match)]
    let path = match &origin.location {
        Location::LocalFile(path) => path,
        #[cfg(feature = "http")]
        Location::Remote(_) => {
            return Err(Error::SourceNotReadable {
                origin: origin.to_string(),
                why: "it is not a file on this filesystem".to_string(),
            });
        }
    };
    let is_xz = origin.probe().await?.leading().starts_with(&XZ_MAGIC);
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

// ---------------------------------------------------------------------------
// The remote source
//
// One `ByteRangeSource` over `object_store`'s HTTP backend, and what an
// `Origin` needs in order to name one. Everything below is behind the `http`
// feature (`docs/design/roadmap-P14-remote-input.md`, "D6").
// ---------------------------------------------------------------------------

/// How long a remote request may go without delivering a byte before it is
/// abandoned.
///
/// **It replaces `object_store`'s *total* request timeout**, which limits link
/// speed rather than liveness: that one counts the response body, so a large
/// ranged GET over a slow-but-working link fails while still progressing. A read timeout resets on every byte that arrives, so a slow
/// transfer completes and only a dead connection fails
/// (`docs/design/roadmap-P14-remote-input.md`, "D9").
///
/// **Unmeasured, and chosen rather than tuned**: a link that has delivered
/// nothing for this long is not a slow link. The phase that tunes the network
/// is where a reading would replace it.
#[cfg(feature = "http")]
pub const REMOTE_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// A URL and the client that reads it — an [`Origin`]'s remote half.
///
/// **The client is built once, here.** The probe and every later read go
/// through this one store, so they share a connection pool and cannot
/// disagree about timeouts or retries.
#[cfg(feature = "http")]
#[derive(Debug)]
struct RemoteObject {
    url: url::Url,
    store: object_store::http::HttpStore,
    /// What the probe's response said the object was: kept whole rather than
    /// projected, because the precondition every later request carries is
    /// built from the validators **as the server stated them**
    /// (`docs/design/roadmap-P14-remote-input.md`, "D11"), and a date
    /// round-tripped through a [`SystemTime`] is a second spelling of one of
    /// them. Set by [`RemoteObject::probe`], which [`Origin::probe`] runs at
    /// most once.
    meta: std::sync::OnceLock<object_store::ObjectMeta>,
}

#[cfg(feature = "http")]
impl RemoteObject {
    /// Build the client for `url`, whose scheme [`Origin::remote`] has already
    /// accepted.
    fn open(url: url::Url, read_timeout: Duration) -> Result<Self> {
        // Four client settings, three of them the crate's own
        // (`docs/design/roadmap-P14-remote-input.md`, "D9"): the retry
        // configuration and `http1_only` are left alone, `allow_http` follows
        // the scheme the user typed — which is their statement that plaintext
        // is acceptable — and only the timeout is ours.
        let options = object_store::ClientOptions::new()
            .with_timeout_disabled()
            .with_read_timeout(read_timeout)
            .with_allow_http(url.scheme() == "http");
        let store = object_store::http::HttpBuilder::new()
            .with_url(url.as_str())
            .with_client_options(options)
            .build()
            .map_err(|e| remote_failure(&url, &e))?;
        Ok(Self { url, store, meta: std::sync::OnceLock::new() })
    }

    /// The identity precondition a ranged GET carries, so that an object
    /// rewritten mid-scan comes back as a refusal instead of as bytes from
    /// two versions of the file mixed together
    /// (`docs/design/roadmap-P14-remote-input.md`, "D10").
    ///
    /// **The server does the comparing**, which is why the cadence is every
    /// request here where it is every cache save locally: the check rides on
    /// a request the read was already making (D11).
    ///
    /// The entity tag is preferred, and `If-Unmodified-Since` is the fallback
    /// for a server that sends none. A `Last-Modified` this build reads as
    /// absence is not sent — `object_store` substitutes the epoch for a
    /// response that carries none (`docs/design/runtime-invariants.md`,
    /// "RT16"), and asking a server to confirm that nothing has changed since
    /// 1970 refuses every read.
    fn precondition(&self) -> object_store::GetOptions {
        let mut options = object_store::GetOptions::default();
        let Some(meta) = self.meta.get() else { return options };
        match &meta.e_tag {
            Some(tag) => options.if_match = Some(tag.clone()),
            None => {
                let stated = weak_identity(
                    meta.last_modified.timestamp(),
                    meta.last_modified.timestamp_subsec_nanos(),
                );
                if stated.is_some() {
                    options.if_unmodified_since = Some(meta.last_modified);
                }
            }
        }
        options
    }

    /// The object a request addresses: the store's own base URL with nothing
    /// appended, since the store was built from the whole URL. That is the
    /// form that **preserves a query string**, which is what lets a presigned
    /// URL work with no credential handling of our own
    /// (`docs/design/roadmap-P14-remote-input.md`, "D17").
    fn object(&self) -> object_store::path::Path {
        object_store::path::Path::default()
    }

    /// One ranged GET of the leading bytes, answering all three of the probe's
    /// questions: a 206 carries the object's **total** size in `Content-Range`
    /// and the response carries the validators beside it
    /// (`docs/design/runtime-invariants.md`, "RT13"). So the probe is one
    /// round trip, not a `HEAD` and a `GET`.
    async fn probe(&self) -> Result<OriginProbe> {
        let options = object_store::GetOptions {
            range: Some(object_store::GetRange::Bounded(0..ORIGIN_LEADING_BYTES as u64)),
            ..Default::default()
        };
        let got = self
            .store
            .get_opts(&self.object(), options)
            .await
            .map_err(|e| remote_failure(&self.url, &e))?;
        let stored_size = got.meta.size;
        let modified = weak_identity(
            got.meta.last_modified.timestamp(),
            got.meta.last_modified.timestamp_subsec_nanos(),
        );
        // Kept before the body is drained, so a probe that fails to read its
        // own bytes leaves no validators for a later request to pin against.
        let meta = got.meta.clone();
        let leading = got.bytes().await.map_err(|e| remote_failure(&self.url, &e))?;
        let _ = self.meta.set(meta);
        Ok(OriginProbe { stored_size, modified, leading: leading.to_vec() })
    }
}

/// What a server's `Last-Modified` is worth as a weak identity: the time it
/// sent, or `None` where it sent none.
///
/// **The Unix epoch reads as absence.** `object_store` substitutes the epoch
/// where a response carries no `Last-Modified`, so "the server said nothing"
/// and "the server said 1970-01-01" arrive identically
/// (`docs/design/runtime-invariants.md`, "RT16"); no dump was modified in
/// 1970, so reading it as silence is the honest half of that pair. A time
/// *before* the epoch reads as silence too, for the same reason and with no
/// second spelling to distinguish.
#[cfg(feature = "http")]
fn weak_identity(seconds: i64, nanos: u32) -> Option<SystemTime> {
    if seconds == 0 && nanos == 0 {
        return None;
    }
    u64::try_from(seconds).ok().map(|s| UNIX_EPOCH + Duration::new(s, nanos))
}

/// A URL's last path segment, or `None` where it has none — a bare host, or a
/// path ending in `/`. Left exactly as the URL spells it, percent-escapes
/// included: the point of the derived cache name is that a user can predict it
/// by reading the URL (`docs/design/roadmap-P14-remote-input.md`, "D4"), and a
/// decoded segment can hold a path separator where the written one cannot.
#[cfg(feature = "http")]
fn remote_name_of(url: &url::Url) -> Option<&str> {
    url.path_segments()?.next_back().filter(|segment| !segment.is_empty())
}

/// A URL as a message may print it: with any username and password taken out.
///
/// The refusal of a credential-carrying URL is the one place a password could
/// reach a terminal or a redirected log, and echoing one back at a user who
/// has just been told it will not be sent is the wrong way to say it.
#[cfg(feature = "http")]
fn without_credentials(url: &url::Url) -> String {
    let mut shown = url.clone();
    // Both setters fail only on a URL that cannot have a host, which one with
    // an `http` scheme always can. A failure leaves the field as it was, so
    // the fallback below is the redaction failing safe rather than silently.
    if shown.set_username("").is_err() || shown.set_password(None).is_err() {
        return format!("{}://{}", url.scheme(), url.host_str().unwrap_or("(no host)"));
    }
    shown.to_string()
}

/// `object_store`'s wording, named against the URL it is about rather than
/// surfaced raw (`docs/design/roadmap-P14-remote-input.md`, "D15").
#[cfg(feature = "http")]
fn remote_failure(url: &url::Url, error: &object_store::Error) -> Error {
    Error::Remote { url: url.to_string(), message: error.to_string() }
}

/// A dump an HTTP server answers ranged GETs for.
///
/// **Almost every advisory trait member is left at its default, and that is
/// the decision rather than an omission**
/// (`docs/design/roadmap-P14-remote-input.md`, "D7"): one worker, no
/// partitioning advice, no memory recommendation, no read-size hint — this
/// source recycles no buffer, so there is nothing for one to size — and an
/// exact size, which is also the stored size. Each is the conservative answer,
/// and the phase that tunes the network is where a reading would replace one.
///
/// **Size and weak identity come from the origin's probe**, so neither costs a
/// round trip and the identity a run opens on is the one the cache claim was
/// settled against.
#[cfg(feature = "http")]
#[derive(Debug)]
pub struct RemoteSource {
    object: Arc<RemoteObject>,
    size: u64,
    modified: Option<SystemTime>,
    /// What the server called this version of the object at the probe — the
    /// half of a remote cache's identity a file has no equivalent of
    /// ([`ByteRangeSource::remote_identity`]).
    etag: Option<String>,
    /// The caller's ask that reading stop, where one has been announced
    /// ([`ByteRangeSource::hint_cancellation`]). Behind a lock because a hint
    /// arrives through `&self`, as every other hint does.
    cancel: Mutex<Option<Arc<Cancellation>>>,
    /// Whether every ranged GET pins the object to the version the probe saw
    /// ([`ByteRangeSource::hint_in_flight_identity`]). **It starts bound**:
    /// the hint arrives once a run has opened its watch, and a read taken
    /// before then must already be pinned.
    precondition: AtomicBool,
}

#[cfg(feature = "http")]
impl RemoteSource {
    fn new(object: Arc<RemoteObject>, probe: &OriginProbe) -> Self {
        let etag = object.meta.get().and_then(|meta| meta.e_tag.clone());
        Self {
            object,
            size: probe.stored_size(),
            modified: probe.modified(),
            etag,
            cancel: Mutex::new(None),
            precondition: AtomicBool::new(true),
        }
    }

    /// The URL this source reads, which is how a message names it.
    pub fn url(&self) -> &url::Url {
        &self.object.url
    }

    /// What a failed request means, named against the URL it is about.
    ///
    /// **A refused precondition is the one failure that is not about the
    /// network**: the server has just said the object is no longer the one
    /// this run opened on, which is exactly what a local `fstat` finds and is
    /// reported in the same words, remedies included
    /// (`docs/design/roadmap-P14-remote-input.md`, "D10", "D12"). Everything
    /// else is [`Error::Remote`].
    fn failed(&self, error: &object_store::Error) -> Error {
        match error {
            object_store::Error::Precondition { .. } => Error::SourceChangedWhileRead {
                differences: "the server refused a read of the version this run opened on"
                    .to_string(),
            },
            other => remote_failure(&self.object.url, other),
        }
    }

    /// One ranged GET, and exactly `len` bytes back.
    ///
    /// **The request pins the object to the version the probe saw**, unless a
    /// caller has said the in-flight identity does not bind, so a rewrite
    /// mid-scan is a refusal on the first read after it rather than bytes from
    /// two files ([`RemoteObject::precondition`]).
    async fn get(&self, offset: u64, len: usize) -> Result<Bytes> {
        let mut options = if self.precondition.load(Ordering::Relaxed) {
            self.object.precondition()
        } else {
            object_store::GetOptions::default()
        };
        options.range = Some(object_store::GetRange::Bounded(offset..offset + len as u64));
        let got = self
            .object
            .store
            .get_opts(&self.object.object(), options)
            .await
            .map_err(|e| self.failed(&e))?;
        let bytes = got.bytes().await.map_err(|e| self.failed(&e))?;
        // A server answering a *prefix* of the range it was asked for is a
        // legal response and a different fault: `object_store` compares the
        // `Content-Range` it got against the one it asked for and refuses
        // before the body is read. What can reach here short is the end of the
        // object, which a caller reading past it hears about in the words a
        // local `read_exact_at` would use.
        if bytes.len() != len {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "{} answered {} byte(s) of the {len} asked for at offset {offset}",
                    self.object.url,
                    bytes.len()
                ),
            )));
        }
        Ok(bytes)
    }
}

#[cfg(feature = "http")]
impl ByteRangeSource for RemoteSource {
    /// **The request is raced against the cancellation rather than polled
    /// beside it.** A network read's wait is bounded by the retry schedule and
    /// not by one `pread`, so a reader that only polled would answer a Ctrl-C
    /// minutes late; losing the race drops the request where it stands
    /// (`docs/design/decisions.md`, "D26").
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        Box::pin(async move {
            if len == 0 {
                return Ok(Bytes::new());
            }
            let cancel = self.cancel.lock().unwrap().clone();
            match cancel {
                Some(cancel) => match cancel.until_cancelled(self.get(offset, len)).await {
                    Some(read) => read,
                    None => Err(Error::ScanCancelled { scanned_through: offset }),
                },
                None => self.get(offset, len).await,
            }
        })
    }

    /// The origin's probe answered this, so it costs no round trip and cannot
    /// disagree with the size the cache claim was settled against.
    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        Box::pin(async move { Ok(self.size) })
    }

    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        Box::pin(async move { Ok(self.modified) })
    }

    fn hint_cancellation(&self, cancel: Arc<Cancellation>) {
        *self.cancel.lock().unwrap() = Some(cancel);
    }

    /// The URL and the entity tag, which is what a remote cache records
    /// beyond the size and modification time every source answers.
    fn remote_identity(&self) -> Option<RemoteIdentity> {
        // The same string `Origin`'s `Display` prints, which is what a
        // refusal naming an origin has to match; a URL carrying a credential
        // never reaches here, `Origin::remote` having refused it.
        Some(RemoteIdentity::new(self.object.url.to_string(), self.etag.clone()))
    }

    /// **The only thing that stops pinning the object**, which is what
    /// `--strict-identity=none` reaches
    /// (`docs/design/roadmap-P14-remote-input.md`, "D5").
    fn hint_in_flight_identity(&self, binds: bool) {
        self.precondition.store(binds, Ordering::Relaxed);
    }
}

/// Open `origin`, which must name a remote object, as a [`ByteRangeSource`].
///
/// [`open_local`]'s sibling, making the same two choices: the compression
/// layer is read off the probe's magic bytes, and `known` is checked rather
/// than believed.
///
/// **A remote `.xz` is read exactly as a local one is**, through
/// [`FetchedXzSource`] over the plain source rather than through [`XzSource`],
/// which pulls from a file (`docs/design/roadmap-P14-remote-input.md`, "D1",
/// "D13"). Where the cache holds the seek table there is no walk at all; where
/// it does not, the walk is announced before it is paid for
/// ([`announce_remote_walk`]).
#[cfg(feature = "http")]
pub async fn open_remote(origin: &Origin, known: KnownCompression) -> Result<Recognized> {
    let Location::Remote(object) = &origin.location else {
        return Err(Error::SourceNotReadable {
            origin: origin.to_string(),
            why: "it is not a URL, so there is nothing to fetch".to_string(),
        });
    };
    let probe = origin.probe().await?;
    let is_xz = probe.leading().starts_with(&XZ_MAGIC);
    let fetched =
        || -> Arc<dyn ByteRangeSource> { Arc::new(RemoteSource::new(Arc::clone(object), probe)) };
    match (is_xz, known) {
        // The saving, and the reason the cold walk below is worth announcing
        // rather than refusing: a table from a previous walk of this object
        // costs no round trip at all (`docs/design/decisions.md`, "D18").
        (true, KnownCompression::Xz(table)) => {
            match FetchedXzSource::with_table(fetched(), probe.stored_size(), table) {
                Ok(source) => Ok(Recognized::Source(Arc::new(source))),
                Err(Error::Xz(xz_seek::Error::InvalidTable { .. })) => Ok(Recognized::Mismatch),
                Err(e) => Err(e),
            }
        }
        (true, KnownCompression::Unknown) => {
            let source = fetched();
            announce_remote_walk(origin);
            let (table, fetches) =
                walk_seek_table(&*source, probe.stored_size(), probe.leading()).await?;
            tracing::info!(
                source = %origin,
                streams = table.stream_count(),
                blocks = table.block_count(),
                fetches,
                "seek table build complete",
            );
            Ok(Recognized::Source(Arc::new(FetchedXzSource::with_table(
                source,
                probe.stored_size(),
                table,
            )?)))
        }
        // Both directions of "the cache describes a different file", exactly
        // as `open_local` reads them.
        (true, KnownCompression::Plain) | (false, KnownCompression::Xz(_)) => {
            Ok(Recognized::Mismatch)
        }
        (false, KnownCompression::Unknown | KnownCompression::Plain) => {
            Ok(Recognized::Source(fetched()))
        }
    }
}

/// Say, **before** it is paid for, what walking a fetched `.xz` file's stream
/// footers is about to cost, and what the two ways out of it are
/// (`docs/design/roadmap-P14-remote-input.md`, "D1").
///
/// **Not refused, announced.** A threshold above which a cold walk was
/// rejected would be a tuned number this phase has said it produces none of,
/// and it would deny a user a command that works; the precedent is exact —
/// `docs/design/decisions.md`, "D19" warns about a one-block `.xz` and refuses
/// nothing, because only the user can judge one decode-from-zero. The same
/// sentence holds with round trips in place of a decode.
///
/// **On the status channel rather than as a `Diagnostic`.** The division is
/// the one `crate::diagnostic`'s module doc draws: `tracing` carries cost and
/// progress as a run meets them, a `Diagnostic` a structured finding about the
/// file, drained from a result. This line is the first kind — a
/// `crate::Diagnostic` rides a `DumpIndex` or a `CacheStatus`, both built from
/// a source that already exists, which is *after* the walk, and the whole
/// value of this line is that it arrives before. It is also where the local
/// source's own "seek table build started" goes
/// (`docs/design/decisions.md`, "D64"), so both providers announce a walk
/// alike.
///
/// **What it does not reach is a caller draining diagnostics**, `pgdq info
/// --json` among them. This file's stream count is a property of the file, as
/// the block count behind
/// [`crate::diagnostic::DiagnosticKind::NonSeekableCompressedSource`] is, and
/// nothing on that channel carries it yet.
///
#[cfg(feature = "http")]
fn announce_remote_walk(origin: &Origin) {
    tracing::warn!(
        source = %origin,
        "seek table build started: a fetched `.xz` file's stream footers are walked one \
         request at a time, which is one round trip per stream — thousands, on a dump \
         compressed in many small streams. It is paid once: keep the cache this run \
         writes, or parse a local copy and read that.",
    );
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
    /// non-decompressing source is meant to answer.
    #[tokio::test]
    async fn stored_size_defaults_to_size() {
        let (_file, source) = source_of(b"0123456789abcdef");
        assert_eq!(source.stored_size().await.unwrap(), source.size().await.unwrap());
    }

    /// Every source answers its addressable length exactly; nothing
    /// overrides the default.
    #[test]
    fn size_is_exact_defaults_true() {
        let (_file, source) = source_of(b"0123456789abcdef");
        assert!(source.size_is_exact());
    }

    /// The pool's whole risk: a reused buffer still holding the previous
    /// read's bytes past the length of the current one. A shorter range read
    /// after a longer one must see the file's bytes and nothing else.
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
    /// `held_bytes()` is an upper bound — the number that divides one stated
    /// budget between a source's two pools (`XzSource::apportion`).
    ///
    /// The buffer it refuses is a whole plain partition against a pool whose
    /// slot is a chunk: a second read unit, which the plain path does not
    /// allocate, reading its piece [`PartitionRead::Chunked`] instead.
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
    /// is `BlockCache`'s retained list (`docs/design/decisions.md`, "D17").
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

    /// The library's own default is the serial path, on both option structs
    /// (`docs/design/decisions.md`, "D1").
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
    /// **The collapse takes the worker count down and not the budget**: a
    /// caller asking for one worker inside a stated budget has still stated
    /// it, and every pool sized from `memory_bytes` sees it
    /// (`docs/design/decisions.md`, "D1").
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

    /// **A one-worker budget reaches the pool it is a budget for**, driven
    /// through the announcement a serial read loop makes rather than the value
    /// alone: the stated budget sizes the pool, where the same source told
    /// nothing keeps `DEFAULT_MEMORY_BUDGET`'s depth.
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
    /// makes a decoded xz block a slot the pool can state a bound for:
    /// [`POOL_DEPTH`] slots of a large block would be the whole of a small
    /// cgroup, so the count falls rather than the budget rising.
    #[test]
    fn the_slot_count_falls_out_of_the_announced_unit() {
        let pool = BufferPool::default();
        // Nothing announced: the ceiling stands in for the unit.
        assert_eq!(pool.slot_bytes(), POOL_MAX_BYTES);
        assert_eq!(pool.slots(), POOL_DEPTH);

        // A block larger than the whole budget gets one slot rather than
        // none: a pool that refused to keep a block at all would hand back the
        // fresh `calloc` it exists to remove.
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
    /// with another holder. The one loop that grants this permission is the
    /// leader's fused worker, exercised against a real source by
    /// `crate::leader`'s scheduler tests.
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
    /// an earlier one's buffers are still out. And it is discharged whether or
    /// not the buffer is kept.
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
    /// wait, so a source nobody announces to allocates rather than blocking.
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
    /// what the *defaults* answer apart from any shipped source, `XzSource`
    /// overriding them and `LocalFileSource` free to.
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
    /// rather than overriding (`docs/design/decisions.md`, "D2").
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

    /// **The shipped configuration must be whole chunks, not capped ones.**
    /// `SCAN_CHUNK_DEFAULT_SIZE_BYTES * PLAIN_PARTITION_CHUNKS` is exactly
    /// [`POOL_MAX_BYTES`], on the cap, and the three constants are justified
    /// independently, so this assertion is what stops a later change to any
    /// one of them from silently converting the default from whole chunks to
    /// capped.
    #[test]
    fn a_shipped_plain_partition_is_eight_whole_chunks() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let source = LocalFileSource::open(file.path()).unwrap();
        source.hint_read_size(crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES);
        assert_eq!(
            source.partitions(0..16).partition_bytes(),
            (PLAIN_PARTITION_CHUNKS as u64) * (crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES as u64),
            "the shipped chunk size must yield a whole multiple, uncapped"
        );
    }

    /// A block-decoding `.xz` advises its block boundaries, so a partition is
    /// a whole number of blocks and no cut falls inside one.
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

        // A partition costs what one concurrent reader holds: one block slot
        // — the one being decoded, which is the one it then retains — the
        // chunk buffer a straddling read is assembled into, and the decoder's
        // own retention. The chunk term is the one a scan settles at, no read
        // loop having announced one here
        // ([`XzSource::charged_chunk_bytes`]).
        let unit = source.blocks.as_ref().unwrap().unit as u64;
        assert_eq!(
            whole.partition_bytes(),
            unit + crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES as u64 + source.decode_bytes
        );
        assert_eq!(whole.worker_memory().bytes_per_worker(), whole.partition_bytes());

        // **And the gate is that same charge at one reader**: the per-reader
        // term plus the `(POOL_DEPTH - 1)` units of retention list nobody else
        // is there to take ([`WorkerMemory`]).
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
    /// table has boundaries** (`docs/design/decisions.md`, "D15"). Asserted
    /// against a synthetic multi-block table, `partition_advice` being a free
    /// function precisely so that the fallback arm can be handed a table that
    /// *does* have boundaries.
    #[test]
    fn a_streaming_fallback_source_advises_one_partition() {
        let table = four_block_table();
        assert!(table.is_seekable(), "the table has boundaries to advise");

        let cache = BlockCache::for_table(&table).expect("4 KiB blocks decode whole");
        let decoding = xz_partition_advice(&table, Some(&cache), 1 << 20, 9 << 20, 0..4 * 4096);
        assert_eq!(decoding.max_partitions(), Some(4));
        assert_eq!(decoding.retained_unit(), RetainedUnit::Partition);
        assert_eq!(
            decoding.partition_bytes(),
            4096 + (1 << 20) + (9 << 20),
            "one block slot, the chunk buffer, and the decoder's own retention"
        );

        let streaming = xz_partition_advice(&table, None, 1 << 20, 9 << 20, 0..4 * 4096);
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
        // **The read shape is stated apart from both of those**: a
        // block-decoding piece is read whole and copies nothing, while a
        // streaming read is assembled into a chunk buffer whatever length is
        // asked for. The unit the block arm states is the table's own largest
        // block rather than the piece.
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
    /// vacuous on a file whose windows run out of boundaries first.
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
    /// The failure mode it guards is `partition_bytes` serving as both the
    /// memory charge and `run_region`'s cut size
    /// (`docs/design/decisions.md`, "D8").
    ///
    /// It walks the window loop rather than one window, a *mid-block* start
    /// being the case a single aligned window would never show.
    #[test]
    fn a_block_decoding_partition_spans_at_most_the_cut_width() {
        // Enough blocks that a window of `workers × BOUNDARIED_PARTITION_UNITS`
        // is satisfied out of the boundary list rather than running off the end
        // of the file, which is what makes the bound below bind.
        let blocks = 8 * BOUNDARIED_PARTITION_UNITS as u64 + 4;
        let table = block_table(blocks);
        let size = blocks * 4096;
        let cache = BlockCache::for_table(&table).expect("4 KiB blocks decode whole");
        let advice = xz_partition_advice(&table, Some(&cache), 1 << 20, 9 << 20, 0..size);
        // The charge is a block, a chunk and the decoder — orders of
        // magnitude above the 4 KiB unit here, so a window sized by it would
        // swallow the whole file.
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

    /// **Whole-block decode is declined unless the budget affords the one
    /// block a reader holds *and* the retention list beside it**
    /// (`docs/design/decisions.md`, "D16"). A reader's decode buffer and the
    /// block it then retains are the same buffer, and a single reader leaves
    /// `(POOL_DEPTH - 1)` units of list nothing else is there to fill, so
    /// [`POOL_DEPTH`] units is the line.
    #[test]
    fn the_block_path_wants_room_for_one_block_and_the_retention_list() {
        let table = four_block_table();
        let cache = BlockCache::for_table(&table).expect("4 KiB blocks have a unit");
        // The two terms a decline does not save, held at zero so that this
        // test is about the block slots alone; the test above is where they
        // bind.
        let unit = cache.unit as u64;
        let retained = POOL_DEPTH as u64 - 1;

        assert!(
            cache.affordable(0, 0, unit * (1 + retained)),
            "one unit for the reader, and the list the pool retains beside it"
        );
        cache.pool.set_limits(cache.unit * 2, POOL_DEPTH);
        assert_eq!(cache.pool.slots(), 2);

        assert!(
            !cache.affordable(0, 0, unit * (1 + retained) - 1),
            "a byte short of the list is a pool the budget never granted, so it is declined"
        );
        // One unit alone is the per-reader term, and the retained list is what
        // it does not carry.
        assert!(!cache.affordable(0, 0, unit));
        // And the reader's other two terms are inside the same number: the
        // same budget that afforded the blocks declines them once a chunk
        // buffer and a decoder are charged beside it.
        assert!(!cache.affordable(1, 1, unit * (1 + retained)));
    }

    /// **One slot count covers the retained blocks and the free ones
    /// together**, so a block pool's ceiling is its stated budget rather than
    /// twice it. Driven through the two calls the decode path makes — a slot,
    /// then a retention — with the most recent block held live.
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

    /// `xz` is not `mise`-pinned (`docs/design/decisions.md`, "D73"), so a
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
    /// into several blocks — a throwaway pattern for pinning `XzSource`'s own
    /// wiring, not a fixture.
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

    /// `stored_size()` is a `stat` on the compressed file itself, not the
    /// uncompressed length `size()` answers — the two must disagree for a
    /// payload that actually compresses
    /// (`docs/design/decisions.md`, "D21").
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
    /// own block count** (`docs/design/decisions.md`, "D2"):
    /// `crate::stream::cut` cuts at block boundaries, so a file with fewer
    /// blocks than the machine has cores offers no seam for the rest.
    ///
    /// The core count is asserted against `available_parallelism()` rather
    /// than a literal, the property pinned being that this source asks `std`
    /// rather than carrying a constant of its own — which is what makes a
    /// container's CPU quota reach the default
    /// (`docs/design/runtime-invariants.md`, `RT7`).
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

    /// **The seek table is held once, at a stated cost per entry** — the check
    /// for what is left of `KD26` (`docs/design/decisions.md`, "D4").
    ///
    /// Two properties, neither checked anywhere else. The **share**:
    /// `XzSource::assembled` takes the reader's own `Arc` through
    /// `xz_seek::Reader::index_shared`, so the source's table and the reader's
    /// are one allocation, and both constructors go through `assembled`. The
    /// **per-entry cost** is a property of a vendored struct, so a re-sync
    /// that grew either entry fails here rather than silently moving a
    /// published number.
    #[test]
    fn the_seek_table_is_held_once_at_a_stated_cost_per_entry() {
        assert_eq!(std::mem::size_of::<xz_seek::StreamEntry>(), 80);
        assert_eq!(std::mem::size_of::<xz_seek::BlockEntry>(), 32);

        let compressed = xz_compress(&xz_test_payload(), &["--block-size=4096"]);

        // The walking constructor: the reader built the table and hands its
        // own handle out.
        let source = XzSource::open(compressed.path()).unwrap();
        let persisted = source.seek_table().unwrap();
        {
            let reader = source.reader.lock().unwrap();
            assert!(
                std::ptr::eq(&*source.table, reader.index()),
                "the source's table must be the reader's own, not a clone of it",
            );
        }

        // The cached one: a table handed in is the reader's table too, so the
        // caller's copy is dropped rather than becoming a third.
        let cached = XzSource::with_table(compressed.path(), persisted).unwrap();
        let reader = cached.reader.lock().unwrap();
        assert!(
            std::ptr::eq(&*cached.table, reader.index()),
            "a cached table must be shared with the reader it was handed to",
        );
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

    /// A read at an offset behind the live decode's position must restart the
    /// covering block rather than return the wrong bytes — the one path
    /// `LocalFileSource` never has.
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
    /// costs a slice rather than a second decode.
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

    /// **A read served from a retained block never touches the reader**: with
    /// the reader's mutex held by another thread outright, the read still
    /// answers. The lock is taken only to name a block's task, and a block
    /// already decoded needs no task.
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
    /// the property the block path buys, a streaming read running under one
    /// mutex instead.
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
    /// line is the budget rather than a constant beside it
    /// (`docs/design/decisions.md`, "D16"). Asserted against a synthetic
    /// table, that being the only way to have a block this size without
    /// writing one.
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
        // file.
        const DECODE: u64 = 9_471_776;
        let affordable = |unit: u64, budget: u64| {
            let cache = BlockCache::for_table(&table(unit)).expect("a block to build a unit from");
            cache.affordable(crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES as u64, DECODE, budget)
        };
        // **`POOL_DEPTH` units, not one.** A single reader is charged the
        // block it holds *and* the retention list beside it
        // ([`BlockCache::worker_memory`]), so the default budget declines an
        // ordinary compressed dump (`docs/design/decisions.md`, "D16").
        assert!(!affordable(24 << 20, DEFAULT_MEMORY_BUDGET));
        assert!(affordable(24 << 20, 107 << 20));
        // Larger blocks — `xz --block-size=128MiB`, and `xz -9 -T0` — are
        // declined at the default and taken once the caller allows the room.
        assert!(!affordable(128 << 20, DEFAULT_MEMORY_BUDGET));
        assert!(!affordable(192 << 20, DEFAULT_MEMORY_BUDGET));
        assert!(!affordable(192 << 20, 768 << 20), "four units of 192 MiB is 768");
        assert!(affordable(192 << 20, 779 << 20));
        // **The unavoidable terms come off the top**: the blocks alone fit
        // this budget and the file is declined all the same, a reader of it
        // also holding the chunk buffer and the decoder — both of which the
        // streaming fallback holds too.
        assert!(!affordable(24 << 20, 96 << 20));
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
    /// permission reaches the block pool** (`docs/design/decisions.md`,
    /// "D5"). Every retained block is also one some reader is holding a view
    /// into, so `BlockCache::slot`'s drain can free nothing the waiting reader
    /// is not itself holding.
    ///
    /// The third read below is the one that would block: it runs on a detached
    /// thread against a bounded receive, so a pool that charges block slots
    /// fails this test rather than hanging the suite.
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

    /// The non-seekable shape: one stream, one block, produced by a bare `xz`
    /// invocation with no `-T`/`--block-size`. Every read still decodes to the
    /// right bytes, backward ones included.
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

    /// The three answers an origin exists to give, from one probe of a local
    /// file: the bytes as stored, the weak identity, and the leading magic.
    #[tokio::test]
    async fn an_origin_probe_answers_size_identity_and_the_leading_bytes() {
        let (file, _source) = source_of(b"\xfd7zXZ\x00 and then some more bytes");
        let origin = Origin::local(file.path());
        let probed = origin.probe().await.unwrap();
        assert_eq!(probed.stored_size(), std::fs::metadata(file.path()).unwrap().len());
        assert_eq!(
            probed.modified(),
            Some(std::fs::metadata(file.path()).unwrap().modified().unwrap())
        );
        assert_eq!(probed.leading(), &XZ_MAGIC[..]);
    }

    /// A source shorter than the magic brings back what it has, which is not
    /// an error and matches nothing.
    #[tokio::test]
    async fn an_origin_probe_of_a_short_source_brings_back_what_there_is() {
        let (file, _source) = source_of(b"ab");
        let origin = Origin::local(file.path());
        let probed = origin.probe().await.unwrap();
        assert_eq!(probed.leading(), b"ab");
        assert_eq!(probed.stored_size(), 2);
    }

    /// **The probe runs once.** Rewriting the file underneath a probed origin
    /// changes nothing it answers — which is the property a source whose
    /// probe is a round trip depends on, and the reason the claim and
    /// recognition see one consistent set of answers.
    #[tokio::test]
    async fn an_origin_probes_at_most_once() {
        let (mut file, _source) = source_of(b"plain");
        let origin = Origin::local(file.path());
        let first = origin.probe().await.unwrap().clone();
        file.write_all(b" and rather more content than there was before").unwrap();
        file.flush().unwrap();
        assert_eq!(origin.probe().await.unwrap(), &first);
    }

    /// **A failed probe is not cached**, so the open that follows a claim
    /// still reaches the failure — and an origin is not poisoned by having
    /// been asked too early.
    #[tokio::test]
    async fn a_failed_origin_probe_is_not_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-yet.sql");
        let origin = Origin::local(&path);
        assert!(origin.probe().await.is_err(), "nothing is there yet");
        std::fs::write(&path, b"now it is").unwrap();
        assert_eq!(origin.probe().await.unwrap().stored_size(), 9);
    }

    /// An origin names itself the way the message that refuses it does.
    #[tokio::test]
    async fn an_origin_displays_as_its_path() {
        let (file, _source) = source_of(b"x");
        assert_eq!(Origin::local(file.path()).to_string(), file.path().display().to_string());
    }

    /// Recognition with nothing claimed, unwrapped: `KnownCompression::Unknown`
    /// claims nothing recognition could contradict.
    async fn recognize(path: &Path) -> Arc<dyn ByteRangeSource> {
        match open_local(&Origin::local(path), KnownCompression::Unknown).await.unwrap() {
            Recognized::Source(source) => source,
            Recognized::Mismatch => panic!("`Unknown` claims nothing to contradict"),
        }
    }

    /// A genuinely `.xz`-compressed file is recognised whatever it is named
    /// (`docs/design/decisions.md`, "D14"): the temp file `xz_compress`
    /// returns carries no `.xz` suffix, and `open_local` still hands back a
    /// source whose `seek_table()` answers `Some`, which only `XzSource` does.
    #[tokio::test]
    async fn open_local_recognizes_xz_content_with_no_xz_name() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        assert_ne!(compressed.path().extension(), Some(std::ffi::OsStr::new("xz")));

        let source = recognize(compressed.path()).await;
        assert!(source.seek_table().is_some(), "content-sniffed as .xz");
        assert_eq!(source.size().await.unwrap(), payload.len() as u64);
        let got = source.read_range(0, payload.len()).await.unwrap();
        assert_eq!(&got[..], &payload[..]);
    }

    /// The other direction: a file *named* `.xz` whose bytes are not, which
    /// content sniffing opens plain (`docs/design/decisions.md`, "D14").
    #[tokio::test]
    async fn open_local_opens_a_dot_xz_named_file_with_plain_content_as_plain() {
        let mut file = tempfile::Builder::new().suffix(".xz").tempfile().unwrap();
        file.write_all(b"not actually compressed").unwrap();
        file.flush().unwrap();

        let source = recognize(file.path()).await;
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
        let source = recognize(&path).await;
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

        let source = match open_local(
            &Origin::local(compressed.path()),
            KnownCompression::Xz(table.clone()),
        )
        .await
        .unwrap()
        {
            Recognized::Source(source) => source,
            Recognized::Mismatch => panic!("the file's own table must describe it"),
        };
        assert_eq!(source.seek_table().as_ref(), Some(&table));
        assert_eq!(source.size().await.unwrap(), payload.len() as u64);
        let got = source.read_range(9_000, 4000).await.unwrap();
        assert_eq!(&got[..], &payload[9_000..13_000]);
    }

    // -----------------------------------------------------------------------
    // The fetched `.xz` source: one block at a time, out of windows the caller
    // fetched (`docs/design/roadmap-P14-remote-input.md`, "D1", "D13", "D20").
    //
    // The transport here is a local file, which is exactly the point: the
    // window-fed path has no local twin to disagree with, so putting a file
    // through it is what makes a divergence surface as a disagreement between
    // two read paths over one byte stream rather than between two sources.
    // -----------------------------------------------------------------------

    /// A `ByteRangeSource` that counts the reads made through it, so an
    /// assertion about the *request stream* — how many fetches a walk spends —
    /// is possible without a server.
    struct Counting {
        inner: Arc<dyn ByteRangeSource>,
        reads: Arc<AtomicUsize>,
    }

    impl ByteRangeSource for Counting {
        fn read_range(
            &self,
            offset: u64,
            len: usize,
        ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            self.inner.read_range(offset, len)
        }
        fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
            self.inner.size()
        }
        fn modified(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
            self.inner.modified()
        }
    }

    /// A fetched source over the compressed file at `path`, walked the way
    /// `open_remote` walks one: the probe's leading bytes first, then whatever
    /// the machine asks for.
    async fn fetched_xz(path: &Path) -> (FetchedXzSource, usize) {
        let transport: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(path).unwrap());
        let stored_size = transport.size().await.unwrap();
        let leading = transport.read_range(0, ORIGIN_LEADING_BYTES).await.unwrap();
        let (table, fetches) = walk_seek_table(&*transport, stored_size, &leading).await.unwrap();
        (FetchedXzSource::with_table(transport, stored_size, table).unwrap(), fetches)
    }

    /// The composition, end to end: a table built by driving the walk over a
    /// byte-range source, and every byte of the file read back out of fetched
    /// windows.
    #[tokio::test]
    async fn a_fetched_xz_source_reads_what_the_file_holds() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let (source, _) = fetched_xz(compressed.path()).await;

        assert_eq!(source.size().await.unwrap(), payload.len() as u64);
        assert_eq!(
            source.stored_size().await.unwrap(),
            std::fs::metadata(compressed.path()).unwrap().len()
        );
        assert!(source.seek_table().unwrap().block_count() > 1, "the fixture must be split");
        // Inside one block, straddling two, and the whole stream.
        assert_eq!(&source.read_range(100, 200).await.unwrap()[..], &payload[100..300]);
        assert_eq!(&source.read_range(4_000, 4_000).await.unwrap()[..], &payload[4_000..8_000]);
        assert_eq!(&source.read_range(0, payload.len()).await.unwrap()[..], &payload[..]);
    }

    /// The walked table is the same table the local source's own walk
    /// produces over the same file — the two drivers differ in who fetches
    /// and in nothing else.
    #[tokio::test]
    async fn a_walked_table_is_the_one_the_local_walk_produces() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let (source, _) = fetched_xz(compressed.path()).await;
        let local = XzSource::open(compressed.path()).unwrap().seek_table().unwrap();
        assert_eq!(source.seek_table().as_ref(), Some(&local));
    }

    /// **The probe's bytes are the walk's first request**, so a caller holding
    /// them spends no round trip on it: the same walk costs one more fetch
    /// when nothing is handed in.
    #[tokio::test]
    async fn the_walk_spends_no_fetch_on_leading_bytes_the_caller_holds() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let file: Arc<dyn ByteRangeSource> =
            Arc::new(LocalFileSource::open(compressed.path()).unwrap());
        let stored_size = file.size().await.unwrap();
        let leading = file.read_range(0, ORIGIN_LEADING_BYTES).await.unwrap();

        let reads = Arc::new(AtomicUsize::new(0));
        let counting = Counting { inner: Arc::clone(&file), reads: Arc::clone(&reads) };
        let (_, with_probe) = walk_seek_table(&counting, stored_size, &leading).await.unwrap();
        assert_eq!(with_probe, reads.load(Ordering::Relaxed));

        let reads = Arc::new(AtomicUsize::new(0));
        let counting = Counting { inner: file, reads: Arc::clone(&reads) };
        let (_, without) = walk_seek_table(&counting, stored_size, &[]).await.unwrap();
        assert_eq!(without, with_probe + 1);
    }

    /// The piecewise arm: a budget too small to hold a decoded block reads the
    /// same bytes out of the same windows, through `xz_seek::BlockRead` rather
    /// than into a pooled slot (D20). The two arms are asserted against each
    /// other rather than against a constant, which is what makes a divergence
    /// visible as one.
    #[tokio::test]
    async fn a_budget_too_small_for_a_block_reads_the_same_bytes_in_pieces() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let (source, _) = fetched_xz(compressed.path()).await;
        assert!(source.block_path().is_some(), "the default budget holds a 4 KiB block");

        source.hint_parallelism(Parallelism::Serial { memory_bytes: Some(1) });
        assert!(source.block_path().is_none(), "a one-byte budget holds no block");

        assert_eq!(&source.read_range(100, 200).await.unwrap()[..], &payload[100..300]);
        assert_eq!(&source.read_range(4_000, 4_000).await.unwrap()[..], &payload[4_000..8_000]);
        assert_eq!(&source.read_range(0, payload.len()).await.unwrap()[..], &payload[..]);
    }

    /// A read past the end of the uncompressed stream is the same
    /// `UnexpectedEof` every other source raises, on both arms.
    #[tokio::test]
    async fn a_read_past_the_end_is_refused_on_both_arms() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let (source, _) = fetched_xz(compressed.path()).await;
        let past = payload.len() as u64;
        for budget in [None, Some(1)] {
            source.hint_parallelism(Parallelism::Serial { memory_bytes: budget });
            let err = source.read_range(past - 10, 100).await.unwrap_err();
            assert!(matches!(&err, Error::Io(e) if e.kind() == std::io::ErrorKind::UnexpectedEof));
        }
    }

    /// A table that does not describe this object is refused by `xz_seek`'s
    /// validation without a byte being read, which is what `open_remote`
    /// reports as `Recognized::Mismatch` rather than paying a walk.
    #[tokio::test]
    async fn a_table_from_another_file_is_an_invalid_table_rather_than_a_read() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let other = xz_compress(&payload[..1000], &["--block-size=512"]);
        let table = XzSource::open(other.path()).unwrap().seek_table().unwrap();

        let transport: Arc<dyn ByteRangeSource> =
            Arc::new(LocalFileSource::open(compressed.path()).unwrap());
        let stored_size = transport.size().await.unwrap();
        let err = FetchedXzSource::with_table(transport, stored_size, table).unwrap_err();
        assert!(matches!(err, Error::Xz(xz_seek::Error::InvalidTable { .. })), "{err:?}");
    }

    /// The single-block shape reads correctly too — there is no seek structure
    /// to cut at, so every read decodes the one block from its start, and the
    /// piecewise arm is what keeps that from allocating the whole plaintext.
    #[tokio::test]
    async fn a_single_block_file_is_read_by_the_same_two_arms() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &[]);
        let (source, _) = fetched_xz(compressed.path()).await;
        assert!(!source.seek_table().unwrap().is_seekable());
        assert_eq!(&source.read_range(1_000, 500).await.unwrap()[..], &payload[1_000..1_500]);
        source.hint_parallelism(Parallelism::Serial { memory_bytes: Some(1) });
        assert_eq!(&source.read_range(1_000, 500).await.unwrap()[..], &payload[1_000..1_500]);
    }

    /// **A wait is never taken here, whatever a loop grants.** The chunk
    /// buffer is obtained on the runtime's own task rather than inside a
    /// `spawn_blocking` closure, so a `Condvar` wait would block the thread a
    /// releasing sibling needs. The local source takes the same grant, which
    /// is what makes this a difference between two transports rather than a
    /// policy nobody implements.
    #[tokio::test]
    async fn a_fetched_source_never_takes_the_wait_a_loop_grants() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let (fetched, _) = fetched_xz(compressed.path()).await;
        fetched.hint_wait_policy(WaitPolicy::MayWait);
        assert_eq!(fetched.pool.policy(), WaitPolicy::NeverWait);

        let local = XzSource::open(compressed.path()).unwrap();
        local.hint_wait_policy(WaitPolicy::MayWait);
        assert_eq!(local.pool.policy(), WaitPolicy::MayWait);
    }

    /// The compressed window is charged rather than left unpooled: the
    /// per-reader charge a caller solves a budget against carries the widest
    /// window this file's blocks would make us fetch, on top of what the local
    /// source charges for the same file (D20).
    #[tokio::test]
    async fn the_compressed_window_is_inside_the_charge() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let (source, _) = fetched_xz(compressed.path()).await;
        let table = source.seek_table().unwrap();
        let widest = table.blocks.iter().map(|b| b.total_size()).max().unwrap();
        assert!(widest > 0);

        let local = XzSource::open(compressed.path()).unwrap();
        let fetched_charge = source.block_decode_bytes().unwrap();
        let local_charge = local.block_decode_bytes().unwrap();
        // The local source's decoder charge carries an input chunk this one
        // never builds, so the two differ by the window less that chunk rather
        // than by the window alone.
        assert!(
            fetched_charge > local_charge.saturating_sub(widest),
            "the window must be billed: fetched {fetched_charge}, local {local_charge}, \
             widest window {widest}",
        );
    }

    /// **The walk is actually skipped**, tested behaviourally rather than by
    /// instrumentation (`docs/design/decisions.md`, "D18"): the table handed
    /// in names a check algorithm this file's streams do not use, which
    /// `validate` does not police. The check's size moves the payload's end,
    /// so a decode from the handed table fails where a silent re-walk would
    /// have read the payload back happily.
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

        let source =
            match open_local(&Origin::local(compressed.path()), KnownCompression::Xz(table))
                .await
                .unwrap()
            {
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
            open_local(&Origin::local(compressed.path()), KnownCompression::Xz(other_table))
                .await
                .unwrap(),
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
            open_local(&Origin::local(plain_file.path()), KnownCompression::Xz(table))
                .await
                .unwrap(),
            Recognized::Mismatch
        ));
        assert!(matches!(
            open_local(&Origin::local(compressed.path()), KnownCompression::Plain).await.unwrap(),
            Recognized::Mismatch
        ));
    }

    /// A plain claim about a plain file is simply right, and costs nothing.
    #[tokio::test]
    async fn open_local_accepts_a_plain_claim_about_a_plain_file() {
        let (file, _plain) = source_of(b"0123456789abcdef");
        let source =
            match open_local(&Origin::local(file.path()), KnownCompression::Plain).await.unwrap() {
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
    /// `name=memory` and holds no memory controller at all.
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
    /// throttle two levels up may be the smallest number in the walk.
    #[test]
    fn the_smallest_of_every_limit_at_every_level_is_what_binds() {
        let root = FakeRoot::new();
        root.v2("/a/b/c")
            .v2_limits("/a/b/c", Some("2147483648"), None)
            .v2_limits("/a/b", None, Some("134217728"))
            .v2_limits("/a", Some("1073741824"), None);
        assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(134217728));
    }

    /// **The file that stated the smallest limit comes back with it**:
    /// `memory.high` throttles where `memory.max` kills (`RT3`) and either may
    /// be an ancestor's (`RT5`), so a status line is only actionable beside the
    /// file that cut the budget (`docs/design/decisions.md`, "D64").
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
    /// limit found means no limit is being enforced. A tree with no cgroup
    /// file at all reads the same way, which is the non-Linux case.
    #[test]
    fn an_unlimited_hierarchy_and_a_missing_one_both_read_as_no_limit() {
        let root = FakeRoot::new();
        root.v2("/leaf").v2_limits("/leaf", Some("max"), Some("max"));
        assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), None);
        assert_eq!(discover_memory_limit_in(FakeRoot::new().path()).map(|l| l.bytes), None);
    }

    /// **A v1 hierarchy is read from its own mount, located through
    /// `mountinfo`** (`RT4`, `RT6`), and "unlimited" there is a *threshold*
    /// near `LONG_MAX` rather than a sentinel string, the value being a
    /// function of the page size and the word width.
    ///
    /// This establishes that the reader handles the shape `RT4` describes. It
    /// observes no kernel (`docs/design/decisions.md`, "D11").
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
            // 32-bit `PAGE_COUNTER_MAX × PAGE_SIZE`, the smallest shape
            // "unlimited" takes and so the one a threshold has to clear.
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

    /// **`MemAvailable`, in kB, and nothing else** (`RT8`). `MemFree` is on
    /// the same file and is not read: it excludes reclaimable page cache.
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
    /// taken no higher, while a recommendation that already fits is left
    /// alone. The count is held to [`MEMORY_MARGIN_PERCENT`] besides, the
    /// tighter of the two conditions at this limit; the budget is still what
    /// the resolved count spends.
    #[test]
    fn a_discovered_limit_caps_the_recommendation_at_the_limit_less_the_reserve() {
        let root = FakeRoot::new();
        root.v2("/leaf").v2_limits("/leaf", Some("1073741824"), None);
        let cap = (1024 << 20) - MEMORY_RESERVE;

        // Far more than the cap affords, so the count comes down with the
        // budget rather than being printed beside one it cannot spend. The
        // budget is still `cap`-bounded and not margin-bounded, so the floor
        // arrangement reports what one reader holds.
        let squeezed =
            Parallelism::discover_in(root.path(), 8, Some(WorkerMemory::per_worker(512 << 20)));
        assert_eq!(squeezed.memory_bytes(), Some(512 << 20));
        assert_eq!(squeezed.jobs(), 1);

        // Inside the margin allowance as well as inside the cap, so both
        // numbers stand.
        let roomy =
            Parallelism::discover_in(root.path(), 8, Some(WorkerMemory::per_worker(16 << 20)));
        assert_eq!(roomy.memory_bytes(), Some(128 << 20));
        assert_eq!(roomy.jobs(), 8);

        // And in between, the count is what the margin affords and the budget
        // is exactly what that many readers spend — never the cap itself,
        // which is the over-ask this pairing exists to remove. One more reader
        // would fit the cap and is refused.
        let fitted =
            Parallelism::discover_in(root.path(), 8, Some(WorkerMemory::per_worker(100 << 20)));
        assert_eq!(fitted.jobs(), 5);
        assert_eq!(fitted.memory_bytes(), Some(500 << 20));
        assert!(fitted.memory_bytes() < Some(cap), "the cap itself would be the over-ask");
    }

    /// **The resolved count answers to the criterion rather than to the host's
    /// width**: a recommendation above what the margin allows resolves the
    /// same arrangement whatever the recommendation is
    /// (`docs/design/decisions.md`, "D3").
    #[test]
    fn the_count_answers_to_the_margin_and_not_to_the_recommendation() {
        let root = FakeRoot::new();
        root.v2("/leaf").v2_limits("/leaf", Some(&(2u64 << 30).to_string()), None);
        // What one reader of a large-block `.xz` holds, and the retention list
        // the pool keeps beside it — a block size at which this limit resolves
        // well inside either host's core count, so what binds is unambiguous.
        let reader = 138 << 20;
        let memory = WorkerMemory::per_worker(reader).pooling(128 << 20, POOL_DEPTH);

        let narrow = Parallelism::discover_in(root.path(), 24, Some(memory));
        let wide = Parallelism::discover_in(root.path(), 64, Some(memory));
        assert_eq!(narrow.jobs(), wide.jobs(), "the host's core count is not the answer");
        assert_eq!(narrow.memory_bytes(), wide.memory_bytes());

        // And what it resolves to leaves the margin, where the cap alone would
        // have admitted another reader.
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
    /// The arrangement needs a limit whose margin allowance falls under one
    /// reader's charge while the cap stays above it, which a large-block file
    /// reaches and a small-block one does not.
    #[test]
    fn the_margin_never_takes_the_last_reader_or_the_budget_it_spends() {
        let root = FakeRoot::new();
        // A limit whose margin allowance is under what one block-decoding
        // reader of this file is charged, while its cap is above it.
        root.v2("/leaf").v2_limits("/leaf", Some(&(960u64 << 20).to_string()), None);
        let memory = WorkerMemory::per_worker(138 << 20).pooling(128 << 20, POOL_DEPTH);
        let tight = Parallelism::discover_in(root.path(), 24, Some(memory));
        assert_eq!(tight.jobs(), 1);
        assert_eq!(tight.memory_bytes(), Some(memory.at(1)));
        assert!(memory.at(1) > margin_allowance(960 << 20), "the margin cannot afford it");
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

    /// **What a mapping pass's statistics are left has two bands, and its
    /// floor is not the margin's** (`docs/design/decisions.md`, "D85"). Both
    /// lines are arithmetic between the three constants rather than readings,
    /// and nothing else would notice one of them moving past another.
    ///
    /// **Its own zero line is `MEMORY_UNPOOLED_BOUND × 100 / (100 −
    /// MEMORY_MARGIN_PERCENT)`** — at and below it the ceiling this subtracts
    /// a budget from is zero on its own, so no allowance there gathers
    /// anything at any budget, and the line is reached rather than merely
    /// approached.
    ///
    /// **The swap is `the_margin_is_the_tighter_condition_only_above_the_crossover`'s
    /// crossover read the other way round.** Below it a count is solved
    /// against the cap, which stands under the ceiling, so `ceiling − cap`
    /// survives whatever the source charges; above it the count is solved
    /// against the ceiling itself and a source whose step divides it takes the
    /// statistics to nothing — a wide allowance, not a narrow one.
    #[test]
    fn statistics_are_starved_below_their_own_line_and_unfloored_above_the_crossover() {
        let zero_line = MEMORY_UNPOOLED_BOUND * 100 / (100 - MEMORY_MARGIN_PERCENT);
        assert_eq!(zero_line, 320 << 20);
        assert_eq!(statistics_allowance(zero_line, 0), 0, "at the line, not only under it");
        assert_eq!(statistics_allowance(zero_line - (1 << 20), 0), 0);
        assert!(statistics_allowance(zero_line + (1 << 20), 0) > 0, "and positive above it");

        let crossover = 5 * (MEMORY_RESERVE - MEMORY_UNPOOLED_BOUND);
        let below = crossover - (64 << 20);
        let above = crossover + (64 << 20);

        // Below the crossover no budget a fit reports can reach the ceiling,
        // the cap being the tighter of the two: `ceiling − cap` is left at
        // every per-worker charge, including one too large for a single worker.
        let floor = margin_allowance(below) - (below - MEMORY_RESERVE);
        assert!(floor > 0, "an allowance in this band cannot starve statistics");
        for per_worker in [1u64 << 20, 17 << 20, 64 << 20, 96 << 20, 1 << 30] {
            let fitted = Parallelism::within(24, Some(WorkerMemory::per_worker(per_worker)), below);
            let left = statistics_allowance(below, fitted.memory_bytes().unwrap());
            assert!(left >= floor, "{per_worker} left {left}, under the cap's own floor {floor}");
        }

        // Above it the ceiling binds instead, so the count is solved right up
        // against the number this subtracts from and there is no floor left.
        let ceiling = margin_allowance(above);
        assert!(ceiling < above - MEMORY_RESERVE, "the ceiling binds here, not the cap");
        let fitted = Parallelism::within(24, Some(WorkerMemory::per_worker(ceiling / 4)), above);
        assert_eq!(fitted.jobs(), 4, "four workers is what the ceiling exactly affords");
        assert_eq!(statistics_allowance(above, fitted.memory_bytes().unwrap()), 0);
    }

    /// **Below the reserve the budget goes to zero rather than to a floor**
    /// (`docs/design/decisions.md`, "D3"). What zero produces is one reader's
    /// worth on the streaming path, through the floors already in the
    /// mechanism.
    #[test]
    fn a_limit_at_or_under_the_reserve_leaves_no_budget_at_all() {
        let root = FakeRoot::new();
        root.v2("/leaf").v2_limits("/leaf", Some(&MEMORY_RESERVE.to_string()), None);
        let starved =
            Parallelism::discover_in(root.path(), 4, Some(WorkerMemory::per_worker(1 << 30)));
        assert_eq!(starved.memory_bytes(), Some(0));
        // One worker at whatever the cap is, not one worker's worth of bytes:
        // the floor is on the count.
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

    /// **The composition is not a `min`.** With no limit found there is no cap
    /// from the environment at all, so a source's recommendation stands, where
    /// a `min` against [`DEFAULT_MEMORY_BUDGET`] would have collapsed a
    /// compressed scan to serial (`docs/design/decisions.md`, "D3").
    #[test]
    fn no_limit_found_leaves_the_recommendation_uncapped_but_for_memavailable() {
        let root = FakeRoot::new();
        root.v2("/leaf")
            .v2_limits("/leaf", Some("max"), None)
            .write("proc/meminfo", "MemAvailable:   20002184 kB\n");
        // Readers of an ordinary compressed dump, well under half of the
        // `MemAvailable` stated above.
        let per_worker = 58 << 20;
        let roomy =
            Parallelism::discover_in(root.path(), 24, Some(WorkerMemory::per_worker(per_worker)));
        assert_eq!(roomy.memory_bytes(), Some(per_worker * 24));
        assert_eq!(roomy.jobs(), 24);

        // Half of `MemAvailable`, because it is an estimate two processes
        // reading at once each see the whole of — and it binds wherever the
        // recommended count costs more than half of what is free, the count
        // then reduced to what half of it buys rather than left standing.
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

    /// **A stated allowance is the discovered limit's own carving**, which is
    /// the whole of what `--memory` promises: the same number reached either
    /// way resolves to the same pair (`docs/design/decisions.md`, "D83"). The
    /// allowances span the margin's crossover, so the check covers the band
    /// where the cap binds and the band where the ceiling does.
    #[test]
    fn a_stated_allowance_resolves_as_the_same_number_discovered_would() {
        let memory = Some(WorkerMemory::per_worker(58 << 20).pooling(24 << 20, POOL_DEPTH));
        for allowance in [512u64 << 20, 640 << 20, 1024 << 20, 4096 << 20] {
            let root = FakeRoot::new();
            root.v2("/leaf").v2_limits("/leaf", Some(&allowance.to_string()), None);
            for jobs in [1, 4, 24] {
                assert_eq!(
                    Parallelism::within(jobs, memory, allowance),
                    Parallelism::discover_in(root.path(), jobs, memory),
                    "{allowance} bytes at {jobs} jobs",
                );
            }
        }
    }

    /// **The typed number bounds the process, and what comes back bounds the
    /// pools**: the reserve is off the top before a reader is counted, and an
    /// allowance the reserve swallows resolves to a budget of zero rather than
    /// to a floor — which the floors inside the mechanism turn into one reader
    /// on the streaming path.
    #[test]
    fn a_stated_allowance_pays_the_reserve_before_anything_else() {
        let per_worker = 58 << 20;
        let memory = Some(WorkerMemory::per_worker(per_worker));

        // Six readers fit inside the 400 MiB *left* by the allowance; the
        // seventh does not. The budget is what those six spend, never the
        // number typed.
        let cut = Parallelism::within(24, memory, MEMORY_RESERVE + (400 << 20));
        assert_eq!(cut.jobs(), 6);
        assert_eq!(cut.memory_bytes(), Some(6 * per_worker), "the carved spend, not the allowance");

        // Room for the whole recommendation leaves it standing.
        let roomy = Parallelism::within(24, memory, 64 << 30);
        assert_eq!(roomy.jobs(), 24);
        assert_eq!(roomy.memory_bytes(), Some(24 * per_worker));

        // At and below the reserve there is nothing left to spend.
        let starved = Parallelism::within(24, memory, MEMORY_RESERVE);
        assert_eq!(starved, Parallelism::Serial { memory_bytes: Some(0) });

        // Nothing to divide by leaves the count where it is, on the constant.
        let blind = Parallelism::within(8, None, 64 << 30);
        assert_eq!(blind.jobs(), 8);
        assert_eq!(blind.memory_bytes(), Some(DEFAULT_MEMORY_BUDGET));
    }

    /// **An unlimited environment falls back to the shipped constant**, and at
    /// a serial count all the way back to [`Parallelism::default`], the state
    /// that says nobody asked for a budget at all. A `Workers` arrangement has
    /// nowhere to record that, so it carries the constant bare.
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

    /// **The closed form agrees with a search, over the whole grid.**
    /// [`WorkerMemory::affords`] answers in one division above `pool_depth`
    /// and by enumeration below it, which is two pieces of arithmetic where
    /// the question is one — and the boundary between them is exactly where a
    /// shape with a pool term stops being `n × (per_worker + pool_unit)`. So
    /// it is checked against the definition it is an optimisation of: the
    /// largest `n` in `1..=most` with `at(n) <= cap`, never zero.
    #[test]
    fn the_afforded_count_is_the_largest_one_the_cap_holds() {
        let shapes = [
            WorkerMemory::default(),
            WorkerMemory::per_worker(0).pooling(24 << 20, POOL_DEPTH),
            WorkerMemory::per_worker(34 << 20),
            WorkerMemory::per_worker(34 << 20).pooling(24 << 20, POOL_DEPTH),
            WorkerMemory::per_worker(138 << 20).pooling(128 << 20, POOL_DEPTH),
            WorkerMemory::per_worker(1).pooling(1, 1),
            WorkerMemory::per_worker(7).pooling(3, 9),
        ];
        for memory in shapes {
            for cap in [0u64, 1, 3, 7, 16, 100, 24 << 20, 128 << 20, 1 << 30, u64::MAX] {
                for most in [1usize, 2, 4, 5, 11, 64] {
                    let want = (1..=most).rev().find(|n| memory.at(*n) <= cap).unwrap_or(1);
                    assert_eq!(
                        memory.affords(cap, most),
                        want,
                        "{memory:?} at cap {cap} most {most}"
                    );
                }
            }
        }
    }

    /// **A source recommends what its workers hold**, so that "no limit found"
    /// cannot mean "serial" (`docs/design/decisions.md`, "D3"). The plain
    /// source inherits the silence it inherits for the worker count.
    ///
    /// **A shape rather than a scalar**, because the block pool's retention
    /// list is not per worker: `block_decode_bytes` is that same shape
    /// evaluated at one reader — the gate's own number — and is strictly above
    /// the per-worker term, while past [`POOL_DEPTH`] readers every further
    /// reader adds a unit of list as well as its own term.
    #[test]
    fn a_compressed_source_recommends_what_its_workers_hold() {
        assert_eq!(BareSource.default_worker_memory(), None);
        let (_file, plain) = source_of(b"0123456789abcdef");
        assert_eq!(plain.default_worker_memory(), None);

        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let source = XzSource::open(compressed.path()).unwrap();
        let memory = source.default_worker_memory().expect("a block path to price");
        let unit = source.blocks.as_ref().expect("a block cache").unit as u64;
        assert_eq!(source.block_decode_bytes(), Some(memory.at(1)));
        assert_eq!(
            memory.at(1),
            memory.bytes_per_worker() + (POOL_DEPTH as u64 - 1) * unit,
            "one reader leaves a retention list nobody else fills"
        );
        assert_eq!(
            memory.at(POOL_DEPTH + 2),
            memory.bytes_per_worker() * (POOL_DEPTH as u64 + 2) + (POOL_DEPTH as u64 + 1) * unit,
            "past the pool's own depth each reader brings a slot of list with it"
        );
    }

    /// **The charge an allowance is solved against before the file is open is
    /// the charge the resulting count then meets.** `Parallelism::fit` solves
    /// a cap against `default_worker_memory` with nothing announced;
    /// `crate::stream::worker_count` solves the budget that produced against
    /// `partitions().worker_memory()` once the scan has announced its chunk.
    /// Charged at the pool ceiling instead of the default chunk, the first
    /// would sit above the second and resolve fewer readers than the scan
    /// admits.
    ///
    /// Asserted end to end rather than by comparing two accessors: what has to
    /// agree is the *resolution* and the *scan*.
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
        source.hint_read_size(crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES);
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
