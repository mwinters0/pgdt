//! The compressed source over a local file: a unit decoded whole where the
//! budget holds it, piecewise past it.

use std::future::Future;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bytes::Bytes;

use pgdt_core::{Error, Result};

use crate::parallelism::{Parallelism, WorkerMemory};
use crate::partition::Partitioning;
use crate::source::pool::BufferPool;
use crate::source::{ByteRangeSource, Extent, WaitPolicy};

use super::blocks::{BlockCache, Claim, DecodeBudget, DecodedBlock};
use super::deflate::{DeflateCodec, is_bgzf, walk_file};
use super::xz::XzCodec;
use super::{
    Codec, CompressionIndex, DumpFile, Format, LiveBlock, Positioned, RECOGNITION_BYTES,
    codec_over, piece_of, short_read,
};

/// A `ByteRangeSource` decoding a compressed local file on the fly, whatever
/// its codec (`docs/design/decisions.md`, "D14", "D15").
///
/// **One file handle, positioned reads only.** Nothing here holds a cursor:
/// `stat` answers
/// [`ByteRangeSource::stored_size`]/[`ByteRangeSource::modified`], and every
/// decode reads its compressed bytes through `pread`, so the handle is shared
/// by everything and disturbs nothing. A decode pulls from it as a
/// [`Positioned`] read, which is where this source and
/// [`FetchedCompressedSource`](crate::FetchedCompressedSource) part, whose
/// bytes have to be fetched into a window first
/// (`docs/design/decisions.md`, "D15").
///
/// **Two arms, and the budget picks between them**, as the fetched source's do:
///
/// - Where the stated budget holds a whole decoded unit, a read decodes the
///   units it lands in into slots of its own [`BlockCache`] and slices the
///   answer out of them. The codec names a unit from its index alone, so no
///   lock is held across a decode; what this arm does take is its own
///   [`BlockCache`]'s retention list.
/// - Where it does not, the unit is read **in pieces** through one live
///   handle, kept across reads so a forward scan continues rather than
///   restarting ([`LiveBlock`]).
///
/// A whole-unit decode compares the check before it returns, and the
/// piecewise arm compares it when it leaves the unit, or as the run ends for
/// the unit a read left it inside ([`ByteRangeSource::complete_reads`]).
pub struct CompressedFileSource {
    /// The file and its path, which a failing read names: `stat` and every
    /// decode share it.
    pub(super) file: Arc<DumpFile>,
    /// The file's units and how to decode one — everything a decode needs
    /// but the file's bytes, a gzip unit's window read through the provider
    /// its index carries. It holds no source.
    pub(super) codec: Arc<dyn Codec>,
    /// The live piecewise decode, or `None` where no read has taken that arm
    /// yet, its last decode failed or `complete_reads` has left it. Behind a
    /// `Mutex` because a handle goes forward only and one position cannot
    /// serve two readers — which is why that arm advises a single partition
    /// ([`partition_advice`](super::blocks::partition_advice)).
    live: Arc<Mutex<Option<LiveBlock>>>,
    /// What the caller announced and everything derived from it: the two
    /// pools, which arm a read takes, and what a reader of this file costs.
    /// [`FetchedCompressedSource`](crate::FetchedCompressedSource) holds the
    /// same value ([`DecodeBudget`]).
    pub(super) budget: DecodeBudget,
}

