//! On-disk structure cache: a serialized [`DumpIndex`], colocated with the
//! dump file by default or at an explicit path
//! (`docs/design/roadmap-phase1-mvp.md`, "Index / structure cache").
//!
//! Reading is best-effort — the cache is never required for correctness, so
//! a missing, foreign, unrecognised-version, or (as of Phase 3.2, see below)
//! size-mismatched file just means "scan instead," never a hard error.
//! Writing is not best-effort: [`save`] propagates I/O failures rather than
//! silently falling back to running without a cache, since a write failure
//! (read-only mount, permissions, disk full) means something is actually
//! wrong and swallowing it would silently degrade every future run of the
//! same command back to a full scan.
//!
//! **The dump file's identity is checked, not assumed**
//! (`docs/design/roadmap-phase3-object-inventory.md`, "Cache: the dump
//! file's identity is checked, not assumed"). Every cache records the
//! source's size and mtime as observed at save time; [`load`] re-observes
//! the live source and compares. A size mismatch means every byte offset in
//! the cache could be wrong, so the cache is invalidated the same way a
//! foreign or wrong-version file is — silently, per this module's
//! best-effort contract, not as a hard error. An mtime mismatch is weaker
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

use crate::index::DumpIndex;
use crate::io::ByteRangeSource;
use crate::{Error, Result};

/// Bumped whenever the on-disk shape changes incompatibly. A cache written
/// under a different version is treated as absent (`roadmap-phase1-mvp.md`,
/// "Decisions that keep later phases open") rather than partially trusted —
/// the three fields reserved on [`DumpIndex`]/[`crate::index::CopyBlock`] are
/// what let most future additions avoid needing a bump at all.
///
/// Bumped to 2 in Phase 3.2 for [`SourceIdentity`]; to 3 in Phase 3.2.1, when
/// `DumpIndex::blocks` (a stored `Vec<CopyBlock>`) was replaced by
/// `DumpIndex::spans` (a stored `Vec<Span>`, with `blocks()` now a derived
/// filter over it); to 4 in Phase 3.2.1.2.1 for
/// [`crate::index::CopyBlock::partition_root`]; to 5 in Phase 3.2.2 for
/// [`crate::map::Span::text`]; to 6 in Phase 3.3 for
/// [`crate::map::Span::toc`] — pre-1.0, so all of them are free (`CLAUDE.md`,
/// "Pre-1.0").
const FORMAT_VERSION: u32 = 6;

/// The dump file's size and modification time as observed when a cache was
/// last saved — see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct SourceIdentity {
    size: u64,
    /// `(seconds, nanoseconds)` since the Unix epoch — `SystemTime` itself
    /// isn't `Serialize`, and this is L1's own on-disk vocabulary rather
    /// than borrowing `std`'s. `None` when the source exposed no mtime.
    mtime: Option<(u64, u32)>,
}

impl SourceIdentity {
    async fn observe<S: ByteRangeSource>(source: &S) -> Result<Self> {
        let size = source.size().await?;
        let mtime = source
            .modified()
            .await?
            .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default())
            .map(|d| (d.as_secs(), d.subsec_nanos()));
        Ok(Self { size, mtime })
    }
}

/// What produced the indexed blocks' byte offsets. Plain-format offsets are
/// raw file positions; a future archive format's (roadmap Phase 8, Track B)
/// are entry-relative, so the two must never be silently conflated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum ContainerKind {
    Plain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFile {
    format_version: u32,
    container_kind: ContainerKind,
    identity: SourceIdentity,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheStatus {
    /// No usable cache: absent, foreign bytes, an unrecognised
    /// format/container version, or a source-size mismatch. All four are
    /// the same "not trustworthy" outcome under this module's best-effort
    /// contract, so callers that don't care why get exactly one case to
    /// handle.
    Absent,
    /// A usable cache. `mtime_changed` is `true` when the source's current
    /// mtime differs from the one recorded at save time — weaker evidence
    /// than a size mismatch (see the module docs), so it does not itself
    /// make the cache [`Absent`](CacheStatus::Absent).
    Valid { index: DumpIndex, mtime_changed: bool },
}

/// Load a cache from `path` and validate it against `source`'s current
/// identity. See [`CacheStatus`] and the module docs for what "validate"
/// means. A hard I/O error reading `path` (anything but "not found") still
/// propagates — only the cache's own *content* is best-effort.
pub async fn load<S: ByteRangeSource>(path: &Path, source: &S) -> Result<CacheStatus> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(CacheStatus::Absent),
        Err(e) => return Err(Error::Io(e)),
    };
    let file: CacheFile =
        match bincode::serde::decode_from_slice(&bytes, bincode::config::standard()) {
            Ok((file, _)) => file,
            Err(_) => return Ok(CacheStatus::Absent),
        };
    if file.format_version != FORMAT_VERSION || file.container_kind != ContainerKind::Plain {
        return Ok(CacheStatus::Absent);
    }
    let live = SourceIdentity::observe(source).await?;
    if file.identity.size != live.size {
        return Ok(CacheStatus::Absent);
    }
    let mtime_changed = file.identity.mtime != live.mtime;
    Ok(CacheStatus::Valid { index: file.index, mtime_changed })
}

/// Write `index` to `path` (colocated or explicit — whichever the caller
/// resolved), overwriting any existing cache there, and record `source`'s
/// current size/mtime for [`load`] to check next time. Propagates I/O
/// failures as `Error::Io` rather than swallowing them — see the module
/// docs.
pub async fn save<S: ByteRangeSource>(path: &Path, source: &S, index: &DumpIndex) -> Result<()> {
    let identity = SourceIdentity::observe(source).await?;
    let file = CacheFile {
        format_version: FORMAT_VERSION,
        container_kind: ContainerKind::Plain,
        identity,
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
    pub async fn load<S: ByteRangeSource>(&self, source: &S) -> Result<Option<DumpIndex>> {
        match self {
            CacheMode::Enabled(path) => match load(path, source).await? {
                CacheStatus::Absent => Ok(None),
                CacheStatus::Valid { mut index, mtime_changed } => {
                    // Reported rather than acted on: too weak to invalidate
                    // (see the module docs), and pointless to persist — the
                    // mismatch is between the cache and *this* run's
                    // observation, so it is recomputed on every load.
                    if mtime_changed {
                        index
                            .diagnostics
                            .push(crate::diagnostic::Diagnostic::cache_mtime_changed());
                    }
                    Ok(Some(index))
                }
            },
            CacheMode::Disabled => Ok(None),
        }
    }

    /// Persist `index` per this mode. A disabled cache is a no-op — nothing
    /// is written, by design, not as a fallback. Callers whose operation's
    /// whole purpose is to populate the cache should reject a disabled mode
    /// up front with [`CacheMode::require_enabled`] instead of relying on
    /// this silently doing nothing.
    pub async fn save<S: ByteRangeSource>(&self, source: &S, index: &DumpIndex) -> Result<()> {
        match self {
            CacheMode::Enabled(path) => save(path, source, index).await,
            CacheMode::Disabled => Ok(()),
        }
    }

    /// The path this mode resolves to, or `Error::CacheDisabled` if the
    /// caller disabled the cache but `operation` requires one. Use this to
    /// reject an incompatible `--cache-path none` up front, rather than
    /// discovering it only when a write silently no-ops.
    pub fn require_enabled(&self, operation: &'static str) -> Result<&Path> {
        match self {
            CacheMode::Enabled(path) => Ok(path),
            CacheMode::Disabled => Err(Error::CacheDisabled { operation }),
        }
    }
}
