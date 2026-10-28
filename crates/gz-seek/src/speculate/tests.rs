use super::*;
use crate::test_support::{
    Bits, HEADER, Truth, bgzf, bgzf_member_of_empty_blocks, gzip, gzip_members, payload, truth,
};

/// Run chunk `chunk`'s speculation over `file`, fed `feed` bytes at a time.
fn speculate(file: &[u8], chunk: u64, bytes: u64, limit: usize, feed: usize) -> Option<Guess> {
    fed(Speculation::new(chunk, bytes, limit, file.len() as u64), file, feed)
}

/// Run `task` over `file`, fed `feed` bytes at a time.
fn fed(mut task: Speculation, file: &[u8], feed: usize) -> Option<Guess> {
    while let Some(at) = task.wants() {
        let at = at as usize;
        task.supply(&file[at..(at + feed).min(file.len())]);
    }
    task.finish().found
}

/// Chunk `chunk`'s guess over `file` under `limit`, and each continuing the
/// one before from its stop while the limit cuts it: every piece the file's,
/// each beginning where the one before stops.
fn pieces(file: &[u8], chunk: u64, bytes: u64, limit: usize, feed: usize) -> Vec<Guess> {
    let truth = truth(file);
    let mut pieces = vec![speculate(file, chunk, bytes, limit, feed).expect("a first piece")];
    while let Some(last) = pieces.last().filter(|guess| guess.cut) {
        let what = format!("piece {} of chunk {chunk}, feed {feed}", pieces.len());
        let task = Speculation::continuing(chunk, last.stop, bytes, limit, file.len() as u64);
        let next = fed(task, file, feed).unwrap_or_else(|| panic!("{what}: nothing"));
        assert_eq!(next.start, last.stop, "{what}");
        assert!(next.len <= limit as u64 && next.len > 0, "{what}: {} of {limit}", next.len);
        assert_is_the_file(&truth, &next, 8 * chunk * bytes, u64::MAX, &what);
        pieces.push(next);
    }
    pieces
}

