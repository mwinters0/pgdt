//! On-disk structure cache: a serialized [`DumpIndex`], colocated with the
//! dump file by default or at an explicit path
//! (`docs/design/architecture.md`, "The cache").
//!
//! Reading is best-effort — the cache is never required for correctness, so
//! a missing, foreign or unrecognised-version file just means "scan instead,"
//! never a hard error. Which of the three it was is still reported — see
//! [`CacheStatus`], and [`CacheLoad`] for the same statuses reaching a caller
//! that holds a live source — because `pgdq info` has no "scan instead" to
//! fall back on and has to say what went wrong.
//!
//! **The size-mismatched file is the exception, and it is not about reading.**
//! A cache whose recorded stored size is not this source's is a cache that
//! describes some *other* file, and scanning past it means overwriting it
//! within the first throttled save. The three scan entry points therefore
//! refuse ([`Error::CacheSourceMismatch`]) rather than starting cold: reading
//! it stays unusable, and being unusable to read stops licensing a write
//! (`docs/design/architecture.md`, "The cache").
//! Writing is not best-effort: [`save`] propagates I/O failures rather than
//! silently falling back to running without a cache, since a write failure
//! (read-only mount, permissions, disk full) means something is actually
//! wrong and swallowing it would silently degrade every future run of the
//! same command back to a full scan.
//!
//! **The dump file's identity is checked, not assumed**
//! (`docs/design/architecture.md`, "The cache"). Every cache records the
//! source's *stored* size and mtime as observed at save time — bytes on the
//! device, not the addressable (possibly decompressed) length
//! (`docs/design/architecture.md`, "The cache"); [`load`]
//! re-observes the live source and compares. A stored-size mismatch means
//! every byte offset in the cache could be wrong, so the cache is unusable
//! the way a foreign or wrong-version file is — and, unlike those, it is
//! reported to a scanning caller as an error rather than started cold past,
//! per the paragraph above. An mtime
//! mismatch is weaker
//! evidence (mtime granularity and preservation vary too much across
//! filesystems to be conclusive) and does **not** invalidate the cache; it
//! is surfaced on [`CacheStatus::Valid`], and [`CacheMode::load`] turns it
//! into a [`crate::diagnostic::DiagnosticKind::CacheMtimeChanged`] on the
//! loaded index rather than acting on it. That diagnostic is recomputed on
//! every load and never persisted — the mismatch is between the cache and
//! *this* run's observation, so a stored one would be a warning about a
//! check that has since passed.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::diagnostic::Diagnostic;
use crate::index::{
    DumpIndex, non_seekable_compression_diagnostic, tiling_diagnostics, toc_coverage_diagnostic,
};
use crate::io::{ByteRangeSource, KnownCompression};
use crate::{Error, Result};

/// Bumped whenever the on-disk shape changes incompatibly. A cache written
/// under a different version is unusable ([`CacheStatus::UnsupportedVersion`])
/// rather than partially trusted (`docs/design/roadmap.md`,
/// "Four decisions that keep later phases additive") —
/// the three fields reserved on [`DumpIndex`]/[`crate::index::CopyBlock`] are
/// what let most future additions avoid needing a bump at all.
///
/// Pre-1.0, a bump is free and nothing migrates (`CLAUDE.md`, "Pre-1.0"), so
/// the rule is simply: **bump whenever a persisted field is added, removed or
/// reshaped.** `git log -p` on this constant is the version history.
///
/// What does *not* need a bump: a new way of *reading* an existing on-disk
/// shape. [`CacheStatus::Incomplete`] is the worked example — it reinterprets
/// `scanned_through` against a size already stored, changing nothing on
/// disk.
const FORMAT_VERSION: u32 = 17;

