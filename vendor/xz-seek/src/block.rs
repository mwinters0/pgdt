//! The block header: the twelve to twenty-four bytes that stand between a
//! block's compressed offset and its payload.
//!
//! Nothing here decodes anything. The parse turns a header into the two things
//! the decode needs — where the payload starts and what filter chain it was
//! written with — and into the one thing the memory limit needs, the
//! dictionary size the chain declares. [`encode`] runs the same layout
//! backwards, for a backend that learns a chain only by parsing a header it was
//! handed; see "The header is emitted here, not spliced from the source" below.
//!
//! ```text
//! [size byte][flags][compressed size?][uncompressed size?][filter flags…][padding][CRC32]
//! ```
//!
//! The size byte is the header's whole length over four, minus one, so the
//! header is `(byte + 1) * 4` bytes and never more than 1024. A zero there is
//! the Index Indicator, not a header: it is the byte that ends the last block
//! of a stream, and finding it where the table says a block starts means the
//! table is not describing this file.
//!
//! # Why the chain is parsed here and not kept in the table
//!
//! The seek table holds no filter chain, on purpose — see
//! `docs/design/architecture.md`, "The seek table". The
//! header is read at seek time regardless, because it is the first bytes of the
//! same range that carries the payload, so storing the chain would save no I/O
//! and make the persisted table variable-length. What the read buys instead is a
//! **detector**: the header's own CRC32 establishes that a block's
//! `compressed_offset` really points at a block header, which is the primary
//! evidence behind [`Error::IndexInconsistent`].
//!
//! # What a failure is called
//!
//! One variant comes out of this module: [`Error::IndexInconsistent`], for a
//! size byte of zero, a CRC32 that does not match, reserved flag bits, a
//! non-minimal variable-length integer, a field that runs past the header,
//! header padding that is not null, a property field whose length is not the
//! one its filter defines, an LZMA2 dictionary code the format does not assign,
//! and a declared size that contradicts the index.
//!
//! That list follows `src/walk.rs`, "What a failure is called", and for its
//! reason: a caller acts on all of them identically — this file's structure
//! disagrees with itself — and each carries the offset that says which it was.
//!
//! Two of them — reserved block-flag bits, and an LZMA2 dictionary code with
//! its reserved bits set or above the last assigned — are the *same class* as a
//! reserved filter id: well-formed bytes behind a matching CRC32 naming
//! something this version of the format does not assign. A reserved filter id
//! gets [`Error::UnsupportedFilter`] and these do not, for one reason, and it
//! is the taxonomy's rule rather than anything about the bytes: no variant can
//! carry a flag bit or a dictionary code, inventing one that could was
//! rejected, and the offset already says which byte it was. The rule is in
//! `src/error.rs`, "When a fault is 'unsupported' rather than 'damaged'".
//!
//! # This parse has no opinion about which filters can be built
//!
//! It recognises an id for exactly two things — deriving LZMA2's dictionary
//! size, which the memory limit needs before any backend object exists, and
//! validating a property field's *length*, which is a format rule and earns
//! this crate's own offset rather than an opaque decoder failure. Both stay.
//! What is not here is a list of buildable ids: the chain walk is already
//! id-agnostic, since every filter's property length is an explicit
//! variable-length integer, and "this crate cannot build it" is a statement
//! about a **backend**. It is made at the seam, in `src/backend.rs`, which is
//! also where the chain-shape rules live — the chain must end in LZMA2 and
//! every filter before it must preserve size, both left to `lzma_raw_decoder`
//! rather than copied here as a second and weaker statement of them.
//!
//! # The header is emitted here, not spliced from the source
//!
//! A backend that cannot be handed a filter chain directly has to be handed a
//! block header to parse one out of, and [`encode`] writes it. It is the
//! layout above run backwards and nothing else: no filter-specific encoding is
//! needed, because [`Filter::props`] holds each filter's property bytes exactly
//! as the header carried them.
//!
//! **It lives beside the parse rather than at the seam** — the seam names no
//! header, which is what makes it a seam (`src/backend.rs`'s module doc, first
//! paragraph) — and it emits fresh bytes rather than keeping the ones
//! [`read_header`] already read. Splicing would put "a block header" into the
//! seam's vocabulary and make `KD3`'s two source reads into something
//! [`crate::decode`] has to keep alive across it. Emitting also gets the
//! backend's own header validation for free: it re-parses the chain out of a
//! CRC32 it checked, so an error here fails loudly rather than being trusted.
//!
//! `encode` is exact rather than merely acceptable, and
//! `every_real_header_in_the_corpus_re_emits_byte_for_byte` is what says so:
//! over all 91 headers `xz` wrote into the fixture corpus, parse then re-emit
//! reproduces the original bytes. Decoding correctly is only indirect evidence
//! — a wrong header that still happens to decode is exactly the fault that
//! survives the differential sweep and surfaces on some other file.

use crate::error::{Error, Result};
use crate::source::CompressedSource;
use crate::table::{BlockEntry, Check};
use crate::walk::{inconsistent, le32, vli};

