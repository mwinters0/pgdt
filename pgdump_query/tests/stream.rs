//! Pull-mode stream, resume, and blocking-iterator tests. See `tests/batch.rs`
//! for the push-mode (`read_table`) equivalent over the same fixtures.

use arrow::datatypes::DataType;
use futures::StreamExt;
use pgdump_query::cache::{CacheLoad, CacheMode};
use pgdump_query::resolve::{ColumnResolution, SchemaMode};
use pgdump_query::{BlockingTableIter, LocalFileSource, QueryOptions, ScanOptions, table_stream};

mod common;
use common::{
    edge_cases, edge_cases_fixture, multidb_fixture, partitions_fixture, rows_of, types_fixture,
    widgets_expected,
};

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
        QueryOptions::default(),
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
    let options = QueryOptions { max_rows: 1, max_bytes: None, ..Default::default() };
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
        QueryOptions { max_rows: 1, max_bytes: None, ..Default::default() },
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
        QueryOptions { max_rows: 1, max_bytes: None, ..Default::default() },
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
        let path = edge_cases_fixture(version, "default");
        let source = LocalFileSource::open(&path).unwrap();
        let options = QueryOptions { max_rows: 132, max_bytes: None, ..Default::default() };
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

/// `TableStream::resolved_schema` reports the same typing the actual
/// `RecordBatch`es carry (see `batch.rs`'s module docs) — `resolved_schema` is a preview available
/// before/during consumption, not a second, independent schema.
#[tokio::test]
async fn resolved_schema_matches_the_batches_it_describes() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let batch_options = QueryOptions { schema_mode: SchemaMode::Typed, ..Default::default() };
    let mut stream = table_stream(
        &source,
        "public.t_int",
        ScanOptions::default(),
        batch_options,
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
    let batch_options = QueryOptions { schema_mode: SchemaMode::Strings, ..Default::default() };
    let mut stream = table_stream(
        &source,
        "public.t_int",
        ScanOptions::default(),
        batch_options,
        None,
        CacheMode::Disabled,
    );
    while stream.next().await.transpose().unwrap().is_some() {}
    let resolved = stream.resolved_schema();
    assert!(resolved.columns.iter().all(|c| *c == ColumnResolution::NotDeclared));
}

/// `--cache-path none` (`CacheMode::Disabled`) disables persistence, not
/// typing (`docs/design/decisions.md`, "D36") — the preamble is still scanned fresh, so typing works identically
/// to `CacheMode::Enabled`, just without leaving a cache file behind.
#[tokio::test]
async fn disabled_cache_still_resolves_types() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let batch_options = QueryOptions { schema_mode: SchemaMode::Typed, ..Default::default() };
    let mut stream = table_stream(
        &source,
        "public.t_int",
        ScanOptions::default(),
        batch_options,
        None,
        CacheMode::Disabled,
    );
    while stream.next().await.transpose().unwrap().is_some() {}
    let resolved = stream.resolved_schema();
    assert_eq!(resolved.schema.field(0).data_type(), &DataType::Int32, "id");
    assert!(resolved.columns.iter().all(|c| *c == ColumnResolution::Mapped));
}

