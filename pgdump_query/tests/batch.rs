//! End-to-end tests for the row/batch layer over the hand-written
//! edge-case dump (see `tests/scan.rs` for the same fixture used at the
//! event-stream level).

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use pgdump_query::cache::CacheMode;
use pgdump_query::{
    Expr, LocalFileSource, NestedPlan, Predicate, PredicateOp, QueryOptions, ScanOptions,
    read_table, render_field,
};

mod common;
use common::{edge_cases, edge_cases_fixture, rows_of, types_fixture, widgets_expected};

/// Collect every batch `read_table` produces for `table`, as decoded rows,
/// along with each batch's row count (so batch-size-limit tests can see the
/// split points, not just the flattened data).
async fn collect(
    path: &Path,
    table: &str,
    scan_options: &ScanOptions,
    batch_options: &QueryOptions,
) -> (Vec<usize>, Vec<Vec<Option<String>>>) {
    let source = LocalFileSource::open(path).unwrap();
    let mut batch_sizes = Vec::new();
    let mut rows = Vec::new();

    read_table(&source, table, scan_options, batch_options, CacheMode::Disabled, |batch| {
        batch_sizes.push(batch.num_rows());
        rows.extend(rows_of(&batch));
        ControlFlow::Continue(())
    })
    .await
    .unwrap();

    (batch_sizes, rows)
}

async fn collect_with_predicate(
    path: &Path,
    table: &str,
    predicate: Predicate,
) -> pgdump_query::Result<Vec<Vec<Option<String>>>> {
    collect_with_filters(path, table, vec![predicate]).await
}

/// The same, for a conjunction: every term must match for a row to survive.
async fn collect_with_filters(
    path: &Path,
    table: &str,
    filters: Vec<Predicate>,
) -> pgdump_query::Result<Vec<Vec<Option<String>>>> {
    collect_with_expr(path, table, Expr::all(filters)).await
}

/// The same, for an arbitrary filter expression: a row survives where the
/// root evaluates true.
async fn collect_with_expr(
    path: &Path,
    table: &str,
    filter: Expr,
) -> pgdump_query::Result<Vec<Vec<Option<String>>>> {
    let source = LocalFileSource::open(path).unwrap();
    let mut rows = Vec::new();
    read_table(
        &source,
        table,
        &ScanOptions::default(),
        &QueryOptions { filter, ..Default::default() },
        CacheMode::Disabled,
        |batch| {
            rows.extend(rows_of(&batch));
            ControlFlow::Continue(())
        },
    )
    .await?;
    Ok(rows)
}

#[tokio::test]
async fn widgets_table_decodes_correctly() {
    let (_, rows) =
        collect(&edge_cases(), "public.widgets", &ScanOptions::default(), &QueryOptions::default())
            .await;
    assert_eq!(rows, widgets_expected());
}

#[tokio::test]
async fn predicate_eq_filters_to_matching_rows() {
    let rows = collect_with_predicate(
        &edge_cases(),
        "public.widgets",
        Predicate { column: "name".into(), op: PredicateOp::Eq, value: Some("beta".into()) },
    )
    .await
    .unwrap();
    assert_eq!(rows, vec![widgets_expected()[1].clone()]);
}

#[tokio::test]
async fn predicate_ne_excludes_the_matching_row() {
    let rows = collect_with_predicate(
        &edge_cases(),
        "public.widgets",
        Predicate { column: "name".into(), op: PredicateOp::Ne, value: Some("beta".into()) },
    )
    .await
    .unwrap();
    let expected: Vec<_> =
        widgets_expected().into_iter().filter(|r| r[1].as_deref() != Some("beta")).collect();
    assert_eq!(rows, expected);
}

/// Row 2 (`beta`) has a NULL `description`. Neither `=` nor `!=` against any
/// value selects it — SQL's own three-valued logic collapses both to
/// "excluded" — which is exactly why `IS [NOT] NULL` exists as its own
/// operator below, rather than trying to express it through `=`/`!=`
/// (`docs/status/history/2026-08-22.md`).
#[tokio::test]
async fn predicate_never_matches_a_null_field() {
    for op in [PredicateOp::Eq, PredicateOp::Ne] {
        let rows = collect_with_predicate(
            &edge_cases(),
            "public.widgets",
            Predicate { column: "description".into(), op, value: Some("a simple widget".into()) },
        )
        .await
        .unwrap();
        assert!(rows.iter().all(|r| r[1].as_deref() != Some("beta")), "{op:?}");
    }
}

