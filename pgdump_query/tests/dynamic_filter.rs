//! A dynamic filter handed to a partitioned replay
//! (`TablePartitions::stream`): the row groups its state rules out are
//! skipped as the replay reaches them, a block sorted on a bound the state
//! requires stops past it, and a term a block cannot resolve keeps every row
//! where it sits.
//!
//! **That skipping loses no row the filter keeps** is checked with generated
//! filters beside DataFusion's own evaluation of them
//! (`datafusion-pgdump/src/dynamic_filter/tests.rs`). Here, that each thing
//! happens where the `statistics` fixture's shapes say it must: `ordered`'s
//! `id` ascends from 1 to 1000, so at [`SMALL_GROUP`] every group ahead of
//! the one holding `990` has a maximum below it.

use std::num::NonZeroU64;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arrow::array::{AsArray, RecordBatch};
use arrow::datatypes::Int32Type;
use futures::StreamExt;
use pgdump_query::cache::{self, CacheMode, SourceWatch, StrictIdentity};
use pgdump_query::{
    ByteRangeSource, DumpIndex, DynamicFilter, Expr, LocalFileSource, Parallelism, Predicate,
    PredicateOp, QueryOptions, ScanOptions, StatisticsRequest, StatisticsSelection,
    TablePartitions, map_file,
};

mod common;
use common::{VERSIONS, sandboxed, statistics_fixture};

/// A group size several `ordered` rows long, so its thousand ascending ids
/// span many groups.
const SMALL_GROUP: u64 = 1024;

/// The sub-streams the split legs ask for.
const SPLIT_JOBS: usize = 3;

/// A dynamic filter that is `before` for the first `after` times it is asked
/// its generation and `state` from then on, counting the times.
struct Moving {
    before: Arc<Expr>,
    state: Arc<Expr>,
    after: u64,
    asked: AtomicU64,
}

impl Moving {
    fn new(before: Expr, state: Expr, after: u64) -> Arc<Self> {
        Arc::new(Self {
            before: Arc::new(before),
            state: Arc::new(state),
            after,
            asked: AtomicU64::new(0),
        })
    }

    /// `state` from the start.
    fn constant(state: Expr) -> Arc<Self> {
        Self::new(Expr::default(), state, 0)
    }
}

impl DynamicFilter for Moving {
    fn generation(&self) -> u64 {
        u64::from(self.asked.fetch_add(1, Ordering::Relaxed) >= self.after)
    }

    fn current(&self) -> (u64, Arc<Expr>) {
        let moved = self.asked.load(Ordering::Relaxed) > self.after;
        let state = if moved { &self.state } else { &self.before };
        (u64::from(moved), Arc::clone(state))
    }
}

fn term(column: &str, op: PredicateOp, value: &str) -> Expr {
    Expr::Term(Predicate { column: column.into(), op, value: Some(value.into()) })
}

/// The `statistics` fixture of `version` beside a cache gathering every
/// statistic at [`SMALL_GROUP`], and the map it holds.
async fn gathered(version: u32) -> (tempfile::TempDir, std::path::PathBuf, DumpIndex) {
    let (dir, copy) = sandboxed(&statistics_fixture(version, "default"), "dump.sql");
    let source = LocalFileSource::open(&copy).unwrap();
    let request = StatisticsRequest {
        selection: StatisticsSelection::All,
        group_size: Some(NonZeroU64::new(SMALL_GROUP).unwrap()),
        ..StatisticsRequest::ALL
    };
    let cache = CacheMode::enabled(cache::colocated_path(&copy));
    let run = map_file(&source, &ScanOptions::default(), &cache, &request).await.unwrap();
    assert!(!run.interrupted);
    (dir, copy, run.index)
}

/// What a replay of `ordered`'s `id` emitted under `dynamic`: the ids in file
/// order, the groups its sub-streams pruned, and the bytes its early stops
/// left unread.
#[derive(Debug)]
struct Run {
    ids: Vec<i32>,
    pruned: u64,
    unread: u64,
}

