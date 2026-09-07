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
    fn partitions(&self, _range: Range<u64>) -> Partitioning {
        Partitioning::single(0)
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
    /// [`Parallelism::Serial`] states no byte count, so a source told that
    /// keeps [`DEFAULT_MEMORY_BUDGET`].
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
    /// a one-slot pool blocks forever.
    MayWait,
}

/// Where a source is willing to be split
/// (`docs/design/architecture.md`, "Execution model and API surface").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartitionBoundaries {
    /// Anywhere in the range: no offset costs more to start reading at than
    /// another. A plain file's answer, and what a caller may cut into as many
    /// equal pieces as it has workers.
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

/// A source's answer to [`ByteRangeSource::partitions`]: where to split, and
/// what one partition holds resident while it reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partitioning {
    boundaries: PartitionBoundaries,
    partition_bytes: u64,
}

impl Partitioning {
    /// Split anywhere; each partition holds `partition_bytes`.
    pub fn anywhere(partition_bytes: u64) -> Self {
        Self { boundaries: PartitionBoundaries::Anywhere, partition_bytes }
    }

    /// Split only at `offsets`, which are sorted and deduplicated here so the
    /// ascending order the accessor promises is a property of the value rather
    /// than of every source that builds one.
    pub fn at(mut offsets: Vec<u64>, partition_bytes: u64) -> Self {
        offsets.sort_unstable();
        offsets.dedup();
        Self { boundaries: PartitionBoundaries::At(offsets), partition_bytes }
    }

    /// One partition: this range is not to be split at all.
    pub fn single(partition_bytes: u64) -> Self {
        Self::at(Vec::new(), partition_bytes)
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
    /// For a plain file that is one read chunk. For a compressed one it is a
    /// decoded block plus the chunk buffer a read straddling a block boundary
    /// is assembled into — 32 MiB against the 24 MiB blocks koji's download
    /// carries.
    ///
    /// **Two costs are deliberately outside it, and a caller budgeting workers
    /// adds them.** The decoder's own compressed input buffer is `xz-seek`'s
    /// and fixed; and the LZMA2 dictionary is written in each block's *header*,
    /// which the seek table does not carry and which would cost a source read
    /// per block to learn — `xz_seek::RangePlan::footprint` excludes it for
    /// that same reason and names 8 MiB a worker as the allowance on the files
    /// this reads.
    pub fn partition_bytes(&self) -> u64 {
        self.partition_bytes
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
}

/// How much concurrency a caller allows a scan or a query, and how much
/// memory that concurrency may hold
/// (`docs/design/architecture.md`, "Execution model and API surface").
///
/// **The library defaults to [`Parallelism::Serial`]**, which is the serial
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
/// **Two mechanisms read it, and neither of them spawns.** `memory_bytes` is
/// what both pools in a source are sized from, and it is what decides whether
/// a compressed source can afford to decode a whole block ("The compressed
/// source"); `jobs` is the block pool's depth, one retained block per
/// concurrent reader, and — capped by what the bytes afford — how many
/// sub-streams a partitioned replay is cut into
/// (`crate::table_stream_partitions`). The caller runs those sub-streams, so
/// what `jobs` states is a ceiling rather than a request
/// (`docs/design/architecture.md`, "Execution model and API surface").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Parallelism {
    /// The serial code path: one thread, reading in file order.
    #[default]
    Serial,
    /// At most `jobs` concurrent workers, holding at most `memory_bytes`
    /// between them.
    Workers {
        /// The ceiling on concurrent workers — a ceiling rather than a
        /// request, since three input shapes admit no parallelism at all
        /// (`docs/design/roadmap-P16-parallel-scan.md`, "`--jobs` is a
        /// ceiling, not a request").
        jobs: NonZeroUsize,
        /// What those workers may hold resident between them, in bytes. This
        /// is the number a memory cgroup is denominated in, and the one a
        /// worker count cannot be promised to: the same count is 192 MiB on a
        /// file of 24 MiB blocks and 1 GiB on a file of 128 MiB ones.
        memory_bytes: u64,
    },
}

impl Parallelism {
    /// `jobs` workers inside `memory_bytes`, or [`Parallelism::Serial`] where
    /// `jobs` is one or zero.
    ///
    /// Zero reads as one rather than as an error, exactly as
    /// `xz_seek::Bulk::new` reads it: it is a shape a caller's own arithmetic
    /// produces, and an error would only make them do that arithmetic twice.
    pub fn workers(jobs: usize, memory_bytes: u64) -> Self {
        match NonZeroUsize::new(jobs) {
            Some(jobs) if jobs.get() > 1 => Self::Workers { jobs, memory_bytes },
            _ => Self::Serial,
        }
    }

