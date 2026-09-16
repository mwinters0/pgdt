//! On-disk structure cache: a serialized [`DumpIndex`], colocated with the
//! dump file by default or at an explicit path
//! (`docs/design/decisions.md`, "The compressed source and the cache").
//!
//! Reading is best-effort — the cache is never required for correctness, so
//! a missing, foreign or unrecognised-version file just means "scan instead,"
//! never a hard error. Which of the three it was is still reported, as far as
//! the decode can tell them apart (`KD30`) — see
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

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use bincode::error::{DecodeError, EncodeError};
use serde::{Deserialize, Serialize};

use crate::diagnostic::Diagnostic;
use crate::index::{
    DumpIndex, non_seekable_compression_diagnostic, tiling_diagnostics, toc_coverage_diagnostic,
};
use crate::instrument::StatisticsScope;
use crate::io::{ByteRangeSource, KnownCompression};
use crate::map::{DataBlock, SpanBody};
use crate::{Error, Result};

/// **Bump whenever a persisted field is added, removed or reshaped.** A cache
/// written under a different version is unusable
/// ([`CacheStatus::UnsupportedVersion`]) rather than partially trusted
/// (`docs/design/roadmap.md`, "Four decisions that keep later phases
/// additive"); pre-1.0 a bump is free, nothing migrates (`CLAUDE.md`,
/// "Pre-1.0"), and nothing records it (`docs/design/decisions.md`, "D22").
/// A new way of *reading* an existing on-disk shape needs no bump:
/// [`CacheStatus::Incomplete`] reinterprets `scanned_through` against a size
/// already stored. A new way of *choosing* what an unchanged field holds does:
/// a block recording the request that sized it is read back as already sized
/// under it, so a cache whose sizes today's rule would not choose is as
/// unusable as one of another shape.
pub(crate) const FORMAT_VERSION: u32 = 22;

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
/// memory budget is compared against — four times over, one reader's block
/// beside the further blocks the pool keeps, plus the chunk buffer and the
/// decoder's own working memory — so it is most of the read-buffer budget
/// `--memory` has to leave when a query says the block path was declined, and
/// that query's own note states the whole of it
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

/// What a cache file holds. [`CacheFileRef`] is what a save encodes, the same
/// fields in this order, so the two change together.
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

