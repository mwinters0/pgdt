use super::*;

/// What [`drive`] saw: the events rendered as text, the high-water mark of
/// the carry, and the bytes handed to the scanner over every pass.
struct Driven {
    events: Vec<String>,
    high_water: usize,
    scanned: usize,
}

/// Drive [`CopyScanner`] over `file` in `chunk_size` pieces exactly as
/// the three read loops do.
fn drive(file: &[u8], chunk_size: usize) -> Driven {
    let mut scanner = CopyScanner::new();
    let mut carry = ChunkCarry::new();
    let mut events = Vec::new();
    let mut read_pos = 0usize;
    let mut high_water = 0usize;
    let mut scanned = 0usize;
    loop {
        let want = chunk_size.min(file.len() - read_pos);
        let chunk = &file[read_pos..read_pos + want];
        read_pos += want;
        let eof = read_pos >= file.len();

        carry.absorb(chunk);
        for pass in ChunkCarry::PASSES {
            let (span, span_eof) = carry.span(pass, chunk, eof);
            scanned += span.len();
            while let Some(event) = scanner.next_event(span, span_eof).unwrap() {
                events.push(format!("{event:?}"));
            }
            carry.consumed(pass, chunk, scanner.take_consumed());
        }
        high_water = high_water.max(carry.len());

        if eof {
            return Driven { events, high_water, scanned };
        }
    }
}