impl CompressedFileSource {
    /// Open `path` as a compressed file of whichever format its first bytes
    /// name, building its index before this call returns — for `.xz`, walking
    /// its stream footers, and for BGZF, its members' headers.
    ///
    /// A file whose first bytes name no format this build reads is refused,
    /// [`Error::SourceNotReadable`], and so is any other gzip one, whose index
    /// only a first pass builds — which [`open`](crate::open) runs — so it
    /// opens here only through [`CompressedFileSource::with_index`].
    /// Recognition that falls back to the plain source, and checks an index a
    /// cache hands in, is [`open`](crate::open).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = DumpFile::open(path.as_ref())?;
        let mut leading = [0u8; RECOGNITION_BYTES];
        let read = Positioned::read_at(&file, 0, &mut leading)?;
        let leading = &leading[..read];
        let named = file.path().display().to_string();
        let refuse =
            |why: &str| Error::SourceNotReadable { origin: named.clone(), why: why.to_string() };
        match Format::recognize(leading) {
            None => Err(refuse("its first bytes name no compressed format this build reads")),
            Some(Format::Xz) => Self::walked_xz(file),
            Some(Format::Gzip) => {
                let walked = if is_bgzf(leading) { Self::walked_bgzf(file, &named)? } else { None };
                walked.ok_or_else(|| {
                    refuse(
                        "it is gzip-compressed and not BGZF, and such a file's index is built by \
                         a first pass over it, which `pgdt_io::open` runs: this source opens one \
                         only over a complete index (`CompressedFileSource::with_index`)",
                    )
                })
            }
        }
    }

    /// Open the `.xz` file at `path`, walking its stream footers for its
    /// table: what [`open`](crate::open) does where the origin's probe has
    /// already recognised it.
    pub(crate) fn xz(path: &Path) -> Result<Self> {
        Self::walked_xz(DumpFile::open(path)?)
    }

    fn walked_xz(file: DumpFile) -> Result<Self> {
        let stored_size = file.metadata()?.len();
        let codec = Arc::new(XzCodec::walk(&file, stored_size)?);
        Ok(Self::over(Arc::new(file), codec))
    }

    /// Open the BGZF file at `path`, walking its members' headers for its
    /// index and naming it `named` as it says so: `None` where the chain
    /// breaks, the file then being a first pass's to read
    /// ([`open`](crate::open)).
    pub(crate) fn bgzf(path: &Path, named: &str) -> Result<Option<Self>> {
        Self::walked_bgzf(DumpFile::open(path)?, named)
    }

    fn walked_bgzf(file: DumpFile, named: &str) -> Result<Option<Self>> {
        let stored_size = file.metadata()?.len();
        let Some(index) = walk_file(named, &file, stored_size)? else {
            return Ok(None);
        };
        let codec = Arc::new(DeflateCodec::over(index, stored_size)?);
        Ok(Some(Self::over(Arc::new(file), codec)))
    }

    /// Open `path` from an index a previous build of the *same* file
    /// produced, **without building it again**
    /// (`docs/design/decisions.md`, "D18").
    ///
    /// The codec validates the index against the file's own length before
    /// trusting it, and a seek table for internal consistency, reading **no
    /// bytes** — so an
    /// index that does not describe this file costs nothing to reject, and a
    /// unit's own check is what catches one that survives validation. A
    /// rejected index is the error [`open`](crate::open) turns into
    /// [`Recognized::Mismatch`](crate::Recognized::Mismatch) rather than a
    /// rebuild.
    pub fn with_index(path: impl AsRef<Path>, index: CompressionIndex) -> Result<Self> {
        let file = DumpFile::open(path.as_ref())?;
        let stored_size = file.metadata()?.len();
        let codec = codec_over(index, stored_size)?;
        Ok(Self::over(Arc::new(file), codec))
    }

    /// The one place the constructors agree: the budget policy is built over
    /// the codec's largest unit ([`DecodeBudget`]).
    ///
    /// **The decoder charge includes the compressed input chunk because this
    /// source does not lend**: a file holds nothing, so every decode here
    /// builds that buffer, where
    /// [`FetchedCompressedSource`](crate::FetchedCompressedSource) reads out of
    /// bytes it already has and charges the lending footprint plus the window
    /// instead.
    ///
    /// **Nothing bills the index.** It is unbilled on an ordering rather than
    /// a size: the build that makes it runs inside
    /// [`CompressedFileSource::open`], or before [`Self::with_index`] is
    /// handed it, before any budget is announced
    /// (`docs/design/decisions.md`, "D4").
    // Deficiency register: `deficiency: KD26` — this index is the only term in
    // the memory account that grows with the *file* rather than with the
    // worker count, and a source whose index is not small beside
    // [`MEMORY_UNPOOLED_BOUND`] therefore holds bytes no charge names.
    // **(c) unowned**; closing it means a per-source term, which
    // [`WorkerMemory`] does not have. Billing it buys accuracy and no
    // protection, the build having already happened.
    fn over(file: Arc<DumpFile>, codec: Arc<dyn Codec>) -> Self {
        let budget = DecodeBudget::over(codec.largest_unit(), codec.pulling_footprint());
        Self { file, codec, live: Arc::new(Mutex::new(None)), budget }
    }

    pub fn path(&self) -> &Path {
        self.file.path()
    }

    /// The unit at `index`, from the retention list, from another reader's
    /// decode of it, or freshly decoded into a slot of the block pool
    /// ([`BlockCache::claim`]).
    ///
    /// **A wait here blocks the thread**, which is the `spawn_blocking` one
    /// [`CompressedFileSource::read_range`](ByteRangeSource::read_range) runs
    /// every read on.
    fn block(
        index: usize,
        cache: &BlockCache,
        codec: &dyn Codec,
        file: &DumpFile,
    ) -> Result<Arc<DecodedBlock>> {
        let decoding = loop {
            match cache.claim(index) {
                Claim::Hit(block) => return Ok(block),
                Claim::Wait(flight) => {
                    if let Some(block) = flight.wait() {
                        return Ok(block);
                    }
                }
                Claim::Decode(decoding) => break decoding,
            }
        };
        let unit = codec.plaintext(index).ok_or_else(short_read)?;
        let len = usize::try_from(unit.end - unit.start).map_err(|_| short_read())?;
        // A slot is the file's largest unit, so `len` is never above it and a
        // released slot is exactly the length the pool was hinted with.
        let mut slot = cache.slot();
        // A decode that fails drops its slot, which `Drop` releases to the
        // pool as the plain source's failed read does, and its claim, which
        // leaves whoever was waiting on it to claim the unit again.
        codec.decode(index, file, &mut slot.as_mut()[..len])?;
        Ok(decoding.land(DecodedBlock { slot, len }))
    }

    /// A read served by decoding the units it lands in.
    ///
    /// A read inside one unit is a **slice of that unit**
    /// ([`DecodedBlock::slice`]), so the common case copies nothing; one that
    /// straddles a boundary is assembled into a chunk-pool buffer
    /// ([`DecodedBlock::copy_piece`]), one covering unit held at a time.
    pub(super) fn read_by_blocks(
        offset: u64,
        len: usize,
        cache: &BlockCache,
        codec: &dyn Codec,
        file: &DumpFile,
        chunks: &Arc<BufferPool>,
    ) -> Result<Bytes> {
        let end = offset.checked_add(len as u64).ok_or_else(short_read)?;
        let covering = codec.units_in(offset..end);
        if covering.is_empty() {
            return Err(short_read());
        }
        let base = |index| codec.plaintext(index).map(|unit| unit.start).ok_or_else(short_read);
        if covering.len() == 1 {
            let index = covering.start;
            let block = Self::block(index, cache, codec, file)?;
            // `units_in` answers the unit *covering* the start.
            return block.slice(base(index)?, offset, len);
        }
        let mut out = chunks.obtain(len);
        let mut covered = 0usize;
        for index in covering {
            let block = Self::block(index, cache, codec, file)?;
            covered += block.copy_piece(base(index)?, offset, end, out.as_mut());
        }
        if covered != len {
            return Err(short_read());
        }
        Ok(Bytes::from_owner(out).slice(..len))
    }

    /// A read served **without holding a whole decoded unit**: the covering
    /// units are read in pieces through one live handle, which holds the
    /// decoder and whatever input it builds and never the plaintext.
    ///
    /// It is what a file whose largest unit does not fit the stated budget is
    /// read through ([`BlockCache::affordable`]), where decoding a unit whole
    /// would allocate more than the caller allowed — up to the whole file, on
    /// a single-unit one — and it is the same mechanism
    /// [`FetchedCompressedSource::read_in_pieces`](crate::FetchedCompressedSource::read_in_pieces)
    /// drives, with a file in place of a fetched window
    /// (`docs/design/decisions.md`, "D15").
    ///
    /// **The handle is kept across reads**, which is what makes a forward scan
    /// cost one decode of the file rather than one per read ([`LiveBlock`]).
    /// The mutex is therefore held across the decode, and one position cannot
    /// serve two readers — the single partition
    /// [`partition_advice`](super::blocks::partition_advice)'s other arm
    /// advises.
    fn read_in_pieces(
        offset: u64,
        len: usize,
        codec: &dyn Codec,
        file: &DumpFile,
        live: &Mutex<Option<LiveBlock>>,
        chunks: &Arc<BufferPool>,
    ) -> Result<Bytes> {
        let end = offset.checked_add(len as u64).ok_or_else(short_read)?;
        let covering = codec.units_in(offset..end);
        if covering.is_empty() {
            return Err(short_read());
        }
        let mut out = chunks.obtain(len);
        let mut live = live.lock().unwrap_or_else(|e| e.into_inner());
        let mut covered = 0usize;
        for index in covering {
            let unit = codec.plaintext(index).ok_or_else(short_read)?;
            let Some((at, from, n)) = piece_of(unit, offset, end) else {
                continue;
            };
            // A failed decode leaves no state the next read can reason about,
            // so the handle is dropped rather than completed, there being
            // nothing to verify against.
            let filled = match LiveBlock::serve(
                &mut live,
                codec,
                index,
                file,
                from,
                &mut out.as_mut()[at..at + n],
            ) {
                Ok(filled) => filled,
                Err(e) => {
                    *live = None;
                    return Err(e);
                }
            };
            covered += filled;
        }
        drop(live);
        if covered != len {
            return Err(short_read());
        }
        Ok(Bytes::from_owner(out).slice(..len))
    }
}

