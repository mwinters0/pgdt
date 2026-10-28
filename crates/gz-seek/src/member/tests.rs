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

/// **A name and comment of any length are read without being held**: a
/// header whose name and comment run to 100 KiB each reads to its end split
/// anywhere among them, its header CRC compared over every byte, keeping
/// only its first [`KEPT`]; one byte of its name changed is refused.
#[test]
fn a_long_name_is_checked_and_not_held() {
    let mut header = vec![0x1f, 0x8b, 0x08, FHCRC | FNAME | FCOMMENT, 1, 2, 3, 4, 0, 3];
    header.extend((0..100 * 1024).map(|i| b'a' + (i % 26) as u8));
    header.push(0);
    header.extend(std::iter::repeat_n(b'c', 100 * 1024));
    header.push(0);
    let crc = crate::zlib::crc32(0, &header) as u16;
    header.extend_from_slice(&crc.to_le_bytes());
    let n = header.len();
    for split in [0, 9, 10, 17, 18, 5000, 100 * 1024 + 11, n - 2, n - 1, n] {
        let mut h = Header::new(0);
        let mut taken = h.read(&header[..split]).unwrap();
        taken += h.read(&header[split..]).unwrap();
        assert!(h.is_done() && taken == n, "split at {split}");
        assert_eq!(h.head(), &header[..KEPT], "split at {split}");
    }
    let mut damaged = header.clone();
    damaged[5000] ^= 1;
    assert!(matches!(Header::new(7).read(&damaged), Err(Error::Header { offset: 7, .. })));
}