/// `(0xff + 1) * 4` — the largest header the size byte can describe.
const HEADER_SIZE_MAX: u64 = 1024;

/// Delta, whose one property byte is `distance - 1`.
const FILTER_DELTA: u64 = 0x03;
/// x86, the first branch/call/jump filter id.
const FILTER_BCJ_FIRST: u64 = 0x04;
/// RISC-V, the last one assigned.
const FILTER_BCJ_LAST: u64 = 0x0b;
/// LZMA2, whose one property byte encodes the dictionary size.
const FILTER_LZMA2: u64 = 0x21;

/// One filter of a block's chain, as the header declares it.
///
/// The properties are kept **verbatim** rather than decoded into a typed
/// options value. They are what the backend wants — `liblzma`'s `Filters` takes
/// each filter's raw property bytes — and decoding them into a shape one
/// backend happens to like is the coupling the block-payload seam exists to
/// avoid. The
/// parse still validates each property field's *length*, and derives the one
/// value this crate needs for itself: [`BlockHeader::dict_size`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Filter {
    /// The filter id, as the header's variable-length integer decoded it.
    pub id: u64,
    /// The property bytes exactly as the header carries them.
    pub props: Vec<u8>,
}

/// A block's header, parsed, and reconciled with the index entry that named it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockHeader {
    /// The header's whole length, `(size byte + 1) * 4`.
    pub header_size: u64,
    /// The Compressed Data field's length, block padding and check excluded.
    ///
    /// Always known, because the index records the block's unpadded size:
    /// `unpadded_size - header_size - check size`. Where the header declares a
    /// Compressed Size of its own, the two are compared and a disagreement is
    /// [`Error::IndexInconsistent`].
    pub payload_size: u64,
    /// Whether the header declared the Compressed Size itself.
    ///
    /// Carried because it is one of the two block flags, and the threaded
    /// encoder writes it while `-T1` does not — see `xz-invariants.md`, `I4`.
    pub declares_compressed_size: bool,
    /// Whether the header declared the Uncompressed Size itself.
    pub declares_uncompressed_size: bool,
    /// The chain, in the order the header lists it: input order, so the last
    /// filter is the one that produced the compressed bytes.
    pub filters: Vec<Filter>,
    /// The dictionary the chain declares, in bytes, and zero when it names no
    /// LZMA2 filter.
    ///
    /// This is the whole of what the *file* dictates about the decoder's
    /// footprint, and it is what the memory limit is stated in — enforced from
    /// here, before any backend object is constructed.
    pub dict_size: u64,
}

/// Read and parse the header of `block`.
///
/// One read, of the header's largest possible extent or the block's own size,
/// whichever is smaller. It stands alone: [`crate::decode`] reads the payload
/// starting past the header rather than folding these bytes into the head of
/// that range, which is `KD3` — free on a local file, a second range request
/// per block over a remote source.
pub(crate) fn read_header<S: CompressedSource>(
    source: &S,
    block: &BlockEntry,
    check: Check,
) -> Result<BlockHeader> {
    let at = block.compressed_offset;
    let want = block.unpadded_size.min(HEADER_SIZE_MAX) as usize;
    let mut buf = vec![0u8; want];
    let got = source.read_at(at, &mut buf).map_err(|e| Error::io(at, e))?;
    if got < want {
        // Every block the walk placed lies inside the file it walked, so a
        // short read here is the source having changed under us.
        return Err(Error::Truncated {
            compressed_offset: at + got as u64,
        });
    }
    parse(&buf, block, check)
}

/// Parse a block header out of bytes already in hand.
///
/// `block` is the index entry that pointed here and `check` its stream's, both
/// needed to resolve the payload's length and to hold the header's own declared
/// sizes to what the index recorded.
pub(crate) fn parse(bytes: &[u8], block: &BlockEntry, check: Check) -> Result<BlockHeader> {
    let at = block.compressed_offset;

    // A zero here is the Index Indicator: the table is pointing at the end of a
    // stream's blocks rather than at a block.
    let Some(&size_byte) = bytes.first() else {
        return inconsistent(at);
    };
    if size_byte == 0 {
        return inconsistent(at);
    }
    let header_size = (size_byte as u64 + 1) * 4;
    if (bytes.len() as u64) < header_size {
        return inconsistent(at);
    }
    // Everything but the trailing CRC32, which is what that CRC32 covers and
    // what every field below must fit inside.
    let body = &bytes[..(header_size - 4) as usize];
    if crate::check::crc32(body) != le32(&bytes[(header_size - 4) as usize..]) {
        return inconsistent(at + header_size - 4);
    }

    let flags = body[1];
    if flags & 0x3c != 0 {
        return inconsistent(at + 1);
    }
    let declares_compressed_size = flags & 0x40 != 0;
    let declares_uncompressed_size = flags & 0x80 != 0;

    let mut p = 2;
    let declared_compressed = declares_compressed_size
        .then(|| vli(body, &mut p, at))
        .transpose()?;
    let declared_uncompressed = declares_uncompressed_size
        .then(|| vli(body, &mut p, at))
        .transpose()?;

    let mut filters = Vec::new();
    let mut dict_size = 0;
    for _ in 0..(flags & 0x03) + 1 {
        let id = vli(body, &mut p, at)?;
        let props_size = vli(body, &mut p, at)? as usize;
        let Some(props) = body.get(p..p + props_size) else {
            return inconsistent(at + p as u64);
        };
        p += props_size;
        dict_size = dict_size.max(properties(id, props, at)?);
        filters.push(Filter {
            id,
            props: props.to_vec(),
        });
    }

    // Header padding: null bytes to the CRC32.
    if body[p..].iter().any(|b| *b != 0) {
        return inconsistent(at + p as u64);
    }

    // What the index left for the payload once the header and the check are
    // taken off. A block with no payload at all is not a block.
    let payload_size = match block
        .unpadded_size
        .checked_sub(header_size + check.size())
        .filter(|n| *n > 0)
    {
        Some(n) => n,
        None => return inconsistent(at),
    };
    if declared_compressed.is_some_and(|n| n != payload_size) {
        return inconsistent(at + 2);
    }
    if declared_uncompressed.is_some_and(|n| n != block.uncompressed_size) {
        return inconsistent(at + 2);
    }

    Ok(BlockHeader {
        header_size,
        payload_size,
        declares_compressed_size,
        declares_uncompressed_size,
        filters,
        dict_size,
    })
}

