use std::future::Future;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;

use crate::{Error, Result};

/// Minimal async byte-range read abstraction.
///
/// Shaped to mirror `object_store`'s `get_range`/`head` semantics so a real
/// `object_store`-backed implementation can be added later (behind a Cargo
/// feature flag, see docs/design/roadmap-phase1-mvp.md) without changing this
/// trait.
pub trait ByteRangeSource: Send + Sync {
    fn read_range(&self, offset: u64, len: usize) -> impl Future<Output = Result<Bytes>> + Send;
    fn size(&self) -> impl Future<Output = Result<u64>> + Send;
    /// Last-modified time, if the source exposes one — `object_store`'s
    /// `head` carries this too. `None` rather than an error for a source
    /// that genuinely has no notion of one; the structure cache
    /// (`docs/design/roadmap-phase3-object-inventory.md`, "Cache: the dump
    /// file's identity is checked, not assumed") treats an absent mtime as
    /// nothing to compare against, never as a mismatch.
    fn modified(&self) -> impl Future<Output = Result<Option<SystemTime>>> + Send;
}

/// The only `ByteRangeSource` implementation shipped in the MVP: a local
/// file, read via blocking positioned reads on a `spawn_blocking` task
/// (`tokio` has no native async positioned-read).
pub struct LocalFileSource {
    path: PathBuf,
    file: Arc<std::fs::File>,
}

impl LocalFileSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = std::fs::File::open(&path)?;
        Ok(Self { path, file: Arc::new(file) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl ByteRangeSource for LocalFileSource {
    async fn read_range(&self, offset: u64, len: usize) -> Result<Bytes> {
        let file = Arc::clone(&self.file);
        let buf = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
            let mut buf = vec![0u8; len];
            file.read_exact_at(&mut buf, offset)?;
            Ok(buf)
        })
        .await
        .map_err(Error::from)?
        .map_err(Error::from)?;
        Ok(Bytes::from(buf))
    }

    async fn size(&self) -> Result<u64> {
        let file = Arc::clone(&self.file);
        let len = tokio::task::spawn_blocking(move || file.metadata().map(|m| m.len()))
            .await
            .map_err(Error::from)?
            .map_err(Error::from)?;
        Ok(len)
    }

    async fn modified(&self) -> Result<Option<SystemTime>> {
        let file = Arc::clone(&self.file);
        let mtime = tokio::task::spawn_blocking(move || file.metadata().and_then(|m| m.modified()))
            .await
            .map_err(Error::from)?
            .map_err(Error::from)?;
        Ok(Some(mtime))
    }
}
