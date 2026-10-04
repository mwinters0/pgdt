//! End-to-end scanner tests over both the hand-written edge-case dump and the
//! generated `fixtures/` tree.

use std::fmt::Write as _;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use pgdump_query::cache::CacheMode;
use pgdump_query::copy::{decode_field, encode_field, split_fields};
use pgdump_query::{
    DEFAULT_MEMORY_BUDGET, Event, LocalFileSource, Parallelism, RowEndingRefusal, ScanOptions,
    StatisticsRequest, build_index, map_file, scan,
};

mod common;
use common::{edge_cases, edge_cases_fixture, types_fixture};

/// Render the whole event stream, decoded, as stable text.
async fn render(path: &Path, chunk_size: usize) -> String {
    let source = LocalFileSource::open(path).unwrap();
    let options = ScanOptions { chunk_size_bytes: chunk_size, ..Default::default() };
    let mut out = String::new();

    scan(&source, &options, |event| {
        match event {
            Event::CopyStart(start) => {
                let cols = if start.header.columns.is_empty() {
                    "<none>".to_string()
                } else {
                    start.header.columns.join(", ")
                };
                let _ = writeln!(
                    out,
                    "START {} [{}] header@{} data@{}",
                    start.header.qualified_name(),
                    cols,
                    start.header_offset,
                    start.data_offset,
                );
            }
            Event::Row(row) => {
                let fields: Vec<String> = split_fields(row.raw)
                    .map(|f| match decode_field(f).unwrap() {
                        Some(v) => format!("{v:?}"),
                        None => "NULL".to_string(),
                    })
                    .collect();
                let _ = writeln!(out, "  ROW {} @{} {}", row.index, row.offset, fields.join(" | "));
            }
            Event::CopyEnd(end) => {
                let _ = writeln!(
                    out,
                    "END rows={} terminator@{} end@{}",
                    end.row_count, end.terminator_offset, end.end_offset,
                );
            }
            // DDL/comment/meta-command lines outside a COPY block —
            // `crate::map::Builder`'s input, not the scanner's own concern.
            Event::Line(_) | Event::DollarQuoteEnd(_) => {}
            Event::LargeObjectStart(start) => {
                let _ = writeln!(out, "LOSTART @{}", start.start_offset);
            }
            Event::LargeObjectEnd(end) => {
                let _ = writeln!(out, "LOEND @{}", end.end_offset);
            }
        }
        ControlFlow::Continue(())
    })
    .await
    .unwrap();

    out
}

#[tokio::test]
async fn edge_case_dump_event_stream() {
    insta::assert_snapshot!(render(&edge_cases(), 1 << 20).await);
}

/// The scanner carries state across buffer refills, so the event stream must
/// not depend on where chunk boundaries land. Chunk size 1 forces a boundary
/// between every pair of bytes.
#[tokio::test]
async fn event_stream_is_independent_of_chunk_size() {
    let reference = render(&edge_cases(), 1 << 20).await;
    for chunk_size in [1, 2, 3, 7, 13, 64, 511, 4096] {
        let got = render(&edge_cases(), chunk_size).await;
        assert_eq!(got, reference, "chunk_size {chunk_size} produced a different event stream");
    }
}

#[tokio::test]
async fn generated_fixtures_have_the_expected_structure() {
    for version in [13, 16, 18] {
        let source = LocalFileSource::open(edge_cases_fixture(version, "default")).unwrap();
        let index = build_index(&source, &ScanOptions::default()).await.unwrap();

        let summary: Vec<(String, String, u64)> = index
            .blocks()
            .map(|b| (b.header.qualified_name(), b.header.columns.join(","), b.row_count))
            .collect();

        let expected: Vec<(String, String, u64)> = [
            ("logs.events", "event_id,widget_id,message,logged_at", 3),
            ("public.dropped_column", "id,keep_me,also_keep", 2),
            ("public.empty_table", "id,value", 0),
            ("public.escapes", "codepoint,value", 132),
            ("public.generated_column", "id,a,b", 2),
            ("public.widgets", "id,name,description,is_active,created_at", 5),
        ]
        .into_iter()
        .map(|(name, cols, rows)| (name.to_string(), cols.to_string(), rows))
        .collect();

        assert_eq!(summary, expected, "pg_dump {version} default fixture");
        assert_eq!(index.total_rows(), 144);
        assert_eq!(
            index.scanned_through,
            std::fs::metadata(edge_cases_fixture(version, "default")).unwrap().len()
        );
    }
}

