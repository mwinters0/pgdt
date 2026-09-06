//! The seek table: what an uncompressed offset has to be turned into before a
//! single byte is decoded.
//!
//! The table is a plain value with public fields, handed out by the reader and
//! taken back by a constructor that does no walk — see `docs/design/roadmap.md`,
//! "The seek table is a plain value the caller serializes". Its cost is 32 bytes
//! a block, which is `KD1`.

use core::ops::Range;

/// A stream's integrity check.
///
/// This crate's own enum rather than the backend's, because it has to mean the
/// same thing under a backend that does not link `liblzma`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum Check {
    /// `--check=none`. There is nothing to verify, and nothing to complete when
    /// the reader seeks away from a partly-decoded block.
    None,
    /// CRC32, check id `0x01`.
    Crc32,
    /// CRC64 (CRC-64/XZ), check id `0x04`.
    Crc64,
    /// SHA-256, check id `0x0a`.
    Sha256,
    /// A check id the format reserves and nothing implements.
    ///
    /// The walk records it rather than refusing the file, so the shape stays
    /// inspectable; decoding a block in such a stream raises
    /// [`Error::UnsupportedCheck`](crate::Error::UnsupportedCheck) under full
    /// verification only.
    Unsupported(u8),
}

impl Check {
    /// The check id as it appears in a stream's flags.
    ///
    /// Only the low nibble of the flags' second byte is the check id, so this
    /// is always `0..=0x0f`.
    pub fn id(self) -> u8 {
        match self {
            Check::None => 0x00,
            Check::Crc32 => 0x01,
            Check::Crc64 => 0x04,
            Check::Sha256 => 0x0a,
            Check::Unsupported(id) => id,
        }
    }

    /// A check id read from a stream's flags.
    ///
    /// Ids above `0x0f` do not exist in the format — a stream declaring one is
    /// rejected by the flags parse, not represented here — so this masks to the
    /// low nibble rather than failing.
    pub fn from_id(id: u8) -> Check {
        match id & 0x0f {
            0x00 => Check::None,
            0x01 => Check::Crc32,
            0x04 => Check::Crc64,
            0x0a => Check::Sha256,
            other => Check::Unsupported(other),
        }
    }

    /// How many bytes this check occupies at the end of a block.
    ///
    /// The format sizes checks from the id in groups of three — `0x01..=0x03`
    /// are four bytes, `0x04..=0x06` eight, and so on to `0x0d..=0x0f` at
    /// sixty-four — which is what makes an unsupported check's *size* known even
    /// though its algorithm is not, and therefore what keeps every offset in a
    /// stream carrying one exactly where `xz` wrote it. See `xz-invariants.md`,
    /// `I7`.
    pub fn size(self) -> u64 {
        let id = self.id();
        if id == 0 { 0 } else { 4 << ((id - 1) / 3) }
    }

    /// Whether this crate can compute the check and compare it.
    pub fn is_supported(self) -> bool {
        !matches!(self, Check::Unsupported(_))
    }
}

/// One `.xz` stream in the file.
///
/// A file is one or more streams concatenated, each optionally followed by
/// stream padding.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StreamEntry {
    /// File-absolute offset of the first byte of the stream header.
    pub compressed_offset: u64,
    /// Uncompressed offset at which this stream's plaintext begins.
    pub uncompressed_offset: u64,
    /// Stream header through stream footer, **excluding** the padding that
    /// follows.
    pub compressed_size: u64,
    /// The plaintext this stream carries.
    pub uncompressed_size: u64,
    /// The integrity check every block in this stream uses.
    pub check: Check,
    /// Stream padding *after* this stream: null bytes, a multiple of four.
    pub padding: u64,
    /// Index into [`SeekTable::blocks`] of this stream's first block.
    ///
    /// Meaningless when `block_count` is zero; a stream may legitimately have
    /// no blocks at all — `xz -c /dev/null` writes one.
    pub first_block: usize,
    /// How many blocks this stream has.
    pub block_count: usize,
}