#[tokio::test]
async fn predicate_is_null_and_is_not_null() {
    let null_rows = collect_with_predicate(
        &edge_cases(),
        "public.widgets",
        Predicate { column: "description".into(), op: PredicateOp::IsNull, value: None },
    )
    .await
    .unwrap();
    assert_eq!(null_rows, vec![widgets_expected()[1].clone()]);

    let not_null_rows = collect_with_predicate(
        &edge_cases(),
        "public.widgets",
        Predicate { column: "description".into(), op: PredicateOp::IsNotNull, value: None },
    )
    .await
    .unwrap();
    let expected: Vec<_> = widgets_expected().into_iter().filter(|r| r[2].is_some()).collect();
    assert_eq!(not_null_rows, expected);
}

/// Terms are ANDed: a row survives only if every one of them matches. Row 2
/// has a NULL `description` and row 3 a NULL `created_at`, so requiring both
/// to be present drops exactly those two.
#[tokio::test]
async fn every_term_of_a_conjunction_must_match() {
    let rows = collect_with_filters(
        &edge_cases(),
        "public.widgets",
        vec![
            Predicate { column: "description".into(), op: PredicateOp::IsNotNull, value: None },
            Predicate { column: "created_at".into(), op: PredicateOp::IsNotNull, value: None },
        ],
    )
    .await
    .unwrap();
    let expected: Vec<_> =
        widgets_expected().into_iter().filter(|r| r[2].is_some() && r[3].is_some()).collect();
    assert_eq!(rows, expected);
}

/// A disjunction keeps a row that satisfies either arm, and it is the whole
/// filter rather than a term inside one.
#[tokio::test]
async fn a_disjunction_keeps_either_arm() {
    let name = |v: &str| {
        Expr::Term(Predicate { column: "name".into(), op: PredicateOp::Eq, value: Some(v.into()) })
    };
    let rows = collect_with_expr(
        &edge_cases(),
        "public.widgets",
        Expr::Or(vec![name("alpha"), name("gamma")]),
    )
    .await
    .unwrap();
    let expected: Vec<_> = widgets_expected()
        .into_iter()
        .filter(|r| matches!(r[1].as_deref(), Some("alpha") | Some("gamma")))
        .collect();
    assert_eq!(rows, expected);
}

/// **`NOT` is why the evaluator had to become three-valued.** Row 2's
/// `description` is NULL, so `description = 'a simple widget'` is *unknown*
/// there rather than false, and `NOT unknown` is unknown — so negating an
/// equality drops the NULL row, exactly as PostgreSQL does. `IS DISTINCT
/// FROM` is the spelling that keeps it, and the two are asserted as the pair
/// they are.
#[tokio::test]
async fn not_drops_a_null_row_and_is_distinct_from_keeps_it() {
    let term =
        |op| Predicate { column: "description".into(), op, value: Some("a simple widget".into()) };
    let negated = collect_with_expr(
        &edge_cases(),
        "public.widgets",
        Expr::Not(Box::new(Expr::Term(term(PredicateOp::Eq)))),
    )
    .await
    .unwrap();
    let expected: Vec<_> = widgets_expected()
        .into_iter()
        .filter(|r| r[2].is_some() && r[2].as_deref() != Some("a simple widget"))
        .collect();
    assert_eq!(negated, expected);
    assert!(expected.iter().all(|r| r[1].as_deref() != Some("beta")), "the NULL row is dropped");

    let distinct = collect_with_expr(
        &edge_cases(),
        "public.widgets",
        Expr::Term(term(PredicateOp::IsDistinctFrom)),
    )
    .await
    .unwrap();
    let with_null: Vec<_> = widgets_expected()
        .into_iter()
        .filter(|r| r[2].as_deref() != Some("a simple widget"))
        .collect();
    assert_eq!(distinct, with_null);
    assert_eq!(with_null.len(), negated.len() + 1);
}

/// Nothing dedupes or contradicts terms: two terms on one column are
/// evaluated independently, so a pair no row can satisfy yields no rows
/// rather than an error.
#[tokio::test]
async fn a_contradictory_conjunction_yields_no_rows() {
    let rows = collect_with_filters(
        &edge_cases(),
        "public.widgets",
        vec![
            Predicate { column: "name".into(), op: PredicateOp::Eq, value: Some("alpha".into()) },
            Predicate { column: "name".into(), op: PredicateOp::Eq, value: Some("beta".into()) },
        ],
    )
    .await
    .unwrap();
    assert!(rows.is_empty());
}

