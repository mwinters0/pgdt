//! An `.xz` file's seek table as this crate hands it out: what a source
//! built over the file answers ([`ByteRangeSource::seek_table`]), what a
//! caller hands back to skip the walk ([`KnownCompression::Xz`]), and what a
//! cache persists between the two (`docs/design/decisions.md`, "D18").
//!
//! [`ByteRangeSource::seek_table`]: crate::ByteRangeSource::seek_table
//! [`KnownCompression::Xz`]: crate::KnownCompression::Xz

use std::ops::Range;

/// Where an `.xz` file's streams and blocks sit, as a walk of its stream
/// footers found them.
///
/// **Opaque**: `xz_seek`, whose table this wraps, is a vendored, unpublished
/// crate (`docs/design/decisions.md`, "D14"), so its type is not part of
/// this crate's API. What an embedder reads is the file's shape, through the
/// methods below; what it persists is the value whole, under the `serde`
/// feature, which serializes exactly as `xz_seek`'s own table does, so a
/// cache written before this type existed reads back unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(transparent))]
pub struct SeekTable(pub(crate) xz_seek::SeekTable);

impl SeekTable {
    /// Whether seeking buys anything: more than one block. A one-block file
    /// is read correctly, necessarily by decoding from its start
    /// (`docs/design/decisions.md`, "D19").
    pub fn is_seekable(&self) -> bool {
        self.0.is_seekable()
    }

    /// How many streams the file holds.
    pub fn stream_count(&self) -> usize {
        self.0.stream_count()
    }

    /// How many blocks the file holds, across every stream.
    pub fn block_count(&self) -> usize {
        self.0.block_count()
    }

    /// The largest block's plaintext length, which is what a reader must hold
    /// to decode any block whole; zero for a file with no blocks.
    pub fn max_block_uncompressed(&self) -> u64 {
        self.0.max_block_uncompressed()
    }

    /// The file's whole plaintext length.
    pub fn uncompressed_size(&self) -> u64 {
        self.0.uncompressed_size()
    }

    /// The compressed file's length, stream padding included: the length a
    /// source's stored size must match for this table to describe it.
    pub fn compressed_size(&self) -> u64 {
        self.0.compressed_file_size
    }

    /// Each block's plaintext range, in file order: adjacent, and together
    /// the whole of [`SeekTable::uncompressed_size`].
    pub fn block_ranges(&self) -> impl ExactSizeIterator<Item = Range<u64>> + '_ {
        self.0.blocks.iter().map(xz_seek::BlockEntry::uncompressed_range)
    }
}