/// A carry holding `line`, reached the way a read loop reaches one: a chunk
/// the scanner could not finish, whose tail it kept.
fn carrying(line: &[u8]) -> ChunkCarry {
    let mut carry = ChunkCarry::new();
    carry.absorb(line);
    carry.consumed(ChunkPass::InPlace, line, 0);
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

/// **A raw CR ending a line keeps its byte only where the line ends inside
/// a quoted literal or identifier** (I90), an `E''` one's escaped CR
/// included; a statement's line, a comment, a block comment's line and a
/// `COPY` row lose it, wherever a chunk boundary falls.
#[test]
fn a_raw_cr_ending_a_line_is_kept_only_inside_a_quoted_region() {
    fn lines(file: &[u8], chunk_size: usize) -> Vec<Vec<u8>> {
        let mut scanner = CopyScanner::new();
        let mut carry = ChunkCarry::new();
        let (mut out, mut read_pos) = (Vec::new(), 0usize);
        loop {
            let want = chunk_size.min(file.len() - read_pos);
            let chunk = &file[read_pos..read_pos + want];
            read_pos += want;
            let eof = read_pos >= file.len();
            carry.absorb(chunk);
            for pass in ChunkCarry::PASSES {
                let (span, span_eof) = carry.span(pass, chunk, eof);
                while let Some(event) = scanner.next_event(span, span_eof).unwrap() {
                    match event {
                        Event::Line(line) => out.push(line.raw.to_vec()),
                        Event::Row(row) => out.push(row.raw.to_vec()),
                        _ => {}
                    }
                }
                carry.consumed(pass, chunk, scanner.take_consumed());
            }
            if eof {
                return out;
            }
        }
    }
    let file: &[u8] = b"-- comment\r\nCREATE TYPE e AS ENUM ('a\r\nb', E'c\\\r\nd');\r\n\
        CREATE TABLE \"t\r\nu\" (/* x\r\n */ m e);\r\n\
        COPY public.t (m) FROM stdin;\r\na\\r\\nb\r\n\\.\r\n";
    let expected: [&[u8]; 8] = [
        b"-- comment",
        b"CREATE TYPE e AS ENUM ('a\r",
        b"b', E'c\\\r",
        b"d');",
        b"CREATE TABLE \"t\r",
        b"u\" (/* x",
        b" */ m e);",
        b"a\\r\\nb",
    ];
    for chunk_size in [1usize, 2, 7, 4096] {
        let got = lines(file, chunk_size);
        assert_eq!(got, expected.map(<[u8]>::to_vec), "chunk_size {chunk_size}");
    }
}

/// Every row's bytes `scanner` surfaces over `file` in `chunk_size` chunks,
/// or the error it stops on.
fn rows_of(mut scanner: CopyScanner, file: &[u8], chunk_size: usize) -> Result<Vec<Vec<u8>>> {
    let mut carry = ChunkCarry::new();
    let (mut out, mut read_pos) = (Vec::new(), 0usize);
    loop {
        let want = chunk_size.min(file.len() - read_pos);
        let chunk = &file[read_pos..read_pos + want];
        read_pos += want;
        let eof = read_pos >= file.len();
        carry.absorb(chunk);
        for pass in ChunkCarry::PASSES {
            let (span, span_eof) = carry.span(pass, chunk, eof);
            while let Some(event) = scanner.next_event(span, span_eof)? {
                if let Event::Row(row) = event {
                    out.push(row.raw.to_vec());
                }
            }
            carry.consumed(pass, chunk, scanner.take_consumed());
        }
        if eof {
            return Ok(out);
        }
    }
}

/// A row's ending is its first CR no backslash escapes, an escaped one —
/// behind an odd run of backslashes — being data.
#[test]
fn a_row_ends_at_its_first_unescaped_cr() {
    let cases: [(&[u8], RowEnding); 8] = [
        (b"1\ta", RowEnding::Lf),
        (b"1\ta\r", RowEnding::CrLf),
        (b"1\ta\\\r", RowEnding::Lf),
        (b"1\ta\\\\\r", RowEnding::CrLf),
        (b"1\\\r\ta\r", RowEnding::CrLf),
        (b"1\ra\r", RowEnding::Cr),
        (b"1\\\\\ra", RowEnding::Cr),
        (b"\r", RowEnding::CrLf),
    ];
    for (line, ending) in cases {
        assert_eq!(RowEnding::of(line), ending, "{:?}", String::from_utf8_lossy(line));
    }
}

/// **A row ending otherwise than its block's first row is refused as a
/// restore refuses it** (I91), naming the table and `COPY`'s line,
/// wherever a chunk boundary falls; the `\.` line ends a block whatever
/// its rows end in, and the next block fixes its own.
#[test]
fn a_row_ending_otherwise_than_its_block_s_first_is_refused() {
    let refused: [(&[u8], u64, RowEndingRefusal); 4] = [
        (b"1\n2\r\n3\n", 2, RowEndingRefusal::LiteralCarriageReturn),
        (b"1\n2\r3\n", 2, RowEndingRefusal::LiteralCarriageReturn),
        (b"1\r\n2\r\n3\n", 3, RowEndingRefusal::LiteralNewline),
        (b"1\r\n2\r3\r\n", 2, RowEndingRefusal::LiteralCarriageReturn),
    ];
    for (rows, at, why) in refused {
        let mut file = b"COPY public.ok (a) FROM stdin;\r\n1\r\n\\.\n".to_vec();
        file.extend_from_slice(b"COPY public.t (a) FROM stdin;\n");
        let line_offset = file.len() as u64
            + rows
                .split_inclusive(|&b| b == b'\n')
                .take(at as usize - 1)
                .map(<[u8]>::len)
                .sum::<usize>() as u64;
        file.extend_from_slice(rows);
        file.extend_from_slice(b"\\.\n");
        for chunk_size in [1usize, 2, 7, 4096] {
            match rows_of(CopyScanner::new(), &file, chunk_size) {
                Err(Error::RowEndingRefused { table, line, line_offset: offset, refusal }) => {
                    assert_eq!(
                        (table.as_str(), line, offset, refusal),
                        ("public.t", at, line_offset, why),
                        "{rows:?} at chunk_size {chunk_size}"
                    );
                }
                other => panic!("{rows:?} at chunk_size {chunk_size}: {other:?}"),
            }
        }
    }
}

/// The rows a restore reads are read, each without its ending: a CR LF
/// block's, a CR a backslash escapes kept as the last field's byte, the
/// `\.` line ending either block in either form; a block whose first row
/// holds a bare CR is checked for nothing (`KD101`); and a scanner resumed
/// inside a block checks nothing, its mapping pass having checked.
#[test]
fn the_rows_a_restore_reads_are_read_without_their_endings() {
    let file: &[u8] = b"COPY public.t (a) FROM stdin;\r\n1\r\n2\\\r\r\n\\.\n\
        COPY public.u (a) FROM stdin;\n3\\\r\n4\n\\.\r\n\
        COPY public.v (a) FROM stdin;\n5\r6\n7\r\n\\.\n";
    let expected: [&[u8]; 6] = [b"1", b"2\\\r", b"3\\\r", b"4", b"5\r6", b"7"];
    for chunk_size in [1usize, 2, 7, 4096] {
        let got = rows_of(CopyScanner::new(), file, chunk_size).unwrap();
        assert_eq!(got, expected.map(<[u8]>::to_vec), "chunk_size {chunk_size}");
    }
    let interior: &[u8] = b"1\n2\r\n3\n\\.\n";
    let resumed = CopyScanner::resume(0, Some((0, 1)));
    assert_eq!(rows_of(resumed, interior, 3).unwrap(), [&b"1"[..], b"2", b"3"].map(<[u8]>::to_vec));
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

/// With nothing carried the chunk is scanned whole, where it lies: no
/// prefix is taken and the in-place span is the chunk itself.
#[test]
fn an_empty_carry_copies_nothing() {
    let mut carry = ChunkCarry::new();
    carry.absorb(b"one\ntwo\n");
    assert!(carry.is_empty());
    assert_eq!(carry.span(ChunkPass::Carry, b"one\ntwo\n", false).0, b"");
    assert_eq!(carry.span(ChunkPass::InPlace, b"one\ntwo\n", false).0, b"one\ntwo\n");
}

/// The carry takes the chunk's first line-terminated prefix and no more —
/// one line's worth of copying, whatever the chunk's length.
#[test]
fn a_carry_takes_only_the_prefix_that_completes_its_line() {
    let mut carry = carrying(b"abc");

    let chunk = b"def\nghi\njkl";
    carry.absorb(chunk);
    let (span, _) = carry.span(ChunkPass::Carry, chunk, false);
    assert_eq!(span, b"abcdef\n");
    // The scanner would consume that whole line, which is the protocol's
    // precondition for the in-place span below.
    carry.consumed(ChunkPass::Carry, chunk, span.len());
    assert_eq!(carry.span(ChunkPass::InPlace, chunk, false).0, b"ghi\njkl");
}

/// The degenerate case: a chunk with no newline in it joins the carry
/// whole, which is the growth `ScanOptions::max_line_bytes` bounds, and
/// neither pass has anything to scan until the file ends.
#[test]
fn a_chunk_with_no_newline_joins_the_carry_whole() {
    let mut carry = carrying(b"abc");

    let chunk = b"defghi";
    carry.absorb(chunk);
    assert_eq!(carry.len(), 9);
    assert_eq!(carry.span(ChunkPass::Carry, chunk, false).0, b"");
    assert_eq!(carry.span(ChunkPass::Carry, chunk, true).0, b"abcdefghi");
    assert_eq!(carry.span(ChunkPass::InPlace, chunk, false).0, b"");

    // The chunk that finishes the line hands the whole of it over once.
    carry.consumed(ChunkPass::InPlace, chunk, 0);
    let last = b"jk\nl";
    carry.absorb(last);
    assert_eq!(carry.span(ChunkPass::Carry, last, false).0, b"abcdefghijk\n");
}

/// The carry pass runs to the end of the file only when the chunk has
/// nothing left for the in-place pass — otherwise it would treat a line
/// still being carried as the file's last.
#[test]
fn the_carry_pass_is_eof_only_when_the_chunk_is_exhausted() {
    let mut carry = carrying(b"abc");

    let split = b"def\nghi";
    carry.absorb(split);
    assert!(!carry.span(ChunkPass::Carry, split, true).1);
    carry.consumed(ChunkPass::Carry, split, carry.len());
    assert!(carry.span(ChunkPass::InPlace, split, true).1);

    let whole = b"defghi";
    let mut carry = carrying(b"abc");
    carry.absorb(whole);
    assert!(carry.span(ChunkPass::Carry, whole, true).1);
}

/// An unterminated `COPY` block is still an error when the file ends
/// inside it, wherever the last chunk boundary fell — the carry pass takes
/// `eof` for exactly the case where nothing follows it.
#[test]
fn a_file_ending_inside_a_copy_block_is_still_rejected() {
    let file = b"COPY public.t (a) FROM stdin;\n1\n2\n3".to_vec();
    for chunk_size in [1usize, 2, 3, 7, 4096] {
        let mut scanner = CopyScanner::new();
        let mut carry = ChunkCarry::new();
        let mut read_pos = 0usize;
        let mut err = None;
        'file: loop {
            let want = chunk_size.min(file.len() - read_pos);
            let chunk = &file[read_pos..read_pos + want];
            read_pos += want;
            let eof = read_pos >= file.len();
            carry.absorb(chunk);
            for pass in ChunkCarry::PASSES {
                let (span, span_eof) = carry.span(pass, chunk, eof);
                loop {
                    match scanner.next_event(span, span_eof) {
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(e) => {
                            err = Some(e);
                            break 'file;
                        }
                    }
                }
                carry.consumed(pass, chunk, scanner.take_consumed());
            }
            if eof {
                break;
            }
        }
        assert!(
            matches!(err, Some(Error::UnterminatedCopyBlock { header_offset: 0 })),
            "chunk_size {chunk_size} gave {err:?}"
        );
    }
}
