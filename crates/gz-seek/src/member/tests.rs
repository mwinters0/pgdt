use super::*;

/// A header with every optional field, its header CRC included.
fn full_header() -> Vec<u8> {
    let mut h = vec![0x1f, 0x8b, 0x08, FHCRC | FEXTRA | FNAME | FCOMMENT, 1, 2, 3, 4, 0, 3];
    h.extend_from_slice(&[5, 0, b'A', b'B', 1, 0, 9]);
    h.extend_from_slice(b"dump.sql\0");
    h.extend_from_slice(b"a comment\0");
    let crc = crate::zlib::crc32(0, &h) as u16;
    h.extend_from_slice(&crc.to_le_bytes());
    h
}

#[test]
fn a_header_split_anywhere_reads_to_its_end() {
    let header = full_header();
    for split in 0..=header.len() {
        let mut h = Header::new(0);
        let mut taken = h.read(&header[..split]).unwrap();
        assert_eq!(taken, split);
        let mut rest = header[split..].to_vec();
        rest.extend_from_slice(b"deflate data");
        taken += h.read(&rest).unwrap();
        assert!(h.is_done());
        assert_eq!(taken, header.len(), "split at {split}");
    }
}

#[test]
fn pg_dump_s_header_is_ten_bytes() {
    let mut h = Header::new(0);
    let header = [0x1f, 0x8b, 0x08, 0, 0, 0, 0, 0, 0, 3, 0xff];
    assert_eq!(h.read(&header).unwrap(), 10);
    assert!(h.is_done());
}

#[test]
fn a_header_gzip_cannot_read_is_refused() {
    let read = |bytes: &[u8]| Header::new(7).read(bytes).err();
    assert_eq!(read(b"\x1f\x8c"), Some(Error::NotGzip { offset: 7 }));
    assert_eq!(read(b"PK"), Some(Error::NotGzip { offset: 7 }));
    assert!(matches!(read(&[0x1f, 0x8b, 0x07]), Some(Error::Header { offset: 7, .. })));
    assert!(matches!(read(&[0x1f, 0x8b, 0x08, 0x20]), Some(Error::Header { offset: 7, .. })));
    let mut header = full_header();
    let n = header.len();
    header[n - 1] ^= 1;
    assert!(matches!(read(&header), Some(Error::Header { offset: 7, .. })));
}

#[test]
fn a_trailer_is_crc_then_length() {
    let t = Trailer::parse(&[1, 0, 0, 0, 2, 0, 0, 0]);
    assert_eq!(t, Trailer { crc: 1, isize: 2 });
}
