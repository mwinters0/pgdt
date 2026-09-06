//! The positioned read: an uncompressed byte offset in, the bytes there out.
//!
//! [`Reader`] holds the seek table, the configuration, and at most one live
//! [`BlockDecode`]. Everything it does is decided by arithmetic over the table
//! before a byte moves.
//!
//! # Continuing is chosen by arithmetic, not by a threshold
//!
//! For a read at `target` the reader knows two exact costs, both in
//! uncompressed bytes it would have to decode:
//!
//! * **continue** — `target - position`, available only when the live decode is
//!   in the block covering `target` and has not passed it;
//! * **restart** — `target - block_start`, always available.
//!
//! It takes the smaller. There is no heuristic and no tuning knob, and the
//! behaviour the roadmap describes falls out of the comparison rather than
//! being written down as three cases: forward within a block always continues,
//! because a live position is never below its own block's start; a forward jump
//! into a later block restarts at that block rather than decoding the blocks
//! between, because a position in an earlier block is below the target block's
//! start; and a backward move within a block restarts, because continuing
//! cannot go backward at all.
//!
//! # Leaving a block, and what verification costs
//!
//! [`Verify::Full`] is the default and it completes a partly-decoded block
//! before the decoder leaves it, so that a random-access caller who never reads
//! a block to its end still gets the check it asked for. That is the roadmap's
//! first escape, and it is on by default rather than being one of the opt-in
//! modes: without it "verified by default" would be true of the code and false
//! of the run.
//!
//! It costs what the roadmap says — reading 1 MiB from block A and then 1 MiB
//! from block B decodes all of A plus half of B, against half and half — and
//! [`Verify::Streaming`] is its off switch. A `--check=none` stream has nothing
//! to complete, so the escape does not run there at all.
//!
//! **A check failure surfaces from whichever call completes the block**, which
//! is a later call than the ones that returned most of the bytes it covers.
//! That is the roadmap's stated contract, and it is what `xz -dc` itself does.
//!
//! A block that has given up its last byte is *completed there*, not when
//! something later seeks away from it — see [`BlockDecode::read`]. The escape is
//! only for a block the reader leaves part-way through, which is why a
//! sequential walk needs no escape and still verifies every block including the
//! file's last, where nothing ever seeks away.

use crate::backend::{self, Backend};
use crate::decode::{BlockDecode, DEFAULT_MEMLIMIT};
use crate::error::{Error, Result};
use crate::source::CompressedSource;
use crate::table::{Check, SeekTable};

/// The discard buffer, in bytes.
///
/// Not configurable, and allocated only when something is actually skipped, so
/// a purely sequential caller never pays for it. Together with the compressed
/// input chunk it is what keeps the reader's steady-state overhead a constant
/// rather than a function of block size.
///
/// **It is allocated as `vec![0u8; DISCARD_CHUNK]`, never grown into with
/// `resize`, and the difference is not stylistic.** `vec![0u8; n]` reaches
/// `alloc_zeroed` and gets zero pages from the allocator; `Vec::resize` on an
/// empty vector writes the zeros itself, which an unoptimized build does one
/// byte at a time — 1.1 ms against 2 µs here. A caller that builds a reader per
/// positioned read pays that once per read, which is a shape the exhaustive
/// sweep has half a million of.
const DISCARD_CHUNK: usize = 256 << 10;

/// How much verification a reader does.
///
/// [`Verify::Full`] is the default. The two weaker states exist so that the
/// default can stay strict instead of being softened: `Streaming` is the
/// seek-away escape's off switch, and `Off` is R6's "defeatable by an explicit
/// call".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Verify {
    /// Checks are computed, and a partly-decoded block is decoded to its end
    /// before the reader leaves it, so that every block the reader touched has
    /// been verified.
    ///
    /// A stream declaring a check id nothing implements is refused here, with
    /// [`Error::UnsupportedCheck`], before any of its blocks is decoded.
    #[default]
    Full,
    /// Checks are computed and compared for a block the reader decodes to its
    /// end, and dropped for one it abandons part-way.
    ///
    /// A stream declaring an unimplemented check id decodes unverified, which
    /// is what `xz` does.
    Streaming,
    /// No checks are computed.
    Off,
}