    /// Whether this is the serial path.
    pub fn is_serial(&self) -> bool {
        matches!(self, Self::Serial)
    }

    /// The bytes this caller allows, or `None` for [`Parallelism::Serial`],
    /// which states no number.
    ///
    /// `None` rather than [`DEFAULT_MEMORY_BUDGET`] because "the caller said
    /// nothing" and "the caller said 64 MiB" are different facts, and a source
    /// that already holds a budget of its own — one set by an earlier
    /// announcement — must be able to tell them apart.
    pub fn memory_bytes(&self) -> Option<u64> {
        match self {
            Self::Serial => None,
            Self::Workers { memory_bytes, .. } => Some(*memory_bytes),
        }
    }

    /// The ceiling on concurrent workers: one for [`Parallelism::Serial`],
    /// which *is* one worker rather than none.
    pub fn jobs(&self) -> usize {
        match self {
            Self::Serial => 1,
            Self::Workers { jobs, .. } => jobs.get(),
        }
    }
}

/// What a source may hold in pooled buffers when the caller has stated no
/// budget of its own — [`Parallelism::Serial`]'s number, and the CLI's default
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
/// **Size stands in for one-off-ness, which is why it is not the only rule.**
/// A chunk buffer at the configured size is asked for once per chunk for the
/// whole scan — the thing the pool is for — and by length alone it is
/// indistinguishable from a span read, so this ceiling on its own turns
/// pooling off for any chunk above it. What tells the two apart is the caller
/// saying so: [`ByteRangeSource::hint_read_size`] names the length a read loop
/// is about to repeat, and a buffer of exactly that length is kept however
/// large it is (`docs/design/architecture.md`, "Execution model and API
/// surface").
const POOL_MAX_BYTES: usize = 8 << 20;

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

    /// The most this pool can hold in free buffers: its slot count at its own
    /// slot size.
    ///
    /// This is what a source with **two** read units subtracts before handing
    /// the remainder to the second pool, so that one stated budget bounds the
    /// source rather than each of its pools separately
    /// (`docs/design/architecture.md`, "The compressed source").
    fn held_bytes(&self) -> usize {
        self.slots().saturating_mul(self.slot_bytes())
    }

