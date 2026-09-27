//! A dynamic filter handed to a partitioned replay
//! (`TablePartitions::under`): the sub-streams are cut over the row groups
//! its state keeps at their first poll, the groups a later state rules out
//! are skipped as the replay reaches them, a block sorted on a bound the state
//! requires stops past it, and a term a block cannot resolve keeps every row
//! where it sits.
//!
//! **That skipping loses no row the filter keeps** is checked with generated
//! filters beside DataFusion's own evaluation of them
//! (`datafusion-pgdump/src/dynamic_filter/tests.rs`). Here, that each thing
//! happens where the `statistics` fixture's shapes say it must: `ordered`'s
//! `id` ascends from 1 to 1000, so at [`SMALL_GROUP`] every group ahead of
//! the one holding `990` has a maximum below it.

use std::collections::BTreeMap;
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
    PredicateOp, QueryOptions, ScanOptions, Sortedness, StatisticsRequest, StatisticsSelection,
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
/// its generation and `state` from then on, counting the times — `state`
/// from the start where `after` is zero, the cut at the first poll included.
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
        let moved = self.after == 0 || self.asked.load(Ordering::Relaxed) > self.after;
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
    let index = mapped(&copy, SMALL_GROUP).await;
    (dir, copy, index)
}

/// The map of `dump`, cached beside it, gathering every statistic at
/// `group_size`.
async fn mapped(dump: &Path, group_size: u64) -> DumpIndex {
    let source = LocalFileSource::open(dump).unwrap();
    let request = StatisticsRequest {
        selection: StatisticsSelection::All,
        group_size: Some(NonZeroU64::new(group_size).unwrap()),
        ..StatisticsRequest::ALL
    };
    let cache = CacheMode::enabled(cache::colocated_path(dump));
    let run = map_file(&source, &ScanOptions::default(), &cache, &request).await.unwrap();
    assert!(!run.interrupted);
    run.index
}

/// What a replay of a table's `id` emitted under `dynamic`: the ids in file
/// order and each sub-stream's, the groups its sub-streams pruned, the bytes
/// its early stops left unread, and the order the plan declared `id` in.
#[derive(Debug)]
struct Run {
    ids: Vec<i32>,
    partitions: Vec<Vec<i32>>,
    pruned: u64,
    unread: u64,
    order: Sortedness,
}

/// [`run_table`] over `ordered`.
async fn run(
    dump: &Path,
    index: &DumpIndex,
    jobs: usize,
    use_statistics: bool,
    dynamic: Option<Arc<dyn DynamicFilter>>,
) -> Run {
    run_table(dump, index, "public.ordered", jobs, use_statistics, dynamic).await
}

async fn run_table(
    dump: &Path,
    index: &DumpIndex,
    table: &str,
    jobs: usize,
    use_statistics: bool,
    dynamic: Option<Arc<dyn DynamicFilter>>,
) -> Run {
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(dump).unwrap());
    let watch =
        Arc::new(SourceWatch::open(source.as_ref(), StrictIdentity::ADVISORY).await.unwrap());
    let table = index.tables().into_iter().find(|t| t.qualified() == table).unwrap();
    let options = QueryOptions {
        projection: Some(vec!["id".into()]),
        use_statistics,
        parallelism: Parallelism::workers(jobs, 1 << 30),
        ..QueryOptions::default()
    };
    let plan = TablePartitions::plan(source, index, watch, &table, ScanOptions::default(), options)
        .await
        .unwrap();
    let plan = Arc::new(plan);
    let dynamic = dynamic.map(|filter| plan.under(filter));
    let (mut partitions, mut pruned, mut unread) = (Vec::new(), 0, 0);
    for partition in 0..plan.len() {
        let mut stream = match &dynamic {
            Some(dynamic) => dynamic.stream(partition, 64),
            None => plan.stream(partition, 64),
        };
        let mut ids = Vec::new();
        while let Some(batch) = stream.next().await {
            let batch: RecordBatch = batch.unwrap();
            ids.extend(batch.column(0).as_primitive::<Int32Type>().iter().map(Option::unwrap));
        }
        partitions.push(ids);
        pruned += stream.dynamic_filter_pruned_groups();
        unread += stream.early_stops().iter().filter_map(|stop| stop.unread_bytes).sum::<u64>();
    }
    let ids = partitions.concat();
    Run { ids, partitions, pruned, unread, order: plan.orders()[0] }
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

