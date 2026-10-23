//! The compression index: what a compressed source answers
//! ([`ByteRangeSource::compression_index`]), what a caller hands back to skip
//! rebuilding it ([`KnownCompression::Compressed`]), and what a cache persists
//! between the two (`docs/design/decisions.md`, "D18") — one vocabulary, a
//! variant per codec layout; and the formats recognition tells apart by their
//! first bytes.
//!
//! [`ByteRangeSource::compression_index`]: crate::ByteRangeSource::compression_index
//! [`KnownCompression::Compressed`]: crate::KnownCompression::Compressed

use super::deflate::{GZIP_MAGIC, GzipIndex};
use super::xz::{SeekTable, XZ_MAGIC};

/// Where a compressed file's units sit, in its codec's own layout.
///
/// **An enum from its first variant**, so a match that assumes one codec fails
/// to compile where another is added rather than going on assuming it. What an
/// embedder reads without matching is the file's shape, through the methods
/// below; what it persists is the value whole, under the `serde` feature,
/// tagged by its variant.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CompressionIndex {
    /// An `.xz` file's streams and blocks, as a walk of its stream footers
    /// found them.
    Xz(SeekTable),
    /// A gzip file's checkpoints, and where each one's window is read from.
    Gzip(GzipIndex),
}

impl CompressionIndex {
    /// The codec's name, as a message says it.
    pub fn codec(&self) -> &'static str {
        self.format().name()
    }

    /// Whether seeking buys anything: more than one unit. A one-unit file is
    /// read correctly, necessarily by decoding from its start
    /// (`docs/design/decisions.md`, "D19").
    pub fn is_seekable(&self) -> bool {
        match self {
            Self::Xz(table) => table.is_seekable(),
            Self::Gzip(index) => index.is_seekable(),
        }
    }

    /// How many units — an `.xz` file's blocks, the spans between a gzip
    /// file's checkpoints — the file holds.
    pub fn unit_count(&self) -> usize {
        match self {
            Self::Xz(table) => table.block_count(),
            Self::Gzip(index) => index.unit_count(),
        }
    }

    /// The largest unit's plaintext length, which is what a reader must hold
    /// to decode any unit whole; zero for a file with none.
    pub fn largest_unit(&self) -> u64 {
        match self {
            Self::Xz(table) => table.max_block_uncompressed(),
            Self::Gzip(index) => index.largest_unit(),
        }
    }

    /// The file's whole plaintext length — as far as the last checkpoint for
    /// a gzip index that stops short of the file's end.
    pub fn uncompressed_size(&self) -> u64 {
        match self {
            Self::Xz(table) => table.uncompressed_size(),
            Self::Gzip(index) => index.uncompressed_size(),
        }
    }

    /// Which format the index is a layout of, for recognition to compare
    /// against the file's own first bytes.
    pub(crate) fn format(&self) -> Format {
        match self {
            Self::Xz(_) => Format::Xz,
            Self::Gzip(_) => Format::Gzip,
        }
    }
}

/// A compressed container this build reads, as its first bytes name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Xz,
    Gzip,
}

impl Format {
    /// The format whose magic `leading` opens with — recognition by
    /// **content**, never by name (`docs/design/decisions.md`, "D14") — or
    /// `None` for a file with no compression layer this build reads.
    pub(crate) fn recognize(leading: &[u8]) -> Option<Self> {
        if leading.starts_with(&XZ_MAGIC) {
            Some(Self::Xz)
        } else if leading.starts_with(&GZIP_MAGIC) {
            Some(Self::Gzip)
        } else {
            None
        }
    }

    /// The format's name, as a message and the codec error say it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Xz => "xz",
            Self::Gzip => "gzip",
        }
    }
}

/// The longest magic [`Format::recognize`] compares, which is how many leading
/// bytes an origin's probe brings back.
pub(crate) const LONGEST_MAGIC: usize =
    if XZ_MAGIC.len() > GZIP_MAGIC.len() { XZ_MAGIC.len() } else { GZIP_MAGIC.len() };