/// The offsets an index records must actually point at what it claims.
#[tokio::test]
async fn recorded_offsets_address_the_right_bytes() {
    for version in [13, 16, 18] {
        let path = edge_cases_fixture(version, "default");
        let bytes = std::fs::read(&path).unwrap();
        let source = LocalFileSource::open(&path).unwrap();
        let index = build_index(&source, &ScanOptions::default()).await.unwrap();

        for block in index.blocks() {
            let header = &bytes[block.header_offset as usize..block.data_offset as usize];
            assert!(header.starts_with(b"COPY "), "header_offset must land on `COPY `");
            assert!(header.ends_with(b"\n"), "data_offset must land just past the newline");

            let terminator = &bytes[block.terminator_offset as usize..block.end_offset as usize];
            assert_eq!(terminator, b"\\.\n", "terminator_offset must land on the `\\.` line");

            let data = &bytes[block.data_offset as usize..block.terminator_offset as usize];
            let rows = if data.is_empty() { 0 } else { data.split(|&b| b == b'\n').count() - 1 };
            assert_eq!(rows as u64, block.row_count);
        }
    }
}

/// `--schema-only` and `--inserts` dumps carry no COPY blocks at all; the
/// scanner must find nothing rather than misfire on the SQL.
#[tokio::test]
async fn dumps_without_copy_blocks_yield_no_blocks() {
    for version in [13, 16, 18] {
        for variant in ["schema-only", "inserts", "column-inserts"] {
            let source = LocalFileSource::open(edge_cases_fixture(version, variant)).unwrap();
            let index = build_index(&source, &ScanOptions::default()).await.unwrap();
            assert!(index.blocks().next().is_none(), "pg_dump {version} {variant}");
        }
    }
}

#[tokio::test]
async fn data_only_dumps_carry_every_block() {
    for version in [13, 16, 18] {
        let source = LocalFileSource::open(edge_cases_fixture(version, "data-only")).unwrap();
        let index = build_index(&source, &ScanOptions::default()).await.unwrap();
        assert_eq!(index.blocks().count(), 6, "pg_dump {version} data-only");
        assert_eq!(index.total_rows(), 144);
    }
}

/// Round-trip check against real PostgreSQL output: `public.escapes` holds one
/// row per codepoint (`chr(n)`), so the decoder is compared against a value
/// the test computes itself rather than a hand-transcribed literal that could
/// bake in the same misreading twice. Covers the escapes pg_dump emits
/// (`\b \t \n \v \f \r \\`), the raw control bytes it does not escape, and
/// 2-, 3- and 4-byte UTF-8.
///
/// Also proves the full on-disk-bytes round trip (`raw -> decode_field ->
/// encode_field -> raw`), not just the decoded-text one above: `encode_field`
/// re-escaping each decoded field must reproduce the exact bytes `pg_dump`
/// wrote, for every codepoint in the table.
#[tokio::test]
async fn copy_text_escaping_round_trips_through_postgres() {
    for version in [13, 16, 18] {
        let source = LocalFileSource::open(edge_cases_fixture(version, "default")).unwrap();
        let mut in_escapes = false;
        let mut checked = 0usize;

        scan(&source, &ScanOptions::default(), |event| {
            match event {
                Event::CopyStart(start) => in_escapes = start.header.matches("public.escapes"),
                Event::CopyEnd(_) => in_escapes = false,
                Event::Row(row) if in_escapes => {
                    let raw_fields: Vec<&[u8]> = split_fields(row.raw).collect();
                    let fields: Vec<Option<String>> = raw_fields
                        .iter()
                        .map(|f| decode_field(f).unwrap().map(|v| v.into_owned()))
                        .collect();
                    assert_eq!(fields.len(), 2, "pg_dump {version} escapes row {}", row.index);

                    let codepoint: u32 = fields[0].as_deref().unwrap().parse().unwrap();
                    let expected = char::from_u32(codepoint).unwrap().to_string();
                    assert_eq!(
                        fields[1].as_deref(),
                        Some(expected.as_str()),
                        "pg_dump {version} codepoint {codepoint}",
                    );

                    for (raw, decoded) in raw_fields.iter().zip(&fields) {
                        assert_eq!(
                            encode_field(decoded.as_deref()),
                            *raw,
                            "pg_dump {version} codepoint {codepoint}",
                        );
                    }
                    checked += 1;
                }
                Event::Row(_) => {}
                Event::Line(_) | Event::DollarQuoteEnd(_) => {}
                Event::LargeObjectStart(_) | Event::LargeObjectEnd(_) => {}
            }
            ControlFlow::Continue(())
        })
        .await
        .unwrap();

        assert_eq!(checked, 132, "pg_dump {version}");
    }
}