async fn run(
    dump: &Path,
    index: &DumpIndex,
    jobs: usize,
    use_statistics: bool,
    dynamic: Option<Arc<dyn DynamicFilter>>,
) -> Run {
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(dump).unwrap());
    let watch =
        Arc::new(SourceWatch::open(source.as_ref(), StrictIdentity::ADVISORY).await.unwrap());
    let table = index.tables().into_iter().find(|t| t.qualified() == "public.ordered").unwrap();
    let options = QueryOptions {
        projection: Some(vec!["id".into()]),
        use_statistics,
        parallelism: Parallelism::workers(jobs, 1 << 30),
        ..QueryOptions::default()
    };
    let plan = TablePartitions::plan(source, index, watch, &table, ScanOptions::default(), options)
        .await
        .unwrap();
    let (mut ids, mut pruned, mut unread) = (Vec::new(), 0, 0);
    for partition in 0..plan.len() {
        let mut stream = plan.stream(partition, 64, dynamic.clone());
        while let Some(batch) = stream.next().await {
            let batch: RecordBatch = batch.unwrap();
            ids.extend(batch.column(0).as_primitive::<Int32Type>().iter().map(Option::unwrap));
        }
        pruned += stream.dynamic_filter_pruned_groups();
        unread += stream.early_stops().iter().filter_map(|stop| stop.unread_bytes).sum::<u64>();
    }
    Run { ids, pruned, unread }
}

/// `ordered`'s block, and the number of its groups whose `id` is at most
/// `below`'s maximum — every group before the one holding `990`.
fn groups_below(index: &DumpIndex, bound: i64) -> (u64, usize) {
    let block = index.blocks_for("public.ordered").next().unwrap();
    let statistics = block.statistics.as_deref().unwrap();
    let id = statistics.columns[0].as_ref().unwrap().bounds.as_ref().unwrap();
    let below =
        id.groups.iter().flatten().filter(|b| b.max.parse::<i64>().unwrap() < bound).count();
    (below as u64, statistics.groups.len())
}

/// **A dynamic filter's state skips every group it rules out, and only
/// those**: under `id >= 990` the replay emits exactly the rows of the groups
/// at and past the one holding `990`, serially and split alike, and counts
/// each group it skipped once; with statistics off, or a state that rules
/// nothing out, it skips nothing.
#[tokio::test]
async fn a_dynamic_filter_skips_every_group_its_state_rules_out() {
    for version in VERSIONS {
        let (_dir, dump, index) = gathered(version).await;
        let (below, _) = groups_below(&index, 990);
        assert!(below > 10, "pg_dump {version}: {below} group(s) below the bound");
        let whole = run(&dump, &index, 1, true, None).await;
        assert_eq!(whole.ids, (1..=1000).collect::<Vec<_>>(), "pg_dump {version}");

        let at_least = || Moving::constant(term("id", PredicateOp::Ge, "990"));
        let serial = run(&dump, &index, 1, true, Some(at_least())).await;
        let first_kept = serial.ids[0];
        assert!(first_kept <= 990 && first_kept > 1, "pg_dump {version}: {:?}", serial.ids);
        assert_eq!(serial.ids, (first_kept..=1000).collect::<Vec<_>>(), "pg_dump {version}");
        assert_eq!(serial.pruned, below, "pg_dump {version}");
        assert_eq!(serial.unread, 0, "pg_dump {version}: no bound its order closes");

        // Each skipped group is counted by the sub-stream holding its start.
        let split = run(&dump, &index, SPLIT_JOBS, true, Some(at_least())).await;
        assert_eq!(split.ids, serial.ids, "pg_dump {version}");
        assert_eq!(split.pruned, below, "pg_dump {version}: {split:?}");

        for (what, unfiltered) in [
            ("statistics off", run(&dump, &index, 1, false, Some(at_least())).await),
            ("a state keeping every row", {
                let everything = Moving::constant(Expr::default());
                run(&dump, &index, SPLIT_JOBS, true, Some(everything)).await
            }),
        ] {
            assert_eq!(unfiltered.ids, whole.ids, "pg_dump {version}: {what}");
            assert_eq!((unfiltered.pruned, unfiltered.unread), (0, 0), "pg_dump {version}: {what}");
        }
    }
}