    /// Whether a released buffer is worth keeping: anything under the ceiling,
    /// plus a buffer of exactly the announced read length.
    ///
    /// **The hint clause matches exactly, not "up to", and it only bites above
    /// the ceiling.** Past [`POOL_MAX_BYTES`] the announced length is the only
    /// thing kept, so a coalesced span read is dropped even when it is
    /// *smaller* than a raised chunk size — which is the pair the hint exists
    /// to tell apart. Below the ceiling nothing is told apart: a span read
    /// under 8 MiB is pooled like any other buffer, exactly as it was before a
    /// hint existed. That is a deliberate floor rather than an oversight —
    /// [`BufferPool::slots`] bounds what it can cost, and the ceiling has to
    /// keep working for a source no caller ever announced to.
    fn keeps(&self, len: usize) -> bool {
        len <= POOL_MAX_BYTES || len == self.hinted.load(Ordering::Relaxed)
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
        let slots = self.slots();
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
            // `obtain` blocks for a transient holder, and blocking the runtime
            // thread that is meant to be draining the reads which would free a
            // slot is the one way to make this wait a deadlock in a build that
            // has no scheduler yet.
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

    /// **Anywhere, one buffer each.** A positioned read costs the same at
    /// every offset, so nothing about this source prefers one split point to
    /// another, and what a partition holds is one read chunk — the pool's own
    /// slot size, which is the length a read loop announced or the ceiling
    /// where none has ([`BufferPool::slot_bytes`]).
    fn partitions(&self, _range: Range<u64>) -> Partitioning {
        Partitioning::anywhere(self.pool.slot_bytes() as u64)
    }

    /// **The budget sizes the free list; the worker count does not.** This
    /// source has one read unit, and a chunk buffer is taken and released
    /// inside a single `read_range` — so what the free list has to hold is the
    /// *replay* path's depth, which is what [`POOL_DEPTH`] is, and not a
    /// worker count. Raising the ceiling with `jobs` would grow a query's
    /// resident set by `(jobs - POOL_DEPTH)` chunks the moment
    /// `crate::batch::RetainedChunks` releases the buffers a flushed batch was
    /// pinning, buying idle capacity for concurrency that no scheduler in this
    /// build produces (`docs/design/architecture.md`, "Execution model and API
    /// surface").
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
/// [`Parallelism::Serial`], and also a `u64` too large to be a length on this
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
/// evicted buffer is what the next decode reuses and the pool's budget bounds
/// the retained blocks and the free ones together — a set bounded at
/// [`BufferPool::slots`] and its own budget again would be two numbers for one
/// bound, which is exactly what the byte budget replaced.
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

    /// Whether a whole block fits the budget this pool was given — the line
    /// that decides between block decode and the streaming reader
    /// (`docs/design/architecture.md`, "The compressed source").
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
    fn affordable(&self) -> bool {
        self.unit <= self.pool.budget()
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
    /// **Eviction before acquisition is what would let a waiting holder
    /// through here.** A read loop holding no decoded block of its own between
    /// reads leaves the drain to `slots - 1` free to hand it the slot the
    /// acquisition below is about to take, so the wait would never fire; a
    /// loop pinning chunks into batches keeps those buffers outstanding past
    /// the drain, which is exactly why such a loop grants no wait at all
    /// ([`BufferPool::obtain`], and `docs/design/architecture.md`, "Execution
    /// model and API surface").
    ///
    /// A block evicted while a [`Bytes`] still views it stays alive until that
    /// view drops.
    fn slot(&self) -> PooledBuffer {
        let keep = self.pool.slots().saturating_sub(1);
        {
            let mut retained = self.retained.lock().unwrap_or_else(|e| e.into_inner());
            let over = retained.len().saturating_sub(keep);
            retained.drain(..over);
        }
        self.pool.obtain(self.unit)
    }

    /// Retain `block` as most-recently-used. [`BlockCache::slot`] has already
    /// made room, so nothing is evicted here.
    fn retain(&self, index: usize, block: Arc<DecodedBlock>) {
        let mut retained = self.retained.lock().unwrap_or_else(|e| e.into_inner());
        retained.retain(|(i, _)| *i != index);
        retained.push((index, block));
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
        let file = std::fs::File::open(&path)?;
        let stat_file = Arc::new(file.try_clone()?);
        let data_file = Arc::new(file.try_clone()?);
        let reader = xz_seek::Reader::new(file)?;
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
        let source = Self {
            path,
            stat_file,
            data_file,
            table,
            reader: Arc::new(Mutex::new(reader)),
            pool: Arc::new(BufferPool::default()),
            blocks,
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
            blocks
                .pool
                .set_limits(budget.saturating_sub(self.pool.held_bytes()), POOL_DEPTH.max(jobs));
        }
    }

    /// The block-decode path, or `None` where this read goes through the
    /// streaming reader: no blocks at all, or a block the caller's budget
    /// cannot hold ([`BlockCache::affordable`]).
    fn block_path(&self) -> Option<&Arc<BlockCache>> {
        self.blocks.as_ref().filter(|cache| cache.affordable())
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
    /// **Two concurrent misses on one block decode it twice**, and that is
    /// accepted rather than coordinated: an in-flight map would serialize the
    /// common case — different readers on different blocks — behind a second
    /// lock to spare a duplicate decode that only a shared boundary produces.
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
    /// holds one decoded block slot plus the chunk buffer a read straddling a
    /// boundary is assembled into.
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
        range: Range<u64>,
    ) -> Partitioning {
        let Some(cache) = blocks else {
            return Partitioning::single(chunk_bytes);
        };
        let covering = table.blocks_in(range.clone());
        let at = covering
            .filter_map(|i| {
                let start = table.blocks[i].uncompressed_offset;
                (start > range.start && start < range.end).then_some(start)
            })
            .collect();
        Partitioning::at(at, cache.unit as u64 + chunk_bytes)
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

    /// **Both pools, one policy.** The two units are read by the same loop, so
    /// where a wait is permitted the chunk assembly and the block decode both
    /// wait, and where it is not both allocate.
    ///
    /// A read that needs both takes the **chunk** slot first and the block
    /// slot inside it ([`XzSource::read_by_blocks`]), never the other way
    /// round, so two waiting readers cannot hold each other's next slot.
    fn hint_wait_policy(&self, policy: WaitPolicy) {
        self.pool.set_policy(policy);
        if let Some(blocks) = &self.blocks {
            blocks.pool.set_policy(policy);
        }
    }

    fn seek_table(&self) -> Option<xz_seek::SeekTable> {
        Some((*self.table).clone())
    }

    fn partitions(&self, range: Range<u64>) -> Partitioning {
        Self::partition_advice(
            &self.table,
            self.block_path().map(|cache| &**cache),
            self.pool.slot_bytes() as u64,
            range,
        )
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

    /// The announced read length is the one thing that survives the ceiling,
    /// and only at exactly that length: a scan configured with a large chunk
    /// keeps its pooling, while a span read of some other oversized length is
    /// still dropped.
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

        // A one-off span read at another oversized length is still dropped,
        // even one *smaller* than the announced chunk.
        pool.give(vec![0u8; chunk - 1]);
        pool.give(vec![0u8; chunk + 1]);
        assert_eq!(pool.free_len(), 1);
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
        assert_eq!(Parallelism::default(), Parallelism::Serial);
        assert!(crate::scan::ScanOptions::default().parallelism.is_serial());
        assert!(crate::batch::QueryOptions::default().parallelism.is_serial());
    }

    /// One worker **is** the serial path, so it is spelled that way rather
    /// than as a `Workers` of one nobody can tell from it; zero is a caller's
    /// own arithmetic and reads as one, exactly as `xz_seek::Bulk::new` reads
    /// it.
    #[test]
    fn one_worker_and_none_are_both_the_serial_state() {
        assert_eq!(Parallelism::workers(0, 512 << 20), Parallelism::Serial);
        assert_eq!(Parallelism::workers(1, 512 << 20), Parallelism::Serial);
        let eight = Parallelism::workers(8, 512 << 20);
        assert!(!eight.is_serial());
        assert_eq!(
            eight,
            Parallelism::Workers { jobs: NonZeroUsize::new(8).unwrap(), memory_bytes: 512 << 20 }
        );
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
        source.hint_parallelism(Parallelism::Serial);
        assert_eq!(source.pool.slots(), POOL_DEPTH, "serial states no number");
    }

    /// **Backpressure**: a loop that permits a wait gets one instead of
    /// allocating past the stated budget, which is what turns the slot count
    /// into a bound on what is *outstanding* rather than on what is idle.
    ///
    /// Two threads, because a wait has no behaviour except its interaction
    /// with another holder: with one it can only be asserted not to have
    /// blocked, which the unwaiting take already guaranteed. **No shipped read
    /// loop grants this permission**, so this is where the mechanism is
    /// exercised at all until `16.10.1`'s fused worker arrives.
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

    /// A plain file prefers no split point to another, and a partition costs
    /// one read chunk — the caller's own announced number where it announced
    /// one, which is the same value the pool sizes a slot by.
    #[test]
    fn a_plain_file_advises_anywhere_at_one_buffer_each() {
        let (_file, source) = source_of(b"0123456789abcdef");
        let advice = source.partitions(0..16);
        assert_eq!(advice.boundaries(), &PartitionBoundaries::Anywhere);
        assert_eq!(advice.max_partitions(), None);
        assert_eq!(advice.partition_bytes(), POOL_MAX_BYTES as u64);

        source.hint_read_size(4 << 20);
        assert_eq!(source.partitions(0..16).partition_bytes(), 4 << 20);
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

        // A partition holds one decoded block plus the chunk buffer a
        // straddling read is assembled into.
        let unit = source.blocks.as_ref().unwrap().unit as u64;
        assert_eq!(whole.partition_bytes(), unit + POOL_MAX_BYTES as u64);
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
        let block = |i: u64| xz_seek::BlockEntry {
            compressed_offset: 12 + i * 128,
            uncompressed_offset: i * 4096,
            unpadded_size: 64,
            uncompressed_size: 4096,
        };
        let table = xz_seek::SeekTable {
            compressed_file_size: 1 << 20,
            streams: vec![xz_seek::StreamEntry {
                compressed_offset: 0,
                uncompressed_offset: 0,
                compressed_size: 1 << 20,
                uncompressed_size: 4 * 4096,
                check: xz_seek::Check::Crc64,
                padding: 0,
                first_block: 0,
                block_count: 4,
            }],
            blocks: (0..4).map(block).collect(),
        };
        assert!(table.is_seekable(), "the table has boundaries to advise");

        let cache = BlockCache::for_table(&table).expect("4 KiB blocks decode whole");
        let decoding = XzSource::partition_advice(&table, Some(&cache), 1 << 20, 0..4 * 4096);
        assert_eq!(decoding.max_partitions(), Some(4));

        let streaming = XzSource::partition_advice(&table, None, 1 << 20, 0..4 * 4096);
        assert_eq!(streaming.max_partitions(), Some(1));
        assert_eq!(streaming.partition_bytes(), 1 << 20, "one chunk buffer, one reader");
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
                padding: 0,
                first_block: 0,
                block_count: 1,
            }],
            blocks: vec![block(uncompressed_size)],
        };
        // A cache exists for any file with blocks; whether the block path is
        // *taken* is the budget's answer, asked again on every read.
        let affordable = |unit: u64, budget: usize| {
            let cache = BlockCache::for_table(&table(unit)).expect("a block to build a unit from");
            cache.pool.set_limits(budget, POOL_DEPTH);
            cache.affordable()
        };
        assert!(affordable(24 << 20, DEFAULT_MEMORY_BUDGET as usize));
        // 128 and 192 MiB blocks — `xz --block-size=128MiB`, and `xz -9 -T0`,
        // whose block is three times its 64 MiB dictionary — are declined at
        // the default and taken once the caller allows the room.
        assert!(!affordable(128 << 20, DEFAULT_MEMORY_BUDGET as usize));
        assert!(!affordable(192 << 20, DEFAULT_MEMORY_BUDGET as usize));
        assert!(affordable(192 << 20, 512 << 20));
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
        one.hint_parallelism(Parallelism::Serial);
        assert_eq!(held(&one).2, POOL_DEPTH, "serial keeps the pool's own depth");
    }

    /// Two read units, one wait policy: the loop that reads a compressed
    /// source is the same loop for its chunk assembly and its block decode, so
    /// a permitted wait applies in both pools and a withheld one in neither.
    #[tokio::test]
    async fn a_compressed_source_states_the_policy_to_both_pools() {
        let payload = xz_test_payload();
        let compressed = xz_compress(&payload, &["--block-size=4096"]);
        let source = XzSource::open(compressed.path()).unwrap();
        let policies = |source: &XzSource| {
            (source.pool.policy(), source.blocks.as_ref().unwrap().pool.policy())
        };
        assert_eq!(policies(&source), (WaitPolicy::NeverWait, WaitPolicy::NeverWait));
        source.hint_wait_policy(WaitPolicy::MayWait);
        assert_eq!(policies(&source), (WaitPolicy::MayWait, WaitPolicy::MayWait));

        // And the block path still reads correctly while a wait is permitted:
        // `BlockCache::slot` evicts before it acquires, so a waiting reader
        // always finds the slot it is about to want.
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

        source.hint_parallelism(Parallelism::Serial);
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
}