/// Assert the pieces a limit cut a guess into are `whole`: from its start to
/// its stop, its length between, and the history at its stop.
fn assert_pieces_are(file: &[u8], pieces: &[Guess], whole: &Guess, what: &str) {
    assert!(pieces.len() > 1, "{what}: nothing cut");
    assert!(!whole.cut && pieces.iter().rev().skip(1).all(|g| g.cut), "{what}: the cuts");
    let last = pieces.last().unwrap();
    assert_eq!((pieces[0].start, last.stop, last.cut), (whole.start, whole.stop, false), "{what}");
    assert_eq!(pieces.iter().map(|g| g.len).sum::<u64>(), whole.len, "{what}: length");
    let truth = truth(file);
    let window = truth.window(truth.offset_of(whole.start).unwrap()).to_vec();
    let history = pieces.iter().try_fold(window.clone(), |w, g| g.history_at_stop(&w));
    assert_eq!(history, whole.history_at_stop(&window), "{what}: the history at the stop");
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
/// further back than its member's start, and some chunk of each file begins
/// at or crosses a member start.
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

/// **A limit cuts a chunk at its last boundary short of it, and a guess
/// continued from there reaches the chunk's stop**: the pieces chain, and
/// make up the uncut guess, its history at the stop included; a piece
/// reaching no boundary under the limit is nothing.
#[test]
fn a_limit_cuts_a_guess_continued_from_its_stop() {
    let plaintext = payload(1_000_000, 11);
    let file = gzip(&plaintext, 6);
    let truth = truth(&file);
    let bytes = 64 * 1024;
    let whole = speculate(&file, 1, bytes, usize::MAX, 1 << 20).unwrap();
    let limit = whole.len as usize / 2;
    let short = speculate(&file, 1, bytes, limit, 1 << 20).unwrap();
    assert_eq!(short.start, whole.start);
    assert!(short.cut, "a guess the limit stopped");
    assert!(short.len <= limit as u64 && short.len > 0, "{} of {limit}", short.len);
    assert!(matches!(short.stop, Point::Block(_)));
    assert_is_the_file(&truth, &short, 8 * bytes, 16 * bytes, "limited");
    assert!(speculate(&file, 1, bytes, 10, 1 << 20).is_none(), "nothing short of a boundary");
    for feed in [3000, 1 << 20] {
        let pieces = pieces(&file, 1, bytes, limit, feed);
        assert_pieces_are(&file, &pieces, &whole, &format!("feed {feed}"));
    }
    let task = Speculation::continuing(1, short.stop, bytes, 10, file.len() as u64);
    assert!(fed(task, &file, 1 << 20).is_none(), "a continuation short of a boundary");

    // Members each a block long: every cut is at a plain member's start, and
    // a continuation decodes from that member's header.
    let parts = plaintext.chunks(20_000).take(40).collect::<Vec<_>>();
    let file = gzip_members(&parts, 6);
    let whole = speculate(&file, 1, bytes, usize::MAX, 1 << 20).unwrap();
    let limit = whole.len as usize / 3;
    for feed in [3000, 1 << 20] {
        let pieces = pieces(&file, 1, bytes, limit, feed);
        assert!(pieces[1..].iter().all(|g| matches!(g.start, Point::Member(_))), "feed {feed}");
        assert_pieces_are(&file, &pieces, &whole, &format!("members, feed {feed}"));
    }
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

/// Each member start of `file`, in bytes, and whether its header is BGZF's.
fn member_starts(truth: &Truth, file: &[u8]) -> Vec<(u64, bool)> {
    let bgzf = |at: u64| crate::bgzf::bgzf_member_len(&file[at as usize..]).is_some();
    truth.members.iter().map(|&(bit, _)| (bit / 8, bgzf(bit / 8))).collect()
}

/// The guess a chunk from byte `from` to `until` holding a BGZF member start
/// is read as: from the first at or past `from` to the first member start
/// after it at or past `until`, or whose header is not BGZF's, or the end.
fn read_off_headers(starts: &[(u64, bool)], from: u64, until: u64) -> Option<(Point, Point)> {
    let &(start, _) = starts.iter().find(|&&(at, bgzf)| bgzf && at >= from && at < until)?;
    let stop = starts
        .iter()
        .find(|&&(at, bgzf)| at > start && (at >= until || !bgzf))
        .map_or(Point::End, |&(at, _)| Point::Member(8 * at));
    Some((Point::Member(8 * start), stop))
}

/// `file` with every BGZF member's deflate data overwritten, its headers and
/// trailers kept: bytes no decode reads.
fn garbled(file: &[u8], starts: &[(u64, bool)]) -> Vec<u8> {
    let mut garbled = file.to_vec();
    for &(at, bgzf) in starts {
        if bgzf {
            let at = at as usize;
            let len = crate::bgzf::bgzf_member_len(&file[at..]).unwrap() as usize;
            garbled[at + crate::bgzf::BGZF_HEADER_LEN..at + len - 8].fill(0xff);
        }
    }
    garbled
}

/// **A BGZF chunk's guess is read off its members' headers**, nothing
/// decoded: from the first BGZF member at or past the chunk's first byte to
/// the first member start at or past its end — or, after it, a plain
/// member's — or the file's end, the plaintext's length between them, and no
/// history; the same with every member's deflate data overwritten, which no
/// decode could read; and each guess beginning where the one before stops.
#[test]
fn a_bgzf_chunk_is_read_off_its_members_headers() {
    let (a, b) = (payload(1_200_000, 23), payload(300_000, 29));
    for (name, file) in [("bgzf", bgzf(&a)), ("mixed", [bgzf(&a), gzip(&b, 6), bgzf(&b)].concat())]
    {
        let truth = truth(&file);
        let starts = member_starts(&truth, &file);
        let garbled = garbled(&file, &starts);
        for bytes in [64 * 1024, 150 * 1024] {
            let (chunks, mut read) = (file.len() as u64 / bytes, 0);
            let mut last: Option<(u64, Guess)> = None;
            for k in 1..=chunks {
                let (from, until) = (k * bytes, (k + 1) * bytes);
                let Some((start, stop)) = read_off_headers(&starts, from, until) else { continue };
                for feed in [5000, 1 << 20] {
                    let what = format!("{name}, chunk {k} of {bytes}, feed {feed}");
                    let guess = speculate(&file, k, bytes, usize::MAX, feed).unwrap();
                    assert_eq!((guess.start, guess.stop), (start, stop), "{what}");
                    assert!(guess.fresh && guess.end.is_empty(), "{what}: a history");
                    assert_is_the_file(&truth, &guess, 8 * from, 8 * until, &what);
                    let blind = speculate(&garbled, k, bytes, usize::MAX, feed).unwrap();
                    let seen = (blind.start, blind.stop, blind.len);
                    assert_eq!(seen, (start, stop, guess.len), "{what}: decoded");
                }
                let guess = speculate(&file, k, bytes, usize::MAX, 7000).unwrap();
                // Where the guess before was not cut short of this chunk, at a
                // plain member, this one begins at its stop.
                let reaches = |stop: Point| matches!(stop, Point::Member(bit) if bit >= 8 * from);
                if let Some((_, before)) =
                    last.take().filter(|(j, g)| j + 1 == k && reaches(g.stop))
                {
                    assert!(guess.begins_at(before.stop), "{name}, chunk {k}: past the stop");
                }
                last = Some((k, guess));
                read += 1;
            }
            // A chunk at least a member long holds a member start.
            let all = if name == "bgzf" { chunks } else { 2 };
            assert!(read >= all, "{name} at {bytes}: {read} of {chunks} chunks read off headers");
        }
    }
}

/// **A BGZF chunk's chain is cut where it cannot be read on**: at the limit,
/// at the last member start before the plaintext would pass it, a guess
/// continued from there being read off the headers on to the chunk's stop,
/// nothing where the first member alone does; and at a member whose stated
/// length leads
/// to no member, at that member's start — inside the chunk, and the member
/// reaching past its end, whose length no stop before it bears out.
#[test]
fn a_bgzf_chunk_s_chain_is_cut_where_it_cannot_be_read_on() {
    let file = bgzf(&payload(1_200_000, 31));
    let truth = truth(&file);
    let starts = member_starts(&truth, &file);
    let bytes = 200 * 1024;
    let whole = speculate(&file, 1, bytes, usize::MAX, 1 << 20).unwrap();
    let limit = whole.len as usize / 2;
    let short = speculate(&file, 1, bytes, limit, 1 << 20).unwrap();
    assert_eq!(short.start, whole.start);
    assert!(short.cut, "a chain the limit stopped");
    assert!(short.len <= limit as u64 && short.len > 0, "{} of {limit}", short.len);
    assert!(matches!(short.stop, Point::Member(_)), "{:?}", short.stop);
    assert_is_the_file(&truth, &short, 8 * bytes, 16 * bytes, "limited");
    assert!(speculate(&file, 1, bytes, 10, 1 << 20).is_none(), "nothing short of a member");
    // Continued from its stop, read off the headers there, to the chunk's.
    for feed in [5000, 1 << 20] {
        let pieces = pieces(&file, 1, bytes, limit, feed);
        assert!(pieces.iter().all(|g| g.end.is_empty()), "feed {feed}: a decoded piece");
        assert_pieces_are(&file, &pieces, &whole, &format!("bgzf, feed {feed}"));
    }

    // The third member of the chunk, and the member reaching past its end,
    // each state a length landing inside the member after it, on an `ISIZE`
    // BGZF allows.
    let Point::Member(first) = whole.start else { panic!("{:?}", whole.start) };
    let Point::Member(stop) = whole.stop else { panic!("{:?}", whole.stop) };
    let i = starts.iter().position(|&(at, _)| 8 * at == first).unwrap();
    let reaching = starts.iter().position(|&(at, _)| 8 * at == stop).unwrap() - 1;
    assert!(reaching > i + 2, "the chunk holds {} members", reaching - i + 1);
    for (j, what) in [(i + 2, "inside the chunk"), (reaching, "reaching past its end")] {
        let at = starts[j].0 as usize;
        let mut lying = file.clone();
        let bsize = u16::from_le_bytes([file[at + 16], file[at + 17]]).checked_add(200).unwrap();
        lying[at + 16..at + 18].copy_from_slice(&bsize.to_le_bytes());
        let next = at + usize::from(bsize) + 1;
        assert!(next > starts[j + 1].0 as usize + 18 && next < starts[j + 2].0 as usize);
        lying[next - 4..next].copy_from_slice(&1000u32.to_le_bytes());
        assert!(!lying[next..].starts_with(&crate::member::GZIP_MAGIC));
        for feed in [5000, 1 << 20] {
            let cut = speculate(&lying, 1, bytes, usize::MAX, feed).unwrap();
            let seen = (cut.start, cut.stop);
            assert_eq!(seen, (whole.start, Point::Member(8 * at as u64)), "{what}, feed {feed}");
            let what = format!("cut at a lying length {what}");
            assert_is_the_file(&truth, &cut, 8 * bytes, 16 * bytes, &what);
        }
    }
}

/// **A lone BGZF header the bytes do not bear out is searched past**: one
/// near a chunk's end, its stated length reaching past it onto no member and
/// its `ISIZE` one BGZF allows, written as a stored block's plaintext so the
/// file is sound, is no chain's start, and the chunk is searched as any
/// file's is.
#[test]
fn a_lone_bgzf_header_nothing_bears_out_is_searched_past() {
    let (bsize, isize) = (1000u16, 4000u32);
    let mut header = bgzf(b"lone")[..crate::bgzf::BGZF_HEADER_LEN].to_vec();
    header[16..18].copy_from_slice(&(bsize - 1).to_le_bytes());
    let mut plaintext = payload(600_000, 37);
    let at = 300_000;
    plaintext[at..at + header.len()].copy_from_slice(&header);
    let trailer = at + usize::from(bsize) - 4;
    plaintext[trailer..trailer + 4].copy_from_slice(&isize.to_le_bytes());
    let file = gzip(&plaintext, 0);
    let truth = truth(&file);
    let lone = file.windows(header.len()).position(|w| w == header).unwrap() as u64;
    let next = (lone + u64::from(bsize)) as usize;
    assert!(!file[next..].starts_with(&crate::member::GZIP_MAGIC), "a member follows it");
    // Chunk 1 ends within the header's stated length past it.
    let bytes = (lone + 101) / 2;
    assert!(lone >= bytes && 2 * bytes < lone + u64::from(bsize), "{lone} in chunk 1 of {bytes}");
    for feed in [5000, 1 << 20] {
        let what = format!("feed {feed}");
        let guess = speculate(&file, 1, bytes, usize::MAX, feed).expect("a stored block found");
        assert_ne!(guess.start, Point::Member(8 * lone), "{what}: the lone header taken");
        assert_is_the_file(&truth, &guess, 8 * bytes, 16 * bytes, &what);
    }
}

/// **The search for a header's first byte finds it where a byte-by-byte one
/// does**, at every place in and around a chunk, and in a slice shorter than
/// one.
#[test]
fn the_first_byte_is_found_where_a_byte_by_byte_search_finds_it() {
    let bytes: Vec<u8> = (0..200u32).map(|i| (i * 7 % 31) as u8).collect();
    for at in [None, Some(0), Some(1), Some(31), Some(32), Some(33), Some(63), Some(150), Some(199)]
    {
        let mut bytes = bytes.clone();
        bytes.iter_mut().filter(|b| **b == 0x1f).for_each(|b| *b = 0);
        if let Some(at) = at {
            bytes[at] = 0x1f;
            bytes[199] = 0x1f;
        }
        for len in [0, 5, 32, 64, 100, 200] {
            let slice = &bytes[..len];
            let expected = slice.iter().position(|&b| b == 0x1f);
            assert_eq!(members::position_of(slice, 0x1f), expected, "at {at:?}, len {len}");
        }
    }
}

/// One member: `blocks` empty fixed blocks — no plaintext for a limit to
/// count — a sync flush, and `after` written by `flate2` over zlib-rs; and
/// the bit the flush's stored block begins at.
fn empty_blocks_then(blocks: usize, after: &[u8]) -> (Vec<u8>, u64) {
    use flate2::{Compress, Compression, FlushCompress};
    let mut bits = Bits::after(HEADER.to_vec());
    for _ in 0..blocks {
        bits.fixed();
        bits.end_of_block();
    }
    let stored = bits.bit;
    bits.sync();
    let mut file = bits.bytes;
    let mut deflate = Compress::new(Compression::new(6), false);
    let mut body = Vec::with_capacity(after.len() + 1024);
    deflate.compress_vec(after, &mut body, FlushCompress::Finish).unwrap();
    file.extend_from_slice(&body);
    let mut crc = flate2::Crc::new();
    crc.update(after);
    file.extend_from_slice(&crc.sum().to_le_bytes());
    file.extend_from_slice(&crc.amount().to_le_bytes());
    (file, stored)
}

/// Chunk `chunk`'s guess over `file` under `limit`, and each continuing the
/// one before from its stop while a bound cuts it, fed `feed` bytes at a
/// time: every piece the file's, each beginning where the one before stops,
/// and no speculation's input more than a feed past its bound.
fn chained(file: &[u8], chunk: u64, bytes: u64, limit: usize, feed: usize) -> Vec<Guess> {
    let truth = truth(file);
    let mut task = Speculation::new(chunk, bytes, limit, file.len() as u64);
    let mut pieces: Vec<Guess> = Vec::new();
    loop {
        let what = format!("piece {} of chunk {chunk}, feed {feed}", pieces.len());
        let guess =
            fed_bounded(task, file, feed, &what).unwrap_or_else(|| panic!("{what}: nothing"));
        assert_is_the_file(&truth, &guess, 8 * chunk * bytes, u64::MAX, &what);
        if let Some(last) = pieces.last() {
            assert_eq!(guess.start, last.stop, "{what}");
        }
        let (cut, stop) = (guess.cut, guess.stop);
        pieces.push(guess);
        if !cut {
            return pieces;
        }
        task = Speculation::continuing(chunk, stop, bytes, limit, file.len() as u64);
    }
}

/// How many blocks of `truth` end after `guess`'s start and no later than
/// its stop.
fn blocks_spanned(truth: &Truth, guess: &Guess) -> usize {
    let (start, stop) = (guess.start.bit(), guess.stop.bit());
    let ends = |e: &Event| matches!(*e, Event::Block { bit } if bit > start && bit <= stop);
    truth.events.iter().filter(|(_, e)| ends(e)).count()
}

/// **A run of empty blocks inside a chunk is cut at [`EVENTS_A_GUESS`]**:
/// the guess stops at its last boundary short of the count, inside the
/// chunk, and the one continuing it reaches the chunk's stop, no piece
/// spanning more blocks than the count.
#[test]
fn a_run_of_empty_blocks_inside_a_chunk_is_cut_at_the_event_bound() {
    let blocks = 3 * EVENTS_A_GUESS / 2;
    // Ten bits a block, each an event of `truth`'s: a run a test can hold.
    assert!(blocks < 1 << 16, "{blocks} blocks");
    let (file, stored) = empty_blocks_then(blocks, &payload(300_000, 41));
    let truth = truth(&file);
    // The chunk holds the run and the flush, and ends inside `after`.
    let bytes = stored / 8 + 20_000;
    for feed in [3000, 1 << 20] {
        let pieces = chained(&file, 0, bytes, usize::MAX, feed);
        assert!(pieces.len() >= 2, "feed {feed}: nothing cut");
        assert!(pieces[0].stop.bit() < 8 * bytes, "feed {feed}: cut past the chunk's end");
        assert!(blocks_spanned(&truth, &pieces[0]) >= EVENTS_A_GUESS - 2, "feed {feed}");
        for (i, piece) in pieces.iter().enumerate() {
            let spanned = blocks_spanned(&truth, piece);
            assert!(spanned <= EVENTS_A_GUESS, "feed {feed}, piece {i}: {spanned} blocks");
        }
        assert!(pieces.last().unwrap().stop.bit() >= 8 * bytes, "feed {feed}: short of the end");
    }
}

/// **A run of empty blocks past a chunk's end is cut at the limit's worth of
/// compressed bytes**, each piece continued from its stop, to the flush the
/// next chunk's search would begin at: each piece's bits past the chunk's
/// end, or past its own start beyond it, the limit's worth and at most one
/// block more.
#[test]
fn a_run_of_empty_blocks_past_a_chunk_s_end_is_cut_at_the_limit_s_worth() {
    let limit = 8 * 1024;
    let bytes = HEADER.len() as u64 + 1000;
    // Past the chunk's end, three and a half limits' worth of ten-bit blocks.
    let blocks = (8 * (1000 + 7 * limit / 2)) / 10;
    assert!(blocks < 2 * EVENTS_A_GUESS && 8 * limit / 10 < EVENTS_A_GUESS);
    let (file, stored) = empty_blocks_then(blocks, &payload(100_000, 43));
    let until = 8 * bytes;
    for feed in [3000, 1 << 20] {
        let pieces = chained(&file, 0, bytes, limit, feed);
        assert!(pieces.len() >= 4, "feed {feed}: {} pieces", pieces.len());
        for (i, piece) in pieces.iter().enumerate().rev().skip(1) {
            let past = piece.stop.bit() - piece.start.bit().max(until);
            let worth = 8 * limit as u64;
            assert!(past >= worth && past <= worth + 10, "feed {feed}, piece {i}: {past} bits");
        }
        assert_eq!(pieces.last().unwrap().stop, Point::Block(stored), "feed {feed}");
    }
}

/// How many members of `truth` begin after `guess`'s start and no later than
/// its stop.
fn members_spanned(truth: &Truth, guess: &Guess) -> usize {
    let (start, stop) = (guess.start.bit(), guess.stop.bit());
    truth.members.iter().filter(|&&(bit, _)| bit > start && bit <= stop).count()
}

/// **A BGZF chunk's chain is cut at [`EVENTS_A_GUESS`]**, three events a
/// member: a chunk of more one-byte members than that allows, each of empty
/// fixed blocks, is read off its headers in pieces each spanning at most a
/// third of the count, the first exactly that, continued to the chunk's stop.
#[test]
fn a_bgzf_chain_is_cut_at_the_event_bound() {
    let members = EVENTS_A_GUESS;
    let file: Vec<u8> =
        (0..members).flat_map(|i| bgzf_member_of_empty_blocks(4, b'a' + (i % 26) as u8)).collect();
    let truth = truth(&file);
    let bytes = file.len() as u64 - 1000;
    for feed in [3000, 1 << 20] {
        let pieces = chained(&file, 0, bytes, usize::MAX, feed);
        assert!(pieces.len() >= 3, "feed {feed}: {} pieces", pieces.len());
        assert!(pieces.iter().all(|g| g.end.is_empty()), "feed {feed}: a decoded piece");
        assert_eq!(members_spanned(&truth, &pieces[0]), EVENTS_A_GUESS / 3, "feed {feed}");
        for (i, piece) in pieces.iter().enumerate() {
            let spanned = members_spanned(&truth, piece);
            assert!(3 * spanned <= EVENTS_A_GUESS, "feed {feed}, piece {i}: {spanned} members");
            assert_eq!(piece.len, spanned as u64, "feed {feed}, piece {i}: one byte a member");
        }
        let last = pieces.last().unwrap();
        assert!(!last.cut && last.stop.bit() >= 8 * bytes, "feed {feed}: short of the stop");
    }
}

/// **A member's name is read no further than the compressed bound**: a
/// member beginning inside a chunk, its name running the limit's worth past
/// the chunk's end and more, cuts the guess at its start, the input held no
/// more than a feed past the bound; the continuation from there, meeting the
/// same name, hands back nothing, its input bounded alike, so the frontier
/// decodes it.
#[test]
fn a_name_past_the_compressed_bound_cuts_the_guess_at_its_member() {
    let limit = 64 * 1024;
    let first = gzip(&payload(50_000, 47), 6);
    let mut named = HEADER.to_vec();
    named[3] = 0x08;
    named.extend(std::iter::repeat_n(b'n', 3 * limit));
    named.push(0);
    let body = gzip(&payload(20_000, 53), 6);
    named.extend_from_slice(&body[HEADER.len()..]);
    let file = [first.clone(), named].concat();
    let start = first.len() as u64;
    let bytes = start + 100;
    for feed in [3000, 1 << 20] {
        let what = format!("feed {feed}");
        let task = Speculation::new(0, bytes, limit, file.len() as u64);
        let guess = fed_bounded(task, &file, feed, &what).unwrap_or_else(|| panic!("{what}"));
        let seen = (guess.start, guess.stop, guess.cut);
        assert_eq!(seen, (Point::Member(0), Point::Member(8 * start), true), "{what}");
        let task = Speculation::continuing(0, guess.stop, bytes, limit, file.len() as u64);
        assert!(fed_bounded(task, &file, feed, &what).is_none(), "{what}: a guess through it");
    }
}

/// Run `task` over `file`, fed `feed` bytes at a time, its input never more
/// than a feed past its bound.
fn fed_bounded(mut task: Speculation, file: &[u8], feed: usize, what: &str) -> Option<Guess> {
    let bound = task.bound;
    while let Some(at) = task.wants() {
        let at = at as usize;
        task.supply(&file[at..(at + feed).min(file.len())]);
        let end = task.input.end();
        assert!(end <= bound / 8 + 8 + feed as u64, "{what}: input to {end}, bound {bound}");
    }
    task.finish().found
}

/// **A marked run grows its sink no further than the limit asks**: a run of
/// copies from before the decode's start, every value a marker and no
/// boundary in it, past `2^24` values under [`SPECULATION_LIMIT`], so the
/// sink grows from `2^24` holding more than half of it — to the limit's room,
/// not to `2^25`.
#[test]
fn a_marked_run_grows_its_sink_no_further_than_the_limit_asks() {
    let mut bits = Bits::after(HEADER.to_vec());
    bits.fixed();
    bits.literal(b'a');
    bits.end_of_block();
    let start = bits.bit;
    bits.fixed();
    for _ in 0..SPECULATION_LIMIT / 258 + 64 {
        bits.copy_258_from_1();
    }
    bits.end_of_block();
    bits.sync();
    let file = bits.bytes;
    let len = file.len() as u64;
    let mut task = Speculation::continuing(0, Point::Block(start), len, SPECULATION_LIMIT, len);
    let (mut most, mut at_2_24) = (0, false);
    while let Some(at) = task.wants() {
        let at = at as usize;
        task.supply(&file[at..(at + 4096).min(file.len())]);
        if let State::Decoding(decode) = &task.state {
            assert!(decode.is_marked(), "a run that came clean");
            let capacity = decode.marked.as_ref().expect("marked").capacity();
            most = most.max(capacity);
            at_2_24 |= capacity == 1 << 24;
        }
    }
    assert!(at_2_24, "the sink never held 2^24 values");
    assert!(most > 1 << 24 && most <= SPECULATION_LIMIT + 258 + 32, "grew to {most}");
    assert!(task.finish().found.is_none(), "a run the limit ended with no boundary");
}

/// **A speculation's two sinks fill together**, so what it holds is bounded
/// by both rooms, not the larger: a marked run past half the limit, its
/// member's end, and a member whose one block runs on to the limit, written
/// into the plain sink made at the decode's start. Its input and sinks never
/// pass [`most_held`], and reach most of it.
#[test]
fn a_speculation_s_sinks_fill_together_within_its_bound() {
    let mut bits = Bits::after(HEADER.to_vec());
    bits.fixed();
    bits.literal(b'a');
    bits.end_of_block();
    let start = bits.bit;
    bits.last_fixed();
    for _ in 0..SPECULATION_LIMIT / 2 / 258 + 64 {
        bits.copy_258_from_1();
    }
    bits.end_of_block();
    let mut file = bits.bytes;
    // The trailer, which a speculation reads unchecked.
    file.extend_from_slice(&[0; 8]);
    let mut bits = Bits::after([file, HEADER.to_vec()].concat());
    bits.last_fixed();
    bits.literal(b'b');
    for _ in 0..SPECULATION_LIMIT / 2 / 258 {
        bits.copy_258_from_1();
    }
    bits.end_of_block();
    let file = bits.bytes;
    let len = file.len() as u64;
    // Fine enough that a supply growing the plain sink does not also reach
    // the limit, ending the decode before its sinks are read.
    let feed = 64;
    let bound = most_held(len, SPECULATION_LIMIT, feed as u64) as usize;
    let mut task = Speculation::continuing(0, Point::Block(start), len, SPECULATION_LIMIT, len);
    let mut most = 0;
    while let Some(at) = task.wants() {
        let at = at as usize;
        task.supply(&file[at..(at + feed).min(file.len())]);
        most = most.max(task.written());
    }
    assert!(most <= bound, "held {most} against {bound}");
    assert!(10 * most > 29 * SPECULATION_LIMIT, "held {most}: the sinks never filled together");
}

/// **The engine's decode of a guess the event count cut records no more than
/// a decode's bound**: over a run of empty fixed blocks, each one's static
/// start and its end, about twice the events the guess spans.
#[test]
fn a_decode_of_a_guess_the_count_cut_records_within_its_bound() {
    use crate::chunk::{ChunkDecode, EVENTS_A_DECODE, PIECES_A_DECODE};
    let blocks = 3 * EVENTS_A_GUESS / 2;
    let (file, stored) = empty_blocks_then(blocks, &payload(300_000, 41));
    let truth = truth(&file);
    let bytes = stored / 8 + 20_000;
    let first = &chained(&file, 0, bytes, usize::MAX, 1 << 20)[0];
    assert!(first.cut, "a guess the count did not cut");
    let window = truth.window(truth.offset_of(first.start).unwrap()).to_vec();
    let len = file.len() as u64;
    let mut task = ChunkDecode::new((0, 0), first.start, window, first.stop, first.len, len);
    while let Some(at) = task.wants() {
        let at = at as usize;
        task.supply(&file[at..(at + (1 << 20)).min(file.len())]);
    }
    let decoded = task.finish().found.expect("the guess decoded");
    let events = decoded.events.len();
    assert!(events >= 2 * (EVENTS_A_GUESS - 2), "{events} events");
    assert!(events <= EVENTS_A_DECODE, "{events} events against {EVENTS_A_DECODE}");
    assert!(decoded.pieces.len() <= PIECES_A_DECODE, "{} pieces", decoded.pieces.len());
}
