//! End-to-end scanner tests over both the hand-written edge-case dump and the
//! generated `fixtures/` tree.

use std::fmt::Write as _;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use pgdump_query::copy::{decode_field, split_fields};
use pgdump_query::{Event, LocalFileSource, ScanOptions, build_index, scan};

fn edge_cases() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/edge_cases.sql")
}

fn fixture(version: u32, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(version.to_string())
        .join("edge_cases")
        .join(format!("{name}.sql"))
}

/// Render the whole event stream, decoded, as stable text.
async fn render(path: &Path, chunk_size: usize) -> String {
    let source = LocalFileSource::open(path).unwrap();
    let options = ScanOptions { chunk_size, ..Default::default() };
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
            // `crate::preamble`'s input, not the scanner's own concern.
            Event::Line(_) => {}
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
        let source = LocalFileSource::open(fixture(version, "default")).unwrap();
        let index = build_index(&source, &ScanOptions::default()).await.unwrap();

        let summary: Vec<(String, String, u64)> = index
            .blocks
            .iter()
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
            std::fs::metadata(fixture(version, "default")).unwrap().len()
        );
    }
}

/// The offsets an index records must actually point at what it claims.
#[tokio::test]
async fn recorded_offsets_address_the_right_bytes() {
    for version in [13, 16, 18] {
        let path = fixture(version, "default");
        let bytes = std::fs::read(&path).unwrap();
        let source = LocalFileSource::open(&path).unwrap();
        let index = build_index(&source, &ScanOptions::default()).await.unwrap();

        for block in &index.blocks {
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
            let source = LocalFileSource::open(fixture(version, variant)).unwrap();
            let index = build_index(&source, &ScanOptions::default()).await.unwrap();
            assert!(index.blocks.is_empty(), "pg_dump {version} {variant}");
        }
    }
}

#[tokio::test]
async fn data_only_dumps_carry_every_block() {
    for version in [13, 16, 18] {
        let source = LocalFileSource::open(fixture(version, "data-only")).unwrap();
        let index = build_index(&source, &ScanOptions::default()).await.unwrap();
        assert_eq!(index.blocks.len(), 6, "pg_dump {version} data-only");
        assert_eq!(index.total_rows(), 144);
    }
}

/// Round-trip check against real PostgreSQL output: `public.escapes` holds one
/// row per codepoint (`chr(n)`), so the decoder is compared against a value
/// the test computes itself rather than a hand-transcribed literal that could
/// bake in the same misreading twice. Covers the escapes pg_dump emits
/// (`\b \t \n \v \f \r \\`), the raw control bytes it does not escape, and
/// 2-, 3- and 4-byte UTF-8.
#[tokio::test]
async fn copy_text_escaping_round_trips_through_postgres() {
    for version in [13, 16, 18] {
        let source = LocalFileSource::open(fixture(version, "default")).unwrap();
        let mut in_escapes = false;
        let mut checked = 0usize;

        scan(&source, &ScanOptions::default(), |event| {
            match event {
                Event::CopyStart(start) => in_escapes = start.header.matches("public.escapes"),
                Event::CopyEnd(_) => in_escapes = false,
                Event::Row(row) if in_escapes => {
                    let fields: Vec<Option<String>> = split_fields(row.raw)
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
                    checked += 1;
                }
                Event::Row(_) => {}
                Event::Line(_) => {}
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
    assert!(index.blocks.is_empty());
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
    assert_eq!(index.blocks.len(), 1);
    assert_eq!(index.blocks[0].row_count, 1);
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

#[tokio::test]
async fn line_length_limit_is_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("longline.sql");
    let mut content = b"COPY public.t (a) FROM stdin;\n".to_vec();
    content.extend(std::iter::repeat_n(b'x', 4096));
    content.extend_from_slice(b"\n\\.\n");
    std::fs::write(&path, &content).unwrap();

    let source = LocalFileSource::open(&path).unwrap();
    let options = ScanOptions { chunk_size: 64, max_line_bytes: 512 };
    let err = build_index(&source, &options).await.unwrap_err();
    assert!(matches!(err, pgdump_query::Error::LineTooLong { .. }), "unexpected error: {err}");
}
