//! One block: decoded whole into a buffer the caller owns, or read through a
//! handle the caller holds.
//!
//! [`BlockTask`] is the public piece a caller drives itself: it names a block,
//! carries everything a decode of that block needs, and is `Send` and `Copy`, so
//! scheduling it is the caller's business and not this crate's. A worker is
//! handed a task and the compressed bytes for it — a [`Window`](crate::Window),
//! usually — and gets the block's entire plaintext back, verified.
//!
//! It exists because the parallel unit a caller wants is often not this crate's:
//! one worker that decodes *and parses* one block on one thread never moves the
//! decoded bytes at all, where an ordered bulk read has to copy them out of a
//! slot. See `docs/design/decisions.md`, "D31".
//!
//! Three properties are the whole of the contract:
//!
//! * **The buffer is the caller's.** Nothing here allocates the output, because
//!   an allocation made in this crate is one the caller's own pool cannot reach.
//! * **It fills or it refuses.** A buffer shorter than the block is
//!   [`Error::BufferTooSmall`] naming the length that would do, never a partial
//!   fill: a short fill on this path could not be told from a block that decoded
//!   short.
//! * **The check is compared before it returns.** A whole-block decode completes
//!   the block, so *"the bytes you have may fail their check two calls from
//!   now"* — true of the seeking path, and stated there — is not true here.
//!
//! # A budget smaller than a block reads the block through a handle
//!
//! [`BlockTask::decode_into`] asks for a buffer the whole block fits in, which a
//! caller whose memory budget is smaller than one block does not have.
//! [`BlockRead`] is the same block taken in pieces: begun from the task, read
//! into whatever buffer the caller has, positioned forward with
//! [`BlockRead::advance_to`], and finished with [`BlockRead::complete`], which
//! compares the check. Both shapes carry the task's *resolved* check, so neither
//! can pair a block with the wrong stream's — `docs/design/decisions.md`, "D67".
//!
//! Fed a [`Window`](crate::Window) holding the block's whole
//! [`compressed_range`](BlockTask::compressed_range), the handle reads the
//! block's header and every byte of its payload out of that one window: one
//! fetch per block however narrow the caller's output buffer is, and a check
//! compared over bytes the caller has already seen.

use core::ops::Range;

use crate::backend::Backend;
use crate::decode::BlockDecode;
use crate::error::{Error, Result};
use crate::reader::Verify;
use crate::source::CompressedSource;
use crate::table::{BlockEntry, Check};

/// One block of one file, and everything needed to decode it.
///
/// Obtained from [`Reader::block_task`](crate::Reader::block_task) by block
/// index — the indices a range covers are
/// [`SeekTable::blocks_in`](crate::SeekTable::blocks_in) — and carrying the
/// reader's own memory limit, verification state and backend, so that a task
/// decodes exactly as a positioned read through that reader would.
///
/// **It carries the block's *resolved* check**, which is the reason the internal
/// decode stays private. A block's check type is written on its enclosing stream
/// and not on the block, so a public API taking the two separately would invite
/// a caller to pair a block with the wrong stream's check — a *silent* wrong
/// verification, which is the failure a consumer can least absorb.
///
/// `Copy`, and `Send` because every field is: a task crosses a channel or a
/// thread boundary with nothing carried along, and the compressed bytes travel
/// separately.
///
/// ```no_run
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use xz_seek::{Reader, Window};
///
/// let file = std::fs::File::open("dump.xz")?;
/// let reader = Reader::new(&file)?;
/// let task = reader.block_task(0).expect("the file has a first block");
///
/// // Fetch the block's compressed bytes however you like, and hand them over.
/// let extent = task.compressed_range();
/// let mut bytes = vec![0u8; (extent.end - extent.start) as usize];
/// // ... fill `bytes` with the file's bytes at `extent` ...
/// let window = Window::new(extent.start, reader.index().compressed_file_size, bytes);
///
/// let mut out = vec![0u8; task.uncompressed_len() as usize];
/// task.decode_into(&window, &mut out)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockTask {
    block: BlockEntry,
    check: Check,
    // A whole-block decode never seeks away, so `Full` and `Streaming` differ
    // here only in `Full`'s refusal of an unimplemented check id.
    verify: Verify,
    memlimit: u64,
    backend: Backend,
}

