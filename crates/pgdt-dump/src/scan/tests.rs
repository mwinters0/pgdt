use super::*;
use bytes::Bytes;

/// Every event `scanner` reads of `file` in `chunk_size` chunks, through
/// [`ChunkedScanner`] as a read loop drives it, handed to `on_event`.
fn each_event(
    scanner: CopyScanner,
    file: &[u8],
    chunk_size: usize,
    mut on_event: impl FnMut(Event<'_>),
) -> Result<()> {
    let options = ScanOptions { chunk_size_bytes: chunk_size, ..ScanOptions::default() };
    let mut chunked = ChunkedScanner::new(scanner, file.len() as u64, &options);
    let mut read_pos = 0usize;
    loop {
        let want = chunk_size.min(file.len() - read_pos);
        let mut chunk = chunked.feed(Bytes::copy_from_slice(&file[read_pos..read_pos + want]));
        read_pos += want;
        while let Some(event) = chunk.next_event()? {
            on_event(event);
        }
        if chunked.at_end() {
            return Ok(());
        }
    }
}

/// **A raw CR ending a line keeps its byte only where the line ends inside
/// a quoted literal or identifier** (I90), an `E''` one's escaped CR
/// included; a statement's line, a comment, a block comment's line and a
/// `COPY` row lose it, wherever a chunk boundary falls.
#[test]
fn a_raw_cr_ending_a_line_is_kept_only_inside_a_quoted_region() {
    fn lines(file: &[u8], chunk_size: usize) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        each_event(CopyScanner::new(), file, chunk_size, |event| match event {
            Event::Line(line) => out.push(line.raw.to_vec()),
            Event::Row(row) => out.push(row.raw.to_vec()),
            _ => {}
        })
        .unwrap();
        out
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
fn rows_of(scanner: CopyScanner, file: &[u8], chunk_size: usize) -> Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    each_event(scanner, file, chunk_size, |event| {
        if let Event::Row(row) = event {
            out.push(row.raw.to_vec());
        }
    })?;
    Ok(out)
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
