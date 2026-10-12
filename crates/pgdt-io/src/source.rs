//! The byte-range read every source implements, the vocabulary a reader and
//! a source exchange through it — identity and wait policy — the read size a
//! caller scales from, and beneath it the sources themselves: the plain file,
//! the `.xz` file pulled or fetched, the remote object, the origin a
//! `--source` argument resolves to, and the buffer pool every source but the
//! remote one reads into.

use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;

use pgdt_core::Result;

use crate::cancel::Cancellation;
use crate::parallelism::{Parallelism, WorkerMemory};
use crate::partition::Partitioning;

pub(crate) mod local;
mod origin;
pub(crate) mod pool;
#[cfg(feature = "http")]
mod remote;
#[cfg(test)]
mod tests;
pub(crate) mod xz;

pub use self::local::LocalFileSource;
pub use self::origin::{KnownCompression, Origin, OriginProbe, Recognized, open, open_local};
#[cfg(feature = "http")]
pub use self::remote::{REMOTE_READ_TIMEOUT, RemoteSource, open_remote};
pub use self::xz::{FetchedXzSource, XzSource, walk_seek_table};

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
    /// ([`walk_seek_table`](crate::walk_seek_table)).
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
    /// under `pgdt_dump::cache::StrictIdentity::time`, where a source that can
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
    /// read loop's `ScanOptions::chunk_size_bytes`, announced once before the loop
    /// starts.
    ///
    /// **Advisory, and it defaults to doing nothing.** One-off-ness is a
    /// property of the *caller* and nothing in a `read_range` call carries it,
    /// so a source that recycles buffers cannot otherwise tell a repeating
    /// chunk read from `map::attach_text`'s one coalesced span read. See
    /// `docs/design/decisions.md`, "D9".
    fn hint_read_size(&self, _len: usize) {}
    /// The xz seek table behind this source, for a caller building a cache
    /// envelope to persist alongside it (`pgdt_dump::internal::CompressionIndex`
    /// wraps this at save time). See `docs/design/decisions.md`, "D18".
    ///
    /// `None` by default, right for a source with no compression layer. Both
    /// compressed sources answer `Some`, and the table is either the one
    /// walked for at open ([`XzSource::open`](crate::XzSource::open), [`open_remote`](crate::open_remote)) or the one
    /// handed in from a cache ([`XzSource::with_table`](crate::XzSource::with_table),
    /// [`FetchedXzSource::with_table`](crate::FetchedXzSource::with_table)), which walks nothing at all —
    /// that saving being the point (`docs/design/decisions.md`, "D18").
    /// A cloned value rather than a borrow: a caller
    /// building a `CacheFile` needs to own it across an `await`.
    fn seek_table(&self) -> Option<xz_seek::SeekTable> {
        None
    }
    /// How this source would like `range` split across concurrent readers,
    /// and what one of those readers costs it resident.
    ///
    /// **Advisory, and the caller never learns what is underneath**: a local
    /// file answers "anywhere, a few chunk buffers each" and a compressed one
    /// "at these block boundaries, a block each".
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
    /// is the line [`BlockCache::affordable`](crate::source::xz::blocks::BlockCache::affordable) draws: a pool serving one reader
    /// still holds [`POOL_DEPTH`](crate::source::pool::POOL_DEPTH) slots. `None` for a source with no such path
    /// to take, which is every source but a compressed one.
    ///
    /// Whether the path was declined is read off
    /// [`ByteRangeSource::partitions`]; this is only the number the message
    /// needs. See `docs/design/decisions.md`, "D15".
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
    /// (`pgdt`'s `Discovered::resolve`).
    ///
    /// **It is a raw count, not a budgeted one**: what the byte budget affords
    /// is `worker_count`'s question, asked of every count
    /// alike. [`LocalFileSource`](crate::LocalFileSource) inherits the default; [`XzSource`](crate::XzSource) overrides
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
    /// caller on [`DEFAULT_MEMORY_BUDGET`](crate::DEFAULT_MEMORY_BUDGET), capped at the discovered allowance
    /// less [`MEMORY_RESERVE`](crate::MEMORY_RESERVE) where one was found. It is what keeps "no
    /// limit found" from meaning "serial", that constant affording no
    /// block-decoding reader (`docs/design/decisions.md`, "D3").
    ///
    /// **Nothing in the library budgets by it**, exactly as with the worker
    /// count: a plan's notes read it only to say whether a larger allowance
    /// would raise a budget.
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
    /// the numbers is its own business — [`LocalFileSource`](crate::LocalFileSource) sizes its one
    /// free list, [`XzSource`](crate::XzSource) divides the budget between its two read units
    /// and decides from it whether it can afford to decode a whole block at
    /// all. A [`Parallelism`] that states no byte count leaves a source on
    /// [`DEFAULT_MEMORY_BUDGET`](crate::DEFAULT_MEMORY_BUDGET).
    fn hint_parallelism(&self, _parallelism: Parallelism) {}
    /// How long the read loop about to start will hold the bytes it gets back
    /// — announced once, beside the other two hints, and only for a holder
    /// that drops each read before it takes the next
    /// (`docs/design/decisions.md`, "D5").
    ///
    /// **A source applies it only where the loop is the holder**: a wait on a
    /// pool the source retains from blocks on a slot the loop could never
    /// free, so [`XzSource`](crate::XzSource)'s block pool is left at
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
    /// `pgdt_dump::ScanOptions::cancelled` bounds a stop by tens of milliseconds
    /// and nothing in the source has to know. A source whose wait is a network
    /// request has no such bound and takes the signal so it can drop the
    /// request in flight (`docs/design/decisions.md`, "D26").
    fn hint_cancellation(&self, _cancel: Arc<Cancellation>) {}
    /// Where this source was fetched from and which version of the object it
    /// is reading — the half of a cache's identity a source reached over a
    /// network has and a file does not
    /// (`docs/design/decisions.md`, "D87").
    ///
    /// **`None` is a statement, not a gap**: a local cache records no origin,
    /// because its default path sits *beside* the dump, so the pairing is the
    /// filesystem's rather than a name we derived, and
    /// `pgdt_dump::cache::StrictIdentity::location` binds nothing there
    /// (`docs/design/decisions.md`, "D87").
    fn remote_identity(&self) -> Option<RemoteIdentity> {
        None
    }
    /// Whether a source that changes under an in-flight read must fail this
    /// run — announced by `pgdt_dump::cache::SourceWatch` beside the other hints,
    /// for a source that enforces it at a finer cadence than the watch's own.
    ///
    /// **Advisory, and it defaults to doing nothing**: a local file is
    /// re-checked by the watch itself, which holds the same answer. A source
    /// whose server can do the comparing pins the object on every request
    /// instead, and this is the only thing that turns that off
    /// (`docs/design/decisions.md`, "D21").
    ///
    /// **It is announced late, so the default must be the binding one.** A
    /// read taken before the watch opens — an origin probe, a cache claim —
    /// has never heard it.
    fn hint_in_flight_identity(&self, _binds: bool) {}
    /// Why nothing during a run can tell this source changed under it, or
    /// `None` where something can — asked by `pgdt_dump::cache::SourceWatch::open`,
    /// which refuses a run binding the in-flight identity over a source
    /// answering `Some` before it reads anything
    /// (`docs/design/decisions.md`, "D21").
    ///
    /// **The default is `None`: the watch's own re-read is the check**, which
    /// holds wherever [`ByteRangeSource::size`] and
    /// [`ByteRangeSource::modified`] answer live. A source whose two answers
    /// are frozen at open, and that cannot have its reads compared some other
    /// way, is the one that must say otherwise — the sentence it returns is
    /// the refusal's reason, naming what is missing rather than the source.
    fn in_flight_unchecked(&self) -> Option<String> {
        None
    }
    /// Compare the integrity check of every byte this source has already
    /// handed out and not yet compared — called once as a run's reading ends,
    /// before the run reports success (`pgdt_dump::cache::SourceWatch::finish`).
    ///
    /// **It exists because a check can trail its bytes.** A compressed block
    /// read in pieces carries one check over the whole block, so the bytes a
    /// read returned out of a block it stopped inside are compared only when
    /// the block is finished; a run whose last read stopped inside one would
    /// otherwise succeed on bytes that fail their own check. Finishing it
    /// decodes the rest of that block, CPU and no further read of the file
    /// beyond the block's own extent.
    ///
    /// **The default does nothing**, right for a source whose bytes carry no
    /// check of their own or whose every read compares before it returns. It
    /// is not a mid-run call: finishing a block a forward scan sits inside
    /// makes the scan's next read start that block again.
    fn complete_reads(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}

