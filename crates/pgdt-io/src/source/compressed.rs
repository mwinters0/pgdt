//! The compressed sources, written once over a codec: the codec trait each
//! container format implements, the positioned read a unit decode pulls its
//! compressed bytes through, the live piecewise decode, the piece of a read a
//! unit answers and the short read — above the two arms, the local file
//! pulled from and the transport fetched through, the block cache and budget
//! they decode into, the compression index a cache persists, and each
//! codec's adapter, holding only what touches its crate
//! (`docs/design/decisions.md`, "D15").

use std::ops::Range;

use pgdt_core::{Error, Result};

pub(crate) mod blocks;
mod codec;
mod deflate;
mod fetched;
mod file;
mod index;
mod xz;

pub use self::deflate::{GzipIndex, WindowProvider};
pub use self::fetched::FetchedCompressedSource;
pub use self::file::CompressedFileSource;
pub use self::index::CompressionIndex;
pub(crate) use self::index::{Format, LONGEST_MAGIC};
pub use self::xz::SeekTable;

pub(crate) use self::codec::{Codec, Positioned, UnitRead, short_read};
pub(crate) use self::deflate::unindexed;
#[cfg(any(feature = "http", test))]
pub(crate) use self::xz::walk_seek_table;

/// The live piecewise decode: one [`UnitRead`] and the unit it sits in, kept
/// between reads.
///
/// **It is the state machine both piecewise arms run**, handed the
/// [`Positioned`] a call reads through — the file [`CompressedFileSource`]
/// pulls from, or a `HeldBlock`'s fetched window. What the two arms do not
/// share is the loop around it, their scheduling differing: the local one
/// decodes a whole read inside one `spawn_blocking`, the fetched one returns
/// to the runtime between units to fetch the next window
/// (`docs/design/decisions.md`, "D15").
///
/// **A kept handle continues a forward read** rather than restarting the
/// covering unit, which is what keeps a forward scan of a single-unit file
/// linear in the file rather than quadratic in it.
///
/// **Leaving a unit completes it**, which is where its check is compared, and
/// the unit a run's last read stopped inside is left as that run's reading
/// ends ([`ByteRangeSource::complete_reads`](crate::ByteRangeSource::complete_reads)).
/// A handle is dropped uncompleted only where its read failed.
struct LiveBlock {
    /// Which unit the handle is decoding.
    index: usize,
    /// The handle. Forward-only, so it serves a read at or after
    /// `read.position()` and nothing behind it.
    read: Box<dyn UnitRead>,
}

impl LiveBlock {
    /// Begin unit `index` out of `source`.
    fn begin(codec: &dyn Codec, index: usize, source: &dyn Positioned) -> Result<Self> {
        Ok(Self { index, read: codec.begin(index, source)? })
    }

    /// Whether this handle can serve `from` out of unit `index`.
    ///
    /// **Continuing is never dearer than restarting**, so there is no
    /// arithmetic to do: restarting skips `from − unit.start` and continuing
    /// skips `from − position`, and a live handle's position is never before
    /// its unit's start.
    fn seats(&self, index: usize, from: u64) -> bool {
        self.index == index && !self.read.is_finished() && self.read.position() <= from
    }

    /// Skip forward to `from` and fill `out`, answering how many bytes the
    /// unit had for it — the loop that turns a handle handing over one
    /// decoder call's output, and zero at the unit's end, into this crate's
    /// fill contract.
    fn fill(&mut self, source: &dyn Positioned, from: u64, out: &mut [u8]) -> Result<usize> {
        self.read.advance_to(source, from)?;
        let mut filled = 0usize;
        while filled < out.len() {
            let got = self.read.read(source, &mut out[filled..])?;
            if got == 0 {
                break;
            }
            filled += got;
        }
        Ok(filled)
    }

    /// Leave the unit, comparing its check.
    fn leave(self, source: &dyn Positioned) -> Result<()> {
        self.read.complete(source)
    }

    /// Fill `out` from unit `index` at `from` through the slot's handle: the
    /// live one where it can reach that byte, a fresh one otherwise, the unit
    /// it leaves completed first.
    ///
    /// This is the local arm's whole loop body, which decodes inside the one
    /// `spawn_blocking` its read already sits in; the fetched arm drives the
    /// same calls around an `await` instead
    /// ([`FetchedCompressedSource::read_in_pieces`]).
    fn serve(
        live: &mut Option<LiveBlock>,
        codec: &dyn Codec,
        index: usize,
        source: &dyn Positioned,
        from: u64,
        out: &mut [u8],
    ) -> Result<usize> {
        if !live.as_ref().is_some_and(|seated| seated.seats(index, from)) {
            if let Some(leaving) = live.take() {
                leaving.leave(source)?;
            }
            *live = Some(LiveBlock::begin(codec, index, source)?);
        }
        live.as_mut().expect("just seated").fill(source, from, out)
    }
}

/// Which part of a read `unit` answers: where in the caller's buffer it lands,
/// the first plaintext offset wanted from it, and how many bytes — or `None`
/// where the two ranges do not actually overlap.
///
/// One function because all four read arms ask it of the same two ranges, and
/// four copies of the arithmetic can disagree about a boundary.
fn piece_of(unit: Range<u64>, offset: u64, end: u64) -> Option<(usize, u64, usize)> {
    let from = unit.start.max(offset);
    let to = unit.end.min(end);
    if to <= from {
        return None;
    }
    Some((usize::try_from(from - offset).ok()?, from, usize::try_from(to - from).ok()?))
}

/// The codec an index describes, over a compressed file of `stored_size`
/// bytes, checked against that length and its own consistency without reading
/// a byte; an index that does not describe the file is refused
/// ([`index_refused`]).
fn codec_over(index: CompressionIndex, stored_size: u64) -> Result<std::sync::Arc<dyn Codec>> {
    match index {
        CompressionIndex::Xz(table) => {
            Ok(std::sync::Arc::new(xz::XzCodec::over(table, stored_size)?))
        }
        CompressionIndex::Gzip(index) => {
            Ok(std::sync::Arc::new(deflate::DeflateCodec::over(index, stored_size)?))
        }
    }
}

/// Whether `error` is a codec refusing an index handed in as not describing
/// the file — what [`open`](crate::open) turns into
/// [`Recognized::Mismatch`](crate::Recognized::Mismatch) rather than a rebuild.
pub(crate) fn index_refused(error: &Error) -> bool {
    xz::refuses_table(error) || deflate::refuses_index(error)
}
