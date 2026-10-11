//! What both `.xz` sources share — the live piecewise decode, the piece of a
//! read a block answers, the short read and the seek-table walk — above the
//! two sources and the block cache they decode into, which advises how a
//! range over either may be cut.

use std::ops::Range;

use pgdt_core::{Error, Result};

use crate::source::ByteRangeSource;

pub(crate) mod blocks;
mod fetched;
mod file;
#[cfg(test)]
mod tests;

pub use self::fetched::FetchedXzSource;
pub use self::file::XzSource;

/// The live piecewise decode: one `xz_seek::BlockRead` and the block it sits
/// in, kept between reads.
///
/// **It is the state machine both piecewise arms run**, generic over the
/// `xz_seek::CompressedSource` a decode call is handed — a `std::fs::File`
/// [`XzSource`] pulls from, or a `HeldBlock`'s fetched window. What the two
/// arms do not share is the loop around it, their scheduling differing: the
/// local one decodes a whole read inside one `spawn_blocking`, the fetched one
/// returns to the runtime between blocks to fetch the next window
/// (`docs/design/decisions.md`, "D15").
///
/// **It is `xz_seek::Reader`'s own live decode with the handle held here.** That
/// reader continued a forward read out of the decode it already had and
/// restarted the covering block otherwise, which is what keeps a forward scan
/// of a single-block file linear in the file rather than quadratic in it; the
/// handle is what carries that property across the mechanism swap
/// (`docs/design/decisions.md`, "D15").
///
/// **Leaving a block completes it**, which is where its check is compared —
/// `xz_seek::Verify::Full`'s seek-away escape, now a call we make — and the
/// block a run's last read stopped inside is left as that run's reading ends
/// ([`ByteRangeSource::complete_reads`]). Dropping a handle part-way compares
/// nothing, so a handle is dropped uncompleted only where its read failed or
/// its stream carries no check ([`LiveBlock::leave`]).
struct LiveBlock {
    /// Which block the handle is decoding.
    index: usize,
    /// The handle. Forward-only, so it serves a read at or after
    /// `read.position()` and nothing behind it.
    read: xz_seek::BlockRead,
}

impl LiveBlock {
    /// Begin block `index` out of `source`.
    fn begin<S: xz_seek::CompressedSource>(
        index: usize,
        task: &xz_seek::BlockTask,
        source: &S,
    ) -> Result<Self> {
        Ok(Self { index, read: task.begin(source).map_err(Error::xz)? })
    }

    /// Whether this handle can serve `from` out of block `index`.
    ///
    /// **Continuing is never dearer than restarting**, so there is no
    /// arithmetic to do: restarting skips `from − block.start` and continuing
    /// skips `from − position`, and a live handle's position is never before
    /// its block's start. That is `xz_seek::Reader`'s own route choice with the
    /// comparison resolved.
    fn seats(&self, index: usize, from: u64) -> bool {
        self.index == index && !self.read.is_finished() && self.read.position() <= from
    }

    /// Skip forward to `from` and fill `out`, answering how many bytes the
    /// block had for it.
    ///
    /// `xz_seek::BlockRead::read` hands over what one decoder call produced
    /// rather than filling, and answers zero at the block's end, so the loop is
    /// what turns it into this crate's fill contract.
    fn fill<S: xz_seek::CompressedSource>(
        &mut self,
        source: &S,
        from: u64,
        out: &mut [u8],
    ) -> Result<usize> {
        let mut skip = [0u8; PIECEWISE_SKIP_CHUNK];
        self.read.advance_to(source, from, &mut skip).map_err(Error::xz)?;
        let mut filled = 0usize;
        while filled < out.len() {
            let got = self.read.read(source, &mut out[filled..]).map_err(Error::xz)?;
            if got == 0 {
                break;
            }
            filled += got;
        }
        Ok(filled)
    }

    /// Leave the block, comparing its check.
    ///
    /// A `--check=none` stream has nothing to compare, so draining it would
    /// decode a block's remainder to verify nothing — which is the one case
    /// `Verify::Full`'s escape also skips.
    fn leave<S: xz_seek::CompressedSource>(self, source: &S) -> Result<()> {
        if self.read.task().check() == xz_seek::Check::None {
            drop(self.read);
            return Ok(());
        }
        self.read.complete(source).map_err(Error::xz)
    }

    /// Fill `out` from block `index` at `from` through the slot's handle:
    /// the live one where it can reach that byte, a fresh one otherwise, the
    /// block it leaves completed first.
    ///
    /// This is the local arm's whole loop body, which decodes inside the one
    /// `spawn_blocking` its read already sits in; the fetched arm drives the
    /// same four calls around an `await` instead
    /// ([`FetchedXzSource::read_in_pieces`](crate::FetchedXzSource::read_in_pieces)).
    fn serve<S: xz_seek::CompressedSource>(
        live: &mut Option<LiveBlock>,
        index: usize,
        task: &xz_seek::BlockTask,
        source: &S,
        from: u64,
        out: &mut [u8],
    ) -> Result<usize> {
        if !live.as_ref().is_some_and(|seated| seated.seats(index, from)) {
            if let Some(leaving) = live.take() {
                leaving.leave(source)?;
            }
            *live = Some(LiveBlock::begin(index, task, source)?);
        }
        live.as_mut().expect("just seated").fill(source, from, out)
    }
}