/// The dump file's identity as observed when a cache was last saved — see
/// the module docs.
///
/// **Opaque, not a struct** (`docs/design/architecture.md`, "The cache"):
/// a remote source has no mtime at all — an ETag is not a
/// `SystemTime` — so a future variant carries whatever evidence its own kind
/// of source actually has, rather than every source being forced through one
/// shared shape. One variant today; a call site reads the fields through the
/// variant it matches, since the two kinds of identity this project will
/// eventually have share no fields worth naming generically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum SourceIdentity {
    /// Backed by [`ByteRangeSource::stored_size`]/`modified` — every source
    /// today, decompressing or not, since a decompressing source still sits
    /// on top of bytes with an on-disk size and (usually) an mtime.
    LocalFile {
        stored_size: u64,
        /// `(seconds, nanoseconds)` since the Unix epoch — `SystemTime`
        /// itself isn't `Serialize`, and this is L1's own on-disk vocabulary
        /// rather than borrowing `std`'s. `None` when the source exposed no
        /// mtime.
        mtime: Option<(u64, u32)>,
    },
}

impl SourceIdentity {
    async fn observe(source: &dyn ByteRangeSource) -> Result<Self> {
        let stored_size = source.stored_size().await?;
        let mtime = source
            .modified()
            .await?
            .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default())
            .map(|d| (d.as_secs(), d.subsec_nanos()));
        Ok(SourceIdentity::LocalFile { stored_size, mtime })
    }
}

/// What produced the indexed blocks' byte offsets. Plain-format offsets are
/// raw file positions; a future archive format's (`docs/design/roadmap.md`,
/// P8 Track B)
/// are entry-relative, so the two must never be silently conflated.
///
/// A compressed source's indexed offsets are *also* plain-format offsets —
/// byte for byte what a plain scan of the same decompressed content
/// produces — so a compressed source never sets this to anything but
/// `Plain`; what changes is [`CompressionIndex`], a sibling field rather
/// than a value of this one (`docs/design/architecture.md`, "The cache").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum ContainerKind {
    Plain,
}

/// What compression sits between this cache's plain-format offsets and the
/// bytes on disk — `None` for a source that needs no such index
/// (`docs/design/architecture.md`, "The cache"). A sibling of
/// [`ContainerKind`], not a value of it: see that type's docs.
///
/// Built at [`save`] time from [`ByteRangeSource::seek_table`], which
/// [`crate::XzSource`] answers from the table it already built while
/// opening — persisting a cache never re-walks the file to get this — and read
/// back by [`claim`] before any source exists, which is what
/// spares every command after the first one that walk. P15's
/// gzip index is a different *shape*, not a variant of this one — a set of
/// decoder checkpoints for a single-member file, and for a multi-member one a
/// member list that resembles a block list without being `xz_seek`'s type — so
/// it gets its own sibling variants here rather than trying to fit this one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum CompressionIndex {
    Xz(xz_seek::SeekTable),
}

/// The shape of the container a cache's offsets sit under, read off the
/// persisted [`CompressionIndex`].
///
/// **Three numbers, and each answers a question the user is otherwise sent to
/// `xz --list` for.** `max_block_uncompressed` is the largest term of what a
/// memory budget is compared against — twice it, the block path holding one
/// block while it decodes the next, plus the chunk buffer and the decoder's
/// own retention — so it is most of the number to raise `--parallel-memory` to
/// when a query says the block path was declined, and that query's own note
/// states the whole of it
/// (`crate::PlanNoteKind::CompressedBlockPathDeclined`); `blocks` is how much
/// seeking the file offers at all; `streams` is what explains a slow first
/// command, a concatenated file costing one seek per stream to walk
/// (`docs/design/architecture.md`, "The compressed source").
///
/// **Derived, not stored.** It is a projection of the seek table the envelope
/// already carries, computed on load like the diagnostics beside it, so a
/// cache written before this existed reports it too and nothing about the
/// on-disk shape changed. It is a property of the *file*, which is why a
/// cache can answer it with no dump present — unlike a budget decline, which
/// depends on the run and reaches a caller through
/// `crate::PlanNoteKind::CompressedBlockPathDeclined` instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CompressionShape {
    /// The container: `"xz"` today, and the only value there is. A word
    /// rather than an enum with one variant, because what a reader wants from
    /// it is the name to print beside the numbers.
    pub container: &'static str,
    pub streams: usize,
    pub blocks: usize,
    pub max_block_uncompressed: u64,
}

