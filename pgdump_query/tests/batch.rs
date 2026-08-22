//! End-to-end tests for the row/batch layer over the hand-written
//! edge-case dump (see `tests/scan.rs` for the same fixture used at the
//! event-stream level).

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use arrow::array::{Array, RecordBatch, StringViewArray};
use pgdump_query::{BatchOptions, LocalFileSource, ScanOptions, read_table};

fn edge_cases() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/edge_cases.sql")
}

fn fixture(version: u32, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(version.to_string())
        .join(format!("{name}.sql"))
}

fn rows_of(batch: &RecordBatch) -> Vec<Vec<Option<String>>> {
    let columns: Vec<&StringViewArray> = batch
        .columns()
        .iter()
        .map(|c| c.as_any().downcast_ref::<StringViewArray>().unwrap())
        .collect();
    (0..batch.num_rows())
        .map(|row| {
            columns.iter().map(|c| c.is_valid(row).then(|| c.value(row).to_string())).collect()
        })
        .collect()
}

/// Collect every batch `read_table` produces for `table`, as decoded rows,
/// along with each batch's row count (so batch-size-limit tests can see the
/// split points, not just the flattened data).
async fn collect(
    path: &Path,
    table: &str,
    scan_options: &ScanOptions,
    batch_options: &BatchOptions,
) -> (Vec<usize>, Vec<Vec<Option<String>>>) {
    let source = LocalFileSource::open(path).unwrap();
    let mut batch_sizes = Vec::new();
    let mut rows = Vec::new();

    read_table(&source, table, scan_options, batch_options, |batch| {
        batch_sizes.push(batch.num_rows());
        rows.extend(rows_of(&batch));
        ControlFlow::Continue(())
    })
    .await
    .unwrap();

    (batch_sizes, rows)
}

fn widgets_expected() -> Vec<Vec<Option<String>>> {
    vec![
        vec![
            Some("1".into()),
            Some("alpha".into()),
            Some("a simple widget".into()),
            Some("2024-01-01 00:00:00+00".into()),
        ],
        vec![Some("2".into()), Some("beta".into()), None, Some("2024-01-02 00:00:00+00".into())],
        vec![
            Some("3".into()),
            Some("gamma".into()),
            Some("multi\nline\twith a backslash \\ inside".into()),
            None,
        ],
        vec![
            Some("4".into()),
            Some("delta".into()),
            Some(
                "contains a COPY-like phrase: COPY public.widgets (id) FROM stdin; -- not a directive"
                    .into(),
            ),
            Some("2024-01-04 00:00:00+00".into()),
        ],
        vec![
            Some("5".into()),
            Some("".into()),
            Some("empty name to the left".into()),
            Some("2024-01-05 00:00:00+00".into()),
        ],
        vec![
            Some("6".into()),
            Some("epsilon".into()),
            Some("carriage\rreturn, octal A, hex B".into()),
            Some("2024-01-06 00:00:00+00".into()),
        ],
    ]
}

#[tokio::test]
async fn widgets_table_decodes_correctly() {
    let (_, rows) =
        collect(&edge_cases(), "public.widgets", &ScanOptions::default(), &BatchOptions::default())
            .await;
    assert_eq!(rows, widgets_expected());
}

#[tokio::test]
async fn table_matching_is_bare_or_qualified() {
    for name in ["widgets", "public.widgets"] {
        let (_, rows) =
            collect(&edge_cases(), name, &ScanOptions::default(), &BatchOptions::default()).await;
        assert_eq!(rows, widgets_expected(), "matching on {name}");
    }
}

#[tokio::test]
async fn empty_table_produces_no_batches() {
    let (sizes, rows) = collect(
        &edge_cases(),
        "public.empty_table",
        &ScanOptions::default(),
        &BatchOptions::default(),
    )
    .await;
    assert!(sizes.is_empty());
    assert!(rows.is_empty());
}

#[tokio::test]
async fn unmatched_table_produces_no_batches() {
    let (sizes, rows) =
        collect(&edge_cases(), "no.such.table", &ScanOptions::default(), &BatchOptions::default())
            .await;
    assert!(sizes.is_empty());
    assert!(rows.is_empty());
}

/// A header with no explicit column list still gets a schema — placeholder
/// names sized to the first row's field count — and its rows decode
/// normally, including the row that reads `\\.` and must not be mistaken
/// for the block terminator.
#[tokio::test]
async fn header_without_column_list_gets_placeholder_schema() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let mut batches = Vec::new();
    read_table(
        &source,
        "public.no_column_list",
        &ScanOptions::default(),
        &BatchOptions::default(),
        |batch| {
            batches.push(batch);
            ControlFlow::Continue(())
        },
    )
    .await
    .unwrap();

    assert_eq!(batches.len(), 1);
    let batch = &batches[0];
    assert_eq!(
        batch.schema().fields().iter().map(|f| f.name().clone()).collect::<Vec<_>>(),
        vec!["column1".to_string()]
    );
    assert_eq!(
        rows_of(batch),
        vec![vec![Some("\\.".to_string())], vec![Some("just a value".to_string())]]
    );
}

