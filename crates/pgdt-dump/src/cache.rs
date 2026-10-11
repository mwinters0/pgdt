//! On-disk structure cache: a serialized [`DumpIndex`], colocated with the
//! dump file by default or at an explicit path
//! (`docs/design/decisions.md`, "The compressed source and the cache").
//!
//! **A cache that cannot be used is refused, not scanned over**
//! (`docs/design/decisions.md`, "D20"): one that is another file's, another
//! build's, or damaged is almost always a user's accident — the wrong path, or
//! a file that changed — so the four scan entry points answer
//! [`Error::CacheUnusable`] before the dump is read past its magic, naming which of
//! those it is ([`Unusable`]). [`CacheMode::with_overwrite_unusable`] opts into
//! starting cold and replacing it instead, for a source whose file is replaced
//! under a stable name; a file that is not a pgdt cache at all is refused
//! whatever is asked. Only a missing cache starts cold unasked. Writing is not
//! best-effort either: [`save`] propagates I/O failures rather than silently
//! degrading every future run of the same command back to a full scan.
//!
//! **The dump file's identity is checked, not assumed**
//! (`docs/design/decisions.md`, "D21"). Every cache records the source's
//! *stored* size and modification signal as observed at save time — bytes on
//! the device, not the addressable (possibly decompressed) length — and, for a
//! source that was fetched from somewhere, **where it came from** and the
//! server's entity tag
//! (`docs/design/decisions.md`, "D87"); [`load`]
//! re-observes the live source and compares. A stored-size mismatch means
//! every byte offset in the cache could be wrong, so the cache is unusable.
//! Neither weak signal invalidates it by default: they are surfaced on
//! [`CacheStatus::Valid`] as a [`WeakIdentity`] and an [`OriginMatch`], and
//! [`CacheMode::load`] turns each into a warning on the loaded index,
//! recomputed on every load and never persisted. A caller that asked for
//! [`StrictIdentity::time`] or [`StrictIdentity::location`] is refused
//! instead — there, or at [`CacheMode::strict_identity_refusal`] where it read
//! the status itself.
//!
//! **Two different questions hide under one word.** *Between* runs a moved,
//! copied or touched dump is a different weak identity holding the same
//! bytes, which is what the advisory default is right for. *During* a run a
//! source whose identity changes is bytes moving underneath a read that has
//! already returned some of them, which no answer survives — so that is an
//! error under every selection but `none`, which only warns, and
//! [`SourceWatch`] is the mechanism.
//!
//! The comparisons are [`identity`]'s, and how a caller asks for the cache
//! handled, and which signals bind, [`mode`]'s; the file's format is
//! [`file`]'s.

pub(crate) mod file;
pub(crate) mod identity;
mod mode;

use std::io::Write;
use std::path::{Path, PathBuf};

use bincode::error::EncodeError;
use serde::Serialize;

use self::file::{
    CACHE_MAGIC, CacheFile, CacheFileRef, CompressionIndex, ContainerKind, read_cache_file,
    write_beside,
};
use self::identity::SourceIdentity;
pub use self::identity::{OriginMatch, SourceWatch, WeakIdentity, advisory_identity_diagnostics};
pub use self::mode::{CacheLoad, CacheMode, StrictIdentity};
use crate::map::{
    non_seekable_compression_diagnostic, tiling_diagnostics, toc_coverage_diagnostic,
};
pub use pgdt_core::Unusable;
pub use pgdt_core::internal::OVERWRITE_WAYS_OUT;
use pgdt_core::internal::StatisticsScope;
use pgdt_core::{Error, Result};
use pgdt_io::{ByteRangeSource, KnownCompression, Origin};
use pgdt_model::{CACHE_FORMAT_VERSION, DataBlock, DumpIndex, SpanBody, calendar_end};

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
/// (`pgdt_query::PlanNoteKind::CompressedBlockPathDeclined`); `blocks` is how much
/// seeking the file offers at all; `streams` is what explains a slow first
/// command, a concatenated file costing one seek per stream to walk
/// (`docs/design/decisions.md`, "D18").
///
/// **Derived, not stored** — a projection of the seek table the envelope
/// already carries, computed on load like the diagnostics beside it. It is a
/// property of the *file*, which is why a cache answers it with no dump
/// present, unlike a budget decline, which depends on the run and reaches a
/// caller as `pgdt_query::PlanNoteKind::CompressedBlockPathDeclined`.
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