impl CompressionShape {
    fn of(index: &CompressionIndex) -> Self {
        let CompressionIndex::Xz(table) = index;
        Self {
            container: "xz",
            streams: table.stream_count(),
            blocks: table.block_count(),
            max_block_uncompressed: table.max_block_uncompressed(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFile {
    format_version: u32,
    container_kind: ContainerKind,
    compression: Option<CompressionIndex>,
    identity: SourceIdentity,
    /// The addressable (decompressed, for a compressed source) length —
    /// [`ByteRangeSource::size`] as observed at save time. Its own field
    /// rather than an alias of `identity`'s stored size
    /// (`docs/design/architecture.md`, "The cache"): the two
    /// diverge for a compressed source, and this is the number
    /// [`CacheStatus::Valid`]/[`CacheStatus::Incomplete`] hand out as
    /// `total_size` for the coverage arithmetic to read.
    total_size: u64,
    index: DumpIndex,
}

/// The default cache location when no explicit path is given:
/// `<dump-path>.dqcache`.
pub fn colocated_path(dump_path: &Path) -> PathBuf {
    let mut path = dump_path.as_os_str().to_owned();
    path.push(".dqcache");
    PathBuf::from(path)
}

/// What [`load`] found at a cache path, once checked against the live
/// source's identity — see the module docs.
///
/// **The four unusable outcomes are named separately, not collapsed**, and
/// they stay named all the way to the caller: [`CacheMode::load`] carries each
/// one across as a [`CacheLoad`] variant rather than answering `None`. A caller
/// that cannot scan — `pgdq info`, which reports from the cache and never reads
/// the dump — has a different sentence to say for each, and "you have never
/// parsed this file" is not the same fact as "your file changed since you
/// parsed it" even though both end in `pgdq parse`. A caller that *can* scan
/// has the reason in hand before it does any work, which is where the decision
/// about what to do with the file at that path belongs
/// (`docs/design/architecture.md`, "The cache").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheStatus {
    /// Nothing at this path.
    Missing,
    /// Something is at this path, but it does not decode as a cache at all —
    /// foreign bytes, or a truncated write.
    Unreadable,
    /// A cache written by a build whose on-disk shape this one does not
    /// recognise (`format_version`/`container_kind`). Pre-1.0 these are free
    /// and frequent, and nothing migrates
    /// (`docs/design/roadmap.md`, "Pre-1.0").
    UnsupportedVersion,
    /// A readable cache whose recorded *stored* size disagrees with the live
    /// source's, so every byte offset in it could be wrong — the staleness
    /// check now reads [`ByteRangeSource::stored_size`], not the addressable
    /// length (`docs/design/architecture.md`, "The cache"). Both
    /// stored sizes are carried because "the file changed" is the fact a
    /// reporting caller states, and the two numbers are the evidence for it.
    SourceChanged { cached_stored_size: u64, live_stored_size: u64 },
    /// A usable cache whose `index.scanned_through` reaches the file's
    /// recorded size — the whole file is mapped. `mtime_changed` is `true`
    /// when the source's current mtime differs from the one recorded at save
    /// time — weaker evidence than a stored-size mismatch (see the module
    /// docs), so it does not itself make the cache unusable. `total_size` is
    /// [`CacheFile::total_size`], carried for the same reason
    /// [`Incomplete`](CacheStatus::Incomplete) carries it: a reporting caller
    /// states coverage against it, and a cache-only caller has no live source
    /// to stat.
    /// `compression` is the container's shape where one sits under these
    /// offsets, and `None` for a plain file — read off the persisted seek
    /// table, so a cache-only caller answers it with no dump present
    /// ([`CompressionShape`]).
    Valid {
        index: DumpIndex,
        mtime_changed: bool,
        total_size: u64,
        compression: Option<CompressionShape>,
    },
    /// A usable cache whose `index.scanned_through` falls short of
    /// `total_size` — a real, not-yet-finished scan (e.g. a preamble-only
    /// scan, or a query that stopped once its target settled), not a defect
    /// (`docs/design/architecture.md`, "The cache"). What "not enough"
    /// means is caller-specific: `pgdq info` reports whatever this holds and
    /// states the coverage (`docs/design/architecture.md`, "CLI surface"),
    /// while a caller that resumes an incremental scan from wherever it left
    /// off (`crate::stream::map_file`, `crate::stream::table_stream`,
    /// `crate::index::preamble_only`) wants exactly this partial index to
    /// build on, the same as [`Valid`](CacheStatus::Valid) —
    /// [`CacheMode::load`] treats it that way. `total_size` is
    /// [`CacheFile::total_size`], which — once this case or `Valid` is
    /// reached — is already known to equal the live source's addressable
    /// length when one is available, so it serves a cache-only caller (no
    /// live source to stat) the same way it serves a live one.
    /// `compression` is [`Valid`](CacheStatus::Valid)'s, and means the same
    /// thing: a partial scan says as much about the container it read through
    /// as a finished one does.
    Incomplete {
        index: DumpIndex,
        mtime_changed: bool,
        total_size: u64,
        compression: Option<CompressionShape>,
    },
}

/// What [`CacheMode::load`] answered a caller that holds a live source: an
/// index to build forward from, or the reason there is none
/// (`docs/design/architecture.md`, "The cache").
///
/// **The reason is not a detail of the cache file alone.** `Disabled` is a
/// statement about the *caller* — no path was consulted, because the caller
/// opted out — and has no [`CacheStatus`] to correspond to; the other four are
/// that status carried across unchanged. A caller therefore answers five
/// outcomes, and answering them separately is the point: they are alike only
/// for a caller that has already decided to scan regardless.
///
/// **`Valid` and `Incomplete` are one variant here**, deliberately, because a
/// caller holding a live source builds forward from either — see
/// [`CacheMode::load`].
///
/// *Rejected:* handing back [`CacheStatus`] itself. It splits the usable
/// outcomes this method exists to treat alike, and it cannot express
/// `Disabled` at all, so every caller would match arms this method never
/// produces and miss one it does.
///
/// *Rejected:* keeping `Option<DumpIndex>` and adding a second, reporting
/// method beside it. The collapsing one stays the shorter call, which is how
/// the reason ended up out of reach of the scan entry points in the first
/// place.
///
/// *Rejected:* an accessor collapsing the four reasons back to `Option` for
/// the callers that do not care. It rebuilds the collapse under a shorter name
/// at exactly the sites that must not have it, and the callers that genuinely
/// do not care are tests, which can say so in a `let … else`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheLoad {
    /// A usable index — a [`CacheStatus::Valid`] or
    /// [`CacheStatus::Incomplete`] cache, carrying whatever diagnostics the
    /// load computed about the cache *file*.
    Index(DumpIndex),
    /// [`CacheMode::Disabled`]: no path was consulted, and whatever sits at
    /// the location this mode would otherwise have resolved is untouched.
    Disabled,
    /// [`CacheStatus::Missing`] — nothing at the cache path.
    Missing,
    /// [`CacheStatus::Unreadable`] — foreign bytes, or a truncated write.
    Unreadable,
    /// [`CacheStatus::UnsupportedVersion`] — another build's envelope.
    UnsupportedVersion,
    /// [`CacheStatus::SourceChanged`] — the cache records a stored size the
    /// live source does not have, so every byte offset in it could be wrong.
    /// Both numbers travel, because "this cache is not about this file" is a
    /// claim a caller has to be able to state the evidence for.
    ///
    /// **The three scan entry points refuse on this one**, where they start
    /// cold on the other three: a cache that describes another file is data
    /// this library does not replace on its own
    /// (`docs/design/architecture.md`, "The cache"). The two sizes are what
    /// [`CacheMode::source_mismatch`] turns into
    /// [`Error::CacheSourceMismatch`].
    SourceChanged { cached_stored_size: u64, live_stored_size: u64 },
}

/// Load a cache from `path` and validate it against `source`'s current
/// identity. See [`CacheStatus`] and the module docs for what "validate"
/// means. A hard I/O error reading `path` (anything but "not found") still
/// propagates — only the cache's own *content* is best-effort.
pub async fn load(path: &Path, source: &dyn ByteRangeSource) -> Result<CacheStatus> {
    let file = match read_cache_file(path)? {
        Ok(file) => file,
        Err(status) => return Ok(status),
    };
    let live = SourceIdentity::observe(source).await?;
    // One variant today, so both patterns are irrefutable; a future variant
    // (P14's `Remote`) is matched explicitly rather than through a shared
    // accessor — see `SourceIdentity`'s docs.
    let SourceIdentity::LocalFile { stored_size: cached_stored_size, mtime: cached_mtime } =
        file.identity;
    let SourceIdentity::LocalFile { stored_size: live_stored_size, mtime: live_mtime } = live;
    if cached_stored_size != live_stored_size {
        return Ok(CacheStatus::SourceChanged { cached_stored_size, live_stored_size });
    }
    let mtime_changed = cached_mtime != live_mtime;
    Ok(status_from_file(file, mtime_changed))
}

/// Read and envelope-check the cache at `path`, shared by [`load`] and
/// [`load_offline`]. `Err(status)` is one of the three unusable outcomes that
/// need no live source to reach; only a stored-size mismatch does, and that
/// is [`load`]'s alone.
fn read_cache_file(path: &Path) -> Result<std::result::Result<CacheFile, CacheStatus>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Err(CacheStatus::Missing));
        }
        Err(e) => return Err(Error::Io(e)),
    };
    let file: CacheFile =
        match bincode::serde::decode_from_slice(&bytes, bincode::config::standard()) {
            Ok((file, _)) => file,
            Err(_) => return Ok(Err(CacheStatus::Unreadable)),
        };
    if file.format_version != FORMAT_VERSION || file.container_kind != ContainerKind::Plain {
        return Ok(Err(CacheStatus::UnsupportedVersion));
    }
    Ok(Ok(file))
}

