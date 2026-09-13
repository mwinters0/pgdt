//! Column projection: what a query materializes, and what it therefore
//! never decodes (`docs/design/decisions.md`, "D28").
//!
//! The library-level tests only: everything here drives `table_stream`
//! directly. The flags are pinned separately, in
//! `pgdump_query-cli/tests/query_projection.rs`.

use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::resolve::ColumnResolution;
use pgdump_query::{Error, LocalFileSource, QueryOptions, ScanOptions, table_stream};

mod common;
use common::{edge_cases, rows_of, types_fixture, widgets_expected};

fn projecting(columns: &[&str]) -> QueryOptions {
    QueryOptions {
        projection: Some(columns.iter().map(|c| c.to_string()).collect()),
        ..Default::default()
    }
}

/// Drain `table` from the hand-written edge-case dump under `options`,
/// returning the batches' schema field names alongside the decoded rows.
async fn drain(
    table: &str,
    options: QueryOptions,
) -> pgdump_query::Result<(Vec<String>, Vec<Vec<Option<String>>>)> {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let mut stream =
        table_stream(&source, table, ScanOptions::default(), options, None, CacheMode::Disabled);
    let mut names = Vec::new();
    let mut rows = Vec::new();
    while let Some(batch) = stream.next().await.transpose()? {
        names = batch.schema().fields().iter().map(|f| f.name().clone()).collect();
        rows.extend(rows_of(&batch));
    }
    Ok((names, rows))
}

/// The batches and the schema the stream *reports* are cut by the same
/// projection, in the requested order — which may reorder the file's. Cutting
/// only one of the two is what `RecordBatch::try_new` would catch, so this is
/// the test that says the four parallel vectors travel together.
#[tokio::test]
async fn a_projection_cuts_the_batches_and_the_reported_schema_together() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let mut stream = table_stream(
        &source,
        "public.widgets",
        ScanOptions::default(),
        projecting(&["created_at", "id"]),
        None,
        CacheMode::Disabled,
    );
    let mut rows = Vec::new();
    while let Some(batch) = stream.next().await {
        let batch = batch.unwrap();
        assert_eq!(batch.num_columns(), 2);
        rows.extend(rows_of(&batch));
    }

    // The file's order is (id, name, description, created_at); the query
    // asked for the last and the first, in that order.
    let expected: Vec<Vec<Option<String>>> =
        widgets_expected().into_iter().map(|r| vec![r[3].clone(), r[0].clone()]).collect();
    assert_eq!(rows, expected);

    let resolved = stream.resolved_schema();
    let names: Vec<String> = resolved.schema.fields().iter().map(|f| f.name().clone()).collect();
    assert_eq!(names, vec!["created_at".to_string(), "id".to_string()]);
    assert_eq!(resolved.notes.iter().map(|n| n.column.clone()).collect::<Vec<_>>(), names);
    assert_eq!(resolved.columns.len(), 2);
    assert_eq!(resolved.plans.len(), 2);
}

/// The `COUNT(*)` shape: no column is built, and the batch still carries its
/// row count. `RecordBatch::try_new` cannot express this at all, which is why
/// `flush` states the row count explicitly.
#[tokio::test]
async fn a_zero_column_projection_still_counts_its_rows() {
    let (names, rows) = drain("public.widgets", projecting(&[])).await.unwrap();
    assert!(names.is_empty());
    assert_eq!(rows.len(), widgets_expected().len());
    assert!(rows.iter().all(|r| r.is_empty()));
}

/// A filter names what may be *tested*; the projection names what is
/// *built*. The two are independent, which is what makes a filtered row
/// count expressible.
#[tokio::test]
async fn a_filter_may_name_a_column_the_projection_does_not() {
    let options = QueryOptions {
        filter: pgdump_query::Expr::all([pgdump_query::Predicate {
            column: "name".into(),
            op: pgdump_query::PredicateOp::Eq,
            value: Some("beta".into()),
        }]),
        ..projecting(&[])
    };
    let (names, rows) = drain("public.widgets", options).await.unwrap();
    assert!(names.is_empty());
    assert_eq!(rows.len(), 1, "one widget is named beta");
}

