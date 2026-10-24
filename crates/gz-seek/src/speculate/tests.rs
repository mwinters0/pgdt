use super::*;
use crate::engine::{Engine, Event as EngineEvent};
use crate::native::MARKER;
use crate::test_support::{bgzf, gzip, gzip_members, payload};
use crate::zlib::WINDOW_SIZE;

/// The file as the engine reads it: its plaintext, and each place a decode
/// may resume at with its plaintext offset and the event standing there, and
/// every other event, all in order.
struct Truth {
    plaintext: Vec<u8>,
    /// Each event, at its plaintext offset.
    events: Vec<(u64, Event)>,
    /// The plaintext offset each member begins at, by its bit.
    members: Vec<(u64, u64)>,
}

fn truth(file: &[u8]) -> Truth {
    let mut engine = Engine::at_member(0, 0).unwrap();
    let (mut plaintext, mut events, mut members) = (Vec::new(), Vec::new(), Vec::new());
    let mut out = vec![0u8; 1 << 16];
    let mut trailer_at = 0;
    loop {
        let at = engine.compressed_position() as usize;
        let step = engine.step(&file[at..], &mut out).unwrap();
        plaintext.extend_from_slice(&out[..step.produced]);
        let here = plaintext.len() as u64;
        match step.event {
            Some(EngineEvent::Block { bit }) => events.push((here, Event::Block { bit })),
            Some(EngineEvent::MemberStart) => {
                let offset = engine.member().offset;
                members.push((8 * offset, here));
                events.push((here, Event::MemberStart { offset }));
            }
            Some(EngineEvent::Header { bgzf }) => events.push((here, Event::Header { bgzf })),
            Some(EngineEvent::MemberEnd) => {
                let t = &file[trailer_at..trailer_at + 8];
                let trailer = Trailer::parse(t.try_into().unwrap());
                events.push((here, Event::MemberEnd { crc: trailer.crc, isize: trailer.isize }));
            }
            None => {}
        }
        if step.progressed() {
            // Where the next trailer would begin, were the stream to end here.
            trailer_at = engine.compressed_position() as usize;
        } else {
            break;
        }
    }
    Truth { plaintext, events, members }
}

impl Truth {
    /// The plaintext offset of the place `point`, if a decode resumes there.
    fn offset_of(&self, point: Point) -> Option<u64> {
        match point {
            Point::Block(bit) => {
                self.events.iter().find(|(_, e)| *e == Event::Block { bit }).map(|&(p, _)| p)
            }
            Point::Member(bit) => self.members.iter().find(|&&(b, _)| b == bit).map(|&(_, p)| p),
            Point::End => Some(self.plaintext.len() as u64),
        }
    }

    /// The window before plaintext offset `at`: the last 32 KiB of its member.
    fn window(&self, at: u64) -> &[u8] {
        let member = self.members.iter().rev().find(|&&(_, p)| p <= at).map_or(0, |&(_, p)| p);
        let from = at.saturating_sub(WINDOW_SIZE as u64).max(member);
        &self.plaintext[from as usize..at as usize]
    }
}

/// Run chunk `chunk`'s speculation over `file`, fed `feed` bytes at a time.
fn speculate(file: &[u8], chunk: u64, bytes: u64, limit: usize, feed: usize) -> Option<Chunk> {
    let mut task = Speculation::new(chunk, bytes, limit, file.len() as u64);
    while let Some(at) = task.wants() {
        let at = at as usize;
        task.supply(&file[at..(at + feed).min(file.len())]);
    }
    task.finish().found
}

/// The chunk's plaintext, its markers resolved against `window`.
fn resolved(chunk: &Chunk, window: &[u8]) -> Vec<u8> {
    let offset = WINDOW_SIZE - window.len();
    let mut out: Vec<u8> = chunk
        .marked
        .iter()
        .map(|&v| match v.checked_sub(MARKER) {
            None => v as u8,
            Some(j) => window[usize::from(j) - offset],
        })
        .collect();
    out.extend_from_slice(&chunk.plain[chunk.history..]);
    out
}

/// Assert `chunk`, speculated over `file` from chunk boundary `from` to
/// `until`, is the file's from its start to its stop.
fn assert_is_the_file(truth: &Truth, chunk: &Chunk, from: u64, until: u64, what: &str) {
    let start = match chunk.start {
        Point::Block(bit) | Point::Member(bit) => bit,
        Point::End => panic!("{what}: a chunk starting at the end"),
    };
    assert!(start >= from && start < until, "{what}: start {start} outside {from}..{until}");
    // The place a pass would stand at to take it.
    let point = truth
        .events
        .iter()
        .find_map(|&(_, e)| e.point().filter(|&p| chunk.begins_at(p)))
        .or_else(|| {
            truth.members.iter().map(|&(b, _)| Point::Member(b)).find(|&p| chunk.begins_at(p))
        })
        .unwrap_or_else(|| panic!("{what}: a false start {:?}", chunk.start));
    let begins = truth.offset_of(point).unwrap();
    let ends = truth.offset_of(chunk.stop).unwrap_or_else(|| panic!("{what}: a false stop"));
    let plaintext = resolved(chunk, truth.window(begins));
    assert!(plaintext[..] == truth.plaintext[begins as usize..ends as usize], "{what}: plaintext");
    // The events after the one standing at the start, to the stop's own.
    let first = 1 + truth.events.iter().position(|&(_, e)| e.point() == Some(point)).unwrap();
    let expected: Vec<_> = truth.events[first..first + chunk.events.len()]
        .iter()
        .map(|&(p, e)| (p - begins, e))
        .collect();
    assert_eq!(chunk.events, expected, "{what}: events");
    let crcs: Vec<u32> = {
        let from = chunk.marked.len() as u64;
        chunk
            .cuts(from, chunk.len())
            .windows(2)
            .map(|w| zlib::crc32(0, &plaintext[w[0] as usize..w[1] as usize]))
            .collect()
    };
    assert_eq!(chunk.plain_crcs, crcs, "{what}: CRC-32s");
}