#[tokio::test]
async fn scanning_can_stop_early() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let mut starts = 0;
    scan(&source, &ScanOptions::default(), |event| {
        if let Event::CopyStart(_) = event {
            starts += 1;
            if starts == 2 {
                return ControlFlow::Break(());
            }
        }
        ControlFlow::Continue(())
    })
    .await
    .unwrap();
    assert_eq!(starts, 2);
}

#[tokio::test]
async fn empty_file_scans_clean() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.sql");
    std::fs::write(&path, b"").unwrap();
    let source = LocalFileSource::open(&path).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    assert!(index.blocks().next().is_none());
    assert_eq!(index.scanned_through, 0);
}

#[tokio::test]
async fn unterminated_copy_block_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("truncated.sql");
    std::fs::write(&path, b"COPY public.t (a) FROM stdin;\n1\n2\n").unwrap();
    let source = LocalFileSource::open(&path).unwrap();
    let err = build_index(&source, &ScanOptions::default()).await.unwrap_err();
    assert!(
        matches!(err, pgdump_query::Error::UnterminatedCopyBlock { .. }),
        "unexpected error: {err}"
    );
}

/// A final line with no trailing newline still terminates the block.
#[tokio::test]
async fn missing_trailing_newline_is_tolerated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("no-final-newline.sql");
    std::fs::write(&path, b"COPY public.t (a) FROM stdin;\n1\n\\.").unwrap();
    let source = LocalFileSource::open(&path).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    let blocks: Vec<_> = index.blocks().collect();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].row_count, 1);
}

#[tokio::test]
async fn crlf_line_endings_are_tolerated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crlf.sql");
    std::fs::write(&path, b"COPY public.t (a, b) FROM stdin;\r\n1\tone\r\n\\.\r\n").unwrap();
    let source = LocalFileSource::open(&path).unwrap();
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    scan(&source, &ScanOptions::default(), |event| {
        if let Event::Row(row) = event {
            rows.push(
                split_fields(row.raw)
                    .map(|f| decode_field(f).unwrap().map(|v| v.into_owned()))
                    .collect(),
            );
        }
        ControlFlow::Continue(())
    })
    .await
    .unwrap();
    assert_eq!(rows, vec![vec![Some("1".to_string()), Some("one".to_string())]]);
}

/// Every block's table and decoded rows, as the serial scanner reads them.
async fn decoded_blocks(path: &Path) -> Vec<(String, Vec<Vec<Option<String>>>)> {
    let source = LocalFileSource::open(path).unwrap();
    let mut blocks: Vec<(String, Vec<Vec<Option<String>>>)> = Vec::new();
    scan(&source, &ScanOptions::default(), |event| {
        match event {
            Event::CopyStart(start) => blocks.push((start.header.qualified_name(), Vec::new())),
            Event::Row(row) => blocks.last_mut().unwrap().1.push(
                split_fields(row.raw)
                    .map(|f| decode_field(f).unwrap().map(|v| v.into_owned()))
                    .collect(),
            ),
            _ => {}
        }
        ControlFlow::Continue(())
    })
    .await
    .unwrap();
    blocks
}

/// A mapping pass read serially, and one split four ways at a size that cuts
/// a fixture's blocks into pieces.
fn serial_and_split() -> [ScanOptions; 2] {
    [
        ScanOptions::default(),
        ScanOptions {
            chunk_size_bytes: 64,
            parallelism: Parallelism::workers(4, DEFAULT_MEMORY_BUDGET),
            ..ScanOptions::default()
        },
    ]
}

