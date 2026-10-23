//! The gzip adapter: the codec over `gz_seek`'s checkpoints, each unit decoded
//! from its checkpoint with the window its provider reads, the bridge from a
//! [`Positioned`] read to the bytes a `gz_seek` decode is fed, and the
//! plaintext spacing this crate hands `gz_seek` — with its submodules, the
//! only modules of the compressed layer naming `gz_seek`.
//!
//! **The codec is deflate; gzip is the framing** (`docs/design/roadmap-P15-gzip.md`,
//! "Settled in the grilling"), so the module is named for the codec and its
//! name for a message is the container's.

use std::num::NonZeroU64;
use std::ops::Range;

use pgdt_core::{Error, Result};

use super::{Codec, CompressionIndex, Positioned, UnitRead, short_read};

mod checkpoints;
mod first_pass;
#[cfg(any(test, feature = "test-support"))]
pub(crate) mod held;

pub(crate) use self::checkpoints::GZIP_MAGIC;
pub use self::checkpoints::{GzipIndex, WindowProvider};
pub(crate) use self::first_pass::{Feed, FirstPass};

/// The plaintext spacing a forward pass places a checkpoint at the first
/// deflate block boundary at or after each multiple of, and BGZF members are
/// coalesced into units of at least: **the block `xz -T0` writes at its
/// default preset**, so a gzip unit is an ordinary `.xz` block's size for the
/// block cache, the partition grain and the resident bounds read off `.xz`
/// (`docs/design/decisions.md`, "D113").
///
/// A shipped constant, never derived from the file: a first pass chooses it
/// before it knows how long the file is.
pub(super) const CHECKPOINT_SPACING: NonZeroU64 = NonZeroU64::new(24 << 20).unwrap();

/// The container's name, which the codec error and a message carry.
const GZIP: &str = "gzip";

/// How many compressed bytes a decode over a source that lends nothing pulls
/// at a time, capped by the widest unit: `xz_seek`'s own input chunk, so a
/// gzip reader is charged the input term an `.xz` one is, under the resident
/// bounds read off `.xz` (`docs/design/decisions.md`, "D113").
const INPUT_CHUNK: u64 = 1 << 20;

/// How many plaintext bytes a piecewise read throws away per decoder call
/// while skipping to its first wanted byte, or draining a unit to its check:
/// on the stack, as the `.xz` adapter's is.
const SKIP_CHUNK: usize = 4096;

/// A `gz_seek` failure as this crate's codec error, naming its container.
fn gzip_error(cause: gz_seek::Error) -> Error {
    Error::codec(GZIP, cause)
}

/// Whether `error` is `gz_seek`, or this adapter on its behalf, refusing an
/// index handed in as not describing the file ([`super::index_refused`]).
pub(super) fn refuses_index(error: &Error) -> bool {
    matches!(error.codec_cause(), Some(gz_seek::Error::InvalidIndex { .. }))
}

/// A gzip file's units as the compressed arms read them, each the span from
/// one checkpoint to the next.
///
/// **The index and its window provider, and nothing else**: a [`gz_seek::UnitTask`]
/// is `Copy` and holds no index, so naming a unit takes no lock, and a unit's
/// window is read as its decode begins and dropped once the inflater holds it.
pub(super) struct DeflateCodec {
    layout: GzipIndex,
    compressed_size: u64,
    largest_unit: u64,
    widest_compressed: u64,
}