/// What the cache at a path settles about a dump file **before any source
/// exists** — [`claim`]'s answer (`docs/design/architecture.md`, "The
/// compressed source").
///
/// **Two outcomes, because one of them is worth more than knowledge about
/// compression.** Everything a caller can do with a cache early is decide
/// which source to build — except for the one unusable outcome that makes the
/// building itself pointless: a cache recorded against a file of another
/// stored size is refused by every scan entry point
/// ([`Error::CacheSourceMismatch`]) and reported by every caller that cannot
/// scan, so opening the source first buys nothing and, for a many-streams
/// `.xz`, costs a stream-footer walk to reach a verdict that was already on
/// disk ("The cache").
///
/// The other unusable outcomes stay collapsed into
/// [`KnownCompression::Unknown`]: no cache, foreign bytes, a version this
/// build does not read. Nothing downstream refuses on those — the caller
/// scans, and [`load`] states the reason a moment later — so naming them here
/// would put a second authority in front of the one that reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheClaim {
    /// What the cache says about the file's compression layer: the table a
    /// previous walk produced, "no compression layer", or nothing known.
    Compression(KnownCompression),
    /// The cache records a stored size this file does not have, so it
    /// describes some *other* file. The same condition [`load`] answers
    /// [`CacheStatus::SourceChanged`] with, and the same two numbers, reached
    /// without opening anything.
    SourceChanged { cached_stored_size: u64, live_stored_size: u64 },
}

