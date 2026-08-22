//! Pull-mode stream, resume, and blocking-iterator tests. See `tests/batch.rs`
//! for the push-mode (`read_table`) equivalent over the same fixtures.

use std::path::{Path, PathBuf};

use arrow::array::{Array, RecordBatch, StringViewArray};
use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::{BatchOptions, BlockingTableIter, LocalFileSource, ScanOptions, table_stream};

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

/// The pull-mode primitive, run to completion with no resume, matches the
/// push-mode fixture data exactly — a regression guard on top of the fact
/// that `read_table` is now implemented by draining this same stream.
#[tokio::test]
async fn stream_matches_push_mode_output() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let mut stream = table_stream(
        &source,
        "public.widgets",
        ScanOptions::default(),
        BatchOptions::default(),
        None,
        CacheMode::Disabled,
    );
    let mut rows = Vec::new();
    while let Some(batch) = stream.next().await {
        rows.extend(rows_of(&batch.unwrap()));
    }
    assert_eq!(rows, widgets_expected());
}

/// Stopping a stream partway, taking its resume token, and continuing a
/// fresh stream from that token yields exactly the rows the first stream
/// hadn't delivered yet — no gap, no repeat — even split across several
/// resume points at different row counts.
#[tokio::test]
async fn resume_continues_without_gap_or_repeat() {
    let options = BatchOptions { max_rows: 1, max_bytes: None };
    for stop_after in [1, 2, 5] {
        let source = LocalFileSource::open(edge_cases()).unwrap();
        let mut stream = table_stream(
            &source,
            "public.widgets",
            ScanOptions::default(),
            options.clone(),
            None,
            CacheMode::Disabled,
        );

        let mut rows = Vec::new();
        for _ in 0..stop_after {
            let batch = stream.next().await.unwrap().unwrap();
            rows.extend(rows_of(&batch));
        }
        let token = stream.resume_token();
        drop(stream);

        let mut resumed = table_stream(
            &source,
            "public.widgets",
            ScanOptions::default(),
            options.clone(),
            Some(token),
            CacheMode::Disabled,
        );
        while let Some(batch) = resumed.next().await {
            rows.extend(rows_of(&batch.unwrap()));
        }

        assert_eq!(rows, widgets_expected(), "stop_after {stop_after}");
    }
}

/// Resuming mid-block when the paused-on block has no explicit COPY column
/// list still reconstructs the right (placeholder) schema, not just the
/// right byte offset.
#[tokio::test]
async fn resume_reconstructs_headerless_schema() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let mut stream = table_stream(
        &source,
        "public.no_column_list",
        ScanOptions::default(),
        BatchOptions { max_rows: 1, max_bytes: None },
        None,
        CacheMode::Disabled,
    );

    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(rows_of(&first), vec![vec![Some("\\.".to_string())]]);
    let token = stream.resume_token();
    drop(stream);

    let mut resumed = table_stream(
        &source,
        "public.no_column_list",
        ScanOptions::default(),
        BatchOptions { max_rows: 1, max_bytes: None },
        Some(token),
        CacheMode::Disabled,
    );
    let second = resumed.next().await.unwrap().unwrap();
    assert_eq!(
        second.schema().fields().iter().map(|f| f.name().clone()).collect::<Vec<_>>(),
        vec!["column1".to_string()]
    );
    assert_eq!(rows_of(&second), vec![vec![Some("just a value".to_string())]]);
    assert!(resumed.next().await.is_none());
}

/// A resume token taken exactly at a block boundary (just after the last row
/// of a fully-flushed block) resumes cleanly with no in-progress block state.
#[tokio::test]
async fn resume_at_a_block_boundary() {
    for version in [13, 16, 18] {
        let path = fixture(version, "default");
        let source = LocalFileSource::open(&path).unwrap();
        let options = BatchOptions { max_rows: 132, max_bytes: None };
        let mut stream = table_stream(
            &source,
            "public.escapes",
            ScanOptions::default(),
            options.clone(),
            None,
            CacheMode::Disabled,
        );

        let only_batch = stream.next().await.unwrap().unwrap();
        assert_eq!(only_batch.num_rows(), 132, "pg_dump {version}");
        let token = stream.resume_token();
        assert!(stream.next().await.is_none());
        drop(stream);

        let mut resumed = table_stream(
            &source,
            "public.escapes",
            ScanOptions::default(),
            options,
            Some(token),
            CacheMode::Disabled,
        );
        assert!(resumed.next().await.is_none(), "pg_dump {version}: nothing left after boundary");
    }
}

/// The blocking `Iterator` wrapper drives the same stream to the same result
/// with no ambient `tokio` runtime — the sync-caller path `mvp.md` calls for.
#[test]
fn blocking_iterator_matches_async_stream() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let stream = table_stream(
        &source,
        "public.widgets",
        ScanOptions::default(),
        BatchOptions::default(),
        None,
        CacheMode::Disabled,
    );
    let iter = BlockingTableIter::new(stream).unwrap();

    let mut rows = Vec::new();
    for batch in iter {
        rows.extend(rows_of(&batch.unwrap()));
    }
    assert_eq!(rows, widgets_expected());
}