#[tokio::test]
async fn quoted_identifiers_are_matched_and_decoded() {
    let (_, rows) = collect(
        &edge_cases(),
        "My Schema.Odd Table",
        &ScanOptions::default(),
        &BatchOptions::default(),
    )
    .await;
    assert_eq!(rows, vec![vec![Some("1".to_string()), Some("quoted identifiers".to_string())]]);
}

/// `max_rows` splits a table's rows into multiple batches at the expected
/// boundaries; the flattened data must be unaffected by where the splits
/// land.
#[tokio::test]
async fn max_rows_splits_batches() {
    for max_rows in [1, 2, 4, 100] {
        let options = BatchOptions { max_rows, max_bytes: None };
        let (sizes, rows) =
            collect(&edge_cases(), "public.widgets", &ScanOptions::default(), &options).await;
        assert_eq!(rows, widgets_expected(), "max_rows {max_rows}");
        assert_eq!(sizes.iter().sum::<usize>(), 6, "max_rows {max_rows}");
        assert!(sizes.iter().all(|&n| n <= max_rows), "max_rows {max_rows}: sizes {sizes:?}");
    }
}

/// Fields eligible for a zero-copy view (no escapes) are only zero-copy when
/// they land fully inside one read chunk; a small `chunk_size` forces most
/// fields — and the multi-byte escaped ones — through every code path
/// (zero-copy view, chunk-straddling copy, and decode-then-copy). The
/// decoded result must be identical regardless.
#[tokio::test]
async fn batch_contents_are_independent_of_chunk_size() {
    let reference =
        collect(&edge_cases(), "public.widgets", &ScanOptions::default(), &BatchOptions::default())
            .await
            .1;
    for chunk_size in [1, 2, 3, 7, 13, 64, 511, 4096] {
        let options = ScanOptions { chunk_size, ..Default::default() };
        let (_, rows) =
            collect(&edge_cases(), "public.widgets", &options, &BatchOptions::default()).await;
        assert_eq!(rows, reference, "chunk_size {chunk_size}");
    }
}

#[tokio::test]
async fn stops_early_on_break() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let options = BatchOptions { max_rows: 1, max_bytes: None };
    let mut batches = 0;
    let token = read_table(&source, "public.widgets", &ScanOptions::default(), &options, |_| {
        batches += 1;
        ControlFlow::Break(())
    })
    .await
    .unwrap();
    assert_eq!(batches, 1);
    assert!(token.is_some(), "a Break should hand back a resume token");
}

/// The resume token `read_table` returns on an early `Break` picks up
/// exactly where the callback stopped: feeding it back in continues without
/// re-delivering the row(s) already seen.
#[tokio::test]
async fn resume_token_from_break_continues_correctly() {
    use pgdump_query::table_stream;

    let source = LocalFileSource::open(edge_cases()).unwrap();
    let options = BatchOptions { max_rows: 1, max_bytes: None };
    let mut rows = Vec::new();
    let token = read_table(&source, "public.widgets", &ScanOptions::default(), &options, |batch| {
        rows.extend(rows_of(&batch));
        ControlFlow::Break(())
    })
    .await
    .unwrap()
    .expect("Break yields a resume token");

    let mut resumed =
        table_stream(&source, "public.widgets", ScanOptions::default(), options, Some(token));
    use futures::StreamExt;
    while let Some(batch) = resumed.next().await {
        rows.extend(rows_of(&batch.unwrap()));
    }
    assert_eq!(rows, widgets_expected());
}

/// Round-trip check against real `pg_dump` output, batched with a small
/// `max_rows` so the 132-row `escapes` table crosses several batches: each
/// batched value must match a value the test computes itself (see
/// `tests/scan.rs`'s equivalent event-stream test for why — comparing
/// against a hand-transcribed literal risks baking in the same misreading
/// twice). Exercises the zero-copy view path against every escape pg_dump
/// emits, not just the hand-written edge cases.
#[tokio::test]
async fn escapes_table_round_trips_through_postgres_batched() {
    for version in [13, 16, 18] {
        let options = BatchOptions { max_rows: 17, max_bytes: None };
        let (_, rows) = collect(
            &fixture(version, "default"),
            "public.escapes",
            &ScanOptions::default(),
            &options,
        )
        .await;

        assert_eq!(rows.len(), 132, "pg_dump {version}");
        for row in &rows {
            let codepoint: u32 = row[0].as_deref().unwrap().parse().unwrap();
            let expected = char::from_u32(codepoint).unwrap().to_string();
            assert_eq!(
                row[1].as_deref(),
                Some(expected.as_str()),
                "pg_dump {version} codepoint {codepoint}"
            );
        }
    }
}
