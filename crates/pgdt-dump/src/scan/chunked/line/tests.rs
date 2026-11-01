use super::*;

/// A line its chunk holds stays in the carry's buffer; the extension taking it
/// past the chunk moves it, whole, to a mapping of exactly its length, which
/// every later extension grows to exactly what it has written.
#[test]
fn a_line_moves_to_a_mapping_once_it_outgrows_its_chunk() {
    let mut line = CarriedLine::new(8);
    line.extend(b"abcd");
    line.extend(b"efgh");
    assert!(!line.is_mapped(), "a line of its chunk's length stays where it is");
    // The carry's buffer is kept for the next line.
    let buffer = line.allocated();
    line.extend(b"i");
    assert!(line.is_mapped());
    assert_eq!(line.bytes(), b"abcdefghi");
    assert_eq!(line.allocated(), buffer + 9);
    for _ in 0..100 {
        line.extend(b"0123456789");
    }
    assert_eq!(line.len(), 1009);
    assert_eq!(line.allocated(), buffer + 1009, "a mapping holds what it has written");
    assert_eq!(&line.bytes()[..9], b"abcdefghi");
    assert_eq!(&line.bytes()[999..], b"0123456789");
}

/// A mapped line is handed over as its mapping, the bytes the carry wrote,
/// and leaves the carry with nothing; a short one as its buffer.
#[test]
fn a_frozen_line_is_its_own_allocation() {
    let mut line = CarriedLine::new(4);
    line.extend(b"abc");
    let short = line.freeze();
    assert_eq!(&short[..], b"abc");
    assert!(line.is_empty());

    line.extend(b"abcdef");
    let address = line.bytes().as_ptr();
    let mapped = line.freeze();
    assert_eq!(&mapped[..], b"abcdef");
    assert_eq!(mapped.as_ptr(), address, "the mapping is handed over, not copied");
    assert!(line.is_empty() && !line.is_mapped());
}

/// The carry's buffer is reused across lines its chunk holds, and a mapped
/// line cleared leaves only that buffer.
#[test]
fn a_cleared_line_keeps_only_its_buffer() {
    let mut line = CarriedLine::new(4);
    let mut tail = b"ab".to_vec();
    line.swap_in(&mut tail);
    assert_eq!(line.bytes(), b"ab");
    line.extend(b"cdef");
    assert!(line.is_mapped());
    line.clear();
    assert!(line.is_empty() && !line.is_mapped());
    assert!(line.allocated() <= tail.capacity().max(8));
}

/// A mapping sized for its line is grown only past that size, and then to
/// exactly what it has written.
#[test]
fn a_reserved_mapping_remaps_only_past_its_size() {
    let mut mapping = LineMapping::reserving(1 << 20, b"x");
    let start = mapping.as_ref().as_ptr();
    let chunk = vec![7u8; 1 << 16];
    for _ in 0..15 {
        mapping.extend(&chunk);
    }
    assert_eq!(mapping.as_ref().as_ptr(), start, "written inside its size");
    assert_eq!(mapping.mapped(), 1 << 20);
    for _ in 0..32 {
        mapping.extend(&chunk);
    }
    assert_eq!(mapping.as_ref().len(), 1 + 47 * (1 << 16));
    assert_eq!(mapping.mapped(), mapping.as_ref().len());
    assert!(mapping.as_ref()[1..].iter().all(|&b| b == 7));
}