/// How many of `ordered`'s groups hold an `id` in `ids`, which its ids being
/// consecutive is every group whose bounds meet the range.
fn groups_meeting(index: &DumpIndex, ids: std::ops::RangeInclusive<i64>) -> u64 {
    let block = index.blocks_for("public.ordered").next().unwrap();
    let statistics = block.statistics.as_deref().unwrap();
    let id = statistics.columns[0].as_ref().unwrap().bounds.as_ref().unwrap();
    let bound = |text: &str| text.parse::<i64>().unwrap();
    let meets =
        |b: &&pgdump_query::Bounds| bound(&b.min) <= *ids.end() && bound(&b.max) >= *ids.start();
    id.groups.iter().flatten().filter(meets).count() as u64
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

        // Each group the cut ruled out is counted once, by one sub-stream.
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
/// reports what it left unread as a static filter's does. The groups past the
/// bound are ruled out by the cut before a row of them is read, serial and
/// split alike, so the stop is what ends the one group left.
#[tokio::test]
async fn a_sorted_block_stops_at_a_bound_the_state_requires() {
    for version in VERSIONS {
        let (_dir, dump, index) = gathered(version).await;
        let (below_bound, groups) = groups_below(&index, 5);
        let past = groups as u64 - below_bound - 1;
        assert!(past > 10, "pg_dump {version}: {past} group(s) past the bound");
        let below = || Moving::constant(term("id", PredicateOp::Lt, "5"));
        let serial = run(&dump, &index, 1, true, Some(below())).await;
        assert_eq!(serial.ids, [1, 2, 3, 4], "pg_dump {version}");
        assert!(serial.unread > 0, "pg_dump {version}: {serial:?}");
        assert_eq!(serial.pruned, past, "pg_dump {version}");

        let split = run(&dump, &index, SPLIT_JOBS, true, Some(below())).await;
        assert_eq!(split.ids, [1, 2, 3, 4], "pg_dump {version}");
        assert!(split.unread > 0, "pg_dump {version}: {split:?}");
        assert_eq!(split.pruned, past, "pg_dump {version}");
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

/// Each row of `table`'s block in `dump` by its first field, an integer: the
/// bytes the row takes, its LF included.
fn row_bytes(dump: &Path, table: &str) -> BTreeMap<i32, u64> {
    let text = std::fs::read_to_string(dump).unwrap();
    let header = format!("COPY {table} (");
    text.lines()
        .skip_while(|line| !line.starts_with(&header))
        .skip(1)
        .take_while(|line| *line != "\\.")
        .map(|line| {
            let id = line.split('\t').next().unwrap().parse().unwrap();
            (id, line.len() as u64 + 1)
        })
        .collect()
}

/// **A selective state on a clustered key is cut balanced over the groups it
/// keeps**: the membership a hash join publishes for a build side holding
/// `ordered`'s ids 400 to 499 keeps a run of groups inside the middle
/// sub-stream's third of the table, and the cut made at the first poll hands
/// each sub-stream a third of that run, where the planned cut would leave the
/// others nothing to read. Each is within a row of its share of the bytes, the
/// rows are the kept groups' in file order, and each group ruled out is
/// counted once.
///
/// The bounds a join publishes beside its membership keep the same groups,
/// and add the stop the sorted-block test above reads; without them no row
/// ends a sub-stream early, so what each emits is what it was handed.
#[tokio::test]
async fn a_selective_state_on_a_clustered_key_is_cut_balanced_over_the_groups_it_keeps() {
    let build_side = || {
        let members = (400..500).map(|id| term("id", PredicateOp::Eq, &id.to_string()));
        Moving::constant(Expr::Or(members.collect()))
    };
    for version in VERSIONS {
        let (_dir, dump, index) = gathered(version).await;
        let (_, groups) = groups_below(&index, 0);
        let kept = groups_meeting(&index, 400..=499);
        assert!(kept >= 3, "pg_dump {version}: {kept} group(s) kept");
        let serial = run(&dump, &index, 1, true, Some(build_side())).await;
        let (first, last) = (serial.ids[0], *serial.ids.last().unwrap());
        assert!(first <= 400 && last >= 499, "pg_dump {version}: {first}..{last}");
        assert_eq!(serial.ids, (first..=last).collect::<Vec<_>>(), "pg_dump {version}");

        let split = run(&dump, &index, SPLIT_JOBS, true, Some(build_side())).await;
        assert_eq!(split.ids, serial.ids, "pg_dump {version}");
        assert_eq!(split.pruned, serial.pruned, "pg_dump {version}");
        assert_eq!(split.pruned + kept, groups as u64, "pg_dump {version}: {split:?}");
        assert_eq!(split.partitions.len(), SPLIT_JOBS, "pg_dump {version}");

        let widths = row_bytes(&dump, "public.ordered");
        let widest = *widths.values().max().unwrap();
        let bytes: Vec<u64> =
            split.partitions.iter().map(|ids| ids.iter().map(|id| widths[id]).sum()).collect();
        let share = bytes.iter().sum::<u64>() / SPLIT_JOBS as u64;
        assert!(
            bytes.iter().all(|&b| b.abs_diff(share) <= 2 * widest),
            "pg_dump {version}: {bytes:?} against {share} apiece"
        );
    }
}

/// `public.t` in two blocks, as a partitioned table loaded through its root
/// arrives: each holds ascending ids, the second's all below the first's, so
/// nothing proves the second's rows follow the first's.
fn out_of_order_blocks(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("out_of_order.sql");
    let rows = |ids: std::ops::RangeInclusive<i32>| -> String {
        ids.map(|id| format!("{id}\tpadding\n")).collect()
    };
    std::fs::write(
        &path,
        format!(
            "CREATE TABLE public.t (\n    id integer,\n    pad text\n);\n\n\
             COPY public.t (id, pad) FROM stdin;\n{}\\.\n\n\
             COPY public.t (id, pad) FROM stdin;\n{}\\.\n",
            rows(101..=200),
            rows(1..=100),
        ),
    )
    .unwrap();
    path
}

/// **A cut that would lose a declared order is not made**: two sub-streams
/// each hold one of two blocks sorted on `id`, which the plan declares, and a
/// state keeping the first block's tail and most of the second would be cut
/// into one sub-stream reading the tail and then the head — the first block's
/// high ids and then the second's low ones. So the planned cut stands, each
/// sub-stream still emitting ascending ids, and the groups the state rules
/// out are skipped as the replay reaches them.
#[tokio::test]
async fn a_cut_that_would_lose_a_declared_order_is_not_made() {
    let dir = tempfile::tempdir().unwrap();
    let dump = out_of_order_blocks(dir.path());
    let index = mapped(&dump, 64).await;
    let tail_and_head = || {
        Moving::constant(Expr::Or(vec![
            term("id", PredicateOp::Ge, "190"),
            term("id", PredicateOp::Le, "90"),
        ]))
    };
    let whole = run_table(&dump, &index, "public.t", 2, true, None).await;
    assert_eq!(whole.order, Sortedness::Ascending, "{whole:?}");
    assert_eq!(whole.partitions.len(), 2);
    assert!(whole.partitions.iter().all(|ids| ids.is_sorted()), "{whole:?}");

    let got = run_table(&dump, &index, "public.t", 2, true, Some(tail_and_head())).await;
    assert!(got.partitions.iter().all(|ids| ids.is_sorted()), "{:?}", got.partitions);
    assert!(got.pruned > 0, "{got:?}");
    let expected: Vec<i32> =
        whole.ids.iter().copied().filter(|&id| id >= 190 || id <= 90).collect();
    let kept: Vec<i32> = got.ids.iter().copied().filter(|&id| id >= 190 || id <= 90).collect();
    assert_eq!(kept, expected);
}

/// A dynamic filter breaking its contract: its state is `first` the first
/// time it is read and `then` from then on, its generation never moving — so
/// what a replay emits says which reading it pruned by.
struct Unmoving {
    first: Arc<Expr>,
    then: Arc<Expr>,
    reads: AtomicU64,
}

impl DynamicFilter for Unmoving {
    fn generation(&self) -> u64 {
        0
    }

    fn current(&self) -> (u64, Arc<Expr>) {
        let first = self.reads.fetch_add(1, Ordering::Relaxed) == 0;
        (0, Arc::clone(if first { &self.first } else { &self.then }))
    }
}

/// **A sub-stream reading the generation the cut read takes the cut's
/// verdicts** rather than asking its groups again, a generation naming one
/// state. The cut reads a state keeping everything; each sub-stream then
/// reads one ruling most groups out under the same generation, and prunes
/// nothing by it.
#[tokio::test]
async fn a_replay_takes_the_cut_s_verdicts_under_the_generation_it_read() {
    let (_dir, dump, index) = gathered(18).await;
    let whole = run(&dump, &index, 1, true, None).await;
    let filter = Arc::new(Unmoving {
        first: Arc::new(Expr::default()),
        then: Arc::new(term("id", PredicateOp::Ge, "990")),
        reads: AtomicU64::new(0),
    });
    let got = run(&dump, &index, SPLIT_JOBS, true, Some(filter)).await;
    assert_eq!(got.ids, whole.ids);
    assert_eq!(got.pruned, 0);
}