/// **A state that moves mid-block is read as the replay enters the next
/// group, and only then**: the groups read before it moved are emitted whole,
/// the ones after it rules out are skipped, and the filter is asked once per
/// group entered, not per row.
#[tokio::test]
async fn a_state_moving_mid_block_prunes_from_the_next_group_it_enters() {
    for version in VERSIONS {
        let (_dir, dump, index) = gathered(version).await;
        let (below, groups) = groups_below(&index, 990);
        let at_least = || term("id", PredicateOp::Ge, "990");
        let serial = run(&dump, &index, 1, true, Some(Moving::constant(at_least()))).await;
        let moving = Moving::new(Expr::default(), at_least(), 3);
        let moved = run(&dump, &index, 1, true, Some(Arc::clone(&moving) as _)).await;
        // The three groups entered before the move are read whole, and from
        // the fourth on the replay reads what the moved state keeps.
        let jump = moved.ids.windows(2).position(|pair| pair[1] != pair[0] + 1).unwrap() + 1;
        let (head, tail) = moved.ids.split_at(jump);
        assert!(head.len() > 3 && head.iter().copied().eq(1..=head.len() as i32), "{head:?}");
        assert_eq!(tail, serial.ids, "pg_dump {version}");
        assert_eq!(moved.pruned, below - 3, "pg_dump {version}");
        let asked = moving.asked.load(Ordering::Relaxed);
        assert!(asked > 3 && asked <= groups as u64, "pg_dump {version}: asked {asked} times");
    }
}

/// **A block sorted on a bound the state requires stops at its first row past
/// it**: under `id < 5` the replay emits the four ids below it, and the stop
/// reports what it left unread as a static filter's does; split, the later
/// sub-streams' groups are ruled out before a row of them is read.
#[tokio::test]
async fn a_sorted_block_stops_at_a_bound_the_state_requires() {
    for version in VERSIONS {
        let (_dir, dump, index) = gathered(version).await;
        let below = || Moving::constant(term("id", PredicateOp::Lt, "5"));
        let serial = run(&dump, &index, 1, true, Some(below())).await;
        assert_eq!(serial.ids, [1, 2, 3, 4], "pg_dump {version}");
        assert!(serial.unread > 0, "pg_dump {version}: {serial:?}");
        assert_eq!(serial.pruned, 0, "pg_dump {version}: stopped before the next group");

        let split = run(&dump, &index, SPLIT_JOBS, true, Some(below())).await;
        assert_eq!(split.ids, [1, 2, 3, 4], "pg_dump {version}");
        assert!(split.unread > 0 && split.pruned > 0, "pg_dump {version}: {split:?}");
    }
}

/// **A term no block can resolve keeps every row where it sits**, never
/// refusing the query: a column the table lacks, or a literal no value of the
/// column's type is, is true where it is not negated and false where it is,
/// so neither rules a group out — while beside a term that resolves, that
/// term still stops the block.
#[tokio::test]
async fn a_term_no_block_resolves_keeps_every_row_where_it_sits() {
    let (_dir, dump, index) = gathered(18).await;
    let whole = run(&dump, &index, 1, true, None).await;
    let lacking = || term("nowhere", PredicateOp::Lt, "5");
    let undecodable = || term("id", PredicateOp::Eq, "five");
    for state in [
        lacking(),
        Expr::Not(Box::new(lacking())),
        Expr::Not(Box::new(undecodable())),
        Expr::Not(Box::new(Expr::Not(Box::new(undecodable())))),
        Expr::Or(vec![undecodable(), term("id", PredicateOp::Lt, "5")]),
    ] {
        let got = run(&dump, &index, 1, true, Some(Moving::constant(state.clone()))).await;
        assert_eq!(got.ids, whole.ids, "{state:?}");
        assert_eq!((got.pruned, got.unread), (0, 0), "{state:?}");
    }
    let beside = Expr::And(vec![lacking(), term("id", PredicateOp::Lt, "5")]);
    let got = run(&dump, &index, 1, true, Some(Moving::constant(beside))).await;
    assert_eq!(got.ids, [1, 2, 3, 4]);
}
