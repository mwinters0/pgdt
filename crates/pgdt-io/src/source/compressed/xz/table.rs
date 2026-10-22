//! What the compression index names of `.xz`: its seek table as this crate
//! hands it out, and the magic recognition compares — a leaf, so the index can
//! name the layout without reaching the codec that builds it.

use std::ops::Range;

/// The six bytes every `.xz` stream opens with
/// (`docs/design/decisions.md`, "D14").
pub(crate) const XZ_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];

/// Where an `.xz` file's streams and blocks sit, as a walk of its stream
/// footers found them: [`CompressionIndex::Xz`]'s layout.
///
/// **Opaque**: `xz_seek`, whose table this wraps, is a vendored, unpublished
/// crate (`docs/design/decisions.md`, "D14"), so its type is not part of
/// this crate's API. What an embedder reads is the file's shape, through the
/// methods below; what it persists is the value whole, under the `serde`
/// feature, which serializes exactly as `xz_seek`'s own table does.
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