/// Everything a cache file persists beside its index and `total_size`, as it
/// was read — handed out on [`CacheStatus::Valid`]/[`CacheStatus::Incomplete`]
/// so a reporting caller can export the whole file, which `pgdt info --json`
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
    calendar_end: i32,
}

/// The default cache location when no explicit path is given:
/// `<dump-path>.dtcache`.
pub fn colocated_path(dump_path: &Path) -> PathBuf {
    let mut path = dump_path.as_os_str().to_owned();
    path.push(".dtcache");
    PathBuf::from(path)
}

/// What [`load`] found at a cache path, once checked against the live
/// source's identity — see the module docs.
///
/// Each unusable outcome is named and stays named all the way to the caller:
/// [`CacheMode::load`] carries it across as [`CacheLoad::Unusable`] rather
/// than answering `None` (`docs/design/decisions.md`, "D22").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheStatus {
    /// Nothing at this path.
    Missing,
    /// Something is at this path that cannot be used, and why.
    Unusable(Unusable),
    /// A usable cache whose `index.scanned_through` reaches the file's
    /// recorded size — the whole file is mapped. `weak` is what the source's
    /// current modification signal says against the one recorded at save time
    /// and `origin` what its origin says against the recorded one, neither of
    /// which itself makes the cache unusable (see the module docs).
    /// `total_size` is [`CacheFile::total_size`]: a reporting caller
    /// states coverage against it, and a cache-only caller has no live source
    /// to stat. `compression` is the container's shape where one sits under
    /// these offsets and `None` for a plain file — read off the persisted
    /// seek table, so a cache-only caller answers it with no dump present
    /// ([`CompressionShape`]). `envelope` is the rest of what the file
    /// persists, for a caller exporting it ([`CacheEnvelope`]).
    Valid {
        index: DumpIndex,
        weak: WeakIdentity,
        origin: OriginMatch,
        total_size: u64,
        compression: Option<CompressionShape>,
        envelope: CacheEnvelope,
    },
    /// A usable cache whose `index.scanned_through` falls short of
    /// `total_size` — a real, not-yet-finished scan (a preamble-only scan, or
    /// a query that stopped once its target settled), not a defect
    /// (`docs/design/decisions.md`, "D22"). What "not enough" means is
    /// caller-specific: `pgdt info` states the coverage
    /// (`docs/design/decisions.md`, "The CLI"), while a caller resuming an
    /// incremental scan (`pgdt_query::map_file`,
    /// `pgdt_query::table_stream`, `crate::preamble::preamble_only`) builds
    /// on this partial index, the same as [`Valid`](CacheStatus::Valid).
    /// `total_size`, `compression` and `envelope` are
    /// [`Valid`](CacheStatus::Valid)'s and mean the same thing.
    Incomplete {
        index: DumpIndex,
        weak: WeakIdentity,
        origin: OriginMatch,
        total_size: u64,
        compression: Option<CompressionShape>,
        envelope: CacheEnvelope,
    },
}

/// Load a cache from `path` and validate it against `source`'s current
/// identity. See [`CacheStatus`] and the module docs for what "validate"
/// means. A hard I/O error reading `path` (anything but "not found") still
/// propagates; what the file *holds* is answered as a status.
///
/// **The compression layer is compared as well as the size**: a source opened
/// without the cache's claim — an embedder's, or `pgdt` under
/// `--overwrite-unusable-cache` after recognition refused the claim — would
/// otherwise be handed offsets another file's seek table produced
/// ([`Unusable::CompressionContradicted`]; `docs/design/decisions.md`, "D20").
pub async fn load(path: &Path, source: &dyn ByteRangeSource) -> Result<CacheStatus> {
    let file = match read_cache_file(path)? {
        Ok(file) => file,
        Err(status) => return Ok(status),
    };
    let live = SourceIdentity::observe(source).await?;
    let (cached_stored_size, live_stored_size) = (file.identity.stored_size(), live.stored_size());
    if cached_stored_size != live_stored_size {
        return Ok(CacheStatus::Unusable(Unusable::SourceChanged {
            cached_stored_size,
            live_stored_size,
        }));
    }
    let cached_table = file.compression.as_ref().map(|CompressionIndex::Xz(table)| table);
    if cached_table != source.seek_table().as_ref() {
        return Ok(CacheStatus::Unusable(Unusable::CompressionContradicted));
    }
    let weak = file.identity.weak_against(&live);
    let origin = file.identity.origin_against(&live);
    Ok(status_from_file(file, weak, origin))
}

