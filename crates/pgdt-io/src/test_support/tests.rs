//! The gzip shapes a gzip source is read against, pinned where they are
//! written: the payloads [`gzip_compress`] and [`bgzf_compress`] write with
//! encoders not ours, and the file `pg_dump -Fp --compress` writes at every
//! major: one member, written through zlib's `gzwrite` with no flush before
//! `gzclose`, its header flags and `MTIME` zero. Nothing here reads them
//! through a source; what is asserted is that each file has the shape the
//! reader built over it will assume. A regenerated fixture is not compared
//! with the committed `default.sql`, `\restrict`'s key and the `now()` column
//! moving every run (D69), so its framing and major are what is checked.

use std::io::Read;

use pgdt_fixtures::{VERSIONS, gzip_fixture};

use super::*;

/// gzip's member header flag stating an extra field (RFC 1952, §2.3.1).
const FEXTRA: u8 = 4;

/// The members of `file` and what follows the last, decoded one at a time:
/// each member's plaintext, then the bytes no member consumed.
fn members(file: &[u8]) -> (Vec<Vec<u8>>, usize) {
    let mut rest = file;
    let mut out = Vec::new();
    while !rest.is_empty() {
        let mut decoder = flate2::bufread::GzDecoder::new(rest);
        let mut plain = Vec::new();
        decoder.read_to_end(&mut plain).unwrap();
        let left = decoder.into_inner().len();
        assert!(left < rest.len(), "a member consumed nothing");
        rest = &rest[rest.len() - left..];
        out.push(plain);
    }
    (out, rest.len())
}

/// The header's flag byte and `MTIME`.
fn header(file: &[u8]) -> (u8, u32) {
    assert_eq!(&file[..3], &[0x1f, 0x8b, 8], "gzip magic and deflate");
    (file[3], u32::from_le_bytes(file[4..8].try_into().unwrap()))
}

/// **The synthetic single member is one member, its header stating
/// nothing**, which is what `pg_dump` writes (the fixture test below), so a
/// test reading it reads that shape.
#[test]
fn a_gzip_payload_is_one_member_whose_header_states_nothing() {
    let payload = gzip_test_payload();
    let file = std::fs::read(gzip_compress(&payload).path()).unwrap();
    assert_eq!(header(&file), (0, 0));
    let (members, trailing) = members(&file);
    assert_eq!(members, vec![payload]);
    assert_eq!(trailing, 0);
}

/// **A BGZF file is walked by its headers alone**: each member states its
/// whole length less one (`BSIZE`) in a `BC` subfield at bytes 12–17, so the
/// next member is found without decoding, its plaintext length is the
/// trailer's `ISIZE`, at most 64 KiB, and an empty 28-byte member ends the
/// file. The eighteen bytes are what recognizing BGZF reads.
#[test]
fn a_bgzf_payload_is_walked_by_its_members_headers() {
    let payload = gzip_test_payload();
    let file = std::fs::read(bgzf_compress(&payload).path()).unwrap();
    let (decoded, trailing) = members(&file);
    assert_eq!(trailing, 0);
    let mut at = 0;
    let mut walked = Vec::new();
    while at < file.len() {
        let member = &file[at..];
        let (flags, _) = header(member);
        assert_eq!(flags & FEXTRA, FEXTRA, "member at {at} states an extra field");
        assert_eq!(u16::from_le_bytes([member[10], member[11]]), 6, "XLEN: one subfield");
        assert_eq!(&member[12..16], b"BC\x02\x00", "the BC subfield, two bytes long");
        let size = u16::from_le_bytes([member[16], member[17]]) as usize + 1;
        let isize = u32::from_le_bytes(member[size - 4..size].try_into().unwrap());
        assert!(isize <= 65_536, "member at {at} holds {isize} bytes");
        walked.push((size, isize as usize));
        at += size;
    }
    assert_eq!(at, file.len(), "the walk ends at the file's end");
    assert_eq!(walked.len(), decoded.len());
    assert_eq!(walked.last(), Some(&(28, 0)), "the end-of-file member");
    assert!(walked.len() > 2, "several members carry the payload: {walked:?}");
    for ((_, isize), plain) in walked.iter().zip(&decoded) {
        assert_eq!(*isize, plain.len());
    }
    assert_eq!(decoded.concat(), payload);
}

/// **`pg_dump -Fp --compress` writes one gzip member at every major, its
/// header stating no name, comment or time, and holding the plain dump
/// whole** — so no member boundary is a restart point in what a user hands
/// us, and the file names nothing a reader could key on.
#[test]
fn pg_dump_s_compressed_plain_dump_is_one_member_holding_the_dump() {
    for version in VERSIONS {
        let path = gzip_fixture(version, "edge_cases", "compress");
        let file = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(header(&file), (0, 0), "{}", path.display());
        let (members, trailing) = members(&file);
        assert_eq!((members.len(), trailing), (1, 0), "{}", path.display());
        let dump = String::from_utf8(members.into_iter().next().unwrap()).unwrap();
        assert!(dump.starts_with("--\n-- PostgreSQL database dump\n"), "{}", path.display());
        assert!(
            dump.contains(&format!("\n-- Dumped by pg_dump version {version}.")),
            "{}",
            path.display()
        );
        assert!(dump.contains("\n-- PostgreSQL database dump complete\n"), "{}", path.display());
    }
}