/// **Each chunk is the file from where its search found a block to the
/// first boundary past the chunk**, whatever the feed.
#[test]
fn a_chunk_is_the_file_from_its_start_to_its_stop() {
    let plaintext = payload(1_500_000, 3);
    for level in [0, 6, 9] {
        let file = gzip(&plaintext, level);
        let truth = truth(&file);
        let bytes = 64 * 1024;
        let chunks = file.len() as u64 / bytes;
        let mut found = 0;
        for k in 1..=chunks {
            for feed in [1 << 20, 3000] {
                let what = format!("level {level}, chunk {k}, feed {feed}");
                let Some(chunk) = speculate(&file, k, bytes, usize::MAX, feed) else { continue };
                found += 1;
                assert_is_the_file(&truth, &chunk, 8 * k * bytes, 8 * (k + 1) * bytes, &what);
                let stop = match chunk.stop {
                    Point::Block(bit) | Point::Member(bit) => bit,
                    Point::End => 8 * file.len() as u64,
                };
                assert!(stop >= 8 * (k + 1) * bytes || chunk.stop == Point::End, "{what}");
            }
        }
        assert!(found >= chunks, "level {level}: {found} of {} found", 2 * chunks);
    }
}

/// **A chunk crosses member boundaries**, its markers ending at the first
/// member start, and a BGZF file's chunks begin at members' headers.
#[test]
fn a_chunk_crosses_members_and_begins_at_headers() {
    let (a, b) = (payload(300_000, 5), payload(200_000, 7));
    for file in
        [gzip_members(&[&a, &b, &a[..5000], &b], 6), bgzf(&a), [bgzf(&a), gzip(&b, 6)].concat()]
    {
        let truth = truth(&file);
        let bytes = 16 * 1024;
        let (mut found, mut members) = (0, 0);
        let chunks = file.len() as u64 / bytes;
        for k in 1..=chunks {
            let what = format!("chunk {k} of {}", file.len());
            // A chunk shorter than a block or a member may hold no start.
            let Some(chunk) = speculate(&file, k, bytes, usize::MAX, 5000) else { continue };
            found += 1;
            let crossed =
                chunk.events.iter().filter(|(_, e)| matches!(e, Event::MemberStart { .. }));
            members += crossed.count() + usize::from(matches!(chunk.start, Point::Member(_)));
            assert_is_the_file(&truth, &chunk, 8 * k * bytes, 8 * (k + 1) * bytes, &what);
        }
        assert!(found * 2 > chunks, "{found} of {chunks} chunks found");
        assert!(members > 0, "no chunk began at or crossed a member start");
    }
}

/// **A limit ends a chunk at its last boundary short of it**, the rest left
/// to the pass.
#[test]
fn a_limit_ends_a_chunk_at_its_last_boundary() {
    let plaintext = payload(1_000_000, 11);
    let file = gzip(&plaintext, 6);
    let truth = truth(&file);
    let bytes = 64 * 1024;
    let whole = speculate(&file, 1, bytes, usize::MAX, 1 << 20).unwrap();
    let limit = whole.len() as usize / 2;
    let short = speculate(&file, 1, bytes, limit, 1 << 20).unwrap();
    assert_eq!(short.start, whole.start);
    assert!(short.len() <= limit as u64 && short.len() > 0, "{} of {limit}", short.len());
    assert!(matches!(short.stop, Point::Block(_)));
    assert_is_the_file(&truth, &short, 8 * bytes, 16 * bytes, "limited");
    assert!(speculate(&file, 1, bytes, 10, 1 << 20).is_none(), "nothing short of a boundary");
}

/// **A chunk past the last member's start reaches the file's end.**
#[test]
fn the_last_chunk_ends_at_the_file_s_end() {
    let plaintext = payload(400_000, 13);
    let file = gzip(&plaintext, 6);
    let truth = truth(&file);
    let bytes = file.len() as u64 / 3;
    let last = speculate(&file, 2, bytes, usize::MAX, 4096).unwrap();
    assert_eq!(last.stop, Point::End);
    assert_is_the_file(&truth, &last, 16 * bytes, 24 * bytes, "the last chunk");
}

/// **A speculation reads any bytes without failing**: garbage, and a gzip
/// file damaged anywhere, each chunk handing back a chunk or nothing.
#[test]
fn a_speculation_over_any_bytes_hands_back_a_chunk_or_nothing() {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let garbage: Vec<u8> = (0..200_000).map(|_| next() as u8).collect();
    let file = gzip(&payload(300_000, 17), 6);
    let mut damaged = Vec::new();
    for i in 0..24 {
        let mut copy = file.clone();
        let at = (next() as usize) % copy.len();
        copy[at] ^= 1 << (i % 8);
        damaged.push(copy);
    }
    for bytes in std::iter::once(&garbage).chain(&damaged) {
        for chunk_bytes in [4096, 32 * 1024] {
            for k in 0..bytes.len() as u64 / chunk_bytes {
                let _ = speculate(bytes, k, chunk_bytes, 1 << 20, 3000);
            }
        }
    }
}