/// What the cache at a path settles about a dump file **before any source
/// exists** — [`claim`]'s answer (`docs/design/decisions.md`, "D18").
///
/// Two outcomes. What a caller does with a cache this early is decide which
/// source to build, except where the cache is unusable, which every scan
/// entry point refuses unless told it may overwrite it
/// ([`CacheMode::refusal`]): opening the source first buys nothing and, for a
/// many-streams `.xz`, costs a stream-footer walk. A missing cache settles
/// [`KnownCompression::Unknown`] and no statistics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheClaim {
    /// What the cache settles short of a refusal.
    Settles {
        /// The file's compression layer: the table a previous walk produced,
        /// "no compression layer", or nothing known.
        compression: KnownCompression,
        /// The heap the cache's statistics hold once loaded
        /// ([`DumpIndex::statistics_heap_bytes`]), zero where nothing decoded:
        /// what a caller carving workers before its scan loads the cache bills
        /// as held (`docs/design/decisions.md`, "D85").
        statistics_heap_bytes: u64,
    },
    /// The cache cannot be used, for the reason [`load`] would answer a
    /// moment later — a stored size compared against the origin's probe rather
    /// than a source, and never [`Unusable::CompressionContradicted`], which
    /// needs the file's own bytes.
    Unusable(Unusable),
}

/// What the cache at `cache_path` settles about `origin`, answered **before
/// any source exists** (`docs/design/decisions.md`, "D18").
///
/// This is the half of the cache a caller needs *early*: recognition decides
/// which source to build, and for an `.xz` file it either walks the stream
/// footers or is handed the table a previous walk already produced. With no
/// source yet to give [`load`], identity is checked against
/// [`pgdt_io::Origin::probe`] — the same stored-size rule, with a differing
/// mtime again too weak to invalidate anything. That comparison lives here
/// rather than at the caller, which keeps the early refusal the same verdict
/// as the late one, and it reads the origin's probe rather than a `stat` so
/// that the verdict is not a statement about local files.
///
/// *Not taken:* stopping the decode short of the index, which comes after
/// `compression` and `identity` in [`CacheFile`]. *Rejected:* a sibling
/// reporting the stored size, which would decode a
/// many-thousand-entry seek table twice on the usable path to spare a walk on
/// the path that is about to fail.
pub async fn claim(cache_path: &Path, origin: &Origin) -> Result<CacheClaim> {
    let file = match read_cache_file(cache_path)? {
        Ok(file) => file,
        Err(CacheStatus::Unusable(why)) => return Ok(CacheClaim::Unusable(why)),
        Err(_) => {
            return Ok(CacheClaim::Settles {
                compression: KnownCompression::Unknown,
                statistics_heap_bytes: 0,
            });
        }
    };
    let stored_size = file.identity.stored_size();
    // The index is decoded only to be sized and dropped, its statistics freed
    // as they were decoded: inside a statistics scope (`pgdt_core::instrument`).
    let CacheFile { compression, index, .. } = file;
    let statistics_heap_bytes = index.statistics_heap_bytes();
    drop_attributed(index);
    // An origin that cannot be probed is left to the open that follows,
    // which is where that failure has a sentence to say.
    let Ok(live) = origin.probe().await else {
        return Ok(CacheClaim::Settles {
            compression: KnownCompression::Unknown,
            statistics_heap_bytes,
        });
    };
    if stored_size != live.stored_size() {
        return Ok(CacheClaim::Unusable(Unusable::SourceChanged {
            cached_stored_size: stored_size,
            live_stored_size: live.stored_size(),
        }));
    }
    let compression = match compression {
        Some(CompressionIndex::Xz(table)) => KnownCompression::Xz(table),
        None => KnownCompression::Plain,
    };
    Ok(CacheClaim::Settles { compression, statistics_heap_bytes })
}