/// What the cache at `cache_path` settles about the file at `dump_path`,
/// answered **before any source exists**
/// (`docs/design/architecture.md`, "The compressed source").
///
/// This is the half of the cache a caller needs *early*: recognition decides
/// which source to build, and for an `.xz` file it either walks the stream
/// footers or is handed the table a previous walk already produced. There is
/// no source yet to give [`load`], so identity is checked here against a
/// plain `stat` on `dump_path` — the same stored-size rule [`load`] applies,
/// with a differing mtime again too weak to invalidate anything. That one
/// comparison lives here rather than at the caller, which is what keeps the
/// early refusal the same verdict as the late one rather than a second
/// authority over it.
///
/// *Rejected:* stopping the decode short of the index, since `compression`
/// and `identity` do sit ahead of it in the envelope — bincode is positional,
/// so reaching them means decoding what precedes them anyway, and the cost of
/// decoding the envelope twice is bounded by the index's own size.
///
/// *Rejected:* leaving this answering [`KnownCompression`] alone and adding a
/// sibling that reports the stored size. Both would decode the envelope, so
/// the *usable* path — the one that matters, since it is the one a warm koji
/// run takes — would decode a 31,150-entry seek table twice to spare a walk on
/// the path that is about to fail.
pub fn claim(cache_path: &Path, dump_path: &Path) -> Result<CacheClaim> {
    let Ok(file) = read_cache_file(cache_path)? else {
        return Ok(CacheClaim::Compression(KnownCompression::Unknown));
    };
    // One variant today; a future one is matched through the variant rather
    // than a shared accessor — see [`SourceIdentity`].
    let SourceIdentity::LocalFile { stored_size, .. } = file.identity;
    // A dump path that cannot be stat'd is left to the open that follows,
    // which is where that failure has a sentence to say.
    let Ok(live) = std::fs::metadata(dump_path) else {
        return Ok(CacheClaim::Compression(KnownCompression::Unknown));
    };
    if stored_size != live.len() {
        return Ok(CacheClaim::SourceChanged {
            cached_stored_size: stored_size,
            live_stored_size: live.len(),
        });
    }
    Ok(CacheClaim::Compression(match file.compression {
        Some(CompressionIndex::Xz(table)) => KnownCompression::Xz(table),
        None => KnownCompression::Plain,
    }))
}

