//! Pull-mode stream, resume, and blocking-iterator tests. See `tests/batch.rs`
//! for the push-mode (`read_table`) equivalent over the same fixtures.

use std::path::{Path, PathBuf};

use arrow::array::RecordBatch;
use arrow::datatypes::DataType;
use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::resolve::{ColumnResolution, SchemaMode};
use pgdump_query::{
    BatchOptions, BlockingTableIter, LocalFileSource, ScanOptions, render_field, table_stream,
};

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
    (0..batch.num_rows())
        .map(|row| batch.columns().iter().map(|c| render_field(c.as_ref(), row)).collect())
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

/// `TableStream::resolved_schema` reports the same typing the actual
/// `RecordBatch`es carry (see `batch.rs`'s module docs) — `resolved_schema` is a preview available
/// before/during consumption, not a second, independent schema.
#[tokio::test]
async fn resolved_schema_matches_the_batches_it_describes() {
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
    assert_eq!(batches[0].schema().field(0).data_type(), &DataType::Int32, "id");
    assert_eq!(batches[0].schema().field(1).data_type(), &DataType::Int16, "v_smallint");
    assert_eq!(batches[0].schema().field(2).data_type(), &DataType::Int32, "v_integer");
    assert_eq!(batches[0].schema().field(3).data_type(), &DataType::Int64, "v_bigint");

    let resolved = stream.resolved_schema();
    assert_eq!(*resolved.schema, *batches[0].schema());
    assert_eq!(resolved.schema.field(0).data_type(), &DataType::Int32, "id");
    assert_eq!(resolved.schema.field(1).data_type(), &DataType::Int16, "v_smallint");
    assert_eq!(resolved.schema.field(2).data_type(), &DataType::Int32, "v_integer");
    assert_eq!(resolved.schema.field(3).data_type(), &DataType::Int64, "v_bigint");
    assert!(resolved.columns.iter().all(|c| *c == ColumnResolution::Mapped));
    assert!(resolved.schema.fields().iter().all(|f| f.is_nullable()));
}

/// `SchemaMode::Strings` never looks at the DDL at all — every column comes
/// back `NotDeclared`, matching the untyped path exactly.
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
/// typing (`docs/design/architecture.md`, "The preamble grammar and `DumpMetadata`") — the preamble is still scanned fresh, so typing works identically
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
/// `docs/design/architecture.md` calls for.
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

/// `table_stream` used to match `COPY` blocks by qualified table name alone,
/// with no notion of which `\connect` segment a block belongs to, and
/// silently unioned both databases' rows for a name genuinely defined
/// twice. The one-target-per-query rule (`docs/design/architecture.md`)
/// closed that: the query now errors, naming both
/// candidates, instead of returning a union of two unrelated tables.
#[tokio::test]
async fn querying_a_table_name_shared_by_two_databases_errors_without_a_database_selector() {
    for version in [13, 16, 18] {
        let (_dir, path) = multidb_fixture(version);
        let source = LocalFileSource::open(&path).unwrap();
        let mut stream = table_stream(
            &source,
            "public.widgets",
            ScanOptions::default(),
            BatchOptions::default(),
            None,
            None,
            CacheMode::Disabled,
        );
        let err = loop {
            match stream.next().await {
                Some(Err(e)) => break e,
                Some(Ok(_)) => continue,
                None => panic!("pg_dump {version}: stream ended without erroring"),
            }
        };
        match &err {
            pgdump_query::Error::AmbiguousTable { name, candidates } => {
                assert_eq!(name, "public.widgets", "pg_dump {version}");
                assert_eq!(
                    candidates,
                    &[
                        "pgdq_fixture.public.widgets".to_string(),
                        "pgdq_fixture_2.public.widgets".to_string()
                    ],
                    "pg_dump {version}"
                );
            }
            other => panic!("pg_dump {version}: expected AmbiguousTable, got {other:?}"),
        }
    }
}

/// `BatchOptions::database` (`--database` at the CLI) is the way out of that
/// ambiguity: naming the *first* database returns exactly that database's
/// rows, matching what querying the un-concatenated single-database fixture
/// returns. The first database is the one an incremental scan's preamble
/// prepass always captures (`crate::index::scan_preamble`), so `Typed` mode
/// resolves it with no extra scan needed.
#[tokio::test]
async fn database_selector_resolves_the_ambiguity_to_the_first_databases_rows() {
    for version in [13, 16, 18] {
        let single_source = LocalFileSource::open(fixture(version, "create")).unwrap();
        let single_rows = all_rows(&single_source, "public.widgets").await;
        assert!(!single_rows.is_empty(), "pg_dump {version}");

        let (_dir, path) = multidb_fixture(version);
        let combined_source = LocalFileSource::open(&path).unwrap();
        let batch_options =
            BatchOptions { database: Some("pgdq_fixture".to_string()), ..Default::default() };
        let mut stream = table_stream(
            &combined_source,
            "public.widgets",
            ScanOptions::default(),
            batch_options,
            None,
            None,
            CacheMode::Disabled,
        );
        let mut rows = Vec::new();
        while let Some(batch) = stream.next().await {
            rows.extend(rows_of(&batch.unwrap()));
        }
        assert_eq!(rows, single_rows, "pg_dump {version}");
    }
}

