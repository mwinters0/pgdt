use super::*;

/// Whether `bytes` start on a step's boundary.
fn aligned(bytes: &[u8]) -> bool {
    (bytes.as_ptr() as usize).is_multiple_of(LineMapping::step())
}

/// A line its chunk holds stays in the carry's buffer; the extension taking it
/// past the chunk moves it, whole, to a mapping of the steps it reaches, which
/// every later extension grows to the steps it has written, its start on a
/// step's boundary wherever it moves.
#[test]
fn a_line_moves_to_a_mapping_once_it_outgrows_its_chunk() {
    let step = LineMapping::step();
    let mut line = CarriedLine::new(8);
    line.extend(b"abcd");
    line.extend(b"efgh");
    assert!(!line.is_mapped(), "a line of its chunk's length stays where it is");
    // The carry's buffer is kept for the next line.
    let buffer = line.allocated();
    line.extend(b"i");
    assert!(line.is_mapped());
    assert_eq!(line.bytes(), b"abcdefghi");
    assert_eq!(line.allocated(), buffer + step);
    assert!(aligned(line.bytes()));
    let chunk: Vec<u8> = (0..step / 3).map(|i| i as u8).collect();
    for _ in 0..10 {
        line.extend(&chunk);
        let len = line.len();
        assert_eq!(line.allocated(), buffer + len.next_multiple_of(step), "{len} bytes written");
        assert!(aligned(line.bytes()), "{len} bytes written");
    }
    assert_eq!(&line.bytes()[..9], b"abcdefghi");
    assert!(line.bytes()[9..].chunks(chunk.len()).all(|written| written == chunk));
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
/// the steps it has written.
#[test]
fn a_reserved_mapping_remaps_only_past_its_size() {
    let step = LineMapping::step();
    let size = 4 * step;
    let mut mapping = LineMapping::reserving(size, b"x");
    let start = mapping.as_ref().as_ptr();
    let chunk = vec![7u8; step / 4];
    for _ in 0..15 {
        mapping.extend(&chunk);
    }
    assert_eq!(mapping.as_ref().as_ptr(), start, "written inside its size");
    assert_eq!(mapping.mapped(), size);
    for _ in 0..8 {
        mapping.extend(&chunk);
    }
    assert_eq!(mapping.as_ref().len(), 1 + 23 * chunk.len());
    assert_eq!(mapping.mapped(), mapping.as_ref().len().next_multiple_of(step));
    assert!(mapping.as_ref()[1..].iter().all(|&b| b == 7));
}

/// **A mapping that cannot grow where it stands moves to a step's boundary**
/// with every byte it holds, and grows on from there.
#[cfg(target_os = "linux")]
#[test]
fn a_moved_mapping_keeps_its_alignment_and_its_bytes() {
    let step = LineMapping::step();
    let written: Vec<u8> = (0..step + step / 2).map(|i| (i % 251) as u8).collect();
    let mut mapping = LineMapping::reserving(0, &written);
    assert_eq!(mapping.mapped(), 2 * step);
    let before = mapping.as_ref().as_ptr();
    mapping.grow_by_moving(3 * step);
    assert_ne!(mapping.as_ref().as_ptr(), before, "moved");
    assert!(aligned(mapping.as_ref()));
    assert_eq!(mapping.mapped(), 3 * step);
    assert_eq!(mapping.as_ref(), &written[..]);
    mapping.extend(&written);
    assert!(aligned(mapping.as_ref()));
    assert_eq!(mapping.mapped(), (2 * written.len()).next_multiple_of(step));
    assert_eq!(&mapping.as_ref()[written.len()..], &written[..]);
}
