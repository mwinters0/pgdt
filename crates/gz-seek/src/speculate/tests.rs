use super::*;
use crate::test_support::{Truth, bgzf, gzip, gzip_members, payload, truth};

/// Run chunk `chunk`'s speculation over `file`, fed `feed` bytes at a time.
fn speculate(file: &[u8], chunk: u64, bytes: u64, limit: usize, feed: usize) -> Option<Guess> {
    let mut task = Speculation::new(chunk, bytes, limit, file.len() as u64);
    while let Some(at) = task.wants() {
        let at = at as usize;
        task.supply(&file[at..(at + feed).min(file.len())]);
    }
    task.finish().found
}

/// Assert `guess`, speculated over `file` from chunk boundary `from` to
/// `until`, is the file's: it begins in that range at a place a decode
/// resumes at, stops at one, its length is the plaintext's between them, and
/// its history at the stop, resolved against the file's at its start, is the
/// file's there.
fn assert_is_the_file(truth: &Truth, guess: &Guess, from: u64, until: u64, what: &str) {
    let start = match guess.start {
        Point::Block(bit) | Point::Member(bit) => bit,
        Point::End => panic!("{what}: a guess starting at the end"),
    };
    assert!(start >= from && start < until, "{what}: start {start} outside {from}..{until}");
    // The place a pass would stand at to take it.
    let point = truth
        .events
        .iter()
        .find_map(|&(_, e)| e.point().filter(|&p| guess.begins_at(p)))
        .or_else(|| {
            truth.members.iter().map(|&(b, _)| Point::Member(b)).find(|&p| guess.begins_at(p))
        })
        .unwrap_or_else(|| panic!("{what}: a false start {:?}", guess.start));
    let begins = truth.offset_of(point).unwrap();
    let ends = truth.offset_of(guess.stop).unwrap_or_else(|| panic!("{what}: a false stop"));
    assert_eq!(guess.len, ends - begins, "{what}: length");
    if let Point::Block(_) = guess.stop {
        let history = guess
            .history_at_stop(truth.window(begins))
            .unwrap_or_else(|| panic!("{what}: a history that does not resolve"));
        assert!(history[..] == *truth.window(ends), "{what}: the history at the stop");
    }
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
        let mut last: Option<(u64, Guess)> = None;
        for k in 1..=chunks {
            for feed in [1 << 20, 3000] {
                let what = format!("level {level}, chunk {k}, feed {feed}");
                let Some(guess) = speculate(&file, k, bytes, usize::MAX, feed) else { continue };
                found += 1;
                assert_is_the_file(&truth, &guess, 8 * k * bytes, 8 * (k + 1) * bytes, &what);
                let stop = match guess.stop {
                    Point::Block(bit) | Point::Member(bit) => bit,
                    Point::End => 8 * file.len() as u64,
                };
                assert!(stop >= 8 * (k + 1) * bytes || guess.stop == Point::End, "{what}");
                if feed == 3000 {
                    // A guess stops where the next chunk's search begins.
                    if let Some((_, before)) = last.take().filter(|&(j, _)| j + 1 == k) {
                        assert!(
                            guess.begins_at(before.stop),
                            "{what}: begins past the stop before"
                        );
                    }
                    last = Some((k, guess));
                }
            }
        }
        assert!(found >= chunks, "level {level}: {found} of {} found", 2 * chunks);
    }
}

/// **A chunk crosses member boundaries**, its history at the stop reaching no
/// further back than its member's start, and a BGZF file's chunks begin at
/// members' headers.
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
            let Some(guess) = speculate(&file, k, bytes, usize::MAX, 5000) else { continue };
            found += 1;
            members += usize::from(guess.fresh);
            assert_is_the_file(&truth, &guess, 8 * k * bytes, 8 * (k + 1) * bytes, &what);
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
    let limit = whole.len as usize / 2;
    let short = speculate(&file, 1, bytes, limit, 1 << 20).unwrap();
    assert_eq!(short.start, whole.start);
    assert!(short.len <= limit as u64 && short.len > 0, "{} of {limit}", short.len);
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
/// file damaged anywhere, each chunk handing back a guess or nothing.
#[test]
fn a_speculation_over_any_bytes_hands_back_a_guess_or_nothing() {
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