/// **A dump converted to CR LF reads as the dump it was converted from**, and
/// one row in it ending otherwise than its block's first is refused where a
/// restore refuses it, serially and split (I91): a stray CR LF in an LF dump
/// as a literal carriage return, a stray LF in a CR LF one as a literal
/// newline, each naming `COPY`'s line.
#[tokio::test]
async fn a_crlf_conversion_reads_alike_and_a_stray_ending_is_refused() {
    let lf = std::fs::read(types_fixture(16, "default")).unwrap();
    let crlf: Vec<u8> = lf.split_inclusive(|&b| b == b'\n').fold(Vec::new(), |mut out, line| {
        match line.strip_suffix(b"\n") {
            Some(head) => out.extend_from_slice(&[head, b"\r\n"].concat()),
            None => out.extend_from_slice(line),
        }
        out
    });
    let dir = tempfile::tempdir().unwrap();
    let (lf_path, crlf_path) = (dir.path().join("lf.sql"), dir.path().join("crlf.sql"));
    std::fs::write(&lf_path, &lf).unwrap();
    std::fs::write(&crlf_path, &crlf).unwrap();

    let expected = decoded_blocks(&lf_path).await;
    assert!(expected.iter().filter(|(_, rows)| rows.len() >= 2).count() > 5);
    assert_eq!(decoded_blocks(&crlf_path).await, expected);
    let counts = |index: &pgdump_query::DumpIndex| -> Vec<(String, u64)> {
        index.blocks().map(|b| (b.header.qualified_name(), b.row_count)).collect()
    };
    let expected_counts: Vec<(String, u64)> =
        expected.iter().map(|(table, rows)| (table.clone(), rows.len() as u64)).collect();
    for options in serial_and_split() {
        let source = LocalFileSource::open(&crlf_path).unwrap();
        let run = map_file(&source, &options, &CacheMode::DISABLED, &StatisticsRequest::DATA)
            .await
            .unwrap();
        assert_eq!(counts(&run.index), expected_counts, "{:?}", options.parallelism);
    }

    // The second row of the first block holding two, its ending flipped.
    let (table, _) = expected.iter().find(|(_, rows)| rows.len() >= 2).unwrap();
    for (file, why) in
        [(&lf, RowEndingRefusal::LiteralCarriageReturn), (&crlf, RowEndingRefusal::LiteralNewline)]
    {
        let header = format!("COPY {table} ");
        let at = file.windows(header.len()).position(|w| w == header.as_bytes()).unwrap();
        let first_row = at + memchr_lf(&file[at..]) + 1;
        let second_row = first_row + memchr_lf(&file[first_row..]) + 1;
        let lf_at = second_row + memchr_lf(&file[second_row..]);
        let mut stray = file.clone();
        if why == RowEndingRefusal::LiteralCarriageReturn {
            stray.insert(lf_at, b'\r');
        } else {
            stray.remove(lf_at - 1);
        }
        let path = dir.path().join("stray.sql");
        std::fs::write(&path, &stray).unwrap();
        for options in serial_and_split() {
            let source = LocalFileSource::open(&path).unwrap();
            match map_file(&source, &options, &CacheMode::DISABLED, &StatisticsRequest::DATA).await
            {
                Err(pgdump_query::Error::RowEndingRefused {
                    table: t,
                    line,
                    line_offset,
                    refusal,
                }) => {
                    assert_eq!(
                        (t.as_str(), line, line_offset, refusal),
                        (table.as_str(), 2, second_row as u64, why),
                        "{:?}",
                        options.parallelism
                    );
                }
                other => panic!("{why:?} under {:?}: {:?}", options.parallelism, other.map(|_| ())),
            }
        }
    }
}

fn memchr_lf(bytes: &[u8]) -> usize {
    bytes.iter().position(|&b| b == b'\n').unwrap()
}

#[tokio::test]
async fn line_length_limit_is_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("longline.sql");
    let mut content = b"COPY public.t (a) FROM stdin;\n".to_vec();
    content.extend(std::iter::repeat_n(b'x', 4096));
    content.extend_from_slice(b"\n\\.\n");
    std::fs::write(&path, &content).unwrap();

    let source = LocalFileSource::open(&path).unwrap();
    let options = ScanOptions { chunk_size_bytes: 64, max_line_bytes: 512, ..Default::default() };
    let err = build_index(&source, &options).await.unwrap_err();
    assert!(matches!(err, pgdump_query::Error::LineTooLong { .. }), "unexpected error: {err}");
}

