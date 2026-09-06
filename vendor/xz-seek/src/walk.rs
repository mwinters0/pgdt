//! The backward footer walk: an `.xz` file's own indexes, turned into a
//! [`SeekTable`].
//!
//! Nothing here decodes anything. The walk reads each stream's footer, the
//! index it points back at, and the stream header it derives the position of,
//! and that is enough to place every block in the file.
//!
//! # One read per stream, because of where a header sits
//!
//! A stream's header is adjacent to its *predecessor's* padding, footer and
//! index:
//!
//! ```text
//! …[stream N-1 index][stream N-1 footer][padding][stream N header]…
//! ```
//!
//! So [`Backfill`] refills as a fixed 4 KiB block **ending at the end of the range
//! asked for**. Walking backward, that one policy straddles the boundary for
//! free: the read that fetches stream N's 12-byte header also carries the 4 KiB
//! below it, which is exactly stream N-1's tail. The walk is therefore one read
//! per stream plus one for the file's own tail, against the naive two per
//! stream — 31,150 reads rather than 62,300 on the real multistream corpus file.
//! An index larger than the window falls back to a read sized to the index,
//! which is the same policy with a bigger `len`.
//!
//! # What a failure is called
//!
//! Three variants come out of this module, and the split is R7's:
//!
//! * [`Error::NotXz`] — the six magic bytes at offset 0 are wrong. Read once,
//!   before anything else, because it is the only thing separating a random
//!   file from a clipped download: both fail at the tail.
//! * [`Error::Truncated`] — the file claims to be `.xz` and the structure runs
//!   off its end. That includes a size that is not a multiple of four, a read
//!   that would pass the last byte, and **a tail with no footer magic where a
//!   footer must be**, which is what a `head -c` cut leaves behind.
//! * [`Error::IndexInconsistent`] — the structure is present and disagrees with
//!   itself: a footer or index or header CRC32 that does not match, reserved
//!   stream-flag bits, a header that is not where the index says the stream
//!   starts, header flags that differ from the footer's copy. The format calls
//!   these separate errors; the taxonomy deliberately does not, because a
//!   caller acts on all of them identically — the file is damaged in its index
//!   region — and the offset each carries says which one it was.
//!
//! `Error::NotXz` is decided *only* by the magic. Once past it, a walk failure
//! is one of the other two, so a damaged file is never reported as "not xz".
//!
//! *Rejected: drawing the `Truncated`/`IndexInconsistent` line at the footer's
//! CRC32 rather than at its magic.* Under that rule a footer whose CRC32 fails
//! is "no real footer here", so a `head -c` cut that happens to end in `YZ`
//! would be called a truncation, which is what it is. The cost is
//! `corrupt-footer.xz` — a single flipped bit in a present footer's backward
//! size — which would report `Truncated` as well, collapsing the distinction
//! the corpus split that fixture out to make and leaving no damaged fixture in
//! the index region that is not called a truncation. The mislabel the magic
//! rule permits needs a cut landing on a four-byte boundary whose last two
//! bytes are `YZ`; the mislabel the CRC32 rule permits is any flipped bit in
//! any footer.
//!
//! What settles it is that `liblzma` partitions the same failure the same way,
//! and says why: `lzma_stream_footer_decode` returns `LZMA_FORMAT_ERROR` on a
//! missing magic and `LZMA_DATA_ERROR` on a failing CRC32, and the header
//! decoder beside it comments that the CRC32 is verified "so we can distinguish
//! between corrupt and unsupported files". The magic asks whether the structure
//! is here; the CRC32 asks whether what is here is damaged. A second backend
//! goes behind this taxonomy, and the rule that matches the format's own
//! reference decoder is the one likeliest to survive that seam. See
//! `xz-invariants.md`, `I11` — which also records that this line can never be
//! settled differentially, since `xz --list` prints one string for every
//! damaged fixture in the corpus.
//!
//! # Only the tail of a file can be cut
//!
//! That line decides which *kind* of damage a failure is; where the walk found
//! it decides whether "damage" is the right word at all. A truncation removes a
//! suffix, so the two failures that mean "no stream ends here" — an absent
//! footer magic, and a boundary with less than a minimum stream's worth of file
//! beneath it — are a truncation **only at the tail-most stream**.
//!
//! The first call to [`one_stream`] starts at the file's own end and is the one
//! that can meet a cut. Every later call starts at a boundary the walk
//! *derived* — from a footer it read, an index whose CRC32 matched, and a header
//! found where that index said one would be — so every byte from there to the
//! end of the file is known to be present, and the same two failures are the
//! container disagreeing with itself: [`Error::IndexInconsistent`].
//!
//! Without that condition a file whose *interior* is damaged is reported as
//! "ends at offset N" for an N that is nowhere near its end, which sends a
//! caller to re-fetch a file it already has whole.

