//! On-disk structure cache: a serialized [`DumpIndex`], colocated with the
//! dump file by default or at an explicit path (`docs/design/mvp.md`,
//! "Index / structure cache"). Best-effort only — the cache is never
//! required for correctness, so a missing, foreign, or unrecognised-version
//! file just means "scan instead," never a hard error.
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
/// resolved), overwriting any existing cache there.
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