/// Drop each block's statistics — the `Arc` sharing them included — inside a
/// statistics scope, as their decode was attributed. `index`'s own
/// allocations are *not* freed in the scope: it is a parameter, so it outlives
/// the guard and is released once this returns, which is right, their decode
/// having been attributed to nobody.
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
/// "The compressed source and the cache"). With no live source to compare
/// against, `weak` and `origin` are always the agreeing answer here;
/// "unverified, historical" is a
/// [`pgdt_model::DiagnosticKind::CacheOffline`] pushed by
/// [`CacheMode::load_offline`] instead.
pub async fn load_offline(path: &Path) -> Result<CacheStatus> {
    match read_cache_file(path)? {
        Ok(file) => Ok(status_from_file(file, WeakIdentity::Agrees, OriginMatch::Agrees)),
        Err(status) => Ok(status),
    }
}

/// Shared by [`load`] and [`load_offline`] once a `CacheFile` has passed its
/// header and (when live) stored-size and compression checks: `Valid` when the index's
/// own `scanned_through` reaches the file's recorded addressable length,
/// `Incomplete` otherwise, with the index's unpersisted diagnostics
/// recomputed either way. Reads [`CacheFile::total_size`] rather than
/// re-deriving one from a live source: by this point the stored sizes are
/// known equal wherever a live source exists, and `load_offline` has none.
fn status_from_file(file: CacheFile, weak: WeakIdentity, origin: OriginMatch) -> CacheStatus {
    let total_size = file.total_size;
    let mut index = file.index;
    // `DumpIndex::diagnostics` is `#[serde(skip)]`, so a loaded index arrives
    // with none. Both file-level figures are pure functions of the spans and
    // the persisted size, so they are recomputed on load rather than persisted
    // (`crate::diagnostic`, "Not persisted").
    index.diagnostics = tiling_diagnostics(&index.spans, total_size);
    index.diagnostics.push(toc_coverage_diagnostic(&index.spans));
    let table = file.compression.as_ref().map(|CompressionIndex::Xz(table)| table);
    index.diagnostics.extend(non_seekable_compression_diagnostic(table));
    // Derived for the same reason the diagnostics above are: a projection of
    // what the envelope already holds.
    let compression = file.compression.as_ref().map(CompressionShape::of);
    let envelope = CacheEnvelope {
        // Only a file whose header states this build's version is decoded.
        format_version: CACHE_FORMAT_VERSION,
        container_kind: file.container_kind,
        seek_table: file.compression,
        identity: file.identity,
        calendar_end: file.calendar_end,
    };
    if index.is_complete(total_size) {
        CacheStatus::Valid { index, weak, origin, total_size, compression, envelope }
    } else {
        CacheStatus::Incomplete { index, weak, origin, total_size, compression, envelope }
    }
}

/// Write `index` to `path` (colocated or explicit — whichever the caller
/// resolved), replacing any existing cache there, and record `source`'s
/// current stored size/mtime plus its addressable length for [`load`] to
/// check next time. Propagates I/O failures as `Error::Io` rather than
/// swallowing them — see the module docs.
///
/// **This is the writer, not the run's save**: the in-flight identity check a
/// scan makes rides [`CacheMode::save`], which is what every scan entry point
/// calls ([`SourceWatch`]).
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
        container_kind: ContainerKind::Plain,
        compression: source.seek_table().map(CompressionIndex::Xz),
        identity,
        total_size,
        calendar_end: calendar_end(),
        index,
    };
    write_beside(path, |writer| {
        writer.write_all(&CACHE_MAGIC)?;
        writer.write_all(&CACHE_FORMAT_VERSION.to_le_bytes())?;
        bincode::serde::encode_into_std_write(&file, writer, bincode::config::standard())
            .map(drop)
            .map_err(|e| match e {
                EncodeError::Io { inner, .. } => Error::Io(inner),
                other => Error::cache_encode(other),
            })
    })
}