use crate::error::{Error, Result};
use crate::source::CompressedSource;
use crate::table::{BlockEntry, Check, SeekTable, StreamEntry};

/// The straddling window, in bytes. Not configurable: see the module docs.
const WINDOW: usize = 4096;

pub(crate) const STREAM_HEADER_SIZE: u64 = 12;
pub(crate) const STREAM_FOOTER_SIZE: u64 = 12;
/// Indicator, a one-byte record count, two bytes of padding and a CRC32.
pub(crate) const INDEX_SIZE_MIN: u64 = 8;
/// The smallest possible stream: a header, an empty index and a footer.
pub(crate) const STREAM_SIZE_MIN: u64 = STREAM_HEADER_SIZE + INDEX_SIZE_MIN + STREAM_FOOTER_SIZE;

const HEADER_MAGIC: [u8; 6] = [0xfd, b'7', b'z', b'X', b'Z', 0x00];
const FOOTER_MAGIC: [u8; 2] = *b"YZ";

/// `LZMA_VLI_BYTES_MAX`: seven bits a byte over 63 bits.
const VLI_BYTES_MAX: usize = 9;
/// The format's stated floor — a block is at least a header and a byte.
pub(crate) const UNPADDED_SIZE_MIN: u64 = 5;

/// Round up to the next multiple of four: block padding, index padding.
fn round4(v: u64) -> u64 {
    (v + 3) & !3
}

/// Shared with [`crate::block`]: the block header carries a CRC32 too.
pub(crate) fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Shared with [`crate::block`], which reaches the same verdict for the same
/// reason: the structure is present and disagrees with itself.
pub(crate) fn inconsistent<T>(compressed_offset: u64) -> Result<T> {
    Err(Error::IndexInconsistent { compressed_offset })
}

/// A structure that is missing where a stream must end: a cut at the tail-most
/// stream, and a self-contradiction anywhere else. See the module docs, "Only
/// the tail of a file can be cut".
fn missing_end<T>(tail_most: bool, compressed_offset: u64) -> Result<T> {
    if tail_most {
        Err(Error::Truncated { compressed_offset })
    } else {
        inconsistent(compressed_offset)
    }
}

/// A 4 KiB read, cached, that refills backward.
///
/// The refill positions the window to **end** where the requested range ends.
/// That is the whole of the straddle: a backward walk asking for the 12 bytes at
/// a stream's start gets the 4 KiB below it in the same read, and that is where
/// the previous stream's index and footer live.
struct Backfill<'s, S: CompressedSource> {
    source: &'s S,
    size: u64,
    start: u64,
    buf: Vec<u8>,
}

impl<'s, S: CompressedSource> Backfill<'s, S> {
    fn new(source: &'s S, size: u64) -> Self {
        Backfill {
            source,
            size,
            start: 0,
            buf: Vec::new(),
        }
    }

    /// The `len` bytes at `offset`, from the window if it holds them.
    ///
    /// A range that would pass the last byte of the file is [`Error::Truncated`]
    /// at the file's size: this is the only place the walk can run off the end,
    /// so it is the only place that check has to be written.
    fn bytes(&mut self, offset: u64, len: usize) -> Result<&[u8]> {
        let end = offset
            .checked_add(len as u64)
            .filter(|end| *end <= self.size)
            .ok_or(Error::Truncated {
                compressed_offset: self.size,
            })?;
        if offset < self.start || end > self.start + self.buf.len() as u64 {
            self.fill(offset, end)?;
        }
        let lo = (offset - self.start) as usize;
        Ok(&self.buf[lo..lo + len])
    }

