use pgdt_io::ByteRangeSource;
use std::sync::Arc;

use pgdt_codec::SchemaMode;
use pgdt_codec::compare::ComparisonSemantics;
use pgdt_dump::cache::CacheMode;
use pgdt_model::{ArrayShape, BoundsSet, StatisticsView};

use crate::predicate::{Expr, PredicateOp, resolve_expr};
use crate::prune::DynamicPruning;
use crate::stream::replay::plan::resolve_block;
use pgdt_dump::ReadOptions;
use pgdt_fixtures::arranged;
use pgdt_model::PostgresInvalidValues;
use tokio_util::sync::CancellationToken;

/// **A dynamic filter's stop is armed only in a group a row of which can
/// pass it**: under `id < 150` over ascending ids, of the groups the
/// state keeps only the one whose maximum reaches the bound is asked
/// row by row, so the groups before it pay nothing for the stop.
#[tokio::test]
async fn a_dynamic_stop_is_armed_only_where_a_row_can_pass_it() {
    use crate::predicate::Predicate;
    use pgdt_io::LocalFileSource;
    use pgdt_mapping::map_file;
    use pgdt_mapping::statistics::{StatisticsRequest, StatisticsSelection, StatisticsSizing};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ascending.sql");
    let rows: String = (1..=200).map(|id| format!("{id}\n")).collect();
    std::fs::write(
        &path,
        format!(
            "CREATE TABLE public.t (\n    id integer\n);\n\n\
                 COPY public.t (id) FROM stdin;\n{rows}\\.\n"
        ),
    )
    .unwrap();
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&path).unwrap());
    let request = StatisticsRequest {
        selection: StatisticsSelection::DATA,
        sizing: StatisticsSizing::Stated(std::num::NonZeroU64::new(64).unwrap()),
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
    let block = run.index.blocks().find(|b| b.header.matches("public.t")).unwrap();
    let statistics = Arc::clone(block.statistics.as_ref().unwrap());
    let metadata = run.index.metadata.as_deref();
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
    let below = Expr::all([Predicate {
        column: "id".into(),
        op: PredicateOp::Lt,
        value: Some("150".into()),
    }]);
    let filter =
        resolve_expr(&below, &full, block.header_offset, ComparisonSemantics::Postgres).unwrap();
    let reading = pgdt_model::StatisticsView::Every;
    let mut pruning =
        DynamicPruning::new(block, Arc::clone(&statistics), metadata, None, reading).unwrap();
    pruning.read(Arc::new(filter));

    let id = statistics.columns[0].as_ref().unwrap();
    let (mut armed, mut reaching) = (Vec::new(), Vec::new());
    for group in 0..pruning.groups() {
        if !pruning.keeps(group) {
            continue;
        }
        pruning.arm(group);
        armed.push(pruning.stop().is_some());
        let bounds =
            id.group_bounds(BoundsSet::Primary, StatisticsView::Representable, group).unwrap();
        let max: i64 = bounds.max.parse().unwrap();
        reaching.push(max >= 150);
    }
    assert!(armed.len() > 3, "{armed:?}");
    assert_eq!(armed, reaching);
    assert_eq!(armed.iter().filter(|&&armed| armed).count(), 1, "{armed:?}");
}