impl BlockTask {
    /// The task a reader hands out for one of its blocks.
    pub(crate) fn new(
        block: BlockEntry,
        check: Check,
        verify: Verify,
        memlimit: u64,
        backend: Backend,
    ) -> BlockTask {
        BlockTask {
            block,
            check,
            verify,
            memlimit,
            backend,
        }
    }

    /// How many uncompressed bytes this block holds — the size the buffer
    /// handed to [`BlockTask::decode_into`] must be at least.
    ///
    /// It comes off the seek table, so sizing a buffer costs no lookup and no
    /// decode.
    pub fn uncompressed_len(&self) -> u64 {
        self.block.uncompressed_size
    }

    /// Where this block's plaintext sits in the uncompressed stream.
    ///
    /// A caller reassembling blocks in order writes them at
    /// `uncompressed_range().start`, and one delivering a caller-named range
    /// clips against it.
    pub fn uncompressed_range(&self) -> Range<u64> {
        self.block.uncompressed_range()
    }

    /// The compressed bytes this block needs, file-absolute.
    ///
    /// **This is the fetch size**, and it is `total_size()` rather than
    /// `unpadded_size`: a block is `header · compressed data · block padding ·
    /// check`, so a window cut to the unpadded size would stop before the
    /// check. A [`Window`](crate::Window) over exactly this range answers every
    /// read the decode makes and refuses everything else, which is what makes a
    /// mis-sized fetch an error rather than a byte served silently from the page
    /// cache.
    pub fn compressed_range(&self) -> Range<u64> {
        let at = self.block.compressed_offset;
        at..at + self.block.total_size()
    }

    /// The integrity check this block's stream declares, already resolved.
    pub fn check(&self) -> Check {
        self.check
    }

