use super::huffman::{Huffman, Kind};
use super::*;
use crate::engine::{Engine, Event};
use crate::test_support::{gzip, payload};
use crate::zlib::WINDOW_SIZE;

/// Every level `flate2` writes a different mix of block kinds at.
const LEVELS: [u32; 4] = [0, 1, 6, 9];

/// What the engine reports of one member: its plaintext, each block
/// boundary's bit and plaintext offset, and the bit its deflate data begins
/// at.
fn engine_view(file: &[u8]) -> (Vec<u8>, Vec<(u64, u64)>, u64) {
    let mut engine = Engine::at_member(0, 0).unwrap();
    let (mut plaintext, mut boundaries, mut out) = (Vec::new(), Vec::new(), vec![0u8; 1 << 16]);
    let mut deflate_start = 0;
    loop {
        let at = engine.compressed_position() as usize;
        let step = engine.step(&file[at..], &mut out).unwrap();
        plaintext.extend_from_slice(&out[..step.produced]);
        match step.event {
            Some(Event::Block { bit }) => boundaries.push((bit, plaintext.len() as u64)),
            Some(Event::Header { .. }) => deflate_start = engine.compressed_position() * 8,
            _ => {}
        }
        if !step.progressed() {
            break;
        }
    }
    (plaintext, boundaries, deflate_start)
}

/// The history a native decode begins with.
enum History<'a> {
    /// Known, so written as bytes from the start.
    Known(&'a [u8]),
    /// Unknown to the decode, written as markers and resolved against this
    /// once it ends.
    Unknown(&'a [u8]),
}

/// A native decode of `file` from `bit`, a block's start, to its stream's
/// end, fed `feed` bytes at a time: the plaintext, and each boundary it
/// stopped at with its plaintext offset from `bit`.
fn native(file: &[u8], bit: u64, history: History<'_>, feed: usize) -> (Vec<u8>, Vec<(u64, u64)>) {
    let mut input = Input::new(bit / 8, file.len() as u64);
    let mut inflater = Inflater::new();
    let mut at = bit;
    let (mut marked, mut plain, resolve) = match history {
        History::Known(window) => (None, Plain::after(window.to_vec()), &[][..]),
        History::Unknown(window) => (Some(Marked::new()), Plain::new(), window),
    };
    let mut in_bytes = marked.is_none();
    let mut boundaries = Vec::new();
    loop {
        let stop = match &mut marked {
            Some(sink) if !in_bytes => inflater.run(&input, &mut at, sink, usize::MAX),
            _ => inflater.run(&input, &mut at, &mut plain, usize::MAX),
        }
        .unwrap();
        let produced = marked.as_ref().map_or(0, |m| m.out.len()) + plain.produced();
        match stop {
            Stop::Block { bit } => boundaries.push((bit, produced as u64)),
            Stop::End { .. } => break,
            Stop::Input => {
                let from = input.end() as usize;
                assert!(from < file.len(), "the decode ran out of file");
                input.push(&file[from..(from + feed).min(file.len())]);
            }
            Stop::Clean => {
                plain = Plain::after(marked.as_ref().unwrap().window());
                in_bytes = true;
            }
            Stop::Limit => unreachable!("no limit"),
        }
    }
    let offset = WINDOW_SIZE - resolve.len();
    let mut out: Vec<u8> = marked.map_or_else(Vec::new, |m| {
        m.out
            .iter()
            .map(|&v| match v.checked_sub(MARKER) {
                None => v as u8,
                Some(j) => resolve[usize::from(j) - offset],
            })
            .collect()
    });
    out.extend_from_slice(&plain.out[plain.history..]);
    (out, boundaries)
}

#[test]
fn a_whole_stream_decodes_to_the_engine_s_plaintext_and_boundaries() {
    let plaintext = payload(700_000, 3);
    for level in LEVELS {
        let file = gzip(&plaintext, level);
        let (expected, boundaries, start) = engine_view(&file);
        assert_eq!(expected, plaintext);
        for feed in [1 << 20, 4096, 97] {
            let (out, found) = native(&file, start, History::Known(&[]), feed);
            assert!(out == plaintext, "level {level}, feed {feed}");
            assert_eq!(found, boundaries, "level {level}, feed {feed}");
        }
    }
}

/// **From any block boundary, with its history unknown**, the decode's
/// markers resolve to the plaintext, and it stops where the engine does.
#[test]
fn a_decode_from_any_boundary_resolves_its_markers_against_the_window() {
    let plaintext = payload(600_000, 5);
    for level in [0, 6, 9] {
        let file = gzip(&plaintext, level);
        let (_, boundaries, _) = engine_view(&file);
        assert!(boundaries.len() > 4, "level {level}: too few blocks to test");
        for (i, &(bit, at)) in boundaries.iter().enumerate() {
            let at = at as usize;
            let window = &plaintext[at.saturating_sub(WINDOW_SIZE)..at];
            let (out, found) = native(&file, bit, History::Unknown(window), 1 << 16);
            assert!(out == plaintext[at..], "level {level}, from boundary {i}");
            let expected: Vec<_> =
                boundaries[i + 1..].iter().map(|&(b, p)| (b, p - at as u64)).collect();
            assert_eq!(found, expected, "level {level}, from boundary {i}");
        }
    }
}

/// zlib's refusals of the codes a header states.
#[test]
fn a_code_zlib_refuses_is_refused() {
    let mut table = Huffman::empty();
    assert!(table.build(&[1, 1, 1], Kind::Other).is_err(), "over-subscribed");
    assert!(table.build(&[1, 2], Kind::Other).is_err(), "incomplete");
    assert!(table.build(&[1, 0], Kind::Other).is_ok(), "a single one-bit code");
    assert!(table.build(&[1, 0], Kind::CodeLengths).is_err(), "incomplete code-length code");
    assert!(table.build(&[0, 0, 0], Kind::Other).is_ok(), "no code at all");
    assert_eq!(table.decode(0), None);
    table.build(&[2, 1, 3, 3], Kind::Other).unwrap();
    // Codes 10, 0, 110, 111, each read low bit first.
    assert_eq!(table.decode(0b01), Some((0, 2)));
    assert_eq!(table.decode(0b0), Some((1, 1)));
    assert_eq!(table.decode(0b011), Some((2, 3)));
    assert_eq!(table.decode(0b111), Some((3, 3)));
}

/// A code longer than one lookup decodes a bit at a time to the same symbol.
#[test]
fn a_long_code_decodes_past_the_root_table() {
    let mut lengths = [0u8; 20];
    // A complete code with lengths 1 through 15, and one more 15.
    for (i, len) in lengths.iter_mut().take(16).enumerate() {
        *len = (i + 1).min(15) as u8;
    }
    let mut table = Huffman::empty();
    table.build(&lengths, Kind::Other).unwrap();
    // The 15-bit codes are 111111111111110 and 111111111111111.
    let ones = (1u64 << 14) - 1;
    assert_eq!(table.decode(ones), Some((14, 15)));
    assert_eq!(table.decode(ones | 1 << 14), Some((15, 15)));
    assert_eq!(table.decode((1 << 11) - 1), Some((11, 12)));
}
