//! A cache read against its source and written beside it: where one sits by
//! default ([`colocated_path`]), what a decoded cache settles before any
//! source exists ([`settled_by`]), a decoded cache checked against a
//! live source ([`against_source`]), a load with no source ([`load_offline`]),
//! and the writer ([`save`]) — see the parent module's docs for what each
//! refuses and why. `load` and [`save`] are no entry point of their own: a
//! caller opens through [`CacheMode::open`](super::CacheMode::open) and saves
//! through its watched [`save`](super::CacheMode::save), and tests that write
//! a cache by hand or read back what a run saved reach them through
//! `internal`.

use std::path::{Path, PathBuf};

use super::file::{
    CacheEnvelope, CacheFile, CacheFileRef, CacheStatus, CompressionIndex, CompressionShape,
    ContainerKind, LoadedCache, read_cache_file, write_cache_file,
};
use super::identity::{OriginMatch, SourceIdentity, WeakIdentity};
use crate::map::derived_diagnostics;
use pgdt_core::internal::StatisticsScope;
use pgdt_core::{Result, Unusable};
use pgdt_io::{ByteRangeSource, KnownCompression, Origin};
use pgdt_model::{CACHE_FORMAT_VERSION, DataBlock, DumpIndex, SpanBody, calendar_end};

/// The default cache location when no explicit path is given:
/// `<dump-path>.dtcache`.
pub fn colocated_path(dump_path: &Path) -> PathBuf {
    let mut path = dump_path.as_os_str().to_owned();
    path.push(".dtcache");
    PathBuf::from(path)
}

/// Load a cache from `path` and validate it against `source`'s current
/// identity. See [`CacheStatus`] and the module docs for what "validate"
/// means. A hard I/O error reading `path` (anything but "not found") still
/// propagates; what the file *holds* is answered as a status. A test's
/// alone: a caller opens through [`CacheMode::open`](super::CacheMode::open).
#[cfg(any(test, feature = "test-support"))]
pub async fn load(path: &Path, source: &dyn ByteRangeSource) -> Result<CacheStatus> {
    match read_cache_file(path)? {
        Ok(file) => against_source(file, source).await,
        Err(status) => Ok(status),
    }
}

/// A decoded cache file checked against `source`'s current identity: the
/// stored size, which refuses, then the weak signals, which do not.
///
/// **The compression layer is compared as well as the size**: a source opened
/// without the cache's claim — an embedder's, or one opened claiming nothing
/// after recognition refused the claim — would otherwise be handed offsets
/// another file's seek table produced ([`Unusable::CompressionContradicted`];
/// `docs/design/decisions.md`, "D20").
pub(super) async fn against_source(
    file: CacheFile,
    source: &dyn ByteRangeSource,
) -> Result<CacheStatus> {
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

/// What a decoded cache file settles about `origin` before any source exists:
/// the compression layer recognition is handed, or the reason the file is
/// another one's — its stored size against [`pgdt_io::Origin::probe`]'s, the
/// same rule the live comparison applies ([`against_source`]). An origin that
/// cannot be probed settles [`KnownCompression::Unknown`], that failure being
/// left to the open that follows, which is where it has a sentence to say.
pub(super) async fn settled_by(
    file: &CacheFile,
    origin: &Origin,
) -> std::result::Result<KnownCompression, Unusable> {
    let Ok(live) = origin.probe().await else {
        return Ok(KnownCompression::Unknown);
    };
    let stored_size = file.identity.stored_size();
    if stored_size != live.stored_size() {
        return Err(Unusable::SourceChanged {
            cached_stored_size: stored_size,
            live_stored_size: live.stored_size(),
        });
    }
    Ok(match &file.compression {
        Some(CompressionIndex::Xz(table)) => KnownCompression::Xz(table.clone()),
        None => KnownCompression::Plain,
    })
}

/// Drop each block's statistics — the `Arc` sharing them included — inside a
/// statistics scope, as their decode was attributed. `index`'s own
/// allocations are *not* freed in the scope: it is a parameter, so it outlives
/// the guard and is released once this returns, which is right, their decode
/// having been attributed to nobody.
pub(super) fn drop_attributed(mut index: DumpIndex) {
    let _attributed = StatisticsScope::enter();
    for span in &mut index.spans {
        if let SpanBody::Data(DataBlock::Copy(block)) = &mut span.body {
            block.statistics = None;
        }
    }
}

/// Load a cache from `path` with no live source to check it against — the
/// cache-only counterpart to `load` (`docs/design/decisions.md`,
/// "The compressed source and the cache"). With no live source to compare
/// against, `weak` and `origin` are always the agreeing answer here;
/// "unverified, historical" is a
/// [`pgdt_model::DiagnosticKind::CacheOffline`] pushed by
/// [`OfflineCache::load`](super::OfflineCache::load) instead.
pub async fn load_offline(path: &Path) -> Result<CacheStatus> {
    match read_cache_file(path)? {
        Ok(file) => Ok(status_from_file(file, WeakIdentity::Agrees, OriginMatch::Agrees)),
        Err(status) => Ok(status),
    }
}

/// Shared by every load once a `CacheFile` has passed its header and (when
/// live) stored-size and compression checks: the index with its unpersisted
/// diagnostics recomputed, complete or not by its own coverage of the file's
/// recorded addressable length ([`LoadedCache::is_complete`]). Reads [`CacheFile::total_size`] rather than
/// re-deriving one from a live source: by this point the stored sizes are
/// known equal wherever a live source exists, and `load_offline` has none.
fn status_from_file(file: CacheFile, weak: WeakIdentity, origin: OriginMatch) -> CacheStatus {
    let total_size = file.total_size;
    let mut index = file.index;
    // `DumpIndex::diagnostics` is `#[serde(skip)]`, so a loaded index arrives
    // with none, and gets what every whole map carries from the persisted
    // spans, size and seek table (`pgdt_model::diagnostic`, "Not persisted").
    let table = file.compression.as_ref().map(|CompressionIndex::Xz(table)| table);
    index.diagnostics = derived_diagnostics(&index.spans, total_size, table);
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
    CacheStatus::Loaded(LoadedCache { index, weak, origin, total_size, compression, envelope })
}

/// Write `index` to `path` (colocated or explicit — whichever the caller
/// resolved), replacing any existing cache there, and record `source`'s
/// current stored size/mtime plus its addressable length for the next open
/// to check. Propagates I/O failures as `Error::Io` rather than
/// swallowing them — see the module docs.
///
/// **This is the writer, not the run's save**: the in-flight identity check a
/// scan makes rides [`CacheMode::save`](super::CacheMode::save), which is
/// what every scan entry point calls ([`SourceWatch`](super::SourceWatch)),
/// and this function is reached only from there and from tests: `store` is
/// private, and `internal` re-exports it under `test-support` alone.
///
/// **The index is encoded straight into a file beside `path`, which is then
/// renamed over it** (`docs/design/decisions.md`, "D78"), so no encoded copy
/// of the cache is held in memory, and at every moment `path` holds either
/// the previous save or this one, whole
/// (`a_save_leaves_the_previous_cache_whole_until_it_renames`). A save that
/// fails removes its file;
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
    write_cache_file(path, &file)
}