/// Where a source was fetched from and which version of it is being read:
/// [`ByteRangeSource::remote_identity`]'s answer, and the two fields a
/// remote cache records that a local one has no equivalent of
/// (`docs/design/decisions.md`, "D87").
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
    /// with no credential in it, [`Origin`](crate::Origin)'s own display form.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// The server's entity tag for this version of the object, `None` where
    /// it sent none. Opaque but for a weak tag's `W/`: it is compared, never
    /// otherwise parsed.
    pub fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }
}

/// Whether an entity tag is weak: `W/`, case-sensitively, before the quoted
/// tag (RFC 9110, §8.8.3). `object_store` hands the header over verbatim.
pub fn is_weak_tag(tag: &str) -> bool {
    tag.starts_with("W/")
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

/// Bytes requested per read from the source, unless a caller says otherwise.
///
/// Shipped, not probed (`docs/design/decisions.md`, "D10"): the default that
/// is worst-case-best across the device classes measured
/// (`docs/design/measurements.md`, "What the read chunk size is worth").
/// Public and named because it is what [`pgdt_dump::ScanOptions::chunk_size_bytes`] is
/// compared against and what callers scale from.
///
/// **Raising it costs memory, not pooling.** Every read loop announces the
/// size it is about to repeat ([`crate::ByteRangeSource::hint_read_size`]), so
/// a chunk of any size is kept and reused by the local source's buffer pool.
/// What a larger chunk costs is the pool holding four buffers of it, or the
/// caller's stated memory budget's worth ([`crate::Parallelism`], defaulting
/// to [`crate::DEFAULT_MEMORY_BUDGET`]), whichever is fewer — the slot count
/// falls out of that budget, so the cost levels off
/// (`docs/design/decisions.md`, "D9").
pub const SCAN_CHUNK_DEFAULT_SIZE_BYTES: usize = 1 << 20;