    /// This block's whole compressed extent, read out of `source` into `buf`.
    ///
    /// **The one fetch policy the bulk range read has**, called by the pool's
    /// fetch stage and by the one-worker path alike, so that "the same `Window`
    /// the fetcher builds" is one function rather than two that agree today —
    /// `docs/design/decisions.md`, "D37".
    ///
    /// `file_size` is the **source's own** size and not the table's: a window is
    /// told what the file would answer, so a file that ended early short-returns
    /// where the file does and reaches [`Error::Truncated`] with the offset the
    /// seeking path names, instead of arriving as a window refusing a read it
    /// should have answered short.
    ///
    /// `buf` is replaced by a fresh `vec![0u8; len]` whenever its **length** is
    /// under the extent and truncated to the extent when it is over. The test
    /// is length rather than capacity, and the window handed back is truncated
    /// to what arrived, so a caller reusing one buffer across blocks of
    /// differing sizes re-allocates on every block bigger than the one before
    /// it: it allocates once only where the first block it fetches is the
    /// largest — which is what sizing off
    /// [`RangePlan::compressed_window_bytes`](crate::RangePlan::compressed_window_bytes)
    /// does *not* by itself guarantee. A caller handing over an empty `Vec`
    /// gets a fresh allocation per block either way.
    ///
    /// **A short read is not an error here.** The window is truncated to what
    /// arrived and the decode is what names it, which keeps every short-read and
    /// `Truncated` path in the one place that already had them.
    ///
    /// Deficiency register: `deficiency: KD12` — a source that already holds the
    /// compressed bytes (a memory map, a `bytes::Bytes`, a
    /// [`Window`](crate::Window)) is copied anyway, because this builds a window
    /// out of a range it could have borrowed: the decode no longer copies, the
    /// fetch still does. It costs one copy of each block's compressed bytes and
    /// the `workers + 2` windows the plan charges.
    /// **(a) deliberate tradeoff**: closing it takes a second fetch policy, a
    /// `lends()` capability on [`CompressedSource`] answering *resident* rather
    /// than addressable, and a `RangePlan` whose window term is conditional on
    /// the source. Promoted by a caller reading through
    /// [`Reader::read_range`](crate::Reader::read_range) over a source that
    /// lends, for whom those windows are memory that matters.
    pub(crate) fn fetch_window<S: CompressedSource>(
        &self,
        source: &S,
        file_size: u64,
        mut buf: Vec<u8>,
    ) -> Result<crate::Window<Vec<u8>>> {
        let extent = self.compressed_range();
        let Ok(len) = usize::try_from(extent.end - extent.start) else {
            return Err(Error::Io {
                compressed_offset: extent.start,
                source: std::io::Error::other(
                    "xz-seek: this block's compressed extent does not fit in memory",
                ),
            });
        };
        if buf.len() < len {
            // `vec![0u8; n]` reaches `alloc_zeroed`, where growing a `Vec` with
            // `resize` writes the zeros a byte at a time in an unoptimized
            // build — the difference `reader::DISCARD_CHUNK` records.
            buf = vec![0u8; len];
        } else {
            buf.truncate(len);
        }
        let mut got = 0usize;
        while got < len {
            match source.read_at(extent.start + got as u64, &mut buf[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(source) => {
                    return Err(Error::Io {
                        compressed_offset: extent.start + got as u64,
                        source,
                    });
                }
            }
        }
        buf.truncate(got);
        Ok(crate::Window::new(extent.start, file_size, buf))
    }

    /// Decode the whole block into `out`, verify it, and return.
    ///
    /// `source` supplies the block's compressed bytes at the file's own
    /// offsets — a [`Window`](crate::Window) over
    /// [`BlockTask::compressed_range`], or the file itself. The first
    /// `uncompressed_len()` bytes of `out` are filled; anything past them is
    /// untouched, so a pool may hand every worker a slot sized to the largest
    /// block in the file.
    ///
    /// # Errors
    ///
    /// [`Error::BufferTooSmall`] if `out` is shorter than
    /// [`BlockTask::uncompressed_len`], **before anything is read or written**.
    ///
    /// Otherwise the errors a positioned read over this block would raise, with
    /// one difference that is the point of this path: the check is compared
    /// before the call returns, so [`Error::BlockCheckFailed`] arrives here and
    /// never from a later call. On any error the contents of `out` are
    /// unspecified — bytes may have been written into it before the fault was
    /// found.
    pub fn decode_into<S: CompressedSource>(&self, source: &S, out: &mut [u8]) -> Result<()> {
        self.decode_into_until(source, out, None).map(|_| ())
    }

    /// [`BlockTask::decode_into`], abandonable between decoder calls.
    ///
    /// `stop` is polled once per call into the block decoder rather than once
    /// per block, which is what bounds a worker pool's shutdown by a decoder
    /// call instead of by a whole 128 MiB block. `Ok(false)` means the decode
    /// was abandoned and `out` holds nothing worth reading; `Ok(true)` is
    /// [`BlockTask::decode_into`]'s success, check compared and all.
    ///
    /// It is `pub(crate)` because an abandoned decode has no result, and a
    /// public API that can return "no bytes, no error" is the shape
    /// [`BlockTask::decode_into`] exists not to be. A caller who wants to stop
    /// early stops between blocks, which is theirs to do.
    pub(crate) fn decode_into_until<S: CompressedSource>(
        &self,
        source: &S,
        out: &mut [u8],
        stop: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<bool> {
        let required = self.block.uncompressed_size;
        if (out.len() as u64) < required {
            return Err(Error::BufferTooSmall {
                required,
                given: out.len() as u64,
            });
        }
        let mut decode = BlockDecode::start(
            source,
            &self.block,
            self.check,
            self.verify,
            self.memlimit,
            self.backend,
        )?;
        // A block declaring no plaintext still has a payload to terminate and a
        // check to compare, and `BlockDecode::read` needs somewhere to put
        // bytes it will not produce. Nothing `xz` writes reaches this — a
        // block's uncompressed size is at least one — so the scratch byte is
        // what keeps a table that says otherwise from being a panic.
        let mut scratch = [0u8; 1];
        let mut written = 0u64;
        loop {
            if stop.is_some_and(|s| s.load(std::sync::atomic::Ordering::Relaxed)) {
                return Ok(false);
            }
            let room: &mut [u8] = if written < required {
                &mut out[written as usize..required as usize]
            } else {
                &mut scratch
            };
            let n = decode.read(source, room)?;
            if n == 0 {
                break;
            }
            written += n as u64;
        }
        debug_assert!(decode.is_finished(), "a block that stopped without ending");
        debug_assert_eq!(written, required, "the block delivered the wrong length");
        Ok(true)
    }

    /// Begin a decode of this block that the caller drives and holds.
    ///
    /// [`BlockTask::decode_into`] asks for a buffer the whole block fits in;
    /// this is the same decode taken in pieces, for a caller whose buffer budget
    /// is smaller than one block. It reads the block's header, checks the memory
    /// limit and builds the decoder, and produces no plaintext yet.
    ///
    /// `source` supplies the block's compressed bytes at the file's own offsets,
    /// and **a [`Window`](crate::Window) over
    /// [`BlockTask::compressed_range`] answers every read this handle will ever
    /// make** — the header's included, which is what makes a block cost one
    /// fetch rather than two over a transport where a round trip is the price.
    /// The source is passed per call rather than held, so the handle carries no
    /// borrow and no lifetime; what the calls owe each other is *the same file's
    /// bytes*, not the same object.
    ///
    /// # Errors
    ///
    /// The errors [`BlockTask::decode_into`] raises before it produces bytes:
    /// a header that does not parse, a dictionary over the limit, a chain no
    /// backend will assemble, or under [`Verify::Full`](crate::Verify) a stream
    /// whose check id nothing implements.
    ///
    /// ```no_run
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use xz_seek::{Reader, Window};
    ///
    /// let file = std::fs::File::open("dump.xz")?;
    /// let reader = Reader::new(&file)?;
    /// let task = reader.block_task(0).expect("the file has a first block");
    ///
    /// // One fetch of the block's compressed bytes, however narrow the buffer
    /// // the plaintext comes back through.
    /// let extent = task.compressed_range();
    /// let mut bytes = vec![0u8; (extent.end - extent.start) as usize];
    /// // ... fill `bytes` with the file's bytes at `extent` ...
    /// let window = Window::new(extent.start, reader.index().compressed_file_size, bytes);
    ///
    /// let mut handle = task.begin(&window)?;
    /// let mut buf = [0u8; 4096];
    /// let mut scratch = [0u8; 4096];
    /// handle.advance_to(&window, task.uncompressed_range().start + 1_000, &mut scratch)?;
    /// let n = handle.read(&window, &mut buf)?;
    /// // The check is compared here, over a block the caller never held whole.
    /// handle.complete(&window)?;
    /// # let _ = n;
    /// # Ok(())
    /// # }
    /// ```
    pub fn begin<S: CompressedSource>(&self, source: &S) -> Result<BlockRead> {
        let decode = BlockDecode::start(
            source,
            &self.block,
            self.check,
            self.verify,
            self.memlimit,
            self.backend,
        )?;
        Ok(BlockRead {
            task: *self,
            decode,
        })
    }
}

/// The drain buffer [`BlockRead::complete`] throws the block's remainder into.
///
/// On the stack, so completing a block allocates nothing behind a caller whose
/// budget this path exists to respect, and large enough that draining a 24 MiB
/// block is thousands of decoder calls rather than millions.
const DRAIN_CHUNK: usize = 4096;

/// One block, decoded in pieces by a caller who holds the handle.
///
/// Begun from [`BlockTask::begin`]. It goes **forward only**, through
/// [`BlockRead::read`] for bytes and [`BlockRead::advance_to`] for the ones the
/// caller does not want, and [`BlockRead::complete`] finishes the block and
/// compares its check.
///
/// **It holds no borrow and has no lifetime**, and is `Send`: the consuming
/// shape is a handle begun outside a blocking pool's closure and moved into it.
/// The source is handed to each call instead of being held, exactly as
/// [`BlockTask::decode_into`] takes one, so the caller may serve one call out of
/// a [`Window`](crate::Window) and the next out of the file. It is not `Sync`;
/// anything shared between threads goes behind the caller's own lock. A backend
/// whose decoder is not `Send` cannot serve this path, which `src/backend.rs`
/// asserts of every compiled backend rather than leaving to be discovered.
///
/// # What it holds
///
/// The decoder, its dictionary, and — over a source that does not lend — one
/// compressed input chunk: [`Reader::decode_footprint`](crate::Reader::decode_footprint)
/// is that charge, and [`Reader::decoder_bytes`](crate::Reader::decoder_bytes)
/// is it over a window that lends. **Not the block's plaintext**, which is the
/// point: the bytes go into the caller's buffer and nowhere else.
///
/// # Verification is the caller's to complete
///
/// A block's check covers the whole block, so it can only be compared once the
/// whole block has been decoded. [`BlockRead::complete`] is that call, and it
/// consumes the handle so that skipping it reads as a value dropped rather than
/// as a method not called. **Dropping the handle part-way through compares
/// nothing** — the same sentence [`Verify`](crate::Verify) already carries for
/// [`Reader::read_at`](crate::Reader::read_at); what moves is who holds the
/// lever. `docs/design/decisions.md`, "D67".
///
/// # There is no restart, and no continue-or-restart arithmetic
///
/// A handle only goes forward inside its own block. A caller that needs an
/// offset behind the handle's position begins a second handle from the same
/// [`BlockTask`], which is [`Copy`]: whether that is cheaper than what it has is
/// a judgement the caller's own scheduler can make and this crate cannot see.
/// Asking a handle to go *backward* is a panic rather than an error variant — it
/// names no offset in a file, and [`BlockRead::position`] answers for free
/// before the call. Asking it to go past its own block is
/// [`Error::OffsetPastBlockEnd`], the target being an offset in the file that a
/// persisted table can produce.
pub struct BlockRead {
    task: BlockTask,
    decode: BlockDecode,
}

impl BlockRead {
    /// The task this handle was begun from.
    ///
    /// A [`BlockTask`] is [`Copy`], so this is the block's ranges, its length
    /// and its resolved check without the caller keeping a second copy beside
    /// the handle.
    pub fn task(&self) -> BlockTask {
        self.task
    }

    /// The uncompressed offset of the next byte this handle will produce,
    /// file-absolute.
    ///
    /// It starts at [`BlockTask::uncompressed_range`]'s start and ends there
    /// plus [`BlockTask::uncompressed_len`]. It is this decode's own count of
    /// what it has produced rather than a claim read from the table, which is
    /// what makes a *backward* [`BlockRead::advance_to`] a caller contradicting
    /// itself rather than a runtime fact (`docs/design/decisions.md`, "D68").
    pub fn position(&self) -> u64 {
        self.decode.position()
    }

    /// Whether the block has been decoded to its end and its check compared.
    ///
    /// A [`BlockRead::read`] that returns zero is what gets it here; after that
    /// [`BlockRead::complete`] has nothing left to do and succeeds.
    pub fn is_finished(&self) -> bool {
        self.decode.is_finished()
    }

    /// Produce up to `out.len()` more of this block's plaintext.
    ///
    /// **Zero means the block is exhausted**, and by then its check has been
    /// compared — the same rule the seeking path follows, and the reason a
    /// short return here is not fill-or-EOF: this hands over what one decoder
    /// call produced, and a caller wanting its buffer filled loops.
    ///
    /// # Errors
    ///
    /// The errors a positioned read over this block would raise. The check is
    /// compared on the call that produces the block's **last byte**, so
    /// [`Error::BlockCheckFailed`] arrives from that call — or from
    /// [`BlockRead::complete`] for a block the caller did not read to the end.
    ///
    /// An empty `out` is [`Error::BufferTooSmall`] asking for one byte, because
    /// a zero-length read cannot be told from the end of the block.
    pub fn read<S: CompressedSource>(&mut self, source: &S, out: &mut [u8]) -> Result<usize> {
        if out.is_empty() {
            return Err(Error::BufferTooSmall {
                required: 1,
                given: 0,
            });
        }
        self.decode.read(source, out)
    }

    /// Decode forward to `offset` and throw the bytes away.
    ///
    /// `offset` is a **file-absolute uncompressed offset**, like every other
    /// coordinate in this crate, and `scratch` is where the skipped bytes land —
    /// the caller's buffer, because an allocation made here is one the caller's
    /// own budget cannot see. It is what a positioned read does internally, and
    /// the reason a caller's read loop can land anywhere inside a block rather
    /// than only at its start.
    ///
    /// Not `seek_to`: this crate disclaims [`std::io::Seek`] in its first
    /// paragraph, and the name says the true thing — it only goes forward.
    ///
    /// # Panics
    ///
    /// If `offset` is behind [`BlockRead::position`]: the caller contradicting
    /// itself one line after being told for free where the handle is, a fault
    /// with no offset in a *file* to name, which is what the taxonomy's variants
    /// are for (`docs/design/decisions.md`, "D67"). A caller that wants a byte
    /// behind the position begins a second handle from the same [`BlockTask`].
    ///
    /// # Errors
    ///
    /// [`Error::OffsetPastBlockEnd`] if `offset` is past the end of the block:
    /// that target is a file-absolute uncompressed offset and can descend from a
    /// table the caller persisted, so it is data and earns a refusal rather than
    /// a panic (`docs/design/decisions.md`, "D68"). It is raised before anything
    /// is decoded, so the handle is left where it was and may be used on.
    ///
    /// Otherwise the errors [`BlockRead::read`] raises, since that is what
    /// skipping is; the check is compared where a skip reaches the block's last
    /// byte. An empty `scratch` with anything to skip is
    /// [`Error::BufferTooSmall`], asking for one byte; `offset` equal to the
    /// position is `Ok(())` and looks at `scratch` not at all.
    pub fn advance_to<S: CompressedSource>(
        &mut self,
        source: &S,
        offset: u64,
        scratch: &mut [u8],
    ) -> Result<()> {
        let here = self.decode.position();
        let block = self.task.uncompressed_range();
        assert!(
            offset >= here,
            "xz-seek: advance_to({offset}) with the handle at {here} in a block \
             holding {}..{}: a handle only goes forward",
            block.start,
            block.end
        );
        if offset > block.end {
            return Err(Error::OffsetPastBlockEnd {
                uncompressed_offset: offset,
                block,
            });
        }
        let mut left = offset - here;
        if left == 0 {
            return Ok(());
        }
        if scratch.is_empty() {
            return Err(Error::BufferTooSmall {
                required: 1,
                given: 0,
            });
        }
        while left > 0 {
            let want = (left as usize).min(scratch.len());
            let got = self.decode.read(source, &mut scratch[..want])?;
            // A block that ends before the index said it would is the decode's
            // own verdict, raised above rather than discovered here.
            if got == 0 {
                break;
            }
            left -= got as u64;
        }
        Ok(())
    }

    /// Decode the rest of the block, compare its check, and spend the handle.
    ///
    /// This is the call that closes the seeking path's caveat — *"the bytes you
    /// have may fail their check two calls from now"* — for a caller whose
    /// budget could not hold the block whole: the bytes it already took are
    /// verified by the time this returns. What it costs is a full decode of
    /// whatever is left of the block, which is CPU and not another fetch.
    ///
    /// A handle already at the block's end has nothing to do, and this succeeds
    /// without reading anything. Under [`Verify::Off`](crate::Verify), or on a
    /// `--check=none` stream, there is no check to compare and the drain only
    /// reconciles the block against the index — so a caller who wants neither
    /// drops the handle instead.
    ///
    /// # Errors
    ///
    /// [`Error::BlockCheckFailed`] where the block's own check does not match
    /// what it decoded to, plus anything [`BlockRead::read`] raises over the
    /// remainder.
    pub fn complete<S: CompressedSource>(mut self, source: &S) -> Result<()> {
        let mut scratch = [0u8; DRAIN_CHUNK];
        while self.decode.read(source, &mut scratch)? != 0 {}
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::COMPILED;
    use crate::decode::DEFAULT_MEMLIMIT;
    use crate::table::SeekTable;

    /// A task carries itself to a worker thread, and does so owning nothing.
    ///
    /// `Send` and `'static` are what a `std::thread` pool needs of the work it
    /// hands out, and both are inferred from the fields — so nothing in the
    /// type's own text would notice a field arriving that broke either. The
    /// assertion is what notices.
    #[test]
    fn a_task_is_send_and_owns_nothing_borrowed() {
        fn assert_send_static<T: Send + 'static>() {}
        fn assert_copy<T: Copy>() {}
        assert_send_static::<BlockTask>();
        assert_copy::<BlockTask>();
    }

    /// The handle crosses a thread boundary, and an `await`, as a value.
    ///
    /// It is the same assertion the walk machine carries and for the same
    /// reason: both properties are inferred from the fields, so nothing in the
    /// type's own text would notice a field arriving that broke one. `Sync` is
    /// not asserted and is not offered — anything shared goes behind the
    /// caller's lock.
    #[test]
    fn a_handle_is_send_and_owns_nothing_borrowed() {
        fn assert_send_static<T: Send + 'static>() {}
        assert_send_static::<BlockRead>();
    }

    /// One fixture's bytes and a task over its first block, under a named
    /// backend. The caller begins the handle.
    fn handle_on(name: &str, backend: Backend) -> (Vec<u8>, BlockTask) {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let bytes = std::fs::read(dir.join(name)).expect("the fixture reads");
        let table = SeekTable::from_source(&bytes.as_slice()).expect("the fixture walks");
        let task = BlockTask::new(
            table.blocks[0],
            table.streams[0].check,
            Verify::Full,
            DEFAULT_MEMLIMIT,
            backend,
        );
        (bytes, task)
    }

    /// A buffer with no room in it is refused rather than read as the block's
    /// end, on both calls that take one.
    ///
    /// Zero is how [`BlockRead::read`] says *exhausted*, so a zero-length
    /// buffer has no honest answer; `advance_to` is the same call underneath and
    /// says the same thing — except where there is nothing to skip, which is a
    /// success that never looks at the buffer.
    #[test]
    fn an_empty_buffer_is_refused_and_a_zero_length_skip_is_not() {
        for &backend in COMPILED {
            let (bytes, task) = handle_on("one-block.xz", backend);
            let mut handle = task.begin(&bytes.as_slice()).expect("the block starts");
            assert_eq!(handle.position(), task.uncompressed_range().start);
            assert!(matches!(
                handle.read(&bytes.as_slice(), &mut []),
                Err(Error::BufferTooSmall {
                    required: 1,
                    given: 0
                })
            ));
            // Nowhere to go is not a skip, so the empty buffer is never asked
            // for.
            let here = handle.position();
            handle
                .advance_to(&bytes.as_slice(), here, &mut [])
                .expect("advancing nowhere needs no buffer");
            assert!(matches!(
                handle.advance_to(&bytes.as_slice(), here + 1, &mut []),
                Err(Error::BufferTooSmall {
                    required: 1,
                    given: 0
                })
            ));
        }
    }

    /// A handle asked to go backward panics rather than growing the taxonomy a
    /// variant that names no offset in a file.
    #[test]
    #[should_panic(expected = "only goes forward")]
    fn a_handle_asked_to_go_backward_panics() {
        let (bytes, task) = handle_on("many-blocks.xz", COMPILED[0]);
        let mut handle = task.begin(&bytes.as_slice()).expect("the block starts");
        let mut scratch = [0u8; 64];
        handle
            .advance_to(
                &bytes.as_slice(),
                task.uncompressed_range().start + 100,
                &mut scratch,
            )
            .expect("forward inside the block");
        let _ = handle.advance_to(
            &bytes.as_slice(),
            task.uncompressed_range().start + 99,
            &mut scratch,
        );
    }

    /// And one asked for an offset past its own block is refused rather than
    /// panicking: the target is an offset in the file, which a persisted table
    /// can produce, so it is data and earns a variant
    /// (`docs/design/decisions.md`, "D68").
    ///
    /// The refusal decodes nothing, so the handle survives it — which is what
    /// makes it a runtime fact a correct caller can absorb rather than a
    /// contradiction. The block's own end is *not* past it: advancing there is
    /// the ordinary way to reach the end.
    #[test]
    fn a_handle_asked_past_its_own_block_is_refused_and_survives() {
        for &backend in COMPILED {
            let (bytes, task) = handle_on("many-blocks.xz", backend);
            let mut handle = task.begin(&bytes.as_slice()).expect("the block starts");
            let mut scratch = [0u8; 64];
            let block = task.uncompressed_range();

            let e = handle
                .advance_to(&bytes.as_slice(), block.end + 1, &mut scratch)
                .expect_err("past the block's end is refused");
            assert!(
                matches!(
                    &e,
                    Error::OffsetPastBlockEnd {
                        uncompressed_offset,
                        block: named,
                    } if *uncompressed_offset == block.end + 1 && *named == block
                ),
                "{backend}: {e}"
            );
            // Neither accessor answers: the block it names is not damage.
            assert_eq!(e.compressed_offset(), None);
            assert_eq!(e.uncompressed_range(), None);

            // Nothing was decoded, and the handle still works.
            assert_eq!(handle.position(), block.start);
            handle
                .advance_to(&bytes.as_slice(), block.end, &mut scratch)
                .expect("the block's own end is inside it");
            assert_eq!(handle.position(), block.end);
        }
    }

    /// The fetch extent is the block's padded total, and the ranges agree with
    /// the table they came from.
    ///
    /// `total_size()` against `unpadded_size` is the one arithmetic mistake a
    /// caller cannot see: a window three bytes short cuts off the check, and the
    /// blocks where the two differ are the ones with block padding — 78 of the
    /// corpus's 91 real headers.
    #[test]
    fn the_ranges_are_the_table_s_own_and_the_fetch_extent_is_padded() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let path = dir.join("many-blocks.xz");
        let source = std::fs::File::open(&path).expect("the fixture opens");
        let table = SeekTable::from_source(&source).expect("the fixture walks");

        let mut padded = 0usize;
        for (i, block) in table.blocks.iter().enumerate() {
            let task = BlockTask::new(
                *block,
                Check::Crc64,
                Verify::Full,
                DEFAULT_MEMLIMIT,
                COMPILED[0],
            );
            assert_eq!(
                task.uncompressed_len(),
                block.uncompressed_size,
                "block {i}"
            );
            assert_eq!(task.uncompressed_range(), block.uncompressed_range());
            assert_eq!(
                task.compressed_range(),
                block.compressed_offset..block.compressed_offset + block.total_size()
            );
            assert_eq!(task.check(), Check::Crc64);
            if block.total_size() != block.unpadded_size {
                padded += 1;
            }
        }
        assert!(padded > 0, "no block in this fixture carries block padding");
    }

    /// A source that fails every read and knows only how large the file is.
    ///
    /// Handing it to `decode_into` is what holds the length check to running
    /// *before* the source is touched: a check that migrated below the header
    /// read would come back as this error rather than as `BufferTooSmall`. It is
    /// `tests/pieces.rs`'s `Empty` and `tests/taxonomy.rs`'s `Failing` at the
    /// one entry point neither of them drives.
    struct Empty(u64);

    impl CompressedSource for Empty {
        fn read_at(&self, offset: u64, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other(format!(
                "decode_into read the source at {offset} before measuring the buffer"
            )))
        }

        fn size(&self) -> std::io::Result<u64> {
            Ok(self.0)
        }
    }

    /// A buffer one byte short is refused, and nothing is written into it.
    ///
    /// The refusal comes before the header is read, which is what makes "never a
    /// partial fill" cheap as well as true: a caller who got the size wrong pays
    /// no source read for it. That is also the rule `src/error.rs` states for
    /// admitting a variant to the half of the taxonomy carrying no offset, so
    /// the source here answers nothing at all — the property is asserted and not
    /// merely claimed, as `BackendUnavailable`'s half already was.
    #[test]
    fn a_short_buffer_is_refused_before_a_byte_is_read() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let path = dir.join("one-block.xz");
        let bytes = std::fs::read(&path).expect("the fixture reads");
        let table = SeekTable::from_source(&bytes.as_slice()).expect("the fixture walks");
        let block = table.blocks[0];
        let task = BlockTask::new(
            block,
            Check::Crc64,
            Verify::Full,
            DEFAULT_MEMLIMIT,
            COMPILED[0],
        );

        // Nothing in the file is readable, so the only thing that can be wrong
        // is the buffer — and only if it is measured first.
        let source = Empty(bytes.len() as u64);

        let short = (block.uncompressed_size - 1) as usize;
        let mut out = vec![0xAAu8; short];
        match task.decode_into(&source, &mut out) {
            Err(Error::BufferTooSmall { required, given }) => {
                assert_eq!(required, block.uncompressed_size);
                assert_eq!(given, short as u64);
            }
            other => panic!("a short buffer must be refused, got {other:?}"),
        }
        assert!(
            out.iter().all(|b| *b == 0xAA),
            "the refusal wrote into the caller's buffer"
        );

        // An empty buffer is the same refusal, not a panic and not a success.
        assert!(matches!(
            task.decode_into(&source, &mut []),
            Err(Error::BufferTooSmall { given: 0, .. })
        ));
    }
}