impl DeflateCodec {
    /// The codec over a complete index, checked against the compressed file's
    /// own length, **reading no bytes**: an index ending elsewhere than the
    /// file is `gz_seek::Error::InvalidIndex` ([`refuses_index`]).
    ///
    /// An index that stops short of the file's end is refused as a caller's
    /// error rather than as another file's: only a first pass continues one,
    /// which [`open`](crate::open) runs over it. So is one whose units begin
    /// inside a member with no provider to read their windows through.
    pub(super) fn over(layout: GzipIndex, stored_size: u64) -> Result<Self> {
        let Some(compressed_size) = layout.compressed_size() else {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "a gzip index that stops short of the file's end is continued only by a first \
                 pass, which `pgdt_io::open` runs over it",
            )));
        };
        if compressed_size != stored_size {
            return Err(gzip_error(gz_seek::Error::InvalidIndex {
                reason: "the index ends elsewhere than the file does",
            }));
        }
        let units = layout.unit_count();
        if layout.windows.is_none() && (0..units).any(|unit| layout.window_len(unit) != Some(0)) {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "a gzip index whose units begin inside a member was handed in with no window \
                 provider to read their windows through",
            )));
        }
        let largest_unit = layout.largest_unit();
        let widest_compressed = (0..units)
            .map(|unit| {
                let range = layout.task(unit).compressed_range();
                range.end - range.start
            })
            .max()
            .unwrap_or(0);
        Ok(Self { layout, compressed_size, largest_unit, widest_compressed })
    }

    /// Unit `unit`'s task, or the short read past the last.
    fn task(&self, unit: usize) -> Result<gz_seek::UnitTask> {
        if unit >= self.layout.unit_count() {
            return Err(short_read());
        }
        Ok(self.layout.task(unit))
    }

    /// A forward decode of unit `unit`, begun with its window, which is read
    /// here and dropped once the inflater holds it.
    fn read(&self, unit: usize) -> Result<DeflateRead> {
        let task = self.task(unit)?;
        let mut window = vec![0u8; task.window_len()];
        if !window.is_empty() {
            let provider = self.layout.windows.as_ref().ok_or_else(|| {
                Error::Io(std::io::Error::other("a gzip unit's window has no provider"))
            })?;
            provider.read_window(unit, &mut window)?;
        }
        let read = task.begin(&window).map_err(gzip_error)?;
        let compressed = task.compressed_range();
        Ok(DeflateRead { read, compressed, input: Vec::new(), input_at: 0, input_len: 0 })
    }

    /// The input term a decode over a source that lends nothing holds.
    fn input_chunk(&self) -> u64 {
        INPUT_CHUNK.min(self.widest_compressed)
    }
}

impl Codec for DeflateCodec {
    fn name(&self) -> &'static str {
        GZIP
    }

    fn index(&self) -> CompressionIndex {
        CompressionIndex::Gzip(self.layout.clone())
    }

    fn plaintext_size(&self) -> u64 {
        self.layout.uncompressed_size()
    }

    fn compressed_size(&self) -> u64 {
        self.compressed_size
    }

    fn unit_count(&self) -> usize {
        self.layout.unit_count()
    }

    /// The units from the last one beginning at or before `range.start` to
    /// the last one beginning before `range.end` — so an empty unit at a
    /// member boundary is never where a read begins.
    fn units_in(&self, range: Range<u64>) -> Range<usize> {
        let units = self.layout.unit_count();
        if range.is_empty() || range.start >= self.plaintext_size() {
            return units..units;
        }
        let checkpoints = &self.layout.index.checkpoints()[..units];
        let first = checkpoints.partition_point(|c| c.plaintext <= range.start).saturating_sub(1);
        let end = checkpoints.partition_point(|c| c.plaintext < range.end);
        first..end.max(first)
    }

    fn plaintext(&self, unit: usize) -> Option<Range<u64>> {
        self.task(unit).ok().map(|task| task.plaintext_range())
    }

    /// From the byte holding the unit's first bit to the one holding its
    /// last, the trailers of the members it ends included: what a fetch must
    /// hold for its decode to reach every check.
    fn compressed(&self, unit: usize) -> Option<Range<u64>> {
        self.task(unit).ok().map(|task| task.compressed_range())
    }

    fn largest_unit(&self) -> u64 {
        self.largest_unit
    }

    fn widest_compressed(&self) -> u64 {
        self.widest_compressed
    }

    /// The inflater, the window read as a decode begins, and the compressed
    /// input chunk it pulls through over a source that lends nothing.
    fn pulling_footprint(&self) -> u64 {
        self.lending_footprint() + self.input_chunk()
    }

    /// The inflater and the window read as a decode begins: a lending source
    /// is fed from where it holds its bytes, so no input chunk is built.
    fn lending_footprint(&self) -> u64 {
        gz_seek::DECODER_BYTES + gz_seek::WINDOW_SIZE as u64
    }

    fn decode(&self, unit: usize, source: &dyn Positioned, out: &mut [u8]) -> Result<()> {
        let mut read = self.read(unit)?;
        let mut filled = 0usize;
        while filled < out.len() {
            let got = read.produce(source, &mut out[filled..])?;
            if got == 0 {
                return Err(short_read());
            }
            filled += got;
        }
        read.drain(source)
    }

    fn begin(&self, unit: usize, _source: &dyn Positioned) -> Result<Box<dyn UnitRead>> {
        Ok(Box::new(self.read(unit)?))
    }
}