/// How to open a [`Reader`].
///
/// Three knobs and two constructors would be six combinations; the builder
/// collapses them to one path, and [`Reader::new`] is the zero-configuration
/// shortcut through it.
///
/// **[`Builder::new`] is the only configured entry point, and there is no
/// `Reader::builder()`.** A `Builder` is not generic and does not decide what
/// source it will open — [`Builder::open`] does, from its argument — so nothing
/// in a `Reader::builder()` sitting in `impl<S: CompressedSource> Reader<S>`
/// constrains `S`. It is `E0283` bare, and the turbofish rustc suggests to fix
/// that is *discarded*: `Reader::<File>::builder().open(&bytes[..])` compiles
/// and yields a `Reader<&[u8]>`. A method whose one required argument means
/// nothing is worse than a missing one.
///
/// Making `Builder` generic (`Builder<S>` over a `PhantomData`) would let
/// `Reader::builder()` infer `S` through the chain, and is rejected for what it
/// costs: a `Builder` is source-agnostic and [`Copy`], so one configured value
/// opens a `File` and then a `&[u8]` today, and a `Builder<File>` could not.
#[derive(Debug, Clone, Copy)]
pub struct Builder {
    memlimit: u64,
    verify: Verify,
    backend: Backend,
}

impl Default for Builder {
    fn default() -> Builder {
        Builder {
            memlimit: DEFAULT_MEMLIMIT,
            verify: Verify::Full,
            backend: backend::DEFAULT,
        }
    }
}

impl Builder {
    /// A builder carrying the defaults: a 64 MiB dictionary limit,
    /// [`Verify::Full`], and [`Backend::Liblzma`] wherever it is compiled.
    pub fn new() -> Builder {
        Builder::default()
    }

    /// The largest LZMA2 dictionary a block may declare, in **bytes of
    /// dictionary**.
    ///
    /// That is the honest unit: the dictionary is the only part of the
    /// decoder's footprint the file dictates, and the rest is this crate's own
    /// and fixed. The limit is compared against the block header's own property
    /// byte before any decoder object is constructed, so a hostile file never
    /// gets an allocation attempted on its behalf.
    ///
    /// The default is 64 MiB, which is `-9`'s dictionary — so the default
    /// admits every preset the `xz` CLI produces, and a refusal means hostile
    /// input rather than a real file.
    pub fn memlimit(mut self, dictionary_bytes: u64) -> Builder {
        self.memlimit = dictionary_bytes;
        self
    }

    /// How much verification the reader does. See [`Verify`].
    pub fn verify(mut self, verify: Verify) -> Builder {
        self.verify = verify;
        self
    }

    /// Which decoder decodes a block's payload. See [`Backend`].
    ///
    /// The default is [`Backend::Liblzma`] where that feature is compiled and
    /// [`Backend::Xz4rust`] otherwise, so the default build behaves exactly as
    /// it did before the second backend existed and the unsafe-free build needs
    /// no call here at all.
    ///
    /// Naming one this build does not carry is **not** rejected here: it is
    /// [`Error::BackendUnavailable`] from [`Builder::open`] or
    /// [`Builder::open_with_table`], which is where a `Result` already is. A
    /// caller that wants to choose without an error round trip asks
    /// [`Backend::is_available`].
    pub fn backend(mut self, backend: Backend) -> Builder {
        self.backend = backend;
        self
    }

    /// The backend, refused here if this build does not carry it.
    ///
    /// Both constructors call this **first**, before the walk or the table's
    /// validation, so a caller who named the wrong backend is not charged the
    /// two to three minutes of footer seeks the koji file's walk costs before
    /// being told.
    fn compiled_backend(self) -> Result<Backend> {
        if self.backend.is_available() {
            Ok(self.backend)
        } else {
            Err(Error::BackendUnavailable {
                backend: self.backend,
            })
        }
    }