    /// The same, copied out, for a range that has to outlive the next refill.
    fn array<const N: usize>(&mut self, offset: u64) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(offset, N)?);
        Ok(out)
    }

    fn fill(&mut self, offset: u64, end: u64) -> Result<()> {
        let want = ((end - offset) as usize).max(WINDOW) as u64;
        let start = end.saturating_sub(want);
        let len = (end - start) as usize;
        self.buf.clear();
        self.buf.resize(len, 0);
        let got = self
            .source
            .read_at(start, &mut self.buf)
            .map_err(|e| Error::io(start, e))?;
        if got < len {
            // `size()` promised these bytes. The source is shorter than it said.
            return Err(Error::Truncated {
                compressed_offset: start + got as u64,
            });
        }
        self.start = start;
        Ok(())
    }
}

/// One stream as the backward walk finds it, before the file is put in order.
struct Walked {
    start: u64,
    /// One past the footer's last byte — the stream's end, padding excluded.
    end: u64,
    check: Check,
    padding: u64,
    /// `(unpadded_size, uncompressed_size)` per block, in stream order.
    records: Vec<(u64, u64)>,
}

/// Build a [`SeekTable`] by walking `source`'s stream footers backward.
pub(crate) fn walk<S: CompressedSource>(source: &S) -> Result<SeekTable> {
    let size = source.size().map_err(|e| Error::io(0, e))?;

    // The magic is read once, and it is the only thing that decides `NotXz`.
    let mut magic = [0u8; HEADER_MAGIC.len()];
    let got = source.read_at(0, &mut magic).map_err(|e| Error::io(0, e))?;
    if got < magic.len() || magic != HEADER_MAGIC {
        return Err(Error::NotXz {
            compressed_offset: 0,
        });
    }
    // Streams and stream padding are both multiples of four, so the file is.
    if size % 4 != 0 {
        return Err(Error::Truncated {
            compressed_offset: size,
        });
    }

    let mut window = Backfill::new(source, size);
    let mut walked: Vec<Walked> = Vec::new();
    let mut end = size;
    // Only the first call starts at the file's own end, so only it can meet a
    // cut rather than a contradiction.
    let mut tail_most = true;
    loop {
        let stream = one_stream(&mut window, end, tail_most)?;
        let start = stream.start;
        walked.push(stream);
        if start == 0 {
            break;
        }
        end = start;
        tail_most = false;
    }

    assemble(size, walked)
}

/// The stream whose padding ends at `end`, read backward from there.
///
/// `tail_most` says whether `end` is the file's own end. It decides nothing but
/// the *name* of the two failures that mean "no stream ends here" — see the
/// module docs, "Only the tail of a file can be cut".
fn one_stream<S: CompressedSource>(
    w: &mut Backfill<'_, S>,
    end: u64,
    tail_most: bool,
) -> Result<Walked> {
    // Stream padding: null bytes, a multiple of four. A footer's last four bytes
    // are its flags and `YZ`, which are never all zero, so this cannot walk into
    // the stream it is looking for.
    let mut padding = 0;
    while end - padding >= 4 && *w.bytes(end - padding - 4, 4)? == [0, 0, 0, 0] {
        padding += 4;
    }

    let footer_end = end - padding;
    if footer_end < STREAM_SIZE_MIN {
        return missing_end(tail_most, footer_end);
    }
    let footer_start = footer_end - STREAM_FOOTER_SIZE;
    let f: [u8; 12] = w.array(footer_start)?;

    // No footer magic here means there is no footer here: at the tail of a file
    // that claims to be xz, that is a cut; anywhere else it is the index region
    // contradicting itself.
    if f[10..12] != FOOTER_MAGIC {
        return missing_end(tail_most, footer_start);
    }
    if crate::check::crc32(&f[4..10]) != le32(&f[0..4]) {
        return inconsistent(footer_start);
    }
    let flags = [f[8], f[9]];
    let check = stream_flags(flags, footer_start + 8)?;
    let index_size = (le32(&f[4..8]) as u64 + 1) * 4;

    let index_start = match footer_start.checked_sub(index_size) {
        Some(at) => at,
        None => return inconsistent(footer_start + 4),
    };
    let records = index(w, index_start, index_size)?;

    let mut blocks_total = 0u64;
    for (unpadded, _) in &records {
        blocks_total = match blocks_total.checked_add(round4(*unpadded)) {
            Some(t) => t,
            None => return inconsistent(index_start),
        };
    }
    let start = match index_start.checked_sub(blocks_total + STREAM_HEADER_SIZE) {
        Some(at) => at,
        None => return inconsistent(index_start),
    };

    // The header costs no extra read: the window refills to end where this
    // range ends, so it carries the previous stream's tail with it.
    let h: [u8; 12] = w.array(start)?;
    if h[0..6] != HEADER_MAGIC {
        return inconsistent(start);
    }
    if crate::check::crc32(&h[6..8]) != le32(&h[8..12]) {
        return inconsistent(start + 8);
    }
    // The format requires the two copies of the flags to be identical.
    if h[6..8] != flags {
        return inconsistent(start + 6);
    }

    Ok(Walked {
        start,
        end: footer_end,
        check,
        padding,
        records,
    })
}

