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
        History::Known(window) => (None, Plain::after(window.to_vec(), usize::MAX), &[][..]),
        History::Unknown(window) => (Some(Marked::new(usize::MAX)), Plain::new(usize::MAX), window),
    };
    let mut in_bytes = marked.is_none();
    let mut boundaries = Vec::new();
    loop {
        let stop = match &mut marked {
            Some(sink) if !in_bytes => inflater.run(&input, &mut at, sink, usize::MAX),
            _ => inflater.run(&input, &mut at, &mut plain, usize::MAX),
        }
        .unwrap();
        let produced = marked.as_ref().map_or(0, Sink::produced) + plain.produced();
        match stop {
            Stop::Block { bit } => boundaries.push((bit, produced as u64)),
            Stop::End { .. } => break,
            Stop::Input => {
                let from = input.end() as usize;
                assert!(from < file.len(), "the decode ran out of file");
                input.push(&file[from..(from + feed).min(file.len())]);
            }
            Stop::Clean => {
                plain = Plain::after(marked.as_ref().unwrap().window(), usize::MAX);
                in_bytes = true;
            }
            Stop::Limit => unreachable!("no limit"),
        }
    }
    let offset = WINDOW_SIZE - resolve.len();
    let mut out: Vec<u8> = marked.map_or_else(Vec::new, |m| {
        m.values()
            .iter()
            .map(|&v| match v.checked_sub(MARKER) {
                None => v as u8,
                Some(j) => resolve[usize::from(j) - offset],
            })
            .collect()
    });
    out.extend_from_slice(plain.written());
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

