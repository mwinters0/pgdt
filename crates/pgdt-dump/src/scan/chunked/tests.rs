use super::*;
use crate::scan::{Row, StreamedRow};

/// What [`drive`] saw: the events rendered as text, the high-water mark of
/// the carry, and the bytes handed to the scanner over every pass.
struct Driven {
    events: Vec<String>,
    high_water: usize,
    scanned: usize,
}

/// Feed `file` to a [`ChunkedScanner`] over `scanner` in `read`'s chunks,
/// as `next_chunk` would read them, each held to the line bound first,
/// handing every event to `on_event`.
fn feed_all(
    scanner: CopyScanner,
    file: &[u8],
    read: ReadOptions,
    mut on_event: impl FnMut(&ChunkEvents<'_>, Event<'_>),
) -> Result<()> {
    let mut chunked = ChunkedScanner::new(scanner, file.len() as u64, read);
    let mut read_pos = 0usize;
    loop {
        let want = read.chunk().min(file.len() - read_pos);
        let bytes = Bytes::copy_from_slice(&file[read_pos..read_pos + want]);
        chunked.hold_to_bound(&bytes)?;
        let mut chunk = chunked.feed(bytes, None);
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
        let mut chunk =
            chunked.feed(Bytes::copy_from_slice(&file[read_pos..read_pos + want]), None);
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
    carrying_streamed(line, false)
}

/// [`carrying`], the line a row of a block whose rows are streamed where
/// `streams`.
fn carrying_streamed(line: &[u8], streams: bool) -> ChunkCarry {
    let mut carry = ChunkCarry::new(4);
    let mut passes = carry.absorb(line, false, 0, None);
    passes.streams(streams);
    passes.consumed(ChunkPass::Carry, 0);
    passes.consumed(ChunkPass::InPlace, 0);
    assert_eq!(carry.carried(), line.len());
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

/// `chunk_size`'s reads, each line held to `bound`.
fn bounded(chunk_size: usize, bound: usize) -> ReadOptions {
    ReadOptions { line_bound: bound, ..options(chunk_size) }
}

/// **A line is refused exactly where it is longer than the bound, its
/// terminator included** (`docs/design/decisions.md`, "D164"): a row of
/// exactly the bound is read and one a byte longer refused at its start,
/// naming its table, whatever the chunk — one past the bound reading in
/// chunks of it — and a line outside a block alike, naming none; a line the
/// file's end ends is measured to it.
#[test]
fn a_line_is_refused_exactly_past_the_bound() {
    let row_start = long_lines(1).windows(2).position(|w| w == b"2\t").unwrap() as u64;
    // `2\t`, the value and its LF: the bound's length at `len = bound - 3`.
    let bound = 48;
    for chunk_size in [1usize, 5, 16, 47, 48, 49, 64, 4096] {
        let read = bounded(chunk_size, bound);
        // The value after the block is a line too, a byte shorter than the
        // row, ending the file unterminated.
        feed_all(CopyScanner::new(), &long_lines(bound - 3), read, |_, _| {})
            .unwrap_or_else(|e| panic!("chunk {chunk_size}: {e}"));
        match feed_all(CopyScanner::new(), &long_lines(bound - 2), read, |_, _| {}) {
            Err(Error::LineTooLong { offset, table, bound: 48 }) => {
                assert_eq!(offset, row_start, "chunk {chunk_size}");
                assert_eq!(table.as_deref(), Some("public.t"), "chunk {chunk_size}");
            }
            other => panic!("chunk {chunk_size}: {other:?}"),
        }

        // A comment of dashes after `SELECT 1;`, its LF included or the
        // file's end ending it.
        for (dashes, ending) in [(bound - 1, "\n"), (bound, "")] {
            let comment = |dashes: usize| format!("SELECT 1;\n{}{ending}", "-".repeat(dashes));
            feed_all(CopyScanner::new(), comment(dashes).as_bytes(), read, |_, _| {})
                .unwrap_or_else(|e| panic!("chunk {chunk_size}, {ending:?}: {e}"));
            match feed_all(CopyScanner::new(), comment(dashes + 1).as_bytes(), read, |_, _| {}) {
                Err(Error::LineTooLong { offset: 10, table: None, bound: 48 }) => {}
                other => panic!("chunk {chunk_size}, {ending:?}: {other:?}"),
            }
        }
    }
}

/// **A read is never longer than the bound**, so a line a read holds whole
/// is within it ([`ReadOptions::chunk`]).
#[test]
fn a_chunk_is_never_longer_than_the_bound() {
    assert_eq!(bounded(4096, 48).chunk(), 48);
    assert_eq!(bounded(16, 48).chunk(), 16);
    assert_eq!(ReadOptions::default().line_bound(), LINE_BOUND);
    assert_eq!(ReadOptions::default().chunk(), SCAN_CHUNK_DEFAULT_SIZE_BYTES);
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
    let mut chunk = chunked.feed(Bytes::copy_from_slice(&file[..5]), None);
    while chunk.next_event().unwrap().is_some() {}
    assert!(chunked.carried() > 0);
    chunked.skip_to(at);
    assert_eq!(chunked.carried(), 0);
    let mut events = Vec::new();
    let mut chunk = chunked.feed(Bytes::copy_from_slice(&file[at as usize..]), None);
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
    let mut carry = ChunkCarry::new(4);
    let passes = carry.absorb(b"one\ntwo\n", false, 0, None);
    assert_eq!(passes.span(ChunkPass::Carry).0, b"");
    assert_eq!(passes.span(ChunkPass::InPlace).0, b"one\ntwo\n");
}

/// The carry takes the chunk's first line-terminated prefix and no more —
/// one line's worth of copying, whatever the chunk's length.
#[test]
fn a_carry_takes_only_the_prefix_that_completes_its_line() {
    let mut carry = carrying(b"abc");

    let mut passes = carry.absorb(b"def\nghi\njkl", false, 0, None);
    let (span, _) = passes.span(ChunkPass::Carry);
    assert_eq!(span, b"abcdef\n");
    // The scanner would consume that whole line, which is the protocol's
    // precondition for the in-place span below.
    passes.consumed(ChunkPass::Carry, span.len());
    assert_eq!(passes.span(ChunkPass::InPlace).0, b"ghi\njkl");
}

/// The degenerate case: a chunk with no newline in it joins the carry
/// whole, which is the growth `ReadOptions::line_bound` bounds, and
/// neither pass has anything to scan until the file ends.
#[test]
fn a_chunk_with_no_newline_joins_the_carry_whole() {
    let mut carry = carrying(b"abc");

    let mut passes = carry.absorb(b"defghi", false, 0, None);
    assert_eq!(passes.carried(), 9);
    assert_eq!(passes.span(ChunkPass::Carry).0, b"");
    assert_eq!(passes.span(ChunkPass::InPlace).0, b"");

    // The chunk that finishes the line hands the whole of it over once.
    passes.consumed(ChunkPass::Carry, 0);
    passes.consumed(ChunkPass::InPlace, 0);
    let passes = carry.absorb(b"jk\nl", false, 0, None);
    assert_eq!(passes.span(ChunkPass::Carry).0, b"abcdefghijk\n");

    // At the end of the file the unfinished line is handed over after all.
    let mut carry = carrying(b"abc");
    let passes = carry.absorb(b"defghi", true, 0, None);
    assert_eq!(passes.span(ChunkPass::Carry).0, b"abcdefghi");
}

/// The carry pass runs to the end of the file only when the chunk has
/// nothing left for the in-place pass — otherwise it would treat a line
/// still being carried as the file's last.
#[test]
fn the_carry_pass_is_eof_only_when_the_chunk_is_exhausted() {
    let mut carry = carrying(b"abc");
    let mut passes = carry.absorb(b"def\nghi", true, 0, None);
    assert!(!passes.span(ChunkPass::Carry).1);
    passes.consumed(ChunkPass::Carry, passes.line.len());
    assert!(passes.span(ChunkPass::InPlace).1);

    let mut carry = carrying(b"abc");
    let passes = carry.absorb(b"defghi", true, 0, None);
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

/// A file held in memory, its extent `Exact` or — with `open` — not stated
/// until a read meets the end.
struct InMemory {
    file: Bytes,
    open: bool,
    reached: std::sync::atomic::AtomicBool,
    /// The bytes every read so far answered.
    read: std::sync::atomic::AtomicUsize,
}

impl ByteRangeSource for InMemory {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Bytes>> + Send + '_>> {
        let at = offset as usize;
        self.read.fetch_add(len, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move { Ok(self.file.slice(at..at + len)) })
    }
    fn read_at_most(
        &self,
        offset: u64,
        len: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Bytes>> + Send + '_>> {
        let at = (offset as usize).min(self.file.len());
        let end = (at + len).min(self.file.len());
        if end == self.file.len() {
            self.reached.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Box::pin(async move { Ok(self.file.slice(at..end)) })
    }
    fn extent(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Extent>> + Send + '_>> {
        let len = self.file.len() as u64;
        let known = !self.open || self.reached.load(std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            Ok(if known { Extent::Exact(len) } else { Extent::Open { at_least: 0 } })
        })
    }
    fn modified(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<std::time::SystemTime>>> + Send + '_>,
    > {
        Box::pin(async { Ok(None) })
    }
}

/// **A scan finds an end it was not told** from the read that answers short —
/// or, where the file is a whole number of chunks, from the empty read after
/// the last full one — and hands out the events a scan told the end hands
/// out, answering the same end.
#[tokio::test]
async fn a_scan_finds_an_open_end_where_a_read_answers_short() {
    let file = b"SET x = 1;\nCOPY public.t (a) FROM stdin;\n1\n2\n\\.\nSELECT 1;\n";
    for chunk_size in [1usize, 3, file.len() / 2, file.len(), file.len() + 5] {
        let mut answers = Vec::new();
        for open in [false, true] {
            let source = InMemory {
                file: Bytes::from_static(file),
                open,
                reached: std::sync::atomic::AtomicBool::new(false),
                read: std::sync::atomic::AtomicUsize::new(0),
            };
            let options = ScanOptions { read: options(chunk_size), ..ScanOptions::default() };
            let mut events = Vec::new();
            let end = scan(&source, &options, |event| {
                events.push(format!("{event:?}"));
                ControlFlow::Continue(())
            })
            .await
            .unwrap();
            answers.push((events, end));
        }
        assert_eq!(answers[0].1, Some(file.len() as u64), "chunk {chunk_size}");
        assert_eq!(answers[0], answers[1], "chunk {chunk_size}");
    }
}

/// **The line a carry pass scans is handed out at its start**, as the bytes
/// the pass scans, so a replay can keep it as a block its views point into;
/// a chunk with nothing carried, or whose carried line waits, hands out none.
#[test]
fn a_held_line_is_handed_out_at_its_start() {
    let file = long_lines(100);
    let row_start = file.windows(2).position(|w| w == b"2\t").unwrap();
    let row_end = row_start + file[row_start..].iter().position(|&b| b == b'\n').unwrap() + 1;
    let mut held = Vec::new();
    feed_all(CopyScanner::new(), &file, options(16), |chunk, event| {
        if let Event::Row(Row { offset, raw, .. }) = event
            && offset == row_start as u64
        {
            let (start, line) = chunk.held().expect("the row arrives on the carry");
            assert_eq!(start, offset);
            assert_eq!(&line[..raw.len()], raw);
            held.push(line.len());
        }
    })
    .unwrap();
    assert_eq!(held, [row_end - row_start]);

    let mut carry = ChunkCarry::new(4);
    assert!(carry.absorb(b"one\ntwo\n", false, 0, None).held().is_empty());
    let mut carry = carrying(b"abc");
    assert!(carry.absorb(b"defghi", false, 0, None).held().is_empty());
}

/// **A carry keeps no line past its row**: after a line many chunks long,
/// what it holds allocated is bounded by the chunk, not by the line, and
/// while it carries one, by the line.
#[test]
fn the_carry_keeps_no_line_past_its_row() {
    // Past two of a mapping's steps, where its whole steps fall short of a
    // doubling's capacity.
    let step = LineMapping::step();
    let len = 2 * step + step / 4;
    // The block alone, without the long line after it.
    let mut file = long_lines(len);
    file.truncate(file.windows(3).position(|w| w == b"\\.\n").unwrap() + 3);
    let chunk_size = 64;
    let mut chunked =
        ChunkedScanner::new(CopyScanner::new(), file.len() as u64, options(chunk_size));
    let mut read_pos = 0usize;
    let mut longest = 0;
    let mut after_row = None;
    while !chunked.at_end() {
        let want = chunk_size.min(file.len() - read_pos);
        let mut chunk =
            chunked.feed(Bytes::copy_from_slice(&file[read_pos..read_pos + want]), None);
        read_pos += want;
        let mut long_row = false;
        while let Some(event) = chunk.next_event().unwrap() {
            long_row |= matches!(event, Event::Row(Row { raw, .. }) if raw.len() > len);
        }
        let allocated = chunked.carry.allocated();
        longest = longest.max(allocated);
        if long_row {
            after_row = Some(read_pos);
        } else if after_row.is_some_and(|at| read_pos > at) {
            assert!(allocated <= 4 * chunk_size, "{allocated} bytes kept after the row");
        }
    }
    assert!(after_row.is_some());
    assert!(longest > len, "the line was carried whole while it grew");
    // Past its chunk it grew in a mapping of its own, holding the steps it
    // wrote rather than a doubling's capacity (`line.rs`).
    let held = len.next_multiple_of(step) + 4 * chunk_size;
    assert!(longest <= held, "{longest} bytes held for a {len}-byte line");
}

/// What a read loop streaming every block's rows — or, with `stream` off,
/// holding them — answers of `file` in `read`'s chunks: every event, a row
/// rendered as its index, its offset, where the scanner stands past it and
/// its fields — a streamed row's as its fold was handed them — so a held row
/// and a streamed one read alike; how many rows were streamed; and the most
/// the carry kept allocated between two chunks.
struct Streamed {
    events: Result<Vec<String>>,
    streamed: usize,
    allocated: usize,
    /// How many times the carry gave up what it held, where it was made to.
    given_up: usize,
}

/// A fold recording each streamed row's start and fields as it was handed
/// them, holding the field `holds` names whole.
#[derive(Default)]
struct Recorder {
    holds: Option<usize>,
    starts: std::collections::VecDeque<u64>,
    rows: std::collections::VecDeque<Vec<Vec<u8>>>,
    row: Vec<Vec<u8>>,
}

impl FieldFold for Recorder {
    fn holds(&self, field: usize) -> bool {
        self.holds == Some(field)
    }

    fn begin_row(&mut self, offset: u64) {
        assert!(self.row.is_empty(), "a row begins after the last ended");
        self.starts.push_back(offset);
    }

    fn fold(&mut self, field: usize, bytes: &[u8], ends: bool) {
        if field == self.row.len() {
            self.row.push(Vec::new());
        }
        assert_eq!(field + 1, self.row.len(), "fields are handed over in order");
        if self.holds(field) {
            assert!(ends && self.row[field].is_empty(), "a held field is handed over once");
        }
        self.row[field].extend_from_slice(bytes);
    }

    fn end_row(&mut self, fields: usize) {
        assert_eq!(fields, self.row.len(), "every field ended");
        self.rows.push_back(std::mem::take(&mut self.row));
    }
}

/// A row's fields as a rendering shows them.
fn render_fields<'a>(fields: impl Iterator<Item = &'a [u8]>) -> String {
    let fields: Vec<String> = fields.map(|f| String::from_utf8_lossy(f).into_owned()).collect();
    format!("{} fields {:?}", fields.len(), fields.join("|"))
}

fn stream(file: &[u8], read: ReadOptions, stream: bool) -> Streamed {
    stream_holding(file, read, stream, None)
}

/// [`stream`], its fold holding field `holds` whole.
fn stream_holding(file: &[u8], read: ReadOptions, stream: bool, holds: Option<usize>) -> Streamed {
    stream_giving_up(file, read, stream, holds, false)
}

/// [`stream_holding`], its carry made — where `give_up` — to give up what it
/// holds past its chunk before every chunk it absorbs and take it back read
/// again from `file` a chunk at a time, as a reader a ledger asks does
/// ([`draw_absorbing`]).
fn stream_giving_up(
    file: &[u8],
    read: ReadOptions,
    stream: bool,
    holds: Option<usize>,
    give_up: bool,
) -> Streamed {
    let mut out = Streamed { events: Ok(Vec::new()), streamed: 0, allocated: 0, given_up: 0 };
    let mut events = Vec::new();
    let mut fold = Recorder { holds, ..Recorder::default() };
    let mut chunked = ChunkedScanner::new(CopyScanner::new(), file.len() as u64, read);
    let mut read_pos = 0usize;
    let ran: Result<()> = (|| loop {
        let want = read.chunk().min(file.len() - read_pos);
        let bytes = Bytes::copy_from_slice(&file[read_pos..read_pos + want]);
        chunked.hold_to_bound(&bytes)?;
        let eof = read_pos + want >= file.len();
        if give_up && chunked.carry.charge_absorbing(&bytes, eof, Some(&fold)) > 0 {
            let given = chunked.carry.give_up();
            let range = given.range(read_pos as u64);
            let (start, end) = (range.start as usize, range.end as usize);
            for again in file[start..end].chunks(read.chunk_size_bytes) {
                chunked.carry.restore(again);
            }
            out.given_up += 1;
        }
        let mut chunk = chunked.feed(bytes, Some(&mut fold));
        read_pos += want;
        while let Some(event) = chunk.next_event()? {
            let past = chunk.scanner().position();
            events.push(match event {
                Event::Row(Row { offset, index, raw }) => {
                    let fields = render_fields(pgdt_core::split_fields(raw));
                    format!("row {index}@{offset}..{past}, {fields}")
                }
                Event::StreamedRow(StreamedRow { offset, index, len, fields }) => {
                    assert_eq!(offset + len, past, "a streamed row's length");
                    out.streamed += 1;
                    let folded = fold.rows.pop_front().expect("the fold ended the row");
                    assert_eq!(fold.starts.pop_front(), Some(offset), "where the row began");
                    assert_eq!(folded.len(), fields, "the fold's fields are the row's");
                    let fields = render_fields(folded.iter().map(Vec::as_slice));
                    format!("row {index}@{offset}..{past}, {fields}")
                }
                Event::CopyStart(start) => {
                    chunk.stream_rows(stream);
                    format!("{:?}", Event::CopyStart(start))
                }
                other => format!("{other:?}"),
            });
        }
        out.allocated = out.allocated.max(chunked.carry.allocated());
        if chunked.at_end() {
            return Ok(());
        }
    })();
    out.events = ran.map(|()| events);
    out
}

/// A block of short rows around rows many chunks long — one in each of the
/// three endings, a backslash before the chunk boundary and a CR escaped
/// across one among them, one ending in a CR a backslash escapes, the last
/// field's own byte, and one whose delimiters lie among its long
/// values, a tab a backslash precedes among them, so its fields are counted
/// across chunk boundaries — and the DDL after it.
fn streamable(len: usize, block_ending: &str, long_ending: &str) -> Vec<u8> {
    let value = "x".repeat(len);
    let mut file = b"COPY public.t (a, b) FROM stdin;\n".to_vec();
    for row in [
        format!("1\tshort{block_ending}"),
        format!("2\t{value}{long_ending}"),
        format!("3\t\\\\{value}\\\r{value}{block_ending}"),
        format!("4\tab{block_ending}"),
        format!("5\t{value}\\\\{block_ending}"),
        format!("6\t{value}\t\t{value}\\\t{value}\t{block_ending}"),
        format!("7\t{value}\\\r{block_ending}"),
    ] {
        file.extend_from_slice(row.as_bytes());
    }
    file.extend_from_slice(format!("\\.{block_ending}\nSELECT 1;\n").as_bytes());
    file
}

/// **A streamed row reads as the row it stands for**: whatever its ending,
/// wherever the chunk boundaries fall, a read loop streaming a block's rows
/// answers the events — rows by index, offset, end and fields, a streamed
/// row's as its fold was handed them, a held field whole and its ending
/// stripped as a held row's is — and the refusals of one holding them, a
/// terminator split across chunks included, and streams each row a whole
/// chunk passes inside.
#[test]
fn a_streamed_row_reads_as_the_row_it_stands_for() {
    for (block, long) in
        [("\n", "\n"), ("\r\n", "\r\n"), ("\n", "\r\n"), ("\r\n", "\n"), ("\n", "\r\rx\n")]
    {
        let file = streamable(200, block, long);
        for chunk_size in [1usize, 2, 3, 4, 5, 7, 13, 64, 199, 4096] {
            let read = options(chunk_size);
            let held = stream(&file, read, false);
            let streamed = stream_holding(&file, read, true, Some(1));
            let context = format!("{block:?} block, {long:?} row, chunk {chunk_size}");
            match (&held.events, &streamed.events) {
                (Ok(held), Ok(got)) => assert_eq!(got, held, "{context}"),
                (Err(held), Err(got)) => {
                    assert_eq!(format!("{got:?}"), format!("{held:?}"), "{context}");
                }
                (held, got) => panic!("{context}: held {held:?}, streamed {got:?}"),
            }
            assert_eq!(held.streamed, 0, "{context}");
            if chunk_size <= 64 && streamed.events.is_ok() {
                assert!(streamed.streamed >= 3, "{context}: {} streamed", streamed.streamed);
            }
        }
    }
}

/// **A streamed row holds nothing**: a block whose rows run to many chunks
/// keeps a few chunks allocated between two chunks at any line length,
/// where holding them keeps the line.
#[test]
fn a_streamed_row_holds_nothing_at_any_length() {
    let chunk_size = 64;
    for len in [1 << 10, 1 << 16, 1 << 20] {
        let file = streamable(len, "\n", "\n");
        let held = stream(&file, options(chunk_size), false);
        let streamed = stream(&file, options(chunk_size), true);
        assert_eq!(streamed.events.unwrap(), held.events.unwrap());
        assert!(held.allocated > len, "{len}: held {}", held.allocated);
        assert!(streamed.allocated <= 4 * chunk_size, "{len}: streamed {}", streamed.allocated);
    }
}

/// **A streamed row past the bound is refused as a held one is**, at its
/// start; and one the file ends inside a block in is the block's refusal,
/// as a held one is.
#[test]
fn a_streamed_row_is_refused_where_a_held_one_is() {
    let file = streamable(200, "\n", "\n");
    let row_start = file.windows(2).position(|w| w == b"2\t").unwrap() as u64;
    for chunk_size in [5usize, 40, 64] {
        for streams in [false, true] {
            match stream(&file, bounded(chunk_size, 100), streams).events {
                Err(Error::LineTooLong { offset, .. }) => assert_eq!(offset, row_start),
                other => panic!("chunk {chunk_size}, streamed {streams}: {other:?}"),
            }
        }
    }

    let mut file = b"COPY public.t (a) FROM stdin;\n1\n".to_vec();
    file.extend_from_slice("x".repeat(300).as_bytes());
    for chunk_size in [1usize, 7, 64] {
        let got = stream(&file, options(chunk_size), true);
        assert!(
            matches!(got.events, Err(Error::UnterminatedCopyBlock { header_offset: 0 })),
            "chunk {chunk_size}: {:?}",
            got.events
        );
        assert_eq!(got.streamed, usize::from(chunk_size < 300), "chunk {chunk_size}");
    }
}

/// **A block's end undoes its streaming**: a line after its terminator that
/// outgrows its chunk is carried whole, as every line outside a block is.
#[test]
fn a_line_after_a_streamed_block_is_held() {
    let file = long_lines(1 << 12);
    let held = stream(&file, options(64), false);
    let streamed = stream(&file, options(64), true);
    assert_eq!(streamed.events.unwrap(), held.events.unwrap());
    assert_eq!(streamed.streamed, 1, "the row alone");
    assert!(streamed.allocated > 1 << 12, "the last line held, {}", streamed.allocated);
}

/// **What a carry will hold past its chunk is the line it carries
/// completed, or joined whole**, and nothing where that fits a chunk or
/// nothing is carried.
#[test]
fn a_carry_charges_what_it_will_hold_past_its_chunk() {
    let carry = carrying(b"abcdef");
    assert_eq!(carry.charge_absorbing(b"gh\nij", false, None), 9, "completed by the prefix");
    assert_eq!(carry.charge_absorbing(b"ghij", false, None), 10, "joined whole");
    let short = carrying(b"ab");
    assert_eq!(short.charge_absorbing(b"c\nde", false, None), 0, "no longer than its chunk");
    let nothing = ChunkCarry::new(4);
    assert_eq!(nothing.charge_absorbing(b"abcdefgh", false, None), 0, "nothing carried");
}

/// **A row its carry starts streaming over the next chunk is drawn for
/// nothing** but the field its fold holds whole across their boundary,
/// keyed by where that field starts, as the next chunk keys it once the row
/// streams; a row the chunk ends, the file ends or whose block reads its
/// rows whole is held, and charged as a line, as is a line held past its
/// chunk before its block streamed rows.
#[test]
fn a_row_a_carry_starts_streaming_draws_only_the_field_its_fold_holds() {
    let carry = carrying_streamed(b"1\tab", true);
    let holding = |field| Recorder { holds: Some(field), ..Recorder::default() };
    let charge = |next: &[u8], eof, fold: Option<&Recorder>| {
        carry.charge_absorbing(next, eof, fold.map(|fold| fold as &dyn FieldFold))
    };
    assert_eq!(charge(b"efgh", false, None), 0, "streamed, read by no fold");
    assert_eq!(charge(b"efgh", false, Some(&Recorder::default())), 0, "its fold holds none");
    assert_eq!(charge(b"efgh", false, Some(&holding(0))), 0, "the field held lies in a chunk");
    assert_eq!(charge(b"efgh", false, Some(&holding(1))), 6, "the field held, joined whole");
    assert_eq!(charge(b"efg\th", false, Some(&holding(1))), 5, "to its delimiter");
    assert_eq!(charge(b"efgh", true, None), 8, "the file ends: held");
    assert_eq!(charge(b"ef\ngh", false, None), 7, "the chunk ends it: held");
    let whole = carrying_streamed(b"1\tab", false);
    assert_eq!(whole.charge_absorbing(b"efgh", false, None), 8, "rows read whole: held");
    let held = carrying_streamed(b"1\tabcd", true);
    assert_eq!(held.charge_absorbing(b"efgh", false, None), 10, "held before it streamed");

    let mut fold = holding(1);
    let mut carry = carrying_streamed(b"1\tab", true);
    let start = carry.holding_from(4, b"efgh", false, Some(&fold));
    assert_eq!(start, 2, "where the held field starts");
    let _ = carry.absorb(b"efgh", false, 0, Some(&mut fold));
    assert_eq!(carry.holding_from(8, b"ij", false, Some(&fold)), start, "streamed, keyed alike");
    assert_eq!(carry.charge_absorbing(b"ij", false, Some(&fold)), 8, "grown on");
}

/// **A carry that gives up what it holds and takes it back reads as one that
/// never gave it up**: the line it carries, and the field a streamed row's
/// fold holds whole, its CR held back included, dropped before every chunk
/// that grows either past its chunk and read again from where they lie —
/// whatever the row's ending and wherever the chunk boundaries fall.
#[test]
fn a_carry_given_up_and_taken_back_reads_as_one_never_given_up() {
    for (block, long) in
        [("\n", "\n"), ("\r\n", "\r\n"), ("\n", "\r\n"), ("\r\n", "\n"), ("\n", "\r\rx\n")]
    {
        let file = streamable(200, block, long);
        for chunk_size in [1usize, 2, 3, 5, 13, 64, 199] {
            let read = options(chunk_size);
            for (stream, holds) in [(false, None), (true, Some(1)), (true, Some(3))] {
                let context = format!(
                    "{block:?} block, {long:?} row, chunk {chunk_size}, {stream} {holds:?}"
                );
                let kept = stream_giving_up(&file, read, stream, holds, false);
                let given = stream_giving_up(&file, read, stream, holds, true);
                match (&kept.events, &given.events) {
                    (Ok(kept), Ok(got)) => assert_eq!(got, kept, "{context}"),
                    (Err(kept), Err(got)) => {
                        assert_eq!(format!("{got:?}"), format!("{kept:?}"), "{context}");
                    }
                    (kept, got) => panic!("{context}: kept {kept:?}, given up {got:?}"),
                }
                assert_eq!(kept.given_up, 0);
                // Field 3 is long only in the sixth row, which a block
                // refused at its second never reaches.
                let reached = holds != Some(3) || kept.events.is_ok();
                assert!(!reached || given.given_up > 0, "{context}: nothing given up");
            }
        }
    }
}

/// **A held line waits behind an earlier one, and gives its own up to it**:
/// on a ledger with no room, a read loop's line past its chunk waits while an
/// earlier line holds, its chunk dropped and read again once admitted; and
/// asked for its room by an earlier line, it drops the line, waits behind
/// that line and reads the line again from its start — its events those of a
/// loop drawing on no ledger, its source read past the file's length.
#[test]
fn a_held_line_waits_behind_an_earlier_one_and_gives_its_own_up() {
    use pgdt_io::{Admission, Allowance, Arrangement, ArrangementRequest};
    use std::future::Future;
    use std::task::{Context, Waker};

    let mut file = b"SET x = 1;\nCOPY public.t (a) FROM stdin;\n".to_vec();
    file.extend_from_slice("x".repeat(300).as_bytes());
    file.extend_from_slice(b"\n1\n\\.\n");
    let chunk_size = 16;
    let reference = drive(&file, chunk_size).events;
    let source = InMemory {
        file: Bytes::from(file.clone()),
        open: false,
        reached: std::sync::atomic::AtomicBool::new(false),
        read: std::sync::atomic::AtomicUsize::new(0),
    };
    // An allowance whose room other scans drew whole: a ledger of no room,
    // which admits the earliest line alone, under a floor refusing none here.
    let (allowance, request) =
        (Allowance::Stated(1 << 30), ArrangementRequest::recommended(1, None));
    let drawn = Arrangement::resolve(&allowance, request).ledger().capacity().unwrap();
    let ledger = Arrangement::resolve(&allowance, ArrangementRequest { drawn, ..request });
    let ledger = ledger.ledger().clone();
    assert_eq!(ledger.capacity(), Some(0));
    assert!(ledger.floor().unwrap().holds > file.len() as u64);
    let (mut earlier, mut reader) = (ledger.holder(), ledger.holder());
    let mut cx = Context::from_waker(Waker::noop());
    let drawn = std::pin::pin!(earlier.draw(0, 1)).as_mut().poll(&mut cx);
    assert_eq!(drawn, Poll::Ready(Admission::Held), "the earliest line alone");

    let mut chunked =
        ChunkedScanner::new(CopyScanner::new(), file.len() as u64, options(chunk_size));
    let (mut events, mut waited, mut gave_up) = (Vec::new(), false, false);
    while !chunked.at_end() {
        // Once the reader holds its line, an earlier one asks for room,
        // handing its holder back once admitted.
        let mut asking = (waited && !gave_up && reader.held() > 0).then(|| {
            let mut asker = ledger.holder();
            let mut draw = Box::pin(async move { (asker.draw(0, 1).await, asker) });
            assert!(draw.as_mut().poll(&mut cx).is_pending(), "the reader holds the room");
            draw
        });
        let mut next = std::pin::pin!(chunked.next_chunk_folding(
            &source,
            None,
            Some(Drawing::held(&mut reader))
        ));
        let mut chunk = loop {
            match next.as_mut().poll(&mut cx) {
                Poll::Ready(chunk) => break chunk.unwrap(),
                // The reader gave its line up and waits behind the earlier
                // one, which is admitted now and done with its line.
                Poll::Pending if asking.is_some() => {
                    let Poll::Ready((admitted, asker)) =
                        asking.take().unwrap().as_mut().poll(&mut cx)
                    else {
                        panic!("the earlier line waits on the reader that gave its own up");
                    };
                    assert_eq!(admitted, Admission::Held);
                    gave_up = true;
                    drop(asker);
                }
                // The reader waits behind the earlier line until it is done.
                Poll::Pending => {
                    assert!(!waited, "it waits behind the earlier line once");
                    waited = true;
                    earlier.release();
                }
            }
        };
        while let Some(event) = chunk.next_event().unwrap() {
            events.push(format!("{event:?}"));
        }
        assert!(asking.is_none(), "the reader drew without giving its line up");
    }
    assert!(waited && gave_up, "waited {waited}, gave up {gave_up}");
    assert_eq!(events, reference);
    let read = source.read.load(std::sync::atomic::Ordering::SeqCst);
    // The chunk dropped while it waited, and the line past its chunk.
    let again = file.len() + 2 * chunk_size;
    assert!(read > again, "the line read again: {read} bytes of {}", file.len());
}
