//! Column projection: what a query materializes, and what it therefore
//! never decodes (`docs/design/decisions.md`, "D28").
//!
//! The library-level tests only: everything here drives a cold query
//! directly. The flags are pinned separately, in
//! `crates/pgdt/tests/query_projection.rs`.

use std::path::Path;
use std::sync::Arc;

use futures::StreamExt;
use pgdt_codec::ColumnResolution;
use pgdt_codec::UnrepresentableMode;
use pgdt_core::Error;
use pgdt_io::{ByteRangeSource, LocalFileSource};
use pgdt_query::{QueryOptions, TablePartitions};

mod common;
use common::{Cold, edge_cases, rows_of, types_fixture, widgets_expected};

/// `path` opened as a source a cold query reads.
fn open(path: impl AsRef<Path>) -> Arc<dyn ByteRangeSource> {
    Arc::new(LocalFileSource::open(path).unwrap())
}

/// The plan of a cold query of `table` in the dump at `path` under
/// `options`, which the dump holds; a refusal its pass or its plan raises is
/// the `Err`.
async fn plan(
    path: &Path,
    table: &str,
    options: QueryOptions,
) -> pgdt_core::Result<TablePartitions> {
    let plan = Cold::default().plan(open(path), table, options).await?;
    Ok(plan.expect("the dump holds the table"))
}

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
) -> pgdt_core::Result<(Vec<String>, Vec<Vec<Option<String>>>)> {
    // Asked first, as `pgdt query` asks it: a request refused whatever the
    // dump holds.
    options.validate()?;
    let mut names = Vec::new();
    let mut rows = Vec::new();
    let stream = Cold::default().stream(open(edge_cases()), table, options).await?;
    let Some(mut stream) = stream else { return Ok((names, rows)) };
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
    let planned = plan(&edge_cases(), "public.widgets", projecting(&["created_at", "id"])).await;
    let mut stream = planned.unwrap().in_file_order(8192);
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
    let names: Vec<String> = resolved.schema().fields().iter().map(|f| f.name().clone()).collect();
    assert_eq!(names, vec!["created_at".to_string(), "id".to_string()]);
    assert_eq!(resolved.columns().iter().map(|c| c.name().to_string()).collect::<Vec<_>>(), names);
    assert_eq!(resolved.columns().len(), 2);
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
        filter: pgdt_query::Expr::all([pgdt_query::Predicate {
            column: "name".into(),
            op: pgdt_query::PredicateOp::Eq("beta".into()),
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

/// **Whether a refusing query succeeds depends on its projection.**
/// `t_numeric.v_small` carries a `NaN` that `Decimal128` cannot represent, so
/// under the refuse mode the whole table is `Error::Unrepresentable`, at planning;
/// projecting that column away is a per-column escape beside `--schema-mode
/// strings`, and it leaves every other column typed.
#[tokio::test]
async fn projecting_a_column_away_escapes_its_decode_failure() {
    for version in [13, 16, 18] {
        let path = types_fixture(version, "default");

        let run = async |options: QueryOptions| {
            let mut stream = plan(&path, "public.t_numeric", options).await?.in_file_order(8192);
            let mut rows = Vec::new();
            while let Some(batch) = stream.next().await.transpose()? {
                rows.extend(rows_of(&batch));
            }
            Ok::<_, Error>((stream.resolved_schema(), rows))
        };

        let refusing = |options: QueryOptions| QueryOptions {
            unrepresentable: UnrepresentableMode::Refuse,
            ..options
        };
        assert!(
            matches!(
                run(refusing(QueryOptions::default())).await,
                Err(Error::Unrepresentable { .. })
            ),
            "pg_dump {version}: the whole table still fails"
        );
        let (resolved, rows) = run(refusing(projecting(&["id", "v_typed"])))
            .await
            .unwrap_or_else(|e| panic!("pg_dump {version}: {e}"));
        assert!(!rows.is_empty(), "pg_dump {version}");
        assert!(
            resolved.columns().iter().all(|c| *c.resolution() == ColumnResolution::Mapped),
            "pg_dump {version}: the surviving columns are still typed: {:?}",
            resolved.columns()
        );
        // Projecting it back in restores the failure, so the escape is the
        // projection and not something else about the narrower query.
        assert!(
            matches!(
                run(refusing(projecting(&["id", "v_small"]))).await,
                Err(Error::Unrepresentable { column, .. }) if column == "v_small"
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
    let options = QueryOptions { max_bytes: None, ..projecting(&["id", "name"]) };
    let planned = plan(&edge_cases(), "public.widgets", options).await.unwrap();
    let mut stream = planned.in_file_order(1);
    let mut rows = rows_of(&stream.next().await.unwrap().unwrap());
    let token = stream.resume_token();
    drop(stream);

    let reordered = QueryOptions { max_bytes: None, ..projecting(&["name", "id"]) };
    let wrong = plan(&edge_cases(), "public.widgets", reordered).await.unwrap();
    assert!(
        matches!(wrong.resume(token.clone(), 1), Err(Error::ResumeQueryMismatch)),
        "a reordered projection is a different query"
    );

    let mut resumed = planned.resume(token, 1).unwrap();
    while let Some(batch) = resumed.next().await {
        rows.extend(rows_of(&batch.unwrap()));
    }
    let expected: Vec<Vec<Option<String>>> =
        widgets_expected().into_iter().map(|r| vec![r[0].clone(), r[1].clone()]).collect();
    assert_eq!(rows, expected, "the resumed half continues the projected stream");
}
