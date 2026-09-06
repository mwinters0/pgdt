use std::future::Future;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bytes::Bytes;

use crate::{Error, Result};

/// Minimal async byte-range read abstraction.
///
/// The three required methods are shaped to mirror `object_store`'s
/// `get_range`/`head` semantics so a real `object_store`-backed implementation
/// can be added later (behind a Cargo feature flag — see
/// `docs/design/architecture.md`, "Execution model and API surface") without
/// changing them. [`ByteRangeSource::hint_read_size`] sits outside that mirror
/// and is defaulted, so such an implementation need not know it exists.
///
/// **Dyn-compatible on purpose** (`docs/design/roadmap-P13-compressed-input.md`,
/// "D3 — `ByteRangeSource` becomes dyn-compatible"): each method returns a
/// boxed future rather than `impl Future` (RPITIT), so callers can hold
/// `&dyn ByteRangeSource` / `Arc<dyn ByteRangeSource>` instead of being
/// generic over the source. With one implementation today the boxing is free
/// insurance; P13's `XzSource` and P14's remote source are what it is for.
/// One allocation per `read_range` call is noise beside a chunk-sized read.
pub trait ByteRangeSource: Send + Sync {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>>;
    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>>;
    /// Last-modified time, if the source exposes one — `object_store`'s
    /// `head` carries this too. `None` rather than an error for a source
    /// that genuinely has no notion of one; the structure cache
    /// (`docs/design/architecture.md`, "The cache") treats an absent mtime as
    /// nothing to compare against, never as a mismatch.
    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>>;
    /// The read length this caller is about to ask for over and over — a
    /// read loop's `ScanOptions::chunk_size`, announced once before the loop
    /// starts.
    ///
    /// **Advisory, and it defaults to doing nothing.** It exists because
    /// one-off-ness is a property of the *caller* and nothing in a
    /// `read_range` call carries it: an implementation that recycles buffers
    /// has to tell a chunk read, which repeats for the whole scan, from
    /// `map::attach_text`'s coalesced span read, which happens once per map,
    /// and by length alone it cannot ([`LocalFileSource`], and
    /// `docs/design/architecture.md`, "Execution model and API surface").
    fn hint_read_size(&self, _len: usize) {}
}

/// How many released buffers a [`LocalFileSource`] keeps.
///
/// A scan holds one chunk at a time, so one slot would serve it; the query
/// replay path retains chunks past the read that produced them
/// (`crate::batch::SourceChunk`), so the buffer of chunk *N* can still be
/// alive when chunk *N+1* is read. Four is that depth with room to spare, and
/// it is what bounds the pool's contribution to RSS: four buffers of at most
/// `max(POOL_MAX_BYTES, announced)` bytes each. In the steady state those are
/// chunk buffers — 4 MiB at the default chunk size, four times a raised one —
/// but a sub-ceiling one-off can hold a slot too ([`BufferPool::keeps`]), so
/// an RSS claim is read against the bound rather than against the steady
/// state.
const POOL_SLOTS: usize = 4;

/// The largest buffer worth keeping, in bytes, for a length nobody has
/// announced as a read size.
///
/// The read path's steady state is chunk-sized — 1 MiB by default, and
/// tunable through `ScanOptions::chunk_size`. What can be far larger is
/// `crate::map::attach_text`'s coalesced span read, which happens once per
/// map and never again; holding one of those for the rest of a process would
/// trade the ~5.9 MiB a scan holds resident (`docs/design/measurements.md`,
/// "What a scan holds resident") for an allocation nothing is going to ask for
/// twice.
///
/// **Size stands in for one-off-ness, which is why it is not the only rule.**
/// A chunk buffer at the configured size is asked for once per chunk for the
/// whole scan — the thing the pool is for — and by length alone it is
/// indistinguishable from a span read, so this ceiling on its own turns
/// pooling off for any chunk above it. What tells the two apart is the caller
/// saying so: [`ByteRangeSource::hint_read_size`] names the length a read loop
/// is about to repeat, and a buffer of exactly that length is kept however
/// large it is (`docs/design/architecture.md`, "Execution model and API
/// surface").
const POOL_MAX_BYTES: usize = 8 << 20;