/// The blocking `Iterator` wrapper drives the same stream to the same result
/// with no ambient `tokio` runtime, for a caller with no async context of its
/// own.
#[test]
fn blocking_iterator_matches_async_stream() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let stream = table_stream(
        &source,
        "public.widgets",
        ScanOptions::default(),
        QueryOptions::default(),
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

/// Every row `table` yields, drained from a pull-mode stream with no
/// predicate and no resume — the whole-table read the multi-database tests
/// compare against each other.
async fn all_rows(source: &LocalFileSource, table: &str) -> Vec<Vec<Option<String>>> {
    let mut stream = table_stream(
        source,
        table,
        ScanOptions::default(),
        QueryOptions::default(),
        None,
        CacheMode::Disabled,
    );
    let mut rows = Vec::new();
    while let Some(batch) = stream.next().await {
        rows.extend(rows_of(&batch.unwrap()));
    }
    rows
}

/// A qualified table name genuinely defined in two `\connect`ed databases is
/// ambiguous by name alone, so the one-target-per-query rule
/// (`docs/design/decisions.md`, "D49") errors, naming both candidates,
/// instead of unioning two unrelated tables' rows.
#[tokio::test]
async fn querying_a_table_name_shared_by_two_databases_errors_without_a_database_selector() {
    for version in [13, 16, 18] {
        let (_dir, path) = multidb_fixture(version);
        let source = LocalFileSource::open(&path).unwrap();
        let mut stream = table_stream(
            &source,
            "public.widgets",
            ScanOptions::default(),
            QueryOptions::default(),
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

/// `QueryOptions::database` (`--database` at the CLI) is the way out of that
/// ambiguity: naming the *first* database returns exactly that database's
/// rows, matching what querying the un-concatenated single-database fixture
/// returns. The first database is the one an incremental scan's preamble
/// prepass always captures (`crate::index::scan_preamble`), so `Typed` mode
/// resolves it with no extra scan needed.
#[tokio::test]
async fn database_selector_resolves_the_ambiguity_to_the_first_databases_rows() {
    for version in [13, 16, 18] {
        let single_source = LocalFileSource::open(edge_cases_fixture(version, "create")).unwrap();
        let single_rows = all_rows(&single_source, "public.widgets").await;
        assert!(!single_rows.is_empty(), "pg_dump {version}");

        let (_dir, path) = multidb_fixture(version);
        let combined_source = LocalFileSource::open(&path).unwrap();
        let batch_options =
            QueryOptions { database: Some("pgdq_fixture".to_string()), ..Default::default() };
        let mut stream = table_stream(
            &combined_source,
            "public.widgets",
            ScanOptions::default(),
            batch_options,
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

/// Selecting the *second* database types it on a **cold** query, exactly as a
/// query after `pgdq parse` does. The mapping pass states `DumpMetadata` at
/// each `\connect`ed database's first `COPY` block — one of the two boundaries
/// `preamble::dump_metadata_from_spans` may be called at (I1) — so the DDL a
/// scan has walked past is DDL it may use.
///
/// This does not reintroduce the rejected "accumulate preamble as the stream
/// goes": mapping and streaming are separate passes, the map is complete
/// before any row is emitted, and a file containing any `\connect` is never
/// early-stopped (`stream::target_settled`). So the schema depends on the
/// finished map, never on how far the row replay has got.
///
/// `SchemaMode::Strings` still bypasses typing entirely, and must return the
/// same rows through the untyped path.
#[tokio::test]
async fn selecting_a_later_databases_table_types_it_on_a_cold_query() {
    for version in [13, 16, 18] {
        let (_dir, path) = multidb_fixture(version);
        let source = LocalFileSource::open(&path).unwrap();

        let typed =
            QueryOptions { database: Some("pgdq_fixture_2".to_string()), ..Default::default() };
        let mut stream = table_stream(
            &source,
            "public.widgets",
            ScanOptions::default(),
            typed,
            None,
            CacheMode::Disabled,
        );
        let mut typed_rows = Vec::new();
        let mut schema = None;
        while let Some(batch) = stream.next().await {
            let batch = batch.expect("pg_dump {version}: the second database's DDL was read");
            schema.get_or_insert_with(|| batch.schema());
            typed_rows.extend(rows_of(&batch));
        }
        let schema = schema.unwrap_or_else(|| panic!("pg_dump {version}: no batches"));
        assert!(
            schema.fields().iter().any(|f| f.data_type() != &DataType::Utf8View),
            "pg_dump {version}: really typed, not degraded to strings: {schema:?}"
        );

        let strings = QueryOptions {
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
            CacheMode::Disabled,
        );
        let mut rows = Vec::new();
        while let Some(batch) = stream.next().await {
            rows.extend(rows_of(&batch.unwrap()));
        }
        assert!(!rows.is_empty(), "pg_dump {version}: SchemaMode::Strings bypasses the check");
        assert_eq!(rows, typed_rows, "pg_dump {version}: same rows, typed or not");
    }
}

/// I2's multi-block shape, end to end. `public.feel` is hash-partitioned on
/// an enum column, so `pg_dump` forces load-via-partition-root with **no
/// flag** and writes two `COPY public.feel` headers — with `public.feel_m`'s
/// block sitting between them, since `TABLE DATA` entries sort by the
/// partition's own name. A query must return every partition's rows, which
/// means the `-- load via partition root` marker has to stop the scan from
/// finishing early at the first match
/// (`docs/design/decisions.md`, "D48").
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
                QueryOptions::default(),
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
        QueryOptions::default(),
        None,
        CacheMode::Enabled(cache_path.clone()),
    );
    let mut rows = Vec::new();
    while let Some(batch) = stream.next().await {
        rows.extend(rows_of(&batch.unwrap()));
    }
    assert_eq!(rows, vec![vec![Some("unrelated".to_string())]]);

    use pgdump_query::ByteRangeSource;
    let CacheLoad::Index(index) = CacheMode::Enabled(cache_path).load(&source).await.unwrap()
    else {
        panic!("the query wrote a cache")
    };
    let evt_m = index.blocks_for("public.evt_m").next().unwrap();
    assert_eq!(index.scanned_through, evt_m.end_offset);
    assert!(index.scanned_through < source.size().await.unwrap());
}

/// A dump whose rows carry multi-byte UTF-8 beside COPY TEXT escapes, the
/// NULL marker and an empty field, written to a temp file so the chunk-size
/// sweep below has something a fixture does not contain.
fn utf8_dump(dir: &std::path::Path, third_column: &[u8]) -> std::path::PathBuf {
    let mut dump = Vec::new();
    dump.extend_from_slice(b"SET client_encoding = 'UTF8';\n\n");
    dump.extend_from_slice(b"COPY public.notes (id, note, tail) FROM stdin;\n");
    for (id, note) in [
        "caf\u{e9} \u{2603} \u{1f408}",
        "multi\\nline\\twith a backslash \\\\ inside",
        "\\N",
        "",
        "\u{1f408}\u{1f408}\u{1f408}\u{1f408}\u{1f408}\u{1f408}\u{1f408}\u{1f408}",
    ]
    .iter()
    .enumerate()
    {
        dump.extend_from_slice(format!("{}\t{note}\t", id + 1).as_bytes());
        dump.extend_from_slice(third_column);
        dump.push(b'\n');
    }
    dump.extend_from_slice(b"\\.\n");
    let path = dir.join("notes.sql");
    std::fs::write(&path, dump).unwrap();
    path
}

async fn notes_rows(
    path: &std::path::Path,
    chunk_size: usize,
    options: QueryOptions,
) -> pgdump_query::Result<Vec<Vec<Option<String>>>> {
    let source = LocalFileSource::open(path).unwrap();
    let mut stream = table_stream(
        &source,
        "public.notes",
        ScanOptions { chunk_size, ..ScanOptions::default() },
        options,
        None,
        CacheMode::Disabled,
    );
    let mut rows = Vec::new();
    while let Some(batch) = stream.next().await {
        rows.extend(rows_of(&batch?));
    }
    Ok(rows)
}

/// **The bulk UTF-8 validation does not depend on where the chunks fall.**
/// A row is decoded off the validated prefix of the span it was scanned in,
/// and at a small enough chunk size every row straddles a boundary and
/// arrives on the carry instead — so the same file read one byte at a time
/// and read whole must give the same values, multi-byte sequences split
/// across the boundary included
/// (`docs/design/decisions.md`, "D27").
#[tokio::test]
async fn a_query_answers_the_same_at_every_chunk_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = utf8_dump(dir.path(), "end".as_bytes());
    let cat = "\u{1f408}";
    let reference: Vec<Vec<Option<String>>> = vec![
        vec![Some("1".into()), Some(format!("caf\u{e9} \u{2603} {cat}")), Some("end".into())],
        vec![
            Some("2".into()),
            Some("multi\nline\twith a backslash \\ inside".into()),
            Some("end".into()),
        ],
        vec![Some("3".into()), None, Some("end".into())],
        vec![Some("4".into()), Some(String::new()), Some("end".into())],
        vec![Some("5".into()), Some(cat.repeat(8)), Some("end".into())],
    ];
    assert_eq!(notes_rows(&path, 1 << 20, QueryOptions::default()).await.unwrap(), reference);
    for chunk_size in [1, 2, 3, 7, 13, 64, 511, 4096] {
        let got = notes_rows(&path, chunk_size, QueryOptions::default()).await.unwrap();
        assert_eq!(got, reference, "chunk_size {chunk_size}");
    }
}

/// **A dump that is not UTF-8 fails exactly where it always did**, at the
/// field that is decoded and not before. The bulk validation refuses the
/// whole span it covers, which puts every row of it back on the per-field
/// check — so a projection that never reads the offending column still
/// answers, and one that reads it still raises `InvalidUtf8`.
#[tokio::test]
async fn a_non_utf8_field_is_refused_only_where_it_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = utf8_dump(dir.path(), b"tail \xff byte");

    for chunk_size in [1, 13, 4096, 1 << 20] {
        let err = notes_rows(&path, chunk_size, QueryOptions::default()).await.unwrap_err();
        assert!(
            matches!(err, pgdump_query::Error::InvalidUtf8 { .. }),
            "chunk_size {chunk_size}: {err}"
        );

        let projected = QueryOptions {
            projection: Some(vec!["id".to_string(), "note".to_string()]),
            ..QueryOptions::default()
        };
        let rows = notes_rows(&path, chunk_size, projected).await.unwrap();
        assert_eq!(rows.len(), 5, "chunk_size {chunk_size}");
        assert_eq!(rows[0][1].as_deref(), Some("caf\u{e9} \u{2603} \u{1f408}"));
    }
}

/// **A filter one block of a table refuses is refused where that block is
/// reached, after the rows of the blocks before it** — on the serial replay
/// and on a partitioned one alike, though both resolve every block's filter
/// before reading any (`docs/design/decisions.md`, "D54"). The second block
/// has no column `b`.
#[tokio::test]
async fn a_later_blocks_refusal_follows_the_rows_before_it() {
    use pgdump_query::{
        Error, Expr, Parallelism, Predicate, PredicateOp, ScanExtent, table_stream_partitions,
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("two_schemas.sql");
    std::fs::write(
        &path,
        "COPY public.t (a, b) FROM stdin;\n1\tx\n2\t\\N\n3\ty\n\\.\n\n\
         COPY public.t (a) FROM stdin;\n4\n\\.\n",
    )
    .unwrap();
    let source = LocalFileSource::open(&path).unwrap();
    let options = QueryOptions {
        filter: Expr::all([Predicate {
            column: "b".into(),
            op: PredicateOp::IsNotNull,
            value: None,
        }]),
        scan_extent: ScanExtent::Full,
        ..QueryOptions::default()
    };
    let expected: Vec<Vec<Option<String>>> =
        vec![vec![Some("1".into()), Some("x".into())], vec![Some("3".into()), Some("y".into())]];
    let refused = |err: &Error| {
        matches!(err, Error::UnknownPredicateColumn { column, header_offset }
            if column == "b" && *header_offset > 0)
    };

    let mut stream = table_stream(
        &source,
        "public.t",
        ScanOptions::default(),
        options.clone(),
        None,
        CacheMode::Disabled,
    );
    let mut rows = Vec::new();
    let err = loop {
        match stream.next().await.expect("the stream ends in the refusal") {
            Ok(batch) => rows.extend(rows_of(&batch)),
            Err(err) => break err,
        }
    };
    assert_eq!(rows, expected);
    assert!(refused(&err), "{err}");

    let partitioned = QueryOptions { parallelism: Parallelism::workers(4, 1 << 30), ..options };
    let streams = table_stream_partitions(
        &source,
        "public.t",
        ScanOptions::default(),
        partitioned,
        CacheMode::Disabled,
    )
    .await
    .expect("the plan does not raise a block's refusal");
    let mut rows = Vec::new();
    let mut errors = Vec::new();
    for mut sub in streams {
        while let Some(batch) = sub.next().await {
            match batch {
                Ok(batch) => rows.extend(rows_of(&batch)),
                Err(err) => {
                    errors.push(err);
                    break;
                }
            }
        }
    }
    assert_eq!(rows, expected);
    assert!(!errors.is_empty() && errors.iter().all(refused), "{errors:?}");
}