/// One block.
///
/// 32 bytes. The filter chain is deliberately not here: the block header is
/// read at seek time regardless, since it is the first bytes of the same read
/// that fetches the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BlockEntry {
    /// File-absolute offset of the first byte of the block *header*.
    pub compressed_offset: u64,
    /// File-absolute uncompressed offset of this block's first plaintext byte.
    pub uncompressed_offset: u64,
    /// Block header + payload + check, without the padding that aligns the
    /// block to four bytes.
    ///
    /// Stored rather than the padded total because it is what the stream index
    /// holds, and because it locates every field of the block exactly. A block
    /// is `header · compressed data · block padding · check`, so with
    /// `header_size` and the stream's check size in hand the payload runs
    /// `compressed_offset + header_size .. compressed_offset + unpadded_size -
    /// check size`, and **the check's last byte is at `compressed_offset +
    /// total_size - 1`** — the padding sits between the two, not after them.
    /// The padded total is [`BlockEntry::total_size`].
    ///
    /// That layout is `docs/design/xz-invariants.md`'s `I15`, which carries the
    /// format specification's own wording for it.
    pub unpadded_size: u64,
    /// The plaintext this block carries.
    pub uncompressed_size: u64,
}

impl BlockEntry {
    /// `unpadded_size` rounded up to a multiple of four — the distance to the
    /// next block's `compressed_offset`, and the `total_size` column of
    /// `xz --list -vv --robot`.
    pub fn total_size(&self) -> u64 {
        (self.unpadded_size + 3) & !3
    }

    /// The uncompressed range this block covers.
    pub fn uncompressed_range(&self) -> Range<u64> {
        self.uncompressed_offset..self.uncompressed_offset + self.uncompressed_size
    }
}

/// The map from uncompressed offsets to compressed ones.
///
/// Handed out by `Reader::index` and taken back by a constructor that does no
/// walk. Every field is public and documented on purpose: the first downstream
/// persists this inside its own cache envelope, so what it needs is the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SeekTable {
    /// The compressed file's size, including any stream padding.
    ///
    /// Recorded so that a re-injected table can be cross-checked against the
    /// source in one call, rather than by re-walking.
    pub compressed_file_size: u64,
    /// Every stream, in file order.
    pub streams: Vec<StreamEntry>,
    /// Every block in the file, flat and in file order.
    ///
    /// Flat rather than per-stream so that a lookup by uncompressed offset is
    /// one binary search over the whole file.
    pub blocks: Vec<BlockEntry>,
}

impl SeekTable {
    /// Build the table by walking `source`'s stream footers backward.
    ///
    /// One read per stream and one for the file's tail; nothing is decoded and
    /// no block header is read. This is what `Reader::new` will do before it has
    /// a reader to hand back, and it is public so that a caller who wants only
    /// the table — to persist it, or to report a file's shape — need not build
    /// one.
    pub fn from_source<S: crate::CompressedSource>(source: &S) -> crate::Result<SeekTable> {
        crate::walk::walk(source)
    }

    /// How many streams the file has.
    pub fn stream_count(&self) -> usize {
        self.streams.len()
    }

    /// How many blocks the file has, across every stream.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// The file's whole uncompressed size.
    pub fn uncompressed_size(&self) -> u64 {
        self.streams.iter().map(|s| s.uncompressed_size).sum()
    }

    /// The largest block, in uncompressed bytes — the worst case for an
    /// arbitrary backward seek, since a backward seek within a block restarts at
    /// that block's start.
    ///
    /// Zero for a file with no blocks.
    pub fn max_block_uncompressed(&self) -> u64 {
        self.blocks
            .iter()
            .map(|b| b.uncompressed_size)
            .max()
            .unwrap_or(0)
    }

    /// Whether seeking buys anything: more than one block.
    ///
    /// A one-block file is read correctly, necessarily by decoding from zero.
    pub fn is_seekable(&self) -> bool {
        self.blocks.len() > 1
    }

    /// The exact number of uncompressed bytes that must be decoded to reach
    /// `offset` from a cold start.
    ///
    /// Exact, not a bound: it is `offset` minus the start of the block covering
    /// it. On a one-stream-one-block file it is `offset` itself, which is what
    /// makes "correct, but necessarily by decoding from zero" a number a caller
    /// can warn its user on rather than a flag it must interpret.
    ///
    /// Zero at or past the end of the file: there is nothing there to decode.
    pub fn seek_cost(&self, offset: u64) -> u64 {
        match self.block_containing(offset) {
            Some(i) => offset - self.blocks[i].uncompressed_offset,
            None => 0,
        }
    }

