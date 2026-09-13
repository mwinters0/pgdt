//! On-disk structure cache: a serialized [`DumpIndex`], colocated with the
//! dump file by default or at an explicit path
//! (`docs/design/decisions.md`, "The compressed source and the cache").
//!
//! Reading is best-effort — the cache is never required for correctness, so
//! a missing, foreign or unrecognised-version file just means "scan instead,"
//! never a hard error. Which of the three it was is still reported — see
//! [`CacheStatus`], and [`CacheLoad`] for the same statuses reaching a caller
//! that holds a live source — because `pgdq info` has no "scan instead" to
//! fall back on and has to say what went wrong.
//!
//! **The size-mismatched file is the exception.** A cache whose recorded
//! stored size is not this source's describes some *other* file, so the three
//! scan entry points refuse ([`Error::CacheSourceMismatch`]) rather than
//! starting cold (`docs/design/decisions.md`, "D20"). Writing is not
//! best-effort either: [`save`] propagates I/O failures rather than silently
//! degrading every future run of the same command back to a full scan.
//!
//! **The dump file's identity is checked, not assumed**
//! (`docs/design/decisions.md`, "D21"). Every cache records the source's
//! *stored* size and mtime as observed at save time — bytes on the device,
//! not the addressable (possibly decompressed) length; [`load`] re-observes
//! the live source and compares. A stored-size mismatch means every byte
//! offset in the cache could be wrong, so the cache is unusable. An mtime
//! mismatch does **not** invalidate it: it is surfaced on
//! [`CacheStatus::Valid`], and [`CacheMode::load`] turns it into a
//! [`crate::diagnostic::DiagnosticKind::CacheMtimeChanged`] on the loaded
//! index, recomputed on every load and never persisted.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::diagnostic::Diagnostic;
use crate::index::{
    DumpIndex, non_seekable_compression_diagnostic, tiling_diagnostics, toc_coverage_diagnostic,
};
use crate::io::{ByteRangeSource, KnownCompression};
use crate::{Error, Result};

/// **Bump whenever a persisted field is added, removed or reshaped.** A cache
/// written under a different version is unusable
/// ([`CacheStatus::UnsupportedVersion`]) rather than partially trusted
/// (`docs/design/roadmap.md`, "Four decisions that keep later phases
/// additive"); pre-1.0 a bump is free, nothing migrates (`CLAUDE.md`,
/// "Pre-1.0"), and nothing records it (`docs/design/decisions.md`, "D22").
/// A new way of *reading* an existing on-disk shape needs no bump:
/// [`CacheStatus::Incomplete`] reinterprets `scanned_through` against a size
/// already stored.
const FORMAT_VERSION: u32 = 17;

/// The dump file's identity as observed when a cache was last saved — see
/// the module docs.
///
/// **Opaque, not a struct** (`docs/design/decisions.md`, "D21"): a future
/// variant carries whatever evidence its own kind of source has (an ETag is
/// not a `SystemTime`). One variant today; a call site reads the fields
/// through the variant it matches, never through a shared accessor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum SourceIdentity {
    /// Backed by [`ByteRangeSource::stored_size`]/`modified` — every source
    /// today, decompressing or not.
    LocalFile {
        stored_size: u64,
        /// `(seconds, nanoseconds)` since the Unix epoch: L1's own on-disk
        /// vocabulary, `SystemTime` not being `Serialize`. `None` when the
        /// source exposed no mtime.
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
/// raw file positions; a future archive format's would be entry-relative, so
/// the two must never be silently conflated.
///
/// A compressed source's indexed offsets are *also* plain-format offsets —
/// byte for byte what a plain scan of the same decompressed content
/// produces — so a compressed source never sets this to anything but
/// `Plain`; what changes is [`CompressionIndex`], a sibling field
/// (`docs/design/decisions.md`, "D21").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum ContainerKind {
    Plain,
}

/// What compression sits between this cache's plain-format offsets and the
/// bytes on disk — `None` for a source that needs no such index
/// (`docs/design/decisions.md`, "D21"). A sibling of
/// [`ContainerKind`], not a value of it: see that type's docs.
///
/// Built at [`save`] time from [`ByteRangeSource::seek_table`], which
/// [`crate::XzSource`] answers from the table it already built while opening,
/// so persisting a cache never re-walks the file; read back by [`claim`]
/// before any source exists, which spares every later command that walk. A
/// future gzip index is a different *shape* and gets its own sibling variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum CompressionIndex {
    Xz(xz_seek::SeekTable),
}

