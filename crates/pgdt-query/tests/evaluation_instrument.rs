//! The introspection build's account of a dynamic filter's row evaluation
//! (`pgdt_core::instrument`, `EvaluationPart`): what it times, and where.
//!
//! **Counts, not times, are what is asserted.** A time is a reading, taken
//! from a commit on the input a figure is (`measure.py --profile-recipe`);
//! what a test can hold is that the instrument times the spans it names —
//! one row per row the state is evaluated on, each leaf's parts once per leaf
//! reached — and nothing outside a row, so a reading divides by what it
//! claims to. Its own test binary, because its counters are the process's.
#![cfg(feature = "introspect")]

use std::num::NonZeroU64;
use std::sync::Arc;

use futures::StreamExt;
use pgdt_core::instrument::{EvaluationPart, evaluation_reading};
use pgdt_dump::cache::{self, CacheMode};
use pgdt_io::{ByteRangeSource, LocalFileSource, Parallelism};
use pgdt_mapping::map_file;
use pgdt_mapping::statistics::{StatisticsRequest, StatisticsSelection, StatisticsSizing};
use pgdt_query::{
    Dump, DynamicFilter, Expr, Predicate, PredicateOp, QueryOptions, RowEvaluation, TableSelector,
};

mod common;
use common::{VERSIONS, sandboxed, statistics_fixture};
use pgdt_dump::ReadOptions;
use pgdt_fixtures::arranged;
use pgdt_model::PostgresInvalidValues;
use tokio_util::sync::CancellationToken;

/// `ordered`'s rows: `id` ascends from 1 to this.
const ROWS: u64 = 1000;
/// The values the membership holds, `1..=IN_VALUES`.
const IN_VALUES: u64 = 150;

/// A dynamic filter whose state never moves.
struct Constant(Arc<Expr>);

impl DynamicFilter for Constant {
    fn generation(&self) -> u64 {
        1
    }

    fn current(&self) -> (u64, Arc<Expr>) {
        (1, Arc::clone(&self.0))
    }
}

fn term(op: fn(String) -> PredicateOp, value: &str) -> Expr {
    Expr::Term(Predicate { column: "id".into(), op: op(value.into()) })
}

/// A join's filter as the provider hands it over: the build side's bounds,
/// then its keys — here bounds every row meets, and keys a few of them are.
fn join_filter() -> Expr {
    Expr::And(vec![
        term(PredicateOp::Ge, "1"),
        term(PredicateOp::Le, &ROWS.to_string()),
        Expr::Term(Predicate {
            column: "id".into(),
            op: PredicateOp::In((1..=IN_VALUES).map(|v| Some(v.to_string())).collect()),
        }),
    ])
}

/// Each part's span count now.
fn counts() -> Vec<u64> {
    let reading = evaluation_reading();
    EvaluationPart::ALL.into_iter().map(|part| reading.count(part)).collect()
}

/// `ordered`'s `id`, read serially with statistics off — so every row is
/// evaluated where `evaluation` evaluates rows — under `static_filter` as the
/// static filter, `dynamic` as the dynamic one, or both; the rows emitted.
async fn read(
    static_filter: Option<Expr>,
    dynamic: Option<Expr>,
    evaluation: RowEvaluation,
) -> u64 {
    let version = *VERSIONS.last().unwrap();
    let (_dir, dump) = sandboxed(&statistics_fixture(version, "default"), "dump.sql");
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&dump).unwrap());
    let request = StatisticsRequest {
        selection: StatisticsSelection::DATA,
        sizing: StatisticsSizing::Stated(NonZeroU64::new(1024).unwrap()),
        ..StatisticsRequest::DATA
    };
    let cache = CacheMode::enabled(cache::colocated_path(&dump));
    let run = map_file(
        cache.open_source(source).await.unwrap(),
        ReadOptions::default(),
        &arranged(1),
        PostgresInvalidValues::Default,
        &request,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let opened = Dump::mapped(run).await.unwrap();
    let selector = TableSelector::named("ordered").in_schema("public");
    let table = opened.table(&selector).unwrap().unwrap();
    let options = QueryOptions {
        projection: Some(vec!["id".into()]),
        filter: static_filter.unwrap_or_default(),
        use_statistics: false,
        parallelism: Parallelism::workers(1, 1 << 30),
        ..QueryOptions::default()
    };
    let plan = table.plan(ReadOptions::default(), options).unwrap();
    let plan = Arc::new(plan);
    let dynamic = dynamic.map(|state| plan.under(Arc::new(Constant(Arc::new(state))), evaluation));
    let mut rows = 0;
    for partition in 0..plan.len() {
        let mut stream = match &dynamic {
            Some(dynamic) => dynamic.stream(partition, 64),
            None => plan.stream(partition, 64),
        };
        while let Some(batch) = stream.next().await {
            rows += batch.unwrap().num_rows() as u64;
        }
    }
    rows
}

/// **One row span per row the state is evaluated on, and each leaf's parts
/// once per leaf that row reaches**: a join's bounds are implied by its keys,
/// so a row is evaluated against the membership alone
/// (`ResolvedExpr::for_rows`) — one field located and unescaped, none keyed
/// or compared, one looked up. An `integer`'s `=` compares the text the file
/// spells, so its membership keys nothing and probes a set of the literals.
/// **A static filter's
/// evaluation of the same tree is timed nowhere**, the leaf parts being timed
/// only inside a row, and **a replay not evaluating rows times none**, only
/// its reads of the state at each chunk; and the reading's derived times are
/// finite.
#[tokio::test]
async fn the_instrument_times_each_row_the_state_is_evaluated_on_and_its_leaves() {
    let before = counts();
    assert_eq!(read(Some(join_filter()), None, RowEvaluation::On).await, IN_VALUES);
    assert_eq!(counts(), before, "a static filter's evaluation is timed");

    let before = counts();
    assert_eq!(read(None, Some(join_filter()), RowEvaluation::Off).await, ROWS);
    let after = counts();
    for part in EvaluationPart::ALL {
        let spans = after[part as usize] - before[part as usize];
        match part {
            EvaluationPart::Chunk => assert!(spans >= 1, "no chunk read the state"),
            _ => assert_eq!(spans, 0, "{part:?} timed with rows not evaluated"),
        }
    }

    let before = counts();
    assert_eq!(read(None, Some(join_filter()), RowEvaluation::On).await, IN_VALUES);
    let after = counts();
    let spans = |part: EvaluationPart| after[part as usize] - before[part as usize];
    assert_eq!(spans(EvaluationPart::Row), ROWS);
    assert_eq!(spans(EvaluationPart::Locate), ROWS);
    assert_eq!(spans(EvaluationPart::Unescape), ROWS);
    assert_eq!(spans(EvaluationPart::Key), 0);
    assert_eq!(spans(EvaluationPart::Compare), 0);
    assert_eq!(spans(EvaluationPart::Lookup), ROWS);
    assert!(spans(EvaluationPart::Chunk) >= 1, "no chunk read the state");

    let reading = evaluation_reading();
    assert!(reading.nanos_per_tick.is_finite() && reading.nanos_per_tick > 0.0, "{reading:?}");
    assert!(reading.empty_span_ticks > 0.0 && reading.nested_span_ticks > 0.0, "{reading:?}");
    assert!(reading.tree_walk_nanos().is_finite(), "{reading:?}");
    let lines = reading.lines();
    for part in EvaluationPart::ALL {
        assert!(lines.contains(&format!("evaluation_{}_nanos=", part.name())), "{lines}");
    }
    assert!(lines.contains("evaluation_tree_walk_nanos="), "{lines}");
}