/// Load a cache from `path` with no live source to check it against — the
/// cache-only counterpart to [`load`]
/// (`docs/design/architecture.md`, "The cache"). There is nothing to compare the recorded mtime to, so
/// `mtime_changed` is always `false` here; the "this is unverified,
/// historical data" fact cache-only mode carries instead is a
/// [`crate::diagnostic::DiagnosticKind::CacheOffline`] pushed by
/// [`CacheMode::load_offline`], not this flag.
pub async fn load_offline(path: &Path) -> Result<CacheStatus> {
    match read_cache_file(path)? {
        Ok(file) => Ok(status_from_file(file, false)),
        Err(status) => Ok(status),
    }
}

/// Shared by [`load`] and [`load_offline`] once a `CacheFile` has passed its
/// format/container/(when live) stored-size checks: `Valid` when the index's
/// own `scanned_through` reaches the file's recorded addressable length,
/// `Incomplete` otherwise, with the index's unpersisted diagnostics
/// recomputed either way. Reads [`CacheFile::total_size`] rather than
/// re-deriving one from a live source — by the time either caller reaches
/// this point the stored sizes are already known equal wherever a live one
/// exists (a live mismatch returns [`CacheStatus::SourceChanged`] earlier in
/// [`load`]), and `load_offline` has no live source to read at all.
fn status_from_file(file: CacheFile, mtime_changed: bool) -> CacheStatus {
    let total_size = file.total_size;
    let mut index = file.index;
    // `DumpIndex::diagnostics` is `#[serde(skip)]`, so a loaded index arrives
    // with none. Both file-level figures are pure functions of the spans and
    // O(spans) to compute, which is what lets them be recomputed on load
    // rather than persisted (`crate::diagnostic`, "Not persisted") — and a
    // caller that reports from a cache without ever scanning (`pgdq info`)
    // would otherwise silently lose the TOC-coverage figure.
    index.diagnostics = tiling_diagnostics(&index.spans, total_size);
    index.diagnostics.push(toc_coverage_diagnostic(&index.spans));
    let table = file.compression.as_ref().map(|CompressionIndex::Xz(table)| table);
    index.diagnostics.extend(non_seekable_compression_diagnostic(table));
    // Derived here for the same reason the diagnostics above are: it is a
    // projection of what the envelope already holds, so a cache written
    // before it existed answers it too.
    let compression = file.compression.as_ref().map(CompressionShape::of);
    if index.is_complete(total_size) {
        CacheStatus::Valid { index, mtime_changed, total_size, compression }
    } else {
        CacheStatus::Incomplete { index, mtime_changed, total_size, compression }
    }
}