/// A native decode of `file` from `bit`, a block's start, to its stream's
/// end — into `plain` where its history is known, else marked — calling
/// `at_stop` at every stop with the sink written to, the plaintext offset it
/// begins at, and the stop. With `release`, each block boundary releases all but the
/// 32 KiB before it, as a speculation releases its sinks.
fn decode_checking(
    file: &[u8],
    bit: u64,
    mut plain: Option<Plain>,
    release: bool,
    mut at_stop: impl FnMut(Either<'_>, usize, Stop),
) {
    let mut input = Input::new(bit / 8, file.len() as u64);
    input.push(&file[bit as usize / 8..]);
    let mut inflater = Inflater::new();
    let (mut at, mut marked) = (bit, Marked::new(usize::MAX));
    loop {
        let stop = match &mut plain {
            None => inflater.run(&input, &mut at, &mut marked, usize::MAX),
            Some(plain) => inflater.run(&input, &mut at, plain, usize::MAX),
        }
        .unwrap();
        let produced = marked.produced() + plain.as_ref().map_or(0, Sink::produced);
        if release && let Stop::Block { .. } = stop {
            let keep = produced.saturating_sub(WINDOW_SIZE);
            marked.release_before(keep);
            if let Some(plain) = &mut plain {
                plain.release_before(keep.saturating_sub(marked.produced()));
            }
        }
        match &plain {
            None => at_stop(Either::Marked(&marked), 0, stop),
            Some(plain) => at_stop(Either::Plain(plain), marked.produced(), stop),
        }
        match stop {
            Stop::Block { .. } => {}
            Stop::End { .. } => break,
            Stop::Clean => plain = Some(Plain::after(marked.window(), usize::MAX)),
            Stop::Input | Stop::Limit => unreachable!("the whole file, no limit"),
        }
    }
}

enum Either<'a> {
    Marked(&'a Marked),
    Plain(&'a Plain),
}

/// **A marked sink's clean mark is never before its last marker**, and at a
/// block's end it is the position after that marker, or no later than the
/// start of the last 32 KiB where they hold none: the sink comes clean only
/// once the last 32 KiB it wrote hold no marker, and at most a block later.
#[test]
fn a_marked_sink_comes_clean_within_a_block_of_its_last_marker() {
    let plaintext = payload(600_000, 5);
    for level in [1, 6, 9] {
        let file = gzip(&plaintext, level);
        let (_, boundaries, _) = engine_view(&file);
        for &(bit, _) in boundaries.iter().step_by(3) {
            decode_checking(&file, bit, None, false, |sink, _, stop| {
                let Either::Marked(sink) = sink else { return };
                let last = sink.values().iter().rposition(|&v| v >= MARKER).map_or(0, |i| i + 1);
                let (mark, window) =
                    (sink.clean_from(), sink.produced().saturating_sub(WINDOW_SIZE));
                assert!(mark >= last, "level {level}, from bit {bit}");
                if let Stop::Block { .. } = stop {
                    if last > window {
                        assert_eq!(mark, last, "level {level}, from bit {bit}");
                    } else {
                        assert!(mark <= window, "level {level}, from bit {bit}");
                    }
                }
            });
        }
    }
}

/// **A released sink holds a window, not the plaintext**, marked or in
/// bytes: a few windows and the longest block, however much it writes, and
/// still the last 32 KiB at every boundary, or all it wrote where that is
/// less, which resolve to the file's.
#[test]
fn a_released_sink_holds_a_window_and_the_last_32_kib() {
    let plaintext = payload(6_000_000, 11);
    let file = gzip(&plaintext, 6);
    let (_, boundaries, _) = engine_view(&file);
    let block = boundaries.windows(2).map(|w| w[1].1 - w[0].1).max().unwrap() as usize;
    let bound = 4 * (block + 9 * WINDOW_SIZE);
    assert!(2 * bound < plaintext.len(), "a payload too short to tell a window from it");
    let (bit, start) = *boundaries.iter().find(|&&(_, at)| at >= WINDOW_SIZE as u64).unwrap();
    let start = start as usize;
    let history = &plaintext[start - WINDOW_SIZE..start];
    for known in [false, true] {
        let plain = known.then(|| Plain::after(history.to_vec(), usize::MAX));
        let (mut held, mut stops) = (0, 0);
        decode_checking(&file, bit, plain, true, |sink, before, _| {
            let (produced, capacity, last) = match sink {
                Either::Marked(sink) => {
                    let at = sink.produced();
                    let last: Vec<u8> = (at - WINDOW_SIZE.min(at)..at)
                        .map(|i| match sink.get(i).checked_sub(MARKER) {
                            None => sink.get(i) as u8,
                            Some(j) => history[usize::from(j)],
                        })
                        .collect();
                    (at, sink.capacity(), last)
                }
                Either::Plain(sink) => {
                    let at = sink.produced();
                    let last: Vec<u8> =
                        (at - WINDOW_SIZE.min(at)..at).map(|i| sink.get(i)).collect();
                    (before + at, sink.capacity(), last)
                }
            };
            held = held.max(capacity);
            stops += 1;
            let at = start + produced;
            assert!(last[..] == plaintext[at - last.len()..at], "known {known}, at {produced}");
        });
        assert!(stops > 100, "known {known}: {stops} stops");
        assert!(held <= bound, "known {known}: held {held} of a bound {bound}");
    }
}

/// **An overlapping copy repeats its markers**, the last marker it writes
/// being its source's last one's last repetition.
#[test]
fn an_overlapping_copy_repeats_its_markers() {
    let mut sink = Marked::new(usize::MAX);
    sink.literal(b'a');
    assert!(sink.copy(3, 2), "two values from before the start");
    sink.literal(b'b');
    assert!(sink.copy(2, 7), "a marker and a byte, repeated");
    let w = WINDOW_SIZE as u16;
    let (m, b) = (MARKER + w - 1, u16::from(b'b'));
    assert_eq!(sink.values(), [u16::from(b'a'), MARKER + w - 2, m, b, m, b, m, b, m, b, m]);
    assert_eq!(sink.clean_from(), 11);
    assert!(!sink.copy(WINDOW_SIZE + 12, 3), "a copy reaching before the history");
}

/// A walk over the static block whose first symbol is at compressed `bit`,
/// fed `feed` bytes at a time as the engine feeds it, asked to stop at each
/// of `marks` in turn: each stop, and the plaintext the block holds.
fn walked(file: &[u8], bit: u64, marks: &[u64], feed: usize) -> (Vec<(u64, u64)>, u64) {
    let chunk = |at: usize| &file[at..(at + feed).min(file.len())];
    let mut walk = Walk::new(bit, 0, chunk((bit / 8) as usize));
    let (mut stops, mut marks) = (Vec::new(), marks.iter().copied());
    let mut mark = marks.next();
    loop {
        match walk.run(mark.unwrap_or(u64::MAX)).unwrap() {
            Walked::Mark { bit, plaintext } => {
                stops.push((bit, plaintext));
                mark = marks.next();
            }
            Walked::Input => {
                let at = walk.end() as usize;
                assert!(at < file.len(), "the walk ran out of file");
                walk.release();
                walk.push(chunk(at));
            }
            Walked::End => return (stops, walk.plaintext()),
        }
    }
}

/// **A walk stops where it is told, however it is fed**: over a level-1
/// member's static block, fed a byte at a time, a few, a push's worth or the
/// whole file, it stops at the same symbols — each the first at or past its
/// mark, no symbol standing for more than 258 bytes — and ends where the
/// engine says the block does.
#[test]
fn a_walk_stops_at_the_same_symbols_however_it_is_fed() {
    let plaintext = payload(300_000, 61);
    let file = gzip(&plaintext, 1);
    let (_, boundaries, start) = engine_view(&file);
    let header = file[(start / 8) as usize] >> (start % 8);
    assert_eq!(header >> 1 & 0b11, 0b01, "level 1 opens on a static block");
    let ends = boundaries.iter().map(|&(_, at)| at).find(|&at| at > 0);
    let block_end = ends.unwrap_or(plaintext.len() as u64);
    let marks: Vec<u64> = (1..).map(|i| i * 997).take_while(|&m| m < block_end - 258).collect();
    let whole = walked(&file, start + 3, &marks, file.len());
    assert_eq!(whole.1, block_end);
    assert_eq!(whole.0.len(), marks.len());
    for (&(_, at), &mark) in whole.0.iter().zip(&marks) {
        assert!((mark..mark + 258).contains(&at), "mark {mark}: stopped at {at}");
    }
    for feed in [1, 7, 4096] {
        assert_eq!(walked(&file, start + 3, &marks, feed), whole, "feed {feed}");
    }
}
