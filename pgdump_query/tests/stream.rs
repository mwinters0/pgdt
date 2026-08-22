//! Pull-mode stream, resume, and blocking-iterator tests. See `tests/batch.rs`
//! for the push-mode (`read_table`) equivalent over the same fixtures.

use std::path::{Path, PathBuf};

use arrow::array::{Array, RecordBatch, StringViewArray};
use arrow::datatypes::DataType;
use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::resolve::{ColumnResolution, SchemaMode};
use pgdump_query::{BatchOptions, BlockingTableIter, LocalFileSource, ScanOptions, table_stream};

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

fn types_fixture(version: u32, flag_set: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(version.to_string())
        .join("types")
        .join(format!("{flag_set}.sql"))
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
    let options = BatchOptions { max_rows: 1, max_bytes: None, ..Default::default() };
    for stop_after in [1, 2, 5] {
        let source = LocalFileSource::open(edge_cases()).unwrap();
        let mut stream = table_stream(
            &source,
            "public.widgets",
            ScanOptions::default(),
            options.clone(),
            None,
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
            None,
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
        BatchOptions { max_rows: 1, max_bytes: None, ..Default::default() },
        None,
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
        BatchOptions { max_rows: 1, max_bytes: None, ..Default::default() },
        None,
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
        let options = BatchOptions { max_rows: 132, max_bytes: None, ..Default::default() };
        let mut stream = table_stream(
            &source,
            "public.escapes",
            ScanOptions::default(),
            options.clone(),
            None,
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
            None,
            Some(token),
            CacheMode::Disabled,
        );
        assert!(resumed.next().await.is_none(), "pg_dump {version}: nothing left after boundary");
    }
}

/// `TableStream::resolved_schema` reports the *target* typing this build
/// already understands (`docs/design/roadmap-phase2-typed-columns.md`,
/// "API shape changes"), even though every `RecordBatch` this same stream
/// yields is still all-`Utf8View` in Phase 2.3 — see `resolve.rs`'s module
/// docs for why the two schemas deliberately disagree at this slice.
#[tokio::test]
async fn resolved_schema_reflects_the_dumps_ddl_while_batches_stay_utf8view() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let batch_options = BatchOptions { schema_mode: SchemaMode::Typed, ..Default::default() };
    let mut stream = table_stream(
        &source,
        "public.t_int",
        ScanOptions::default(),
        batch_options,
        None,
        None,
        CacheMode::Disabled,
    );
    let mut batches = Vec::new();
    while let Some(batch) = stream.next().await.transpose().unwrap() {
        batches.push(batch);
    }
    assert_eq!(batches.len(), 1);
    // The actual batch: every field Utf8View, decoders don't exist yet.
    assert!(batches[0].schema().fields().iter().all(|f| f.data_type() == &DataType::Utf8View));

    let resolved = stream.resolved_schema();
    assert_eq!(resolved.schema.field(0).data_type(), &DataType::Int32, "id");
    assert_eq!(resolved.schema.field(1).data_type(), &DataType::Int16, "v_smallint");
    assert_eq!(resolved.schema.field(2).data_type(), &DataType::Int32, "v_integer");
    assert_eq!(resolved.schema.field(3).data_type(), &DataType::Int64, "v_bigint");
    assert!(resolved.columns.iter().all(|c| *c == ColumnResolution::Mapped));
    assert!(resolved.schema.fields().iter().all(|f| f.is_nullable()));
}

/// `SchemaMode::Strings` never looks at the DDL at all — every column comes
/// back `NotDeclared`, matching Phase 1 exactly.
#[tokio::test]
async fn strings_mode_never_resolves_types() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let batch_options = BatchOptions { schema_mode: SchemaMode::Strings, ..Default::default() };
    let mut stream = table_stream(
        &source,
        "public.t_int",
        ScanOptions::default(),
        batch_options,
        None,
        None,
        CacheMode::Disabled,
    );
    while stream.next().await.transpose().unwrap().is_some() {}
    let resolved = stream.resolved_schema();
    assert!(resolved.columns.iter().all(|c| *c == ColumnResolution::NotDeclared));
}