/// Emit a block header for `filters`, declaring each size that is `Some`.
///
/// The inverse of [`parse`], and byte-for-byte the inverse: over every header
/// `xz` wrote into the fixture corpus, parsing and re-emitting with the
/// original's declared sizes reproduces the original bytes. That holds because
/// every field the format leaves free is fixed the one way `xz` fixes it — the
/// variable-length integers are minimal, and the padding is the fewest null
/// bytes that put the header, its CRC32 included, on a multiple of four.
///
/// `compressed_size` is the Compressed Data field's length and
/// `uncompressed_size` the block's output length, each `Some` exactly when its
/// flag bit is to be set. **The `xz4rust` arm passes `Some` for both**, so that
/// the decoder enforces them and its `EndOfStream` lands on the block's last
/// byte by being checked rather than by being hoped for
/// (`docs/design/architecture.md`, "Two arms, and which one runs"). Passing the
/// *original's* flags is what the round-trip test does, and it is the only
/// caller that does.
///
/// The two are independent because the format's two flag bits are, even though
/// no fixture and no `xz` encoder mode declares exactly one: a single boolean
/// would cover the corpus and would silently stop being a byte-identity claim
/// the first time a header declared one size alone.
#[cfg(any(feature = "xz4rust", test))]
pub(crate) fn encode(
    filters: &[Filter],
    compressed_size: Option<u64>,
    uncompressed_size: Option<u64>,
) -> Vec<u8> {
    // The flags byte carries the chain length in two bits, so the format's
    // ceiling is four filters and [`parse`] never produces another count.
    debug_assert!(
        (1..=4).contains(&filters.len()),
        "a block chain is one to four filters, not {}",
        filters.len()
    );

    let mut out = vec![
        // The size byte, which is not known until the padding is in.
        0u8,
        (filters.len() as u8 - 1)
            | if compressed_size.is_some() { 0x40 } else { 0 }
            | if uncompressed_size.is_some() { 0x80 } else { 0 },
    ];
    for size in [compressed_size, uncompressed_size].into_iter().flatten() {
        push_vli(&mut out, size);
    }
    for f in filters {
        push_vli(&mut out, f.id);
        push_vli(&mut out, f.props.len() as u64);
        out.extend_from_slice(&f.props);
    }

    // Null padding to the CRC32, which is itself part of the four-byte unit the
    // size byte counts.
    while !(out.len() + 4).is_multiple_of(4) {
        out.push(0);
    }
    out[0] = ((out.len() + 4) / 4 - 1) as u8;
    out.extend_from_slice(&crate::check::crc32(&out).to_le_bytes());
    out
}