    /// Walk `source`'s stream footers and open a reader over it.
    ///
    /// The walk is one read per stream plus one for the file's tail, and it
    /// decodes nothing.
    ///
    /// # Errors
    ///
    /// [`Error::BackendUnavailable`] if [`Builder::backend`] named a backend
    /// this build does not carry — checked **before** the walk begins, so a
    /// caller who named the wrong one does not pay for it.
    pub fn open<S: CompressedSource>(self, source: S) -> Result<Reader<S>> {
        let backend = self.compiled_backend()?;
        let table = SeekTable::from_source(&source)?;
        Ok(self.with(source, table, backend))
    }

    /// Open a reader over `source` from a [`SeekTable`] the caller already
    /// holds, **without walking**.
    ///
    /// This is the other half of [`Reader::index`]: a caller persists the table
    /// in its own envelope and hands it back here, and the two to three minutes of
    /// footer seeks the walk costs on the 40 GB koji file is paid once in that
    /// file's life rather than once per process.
    ///
    /// # What it checks, and what it does not
    ///
    /// The table is validated for internal consistency, and cross-checked
    /// against the source in **one call** —
    /// [`SeekTable::compressed_file_size`] against
    /// [`CompressedSource::size`]. It does not re-walk, because re-walking is
    /// the cost the table exists to avoid, and it therefore reads **no bytes at
    /// all**: a table that describes a source correctly in size and shape is
    /// taken at its word until a block is decoded, at which point the block
    /// header's own CRC32 is the evidence that `compressed_offset` really
    /// points at a block header.
    ///
    /// So a source that is not an `.xz` file cannot raise
    /// [`Error::NotXz`](crate::Error::NotXz) here — nothing reads its magic.
    /// The failure a caller actually has, a stale or corrupted persisted table,
    /// is [`Error::InvalidTable`], which is deliberately not
    /// [`Error::IndexInconsistent`](crate::Error::IndexInconsistent): a
    /// downstream told its *file* is inconsistent re-fetches the file, where
    /// what it has is its own bad cache.
    ///
    /// [`Error::BackendUnavailable`] comes first of all, before the table is
    /// looked at — see [`Builder::open`].
    pub fn open_with_table<S: CompressedSource>(
        self,
        source: S,
        table: SeekTable,
    ) -> Result<Reader<S>> {
        let backend = self.compiled_backend()?;
        let size = source.size().map_err(|e| Error::io(0, e))?;
        table.validate(size)?;
        Ok(self.with(source, table, backend))
    }

    /// The reader both constructors build once the table is in hand.
    fn with<S: CompressedSource>(self, source: S, table: SeekTable, backend: Backend) -> Reader<S> {
        Reader {
            source,
            table,
            memlimit: self.memlimit,
            verify: self.verify,
            backend,
            live: None,
            discard: Vec::new(),
        }
    }
}

/// How the reader gets to an offset, and how many uncompressed bytes it has to
/// decode and throw away on the way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// Keep the live decode and skip forward from where it is.
    Continue(u64),
    /// Start the covering block again and skip forward from its first byte.
    Restart(u64),
}

/// The cheaper of the two routes to `target`, by exact byte count.
///
/// `live_position` is the live decode's next byte, and `None` where there is no
/// live decode that could reach `target` at all — a decode in another block, or
/// one already finished. Both costs are known before either route begins, which
/// is what makes this arithmetic rather than a threshold.
fn route(block_start: u64, live_position: Option<u64>, target: u64) -> Route {
    let restart = target - block_start;
    match live_position {
        // Continuing cannot go backward, so a position past the target is not a
        // route at any price.
        Some(p) if p <= target && target - p <= restart => Route::Continue(target - p),
        _ => Route::Restart(restart),
    }
}

/// The live decode, and which block it is in.
struct Live {
    block: usize,
    check: Check,
    decode: BlockDecode,
}

/// A positioned read over an `.xz` file.
///
/// **There is no [`std::io::Seek`] implementation**, deliberately — see
/// [`Reader::read_at`], which is the whole interface.
///
/// `&mut self`, because the reader owns a live decoder and a caller's forward
/// walk must not re-decode the covering block on every call. The mutability is
/// visible rather than hidden behind a lock; a caller wanting several
/// independent positions builds several readers over one source, which is what
/// [`CompressedSource`]'s `&self` is for.
///
/// [`Reader::new`] opens with the defaults; [`Builder`] is where the knobs are.
pub struct Reader<S: CompressedSource> {
    source: S,
    table: SeekTable,
    memlimit: u64,
    verify: Verify,
    backend: Backend,
    live: Option<Live>,
    discard: Vec<u8>,
}