/// One `gz_seek::UnitRead`, fed from a [`Positioned`] read: out of the bytes
/// a fetched window lends, or through a chunk pulled from a file.
///
/// **Fed no further than the unit's own compressed extent**, so the decode's
/// refusal of a unit running on past where its index says it ends is about the
/// file, never about bytes the feed over-supplied.
struct DeflateRead {
    read: gz_seek::UnitRead,
    /// The unit's compressed extent.
    compressed: Range<u64>,
    /// The pulled chunk, built on the first pull and only over a source that
    /// lends nothing.
    input: Vec<u8>,
    /// The compressed offset of `input`'s first byte.
    input_at: u64,
    /// How many of `input`'s bytes the last pull filled.
    input_len: usize,
}

impl DeflateRead {
    /// One decoder call into `out`, fed the unit's bytes from where the decode
    /// has got to, answering how much plaintext it produced — possibly none,
    /// where the call read framing or a block's end.
    fn step(&mut self, source: &dyn Positioned, out: &mut [u8]) -> Result<usize> {
        let at = self.read.compressed_position();
        let want = self.compressed.end.saturating_sub(at);
        let want = usize::try_from(want).map_err(|_| short_read())?;
        let Self { read, compressed, input, input_at, input_len } = self;
        let fed: &[u8] = if want == 0 {
            &[]
        } else if let Some(lent) = source.slice_at(at, want) {
            lent
        } else {
            let held = at >= *input_at && at < *input_at + *input_len as u64;
            if !held {
                if input.is_empty() {
                    let unit = compressed.end - compressed.start;
                    input.resize(usize::try_from(INPUT_CHUNK.min(unit)).unwrap_or(0), 0);
                }
                let n = input.len().min(want);
                *input_len = source.read_at(at, &mut input[..n])?;
                *input_at = at;
            }
            &input[(at - *input_at) as usize..*input_len]
        };
        Ok(read.read(fed, out).map_err(gzip_error)?.produced)
    }

    /// Decoder calls until one produces plaintext or the unit ends: the
    /// [`UnitRead::read`] contract, zero meaning the end.
    fn produce(&mut self, source: &dyn Positioned, out: &mut [u8]) -> Result<usize> {
        while !self.read.is_finished() {
            let produced = self.step(source, out)?;
            if produced > 0 {
                return Ok(produced);
            }
        }
        Ok(0)
    }

    /// Decode the rest of the unit, discarding it, until its checks have been
    /// compared.
    fn drain(&mut self, source: &dyn Positioned) -> Result<()> {
        let mut skip = [0u8; SKIP_CHUNK];
        while !self.read.is_finished() {
            self.step(source, &mut skip)?;
        }
        Ok(())
    }
}

impl UnitRead for DeflateRead {
    fn position(&self) -> u64 {
        self.read.position()
    }

    fn is_finished(&self) -> bool {
        self.read.is_finished()
    }

    fn advance_to(&mut self, source: &dyn Positioned, to: u64) -> Result<()> {
        let mut skip = [0u8; SKIP_CHUNK];
        while self.read.position() < to {
            let n = usize::try_from(to - self.read.position()).unwrap_or(SKIP_CHUNK);
            if self.produce(source, &mut skip[..n.min(SKIP_CHUNK)])? == 0 {
                return Err(short_read());
            }
        }
        Ok(())
    }

    fn read(&mut self, source: &dyn Positioned, out: &mut [u8]) -> Result<usize> {
        self.produce(source, out)
    }

    /// Every gzip unit carries a check — its own CRC-32, or the trailers of
    /// the members it holds whole — so leaving one always decodes its rest.
    fn complete(mut self: Box<Self>, source: &dyn Positioned) -> Result<()> {
        self.drain(source)
    }
}

#[cfg(test)]
mod tests;