/// Everything a cache file persists beside its index and `total_size`, as it
/// was read — handed out on [`CacheStatus::Valid`]/[`CacheStatus::Incomplete`]
/// so a reporting caller can export the whole file, which `pgdq info --json`
/// does (`docs/design/decisions.md`, "D67").
///
/// **Opaque to Rust, whole to serde**: the fields stay private, so no caller
/// matches on [`SourceIdentity`] or reads the seek table through this, and
/// its `Serialize` is the persisted types' own, variant tags included. The
/// seek table serializes as `seek_table`, the name `compression` being
/// [`CompressionShape`]'s wherever the two sit side by side. Moved out of the
/// decoded file rather than copied, so carrying it costs nothing the load did
/// not already hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheEnvelope {
    format_version: u32,
    container_kind: ContainerKind,
    seek_table: Option<CompressionIndex>,
    identity: SourceIdentity,
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
    /// foreign bytes, or a file cut short.
    Unreadable,
    /// A cache whose `format_version` or `container_kind` this build does not
    /// recognise, where the rest of the file still decodes; nothing migrates
    /// (`docs/design/roadmap.md`, "Pre-1.0").
    ///
    /// Deficiency register: `deficiency: KD30` — the whole [`CacheFile`] is
    /// decoded before its version is read, so a cache from a build whose
    /// persisted shape changed — the change that bumps `FORMAT_VERSION` —
    /// almost always fails to decode and is [`CacheStatus::Unreadable`], which
    /// `pgdq info` words as not a pgdq cache at all; an unknown `ContainerKind`
    /// cannot decode at all. **(c) unowned**; promoted by a user sent to check a
    /// path that holds an old cache, the fix being the version read ahead of
    /// the rest.
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
    /// ([`CompressionShape`]). `envelope` is the rest of what the file
    /// persists, for a caller exporting it ([`CacheEnvelope`]).
    Valid {
        index: DumpIndex,
        mtime_changed: bool,
        total_size: u64,
        compression: Option<CompressionShape>,
        envelope: CacheEnvelope,
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
    /// `total_size`, `compression` and `envelope` are
    /// [`Valid`](CacheStatus::Valid)'s and mean the same thing.
    Incomplete {
        index: DumpIndex,
        mtime_changed: bool,
        total_size: u64,
        compression: Option<CompressionShape>,
        envelope: CacheEnvelope,
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
    /// [`CacheStatus::Unreadable`] — foreign bytes, or a file cut short.
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
///
/// The file is decoded through a buffered reader, never read whole first
/// (`docs/design/decisions.md`, "D78"). A file that ends early is
/// [`CacheStatus::Unreadable`], as any other undecodable content is; an I/O
/// failure reading it is still an error.
fn read_cache_file(path: &Path) -> Result<std::result::Result<CacheFile, CacheStatus>> {
    let reader = match File::open(path) {
        Ok(file) => BufReader::new(file),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Ok(Err(CacheStatus::Missing));
        }
        Err(e) => return Err(Error::Io(e)),
    };
    let file: CacheFile =
        match bincode::serde::decode_from_reader(reader, bincode::config::standard()) {
            Ok(file) => file,
            Err(DecodeError::Io { inner, .. }) if inner.kind() != ErrorKind::UnexpectedEof => {
                return Err(Error::Io(inner));
            }
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
/// *Not taken:* stopping the decode short of the index, which comes after
/// `compression` and `identity` in [`CacheFile`]. *Rejected:* a sibling
/// reporting the stored size, which would decode a
/// many-thousand-entry seek table twice on the usable path to spare a walk on
/// the path that is about to fail.
pub fn claim(cache_path: &Path, dump_path: &Path) -> Result<CacheClaim> {
    let Ok(file) = read_cache_file(cache_path)? else {
        return Ok(CacheClaim::Compression(KnownCompression::Unknown));
    };
    // One variant today; a future one is matched through the variant rather
    // than a shared accessor — see [`SourceIdentity`].
    let SourceIdentity::LocalFile { stored_size, .. } = file.identity;
    // The index is decoded only to be dropped, its statistics freed as they
    // were decoded: inside a statistics scope (`crate::instrument`).
    let CacheFile { compression, index, .. } = file;
    drop_attributed(index);
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
    Ok(CacheClaim::Compression(match compression {
        Some(CompressionIndex::Xz(table)) => KnownCompression::Xz(table),
        None => KnownCompression::Plain,
    }))
}

/// Drop `index`, each block's statistics — the `Arc` sharing them included —
/// inside a statistics scope, as their decode was attributed.
fn drop_attributed(mut index: DumpIndex) {
    let _attributed = StatisticsScope::enter();
    for span in &mut index.spans {
        if let SpanBody::Data(DataBlock::Copy(block)) = &mut span.body {
            block.statistics = None;
        }
    }
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
    let envelope = CacheEnvelope {
        format_version: file.format_version,
        container_kind: file.container_kind,
        seek_table: file.compression,
        identity: file.identity,
    };
    if index.is_complete(total_size) {
        CacheStatus::Valid { index, mtime_changed, total_size, compression, envelope }
    } else {
        CacheStatus::Incomplete { index, mtime_changed, total_size, compression, envelope }
    }
}

/// What [`save`] encodes: [`CacheFile`]'s fields in [`CacheFile`]'s order,
/// the index borrowed rather than cloned, so the bytes are the ones an owned
/// `CacheFile` encodes to and [`read_cache_file`] decodes.
#[derive(Serialize)]
struct CacheFileRef<'a> {
    format_version: u32,
    container_kind: ContainerKind,
    compression: Option<CompressionIndex>,
    identity: SourceIdentity,
    total_size: u64,
    index: &'a DumpIndex,
}

/// Write `index` to `path` (colocated or explicit — whichever the caller
/// resolved), replacing any existing cache there, and record `source`'s
/// current stored size/mtime plus its addressable length for [`load`] to
/// check next time. Propagates I/O failures as `Error::Io` rather than
/// swallowing them — see the module docs.
///
/// **The index is encoded straight into a file beside `path`, which is then
/// renamed over it** (`docs/design/decisions.md`, "D78"), so no encoded copy
/// of the cache is held in memory, and at every moment `path` holds either
/// the previous save or this one, whole. A save that fails removes its file;
/// one the process is killed during leaves it, named for the cache and ending
/// `.tmp`, and the previous cache untouched. The replacement is a new file: a
/// cache path that was a symbolic link becomes a regular file, and the file
/// takes default permissions rather than the old one's. Nothing is synced, so
/// the guarantee is against a killed process, not a lost machine.
pub async fn save(path: &Path, source: &dyn ByteRangeSource, index: &DumpIndex) -> Result<()> {
    let identity = SourceIdentity::observe(source).await?;
    let total_size = source.size().await?;
    let file = CacheFileRef {
        format_version: FORMAT_VERSION,
        container_kind: ContainerKind::Plain,
        compression: source.seek_table().map(CompressionIndex::Xz),
        identity,
        total_size,
        index,
    };
    write_beside(path, |writer| {
        bincode::serde::encode_into_std_write(&file, writer, bincode::config::standard())
            .map(drop)
            .map_err(|e| match e {
                EncodeError::Io { inner, .. } => Error::Io(inner),
                other => Error::CacheEncode(other),
            })
    })
}

/// Run `encode` into a new file [`beside`] `path`, then rename that file over
/// `path`; on any failure, remove it and leave `path` as it was.
fn write_beside(
    path: &Path,
    encode: impl FnOnce(&mut BufWriter<File>) -> Result<()>,
) -> Result<()> {
    let beside = beside(path);
    let file = OpenOptions::new().write(true).create_new(true).open(&beside)?;
    let written = (|| {
        let mut writer = BufWriter::new(file);
        encode(&mut writer)?;
        writer.flush()?;
        // Closed before the rename, which some platforms need.
        drop(writer);
        std::fs::rename(&beside, path)?;
        Ok(())
    })();
    if written.is_err() {
        // Best-effort: the error being returned is the one worth reporting.
        let _ = std::fs::remove_file(&beside);
    }
    written
}

/// The file a save writes before renaming it over `path`: in `path`'s own
/// directory, so the rename never crosses a filesystem, and named
/// `<cache file name>.<pid>-<n>.tmp`, unique to this process and this save,
/// so two saves of one cache — two processes, or two tasks of one — never
/// write into one file.
fn beside(path: &Path) -> PathBuf {
    static SAVES: AtomicU64 = AtomicU64::new(0);
    let mut name: OsString = path.file_name().map(ToOwned::to_owned).unwrap_or_default();
    name.push(format!(".{}-{}.tmp", std::process::id(), SAVES.fetch_add(1, Ordering::Relaxed)));
    path.with_file_name(name)
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
    /// Resolve a `--dqcache`-style argument against `dump_path`: `None`
    /// selects the colocated default (`<dump_path>.dqcache`), the literal
    /// path `none` disables the cache, and any other path is used as-is.
    pub fn resolve(dump_path: &Path, cache_path: Option<&Path>) -> CacheMode {
        match cache_path {
            None => CacheMode::Enabled(colocated_path(dump_path)),
            Some(p) if p == Path::new("none") => CacheMode::Disabled,
            Some(p) => CacheMode::Enabled(p.to_path_buf()),
        }
    }

    /// Load the cache this mode points at, turning the
    /// [`CacheStatus::Valid::mtime_changed`] bit into a
    /// [`crate::diagnostic::DiagnosticKind::CacheMtimeChanged`] on the index
    /// (see the module docs). A disabled cache always
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

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;
    use crate::{LocalFileSource, ScanOptions, StatisticsRequest, XzSource, map_file};

    /// Real `pg_dump` output whose columns gather bounds and dictionaries.
    fn statistics_fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/16/statistics/default.sql")
    }

    /// Every name in `dir`, sorted.
    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// Streaming changed where the bytes go, not what they are: a save writes
    /// exactly what the owned [`CacheFile`] encodes to in one buffer, for a
    /// plain source and for a compressed one carrying its seek table, with
    /// statistics gathered — and reads back through the reader as the index it
    /// wrote. The same file cut short reads as [`CacheStatus::Unreadable`],
    /// never as an I/O error.
    #[tokio::test]
    async fn a_save_writes_the_bytes_the_whole_file_encodes_to() {
        let dir = tempfile::tempdir().unwrap();
        let compressed = dir.path().join("default.sql.xz");
        let out = Command::new("xz")
            .args(["--block-size=65536", "-c"])
            .arg(statistics_fixture())
            .output()
            .expect("`xz` is not runnable, so this test cannot build its fixture; install it.");
        assert!(out.status.success(), "xz failed: {}", String::from_utf8_lossy(&out.stderr));
        std::fs::write(&compressed, &out.stdout).unwrap();

        let plain = LocalFileSource::open(statistics_fixture()).unwrap();
        let xz = XzSource::open(&compressed).unwrap();
        let sources: [(&str, &dyn ByteRangeSource); 2] = [("plain", &plain), ("xz", &xz)];
        for (name, source) in sources {
            let run = map_file(
                source,
                &ScanOptions::default(),
                &CacheMode::Disabled,
                &StatisticsRequest::default(),
            )
            .await
            .unwrap();
            assert!(
                run.index.spans.iter().any(|span| matches!(
                    &span.body,
                    SpanBody::Data(DataBlock::Copy(block)) if block.statistics.as_ref().is_some_and(
                        |statistics| statistics.columns.iter().flatten().any(|column| {
                            column.bounds.is_some() && column.dictionary.is_some()
                        })
                    )
                )),
                "{name}: the encoding under test must carry statistics"
            );
            assert_eq!(source.seek_table().is_some(), name == "xz");

            let path = dir.path().join(format!("{name}.dqcache"));
            save(&path, source, &run.index).await.unwrap();
            let whole = CacheFile {
                format_version: FORMAT_VERSION,
                container_kind: ContainerKind::Plain,
                compression: source.seek_table().map(CompressionIndex::Xz),
                identity: SourceIdentity::observe(source).await.unwrap(),
                total_size: source.size().await.unwrap(),
                index: run.index.clone(),
            };
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(
                bytes,
                bincode::serde::encode_to_vec(&whole, bincode::config::standard()).unwrap(),
                "{name}: the streamed save's bytes"
            );
            let Ok(Ok(read)) = read_cache_file(&path) else {
                panic!("{name}: the streamed save must read back");
            };
            // Diagnostics are not persisted, so the index is compared by its
            // encoding: what reads back encodes to what was written.
            assert_eq!(
                bincode::serde::encode_to_vec(&read, bincode::config::standard()).unwrap(),
                bytes,
                "{name}: the file read back"
            );

            std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
            assert!(
                matches!(read_cache_file(&path), Ok(Err(CacheStatus::Unreadable))),
                "{name}: a cache cut short is unreadable, not an I/O failure"
            );
        }
    }

    /// A kill mid-save leaves the previous cache whole. No test races a
    /// signal (`docs/design/decisions.md`, "D73"), so the save is stopped at
    /// the moment a kill would land — partway through its encoding — and what
    /// it leaves on disk is read there: the previous cache, untouched, and the
    /// partial file beside it. A save that fails removes that file, and one
    /// that finishes replaces the cache with nothing left beside it.
    #[test]
    fn a_save_leaves_the_previous_cache_whole_until_it_renames() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dump.sql.dqcache");
        std::fs::write(&path, b"the previous cache").unwrap();

        let failed = write_beside(&path, |writer| {
            writer.write_all(b"the first half of the next")?;
            writer.flush()?;
            assert_eq!(std::fs::read(&path).unwrap(), b"the previous cache");
            let names = listing(dir.path());
            assert_eq!(names.len(), 2, "the cache and the save's own file: {names:?}");
            let partial = names.iter().find(|name| *name != "dump.sql.dqcache").unwrap();
            assert!(
                partial.starts_with("dump.sql.dqcache.") && partial.ends_with(".tmp"),
                "the partial file is named for its cache: {partial}"
            );
            assert_eq!(
                std::fs::read(dir.path().join(partial)).unwrap(),
                b"the first half of the next"
            );
            Err(Error::CacheModeMismatch("the save stops here"))
        });
        assert!(matches!(failed, Err(Error::CacheModeMismatch(_))));
        assert_eq!(std::fs::read(&path).unwrap(), b"the previous cache");
        assert_eq!(listing(dir.path()), ["dump.sql.dqcache"]);

        write_beside(&path, |writer| {
            writer.write_all(b"the next cache")?;
            assert_eq!(std::fs::read(&path).unwrap(), b"the previous cache");
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"the next cache");
        assert_eq!(listing(dir.path()), ["dump.sql.dqcache"]);
    }
}
