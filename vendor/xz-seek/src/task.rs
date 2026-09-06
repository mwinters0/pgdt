//! One block, decoded whole into a buffer the caller owns.
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
//! slot. See `docs/design/architecture.md`, "The public pieces: one block into
//! the caller's buffer".
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
