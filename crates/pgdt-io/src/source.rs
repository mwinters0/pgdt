//! The byte-range read every source implements, the vocabulary a reader and
//! a source exchange through it — identity and wait policy — the read size a
//! caller scales from, and beneath it the sources themselves: the plain file,
//! the compressed file pulled or fetched, the remote object, the origin a
//! `--source` argument resolves to, and the buffer pool every source but the
//! remote one reads into.

use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::time::SystemTime;

use bytes::Bytes;
use tokio_util::sync::CancellationToken;

use pgdt_core::Result;

use crate::parallelism::{Parallelism, WorkerMemory};
use crate::partition::Partitioning;

pub(crate) mod compressed;
pub(crate) mod local;
mod origin;
pub(crate) mod pool;
#[cfg(feature = "http")]
mod remote;
#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-support"))]
pub use self::compressed::gzip_indexed_in_memory;
pub use self::compressed::{
    CompressedFileSource, CompressionIndex, FetchedCompressedSource, GzipIndex, SeekTable,
    WindowProvider,
};
pub use self::local::LocalFileSource;
pub use self::origin::{KnownCompression, Origin, OriginProbe, Recognized, open};
#[cfg(feature = "http")]
pub use self::remote::{REMOTE_READ_TIMEOUT, RemoteSource};