/// Write `index` to `path` (colocated or explicit — whichever the caller
/// resolved), overwriting any existing cache there, and record `source`'s
/// current stored size/mtime plus its addressable length for [`load`] to
/// check next time. Propagates I/O failures as `Error::Io` rather than
/// swallowing them — see the module docs.
pub async fn save(path: &Path, source: &dyn ByteRangeSource, index: &DumpIndex) -> Result<()> {
    let identity = SourceIdentity::observe(source).await?;
    let total_size = source.size().await?;
    let file = CacheFile {
        format_version: FORMAT_VERSION,
        container_kind: ContainerKind::Plain,
        compression: source.seek_table().map(CompressionIndex::Xz),
        identity,
        total_size,
        index: index.clone(),
    };
    let bytes = bincode::serde::encode_to_vec(&file, bincode::config::standard())?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// How a caller wants the structure cache handled for one operation.
/// Constructed via [`CacheMode::resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheMode {
    /// Read/write the cache at this path (the colocated default, or an
    /// explicit path).
    Enabled(PathBuf),
    /// The caller explicitly opted out: any existing file at what would
    /// otherwise be the resolved location is ignored, and a fresh scan is
    /// not persisted.
    Disabled,
    /// No live dump source at all — answer strictly from the cache at this
    /// path (`docs/design/architecture.md`, "The cache"). Never constructed by [`CacheMode::resolve`]; a caller
    /// builds it directly (`pgdq info` with no `--source`). Rejected by
    /// every method below that takes a live `source` — being handed
    /// `Offline` while a live source is in hand is a caller contract
    /// violation, not a degenerate case to tolerate — and [`load_offline`]
    /// is its own counterpart, rejecting `Enabled`/`Disabled` the other way.
    Offline(PathBuf),
}

impl CacheMode {
    /// Resolve a `--cache-path`-style argument against `dump_path`: `None`
    /// selects the colocated default (`<dump_path>.dqcache`), the literal
    /// path `none` disables the cache, and any other path is used as-is.
    pub fn resolve(dump_path: &Path, cache_path: Option<&Path>) -> CacheMode {
        match cache_path {
            None => CacheMode::Enabled(colocated_path(dump_path)),
            Some(p) if p == Path::new("none") => CacheMode::Disabled,
            Some(p) => CacheMode::Enabled(p.to_path_buf()),
        }
    }

    /// Load the cache this mode points at, discarding the
    /// [`CacheStatus::Valid::mtime_changed`] bit (see the module docs — a
    /// caller that wants it can call [`load`] directly instead). A disabled
    /// cache always yields `Ok(None)` — even if a file happens to sit at
    /// what would otherwise be its resolved location, per
    /// [`CacheMode::Disabled`]'s contract that existing files are ignored,
    /// never read.
    ///
    /// `Incomplete` is treated exactly like `Valid` — both are
    /// [`CacheLoad::Index`] — because every caller of this method
    /// (`crate::stream::map_file`, `crate::stream::table_stream`,
    /// `crate::index::preamble_only`) wants whatever partial map already
    /// exists to build forward from. Answering "no index" for `Incomplete`
    /// here would make every partial cache invisible to the next scan, which is
    /// exactly the incremental caching `docs/design/architecture.md`,
    /// "Query: mapping and streaming are separate passes" depends on. A
    /// caller that instead *reports* what a cache holds reaches for
    /// [`load`]/[`CacheMode::load_offline`] and the full [`CacheStatus`],
    /// which is what `pgdq info` does.
    ///
    /// **The four unusable statuses are not collapsed**, and neither is the
    /// caller's own opt-out: each arrives as its own [`CacheLoad`] variant, so
    /// a caller about to scan holds the reason before it does any work rather
    /// than after (`docs/design/architecture.md`, "The cache").
    pub async fn load(&self, source: &dyn ByteRangeSource) -> Result<CacheLoad> {
        match self {
            CacheMode::Enabled(path) => Ok(match load(path, source).await? {
                CacheStatus::Missing => CacheLoad::Missing,
                CacheStatus::Unreadable => CacheLoad::Unreadable,
                CacheStatus::UnsupportedVersion => CacheLoad::UnsupportedVersion,
                CacheStatus::SourceChanged { cached_stored_size, live_stored_size } => {
                    CacheLoad::SourceChanged { cached_stored_size, live_stored_size }
                }
                CacheStatus::Valid { mut index, mtime_changed, .. }
                | CacheStatus::Incomplete { mut index, mtime_changed, .. } => {
                    // Reported rather than acted on: too weak to invalidate
                    // (see the module docs), and pointless to persist — the
                    // mismatch is between the cache and *this* run's
                    // observation, so it is recomputed on every load.
                    if mtime_changed {
                        index.diagnostics.push(Diagnostic::cache_mtime_changed());
                    }
                    CacheLoad::Index(index)
                }
            }),
            CacheMode::Disabled => Ok(CacheLoad::Disabled),
            CacheMode::Offline(_) => Err(Error::CacheModeMismatch(
                "a live dump source requires CacheMode::Enabled or CacheMode::Disabled, not Offline",
            )),
        }
    }

