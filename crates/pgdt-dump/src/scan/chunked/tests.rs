use super::*;
use crate::scan::Row;

/// What [`drive`] saw: the events rendered as text, the high-water mark of
/// the carry, and the bytes handed to the scanner over every pass.
struct Driven {
    events: Vec<String>,
    high_water: usize,
    scanned: usize,
}

/// Feed `file` to a [`ChunkedScanner`] over `scanner` in `read`'s chunks,
/// as `next_chunk` would read them, handing every event to `on_event`.
fn feed_all(
    scanner: CopyScanner,
    file: &[u8],
    read: ReadOptions,
    mut on_event: impl FnMut(&ChunkEvents<'_>, Event<'_>),
) -> Result<()> {
    let mut chunked = ChunkedScanner::new(scanner, file.len() as u64, read);
    let mut read_pos = 0usize;
    loop {
        let want = read.chunk_size_bytes.min(file.len() - read_pos);
        let mut chunk = chunked.feed(Bytes::copy_from_slice(&file[read_pos..read_pos + want]));
        read_pos += want;
        while let Some(event) = chunk.next_event()? {
            on_event(&chunk, event);
        }
        if chunked.at_end() {
            return Ok(());
        }
    }
}

fn options(chunk_size: usize) -> ReadOptions {
    ReadOptions { chunk_size_bytes: chunk_size, ..ReadOptions::default() }
}

/// Drive [`ChunkedScanner`] over `file` in `chunk_size` pieces.
fn drive(file: &[u8], chunk_size: usize) -> Driven {
    let mut driven = Driven { events: Vec::new(), high_water: 0, scanned: 0 };
    let mut chunked =
        ChunkedScanner::new(CopyScanner::new(), file.len() as u64, options(chunk_size));
    let mut read_pos = 0usize;
    loop {
        let want = chunk_size.min(file.len() - read_pos);
        let mut chunk = chunked.feed(Bytes::copy_from_slice(&file[read_pos..read_pos + want]));
        read_pos += want;
        driven.scanned += chunk.scanned();
        while let Some(event) = chunk.next_event().unwrap() {
            driven.events.push(format!("{event:?}"));
        }
        driven.high_water = driven.high_water.max(chunked.carried());
        if chunked.at_end() {
            return driven;
        }
    }
}

/// A carry holding `line`, reached the way a read loop reaches one: a chunk
/// the scanner could not finish, whose tail it kept.
fn carrying(line: &[u8]) -> ChunkCarry {
    let mut carry = ChunkCarry::default();
    let mut passes = carry.absorb(line, false);
    passes.consumed(ChunkPass::Carry, 0);
    passes.consumed(ChunkPass::InPlace, 0);
    assert_eq!(carry.len(), line.len());
    carry
}

fn control() -> Vec<u8> {
    let mut file = b"--\n-- A comment\n--\nCOPY public.t (a, b) FROM stdin;\n".to_vec();
    for i in 0..64u32 {
        file.extend_from_slice(format!("{i}\tvalue-{i}\n").as_bytes());
    }
    file.extend_from_slice(b"\\.\n\nSELECT 1;\n");
    file
}

/// The claim the whole mechanism rests on: what a chunk leaves unconsumed
/// is one unterminated line, so the bytes a read loop copies are bounded
/// by the longest line and not by the chunk size.
#[test]
fn the_carry_is_bounded_by_the_longest_line_not_by_the_chunk() {
    let file = control();
    let longest = file.split(|&b| b == b'\n').map(<[u8]>::len).max().unwrap();
    for chunk_size in [1usize, 2, 3, 7, 13, 64, 511, 4096] {
        let high_water = drive(&file, chunk_size).high_water;
        assert!(
            high_water <= longest,
            "chunk_size {chunk_size} carried {high_water} bytes, longest line is {longest}"
        );
    }
}

/// Two buffers within one chunk are as good as one, because
/// `CopyScanner::base` is an absolute file offset — so every event,
/// offsets included, is the same wherever the boundaries fall.
#[test]
fn the_event_stream_does_not_depend_on_where_the_split_falls() {
    let file = control();
    let reference = drive(&file, 1 << 20).events;
    for chunk_size in [1usize, 2, 3, 7, 13, 64, 511, 4096] {
        let got = drive(&file, chunk_size).events;
        assert_eq!(got, reference, "chunk_size {chunk_size}");
    }
}