/// Selecting the *second* database still requires either `SchemaMode::Strings`
/// or a prior full scan: an incremental scan never learns a later
/// `\connect`ed database's DDL (`table_stream`'s live scan deliberately does
/// not accumulate preamble as it goes — "The preamble pass" in the phase
/// doc), so a `Typed` query against it is `Error::MetadataNotScanned`, not a
/// silent `Utf8View` degradation.
#[tokio::test]
async fn selecting_a_later_databases_table_needs_strings_mode_or_a_prior_full_scan() {
    for version in [13, 16, 18] {
        let (_dir, path) = multidb_fixture(version);
        let source = LocalFileSource::open(&path).unwrap();

        let typed =
            BatchOptions { database: Some("pgdq_fixture_2".to_string()), ..Default::default() };
        let mut stream = table_stream(
            &source,
            "public.widgets",
            ScanOptions::default(),
            typed,
            None,
            None,
            CacheMode::Disabled,
        );
        match stream.next().await {
            Some(Err(pgdump_query::Error::MetadataNotScanned { database })) => {
                assert_eq!(database.as_deref(), Some("pgdq_fixture_2"), "pg_dump {version}");
            }
            other => panic!("pg_dump {version}: expected MetadataNotScanned, got {other:?}"),
        }

        let strings = BatchOptions {
            database: Some("pgdq_fixture_2".to_string()),
            schema_mode: SchemaMode::Strings,
            ..Default::default()
        };
        let mut stream = table_stream(
            &source,
            "public.widgets",
            ScanOptions::default(),
            strings,
            None,
            None,
            CacheMode::Disabled,
        );
        let mut rows = Vec::new();
        while let Some(batch) = stream.next().await {
            rows.extend(rows_of(&batch.unwrap()));
        }
        assert!(!rows.is_empty(), "pg_dump {version}: SchemaMode::Strings bypasses the check");
    }
}

fn partitions_fixture(version: u32, flag_set: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(version.to_string())
        .join("partitions")
        .join(format!("{flag_set}.sql"))
}

/// I2's multi-block shape, end to end. `public.feel` is hash-partitioned on
/// an enum column, so `pg_dump` forces load-via-partition-root with **no
/// flag** and writes two `COPY public.feel` headers — with `public.feel_m`'s
/// block sitting between them, since `TABLE DATA` entries sort by the
/// partition's own name. A query must return every partition's rows, which
/// means the `-- load via partition root` marker has to stop the scan from
/// finishing early at the first match
/// (`docs/design/architecture.md`, "Query: mapping and streaming are separate passes").
#[tokio::test]
async fn a_partition_root_name_yields_every_partitions_rows() {
    for version in [13, 16, 18] {
        for flag_set in ["default", "load-via-partition-root"] {
            let path = partitions_fixture(version, flag_set);
            let source = LocalFileSource::open(&path).unwrap();
            let mut stream = table_stream(
                &source,
                "public.feel",
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
            rows.sort();
            assert_eq!(
                rows,
                vec![
                    vec![Some("1".to_string()), Some("sad".to_string())],
                    vec![Some("2".to_string()), Some("ok".to_string())],
                    vec![Some("3".to_string()), Some("happy".to_string())],
                ],
                "pg_dump {version}, {flag_set}: every partition's rows, not just the first block's"
            );
        }
    }
}

/// The marker is recorded on the blocks that carry it and on no others, so
/// the stop rule reads a stored fact rather than re-sniffing the file
/// (`CopyBlock::partition_root`).
#[tokio::test]
async fn partition_root_is_recorded_only_on_marked_blocks() {
    for version in [13, 16, 18] {
        let path = partitions_fixture(version, "default");
        let source = LocalFileSource::open(&path).unwrap();
        let index = pgdump_query::build_index(&source, &ScanOptions::default()).await.unwrap();

        let marked: Vec<(&str, &str)> = index
            .blocks()
            .filter_map(|b| b.partition_root.as_deref().map(|root| (b.header.table.as_str(), root)))
            .collect();
        assert_eq!(
            marked,
            vec![
                ("feel", "public.feel"),
                ("feel", "public.feel"),
                ("spread", "public.spread"),
                ("spread", "public.spread"),
            ],
            "pg_dump {version}"
        );

        // The LIST-partitioned table is not forced, so its partitions dump
        // under their own names with no marker at all.
        for block in index.blocks().filter(|b| b.header.table.starts_with("evt")) {
            assert_eq!(block.partition_root, None, "pg_dump {version}: {}", block.header.table);
        }
    }
}

/// An ordinary table in a file that *also* contains partition-root blocks
/// still stops early — the marker gates the stop per matched block, not per
/// file. `evt_m` is the second of nine blocks, and the first marked block
/// comes after it.
#[tokio::test]
async fn an_unmarked_target_still_stops_early_in_a_file_containing_marked_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("partitions.sql");
    std::fs::copy(partitions_fixture(16, "default"), &dump).unwrap();
    let cache_path = pgdump_query::cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();

    let mut stream = table_stream(
        &source,
        "public.evt_m",
        ScanOptions::default(),
        BatchOptions::default(),
        None,
        None,
        CacheMode::Enabled(cache_path.clone()),
    );
    let mut rows = Vec::new();
    while let Some(batch) = stream.next().await {
        rows.extend(rows_of(&batch.unwrap()));
    }
    assert_eq!(rows, vec![vec![Some("unrelated".to_string())]]);

    use pgdump_query::ByteRangeSource;
    let index = CacheMode::Enabled(cache_path).load(&source).await.unwrap().unwrap();
    let evt_m = index.blocks_for("public.evt_m").next().unwrap();
    assert_eq!(index.scanned_through, evt_m.end_offset);
    assert!(index.scanned_through < source.size().await.unwrap());
}