/// The shape of the container a cache's offsets sit under, read off the
/// persisted [`CompressionIndex`].
///
/// Three numbers, each answering a question the user is otherwise sent to
/// `xz --list` for. `max_block_uncompressed` is the largest term of what a
/// memory budget is compared against — twice it, the block path holding one
/// block while it decodes the next, plus the chunk buffer and the decoder's
/// own retention — so it is most of the number to raise `--parallel-memory`
/// to when a query says the block path was declined, and that query's own
/// note states the whole of it
/// (`crate::PlanNoteKind::CompressedBlockPathDeclined`); `blocks` is how much
/// seeking the file offers at all; `streams` is what explains a slow first
/// command, a concatenated file costing one seek per stream to walk
/// (`docs/design/decisions.md`, "D18").
///
/// **Derived, not stored** — a projection of the seek table the envelope
/// already carries, computed on load like the diagnostics beside it. It is a
/// property of the *file*, which is why a cache answers it with no dump
/// present, unlike a budget decline, which depends on the run and reaches a
/// caller as `crate::PlanNoteKind::CompressedBlockPathDeclined`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CompressionShape {
    /// The container: `"xz"` today, and the only value there is. A word
    /// rather than a one-variant enum, being a name to print.
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
    /// [`ByteRangeSource::size`] as observed at save time. Its own field, not
    /// an alias of `identity`'s stored size (`docs/design/decisions.md`,
    /// "D21"), and the number
    /// [`CacheStatus::Valid`]/[`CacheStatus::Incomplete`] hand out as
    /// `total_size` for the coverage arithmetic.
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
/// The four unusable outcomes are named separately and stay named all the way
/// to the caller: [`CacheMode::load`] carries each one across as a
/// [`CacheLoad`] variant rather than answering `None`
/// (`docs/design/decisions.md`, "D22").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheStatus {
    /// Nothing at this path.
    Missing,
    /// Something is at this path, but it does not decode as a cache at all —
    /// foreign bytes, or a truncated write.
    Unreadable,
    /// A cache written by a build whose on-disk shape this one does not
    /// recognise (`format_version`/`container_kind`); nothing migrates
    /// (`docs/design/roadmap.md`, "Pre-1.0").
    UnsupportedVersion,
    /// A readable cache whose recorded *stored* size disagrees with the live
    /// source's, so every byte offset in it could be wrong — the staleness
    /// check reads [`ByteRangeSource::stored_size`], not the addressable
    /// length (`docs/design/decisions.md`, "D21"). Both stored sizes are
    /// carried: they are the evidence a reporting caller states.
    SourceChanged { cached_stored_size: u64, live_stored_size: u64 },
    /// A usable cache whose `index.scanned_through` reaches the file's
    /// recorded size — the whole file is mapped. `mtime_changed` is `true`
    /// when the source's current mtime differs from the one recorded at save
    /// time, which does not itself make the cache unusable (see the module
    /// docs). `total_size` is [`CacheFile::total_size`]: a reporting caller
    /// states coverage against it, and a cache-only caller has no live source
    /// to stat. `compression` is the container's shape where one sits under
    /// these offsets and `None` for a plain file — read off the persisted
    /// seek table, so a cache-only caller answers it with no dump present
    /// ([`CompressionShape`]).
    Valid {
        index: DumpIndex,
        mtime_changed: bool,
        total_size: u64,
        compression: Option<CompressionShape>,
    },
    /// A usable cache whose `index.scanned_through` falls short of
    /// `total_size` — a real, not-yet-finished scan (a preamble-only scan, or
    /// a query that stopped once its target settled), not a defect
    /// (`docs/design/decisions.md`, "D22"). What "not enough" means is
    /// caller-specific: `pgdq info` states the coverage
    /// (`docs/design/decisions.md`, "The CLI"), while a caller resuming an
    /// incremental scan (`crate::stream::map_file`,
    /// `crate::stream::table_stream`, `crate::index::preamble_only`) builds
    /// on this partial index, the same as [`Valid`](CacheStatus::Valid).
    /// `total_size` and `compression` are
    /// [`Valid`](CacheStatus::Valid)'s and mean the same thing.
    Incomplete {
        index: DumpIndex,
        mtime_changed: bool,
        total_size: u64,
        compression: Option<CompressionShape>,
    },
}