/// The two stream-flag bytes, which the header and the footer both carry.
///
/// The first byte is reserved and must be zero; the second's high nibble is
/// reserved too. The low nibble is the check id, and an id nothing implements is
/// recorded rather than refused — see [`Check::Unsupported`].
fn stream_flags(flags: [u8; 2], at: u64) -> Result<Check> {
    if flags[0] != 0 || flags[1] & 0xf0 != 0 {
        return inconsistent(at);
    }
    Ok(Check::from_id(flags[1]))
}

/// The stream index: an indicator, a record count, the records, padding, CRC32.
///
/// Read whole. An index bigger than the window is one sized read rather than a
/// walk through it, because the alternative — a cursor stepping forward through
/// a window that refills backward — would refill on nearly every record.
fn index<S: CompressedSource>(
    w: &mut Backfill<'_, S>,
    start: u64,
    size: u64,
) -> Result<Vec<(u64, u64)>> {
    if size < INDEX_SIZE_MIN {
        return inconsistent(start);
    }
    let idx = w.bytes(start, size as usize)?.to_vec();
    let crc_at = idx.len() - 4;
    if crate::check::crc32(&idx[..crc_at]) != le32(&idx[crc_at..]) {
        return inconsistent(start);
    }
    if idx[0] != 0x00 {
        return inconsistent(start);
    }

    let mut p = 1;
    let count = vli(&idx[..crc_at], &mut p, start)?;
    let mut records = Vec::new();
    for _ in 0..count {
        let unpadded = vli(&idx[..crc_at], &mut p, start)?;
        let uncompressed = vli(&idx[..crc_at], &mut p, start)?;
        if unpadded < UNPADDED_SIZE_MIN {
            return inconsistent(start + p as u64);
        }
        records.push((unpadded, uncompressed));
    }

    // Index padding: 0-3 null bytes, which is what makes the whole index a
    // multiple of four.
    let pad = &idx[p..crc_at];
    if pad.len() > 3 || pad.iter().any(|b| *b != 0) {
        return inconsistent(start + p as u64);
    }
    Ok(records)
}

/// One variable-length integer: seven bits a byte, little end first.
///
/// A terminating `0x00` anywhere but the first byte is a non-minimal encoding,
/// which the format forbids — `vli_decoder.c` returns `LZMA_DATA_ERROR` for it,
/// and accepting it here would make two byte strings mean one index.
///
/// Shared with [`crate::block`]: a block header's declared sizes, filter ids and
/// property lengths are the same encoding.
pub(crate) fn vli(buf: &[u8], p: &mut usize, at: u64) -> Result<u64> {
    let mut value = 0u64;
    for i in 0..VLI_BYTES_MAX {
        let Some(byte) = buf.get(*p).copied() else {
            return inconsistent(at + *p as u64);
        };
        *p += 1;
        value |= ((byte & 0x7f) as u64) << (i * 7);
        if byte & 0x80 == 0 {
            if byte == 0x00 && i > 0 {
                return inconsistent(at + *p as u64);
            }
            return Ok(value);
        }
    }
    inconsistent(at + *p as u64)
}