/// A `BEGIN;`/`COMMIT;`-wrapped large-object region (I12) is recognized at
/// its opening line and skipped: no `Event::Line` for anything in between,
/// including the `lo_open`/`lowrite`/`lo_close` calls themselves — the same
/// "contents are never parsed" treatment `Event::Row` gets inside a `COPY`
/// block, but with nothing surfaced at all rather than one event per line,
/// since nothing downstream wants those lines
/// (`docs/design/decisions.md`, "D33").
#[tokio::test]
async fn large_object_region_is_skipped_without_surfacing_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lo.sql");
    let text = "SELECT 1;\n\
                BEGIN;\n\
                SELECT pg_catalog.lo_open('16490', 131072);\n\
                SELECT pg_catalog.lowrite(0, '\\x48656c6c6f');\n\
                SELECT pg_catalog.lo_close(0);\n\
                COMMIT;\n\
                SELECT 2;\n";
    std::fs::write(&path, text).unwrap();
    let source = LocalFileSource::open(&path).unwrap();

    let mut lines = Vec::new();
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    scan(&source, &ScanOptions::default(), |event| {
        match event {
            Event::Line(line) => lines.push(String::from_utf8_lossy(line.raw).into_owned()),
            Event::LargeObjectStart(start) => starts.push(start.start_offset),
            Event::LargeObjectEnd(end) => ends.push(end.end_offset),
            _ => {}
        }
        ControlFlow::Continue(())
    })
    .await
    .unwrap();

    assert_eq!(
        lines,
        vec!["SELECT 1;".to_string(), "SELECT 2;".to_string()],
        "nothing between BEGIN; and COMMIT; is surfaced as a line"
    );
    assert_eq!(starts, vec![text.find("BEGIN;").unwrap() as u64]);
    let commit_end = (text.find("COMMIT;\n").unwrap() + "COMMIT;\n".len()) as u64;
    assert_eq!(ends, vec![commit_end]);
}

/// A large-object region with no closing `COMMIT;` is an error, the same way
/// an unterminated `COPY` block is.
#[tokio::test]
async fn unterminated_large_object_region_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("truncated_lo.sql");
    std::fs::write(&path, b"BEGIN;\nSELECT pg_catalog.lo_open('1', 131072);\n").unwrap();
    let source = LocalFileSource::open(&path).unwrap();
    let err = build_index(&source, &ScanOptions::default()).await.unwrap_err();
    assert!(
        matches!(err, pgdump_query::Error::UnterminatedLargeObjectRegion { .. }),
        "unexpected error: {err}"
    );
}

/// Real `pg_dump` v17+ output splits large-object data into one `BLOBS`
/// archive entry per object, each with its own `BEGIN;`/`COMMIT;` (I12) — the
/// scanner reports both pairs, not one merged region: merging them into one
/// `Data` span is `crate::map`'s job (`tests/map.rs`), not the scanner's.
#[tokio::test]
async fn v17_plus_reports_one_large_object_region_per_blobs_entry() {
    let source = LocalFileSource::open(fixture18_objects("default")).unwrap();
    let mut starts = 0;
    let mut ends = 0;
    scan(&source, &ScanOptions::default(), |event| {
        match event {
            Event::LargeObjectStart(_) => starts += 1,
            Event::LargeObjectEnd(_) => ends += 1,
            _ => {}
        }
        ControlFlow::Continue(())
    })
    .await
    .unwrap();
    assert_eq!((starts, ends), (2, 2), "fixtures/18/objects/default.sql has two large objects");
}

fn fixture18_objects(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/18/objects").join(format!("{name}.sql"))
}

/// `Event::DollarQuoteEnd` reports **where** a dollar-quoted region closed
/// and nothing else: the lines of the region — including the one that closes
/// it, which carries the statement's own `;` — stay unsurfaced, so L1's event
/// contract still says nothing about DDL text
/// (`docs/design/decisions.md`, "D30").
#[tokio::test]
async fn dollar_quote_end_reports_a_position_and_surfaces_no_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fn.sql");
    let text = "CREATE FUNCTION f() RETURNS void\n    AS $$\nbody line\n$$;\nSELECT 1;\n";
    std::fs::write(&path, text).unwrap();
    let source = LocalFileSource::open(&path).unwrap();

    let mut lines = Vec::new();
    let mut ends = Vec::new();
    scan(&source, &ScanOptions::default(), |event| {
        match event {
            Event::Line(line) => lines.push(String::from_utf8_lossy(line.raw).into_owned()),
            Event::DollarQuoteEnd(end) => ends.push(end.offset),
            _ => {}
        }
        std::ops::ControlFlow::Continue(())
    })
    .await
    .unwrap();

    assert_eq!(
        lines,
        vec!["CREATE FUNCTION f() RETURNS void".to_string(), "SELECT 1;".to_string()],
        "neither `AS $$`, the body, nor the closing `$$;` is surfaced as a line"
    );
    // One region, closing just past the `$$;` line — which is exactly where
    // the next span would start.
    let closing_line_end = (text.find("$$;\n").unwrap() + "$$;\n".len()) as u64;
    assert_eq!(ends, vec![closing_line_end]);
}