    /// The refusal a scan entry point answers [`CacheLoad::SourceChanged`]
    /// with: this mode's path, plus the two stored sizes the load already
    /// compared (`docs/design/architecture.md`, "The cache"). The path lives
    /// here rather than on `CacheLoad` because [`CacheMode::Disabled`] has
    /// none, and the mode is what every one of those callers holds anyway.
    ///
    /// Only [`CacheMode::Enabled`] can reach it: `Disabled` consults no path
    /// and answers [`CacheLoad::Disabled`], and `Offline` is refused by
    /// [`CacheMode::load`] before any status is read. Both therefore answer
    /// the caller-contract error rather than a plausible sentence about a file
    /// nothing looked at — the same treatment `load` gives `Offline`.
    pub fn source_mismatch(&self, cached_stored_size: u64, live_stored_size: u64) -> Error {
        match self {
            CacheMode::Enabled(path) => Error::CacheSourceMismatch {
                path: path.clone(),
                cached_stored_size,
                live_stored_size,
            },
            CacheMode::Disabled | CacheMode::Offline(_) => Error::CacheModeMismatch(
                "a source mismatch is only reachable from CacheMode::Enabled",
            ),
        }
    }

    /// Load this mode's cache with no live source to check it against — the
    /// cache-only counterpart to [`CacheMode::load`]
    /// (`docs/design/architecture.md`, "The cache"). Unlike `load`, this returns the full [`CacheStatus`]
    /// rather than a [`CacheLoad`]: cache-only mode has no scan to fall back
    /// on, so a caller needs to tell `Valid` apart from
    /// `Incomplete` to decide whether it has enough to answer from — the one
    /// distinction `load` exists to erase. Every
    /// successful load (`Valid` or `Incomplete`) gets a
    /// [`crate::diagnostic::DiagnosticKind::CacheOffline`] pushed onto it
    /// unconditionally — there is no live file to compare against, so the
    /// result is unverified and historical regardless of completeness.
    pub async fn load_offline(&self) -> Result<CacheStatus> {
        match self {
            CacheMode::Offline(path) => {
                let mut status = load_offline(path).await?;
                match &mut status {
                    CacheStatus::Missing
                    | CacheStatus::Unreadable
                    | CacheStatus::UnsupportedVersion
                    | CacheStatus::SourceChanged { .. } => {}
                    CacheStatus::Valid { index, .. } | CacheStatus::Incomplete { index, .. } => {
                        index.diagnostics.push(Diagnostic::cache_offline());
                    }
                }
                Ok(status)
            }
            CacheMode::Enabled(_) | CacheMode::Disabled => {
                Err(Error::CacheModeMismatch("cache-only access requires CacheMode::Offline"))
            }
        }
    }

    /// Persist `index` per this mode. A disabled cache is a no-op — nothing
    /// is written, by design, not as a fallback. Callers whose operation's
    /// whole purpose is to populate the cache should reject a disabled mode
    /// up front with [`CacheMode::require_enabled`] instead of relying on
    /// this silently doing nothing.
    pub async fn save(&self, source: &dyn ByteRangeSource, index: &DumpIndex) -> Result<()> {
        match self {
            CacheMode::Enabled(path) => save(path, source, index).await,
            CacheMode::Disabled => Ok(()),
            CacheMode::Offline(_) => {
                Err(Error::CacheModeMismatch("cache-only mode never has a fresh scan to persist"))
            }
        }
    }

    /// The path this mode resolves to, or `Error::CacheDisabled` if the
    /// caller disabled the cache but `operation` requires one. Use this to
    /// reject an incompatible `--dqcache none` up front, rather than
    /// discovering it only when a write silently no-ops.
    pub fn require_enabled(&self, operation: &'static str) -> Result<&Path> {
        match self {
            CacheMode::Enabled(path) => Ok(path),
            CacheMode::Disabled => Err(Error::CacheDisabled { operation }),
            CacheMode::Offline(_) => Err(Error::CacheModeMismatch(
                "cache-only mode has no live source to scan and persist",
            )),
        }
    }
}