/// The error a read past the end of the uncompressed stream is — the same
/// `UnexpectedEof` a short `read_exact_at` raises on the plain source, so
/// both sources report a caller/source disagreement identically.
fn xz_short_read() -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "xz stream ended before the requested range",
    ))
}

/// How many uncompressed bytes a piecewise read throws away per decoder call
/// while skipping to its first wanted byte.
///
/// On the stack, so a read that holds no whole block allocates nothing beyond
/// the buffer it is filling — which is the budget that arm exists to respect —
/// and large enough that skipping a 24 MiB block is thousands of decoder calls
/// rather than millions. It is `xz_seek`'s own drain chunk, which sizes the
/// same skip on the other side of `complete`.
const PIECEWISE_SKIP_CHUNK: usize = 4096;

/// Which part of a read `block` answers: where in the caller's buffer it lands,
/// the first uncompressed offset wanted from it, and how many bytes — or `None`
/// where the two ranges do not actually overlap.
///
/// One function because all four of the `.xz` read arms ask it of the same two
/// ranges, and four copies of the arithmetic can disagree about a boundary.
fn piece_of(block: Range<u64>, offset: u64, end: u64) -> Option<(usize, u64, usize)> {
    let from = block.start.max(offset);
    let to = block.end.min(end);
    if to <= from {
        return None;
    }
    Some((usize::try_from(from - offset).ok()?, from, usize::try_from(to - from).ok()?))
}

/// Build an `.xz` file's seek table by driving `xz_seek`'s footer walk over an
/// **asynchronous** source, and say how many fetches it took.
///
/// `xz_seek::Walk` is the footer parse with the reads taken out: it hands out
/// a range and takes the bytes back, holding no borrow and no lifetime, so the
/// fetch is ours and may `await`. That is the whole reason a dump reached over
/// the network can be `.xz` at all —
/// `xz_seek::Reader` pulls through a *synchronous* positional trait, and
/// nothing can `await` inside it
/// (`docs/design/decisions.md`, "D14").
///
/// `leading` is what the caller already holds of the file's first bytes — an
/// [`Origin`](crate::Origin) probe's, normally. The walk's first request is the six magic
/// bytes at offset zero, which is exactly what that probe fetched, so a caller
/// passing them spends no round trip on them. Passing an empty slice is
/// correct and costs one.
///
/// **The count it returns is the file's, not this driver's.** The walk asks for
/// a footer, an index, a header and at least one padding probe per stream, in a
/// strictly backward chain where each request's position comes out of the bytes
/// of the one before it — so neither coalescing nor concurrency buys anything
/// on its own (`docs/design/decisions.md`, "D18").
///
/// Deficiency register: `deficiency: KD36` — this driver fetches exactly what
/// it is asked for, so a cold walk over a dump compressed as many small streams
/// is at least four round trips a stream before a row is read. The remedy is a **straddling
/// window**: one read per stream positioned backward from the pending request
/// and sized to cover what the walk asks for next, which answers three of every
/// stream's four requests because a stream's header is adjacent to its
/// predecessor's padding, footer and index. **One fetch a stream is then a
/// floor, not a cost still looking for a remedy** — the distance to the next
/// boundary is knowable only from the index just read, and this file family has
/// no stream stride to predict it from. What would beat the floor is not a
/// better driver but a forward table build, which stops paying separately for
/// the walk at all. **(c) unowned**; promoted by a phase that tunes the network,
/// which is where the fetch policy this source defers belongs, and which is also
/// what would take the figure.
pub async fn walk_seek_table(
    source: &dyn ByteRangeSource,
    stored_size: u64,
    leading: &[u8],
) -> Result<(xz_seek::SeekTable, usize)> {
    let (mut walk, mut request) = xz_seek::Walk::begin(stored_size);
    let mut fetches = 0usize;
    loop {
        let asked = usize::try_from(request.len()).map_err(|_| xz_short_read())?;
        let step = if request.start() == 0 && asked <= leading.len() {
            walk.supply(request, &leading[..asked]).map_err(Error::xz)?
        } else {
            fetches += 1;
            let bytes = source.read_range(request.start(), asked).await?;
            walk.supply(request, &bytes).map_err(Error::xz)?
        };
        match step {
            xz_seek::Step::Need(next) => request = next,
            xz_seek::Step::Done(table) => return Ok((table, fetches)),
        }
    }
}