/// Minimal async byte-range read abstraction.
///
/// The three required methods mirror `object_store`'s `get_range`/`head`
/// semantics, `extent` answering `head`'s size where the end is known.
/// [`ByteRangeSource::hint_read_size`] sits outside that mirror and is
/// defaulted, so an implementation need not know it exists.
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
    /// ones — but a fetched range *starting* at or past the end, which the
    /// server refuses, an `Error::Remote`.
    ///
    /// **A driver that is told what to fetch relies on that.**
    /// `xz_seek::Walk` derives each request from the bytes of the one before
    /// it and reads a short supply as the file being shorter than the size it
    /// was constructed with, so a source answering a prefix would make a
    /// truncated file and a truncated *read* the same event (the footer walk
    /// over a fetched `.xz`, `source/compressed/xz.rs`'s `walk_seek_table`).
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>>;
    /// Up to `len` bytes at `offset`, **short only where the source ends
    /// first** — empty at or past its end. The read a loop reading to the
    /// source's end makes where [`ByteRangeSource::extent`] is
    /// [`Extent::Open`], so that loop learns the end from the read that
    /// meets it rather than from a size it cannot be told
    /// (`pgdt_dump`'s `ChunkedScanner`).
    ///
    /// **The default reads within the exact extent**, right for every source
    /// whose extent is always `Exact`; a source that can answer `Open` must
    /// answer this itself, and the default refuses to read one.
    fn read_at_most(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        Box::pin(async move {
            let end = self.extent().await?.require_exact("read_at_most")?;
            let len = (len as u64).min(end.saturating_sub(offset)) as usize;
            if len == 0 { Ok(Bytes::new()) } else { self.read_range(offset, len).await }
        })
    }
    /// How many addressable (decompressed, for a compressed source) bytes
    /// this source holds, as far as it knows: [`Extent::Exact`], or
    /// [`Extent::Open`] where its end has not been reached yet. A source
    /// answers `Exact` once a read has reached its end or a complete index
    /// was handed to it, and never `Open` again after that.
    ///
    /// **A type, not a number beside a flag**, so every site needing the end
    /// says what it does where there is none yet: a read loop reads to the
    /// end with [`ByteRangeSource::read_at_most`], anything cutting
    /// partitions requires `Exact`, and a cache records the extent as
    /// it was (`docs/design/decisions.md`, "D6").
    fn extent(&self) -> Pin<Box<dyn Future<Output = Result<Extent>> + Send + '_>>;
    /// Last-modified time, if the source exposes one — `object_store`'s
    /// `head` carries this too. `None` rather than an error for a source that
    /// genuinely has no notion of one; the structure cache treats an absent
    /// mtime as nothing to compare against rather than as a mismatch, except
    /// under `pgdt_dump::cache::StrictIdentity::time`, where a source that can
    /// offer no such signal is refused. See
    /// `docs/design/decisions.md`, "D21".
    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>>;
    /// Bytes as stored on the device — what a `stat` reports — as opposed to
    /// [`ByteRangeSource::extent`]'s addressable (possibly decompressed)
    /// length. This is the structure cache's staleness check. See
    /// `docs/design/decisions.md`, "D21".
    ///
    /// Defaults to the exact extent, right for a source that does not
    /// decompress, whose extent is always `Exact`; a decompressing source
    /// overrides it to the compressed file's own length.
    fn stored_size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        Box::pin(async move { self.extent().await?.require_exact("stored_size") })
    }
    /// The read length this caller is about to ask for over and over — a
    /// read loop's `ReadOptions::chunk_size_bytes`, announced once before the loop
    /// starts.
    ///
    /// **Advisory, and it defaults to doing nothing.** One-off-ness is a
    /// property of the *caller* and nothing in a `read_range` call carries it,
    /// so a source that recycles buffers cannot otherwise tell a repeating
    /// chunk read from `map::attach_text`'s one coalesced span read.
    fn hint_read_size(&self, _len: usize) {}
    /// The compression index behind this source, for a caller building a
    /// cache envelope to persist alongside it. See
    /// `docs/design/decisions.md`, "D18".
    ///
    /// `None` by default, right for a source with no compression layer. Every
    /// compressed source answers `Some` — a gzip file's first pass the index
    /// as far as it has got — and the index is otherwise either the one built
    /// at open ([`CompressedFileSource::open`](crate::CompressedFileSource::open),
    /// [`open`](crate::open) over a URL) or the one handed in from a cache
    /// ([`CompressedFileSource::with_index`](crate::CompressedFileSource::with_index),
    /// [`FetchedCompressedSource::with_index`](crate::FetchedCompressedSource::with_index)),
    /// which builds nothing at all — that saving being the point. A cloned
    /// value rather than a borrow: a caller building a `CacheFile` needs to
    /// own it across an `await`.
    fn compression_index(&self) -> Option<CompressionIndex> {
        None
    }
    /// How this source would like `range` split across concurrent readers,
    /// and what one of those readers costs it resident.
    ///
    /// **Advisory, and the caller never learns what is underneath**: a local
    /// file answers "anywhere, a few chunk buffers each" and a compressed one
    /// "at these block boundaries, a block each" where its budget
    /// block-decodes, and one partition where it does not.
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
    /// is the line [`BlockCache::affordable`](crate::source::compressed::blocks::BlockCache::affordable) draws: a pool serving one reader
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
    /// [`ByteRangeSource::partitions`] follows. The library reads it only
    /// where a caller states no count ([`ArrangementRequest::of`](crate::ArrangementRequest::of))
    /// — a caller that states one gets that count — for the layer above, which
    /// has a person's flags to fill in (`pgdt`'s `Discovered::resolve`).
    ///
    /// **It is a raw count, not a budgeted one**: what the byte budget affords
    /// is `worker_count`'s question, asked of every count
    /// alike. [`LocalFileSource`](crate::LocalFileSource) inherits the default;
    /// [`CompressedFileSource`](crate::CompressedFileSource) overrides it. See `docs/design/decisions.md`, "D1" and "D2".
    fn default_workers(&self) -> usize {
        1
    }

    /// What the workers of this source hold, where the caller has stated no
    /// budget of its own — a **recommendation**, never a bound, and the budget
    /// sibling of [`ByteRangeSource::default_workers`].
    ///
    /// **It is per worker rather than a total**: the only count a source knows
    /// is its own, so multiplying is the caller's
    /// ([`Arrangement::resolve`](crate::Arrangement::resolve), where the two meet).
    ///
    /// **The default is `None`: no recommendation at all**, which leaves a
    /// caller on [`DEFAULT_MEMORY_BUDGET`](crate::DEFAULT_MEMORY_BUDGET), capped at the discovered allowance
    /// less [`MEMORY_RESERVE`](crate::internal::MEMORY_RESERVE) where one was found. It is what keeps "no
    /// limit found" from meaning "serial", that constant affording no
    /// block-decoding reader (`docs/design/decisions.md`, "D3").
    ///
    /// **The arrangement carves by it** ([`ArrangementRequest::of`](crate::ArrangementRequest::of)),
    /// solving the worker count and the budget against it, and a plan's notes
    /// read it to say whether a larger allowance would raise a budget.
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
    /// free list, [`CompressedFileSource`](crate::CompressedFileSource) divides the budget between
    /// its two read units and decides from it whether it can afford to decode
    /// a whole unit at all. A [`Parallelism`] that states no byte count leaves a source on
    /// [`DEFAULT_MEMORY_BUDGET`](crate::DEFAULT_MEMORY_BUDGET).
    fn hint_parallelism(&self, _parallelism: Parallelism) {}
    /// How long the read loop about to start will hold the bytes it gets back
    /// — announced once, beside the other two hints, and only for a holder
    /// that drops each read before it takes the next
    /// (`docs/design/decisions.md`, "D5").
    ///
    /// **A source applies it only where the loop is the holder**: a wait on a
    /// pool the source retains from blocks on a slot the loop could never
    /// free, so [`CompressedFileSource`](crate::CompressedFileSource)'s block pool is left at
    /// [`WaitPolicy::NeverWait`] however a loop announces itself.
    ///
    /// **Advisory, and it defaults to doing nothing** — which is
    /// [`WaitPolicy::NeverWait`]'s behaviour.
    fn hint_wait_policy(&self, _policy: WaitPolicy) {}
    /// The caller's ask that reading stop, announced beside the other hints
    /// for a source whose own wait needs it.
    ///
    /// **Advisory, and it defaults to doing nothing**, which is right for
    /// every source whose wait is a `pread`: the mapping pass's own poll of
    /// the token bounds a stop by tens of milliseconds and nothing in the
    /// source has to know. A source whose wait is a network request has no
    /// such bound and keeps the token so it can drop the request in flight
    /// (`docs/design/decisions.md`, "D26").
    fn hint_cancellation(&self, _cancel: CancellationToken) {}
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
    /// `None` where something can — asked by
    /// `pgdt_dump::cache::SourceWatch::require_checkable`, which refuses a run
    /// binding the in-flight identity over a source answering `Some` before it
    /// reads anything
    /// (`docs/design/decisions.md`, "D21").
    ///
    /// **The default is `None`: the watch's own re-read is the check**, which
    /// holds wherever [`ByteRangeSource::stored_size`] and
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

