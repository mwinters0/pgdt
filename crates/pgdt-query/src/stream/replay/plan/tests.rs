use pgdt_codec::SchemaMode;
use pgdt_codec::compare::ComparisonSemantics;
use pgdt_core::Error;
use pgdt_dump::cache::CacheMode;
use pgdt_io::ByteRangeSource;
use pgdt_model::{ArrayShape, DumpMetadata};
use std::sync::Arc;

use super::resolve_block;
use crate::predicate::{Expr, PredicateOp, resolve_expr};
use crate::prune::prune_block;
use pgdt_dump::ReadOptions;
use pgdt_fixtures::arranged;
use pgdt_model::PostgresInvalidValues;
use tokio_util::sync::CancellationToken;

/// **The guard no caller can trip, pinned so it stays that way.**
/// [`resolve_block`] refuses `Typed` resolution against metadata with no
/// complete entry for the block's database, which every call site
/// satisfies by construction — so `Error::MetadataNotScanned` is
/// unreachable but through a hand-built `DumpIndex`, and this is what stands
/// between a future reordering and a silently wrongly-typed row.
#[test]
fn resolving_a_block_against_a_database_the_metadata_lacks_refuses() {
    use pgdt_model::DatabaseMetadata;

    let header = pgdt_core::parse_copy_header(b"COPY public.t (id) FROM stdin;")
        .expect("a well-formed header");
    let first = DatabaseMetadata {
        name: Some("first".to_string()),
        preamble_complete: true,
        server_version: None,
        pg_dump_version: None,
        extensions: Vec::new(),
        types: Vec::new(),
        collations: Vec::new(),
        tables: Default::default(),
    };
    let metadata = DumpMetadata { databases: vec![first] };

    let err = resolve_block(
        &header,
        Some(&metadata),
        Some("second"),
        SchemaMode::Typed,
        &[],
        &[],
        ComparisonSemantics::Postgres,
    )
    .expect_err("the metadata has no entry for `second`");
    assert!(
        matches!(&err, Error::MetadataNotScanned { database } if database.as_deref() == Some("second")),
        "{err:?}"
    );

    // The same block in a database the metadata covers resolves, so the
    // refusal is about coverage and not about the lookup failing.
    assert!(
        resolve_block(
            &header,
            Some(&metadata),
            Some("first"),
            SchemaMode::Typed,
            &[],
            &[],
            ComparisonSemantics::Postgres
        )
        .is_ok()
    );

    // `Strings` never looks, so it is never refused.
    assert!(
        resolve_block(
            &header,
            Some(&metadata),
            Some("second"),
            SchemaMode::Strings,
            &[],
            &[],
            ComparisonSemantics::Postgres
        )
        .is_ok()
    );
}

/// **A block prunes in the view its query reads its statistics in**: over
/// a `date` column, one value a group, a value past the format spec is
/// its rank in every value's view and a NULL in the representable one, and
/// one past the calendar is a NULL in the displayable one too — so a
/// bound rules each out, or `IS NULL` keeps it, by the view — and a column
/// sorted but for an infinity is sorted, and stops a read, only where the
/// infinity is a NULL.
#[tokio::test]
async fn a_block_prunes_in_the_view_its_query_reads() {
    use crate::predicate::Predicate;
    use pgdt_io::LocalFileSource;
    use pgdt_mapping::map_file;
    use pgdt_mapping::statistics::{StatisticsRequest, StatisticsSizing};
    use pgdt_model::StatisticsView;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dates.sql");
    std::fs::write(
        &path,
        "CREATE TABLE public.t (\n    d date\n);\n\n\
             CREATE TABLE public.u (\n    d date\n);\n\n\
             COPY public.t (d) FROM stdin;\n\
             2024-01-01\ninfinity\n-infinity\n262143-01-01\n\\N\n\\.\n\n\
             COPY public.u (d) FROM stdin;\n\
             2020-01-01\n2021-01-01\n2022-01-01\n-infinity\n\\.\n",
    )
    .unwrap();
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&path).unwrap());
    // One row a group: a row's first byte is all a group holds.
    let request = StatisticsRequest {
        sizing: StatisticsSizing::Stated(std::num::NonZeroU64::new(1).unwrap()),
        ..StatisticsRequest::DATA
    };
    let run = map_file(
        CacheMode::DISABLED.open_source(Arc::clone(&source)).await.unwrap(),
        ReadOptions::default(),
        &arranged(1),
        PostgresInvalidValues::Default,
        &request,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let metadata = run.index.metadata.as_deref();
    let filter = |table: &str, op: PredicateOp, value: Option<&str>| {
        let block = run.index.blocks().find(|b| b.header.matches(table)).unwrap();
        let census = vec![ArrayShape::default(); block.header.columns.len()];
        let full = resolve_block(
            &block.header,
            metadata,
            None,
            SchemaMode::Typed,
            &census,
            &[],
            ComparisonSemantics::Postgres,
        )
        .unwrap();
        let term = Predicate { column: "d".into(), op, value: value.map(str::to_string) };
        let expr = Expr::all([term]);
        (
            block,
            resolve_expr(&expr, &full, block.header_offset, ComparisonSemantics::Postgres).unwrap(),
        )
    };
    // The values each view's pruning keeps a group of, in file order.
    let kept = |table: &str, op: PredicateOp, value: Option<&str>, view: StatisticsView| {
        let (block, filter) = filter(table, op, value);
        let pruning = prune_block(block, &filter, metadata, None, view).unwrap();
        let statistics = block.statistics.as_ref().unwrap();
        let values = ["2024-01-01", "infinity", "-infinity", "262143-01-01", "NULL"];
        let holding: Vec<usize> =
            (0..statistics.groups.len()).filter(|&g| statistics.groups[g].rows > 0).collect();
        assert_eq!(holding.len(), values.len(), "one row a group");
        holding
            .iter()
            .zip(values)
            .filter(|&(&g, _)| pruning.kept_groups[g])
            .map(|(_, value)| value)
            .collect::<Vec<_>>()
    };
    use StatisticsView::{Displayable, Every, Representable};
    let later = |view| kept("public.t", PredicateOp::Gt, Some("2030-01-01"), view);
    assert_eq!(later(Every), ["infinity", "262143-01-01"]);
    assert_eq!(later(Representable), ["262143-01-01"]);
    assert_eq!(later(Displayable), Vec::<&str>::new());
    let null = |view| kept("public.t", PredicateOp::IsNull, None, view);
    assert_eq!(null(Every), ["NULL"]);
    assert_eq!(null(Representable), ["infinity", "-infinity", "NULL"]);
    assert_eq!(null(Displayable), ["infinity", "-infinity", "262143-01-01", "NULL"]);

    let stops = |view| {
        let (block, filter) = filter("public.u", PredicateOp::Lt, Some("2021-06-01"));
        prune_block(block, &filter, metadata, None, view).unwrap().stop.is_some()
    };
    assert!(!stops(Every), "-infinity closes the column out of order");
    assert!(stops(Representable) && stops(Displayable));
}