    /// Check that this table could have come from a walk of a source
    /// `source_size` bytes long, and that the reader may therefore trust it.
    ///
    /// This is what [`Builder::open_with_table`](crate::Builder::open_with_table)
    /// runs in place of the walk. It reads no bytes: `source_size` is the one
    /// thing it takes from the source, which is the roadmap's "cross-check the
    /// source cheaply" — re-walking is the cost the table exists to avoid.
    ///
    /// # What it enforces, and why that list
    ///
    /// **More than the reader assumes, and exactly what the walk
    /// guarantees.** The table is a plain value with public fields, so a caller
    /// can hand back anything at all; without this the failure is not an error
    /// but confidently wrong bytes, which is the whole reason
    /// [`Error::InvalidTable`] exists.
    ///
    /// *Rejected: loosening this to the reader's own assumptions* — monotonic
    /// uncompressed offsets, blocks inside the source, blocks inside some
    /// stream. It would accept a table with a hole, which
    /// [`Reader::read_at`](crate::Reader::read_at) can only report as the short
    /// return that fill-or-EOF reserves for the end of the file, and a table
    /// whose blocks sit outside their stream, which
    /// `Reader::check_of` would decode under a neighbouring stream's check
    /// algorithm. It would also buy nothing back: [`SeekTable::from_source`]
    /// derives a stream's start by subtracting its blocks from its index, so
    /// the strict rule refuses no table the walk can build.
    ///
    /// The streams tile the file exactly — `[header · blocks · index · footer]`
    /// then padding, the next stream starting where that ends — and each
    /// stream's blocks tile its own body from `compressed_offset + 12`, leaving
    /// room beneath them for the smallest possible index and a footer. The
    /// plaintext is one contiguous run from zero: each block's
    /// `uncompressed_offset` must be exactly where its predecessor ended, which
    /// is what "monotonic, no gaps or overlaps" means once it is stated as an
    /// equality rather than as an inequality. A gap would show up as a
    /// [`Reader::read_at`](crate::Reader::read_at) that returns short in the
    /// middle of a file, which is the one thing fill-or-EOF promises never
    /// happens.
    ///
    /// **`first_block` is checked only where it means something.** A stream
    /// with no blocks is legitimate — `xz -c /dev/null` writes one — and the
    /// field's own documentation calls it meaningless there, so validating it
    /// would reject a table whose producer honoured what the field says. The
    /// block *count* is still checked in full: the counts must partition
    /// `blocks` with nothing left over.
    ///
    /// A block whose `uncompressed_size` is zero is accepted. Nothing produces
    /// one, but it does not break
    /// [`block_containing`](SeekTable::block_containing) — the binary search
    /// lands on the last block sharing an offset, which is the non-empty one —
    /// and rejecting it would make a table the walk *could* produce fail
    /// re-injection, which is the property this function exists to keep.
    pub(crate) fn validate(&self, source_size: u64) -> crate::Result<()> {
        use crate::walk::{
            INDEX_SIZE_MIN, STREAM_FOOTER_SIZE, STREAM_HEADER_SIZE, STREAM_SIZE_MIN,
            UNPADDED_SIZE_MIN,
        };

        fn bad<T>(compressed_offset: u64) -> crate::Result<T> {
            Err(crate::Error::InvalidTable { compressed_offset })
        }

        if self.compressed_file_size != source_size {
            return bad(self.compressed_file_size);
        }
        // Streams and stream padding are both multiples of four, so the file
        // is — which is also what bounds every `total_size()` below.
        if !self.compressed_file_size.is_multiple_of(4) {
            return bad(self.compressed_file_size);
        }
        if self.streams.is_empty() {
            return bad(0);
        }

        let mut next_block = 0usize;
        // Where the next stream must begin, in both coordinate spaces.
        let mut c = 0u64;
        let mut u = 0u64;

        for s in &self.streams {
            if s.compressed_offset != c || s.uncompressed_offset != u {
                return bad(c);
            }
            if s.compressed_size < STREAM_SIZE_MIN || !s.compressed_size.is_multiple_of(4) {
                return bad(c);
            }
            if !s.padding.is_multiple_of(4) {
                return bad(c);
            }
            let Some(body_end) = c.checked_add(s.compressed_size) else {
                return bad(c);
            };
            let Some(next_c) = body_end.checked_add(s.padding) else {
                return bad(c);
            };
            if next_c > self.compressed_file_size {
                return bad(c);
            }

            let Some(last_block) = next_block.checked_add(s.block_count) else {
                return bad(c);
            };
            if last_block > self.blocks.len() {
                return bad(c);
            }
            if s.block_count > 0 && s.first_block != next_block {
                return bad(c);
            }

            // The stream's blocks tile its body from just past its header, and
            // must leave room for the smallest index and a footer beneath them.
            let mut bc = c + STREAM_HEADER_SIZE;
            let mut bu = u;
            for b in &self.blocks[next_block..last_block] {
                if b.compressed_offset != bc || b.uncompressed_offset != bu {
                    return bad(bc);
                }
                if b.unpadded_size < UNPADDED_SIZE_MIN
                    || b.unpadded_size > self.compressed_file_size
                {
                    return bad(bc);
                }
                bc = match bc.checked_add(b.total_size()) {
                    Some(bc) => bc,
                    None => return bad(b.compressed_offset),
                };
                let room = body_end.checked_sub(bc);
                if room.is_none_or(|room| room < INDEX_SIZE_MIN + STREAM_FOOTER_SIZE) {
                    return bad(b.compressed_offset);
                }
                bu = match bu.checked_add(b.uncompressed_size) {
                    Some(bu) => bu,
                    None => return bad(b.compressed_offset),
                };
            }
            if bu - u != s.uncompressed_size {
                return bad(c);
            }

            next_block = last_block;
            c = next_c;
            u = bu;
        }

        if c != self.compressed_file_size {
            return bad(c);
        }
        if next_block != self.blocks.len() {
            return bad(self.blocks[next_block].compressed_offset);
        }
        Ok(())
    }