/// A file whose lines are many chunks long: a row inside a `COPY` block,
/// and an unterminated last line that only the end of the file finishes.
fn long_lines(len: usize) -> Vec<u8> {
    let value = "x".repeat(len);
    let mut file = b"COPY public.t (a, b) FROM stdin;\n1\tshort\n2\t".to_vec();
    file.extend_from_slice(value.as_bytes());
    file.extend_from_slice(b"\n3\tshort\n\\.\n");
    file.extend_from_slice(value.as_bytes());
    file
}

/// A line many chunks long is handed to the scanner once, when the chunk
/// holding its newline — or the end of the file — arrives, so the bytes
/// scanned are linear in the file and not in the square of the line.
/// Counted, not timed: re-searching the carry at every chunk costs about
/// `len² / (2 × chunk)` bytes here, orders past the bound.
#[test]
fn a_line_many_chunks_long_is_scanned_once() {
    let file = long_lines(1 << 16);
    let reference = drive(&file, 1 << 20).events;
    for chunk_size in [1usize, 7, 64, 4096] {
        let driven = drive(&file, chunk_size);
        assert_eq!(driven.events, reference, "chunk_size {chunk_size}");
        assert!(
            driven.scanned <= 2 * file.len(),
            "chunk_size {chunk_size} scanned {} bytes of a {}-byte file",
            driven.scanned,
            file.len()
        );
    }
}

/// **A line carried past the limit is refused where the chunk leaving it
/// ends**, at the scanner's position — the line's start — and a line as long
/// that the file's last chunk ends is not measured.
#[test]
fn a_carried_line_past_the_limit_is_refused() {
    let file = long_lines(64);
    let limit = ReadOptions { max_line_bytes: 16, ..options(40) };
    let refused = feed_all(CopyScanner::new(), &file, limit, |_, _| {});
    let row_start = file.windows(2).position(|w| w == b"2\t").unwrap() as u64;
    match refused {
        Err(Error::LineTooLong { offset, limit: 16 }) => assert_eq!(offset, row_start),
        other => panic!("{other:?}"),
    }

    // The comment's first fourteen bytes are carried, under the limit; the last
    // chunk ends it past the limit.
    let last_line = b"SELECT 1;\n-- a comment longer than the limit".to_vec();
    let limit = ReadOptions { max_line_bytes: 16, ..options(24) };
    feed_all(CopyScanner::new(), &last_line, limit, |_, _| {})
        .expect("the last chunk's line is not measured");
}

/// **A row's offset less its span's start indexes the row in the span**, in
/// either pass: what a replay validating a span once and slicing each row
/// out of it relies on.
#[test]
fn a_row_sits_in_its_span_at_its_offset() {
    let file = control();
    for chunk_size in [1usize, 2, 7, 64, 4096] {
        let mut rows = 0;
        feed_all(CopyScanner::new(), &file, options(chunk_size), |chunk, event| {
            if let Event::Row(Row { offset, raw, .. }) = event {
                let (base, span) = chunk.span();
                let at = (offset - base) as usize;
                assert_eq!(&span[at..at + raw.len()], raw, "chunk_size {chunk_size}");
                rows += 1;
            }
        })
        .unwrap();
        assert_eq!(rows, 64);
    }
}

/// A scanner put down past bytes read elsewhere reads on from there, its
/// carry dropped: the events after `skip_to` are those of the file from
/// that line on.
#[test]
fn skip_to_reads_on_from_a_line_past_the_carry() {
    let file = control();
    let at = file.windows(3).position(|w| w == b"\\.\n").unwrap() as u64 + 3;
    let reference = drive(&file, 1 << 20).events;
    // The blank line and `SELECT 1;` after the block's `\.`.
    let reference = &reference[reference.len() - 2..];
    let mut chunked = ChunkedScanner::new(CopyScanner::new(), file.len() as u64, options(5));
    // Leave a line carried, as a chunk ends mid-row.
    let mut chunk = chunked.feed(Bytes::copy_from_slice(&file[..5]));
    while chunk.next_event().unwrap().is_some() {}
    assert!(chunked.carried() > 0);
    chunked.skip_to(at);
    assert_eq!(chunked.carried(), 0);
    let mut events = Vec::new();
    let mut chunk = chunked.feed(Bytes::copy_from_slice(&file[at as usize..]));
    while let Some(event) = chunk.next_event().unwrap() {
        events.push(format!("{event:?}"));
    }
    assert!(chunked.at_end());
    assert_eq!(events, reference);
}