/// A projected name the block does not carry is refused the way a predicate's
/// is, naming the column and the block it was resolved against.
#[tokio::test]
async fn a_name_the_block_does_not_carry_is_refused() {
    let err = drain("public.widgets", projecting(&["id", "nope"])).await.unwrap_err();
    match err {
        Error::UnknownProjectionColumn { column, header_offset } => {
            assert_eq!(column, "nope");
            assert!(header_offset > 0, "the refusal names the block it resolved against");
        }
        other => panic!("expected UnknownProjectionColumn, got {other:?}"),
    }
}

/// A repeated name is a property of the *request*, so it is refused before a
/// byte is read — here against a table the dump does not contain at all,
/// which would otherwise produce no error and no rows.
#[tokio::test]
async fn a_repeated_name_is_refused_without_reading_the_file() {
    let err = drain("public.not_a_table", projecting(&["id", "id"])).await.unwrap_err();
    match err {
        Error::DuplicateProjectionColumn { column } => assert_eq!(column, "id"),
        other => panic!("expected DuplicateProjectionColumn, got {other:?}"),
    }
}

/// **Whether a query succeeds depends on its projection.** `t_numeric.v_small`
/// carries a `NaN` that `Decimal128` cannot represent, so the whole table is a
/// hard `Error::FieldDecode` today; projecting that column away is the
/// per-column escape `--schema-mode strings` used to be the only form of, and
/// it leaves every other column typed.
#[tokio::test]
async fn projecting_a_column_away_escapes_its_decode_failure() {
    for version in [13, 16, 18] {
        let path = types_fixture(version, "default");
        let source = LocalFileSource::open(&path).unwrap();

        let run = async |options: QueryOptions| {
            let mut stream = table_stream(
                &source,
                "public.t_numeric",
                ScanOptions::default(),
                options,
                None,
                CacheMode::Disabled,
            );
            let mut rows = Vec::new();
            while let Some(batch) = stream.next().await.transpose()? {
                rows.extend(rows_of(&batch));
            }
            Ok::<_, Error>((stream.resolved_schema(), rows))
        };

        assert!(
            matches!(run(QueryOptions::default()).await, Err(Error::FieldDecode { .. })),
            "pg_dump {version}: the whole table still fails"
        );
        let (resolved, rows) = run(projecting(&["id", "v_typed"]))
            .await
            .unwrap_or_else(|e| panic!("pg_dump {version}: {e}"));
        assert!(!rows.is_empty(), "pg_dump {version}");
        assert!(
            resolved.columns.iter().all(|c| *c == ColumnResolution::Mapped),
            "pg_dump {version}: the surviving columns are still typed: {:?}",
            resolved.columns
        );
        // Projecting it back in restores the failure, so the escape is the
        // projection and not something else about the narrower query.
        assert!(
            matches!(
                run(projecting(&["id", "v_small"])).await,
                Err(Error::FieldDecode { column, .. }) if column == "v_small"
            ),
            "pg_dump {version}: the column is what fails"
        );
    }
}

/// A resume token stamps the query that produced it. Handing it to a stream
/// with a different projection is refused rather than silently emitting
/// batches of a different shape into the same consumption.
#[tokio::test]
async fn a_resume_token_belongs_to_the_query_that_made_it() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let options = QueryOptions { max_rows: 1, max_bytes: None, ..projecting(&["id", "name"]) };
    let mut stream = table_stream(
        &source,
        "public.widgets",
        ScanOptions::default(),
        options.clone(),
        None,
        CacheMode::Disabled,
    );
    let mut rows = rows_of(&stream.next().await.unwrap().unwrap());
    let token = stream.resume_token();
    drop(stream);

    let mut wrong = table_stream(
        &source,
        "public.widgets",
        ScanOptions::default(),
        QueryOptions { max_rows: 1, max_bytes: None, ..projecting(&["name", "id"]) },
        Some(token.clone()),
        CacheMode::Disabled,
    );
    assert!(
        matches!(wrong.next().await, Some(Err(Error::ResumeQueryMismatch))),
        "a reordered projection is a different query"
    );

    let mut resumed = table_stream(
        &source,
        "public.widgets",
        ScanOptions::default(),
        options,
        Some(token),
        CacheMode::Disabled,
    );
    while let Some(batch) = resumed.next().await {
        rows.extend(rows_of(&batch.unwrap()));
    }
    let expected: Vec<Vec<Option<String>>> =
        widgets_expected().into_iter().map(|r| vec![r[0].clone(), r[1].clone()]).collect();
    assert_eq!(rows, expected, "the resumed half continues the projected stream");
}