/// Append `value` as a variable-length integer, in the minimal number of bytes.
///
/// Minimality is not an optimisation: [`crate::walk::vli`] refuses a
/// non-minimal encoding, and so does `xz`.
#[cfg(any(feature = "xz4rust", test))]
pub(crate) fn push_vli(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// The dictionary size an LZMA2 property byte declares, in bytes.
///
/// `(2 | (code & 1)) << (code / 2 + 11)`, which is `lzma2_decoder.c`'s
/// `lzma_lzma2_props_decode`, with the format's own special case: code 40 means
/// `UINT32_MAX` rather than the 4 GiB the formula would give.
///
/// **One derivation, two callers.** [`properties`] turns it into
/// [`BlockHeader::dict_size`], which is what the memory limit is enforced from;
/// the `xz4rust` arm of [`crate::backend`] passes the same number to its
/// decoder as both the initial and the maximum allocation, so the backend can
/// never allocate beyond what this crate already approved. A second copy of the
/// formula in the arm would be a second thing to get wrong.
///
/// A code above 40 is not one [`properties`] admits; it saturates here rather
/// than shifting past a `u64`.
pub(crate) fn lzma2_dict_size(code: u8) -> u64 {
    if code >= 40 {
        u32::MAX as u64
    } else {
        ((2 | (code & 1)) as u64) << (code / 2 + 11)
    }
}

/// Validate one filter's property field, and return the dictionary it declares.
///
/// Only LZMA2 declares one; every other filter returns zero, and so does an id
/// this parse does not recognise — see the module docs, "This parse has no
/// opinion about which filters can be built". The lengths are the format's own
/// and are checked here rather than left to the backend, so that a malformed
/// header is this crate's error with this crate's offset rather than a decoder
/// failure two slices later.
fn properties(id: u64, props: &[u8], at: u64) -> Result<u64> {
    match id {
        FILTER_LZMA2 => {
            if props.len() != 1 {
                return inconsistent(at);
            }
            // Six bits of dictionary code; the top two are reserved, and 40 is
            // the largest assigned — `lzma2_decoder.c`, `lzma_lzma2_props_decode`.
            let code = props[0];
            if code & 0xc0 != 0 || code > 40 {
                return inconsistent(at);
            }
            Ok(lzma2_dict_size(code))
        }
        FILTER_DELTA => {
            if props.len() != 1 {
                return inconsistent(at);
            }
            Ok(0)
        }
        FILTER_BCJ_FIRST..=FILTER_BCJ_LAST => {
            // Absent, or a four-byte start offset.
            if !matches!(props.len(), 0 | 4) {
                return inconsistent(at);
            }
            Ok(0)
        }
        // An id this parse does not recognise. It declares no dictionary and
        // has no property length the format fixes, so there is nothing here to
        // say about it — whether it can be *built* is the backend's to answer,
        // at the seam, since a second backend's buildable set may differ. See
        // `src/backend.rs`, "Buildability is declared here".
        _ => Ok(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::SeekTable;

    /// The index entry a header is parsed against, with the sizes a test needs.
    fn entry(unpadded_size: u64, uncompressed_size: u64) -> BlockEntry {
        BlockEntry {
            compressed_offset: 12,
            uncompressed_offset: 0,
            unpadded_size,
            uncompressed_size,
        }
    }

    /// A header built field by field, with its size byte and CRC32 filled in.
    ///
    /// `body` is everything after the two leading bytes; the header is padded
    /// out to the next multiple of four.
    fn header(flags: u8, body: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8, flags];
        out.extend_from_slice(body);
        while (out.len() + 4) % 4 != 0 {
            out.push(0);
        }
        out[0] = ((out.len() + 4) / 4 - 1) as u8;
        let crc = crate::check::crc32(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// `--lzma2=dict=4MiB` and nothing else: one filter, no declared sizes.
    fn lzma2_header() -> Vec<u8> {
        header(0x00, &[FILTER_LZMA2 as u8, 0x01, 20])
    }

    #[test]
    fn a_plain_lzma2_header_is_twelve_bytes_and_declares_neither_size() {
        let bytes = lzma2_header();
        assert_eq!(bytes.len(), 12);
        let h = parse(&bytes, &entry(12 + 400 + 8, 1024), Check::Crc64).unwrap();
        assert_eq!(h.header_size, 12);
        assert_eq!(h.payload_size, 400);
        assert!(!h.declares_compressed_size);
        assert!(!h.declares_uncompressed_size);
        assert_eq!(h.dict_size, 4 << 20);
        assert_eq!(
            h.filters,
            vec![Filter {
                id: FILTER_LZMA2,
                props: vec![20]
            }]
        );
    }

    #[test]
    fn the_declared_sizes_are_held_to_what_the_index_recorded() {
        // Flags `cu`, a Compressed Size of 100 and an Uncompressed Size of 120,
        // which is the shape only the threaded encoder writes (`I4`).
        let bytes = header(0xc0, &[100, 120, FILTER_LZMA2 as u8, 0x01, 20]);
        let h = parse(&bytes, &entry(12 + 100 + 8, 120), Check::Crc64).unwrap();
        assert_eq!(h.header_size, 12);
        assert_eq!(h.payload_size, 100);
        assert!(h.declares_compressed_size);
        assert!(h.declares_uncompressed_size);

        // A declared compressed size the index contradicts, and a declared
        // uncompressed size it contradicts, are each the file disagreeing with
        // itself.
        assert!(matches!(
            parse(&bytes, &entry(12 + 101 + 8, 120), Check::Crc64),
            Err(Error::IndexInconsistent { .. })
        ));
        assert!(matches!(
            parse(&bytes, &entry(12 + 100 + 8, 121), Check::Crc64),
            Err(Error::IndexInconsistent { .. })
        ));
    }

    #[test]
    fn a_zero_size_byte_is_an_index_indicator_and_not_a_header() {
        let bytes = [0x00u8; 12];
        assert!(matches!(
            parse(&bytes, &entry(32, 64), Check::Crc64),
            Err(Error::IndexInconsistent {
                compressed_offset: 12
            })
        ));
    }

    #[test]
    fn a_header_is_rejected_by_its_own_crc32_before_anything_is_read_from_it() {
        let mut bytes = lzma2_header();
        bytes[2] ^= 0x01;
        assert!(matches!(
            parse(&bytes, &entry(12 + 400 + 8, 1024), Check::Crc64),
            Err(Error::IndexInconsistent {
                compressed_offset: 20
            })
        ));
    }

    #[test]
    fn reserved_flag_bits_and_non_null_padding_are_a_file_disagreeing_with_itself() {
        // Bits 2..5 of the flags byte are reserved.
        let bytes = header(0x04, &[FILTER_LZMA2 as u8, 0x01, 20]);
        assert!(matches!(
            parse(&bytes, &entry(12 + 400 + 8, 1024), Check::Crc64),
            Err(Error::IndexInconsistent {
                compressed_offset: 13
            })
        ));

        // Header padding that is not null. A plain LZMA2 header runs out at
        // byte 5 and pads to 8, so byte 5 is padding and nothing else.
        let mut bytes = lzma2_header();
        bytes[5] = 0x01;
        let crc = crate::check::crc32(&bytes[..8]);
        bytes[8..].copy_from_slice(&crc.to_le_bytes());
        assert!(matches!(
            parse(&bytes, &entry(12 + 400 + 8, 1024), Check::Crc64),
            Err(Error::IndexInconsistent {
                compressed_offset: 17
            })
        ));
    }

    #[test]
    fn a_field_that_runs_past_the_header_never_reads_the_crc32_as_data() {
        // A property length that reaches into the CRC32.
        let bytes = header(0x00, &[FILTER_LZMA2 as u8, 0x08, 22]);
        assert!(matches!(
            parse(&bytes, &entry(400, 1024), Check::Crc64),
            Err(Error::IndexInconsistent { .. })
        ));

        // A header longer than the bytes in hand.
        let mut bytes = lzma2_header();
        bytes[0] = 0x0f;
        assert!(matches!(
            parse(&bytes, &entry(400, 1024), Check::Crc64),
            Err(Error::IndexInconsistent { .. })
        ));
    }

    #[test]
    fn the_lzma2_dictionary_code_is_the_format_s_and_stops_at_forty() {
        for (code, dict) in [
            (0u8, 4u64 << 10),
            (1, 6 << 10),
            (20, 4 << 20),
            (30, 128 << 20),
            (39, 3 << 30),
            (40, u32::MAX as u64),
        ] {
            assert_eq!(
                properties(FILTER_LZMA2, &[code], 0).unwrap(),
                dict,
                "dictionary code {code}"
            );
        }
        // 41 is past the last assigned code, and the top two bits are reserved.
        assert!(properties(FILTER_LZMA2, &[41], 0).is_err());
        assert!(properties(FILTER_LZMA2, &[0xc0], 0).is_err());
        // One byte, exactly.
        assert!(properties(FILTER_LZMA2, &[], 0).is_err());
        assert!(properties(FILTER_LZMA2, &[20, 20], 0).is_err());
    }

    #[test]
    fn a_property_field_the_wrong_length_for_its_filter_is_refused() {
        assert!(properties(FILTER_DELTA, &[3], 0).is_ok());
        assert!(properties(FILTER_DELTA, &[3, 3], 0).is_err());
        for id in FILTER_BCJ_FIRST..=FILTER_BCJ_LAST {
            assert!(properties(id, &[], 0).is_ok());
            assert!(properties(id, &[0, 0, 0, 0], 0).is_ok());
            assert!(properties(id, &[0, 0], 0).is_err());
        }
    }

    /// An id outside the three the parse recognises is parsed, not judged: it
    /// declares no dictionary and the chain walk carries its properties
    /// verbatim. Whether it can be *built* is asserted at the seam, in
    /// `src/backend.rs`.
    #[test]
    fn an_unrecognised_filter_id_is_carried_rather_than_refused() {
        for id in [0x4000_0000_0000_0001u64, 0x02, 0x0c, 0x20, 0x22] {
            assert_eq!(properties(id, &[], 7).unwrap(), 0, "filter id {id:#x}");
        }
        let bytes = header(0x01, &[0x0c, 0x01, 0x77, FILTER_LZMA2 as u8, 0x01, 20]);
        let h = parse(&bytes, &entry(12 + 400 + 8, 1024), Check::Crc64).unwrap();
        assert_eq!(
            h.filters,
            vec![
                Filter {
                    id: 0x0c,
                    props: vec![0x77]
                },
                Filter {
                    id: FILTER_LZMA2,
                    props: vec![20]
                }
            ]
        );
    }

    #[test]
    fn a_block_with_no_room_for_a_payload_is_a_file_disagreeing_with_itself() {
        let bytes = lzma2_header();
        // header 12 + check 8 leaves nothing.
        assert!(matches!(
            parse(&bytes, &entry(20, 1024), Check::Crc64),
            Err(Error::IndexInconsistent { .. })
        ));
        assert!(matches!(
            parse(&bytes, &entry(5, 1024), Check::Crc64),
            Err(Error::IndexInconsistent { .. })
        ));
        // One byte of payload is enough to be a block.
        assert!(parse(&bytes, &entry(21, 1024), Check::Crc64).is_ok());
    }

    /// The two shapes [`encode`] is asked for: neither size declared, which is
    /// what `xz -T1` writes, and both declared, which is what the `xz4rust` arm
    /// always emits.
    #[test]
    fn declaring_both_sizes_lengthens_the_header_the_format_s_own_way() {
        let chain = [Filter {
            id: FILTER_LZMA2,
            props: vec![20],
        }];

        // Two bytes of preamble, three of filter, three of padding, four of
        // CRC32 — which is the twelve-byte header the corpus is mostly made of.
        let plain = encode(&chain, None, None);
        assert_eq!(plain.len(), 12);
        assert_eq!(plain, lzma2_header());

        // The same chain with both sizes declared, at `many-blocks.xz` block
        // 0's real sizes: 26,392 and 65,536 are three variable-length integer
        // bytes each, so eleven bytes of content pad to twelve and the header is
        // **sixteen** where `xz -T1` wrote twelve. That is the arithmetic the
        // `xz4rust` arm's synthesized index record has to use.
        let declared = encode(&chain, Some(26_392), Some(65_536));
        assert_eq!(declared.len(), 16);
        assert_eq!(declared[1], 0xc0);
        let h = parse(&declared, &entry(16 + 26_392 + 8, 65_536), Check::Crc64).unwrap();
        assert_eq!(h.header_size, 16);
        assert_eq!(h.payload_size, 26_392);
        assert!(h.declares_compressed_size && h.declares_uncompressed_size);
        assert_eq!(h.filters, chain);
    }

    /// Every field the encoder chooses the width of, at a width the corpus does
    /// not reach.
    #[test]
    fn the_variable_width_fields_are_encoded_minimally_and_parse_back() {
        // A four-filter chain — the format's ceiling — with a two-byte filter
        // id, a four-byte property field, and sizes needing four and five
        // variable-length integer bytes.
        let chain = vec![
            Filter {
                id: 0x0b,
                props: vec![0x00, 0x10, 0x00, 0x00],
            },
            Filter {
                id: FILTER_DELTA,
                props: vec![3],
            },
            Filter {
                id: 0x1234,
                props: vec![],
            },
            Filter {
                id: FILTER_LZMA2,
                props: vec![40],
            },
        ];
        let bytes = encode(&chain, Some(1 << 21), Some(1 << 28));
        assert_eq!(bytes[1] & 0x03, 3, "four filters is a count of three");

        let h = parse(
            &bytes,
            &entry(bytes.len() as u64 + (1 << 21) + 8, 1 << 28),
            Check::Crc64,
        )
        .unwrap();
        assert_eq!(h.header_size, bytes.len() as u64);
        assert_eq!(h.payload_size, 1 << 21);
        assert_eq!(h.filters, chain);
        assert_eq!(h.dict_size, u32::MAX as u64);

        // Minimal, in the sense `crate::walk::vli` enforces: the same value one
        // byte wider is a header the parse refuses.
        let mut wide = Vec::new();
        push_vli(&mut wide, 0);
        assert_eq!(wide, [0x00]);
        wide.clear();
        push_vli(&mut wide, 0x7f);
        assert_eq!(wide, [0x7f]);
        wide.clear();
        push_vli(&mut wide, 0x80);
        assert_eq!(wide, [0x80, 0x01]);
        wide.clear();
        push_vli(&mut wide, u64::MAX >> 1);
        assert_eq!(wide.len(), 9);
    }

    /// The direct evidence for the encoder, and the reason it is not left to the
    /// differential sweep: a wrong header that still happens to decode is
    /// exactly the fault that passes every fixture and surfaces on some other
    /// file.
    ///
    /// **All 91 headers `xz` wrote into the corpus** — the seventeen fixtures
    /// `xz --list` reads plus `corrupt-payload-check.xz` and
    /// `corrupt-payload-derail.xz`, whose flipped bits are in a payload and
    /// whose headers are therefore `xz`'s own.
    #[test]
    fn every_real_header_in_the_corpus_re_emits_byte_for_byte() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let mut headers = 0;
        let mut files = 0;
        // Both branches of the flags byte have to be reached, or the test is
        // asserting byte identity over one shape and calling it two.
        let (mut declaring, mut silent, mut grew) = (0, 0, 0);

        for f in fixtures_gen::FIXTURES
            .iter()
            .filter(|f| f.intact || f.name.starts_with("corrupt-payload-"))
        {
            let source = std::fs::File::open(dir.join(f.name)).expect("the fixture opens");
            let table = SeekTable::from_source(&source)
                .unwrap_or_else(|e| panic!("{}: the walk: {e}", f.name));
            files += 1;

            for (i, block) in table.blocks.iter().enumerate() {
                let stream = table
                    .streams
                    .iter()
                    .find(|s| i >= s.first_block && i < s.first_block + s.block_count)
                    .expect("every block belongs to a stream");
                let h = read_header(&source, block, stream.check)
                    .unwrap_or_else(|e| panic!("{} block {i}: {e}", f.name));

                let mut original = vec![0u8; h.header_size as usize];
                let got = source
                    .read_at(block.compressed_offset, &mut original)
                    .expect("the fixture reads");
                assert_eq!(got, original.len(), "{} block {i}", f.name);

                let re_emitted = encode(
                    &h.filters,
                    h.declares_compressed_size.then_some(h.payload_size),
                    h.declares_uncompressed_size
                        .then_some(block.uncompressed_size),
                );
                assert_eq!(re_emitted, original, "{} block {i}", f.name);

                // The other caller's shape, over the same real chains: both
                // sizes declared, which is what the `xz4rust` arm emits. It is
                // never shorter than what `xz` wrote and is generally longer,
                // which is the arithmetic that arm's synthesized index record
                // has to use instead of the file's own `header_size`.
                let arm = encode(
                    &h.filters,
                    Some(h.payload_size),
                    Some(block.uncompressed_size),
                );
                assert!(arm.len() >= original.len(), "{} block {i}", f.name);
                grew += usize::from(arm.len() > original.len());
                let re_parsed = parse(
                    &arm,
                    &BlockEntry {
                        unpadded_size: arm.len() as u64 + h.payload_size + stream.check.size(),
                        ..*block
                    },
                    stream.check,
                )
                .unwrap_or_else(|e| panic!("{} block {i}: the arm's header: {e}", f.name));
                assert_eq!(re_parsed.header_size, arm.len() as u64);
                assert_eq!(re_parsed.payload_size, h.payload_size);
                assert_eq!(re_parsed.filters, h.filters);
                assert!(re_parsed.declares_compressed_size && re_parsed.declares_uncompressed_size);

                if h.declares_compressed_size || h.declares_uncompressed_size {
                    declaring += 1;
                } else {
                    silent += 1;
                }
                headers += 1;
            }
        }

        // The nineteen fixtures whose indexes are intact, and the block column
        // of `harness.md`'s fixture table summed over them.
        assert_eq!(files, 19);
        assert!(headers >= 91, "{headers} headers re-emitted");
        assert!(declaring >= 4, "{declaring} headers declare a size");
        assert!(silent >= 83, "{silent} headers declare neither");
        // The four of `header-declared-sizes.xz` already declare both, so they
        // re-emit at their own length; every other header grows.
        assert_eq!(grew, headers - declaring, "{grew} headers grew");
    }

    /// `xz`'s own rendering of a filter chain, so that the parse can be compared
    /// against the listing's `filters` column as a string.
    ///
    /// This is `lzma_str_from_filters(LZMA_STR_DECODER | LZMA_STR_GETOPT_LONG)`:
    /// `--name`, space-separated, with the decoder-relevant options appended
    /// after `=` and separated by commas. Only LZMA2's `dict`, delta's `dist`
    /// and BCJ's `start` are decoder-relevant, and `start` is omitted when zero.
    ///
    /// **It reads `dist` and `start` out of [`Filter::props`] rather than out of
    /// a typed field**, which is the point: the library keeps those bytes
    /// opaque, so what agreement with `xz` establishes here is that the chain
    /// was cut out of the header at the right offsets.
    fn chain_as_xz_prints_it(h: &BlockHeader) -> String {
        let mut out = String::new();
        for f in &h.filters {
            if !out.is_empty() {
                out.push(' ');
            }
            let name = match f.id {
                FILTER_DELTA => "delta",
                0x04 => "x86",
                0x05 => "powerpc",
                0x06 => "ia64",
                0x07 => "arm",
                0x08 => "armthumb",
                0x09 => "sparc",
                0x0a => "arm64",
                0x0b => "riscv",
                FILTER_LZMA2 => "lzma2",
                other => panic!("filter {other:#x} parsed but has no name here"),
            };
            out.push_str("--");
            out.push_str(name);
            match f.id {
                FILTER_LZMA2 => {
                    out.push_str("=dict=");
                    out.push_str(&with_byte_suffix(h.dict_size as u32));
                }
                FILTER_DELTA => {
                    out.push_str(&format!("=dist={}", f.props[0] as u32 + 1));
                }
                _ => {
                    let start = if f.props.is_empty() {
                        0
                    } else {
                        u32::from_le_bytes([f.props[0], f.props[1], f.props[2], f.props[3]])
                    };
                    if start != 0 {
                        out.push_str("=start=");
                        out.push_str(&with_byte_suffix(start));
                    }
                }
            }
        }
        out
    }

    /// `str_append_u32` with `OPTMAP_USE_BYTE_SUFFIX`: divide by 1024 while it
    /// divides exactly, at most three times.
    fn with_byte_suffix(mut v: u32) -> String {
        if v == 0 {
            return "0".to_owned();
        }
        let mut suffix = 0;
        while v.is_multiple_of(1024) && suffix < 3 {
            v /= 1024;
            suffix += 1;
        }
        format!("{v}{}", ["", "KiB", "MiB", "GiB"][suffix])
    }

    /// The differential evidence for this slice: every block header in the
    /// corpus, against the four columns of `xz --list -vv --robot` that describe
    /// one.
    ///
    /// **It lives here rather than in `tests/` because the parse is internal.**
    /// The seek table does not carry a filter chain, so nothing public exposes
    /// one, and inventing a public type so that an integration test could reach
    /// it would be a public surface built for a test. `harness::oracle` still
    /// supplies the oracle side — its `BlockHeaderColumns` holds only plain
    /// integers and strings, which is what lets it cross the dev-dependency
    /// cycle's second instance of `xz_seek`.
    ///
    /// The damaged fixtures are skipped, and the two whose damage is confined to
    /// a block payload lose nothing: `corrupt-payload-check.xz` and
    /// `corrupt-payload-derail.xz` are `many-blocks.xz` with one flipped bit
    /// well past the headers, so their headers are that fixture's headers.
    #[test]
    fn every_header_in_the_corpus_is_the_one_xz_reports() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let mut blocks_checked = 0;
        let mut files_checked = 0;

        for f in fixtures_gen::FIXTURES.iter().filter(|f| f.intact) {
            let path = dir.join(f.name);
            let source = std::fs::File::open(&path).expect("the fixture opens");
            let table = SeekTable::from_source(&source)
                .unwrap_or_else(|e| panic!("{}: the walk: {e}", f.name));
            let columns = harness::oracle::block_headers_for(&path)
                .unwrap_or_else(|e| panic!("{}: the oracle: {e}", f.name));
            assert_eq!(columns.len(), table.blocks.len(), "{}", f.name);
            files_checked += 1;

            for (i, block) in table.blocks.iter().enumerate() {
                let stream = table
                    .streams
                    .iter()
                    .find(|s| i >= s.first_block && i < s.first_block + s.block_count)
                    .expect("every block belongs to a stream");
                let h = read_header(&source, block, stream.check)
                    .unwrap_or_else(|e| panic!("{} block {i}: {e}", f.name));
                let want = &columns[i];
                assert_eq!(
                    want.compressed_offset, block.compressed_offset,
                    "{}",
                    f.name
                );
                assert_eq!(h.header_size, want.header_size, "{} block {i}", f.name);
                assert_eq!(h.payload_size, want.payload_size, "{} block {i}", f.name);
                assert_eq!(
                    format!(
                        "{}{}",
                        if h.declares_compressed_size { "c" } else { "-" },
                        if h.declares_uncompressed_size {
                            "u"
                        } else {
                            "-"
                        }
                    ),
                    want.flags,
                    "{} block {i}",
                    f.name
                );
                assert_eq!(
                    chain_as_xz_prints_it(&h),
                    want.filters,
                    "{} block {i}",
                    f.name
                );
                blocks_checked += 1;
            }
        }

        // The corpus is what it was designed to be: every intact fixture walked,
        // and every one of their blocks compared.
        assert_eq!(files_checked, 17);
        assert!(blocks_checked >= 80, "{blocks_checked} blocks compared");
    }

    /// The three header shapes the corpus was built to carry, named so that a
    /// recipe change that quietly drops one fails here rather than silently
    /// narrowing the test above.
    #[test]
    fn the_corpus_still_carries_every_header_shape_the_parse_has_a_branch_for() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let first = |name: &str| {
            let source = std::fs::File::open(dir.join(name)).expect("the fixture opens");
            let table = SeekTable::from_source(&source).expect("the fixture walks");
            let block = table.blocks[0];
            let check = table.streams[0].check;
            read_header(&source, &block, check).expect("the header parses")
        };

        // No declared sizes, one filter.
        let plain = first("one-block.xz");
        assert_eq!(plain.header_size, 12);
        assert!(!plain.declares_compressed_size && !plain.declares_uncompressed_size);
        assert_eq!(plain.filters.len(), 1);

        // Both sizes declared: the `cu` branch, which only the threaded encoder
        // writes (`xz-invariants.md`, `I4`).
        let declared = first("header-declared-sizes.xz");
        assert_eq!(declared.header_size, 16);
        assert!(declared.declares_compressed_size && declared.declares_uncompressed_size);

        // Two filters, the first of them not LZMA2.
        let chained = first("filter-x86.xz");
        assert_eq!(chained.filters.len(), 2);
        assert_eq!(chained.filters[0].id, 0x04);
        assert_eq!(chained.filters[1].id, FILTER_LZMA2);
        assert_eq!(chained.dict_size, 4 << 20);
        assert_eq!(first("filter-delta.xz").filters[0].id, FILTER_DELTA);

        // The dictionary the memory limit is stated in, at both ends of the
        // range the corpus covers.
        assert_eq!(first("large-dict.xz").dict_size, 64 << 20);
        assert_eq!(first("header-declared-sizes.xz").dict_size, 1 << 20);
    }

    /// A source that is shorter than the table says is not a damaged file.
    #[test]
    fn a_short_read_at_a_block_header_is_a_truncation() {
        let bytes = lzma2_header();
        let short: &[u8] = &bytes[..8];
        assert!(matches!(
            read_header(&short, &entry(30, 0), Check::Crc64),
            Err(Error::Truncated { .. })
        ));
    }
}