/// A region that opens and closes on the same line still reports its end —
/// the `had a tag` / `has a tag` transition is not the signal, "touched and
/// now closed" is.
#[tokio::test]
async fn a_single_line_dollar_quoted_region_reports_its_end() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("inline.sql");
    let first = "CREATE FUNCTION f() RETURNS int AS $$ SELECT 1 $$;\n";
    std::fs::write(&path, format!("{first}SELECT 2;\n")).unwrap();
    let source = LocalFileSource::open(&path).unwrap();

    let mut ends = Vec::new();
    scan(&source, &ScanOptions::default(), |event| {
        if let Event::DollarQuoteEnd(end) = event {
            ends.push(end.offset);
        }
        std::ops::ControlFlow::Continue(())
    })
    .await
    .unwrap();
    assert_eq!(ends, vec![first.len() as u64], "just past the line the region closed on");
}

/// The `COPY` headers `scan` reports over `text`, by table name.
async fn headers_of(text: &str) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dump.sql");
    std::fs::write(&path, text).unwrap();
    let source = LocalFileSource::open(&path).unwrap();
    let mut headers = Vec::new();
    scan(&source, &ScanOptions::default(), |event| {
        if let Event::CopyStart(start) = event {
            headers.push(start.header.qualified_name());
        }
        ControlFlow::Continue(())
    })
    .await
    .unwrap();
    headers
}

/// A `$` outside a dollar-quoted body opens one only where psql's lexer
/// would (`crate::lex`): never inside a literal, a quoted identifier or a
/// comment, each carried across lines, a plain literal's backslash read
/// under the dump's own `standard_conforming_strings` (I50). Before, any of
/// these swallowed every line after it, and both tables' data with them.
#[tokio::test]
async fn a_dollar_inside_a_literal_identifier_or_comment_opens_no_body() {
    let blocks =
        "COPY public.a (id) FROM stdin;\n1\n\\.\n\nCOPY public.b (id) FROM stdin;\n2\n\\.\n";
    for ddl in [
        "COMMENT ON TABLE public.a IS 'costs $$ here';",
        "COMMENT ON TABLE public.a IS E'it\\'s $$ here';",
        "COMMENT ON TABLE public.a IS 'costs\n$x$ across\nlines';",
        "CREATE TABLE public.\"a$$\" (id integer);",
        "-- note: $x$",
        "/* a /* nested */ $$ still\n a comment */",
        "SET standard_conforming_strings = off;\nCOMMENT ON TABLE public.a IS 'it\\'s $$ here';",
        "COMMENT ON TABLE public.a IS E'a' -- continued\n'\\'$$';",
    ] {
        let text = format!("SET standard_conforming_strings = on;\n{ddl}\n\n{blocks}");
        assert_eq!(headers_of(&text).await, ["public.a", "public.b"], "after {ddl:?}");
    }
    // A quoted table name holding `$$` is still a header.
    let text =
        "COPY public.\"a$$\" (id) FROM stdin;\n1\n\\.\nCOPY public.b (id) FROM stdin;\n\\.\n";
    assert_eq!(headers_of(text).await, ["public.a$$", "public.b"]);
}

/// A line inside a literal, an identifier or a comment that spans lines is
/// that region's content, never structure, whatever it looks like.
#[tokio::test]
async fn a_header_shaped_line_inside_a_quoted_region_is_not_a_block() {
    for ddl in [
        "COMMENT ON TABLE public.a IS 'first\nCOPY public.z (id) FROM stdin;\n';",
        "COMMENT ON TABLE public.a IS 'first\nBEGIN;\n';",
        "/*\nCOPY public.z (id) FROM stdin;\n*/",
        "CREATE TABLE public.\"x\nCOPY public.z (id) FROM stdin;\n\" (id integer);",
    ] {
        let text = format!("{ddl}\nCOPY public.a (id) FROM stdin;\n1\n\\.\n");
        assert_eq!(headers_of(&text).await, ["public.a"], "after {ddl:?}");
    }
}