    /// The block indices a half-open uncompressed byte range covers.
    ///
    /// The answer is a contiguous `Range<usize>` into
    /// [`SeekTable::blocks`], and it is the *work* a bulk read over `range`
    /// implies: a caller sizes a worker pool with it, and a caller who has not
    /// set anything up yet uses it as the diagnostic — *"this range admits three
    /// workers"* — that `xz -T0` or `--block-size` is the remedy for. It needs
    /// no source and no configuration, so a persisted table answers it.
    ///
    /// Three edges are pinned, so that a caller who did defensive arithmetic and
    /// one who did not get the same answer:
    ///
    /// * **An empty range covers no blocks.** `start >= end`, or a range
    ///   beginning at or past [`SeekTable::uncompressed_size`], gives `0..0` —
    ///   not an error, and not the block containing the start. Zero-length
    ///   ranges arise in a caller's span arithmetic, and a *"how much work is
    ///   this"* query answering non-zero for no work is wrong.
    /// * **A range starting mid-block includes that block**, because a bulk read
    ///   decodes a partial first block whole; the answer is a true cost only if
    ///   it says so.
    /// * **A range extending past the end of the stream clamps**, with no error.
    ///   An error would make every caller do the arithmetic twice — once to ask,
    ///   and once to be allowed to ask.
    ///
    /// The end is the first block starting at or after `range.end`, so the
    /// result is a contiguous run of indices rather than the set of blocks whose
    /// plaintext actually overlaps. The two differ only for a zero-length block
    /// strictly inside the range, which nothing produces and which the table's
    /// own validation accepts; a contiguous run is what a caller scheduling by
    /// index needs.
    ///
    /// The clamp is against the last block's end rather than
    /// [`SeekTable::uncompressed_size`], which is a sum over every stream and so
    /// costs 31,150 additions on the corpus file this crate was built for. The
    /// two are equal: a stream with no blocks carries no plaintext.
    pub fn blocks_in(&self, range: Range<u64>) -> Range<usize> {
        let Some(last) = self.blocks.last() else {
            return 0..0;
        };
        let end = range
            .end
            .min(last.uncompressed_offset + last.uncompressed_size);
        if range.start >= end {
            return 0..0;
        }
        let start_i = self
            .blocks
            .partition_point(|b| b.uncompressed_offset <= range.start)
            .saturating_sub(1);
        let end_i = self.blocks.partition_point(|b| b.uncompressed_offset < end);
        start_i..end_i
    }