impl ByteRangeSource for CompressedFileSource {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        Box::pin(async move {
            // A zero-length read covers no units, so it is answered here
            // rather than left to look like a read past the end.
            if len == 0 {
                return Ok(Bytes::new());
            }
            let chunks = Arc::clone(&self.budget.pool);
            let codec = Arc::clone(&self.codec);
            let file = Arc::clone(&self.file);
            let live = Arc::clone(&self.live);
            let blocks = self.budget.block_path().cloned();
            tokio::task::spawn_blocking(move || match blocks {
                Some(cache) => Self::read_by_blocks(offset, len, &cache, &*codec, &file, &chunks),
                None => Self::read_in_pieces(offset, len, &*codec, &file, &live, &chunks),
            })
            .await
            .map_err(Error::join)?
        })
    }

    /// The plaintext length, from the index built at open — no further
    /// decode, no I/O, and no lock, and exact, the index being complete.
    fn extent(&self) -> Pin<Box<dyn Future<Output = Result<Extent>> + Send + '_>> {
        Box::pin(async move { Ok(Extent::Exact(self.codec.plaintext_size())) })
    }

    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        Box::pin(async move {
            let file = Arc::clone(&self.file);
            let mtime = tokio::task::spawn_blocking(move || file.modified())
                .await
                .map_err(Error::join)??;
            Ok(Some(mtime))
        })
    }

    /// The compressed file's own on-disk length — a `stat` on the handle every
    /// decode also reads through, never the compressed length the index
    /// answers from memory (`docs/design/decisions.md`, "D21").
    fn stored_size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        Box::pin(async move {
            let file = Arc::clone(&self.file);
            let len = tokio::task::spawn_blocking(move || file.metadata().map(|m| m.len()))
                .await
                .map_err(Error::join)??;
            Ok(len)
        })
    }

    fn hint_read_size(&self, len: usize) {
        self.budget.hint_read_size(len);
    }

    fn hint_parallelism(&self, parallelism: Parallelism) {
        self.budget.hint_parallelism(parallelism);
    }

    /// **The permission reaches the chunk pool only.** A chunk buffer is taken
    /// by one `read_range` and returned when its bytes are dropped, so a loop
    /// releasing each chunk before its next read holds one of them — which is
    /// the discipline
    /// [`BufferPool::obtain`] documents. The **block** pool's holder is
    /// [`BlockCache`], which *retains*: it is the exempt class, exactly as the
    /// replay loop is, so it is left at [`WaitPolicy::NeverWait`] whatever a
    /// loop permits (`docs/design/decisions.md`, "D5").
    ///
    /// A read that needs both takes the **chunk** slot first and the block
    /// slot inside it ([`CompressedFileSource::read_by_blocks`]), never the
    /// other way round, so a waiting reader holds no block slot at all.
    fn hint_wait_policy(&self, policy: WaitPolicy) {
        self.budget.pool.set_policy(policy);
    }

    fn compression_index(&self) -> Option<CompressionIndex> {
        Some(self.codec.index())
    }

    fn partitions(&self, range: Range<u64>) -> Partitioning {
        self.budget.partitions(&*self.codec, range)
    }

    fn block_decode_bytes(&self) -> Option<u64> {
        self.budget.block_decode_bytes()
    }

    /// **The cores this process was given** — `available_parallelism()`, which
    /// is already the minimum of the affinity mask and the CPU quota of every
    /// ancestor cgroup it can see, rounded down and floored at one
    /// (`docs/design/runtime-invariants.md`, `RT7`). So a container told
    /// `--cpus=3.5` recommends three, and nothing here re-derives what `std`
    /// already reads.
    ///
    /// **Decode is the one shape that demonstrably scales**
    /// (`parallel-scan-throughput`; `docs/design/decisions.md`, "D2"). What
    /// the caller's budget affords still binds afterwards
    /// (`worker_count`).
    ///
    /// **Capped at this file's own unit count**, `cut` cutting
    /// at unit boundaries — and capped *here*, where the count is
    /// recommended, because the recommendation is multiplied into a budget
    /// request ([`ByteRangeSource::default_worker_memory`]) and printed beside
    /// it ([`Arrangement::resolve`](crate::Arrangement::resolve)).
    ///
    /// A failure to read the count answers **one** rather than propagating.
    fn default_workers(&self) -> usize {
        let cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        cores.min(self.codec.unit_count().max(1))
    }

    /// **What readers of this file hold**: [`DecodeBudget::block_worker_memory`]
    /// — the per-reader charge plus the block pool's retention list, the shape
    /// `worker_count` and [`Parallelism::fit`] both solve
    /// against to hand back a count. `None` where there is no block path to
    /// buy — a file with no units, or one whose largest unit is not a length
    /// on this target — since the piecewise arm has no second worker to
    /// give at any budget.
    ///
    /// **It is charged at the chunk a scan settles at**
    /// ([`DecodeBudget::charged_chunk_bytes`]), which is what
    /// [`BlockCache::affordable`] compares a budget against once the file is
    /// open. *Rejected:* charging the unannounced pool's [`POOL_MAX_BYTES`](crate::source::pool::POOL_MAX_BYTES)
    /// ceiling instead, which runs the recommendation high and resolves fewer
    /// readers than fit.
    fn default_worker_memory(&self) -> Option<WorkerMemory> {
        self.budget.block_worker_memory()
    }

    /// Leave the live piecewise unit, if a read left one seated, comparing
    /// its check ([`LiveBlock`]). The whole-unit arm compares before a read
    /// returns, so this is the only unit whose bytes can have gone out
    /// unverified — seated by the piecewise arm, and still seated where a
    /// raised budget moved later reads to the block arm.
    fn complete_reads(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let file = Arc::clone(&self.file);
            let live = Arc::clone(&self.live);
            tokio::task::spawn_blocking(move || {
                let leaving = live.lock().unwrap_or_else(|e| e.into_inner()).take();
                leaving.map_or(Ok(()), |leaving| leaving.leave(&*file))
            })
            .await
            .map_err(Error::join)?
        })
    }
}

#[cfg(test)]
mod tests;
