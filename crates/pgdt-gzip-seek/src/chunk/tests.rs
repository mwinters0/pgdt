use super::*;
use crate::speculate::EVENTS_A_GUESS;
use crate::test_support::{
    Truth, bgzf, bgzf_member_of_empty_blocks, gzip, gzip_members, payload, truth,
};
use crate::zlib;

/// Decode from `start` to the first place at or past `until` over `file`,
/// with the file's own history at the start, fed `feed` bytes at a time.
fn decode(
    file: &[u8],
    truth: &Truth,
    start: Point,
    until: Point,
    limit: u64,
    feed: usize,
) -> Option<Decoded> {
    let window = truth.window(truth.offset_of(start).unwrap()).to_vec();
    let mut task = ChunkDecode::new((0, 0), start, window, until, limit, file.len() as u64);
    while let Some(at) = task.wants() {
        let at = at as usize;
        task.supply(&file[at..(at + feed).min(file.len())]);
    }
    task.finish().found
}

/// The places a decode resumes at, in file order.
fn points(truth: &Truth) -> Vec<Point> {
    truth.events.iter().filter_map(|&(_, e)| e.point()).collect()
}

/// `events`, which a decode begun before them reads to `until`, as it records
/// them: inside a member whose header it read as BGZF's, no block's end but
/// the stop's and no static block.
fn recorded(events: &[(u64, Event)], until: Point) -> Vec<(u64, Event)> {
    let mut bgzf = false;
    let mut kept = Vec::new();
    for &(at, event) in events {
        match event {
            Event::MemberStart { .. } => bgzf = false,
            Event::Header { bgzf: header } => bgzf = header,
            Event::Block { bit } if bgzf && bit < until.bit() => continue,
            Event::Static { .. } if bgzf => continue,
            _ => {}
        }
        kept.push((at, event));
    }
    kept
}

/// **A decode begun at a place with its history is the file's from there to
/// the first place at or past where it is told to stop**: its plaintext, its
/// events — inside a BGZF member whose header it read, none of its blocks but
/// the stop — and each piece's CRC-32, whatever the feed.
#[test]
fn a_decode_is_the_file_from_its_start_to_its_stop() {
    let (a, b) = (payload(400_000, 3), payload(150_000, 5));
    for file in
        [gzip(&a, 0), gzip(&a, 6), gzip(&a, 9), gzip_members(&[&a, &b, &b[..900]], 6), bgzf(&b)]
    {
        let truth = truth(&file);
        let points = points(&truth);
        let step = (points.len() / 7).max(1);
        for (i, &start) in points.iter().enumerate().step_by(step) {
            for &until in [points.get(i + 1), points.get(i + 3), Some(&Point::End)].iter().flatten()
            {
                let what = format!("{} bytes, {start:?} to {until:?}", file.len());
                let begins = truth.offset_of(start).unwrap();
                let ends = truth.offset_of(*until).unwrap();
                for feed in [1 << 20, 777] {
                    let decoded = decode(&file, &truth, start, *until, ends - begins, feed)
                        .unwrap_or_else(|| panic!("{what}: refused"));
                    assert_eq!(decoded.stop, *until, "{what}");
                    assert!(
                        decoded.plain[..] == truth.plaintext[begins as usize..ends as usize],
                        "{what}"
                    );
                    let first = 1 + truth
                        .events
                        .iter()
                        .position(|&(_, e)| e.point() == Some(start))
                        .unwrap();
                    let expected: Vec<_> = recorded(&truth.events[first..], *until)
                        .into_iter()
                        .take(decoded.events.len())
                        .map(|(p, e)| (p - begins, e))
                        .collect();
                    assert_eq!(decoded.events, expected, "{what}: events");
                    // The stop's event last; at the file's end, the last
                    // trailer's.
                    let &(at, e) = decoded.events.last().expect("an event");
                    let last = match until {
                        Point::End => matches!(e, Event::MemberEnd { .. }),
                        until => e.point() == Some(*until),
                    };
                    assert!(last && at == ends - begins, "{what}: the stop's event last");
                    let mut covered = 0;
                    for &(from, to, crc) in &decoded.pieces {
                        assert_eq!(from, covered, "{what}: pieces in order");
                        let bytes = &decoded.plain[from as usize..to as usize];
                        assert_eq!(crc, zlib::crc32(0, bytes), "{what}: a piece's CRC-32");
                        covered = to;
                    }
                    assert_eq!(covered, decoded.len(), "{what}: pieces cover the plaintext");
                }
            }
        }
    }
}

/// **A decode hands back nothing past its limit or short of the file's end**,
/// and **reads a trailer without comparing it**: a damaged one is the pass's
/// to refuse.
#[test]
fn a_decode_hands_back_nothing_past_its_limit_and_compares_no_trailer() {
    let plaintext = payload(300_000, 7);
    let mut file = gzip(&plaintext, 6);
    let truth = truth(&file);
    let points = points(&truth);
    let start = points[points.len() / 2];
    let begins = truth.offset_of(start).unwrap();
    let len = truth.plaintext.len() as u64 - begins;
    assert!(decode(&file, &truth, start, Point::End, len - 1, 4096).is_none(), "past its limit");
    let cut = &file[..file.len() - 3];
    assert!(decode(cut, &truth, start, Point::End, len, 4096).is_none(), "short of the end");
    let n = file.len();
    file[n - 8] ^= 1;
    let decoded =
        decode(&file, &truth, start, Point::End, len, 4096).expect("a damaged trailer read");
    let Some(&(_, Event::MemberEnd { crc, isize })) = decoded.events.last() else {
        panic!("no trailer read")
    };
    assert_eq!(isize, plaintext.len() as u32);
    assert_ne!(crc, zlib::crc32(0, &plaintext), "the damaged CRC-32, as read");
}

/// **A decode through BGZF members records three events and one piece a
/// member, whatever blocks they hold**: members of empty fixed blocks, ten
/// bits each, read from the first member's start to the file's end, record
/// each one's header and end and each start after the first, and no more
/// than [`EVENTS_A_GUESS`] for as many members as a chain read spans.
#[test]
fn a_decode_through_bgzf_members_records_only_their_starts_headers_and_ends() {
    let members = EVENTS_A_GUESS / 3;
    let file: Vec<u8> =
        (0..members).flat_map(|i| bgzf_member_of_empty_blocks(40, b'a' + (i % 26) as u8)).collect();
    let truth = truth(&file);
    let blocks = truth.events.iter().filter(|(_, e)| matches!(e, Event::Block { .. })).count();
    assert!(blocks >= 40 * members, "{blocks} blocks");
    for feed in [1 << 20, 777] {
        let decoded = decode(&file, &truth, Point::Member(0), Point::End, members as u64, feed)
            .expect("a decode");
        assert_eq!(decoded.stop, Point::End);
        assert!(decoded.plain == truth.plaintext, "feed {feed}");
        assert_eq!(decoded.events.len(), 3 * members - 1, "feed {feed}: events");
        assert!(decoded.events.len() <= EVENTS_A_GUESS, "feed {feed}");
        assert_eq!(decoded.pieces.len(), members, "feed {feed}: pieces");
        for (i, &(from, to, crc)) in decoded.pieces.iter().enumerate() {
            assert_eq!((from, to), (i as u64, i as u64 + 1), "feed {feed}: piece {i}");
            assert_eq!(crc, zlib::crc32(0, &decoded.plain[i..=i]), "feed {feed}: piece {i}");
        }
    }
}