/// An unknown column is refused wherever in the conjunction it sits, and the
/// error names that term's column rather than the first term's.
#[tokio::test]
async fn an_unknown_column_in_a_later_term_is_refused() {
    let err = collect_with_filters(
        &edge_cases(),
        "public.widgets",
        vec![
            Predicate { column: "name".into(), op: PredicateOp::IsNotNull, value: None },
            Predicate { column: "nope".into(), op: PredicateOp::Eq, value: Some("x".into()) },
        ],
    )
    .await
    .unwrap_err();
    match err {
        pgdump_query::Error::UnknownPredicateColumn { column, .. } => assert_eq!(column, "nope"),
        other => panic!("expected UnknownPredicateColumn, got {other:?}"),
    }
}

/// The resume token's query fingerprint covers the whole conjunction, not
/// just its first term: adding a term changes which rows come back, so a
/// token from the narrower query must not silently continue the wider one.
#[tokio::test]
async fn a_resume_token_covers_the_whole_conjunction() {
    use futures::StreamExt;
    use pgdump_query::table_stream;

    let source = LocalFileSource::open(edge_cases()).unwrap();
    let one = Predicate { column: "id".into(), op: PredicateOp::IsNotNull, value: None };
    let two = Predicate { column: "name".into(), op: PredicateOp::IsNotNull, value: None };
    let options =
        QueryOptions { max_rows: 1, filter: Expr::all([one.clone()]), ..QueryOptions::default() };
    let mut stream = table_stream(
        &source,
        "public.widgets",
        ScanOptions::default(),
        options.clone(),
        None,
        CacheMode::Disabled,
    );
    stream.next().await.unwrap().unwrap();
    let token = stream.resume_token();
    drop(stream);

    let mut wrong = table_stream(
        &source,
        "public.widgets",
        ScanOptions::default(),
        QueryOptions { filter: Expr::all([one.clone(), two]), ..options.clone() },
        Some(token.clone()),
        CacheMode::Disabled,
    );
    assert!(matches!(wrong.next().await, Some(Err(pgdump_query::Error::ResumeQueryMismatch))));

    // The stamp covers the tree's *shape*, not just its terms: one term
    // under `Or` is a different query from the same term under `And`, even
    // though the two select the same rows.
    let mut reshaped = table_stream(
        &source,
        "public.widgets",
        ScanOptions::default(),
        QueryOptions {
            filter: pgdump_query::Expr::Or(vec![pgdump_query::Expr::Term(one)]),
            ..options.clone()
        },
        Some(token.clone()),
        CacheMode::Disabled,
    );
    assert!(matches!(reshaped.next().await, Some(Err(pgdump_query::Error::ResumeQueryMismatch))));

    let mut resumed = table_stream(
        &source,
        "public.widgets",
        ScanOptions::default(),
        options,
        Some(token),
        CacheMode::Disabled,
    );
    assert!(resumed.next().await.unwrap().is_ok(), "the same conjunction resumes");
}

/// A predicate on a nested column is where the two schema modes part
/// company, and deliberately: a column typed `List<Utf8View>` is compared
/// **structurally**, through the register's nested plan, while
/// `SchemaMode::Strings` resolves no column at all and so falls back to the
/// byte comparison every column made before types existed.
///
/// They agree on the canonical spelling the file holds — which is the case
/// that must not move — and disagree on `{NULL, plain}`, which the typed path
/// reads through the `array_in` superset and the untyped path reads as bytes.
#[tokio::test]
async fn a_nested_predicate_compares_structurally_when_the_column_is_typed() {
    use futures::StreamExt;
    use pgdump_query::table_stream;

    async fn ids(path: &Path, mode: pgdump_query::SchemaMode, value: &str) -> Vec<Option<String>> {
        let source = LocalFileSource::open(path).unwrap();
        let predicate = Predicate {
            column: "v_text_special".into(),
            op: PredicateOp::Eq,
            value: Some(value.to_string()),
        };
        let options = QueryOptions {
            schema_mode: mode,
            filter: Expr::all([predicate]),
            ..Default::default()
        };
        let mut stream = table_stream(
            &source,
            "public.t_array",
            ScanOptions::default(),
            options,
            None,
            CacheMode::Disabled,
        );
        let mut ids = Vec::new();
        while let Some(batch) = stream.next().await {
            let batch = batch.unwrap();
            for row in 0..batch.num_rows() {
                // `id` is `integer` in both modes; only the array column's
                // type changed underneath.
                ids.push(
                    render_field(batch.column(0).as_ref(), row, &NestedPlan::Scalar)
                        .expect("renders back"),
                );
            }
        }
        ids
    }

    for version in [13, 16, 18] {
        let path = types_fixture(version, "default");
        let matched = r#"{NULL,plain}"#;
        assert_eq!(
            ids(&path, pgdump_query::SchemaMode::Typed, matched).await,
            vec![Some("2".to_string())],
            "pg_dump {version}"
        );
        assert_eq!(
            ids(&path, pgdump_query::SchemaMode::Typed, matched).await,
            ids(&path, pgdump_query::SchemaMode::Strings, matched).await,
            "pg_dump {version}: the canonical spelling matches the same rows either way"
        );
        // A spelling `array_in` accepts and `array_out` never writes: the
        // typed path takes it, the untyped one cannot, because it has no
        // declared type to read a container grammar out of.
        assert_eq!(
            ids(&path, pgdump_query::SchemaMode::Typed, "{NULL, plain}").await,
            vec![Some("2".to_string())],
            "pg_dump {version}"
        );
        assert!(
            ids(&path, pgdump_query::SchemaMode::Strings, "{NULL, plain}").await.is_empty(),
            "pg_dump {version}"
        );
    }
}

