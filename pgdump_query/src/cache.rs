//! On-disk structure cache: a serialized [`DumpIndex`], colocated with the
//! dump file by default or at an explicit path (`docs/design/mvp.md`,
//! "Index / structure cache").
//!
//! Reading is best-effort — the cache is never required for correctness, so
//! a missing, foreign, or unrecognised-version file just means "scan
//! instead," never a hard error. Writing is not best-effort: [`save`]
//! propagates I/O failures rather than silently falling back to running
//! without a cache, since a write failure (read-only mount, permissions,
//! disk full) means something is actually wrong and swallowing it would
//! silently degrade every future run of the same command back to a full
//! scan.
//!
//! No dump-file identity check (size/mtime) is performed: `mvp.md` puts that
//! under "Configurable (future)" rather than MVP, so a cache is trusted
//! as-is once its format/container are recognised.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::index::DumpIndex;
use crate::{Error, Result};

/// Bumped whenever the on-disk shape changes incompatibly. A cache written
/// under a different version is treated as absent (`mvp.md`, "Decisions
/// that keep later phases open") rather than partially trusted — the three
/// fields reserved on [`DumpIndex`]/[`crate::index::CopyBlock`] are what let
/// most future additions avoid needing a bump at all.
const FORMAT_VERSION: u32 = 1;

/// What produced the indexed blocks' byte offsets. Plain-format offsets are
/// raw file positions; a future archive format's (roadmap Phase 6, Track B)
/// are entry-relative, so the two must never be silently conflated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum ContainerKind {
    Plain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFile {
    format_version: u32,
    container_kind: ContainerKind,
    index: DumpIndex,
}

/// The default cache location when no explicit path is given:
/// `<dump-path>.dqcache`.
pub fn colocated_path(dump_path: &Path) -> PathBuf {
    let mut path = dump_path.as_os_str().to_owned();
    path.push(".dqcache");
    PathBuf::from(path)
}

/// Load a cache from `path`. `Ok(None)` covers every "no usable cache" case
/// that isn't a hard I/O error — file absent, or bytes this build's
/// format/container doesn't recognise — since both are safe to treat as
/// absent under the cache's best-effort contract.
pub fn load(path: &Path) -> Result<Option<DumpIndex>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::Io(e)),
    };
    let file: CacheFile =
        match bincode::serde::decode_from_slice(&bytes, bincode::config::standard()) {
            Ok((file, _)) => file,
            Err(_) => return Ok(None),
        };
    if file.format_version != FORMAT_VERSION || file.container_kind != ContainerKind::Plain {
        return Ok(None);
    }
    Ok(Some(file.index))
}

/// Write `index` to `path` (colocated or explicit — whichever the caller
/// resolved), overwriting any existing cache there. Propagates I/O failures
/// as `Error::Io` rather than swallowing them — see the module docs.
pub fn save(path: &Path, index: &DumpIndex) -> Result<()> {
    let file = CacheFile {
        format_version: FORMAT_VERSION,
        container_kind: ContainerKind::Plain,
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

    /// Load the cache this mode points at. A disabled cache always yields
    /// `Ok(None)` — even if a file happens to sit at what would otherwise be
    /// its resolved location, per [`CacheMode::Disabled`]'s contract that
    /// existing files are ignored, never read.
    pub fn load(&self) -> Result<Option<DumpIndex>> {
        match self {
            CacheMode::Enabled(path) => load(path),
            CacheMode::Disabled => Ok(None),
        }
    }

    /// Persist `index` per this mode. A disabled cache is a no-op — nothing
    /// is written, by design, not as a fallback. Callers whose operation's
    /// whole purpose is to populate the cache should reject a disabled mode
    /// up front with [`CacheMode::require_enabled`] instead of relying on
    /// this silently doing nothing.
    pub fn save(&self, index: &DumpIndex) -> Result<()> {
        match self {
            CacheMode::Enabled(path) => save(path, index),
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
