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
    let filter = |table: &str, op: PredicateOp| {
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
        let term = Predicate { column: "d".into(), op };
        let expr = Expr::all([term]);
        (
            block,
            resolve_expr(&expr, &full, block.address.header_offset, ComparisonSemantics::Postgres)
                .unwrap(),
        )
    };
    // The values each view's pruning keeps a group of, in file order.
    let kept = |table: &str, op: PredicateOp, view: StatisticsView| {
        let (block, filter) = filter(table, op);
        let pruning = prune_block(block, &filter, metadata, None, view).unwrap();
        let statistics = block.record.statistics.as_ref().unwrap();
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
    let later = |view| kept("public.t", PredicateOp::Gt("2030-01-01".into()), view);
    assert_eq!(later(Every), ["infinity", "262143-01-01"]);
    assert_eq!(later(Representable), ["262143-01-01"]);
    assert_eq!(later(Displayable), Vec::<&str>::new());
    let null = |view| kept("public.t", PredicateOp::IsNull, view);
    assert_eq!(null(Every), ["NULL"]);
    assert_eq!(null(Representable), ["infinity", "-infinity", "NULL"]);
    assert_eq!(null(Displayable), ["infinity", "-infinity", "262143-01-01", "NULL"]);

    let stops = |view| {
        let (block, filter) = filter("public.u", PredicateOp::Lt("2021-06-01".into()));
        prune_block(block, &filter, metadata, None, view).unwrap().stop.is_some()
    };
    assert!(!stops(Every), "-infinity closes the column out of order");
    assert!(stops(Representable) && stops(Displayable));
}

/// **A query has one factor**: the largest of each column its batch reads
/// and each term its filter keys by positions, none where neither grows
/// with a line — a `date[]` holding a value its type cannot hold, its tier
/// test a walk of its own staging nothing, holding what its decode's walk
/// does, and a nested term ordering by its parts holding none.
#[tokio::test]
async fn a_query_s_factor_is_its_columns_and_terms_largest() {
    use crate::Predicate;
    use crate::batch::Factor;
    use crate::options::QueryOptions;
    use pgdt_io::LocalFileSource;
    use pgdt_mapping::map_file;
    use pgdt_mapping::statistics::StatisticsRequest;
    use pgdt_model::CopyBlock;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("factors.sql");
    std::fs::write(
        &path,
        "CREATE TABLE public.t (\n    id integer,\n    t text,\n    a bigint[],\n    \
             j jsonb,\n    d date[]\n);\n\n\
             COPY public.t (id, t, a, j, d) FROM stdin;\n\
             1\tx\t{1,2}\t{\"a\": 1}\t{2020-01-01,infinity}\n\\.\n",
    )
    .unwrap();
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&path).unwrap());
    let run = map_file(
        CacheMode::DISABLED.open_source(Arc::clone(&source)).await.unwrap(),
        ReadOptions::default(),
        &arranged(1),
        PostgresInvalidValues::Default,
        &StatisticsRequest::DATA,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let matches: Vec<CopyBlock> = run.index.blocks().cloned().collect();
    let factor = |columns: &[&str], filter: Vec<Predicate>| {
        let query_options = QueryOptions {
            projection: Some(columns.iter().map(|c| c.to_string()).collect()),
            filter: Expr::all(filter),
            ..QueryOptions::default()
        };
        super::ReplayPlan::new(
            ReadOptions::default(),
            query_options,
            pgdt_io::Parallelism::default(),
            None,
            "public.t",
            &matches,
            run.index.metadata.clone(),
        )
        .unwrap()
        .factor()
    };
    let term = |column: &str, op| Predicate { column: column.into(), op };
    let jsonb = || term("j", PredicateOp::Gt("{}".into()));

    assert_eq!(factor(&["id"], vec![]), None);
    assert_eq!(factor(&[], vec![]), None);
    assert_eq!(factor(&["t"], vec![]), Some(Factor::new(1, 1)));
    assert_eq!(factor(&["id", "a", "t"], vec![]), Some(Factor::new(65, 16)));
    assert_eq!(factor(&["id"], vec![jsonb()]), Some(Factor::new(4, 5)));
    assert_eq!(factor(&["t"], vec![jsonb()]), Some(Factor::new(1, 1)));
    assert_eq!(factor(&["id"], vec![term("a", PredicateOp::Gt("{1}".into()))]), None);
    assert_eq!(factor(&["d"], vec![]), Some(Factor::new(33, 16)));
}

/// **An instrumented binary reports the factor its replay planned**:
/// `plan_replay` hands each plan's to the instrument, which keeps the
/// largest. Exact under nextest, which runs each test in its own process.
#[cfg(feature = "introspect")]
#[tokio::test]
async fn a_planned_replay_hands_its_factor_to_the_instrument() {
    use crate::Predicate;
    use crate::options::QueryOptions;
    use pgdt_core::instrument::factor_reading;
    use pgdt_io::LocalFileSource;
    use pgdt_mapping::map_file;
    use pgdt_mapping::statistics::StatisticsRequest;
    use pgdt_model::CopyBlock;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("planned.sql");
    std::fs::write(
        &path,
        "CREATE TABLE public.t (\n    id integer,\n    a bigint[],\n    j jsonb\n);\n\n\
             COPY public.t (id, a, j) FROM stdin;\n\
             1\t{1,2}\t{\"a\": 1}\n\\.\n",
    )
    .unwrap();
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&path).unwrap());
    let run = map_file(
        CacheMode::DISABLED.open_source(Arc::clone(&source)).await.unwrap(),
        ReadOptions::default(),
        &arranged(1),
        PostgresInvalidValues::Default,
        &StatisticsRequest::DATA,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let matches: Vec<CopyBlock> = run.index.blocks().cloned().collect();
    let plan = |columns: &[&str], filter: Vec<Predicate>| {
        let query_options = QueryOptions {
            projection: Some(columns.iter().map(|c| c.to_string()).collect()),
            filter: Expr::all(filter),
            ..QueryOptions::default()
        };
        super::plan_replay(
            source.as_ref(),
            None,
            "public.t",
            &matches,
            run.index.metadata.clone(),
            ReadOptions::default(),
            query_options,
            pgdt_io::Parallelism::default(),
        )
        .unwrap()
        .plan
        .factor()
        .map(crate::batch::Factor::parts)
    };

    assert_eq!(factor_reading(), None);
    assert_eq!(plan(&["id"], vec![]), None);
    assert_eq!(factor_reading(), Some(None));
    let jsonb = Predicate { column: "j".into(), op: PredicateOp::Gt("{}".into()) };
    let keyed = plan(&["id"], vec![jsonb]);
    assert_eq!(factor_reading(), Some(keyed));
    let decoded = plan(&["a"], vec![]);
    assert_eq!(factor_reading(), Some(decoded));
    plan(&["id"], vec![]);
    assert_eq!(factor_reading(), Some(decoded), "the largest a process planned is kept");
}