    /// The index into [`SeekTable::blocks`] of the block covering `offset`, or
    /// `None` at or past the end of the file.
    ///
    /// Binary search over the flat block list: uncompressed offsets are
    /// monotonic across the whole file, streams included, because a stream's
    /// plaintext continues its predecessor's.
    pub(crate) fn block_containing(&self, offset: u64) -> Option<usize> {
        let i = self
            .blocks
            .partition_point(|b| b.uncompressed_offset <= offset);
        let i = i.checked_sub(1)?;
        let b = &self.blocks[i];
        (offset < b.uncompressed_offset + b.uncompressed_size).then_some(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-built rather than walked: these are the queries, not the walk, and
    /// the walk does not exist yet. The differential evidence for the *table*
    /// is `tests/oracle.rs`, against `xz --list -vv --robot`.
    fn table(block_sizes: &[u64]) -> SeekTable {
        let mut blocks = Vec::new();
        let mut c = 12u64;
        let mut u = 0u64;
        for &size in block_sizes {
            blocks.push(BlockEntry {
                compressed_offset: c,
                uncompressed_offset: u,
                unpadded_size: size / 2 + 21,
                uncompressed_size: size,
            });
            c += blocks.last().unwrap().total_size();
            u += size;
        }
        SeekTable {
            compressed_file_size: c + 24,
            streams: vec![StreamEntry {
                compressed_offset: 0,
                uncompressed_offset: 0,
                compressed_size: c + 24,
                uncompressed_size: u,
                check: Check::Crc64,
                padding: 0,
                first_block: 0,
                block_count: blocks.len(),
            }],
            blocks,
        }
    }

    #[test]
    fn check_ids_and_sizes_follow_the_format_s_groups_of_three() {
        assert_eq!(Check::None.size(), 0);
        assert_eq!(Check::Crc32.size(), 4);
        assert_eq!(Check::Crc64.size(), 8);
        assert_eq!(Check::Sha256.size(), 32);
        for id in 0..=0x0fu8 {
            let expected = if id == 0 { 0 } else { 4 << ((id - 1) / 3) };
            assert_eq!(Check::from_id(id).size(), expected, "check id {id:#x}");
            assert_eq!(Check::from_id(id).id(), id, "check id {id:#x}");
        }
        assert!(!Check::from_id(0x05).is_supported());
        assert!(Check::Crc64.is_supported());
    }

    #[test]
    fn the_shape_queries_read_off_the_block_list() {
        let t = table(&[100, 300, 200]);
        assert_eq!(t.stream_count(), 1);
        assert_eq!(t.block_count(), 3);
        assert_eq!(t.uncompressed_size(), 600);
        assert_eq!(t.max_block_uncompressed(), 300);
        assert!(t.is_seekable());

        let one = table(&[100]);
        assert!(!one.is_seekable());

        let empty = table(&[]);
        assert!(!empty.is_seekable());
        assert_eq!(empty.uncompressed_size(), 0);
        assert_eq!(empty.max_block_uncompressed(), 0);
        assert_eq!(empty.seek_cost(0), 0);
    }

    #[test]
    fn seek_cost_is_the_distance_from_the_covering_block_s_start() {
        let t = table(&[100, 300, 200]);
        assert_eq!(t.seek_cost(0), 0);
        assert_eq!(t.seek_cost(99), 99);
        assert_eq!(t.seek_cost(100), 0);
        assert_eq!(t.seek_cost(399), 299);
        assert_eq!(t.seek_cost(400), 0);
        assert_eq!(t.seek_cost(599), 199);
        // At and past the end there is nothing to decode.
        assert_eq!(t.seek_cost(600), 0);
        assert_eq!(t.seek_cost(u64::MAX), 0);
    }

    #[test]
    fn on_a_one_block_file_seek_cost_is_the_offset_itself() {
        let t = table(&[1 << 20]);
        for offset in [0, 1, 4096, (1 << 20) - 1] {
            assert_eq!(t.seek_cost(offset), offset);
        }
    }

    #[test]
    fn block_containing_finds_every_block_at_its_own_boundaries() {
        let t = table(&[100, 300, 200]);
        assert_eq!(t.block_containing(0), Some(0));
        assert_eq!(t.block_containing(100), Some(1));
        assert_eq!(t.block_containing(399), Some(1));
        assert_eq!(t.block_containing(400), Some(2));
        assert_eq!(t.block_containing(600), None);
    }

    #[test]
    fn blocks_in_covers_every_block_a_range_touches() {
        let t = table(&[100, 300, 200]);

        // The whole file, and each block on its own boundaries.
        assert_eq!(t.blocks_in(0..600), 0..3);
        assert_eq!(t.blocks_in(0..1), 0..1);
        assert_eq!(t.blocks_in(0..100), 0..1);
        assert_eq!(t.blocks_in(0..101), 0..2);
        assert_eq!(t.blocks_in(100..400), 1..2);
        assert_eq!(t.blocks_in(399..400), 1..2);
        assert_eq!(t.blocks_in(400..401), 2..3);
        assert_eq!(t.blocks_in(599..600), 2..3);

        // A range starting mid-block includes that block, because the read
        // decodes it whole.
        assert_eq!(t.blocks_in(50..60), 0..1);
        assert_eq!(t.blocks_in(150..350), 1..2);
        assert_eq!(t.blocks_in(99..401), 0..3);

        // The answer is a contiguous run, so it agrees with the covering-block
        // lookup at every offset the file has.
        for offset in 0..600u64 {
            assert_eq!(
                t.blocks_in(offset..offset + 1),
                t.block_containing(offset).unwrap()..t.block_containing(offset).unwrap() + 1,
                "at {offset}"
            );
        }
    }

    #[test]
    fn blocks_in_s_three_edges_are_empty_mid_block_and_clamped() {
        let t = table(&[100, 300, 200]);

        // Empty: zero-length anywhere, inverted, and at or past the end. An
        // inverted range is one of the shapes being pinned, so the lint that
        // says a literal one yields nothing is exactly the point.
        #[allow(clippy::reversed_empty_ranges)]
        let empties = [0..0, 150..150, 600..600, 600..700, 599..599, 300..100];
        for r in empties {
            assert_eq!(t.blocks_in(r.clone()), 0..0, "{r:?}");
        }
        assert_eq!(t.blocks_in(u64::MAX..u64::MAX), 0..0);

        // Past the end clamps rather than failing.
        assert_eq!(t.blocks_in(0..u64::MAX), 0..3);
        assert_eq!(t.blocks_in(500..u64::MAX), 2..3);
        assert_eq!(t.blocks_in(599..1_000_000), 2..3);

        // A file with no blocks has no work in any range.
        let empty = table(&[]);
        assert_eq!(empty.blocks_in(0..u64::MAX), 0..0);

        // A zero-length block resolves the way `block_containing` does: the
        // last block sharing an offset is the one that covers it.
        let zero = table(&[100, 0, 200]);
        assert_eq!(zero.blocks_in(100..300), 2..3);
        assert_eq!(zero.blocks_in(0..300), 0..3);
    }

    /// The two shapes `validate` deliberately lets through.
    ///
    /// Both are cases where rejecting would refuse a table the walk can
    /// produce, which is the property `tests/roundtrip.rs` asserts over the
    /// whole corpus and this pins where the corpus has no fixture: a stream
    /// with no blocks has a `first_block` its own documentation calls
    /// meaningless, and a zero-length block does not break
    /// [`SeekTable::block_containing`] — the binary search lands on the last
    /// block sharing an offset, which is the non-empty one.
    ///
    /// The rejections themselves are `tests/roundtrip.rs`'s, one mutation to a
    /// walked table per rule.
    #[test]
    fn validation_accepts_what_a_walk_could_have_produced() {
        let t = table(&[100, 300, 200]);
        assert!(t.validate(t.compressed_file_size).is_ok());
        assert!(t.validate(t.compressed_file_size + 4).is_err());

        let mut empty = table(&[]);
        empty.streams[0].first_block = usize::MAX;
        assert!(empty.validate(empty.compressed_file_size).is_ok());

        let zero = table(&[100, 0, 200]);
        assert!(zero.validate(zero.compressed_file_size).is_ok());
        assert_eq!(zero.block_containing(100), Some(2));
        assert_eq!(zero.uncompressed_size(), 300);
    }

    #[test]
    fn total_size_rounds_the_unpadded_size_up_to_four() {
        for (unpadded, total) in [(12, 12), (13, 16), (14, 16), (15, 16), (16, 16)] {
            let b = BlockEntry {
                compressed_offset: 12,
                uncompressed_offset: 0,
                unpadded_size: unpadded,
                uncompressed_size: 1,
            };
            assert_eq!(b.total_size(), total);
        }
    }
}