/// Put the backward walk's streams in file order and place every block.
fn assemble(size: u64, walked: Vec<Walked>) -> Result<SeekTable> {
    let mut streams = Vec::with_capacity(walked.len());
    let mut blocks = Vec::new();
    let mut uncompressed_offset = 0u64;

    for s in walked.into_iter().rev() {
        let first_block = blocks.len();
        let mut c = s.start + STREAM_HEADER_SIZE;
        let mut u = uncompressed_offset;
        for (unpadded, uncompressed) in &s.records {
            blocks.push(BlockEntry {
                compressed_offset: c,
                uncompressed_offset: u,
                unpadded_size: *unpadded,
                uncompressed_size: *uncompressed,
            });
            c += round4(*unpadded);
            u = match u.checked_add(*uncompressed) {
                Some(u) => u,
                None => return inconsistent(s.start),
            };
        }
        streams.push(StreamEntry {
            compressed_offset: s.start,
            uncompressed_offset,
            compressed_size: s.end - s.start,
            uncompressed_size: u - uncompressed_offset,
            check: s.check,
            padding: s.padding,
            first_block,
            block_count: s.records.len(),
        });
        uncompressed_offset = u;
    }

    Ok(SeekTable {
        compressed_file_size: size,
        streams,
        blocks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A source that counts the reads made through it, so that the window's
    /// caching is a property a test can state rather than an intention.
    struct Counting {
        bytes: Vec<u8>,
        reads: Cell<usize>,
        last_len: Cell<usize>,
    }

    impl Counting {
        fn new(len: usize) -> Counting {
            Counting {
                bytes: (0..len).map(|i| (i % 251) as u8).collect(),
                reads: Cell::new(0),
                last_len: Cell::new(0),
            }
        }
    }

    impl CompressedSource for Counting {
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reads.set(self.reads.get() + 1);
            self.last_len.set(buf.len());
            let slice: &[u8] = &self.bytes;
            slice.read_at(offset, buf)
        }

        fn size(&self) -> std::io::Result<u64> {
            Ok(self.bytes.len() as u64)
        }
    }

    #[test]
    fn the_window_refills_to_end_where_the_request_ends() {
        let src = Counting::new(100_000);
        let mut w = Backfill::new(&src, 100_000);

        // The straddle: asking for twelve bytes brings the 4084 below them.
        assert_eq!(w.bytes(50_000, 12).unwrap().len(), 12);
        assert_eq!(src.reads.get(), 1);
        assert_eq!(src.last_len.get(), WINDOW);
        assert_eq!(w.start, 50_012 - WINDOW as u64);

        // Everything already inside it is free.
        for offset in (50_012 - WINDOW as u64..50_012).step_by(4) {
            assert_eq!(
                w.bytes(offset, 4).unwrap(),
                &src.bytes[offset as usize..][..4]
            );
        }
        assert_eq!(src.reads.get(), 1);

        // A range bigger than the window is one read sized to it.
        assert_eq!(w.bytes(1_000, 9_000).unwrap().len(), 9_000);
        assert_eq!(src.reads.get(), 2);
        assert_eq!(src.last_len.get(), 9_000);
    }

    #[test]
    fn the_window_clamps_at_the_start_of_the_file_and_refuses_to_pass_its_end() {
        let src = Counting::new(1_000);
        let mut w = Backfill::new(&src, 1_000);
        assert_eq!(w.bytes(0, 12).unwrap(), &src.bytes[..12]);
        // Clamped at the start of the file: the window still ends where the
        // request ends, so there is simply less of it.
        assert_eq!(w.start, 0);
        assert_eq!(src.last_len.get(), 12);

        assert!(matches!(
            w.bytes(996, 8),
            Err(Error::Truncated {
                compressed_offset: 1_000
            })
        ));
    }

    /// The two derivations the walk makes from bytes it has read, checked
    /// against values written out by hand. The differential evidence for the
    /// walk as a whole is `tests/walk.rs`, against the `--robot` oracle.
    #[test]
    fn a_variable_length_integer_is_seven_bits_a_byte_and_must_be_minimal() {
        let mut p = 0;
        assert_eq!(vli(&[0x00], &mut p, 0).unwrap(), 0);
        assert_eq!(p, 1);

        let mut p = 0;
        assert_eq!(vli(&[0x7f], &mut p, 0).unwrap(), 127);

        let mut p = 0;
        assert_eq!(vli(&[0x80, 0x01], &mut p, 0).unwrap(), 128);
        assert_eq!(p, 2);

        let mut p = 0;
        assert_eq!(
            vli(&[0xff, 0xff, 0xff, 0xff, 0x07], &mut p, 0).unwrap(),
            (1u64 << 31) - 1
        );

        // Non-minimal: a continuation byte followed by a terminating zero.
        let mut p = 0;
        assert!(vli(&[0x80, 0x00], &mut p, 0).is_err());
        // Ten bytes is one too many.
        let mut p = 0;
        assert!(vli(&[0x80; 10], &mut p, 0).is_err());
        // Ran out of buffer.
        let mut p = 0;
        assert!(vli(&[0x80], &mut p, 0).is_err());
    }

    #[test]
    fn reserved_stream_flag_bits_are_a_damaged_file_and_a_reserved_check_is_not() {
        assert_eq!(stream_flags([0x00, 0x04], 0).unwrap(), Check::Crc64);
        assert_eq!(
            stream_flags([0x00, 0x05], 0).unwrap(),
            Check::Unsupported(5)
        );
        assert!(stream_flags([0x01, 0x04], 0).is_err());
        assert!(stream_flags([0x00, 0x14], 0).is_err());
    }

    #[test]
    fn round4_is_the_block_padding_the_index_leaves_out() {
        assert_eq!(round4(0), 0);
        assert_eq!(round4(5), 8);
        assert_eq!(round4(12), 12);
        assert_eq!(round4(13), 16);
    }

    fn put_vli(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push(0x80 | (v & 0x7f) as u8);
            v >>= 7;
        }
        out.push(v as u8);
    }

    /// One synthetic stream: a real header, index and footer around filler.
    ///
    /// The walk never looks inside a block, so the blocks can be filler — which
    /// is what makes it possible to hand-build a stream with an index larger
    /// than the window without an encoder. `xz` would reject the filler, so this
    /// stays a unit test rather than becoming a fixture.
    fn synthetic_stream(blocks: u64, unpadded: u64, uncompressed: u64) -> Vec<u8> {
        let flags = [0x00u8, 0x04];
        let mut out = Vec::new();
        out.extend_from_slice(&HEADER_MAGIC);
        out.extend_from_slice(&flags);
        out.extend_from_slice(&crate::check::crc32(&flags).to_le_bytes());
        out.resize(out.len() + (blocks * round4(unpadded)) as usize, 0xaa);

        let mut index = vec![0x00];
        put_vli(&mut index, blocks);
        for _ in 0..blocks {
            put_vli(&mut index, unpadded);
            put_vli(&mut index, uncompressed);
        }
        index.resize(round4(index.len() as u64 + 4) as usize - 4, 0x00);
        let crc = crate::check::crc32(&index);
        index.extend_from_slice(&crc.to_le_bytes());
        let backward = (index.len() / 4 - 1) as u32;
        out.extend_from_slice(&index);

        let mut footer_body = backward.to_le_bytes().to_vec();
        footer_body.extend_from_slice(&flags);
        out.extend_from_slice(&crate::check::crc32(&footer_body).to_le_bytes());
        out.extend_from_slice(&footer_body);
        out.extend_from_slice(&FOOTER_MAGIC);
        out
    }

    /// The window's fallback, end to end.
    ///
    /// No fixture reaches it: the biggest index in the corpus is a few dozen
    /// bytes, because a 4 KiB index needs on the order of 1,500 blocks. The real
    /// multiblock corpus file has ~5,700 and lands here on every walk, so the
    /// path is not exotic — it is simply too large to generate into `fixtures/`.
    #[test]
    fn an_index_larger_than_the_window_is_read_whole_and_still_tiles() {
        let one = synthetic_stream(1_500, 100, 4_096);
        let mut bytes = one.clone();
        bytes.extend_from_slice(&[0, 0, 0, 0]); // stream padding
        bytes.extend_from_slice(&one);
        let src = Counting {
            bytes,
            reads: Cell::new(0),
            last_len: Cell::new(0),
        };

        let t = walk(&src).expect("the synthetic file walks");
        assert_eq!(t.stream_count(), 2);
        assert_eq!(t.block_count(), 3_000);
        assert_eq!(t.uncompressed_size(), 3_000 * 4_096);
        assert_eq!(t.compressed_file_size, (one.len() * 2 + 4) as u64);
        assert_eq!(t.streams[0].padding, 4);
        assert_eq!(t.streams[1].padding, 0);
        assert_eq!(t.streams[0].compressed_size, one.len() as u64);
        assert_eq!(t.streams[1].first_block, 1_500);

        // The blocks tile each stream from its header onward.
        for (i, b) in t.blocks.iter().enumerate() {
            let s = &t.streams[i / 1_500];
            let n = (i % 1_500) as u64;
            assert_eq!(b.compressed_offset, s.compressed_offset + 12 + n * 100);
            assert_eq!(b.uncompressed_offset, s.uncompressed_offset + n * 4_096);
            assert_eq!(b.unpadded_size, 100);
        }

        // The magic, two tail reads and two sized index reads: still no more
        // than one read a stream beyond the ones the window cannot avoid.
        assert!(src.reads.get() <= 1 + 2 * (t.stream_count() + 1));
    }

    fn walk_bytes(bytes: Vec<u8>) -> Result<SeekTable> {
        let src: &[u8] = &bytes;
        walk(&src)
    }

    /// The tail-most stream is the only one a cut can reach.
    ///
    /// No fixture reaches these: the corpus damages the tail, which is what both
    /// a `head -c` cut and a flipped footer bit are, so the two synthetic files
    /// here are the only place the inner boundary is exercised. Building a
    /// fixture for it would mean writing a multi-stream file whose *inner*
    /// footer is wrong, which no recipe over `xz`'s own output produces.
    #[test]
    fn a_missing_footer_below_a_derived_boundary_is_not_a_truncation() {
        let one = synthetic_stream(2, 100, 4_096);
        let mut bytes = one.clone();
        bytes.extend_from_slice(&one);

        // The tail stream walks, derives its start, and the walk then looks for
        // a footer immediately below it. Break only that one's magic.
        let at = one.len() - 2;
        bytes[at..at + 2].copy_from_slice(b"XX");

        assert!(
            matches!(
                walk_bytes(bytes),
                Err(Error::IndexInconsistent {
                    compressed_offset
                }) if compressed_offset == one.len() as u64 - STREAM_FOOTER_SIZE
            ),
            "the interior of a file that reaches its own end is not cut short"
        );

        // The same damage at the file's own end *is* a cut: one rule, two
        // answers, and the only difference is which boundary it was found at.
        let mut tail = one.clone();
        let at = tail.len() - 2;
        tail[at..at + 2].copy_from_slice(b"XX");
        assert!(matches!(walk_bytes(tail), Err(Error::Truncated { .. })));
    }

    #[test]
    fn a_derived_start_with_no_room_for_a_stream_is_not_a_truncation() {
        // Eight bytes of junk carrying the file's magic, then a whole stream.
        // The tail stream's derived start is 8, so the walk continues below it
        // and finds less than a minimum stream's worth of file there.
        let mut bytes = HEADER_MAGIC.to_vec();
        bytes.extend_from_slice(&[0xaa, 0xaa]);
        bytes.extend_from_slice(&synthetic_stream(2, 100, 4_096));

        assert!(matches!(
            walk_bytes(bytes),
            Err(Error::IndexInconsistent {
                compressed_offset: 8
            })
        ));

        // A file that is *only* those eight bytes has no derived boundary below
        // it, so the same floor is the truncation it was written for.
        let mut stub = HEADER_MAGIC.to_vec();
        stub.extend_from_slice(&[0xaa, 0xaa]);
        assert!(matches!(
            walk_bytes(stub),
            Err(Error::Truncated {
                compressed_offset: 8
            })
        ));
    }
}