/// Read buffers, reused rather than allocated per chunk.
///
/// **What this is for is not allocator throughput.** A fresh `vec![0u8; len]`
/// per chunk is `calloc`, so the kernel or the allocator zeroes a megabyte
/// that `read_exact_at` immediately overwrites — 23.2% of a warm `parse`'s
/// user time on the 3.00 GiB control
/// (`docs/design/architecture.md`, "parse-profile"). It also hands the region
/// back to the allocator once per chunk, which is what put `jemalloc` at 3,161
/// `madvise` calls against glibc's 50 over the same file
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
    /// The read length a caller announced through
    /// [`ByteRangeSource::hint_read_size`], or `0` for none — a buffer of
    /// exactly this length is kept past [`POOL_MAX_BYTES`].
    ///
    /// It is an atomic rather than part of the `Mutex` because it is written
    /// once per read loop and read once per released buffer, and because
    /// `hint_read_size` takes `&self`: a source is shared, and the announcement
    /// must not have to wait behind a `take` on another task. `Relaxed` is
    /// enough — nothing is published through it, and a hint that arrives a
    /// buffer late costs one allocation.
    hinted: AtomicUsize,
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

    /// Announce the read length a loop is about to repeat, so a buffer of
    /// that length survives [`POOL_MAX_BYTES`].
    fn hint(&self, len: usize) {
        self.hinted.store(len, Ordering::Relaxed);
    }

    /// Whether a released buffer is worth keeping: anything under the ceiling,
    /// plus a buffer of exactly the announced read length.
    ///
    /// **The hint clause matches exactly, not "up to", and it only bites above
    /// the ceiling.** Past [`POOL_MAX_BYTES`] the announced length is the only
    /// thing kept, so a coalesced span read is dropped even when it is
    /// *smaller* than a raised chunk size — which is the pair the hint exists
    /// to tell apart. Below the ceiling nothing is told apart: a span read
    /// under 8 MiB is pooled like any other buffer, exactly as it was before a
    /// hint existed. That is a deliberate floor rather than an oversight —
    /// [`POOL_SLOTS`] bounds what it can cost, and the ceiling has to keep
    /// working for a source no caller ever announced to.
    fn keeps(&self, len: usize) -> bool {
        len <= POOL_MAX_BYTES || len == self.hinted.load(Ordering::Relaxed)
    }

    fn give(&self, buf: Vec<u8>) {
        if !self.keeps(buf.len()) {
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
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        Box::pin(async move {
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
        })
    }

    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        Box::pin(async move {
            let file = Arc::clone(&self.file);
            let len = tokio::task::spawn_blocking(move || file.metadata().map(|m| m.len()))
                .await
                .map_err(Error::from)?
                .map_err(Error::from)?;
            Ok(len)
        })
    }

    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        Box::pin(async move {
            let file = Arc::clone(&self.file);
            let mtime =
                tokio::task::spawn_blocking(move || file.metadata().and_then(|m| m.modified()))
                    .await
                    .map_err(Error::from)?
                    .map_err(Error::from)?;
            Ok(Some(mtime))
        })
    }

    /// **The pool's one non-size rule.** A read loop announcing its chunk size
    /// is what lets a buffer of that length be kept above [`POOL_MAX_BYTES`],
    /// so a raised `ScanOptions::chunk_size` keeps its pooling instead of
    /// paying a fresh `calloc` per chunk. The cost is bounded and is the
    /// caller's own number: [`POOL_SLOTS`] buffers of the size it asked for.
    fn hint_read_size(&self, len: usize) {
        self.pool.hint(len);
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

    /// A buffer above the ceiling that no caller announced as its read length
    /// is dropped rather than pooled, so one oversized `attach_text` read
    /// cannot hold megabytes for the life of the process.
    #[test]
    fn the_pool_refuses_a_buffer_above_the_ceiling() {
        let pool = BufferPool::default();
        pool.give(vec![0u8; POOL_MAX_BYTES + 1]);
        assert!(pool.free.lock().unwrap().is_empty());
        pool.give(vec![0u8; 64]);
        assert_eq!(pool.free.lock().unwrap().len(), 1);
    }

    /// The announced read length is the one thing that survives the ceiling,
    /// and only at exactly that length: a scan configured with a large chunk
    /// keeps its pooling, while a span read of some other oversized length is
    /// still dropped.
    #[test]
    fn the_pool_keeps_a_buffer_of_the_announced_read_length() {
        let chunk = 16 << 20;
        let pool = BufferPool::default();
        pool.give(vec![0u8; chunk]);
        assert!(pool.free.lock().unwrap().is_empty());

        pool.hint(chunk);
        pool.give(vec![0u8; chunk]);
        assert_eq!(pool.free.lock().unwrap().len(), 1);
        // The same read asked for again gets that buffer back rather than a
        // fresh `calloc`, which is the whole point.
        let taken = pool.take(chunk);
        assert_eq!(taken.len(), chunk);
        assert!(pool.free.lock().unwrap().is_empty());
        pool.give(taken);

        // A one-off span read at another oversized length is still dropped,
        // even one *smaller* than the announced chunk.
        pool.give(vec![0u8; chunk - 1]);
        pool.give(vec![0u8; chunk + 1]);
        assert_eq!(pool.free.lock().unwrap().len(), 1);
    }

    /// The hint reaches the pool through the trait method, which is the only
    /// way a read loop has of announcing anything.
    #[test]
    fn a_source_passes_the_hint_to_its_pool() {
        let (_file, source) = source_of(b"0123456789abcdef");
        let chunk = POOL_MAX_BYTES + 1;
        source.hint_read_size(chunk);
        source.pool.give(vec![0u8; chunk]);
        assert_eq!(source.pool.free.lock().unwrap().len(), 1);
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