/// What [`CacheMode::load`] answered a caller that holds a live source: an
/// index to build forward from, or the reason there is none
/// (`docs/design/decisions.md`, "D22").
///
/// `Disabled` is a statement about the *caller*, with no [`CacheStatus`] to
/// correspond to; the other four are that status carried across unchanged, so
/// a caller answers five outcomes. `Valid` and `Incomplete` are one variant
/// here, because a caller holding a live source builds forward from either —
/// see [`CacheMode::load`].
///
/// *Rejected:* an accessor collapsing the four reasons back to `Option` for
/// the callers that do not care. It rebuilds the collapse under a shorter
/// name at exactly the sites that must not have it, and the callers that
/// genuinely do not care are tests, which can say so in a `let … else`.
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
    /// Both numbers travel as the evidence for the claim.
    ///
    /// **The three scan entry points refuse on this one**, where they start
    /// cold on the other three (`docs/design/decisions.md`, "D20"). The two
    /// sizes are what [`CacheMode::source_mismatch`] turns into
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
    // One variant today, so both patterns are irrefutable; a future
    // `Remote` variant is matched explicitly rather than through a shared
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
/// exists** — [`claim`]'s answer (`docs/design/decisions.md`, "D18").
///
/// Two outcomes. What a caller does with a cache this early is decide which
/// source to build, except for the one unusable outcome that makes the
/// building pointless: a cache recorded against a file of another stored size
/// is refused by every scan entry point ([`Error::CacheSourceMismatch`]), so
/// opening the source first buys nothing and, for a many-streams `.xz`, costs
/// a stream-footer walk. The other unusable outcomes stay collapsed into
/// [`KnownCompression::Unknown`] — no cache, foreign bytes, a version this
/// build does not read — since nothing downstream refuses on those and
/// [`load`] states the reason a moment later.
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
/// (`docs/design/decisions.md`, "D18").
///
/// This is the half of the cache a caller needs *early*: recognition decides
/// which source to build, and for an `.xz` file it either walks the stream
/// footers or is handed the table a previous walk already produced. With no
/// source yet to give [`load`], identity is checked against a plain `stat` on
/// `dump_path` — the same stored-size rule, with a differing mtime again too
/// weak to invalidate anything. That comparison lives here rather than at the
/// caller, which keeps the early refusal the same verdict as the late one.
///
/// *Rejected:* stopping the decode short of the index (bincode is positional,
/// so reaching `compression` and `identity` decodes what precedes them
/// anyway); a sibling reporting the stored size, which would decode a
/// many-thousand-entry seek table twice on the usable path to spare a walk on
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
/// cache-only counterpart to [`load`] (`docs/design/decisions.md`,
/// "The compressed source and the cache"). With no mtime to compare,
/// `mtime_changed` is always `false` here; "unverified, historical" is a
/// [`crate::diagnostic::DiagnosticKind::CacheOffline`] pushed by
/// [`CacheMode::load_offline`] instead.
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
/// re-deriving one from a live source: by this point the stored sizes are
/// known equal wherever a live source exists, and `load_offline` has none.
fn status_from_file(file: CacheFile, mtime_changed: bool) -> CacheStatus {
    let total_size = file.total_size;
    let mut index = file.index;
    // `DumpIndex::diagnostics` is `#[serde(skip)]`, so a loaded index arrives
    // with none. Both file-level figures are pure functions of the spans, so
    // they are recomputed on load rather than persisted
    // (`crate::diagnostic`, "Not persisted").
    index.diagnostics = tiling_diagnostics(&index.spans, total_size);
    index.diagnostics.push(toc_coverage_diagnostic(&index.spans));
    let table = file.compression.as_ref().map(|CompressionIndex::Xz(table)| table);
    index.diagnostics.extend(non_seekable_compression_diagnostic(table));
    // Derived for the same reason the diagnostics above are: a projection of
    // what the envelope already holds.
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
    /// path (`docs/design/decisions.md`,
    /// "The compressed source and the cache"). Never constructed by
    /// [`CacheMode::resolve`]; a caller builds it directly (`pgdq info` with
    /// no `--source`). Every method below that takes a live `source` rejects
    /// it as a caller-contract violation, and [`load_offline`] rejects
    /// `Enabled`/`Disabled` the other way.
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
    /// caller that wants it calls [`load`] directly). A disabled cache always
    /// yields [`CacheLoad::Disabled`], even if a file sits at what would
    /// otherwise be its resolved location.
    ///
    /// `Incomplete` is treated exactly like `Valid` — both are
    /// [`CacheLoad::Index`] — because every caller of this method
    /// (`crate::stream::map_file`, `crate::stream::table_stream`,
    /// `crate::index::preamble_only`) builds forward from whatever partial
    /// map exists (`docs/design/decisions.md`, "D22"). A caller that instead
    /// *reports* what a cache holds reaches for
    /// [`load`]/[`CacheMode::load_offline`] and the full [`CacheStatus`], as
    /// `pgdq info` does. Neither the four unusable statuses nor the caller's
    /// own opt-out is collapsed: each arrives as its own variant.
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
                    // Reported rather than acted on: too weak to invalidate,
                    // and recomputed on every load (see the module docs).
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
    /// compared (`docs/design/decisions.md`, "D20"). The path lives here
    /// rather than on `CacheLoad` because [`CacheMode::Disabled`] has none.
    ///
    /// Only [`CacheMode::Enabled`] can reach it; `Disabled` and `Offline`
    /// answer the caller-contract error.
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
    /// (`docs/design/decisions.md`,
    /// "The compressed source and the cache"). Returns the full
    /// [`CacheStatus`] rather than a [`CacheLoad`]: with no scan to fall back
    /// on, a caller needs `Valid` apart from `Incomplete` — the one
    /// distinction `load` exists to erase. Every successful load gets a
    /// [`crate::diagnostic::DiagnosticKind::CacheOffline`] pushed onto it
    /// unconditionally.
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
    /// is written, by design. A caller whose operation exists to populate the
    /// cache rejects a disabled mode up front with
    /// [`CacheMode::require_enabled`] instead of relying on this.
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
    /// caller disabled the cache but `operation` requires one — so an
    /// incompatible `--dqcache none` is rejected up front rather than
    /// discovered when a write silently no-ops.
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