/// `--cache-path none` (`CacheMode::Disabled`) disables persistence, not
/// typing (`docs/design/roadmap-phase2-typed-columns.md`, "The preamble
/// pass") — the preamble is still scanned fresh, so typing works identically
/// to `CacheMode::Enabled`, just without leaving a cache file behind.
#[tokio::test]
async fn disabled_cache_still_resolves_types() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let batch_options = BatchOptions { schema_mode: SchemaMode::Typed, ..Default::default() };
    let mut stream = table_stream(
        &source,
        "public.t_int",
        ScanOptions::default(),
        batch_options,
        None,
        None,
        CacheMode::Disabled,
    );
    while stream.next().await.transpose().unwrap().is_some() {}
    let resolved = stream.resolved_schema();
    assert_eq!(resolved.schema.field(0).data_type(), &DataType::Int32, "id");
    assert!(resolved.columns.iter().all(|c| *c == ColumnResolution::Mapped));
}

/// The blocking `Iterator` wrapper drives the same stream to the same result
/// with no ambient `tokio` runtime — the sync-caller path
/// `roadmap-phase1-mvp.md` calls for.
#[test]
fn blocking_iterator_matches_async_stream() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let stream = table_stream(
        &source,
        "public.widgets",
        ScanOptions::default(),
        BatchOptions::default(),
        None,
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

/// Two copies of `edge_cases/create.sql`, concatenated into one real
/// `\connect`-delimited multi-database dump — see `tests/preamble.rs`'s
/// `multidb_fixture` for the full rationale (duplicated here since each
/// `tests/*.rs` file is its own crate with no shared support module).
fn multidb_fixture(version: u32) -> (tempfile::TempDir, PathBuf) {
    let content = std::fs::read_to_string(fixture(version, "create")).unwrap();
    let renamed = content.replace("pgdq_fixture", "pgdq_fixture_2");
    let dir = tempfile::tempdir().unwrap();
    let combined = dir.path().join("multidb.sql");
    std::fs::write(&combined, format!("{content}{renamed}")).unwrap();
    (dir, combined)
}

async fn all_rows(source: &LocalFileSource, table: &str) -> Vec<Vec<Option<String>>> {
    let mut stream = table_stream(
        source,
        table,
        ScanOptions::default(),
        BatchOptions::default(),
        None,
        None,
        CacheMode::Disabled,
    );
    let mut rows = Vec::new();
    while let Some(batch) = stream.next().await {
        rows.extend(rows_of(&batch.unwrap()));
    }
    rows
}

/// `table_stream` matches `COPY` blocks by qualified table name alone, with
/// no notion of which `\connect` segment a block belongs to (`resolve.rs`'s
/// `database_for` has the same simplification for typing — see
/// `docs/status/STATUS.md`, "Decisions worth a second look"). Against a real
/// multi-database dump where the same table name is genuinely defined twice,
/// today's behavior is a silent union of both databases' rows, in file
/// order — not the per-database error `docs/design/roadmap-phase2-typed-columns.md`
/// ("Multi-database dumps") describes as the eventual intent. This test
/// locks in and documents *current* behavior; it is not an endorsement of it.
#[tokio::test]
async fn querying_a_table_name_shared_by_two_databases_silently_unions_both() {
    for version in [13, 16, 18] {
        let single_source = LocalFileSource::open(fixture(version, "create")).unwrap();
        let single_rows = all_rows(&single_source, "public.widgets").await;
        assert!(!single_rows.is_empty(), "pg_dump {version}");

        let (_dir, path) = multidb_fixture(version);
        let combined_source = LocalFileSource::open(&path).unwrap();
        let combined_rows = all_rows(&combined_source, "public.widgets").await;

        let mut expected = single_rows.clone();
        expected.extend(single_rows);
        assert_eq!(combined_rows, expected, "pg_dump {version}");
    }
}
