//! What a compressed file's codec answers the two arms: the `Codec` trait over
//! its units, the forward-only `UnitRead` handle, the `Positioned` read a
//! decode pulls its compressed bytes through, and the short read every arm and
//! codec reports a read past the plaintext as.

use std::ops::Range;

use pgdt_core::{Error, Result};

use super::CompressionIndex;

/// What a unit decode reads its compressed bytes through: a file pulled from,
/// or a window of bytes already fetched, in the file's own coordinates.
///
/// **Fill-or-end**, as every codec's own source contract is: a
/// [`Positioned::read_at`] shorter than its buffer means the file ended there
/// and nothing else, and [`Positioned::slice_at`] lends a range only where it
/// holds every byte of it, declining being always correct. A codec's adapter
/// bridges this to its crate's own source trait, so the arms name no codec's
/// crate.
pub(crate) trait Positioned: Send + Sync {
    /// Fill `buf` with the bytes at `offset`, answering how many were written.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize>;
    /// The whole compressed file's length.
    fn size(&self) -> std::io::Result<u64>;
    /// The `len` bytes at `offset`, borrowed, where every one is held.
    fn slice_at(&self, _offset: u64, _len: usize) -> Option<&[u8]> {
        None
    }
}

/// Positioned reads on a file, looping past a short `pread` that is not the
/// file's end — what makes the fill-or-end contract true of a file. It holds
/// nothing, so it lends nothing.
#[cfg(unix)]
impl Positioned for std::fs::File {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::os::unix::fs::FileExt;
        let mut done = 0;
        while done < buf.len() {
            match FileExt::read_at(self, &mut buf[done..], offset + done as u64) {
                Ok(0) => break,
                Ok(n) => done += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(done)
    }

    fn size(&self) -> std::io::Result<u64> {
        Ok(self.metadata()?.len())
    }
}

/// One compressed file's units, as both arms read them: an `.xz` file's
/// blocks or the spans between a gzip file's checkpoints, numbered in file
/// order.
///
/// **What the arms need and nothing else** — each unit's plaintext and
/// compressed ranges, the largest unit, what a decoder holds, a unit decoded
/// whole out of a positioned source, and a forward-only handle over one — so
/// the local arm and the fetched arm are each written once, and a codec is an
/// implementation of this rather than a third and fourth source
/// (`docs/design/decisions.md`, "D15").
///
/// **Object-safe, held as `Arc<dyn Codec>`**: the codec is chosen at run time,
/// by the file's first bytes or the index a cache hands back
/// (`docs/design/decisions.md`, "D15").
pub(crate) trait Codec: Send + Sync {
    /// The format's name, as a message and the codec error say it.
    fn name(&self) -> &'static str;
    /// What a cache persists to open this file again without rebuilding it
    /// (`docs/design/decisions.md`, "D18").
    fn index(&self) -> CompressionIndex;
    /// The file's whole plaintext length.
    fn plaintext_size(&self) -> u64;
    /// The compressed file's length, as the index describes it.
    fn compressed_size(&self) -> u64;
    /// How many units the file holds.
    fn unit_count(&self) -> usize;
    /// The units whose plaintext overlaps `range`, as an index range.
    fn units_in(&self, range: Range<u64>) -> Range<usize>;
    /// Unit `unit`'s plaintext range, `None` past the last.
    fn plaintext(&self, unit: usize) -> Option<Range<u64>>;
    /// Unit `unit`'s whole compressed extent — what a fetch must hold for a
    /// decode of it to reach its check — `None` past the last.
    fn compressed(&self, unit: usize) -> Option<Range<u64>>;
    /// The largest unit's plaintext length: what a reader must hold to decode
    /// any unit whole, zero for a file with none.
    fn largest_unit(&self) -> u64;
    /// The largest unit's compressed extent: the widest window a fetch of one
    /// makes.
    fn widest_compressed(&self) -> u64;
    /// What one decode retains besides its output over a source that lends
    /// nothing, so that the decoder builds its own input buffer.
    fn pulling_footprint(&self) -> u64;
    /// What one decode retains besides its output over a source that lends,
    /// so that no input buffer is built.
    fn lending_footprint(&self) -> u64;
    /// Decode unit `unit` whole into `out`, exactly its plaintext long,
    /// comparing its check before returning.
    fn decode(&self, unit: usize, source: &dyn Positioned, out: &mut [u8]) -> Result<()>;
    /// A forward-only handle at the start of unit `unit`.
    fn begin(&self, unit: usize, source: &dyn Positioned) -> Result<Box<dyn UnitRead>>;
}

/// A forward-only decode of one unit, handed the source on every call rather
/// than holding it, which is what lets the fetched arm keep its window beside
/// the handle and the local arm its file in the source.
pub(crate) trait UnitRead: Send {
    /// The plaintext offset the next byte read comes from.
    fn position(&self) -> u64;
    /// Whether the unit has been read to its end.
    fn is_finished(&self) -> bool;
    /// Decode forward to plaintext offset `to`, discarding what lies before it.
    fn advance_to(&mut self, source: &dyn Positioned, to: u64) -> Result<()>;
    /// What one decoder call produced into `out`, zero at the unit's end.
    fn read(&mut self, source: &dyn Positioned, out: &mut [u8]) -> Result<usize>;
    /// Leave the unit, comparing its check — decoding its remainder where it
    /// carries one.
    fn complete(self: Box<Self>, source: &dyn Positioned) -> Result<()>;
}

/// The error a read past the end of the plaintext is — the same
/// `UnexpectedEof` a short `read_exact_at` raises on the plain source, so
/// every source reports a caller/source disagreement identically.
pub(crate) fn short_read() -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "compressed stream ended before the requested range",
    ))
}