#[tokio::test]
async fn predicate_on_unknown_column_errors() {
    let err = collect_with_predicate(
        &edge_cases(),
        "public.widgets",
        Predicate { column: "nope".into(), op: PredicateOp::Eq, value: Some("x".into()) },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, pgdump_query::Error::UnknownPredicateColumn { .. }));
}

#[tokio::test]
async fn table_matching_is_bare_or_qualified() {
    for name in ["widgets", "public.widgets"] {
        let (_, rows) =
            collect(&edge_cases(), name, &ScanOptions::default(), &QueryOptions::default()).await;
        assert_eq!(rows, widgets_expected(), "matching on {name}");
    }
}

#[tokio::test]
async fn empty_table_produces_no_batches() {
    let (sizes, rows) = collect(
        &edge_cases(),
        "public.empty_table",
        &ScanOptions::default(),
        &QueryOptions::default(),
    )
    .await;
    assert!(sizes.is_empty());
    assert!(rows.is_empty());
}

#[tokio::test]
async fn unmatched_table_produces_no_batches() {
    let (sizes, rows) =
        collect(&edge_cases(), "no.such.table", &ScanOptions::default(), &QueryOptions::default())
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
        &QueryOptions::default(),
        CacheMode::Disabled,
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
        &QueryOptions::default(),
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
        let options = QueryOptions { max_rows, max_bytes: None, ..Default::default() };
        let (sizes, rows) =
            collect(&edge_cases(), "public.widgets", &ScanOptions::default(), &options).await;
        assert_eq!(rows, widgets_expected(), "max_rows {max_rows}");
        assert_eq!(sizes.iter().sum::<usize>(), 6, "max_rows {max_rows}");
        assert!(sizes.iter().all(|&n| n <= max_rows), "max_rows {max_rows}: sizes {sizes:?}");
    }
}

/// The third flush trigger splits a batch on the *source* span it covers,
/// which is the only one of the three that bounds what an in-flight batch
/// pins: `max_rows` counts selected rows and `max_bytes` selected field
/// bytes, and a filter makes both arbitrarily sparse in the file. Both are
/// disabled here so the split points are the span's alone, and the decoded
/// data must be identical however they land — including under a `chunk_size`
/// small enough that a mid-block flush lands between two views into the same
/// chunk, which is what `invalidate_block_cache` is there for.
#[tokio::test]
async fn max_source_span_splits_batches() {
    let reference = collect(
        &edge_cases_fixture(16, "default"),
        "public.escapes",
        &ScanOptions::default(),
        &QueryOptions::default(),
    )
    .await
    .1;
    assert_eq!(reference.len(), 132);

    let mut previous = 0;
    for max_source_span in [1 << 20, 512, 64, 16, 1] {
        for chunk_size in [7, 4096, 1 << 20] {
            let options = QueryOptions {
                max_rows: usize::MAX,
                max_bytes: None,
                max_source_span: Some(max_source_span),
                ..Default::default()
            };
            let scan = ScanOptions { chunk_size, ..Default::default() };
            let (sizes, rows) =
                collect(&edge_cases_fixture(16, "default"), "public.escapes", &scan, &options)
                    .await;
            assert_eq!(rows, reference, "span {max_source_span} chunk {chunk_size}");
            assert_eq!(sizes.iter().sum::<usize>(), 132);
            if chunk_size == 7 {
                // Tighter caps never split less; the widest is one batch and
                // the tightest is one batch per row.
                assert!(
                    sizes.len() >= previous,
                    "span {max_source_span}: {} batches, was {previous}",
                    sizes.len()
                );
                previous = sizes.len();
            }
        }
    }
    assert_eq!(previous, 132, "a one-byte cap flushes after every row");
}