impl<S: CompressedSource> Reader<S> {
    /// Open a reader over `source` with the defaults, walking its footers.
    ///
    /// To set the memory limit or the verification state, start from
    /// [`Builder::new`] instead — there is no `Reader::builder()`, and
    /// [`Builder`]'s own docs say why.
    pub fn new(source: S) -> Result<Reader<S>> {
        Builder::new().open(source)
    }

    /// The seek table this reader is using.
    ///
    /// The table is a plain value with public documented fields, so this is
    /// also the extraction half of persisting it: serialize what comes back —
    /// the `serde` feature derives `Serialize` and `Deserialize` on every type
    /// in it — and hand it to [`Builder::open_with_table`] next time.
    ///
    /// The shape and cost queries live on [`SeekTable`] and the reader does not
    /// forward them: `reader.index().uncompressed_size()` is one keystroke more
    /// than a forwarding method and one fewer thing to keep in step. That is
    /// also why [`SeekTable::seek_cost`] is unambiguous — the table has no live
    /// position to confuse a cold-start cost with.
    pub fn index(&self) -> &SeekTable {
        &self.table
    }

    /// Read the uncompressed bytes at `offset` into `buf`.
    ///
    /// **Fills `buf` completely.** A return shorter than `buf.len()` means the
    /// uncompressed stream ended, and means nothing else — never a partial
    /// decode, a block boundary, or a short read from the source. An offset at
    /// or past the end of the file returns zero rather than an error.
    ///
    /// This is stronger than [`std::os::unix::fs::FileExt::read_at`], and
    /// deliberately: without it every caller asking for a range would write the
    /// same retry loop, and the ones that forgot would be silently wrong.
    ///
    /// # Errors
    ///
    /// A block's integrity check is compared when its last byte is decoded, so
    /// under [`Verify::Full`] and [`Verify::Streaming`]
    /// [`Error::BlockCheckFailed`] can arrive from a *later* call than the one
    /// that returned the bytes it covers. Bytes this call already wrote into
    /// `buf` are not reported when it returns an error.
    pub fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let mut done = 0usize;
        while done < buf.len() {
            let target = offset + done as u64;
            // Past the last byte of the file: fill-or-EOF's short return.
            let Some(block) = self.table.block_containing(target) else {
                break;
            };
            let n = match self.read_from(block, target, &mut buf[done..]) {
                Ok(n) => n,
                Err(e) => {
                    // Whatever the live decode was in the middle of, it is no
                    // longer a state the next call can reason about.
                    self.live = None;
                    return Err(e);
                }
            };
            // The block covers `target`, so it has a byte there to give.
            debug_assert!(n > 0, "a block covering an offset produced nothing");
            if n == 0 {
                break;
            }
            done += n;
        }
        Ok(done)
    }

    /// Position the decoder at `target` and take what the covering block gives.
    fn read_from(&mut self, block: usize, target: u64, out: &mut [u8]) -> Result<usize> {
        self.position_at(block, target)?;
        let live = self.live.as_mut().expect("positioned leaves a live decode");
        live.decode.read(&self.source, out)
    }

    /// Put the live decode where `target`'s bytes come next, by whichever of
    /// the two routes costs fewer decoded bytes.
    fn position_at(&mut self, block: usize, target: u64) -> Result<()> {
        // A live decode is a candidate only where it could reach the target at
        // all: in the target's own block, and not already past its end.
        let live_position = match &self.live {
            Some(l) if l.block == block && !l.decode.is_finished() => Some(l.decode.position()),
            _ => None,
        };
        let start = self.table.blocks[block].uncompressed_offset;
        let skip = match route(start, live_position, target) {
            Route::Continue(skip) => skip,
            Route::Restart(skip) => {
                self.start_block(block)?;
                skip
            }
        };
        self.discard_forward(skip)
    }

    /// Leave whatever block the decoder is in and start `block`.
    fn start_block(&mut self, block: usize) -> Result<()> {
        self.leave_live_block()?;
        let entry = self.table.blocks[block];
        let check = self.check_of(block)?;
        // Checked as compiled at the constructor, so the seam is never reached
        // with a backend this build does not carry.
        let decode = BlockDecode::start(
            &self.source,
            &entry,
            check,
            self.verify,
            self.memlimit,
            self.backend,
        )?;
        self.live = Some(Live {
            block,
            check,
            decode,
        });
        Ok(())
    }

    /// The seek-away escape: complete a partly-decoded block's check before
    /// abandoning it.
    ///
    /// Runs only under [`Verify::Full`], and not at all on a `--check=none`
    /// stream, where there is nothing to complete and the escape would decode
    /// the rest of a block to verify nothing.
    fn leave_live_block(&mut self) -> Result<()> {
        let Some(mut live) = self.live.take() else {
            return Ok(());
        };
        if self.verify != Verify::Full || live.decode.is_finished() || live.check == Check::None {
            return Ok(());
        }
        if self.discard.is_empty() {
            self.discard = vec![0u8; DISCARD_CHUNK];
        }
        while live.decode.read(&self.source, &mut self.discard)? != 0 {}
        Ok(())
    }

    /// Decode `count` bytes out of the live block and throw them away.
    fn discard_forward(&mut self, count: u64) -> Result<()> {
        if count == 0 {
            return Ok(());
        }
        if self.discard.is_empty() {
            self.discard = vec![0u8; DISCARD_CHUNK];
        }
        let live = self.live.as_mut().expect("something to discard through");
        let mut left = count;
        while left > 0 {
            let want = (left as usize).min(self.discard.len());
            // The block covers the target, so it has these bytes; a zero here
            // would mean it ended early, which the decode raises for itself.
            let got = live.decode.read(&self.source, &mut self.discard[..want])?;
            if got == 0 {
                break;
            }
            left -= got as u64;
        }
        Ok(())
    }

    /// The check declared by the stream holding block `block`.
    ///
    /// A block's check type is written in its stream's flags and nowhere else,
    /// and streams are ordered by compressed offset like the blocks inside
    /// them, so this is one binary search. It reads only fields that mean
    /// something for every stream — `first_block` does not, for a stream with no
    /// blocks.
    fn check_of(&self, block: usize) -> Result<Check> {
        let at = self.table.blocks[block].compressed_offset;
        let i = self
            .table
            .streams
            .partition_point(|s| s.compressed_offset <= at)
            .checked_sub(1);
        match i {
            Some(i) => Ok(self.table.streams[i].check),
            // A walked table cannot put a block outside every stream.
            None => Err(Error::IndexInconsistent {
                compressed_offset: at,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three behaviours the roadmap names, read off the arithmetic rather
    /// than written down as cases.
    ///
    /// The reader driving this over real bytes is `tests/reader.rs`; here the
    /// rule itself is pinned, because it is what a second backend must not
    /// quietly change.
    #[test]
    fn the_route_is_whichever_costs_fewer_decoded_bytes() {
        // A block covering 1000..2000, with a live decode at 1100.
        let start = 1_000;

        // Forward within the block always continues: a live position is never
        // below its own block's start.
        assert_eq!(route(start, Some(1_100), 1_100), Route::Continue(0));
        assert_eq!(route(start, Some(1_100), 1_101), Route::Continue(1));
        assert_eq!(route(start, Some(1_100), 1_999), Route::Continue(899));

        // Backward within the block restarts, whatever the distance.
        assert_eq!(route(start, Some(1_100), 1_099), Route::Restart(99));
        assert_eq!(route(start, Some(1_100), 1_000), Route::Restart(0));

        // No live decode in this block — a forward jump into a later block, or
        // a first read — restarts at the block, never decoding what is between.
        assert_eq!(route(start, None, 1_500), Route::Restart(500));
        assert_eq!(route(start, None, 1_000), Route::Restart(0));
    }
}