/// How many addressable bytes a source holds, as far as it knows
/// ([`ByteRangeSource::extent`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Extent {
    /// Exactly this many bytes.
    Exact(u64),
    /// At least this many, the end not reached yet: a stream whose length
    /// only a decode to its end tells, before one has.
    Open { at_least: u64 },
}

impl Extent {
    /// The exact length, `None` while the end is not known.
    pub fn exact(self) -> Option<u64> {
        match self {
            Self::Exact(len) => Some(len),
            Self::Open { .. } => None,
        }
    }

    /// Whether a scan that has read through `through` has read everything:
    /// never while the end is not known.
    pub fn reached_by(self, through: u64) -> bool {
        self.exact().is_some_and(|len| through >= len)
    }

    /// The exact length, or an error naming `what` needed it — for a caller
    /// whose source answering `Open` breaks a contract, never one a user
    /// reaches.
    pub fn require_exact(self, what: &str) -> Result<u64> {
        self.exact().ok_or_else(|| {
            pgdt_core::Error::Io(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!("`{what}` needs the source's exact extent, and its end is not known yet"),
            ))
        })
    }
}

/// The length as a status line states it: the number, or `at least` it.
impl std::fmt::Display for Extent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exact(len) => write!(f, "{len}"),
            Self::Open { at_least } => write!(f, "at least {at_least}"),
        }
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
/// Public and named because it is what [`pgdt_dump::ReadOptions::chunk_size_bytes`]
/// defaults to and what callers scale from.
///
/// **Raising it costs memory, not pooling.** Every read loop announces the
/// size it is about to repeat ([`crate::ByteRangeSource::hint_read_size`]), so
/// a chunk of any size is kept and reused by the local source's buffer pool.
/// What a larger chunk costs is the pool holding four buffers of it, or the
/// caller's stated memory budget's worth ([`crate::Parallelism`], defaulting
/// to [`crate::DEFAULT_MEMORY_BUDGET`]), whichever is fewer, and never fewer
/// than one — the slot count
/// falls out of that budget, so the cost levels off
/// (`docs/design/decisions.md`, "D10").
pub const SCAN_CHUNK_DEFAULT_SIZE_BYTES: usize = 1 << 20;
