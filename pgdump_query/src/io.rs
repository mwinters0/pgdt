use std::future::Future;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bytes::Bytes;

use crate::{Error, Result};

/// Minimal async byte-range read abstraction.
///
/// Shaped to mirror `object_store`'s `get_range`/`head` semantics so a real
/// `object_store`-backed implementation can be added later (behind a Cargo
/// feature flag — see `docs/design/architecture.md`, "Execution model and API
/// surface") without changing this trait.
pub trait ByteRangeSource: Send + Sync {
    fn read_range(&self, offset: u64, len: usize) -> impl Future<Output = Result<Bytes>> + Send;
    fn size(&self) -> impl Future<Output = Result<u64>> + Send;
    /// Last-modified time, if the source exposes one — `object_store`'s
    /// `head` carries this too. `None` rather than an error for a source
    /// that genuinely has no notion of one; the structure cache
    /// (`docs/design/architecture.md`, "The cache") treats an absent mtime as
    /// nothing to compare against, never as a mismatch.
    fn modified(&self) -> impl Future<Output = Result<Option<SystemTime>>> + Send;
}

/// How many released buffers a [`LocalFileSource`] keeps.
///
/// A scan holds one chunk at a time, so one slot would serve it; the query
/// replay path retains chunks past the read that produced them
/// (`crate::batch::SourceChunk`), so the buffer of chunk *N* can still be
/// alive when chunk *N+1* is read. Four is that depth with room to spare, and
/// it is what bounds the pool's contribution to RSS.
const POOL_SLOTS: usize = 4;

/// The largest buffer worth keeping, in bytes.
///
/// The read path's steady state is chunk-sized — 1 MiB by default, and
/// tunable through `ScanOptions::chunk_size`. What can be far larger is
/// `crate::map::attach_text`'s coalesced span read, which happens once per
/// map and never again; holding one of those for the rest of a process would
/// trade the flat ~9 MiB RSS this design is built around for an allocation
/// nothing is going to ask for twice.
const POOL_MAX_BYTES: usize = 8 << 20;

/// Read buffers, reused rather than allocated per chunk.
///
/// **What this is for is not allocator throughput.** A fresh `vec![0u8; len]`
/// per chunk is `calloc`, so the kernel or the allocator zeroes a megabyte
/// that `read_exact_at` immediately overwrites — 23.2% of a warm `parse`'s
/// user time on the 3.00 GiB control
/// (`docs/design/architecture.md`, "`parse`: three-quarters of the wall is the
/// kernel, and the rest is two SIMD passes"). It also hands the region back to
/// the
/// allocator once per chunk, which is what put `jemalloc` at 3,161 `madvise`
/// calls against glibc's 50 over the same file
/// (`docs/design/measurements.md`, "Which allocator a figure was taken
/// under"). Neither cost is work the problem requires: the same megabyte is
/// wanted again a moment later.
///
/// **A pooled buffer is fully initialized and stays at its own length.** It
/// is created once as `vec![0u8; len]` and thereafter only ever read into, so
/// no reuse memsets anything. A read shorter than the buffer takes the
/// buffer whole and the `Bytes` handed back is sliced down to the bytes that
/// were actually read, which is why a short final chunk does not shrink a
/// pooled buffer and force the next full chunk to grow one back.
#[derive(Debug, Default)]
struct BufferPool {
    /// A poisoned lock is not a corruption hazard here — the only thing under
    /// it is a list of scratch buffers — so every caller recovers the guard
    /// rather than propagating a panic from an unrelated task.
    free: Mutex<Vec<Vec<u8>>>,
}

impl BufferPool {
    /// A fully initialized buffer of at least `len` bytes: the smallest
    /// pooled one that fits, or a fresh allocation.
    fn take(&self, len: usize) -> Vec<u8> {
        let mut free = self.free.lock().unwrap_or_else(|e| e.into_inner());
        let pick = free
            .iter()
            .enumerate()
            .filter(|(_, buf)| buf.len() >= len)
            .min_by_key(|(_, buf)| buf.len())
            .map(|(i, _)| i);
        match pick {
            Some(i) => free.swap_remove(i),
            None => vec![0u8; len],
        }
    }

    fn give(&self, buf: Vec<u8>) {
        if buf.len() > POOL_MAX_BYTES {
            return;
        }
        let mut free = self.free.lock().unwrap_or_else(|e| e.into_inner());
        if free.len() < POOL_SLOTS {
            free.push(buf);
        }
    }
}