/// `None` restores the pre-trigger behaviour: with the other two triggers off
/// as well, a block is one batch however far apart its rows are.
#[tokio::test]
async fn a_none_source_span_leaves_the_block_as_one_batch() {
    let options = QueryOptions {
        max_rows: usize::MAX,
        max_bytes: None,
        max_source_span: None,
        ..Default::default()
    };
    let (sizes, rows) = collect(
        &edge_cases_fixture(16, "default"),
        "public.escapes",
        &ScanOptions::default(),
        &options,
    )
    .await;
    assert_eq!(sizes, vec![132]);
    assert_eq!(rows.len(), 132);
}

/// Fields eligible for a zero-copy view (no escapes) are only zero-copy when
/// they land fully inside one read chunk; a small `chunk_size` forces most
/// fields — and the multi-byte escaped ones — through every code path
/// (zero-copy view, chunk-straddling copy, and decode-then-copy). The
/// decoded result must be identical regardless.
#[tokio::test]
async fn batch_contents_are_independent_of_chunk_size() {
    let reference =
        collect(&edge_cases(), "public.widgets", &ScanOptions::default(), &QueryOptions::default())
            .await
            .1;
    for chunk_size in [1, 2, 3, 7, 13, 64, 511, 4096] {
        let options = ScanOptions { chunk_size, ..Default::default() };
        let (_, rows) =
            collect(&edge_cases(), "public.widgets", &options, &QueryOptions::default()).await;
        assert_eq!(rows, reference, "chunk_size {chunk_size}");
    }
}

#[tokio::test]
async fn stops_early_on_break() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let options = QueryOptions { max_rows: 1, max_bytes: None, ..Default::default() };
    let mut batches = 0;
    let (_, token) = read_table(
        &source,
        "public.widgets",
        &ScanOptions::default(),
        &options,
        CacheMode::Disabled,
        |_| {
            batches += 1;
            ControlFlow::Break(())
        },
    )
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
    let options = QueryOptions { max_rows: 1, max_bytes: None, ..Default::default() };
    let mut rows = Vec::new();
    let (_, token) = read_table(
        &source,
        "public.widgets",
        &ScanOptions::default(),
        &options,
        CacheMode::Disabled,
        |batch| {
            rows.extend(rows_of(&batch));
            ControlFlow::Break(())
        },
    )
    .await
    .unwrap();
    let token = token.expect("Break yields a resume token");

    let mut resumed = table_stream(
        &source,
        "public.widgets",
        ScanOptions::default(),
        options,
        Some(token),
        CacheMode::Disabled,
    );
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
///
/// Runs in both `SchemaMode`s — the one pair of existing tests that does
/// (`docs/design/decisions.md`, "D73"): `codepoint` is `integer` (Int32 in `Typed`, Utf8View
/// in `Strings`) but `value` (`text`) is `Utf8View` either way, so both modes
/// must agree once rendered back through `rows_of`.
#[tokio::test]
async fn escapes_table_round_trips_through_postgres_batched() {
    use pgdump_query::resolve::SchemaMode;
    for version in [13, 16, 18] {
        for schema_mode in [SchemaMode::Typed, SchemaMode::Strings] {
            let options =
                QueryOptions { max_rows: 17, max_bytes: None, schema_mode, ..Default::default() };
            let (_, rows) = collect(
                &edge_cases_fixture(version, "default"),
                "public.escapes",
                &ScanOptions::default(),
                &options,
            )
            .await;

            assert_eq!(rows.len(), 132, "pg_dump {version} {schema_mode:?}");
            for row in &rows {
                let codepoint: u32 = row[0].as_deref().unwrap().parse().unwrap();
                let expected = char::from_u32(codepoint).unwrap().to_string();
                assert_eq!(
                    row[1].as_deref(),
                    Some(expected.as_str()),
                    "pg_dump {version} {schema_mode:?} codepoint {codepoint}"
                );
            }
        }
    }
}

/// `query` requires a live source and can never answer from a cache alone —
/// `Span::text` is `None` for every `Data` span regardless of this decision
/// (`docs/design/decisions.md`, "The compressed source and the cache") — so `read_table` rejects `CacheMode::Offline` up front
/// rather than silently doing the wrong thing.
#[tokio::test]
async fn read_table_rejects_offline_cache_mode() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let result = read_table(
        &source,
        "widgets",
        &ScanOptions::default(),
        &QueryOptions::default(),
        CacheMode::Offline(PathBuf::from("/nonexistent.dqcache")),
        |_batch| ControlFlow::Continue(()),
    )
    .await;
    assert!(matches!(result, Err(pgdump_query::Error::CacheModeMismatch(_))));
}