/// With nothing carried the chunk is scanned whole, where it lies: no
/// prefix is taken and the in-place span is the chunk itself.
#[test]
fn an_empty_carry_copies_nothing() {
    let mut carry = ChunkCarry::default();
    let passes = carry.absorb(b"one\ntwo\n", false);
    assert_eq!(passes.span(ChunkPass::Carry).0, b"");
    assert_eq!(passes.span(ChunkPass::InPlace).0, b"one\ntwo\n");
}

/// The carry takes the chunk's first line-terminated prefix and no more —
/// one line's worth of copying, whatever the chunk's length.
#[test]
fn a_carry_takes_only_the_prefix_that_completes_its_line() {
    let mut carry = carrying(b"abc");

    let mut passes = carry.absorb(b"def\nghi\njkl", false);
    let (span, _) = passes.span(ChunkPass::Carry);
    assert_eq!(span, b"abcdef\n");
    // The scanner would consume that whole line, which is the protocol's
    // precondition for the in-place span below.
    passes.consumed(ChunkPass::Carry, span.len());
    assert_eq!(passes.span(ChunkPass::InPlace).0, b"ghi\njkl");
}

/// The degenerate case: a chunk with no newline in it joins the carry
/// whole, which is the growth `ReadOptions::max_line_bytes` bounds, and
/// neither pass has anything to scan until the file ends.
#[test]
fn a_chunk_with_no_newline_joins_the_carry_whole() {
    let mut carry = carrying(b"abc");

    let mut passes = carry.absorb(b"defghi", false);
    assert_eq!(passes.carried(), 9);
    assert_eq!(passes.span(ChunkPass::Carry).0, b"");
    assert_eq!(passes.span(ChunkPass::InPlace).0, b"");

    // The chunk that finishes the line hands the whole of it over once.
    passes.consumed(ChunkPass::Carry, 0);
    passes.consumed(ChunkPass::InPlace, 0);
    let passes = carry.absorb(b"jk\nl", false);
    assert_eq!(passes.span(ChunkPass::Carry).0, b"abcdefghijk\n");

    // At the end of the file the unfinished line is handed over after all.
    let mut carry = carrying(b"abc");
    let passes = carry.absorb(b"defghi", true);
    assert_eq!(passes.span(ChunkPass::Carry).0, b"abcdefghi");
}

/// The carry pass runs to the end of the file only when the chunk has
/// nothing left for the in-place pass — otherwise it would treat a line
/// still being carried as the file's last.
#[test]
fn the_carry_pass_is_eof_only_when_the_chunk_is_exhausted() {
    let mut carry = carrying(b"abc");
    let mut passes = carry.absorb(b"def\nghi", true);
    assert!(!passes.span(ChunkPass::Carry).1);
    passes.consumed(ChunkPass::Carry, passes.line.len());
    assert!(passes.span(ChunkPass::InPlace).1);

    let mut carry = carrying(b"abc");
    let passes = carry.absorb(b"defghi", true);
    assert!(passes.span(ChunkPass::Carry).1);
}

/// An unterminated `COPY` block is still an error when the file ends
/// inside it, wherever the last chunk boundary fell — the carry pass takes
/// `eof` for exactly the case where nothing follows it.
#[test]
fn a_file_ending_inside_a_copy_block_is_still_rejected() {
    let file = b"COPY public.t (a) FROM stdin;\n1\n2\n3".to_vec();
    for chunk_size in [1usize, 2, 3, 7, 4096] {
        let err = feed_all(CopyScanner::new(), &file, options(chunk_size), |_, _| {});
        assert!(
            matches!(err, Err(Error::UnterminatedCopyBlock { header_offset: 0 })),
            "chunk_size {chunk_size} gave {err:?}"
        );
    }
}