/// The owner behind a [`BufferPool`]-backed [`Bytes`]: it returns the buffer
/// when the last reference to those bytes is dropped, which is what lets the
/// query path keep a chunk alive for as long as a zero-copy view points into
/// it and still get the buffer back afterwards.
struct PooledBuffer {
    /// `None` only between [`Drop`] taking the buffer and the struct going
    /// away, which no `as_ref` can observe.
    buf: Option<Vec<u8>>,
    pool: Arc<BufferPool>,
}

impl AsRef<[u8]> for PooledBuffer {
    fn as_ref(&self) -> &[u8] {
        self.buf.as_deref().unwrap_or(&[])
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        if let Some(buf) = self.buf.take() {
            self.pool.give(buf);
        }
    }
}

/// The only `ByteRangeSource` implementation shipped in the MVP: a local
/// file, read via blocking positioned reads on a `spawn_blocking` task
/// (`tokio` has no native async positioned-read).
pub struct LocalFileSource {
    path: PathBuf,
    file: Arc<std::fs::File>,
    pool: Arc<BufferPool>,
}

impl LocalFileSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = std::fs::File::open(&path)?;
        Ok(Self { path, file: Arc::new(file), pool: Arc::new(BufferPool::default()) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl ByteRangeSource for LocalFileSource {
    async fn read_range(&self, offset: u64, len: usize) -> Result<Bytes> {
        let file = Arc::clone(&self.file);
        let pool = Arc::clone(&self.pool);
        let buf = pool.take(len);
        // A read that fails drops its buffer instead of returning it: the
        // pool is an optimization, and an error path is the one place where
        // re-allocating costs nothing anyone will measure.
        let buf = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
            let mut buf = buf;
            file.read_exact_at(&mut buf[..len], offset)?;
            Ok(buf)
        })
        .await
        .map_err(Error::from)?
        .map_err(Error::from)?;
        Ok(Bytes::from_owner(PooledBuffer { buf: Some(buf), pool }).slice(..len))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn source_of(bytes: &[u8]) -> (tempfile::NamedTempFile, LocalFileSource) {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(bytes).unwrap();
        file.flush().unwrap();
        let source = LocalFileSource::open(file.path()).unwrap();
        (file, source)
    }

    /// The pool's whole risk: a reused buffer still holding the previous
    /// read's bytes past the length of the current one. Reading a shorter
    /// range after a longer one from the same source must see the file's
    /// bytes and nothing else.
    #[tokio::test]
    async fn short_read_after_long_read_sees_only_its_own_bytes() {
        let (_file, source) = source_of(b"0123456789abcdef");
        let long = source.read_range(0, 16).await.unwrap();
        assert_eq!(&long[..], b"0123456789abcdef");
        drop(long);
        let short = source.read_range(0, 4).await.unwrap();
        assert_eq!(&short[..], b"0123");
        let elsewhere = source.read_range(10, 3).await.unwrap();
        assert_eq!(&elsewhere[..], b"abc");
    }

    /// A retained chunk must not be handed back out while it is still
    /// referenced — the query path holds chunks past the read that produced
    /// them, and a zero-copy view into a recycled buffer would silently
    /// return another chunk's bytes.
    #[tokio::test]
    async fn a_retained_chunk_is_not_recycled_under_it() {
        let (_file, source) = source_of(b"0123456789abcdef");
        let held = source.read_range(0, 8).await.unwrap();
        let next = source.read_range(8, 8).await.unwrap();
        assert_eq!(&held[..], b"01234567");
        assert_eq!(&next[..], b"89abcdef");
    }

    /// A buffer above the ceiling is dropped rather than pooled, so one
    /// oversized `attach_text` read cannot hold megabytes for the life of the
    /// process.
    #[test]
    fn the_pool_refuses_a_buffer_above_the_ceiling() {
        let pool = BufferPool::default();
        pool.give(vec![0u8; POOL_MAX_BYTES + 1]);
        assert!(pool.free.lock().unwrap().is_empty());
        pool.give(vec![0u8; 64]);
        assert_eq!(pool.free.lock().unwrap().len(), 1);
    }

    /// The pool holds `POOL_SLOTS` buffers and no more, and a take picks the
    /// smallest that fits rather than the first.
    #[test]
    fn the_pool_is_bounded_and_takes_the_smallest_that_fits() {
        let pool = BufferPool::default();
        for len in [8usize, 64, 32, 16, 128] {
            pool.give(vec![0u8; len]);
        }
        assert_eq!(pool.free.lock().unwrap().len(), POOL_SLOTS);
        assert_eq!(pool.take(16).len(), 16);
        assert_eq!(pool.take(16).len(), 32);
        // Nothing left that fits: a fresh allocation, exactly as long as asked.
        assert_eq!(pool.take(1024).len(), 1024);
    }
}
