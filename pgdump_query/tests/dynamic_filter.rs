//! A dynamic filter handed to a partitioned replay
//! (`TablePartitions::under`): the sub-streams are cut over the row groups
//! its state keeps at their first poll, the groups a later state rules out
//! are skipped as the replay reaches them, the state is read again at each
//! chunk the replay takes, a row the state rejects is dropped before it
//! decodes in every block, a block sorted on a bound the state requires stops
//! past it, and a term a block cannot resolve keeps every row where it sits.
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
    ByteRangeSource, DumpIndex, DynamicFilter, Expr, LocalFileSource, Membership, Parallelism,
    Predicate, PredicateOp, QueryOptions, ScanOptions, Sortedness, StatisticsRequest,
    StatisticsSelection, TablePartitions, map_file,
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
/// order and each sub-stream's, the groups its sub-streams pruned, the rows
/// they dropped, the bytes its early stops left unread, and the order the
/// plan declared `id` in.
#[derive(Debug)]
struct Run {
    ids: Vec<i32>,
    partitions: Vec<Vec<i32>>,
    pruned: u64,
    dropped: u64,
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
    let scan = ScanOptions::default();
    run_scanned(dump, index, table, jobs, use_statistics, scan, dynamic).await
}

/// [`run_table`], reading in `scan`'s chunks.
async fn run_scanned(
    dump: &Path,
    index: &DumpIndex,
    table: &str,
    jobs: usize,
    use_statistics: bool,
    scan: ScanOptions,
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
    let plan = TablePartitions::plan(source, index, watch, &table, scan, options).await.unwrap();
    let plan = Arc::new(plan);
    let dynamic = dynamic.map(|filter| plan.under(filter));
    let (mut partitions, mut pruned, mut dropped, mut unread) = (Vec::new(), 0, 0, 0);
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
        dropped += stream.dynamic_filter_pruned_rows();
        unread += stream.early_stops().iter().filter_map(|stop| stop.unread_bytes).sum::<u64>();
    }
    let ids = partitions.concat();
    Run { ids, partitions, pruned, dropped, unread, order: plan.orders()[0] }
}

/// The number of `ordered`'s groups whose `id` is below `bound` throughout —
/// every group before the one holding it — and the groups its block lists.
fn groups_below(index: &DumpIndex, bound: i64) -> (u64, usize) {
    let (below, _) = below(index, bound);
    let block = index.blocks_for("public.ordered").next().unwrap();
    (below, block.statistics.as_deref().unwrap().groups.len())
}

/// The number of `ordered`'s groups whose `id` is below `bound` throughout,
/// and the rows they hold.
fn below(index: &DumpIndex, bound: i64) -> (u64, u64) {
    let block = index.blocks_for("public.ordered").next().unwrap();
    let statistics = block.statistics.as_deref().unwrap();
    let id = statistics.columns[0].as_ref().unwrap().bounds.as_ref().unwrap();
    let below: Vec<usize> = (0..statistics.groups.len())
        .filter(|&g| id.groups[g].as_ref().unwrap().max.parse::<i64>().unwrap() < bound)
        .collect();
    let rows = below.iter().map(|&g| statistics.groups[g].rows).sum();
    (below.len() as u64, rows)
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

/// **A dynamic filter's state skips every group it rules out, and drops each
/// row of the rest it rejects**: under `id >= 990` the replay reads exactly
/// the groups at and past the one holding `990`, emits exactly the rows the
/// state keeps, serially and split alike, and counts each group it skipped
/// and each row it dropped once. With statistics off it skips no group and
/// drops every row the state rejects; under a state that rules nothing out it
/// skips and drops nothing.
#[tokio::test]
async fn a_dynamic_filter_skips_every_group_its_state_rules_out() {
    for version in VERSIONS {
        let (_dir, dump, index) = gathered(version).await;
        let (below, _) = groups_below(&index, 990);
        assert!(below > 10, "pg_dump {version}: {below} group(s) below the bound");
        let whole = run(&dump, &index, 1, true, None).await;
        assert_eq!(whole.ids, (1..=1000).collect::<Vec<_>>(), "pg_dump {version}");

        let (_, skipped_rows) = self::below(&index, 990);
        let at_least = || Moving::constant(term("id", PredicateOp::Ge, "990"));
        let serial = run(&dump, &index, 1, true, Some(at_least())).await;
        assert_eq!(serial.ids, (990..=1000).collect::<Vec<_>>(), "pg_dump {version}");
        assert_eq!(serial.pruned, below, "pg_dump {version}");
        // The kept groups' rows below the bound, read and dropped.
        assert!(skipped_rows < 989, "pg_dump {version}: every row below 990 skipped");
        assert_eq!(serial.dropped, 989 - skipped_rows, "pg_dump {version}");
        assert_eq!(serial.unread, 0, "pg_dump {version}: no bound its order closes");

        // Each group the cut ruled out is counted once, by one sub-stream,
        // and each row dropped by the sub-stream that read it.
        let split = run(&dump, &index, SPLIT_JOBS, true, Some(at_least())).await;
        assert_eq!(split.ids, serial.ids, "pg_dump {version}");
        assert_eq!(split.pruned, below, "pg_dump {version}: {split:?}");
        assert_eq!(split.dropped, serial.dropped, "pg_dump {version}: {split:?}");

        for jobs in [1, SPLIT_JOBS] {
            let blind = run(&dump, &index, jobs, false, Some(at_least())).await;
            assert_eq!(blind.ids, serial.ids, "pg_dump {version}: statistics off, {jobs} job(s)");
            let counts = (blind.pruned, blind.dropped, blind.unread);
            assert_eq!(counts, (0, 989, 0), "pg_dump {version}: statistics off, {jobs} job(s)");
        }

        let everything = Moving::constant(Expr::default());
        let unfiltered = run(&dump, &index, SPLIT_JOBS, true, Some(everything)).await;
        assert_eq!(unfiltered.ids, whole.ids, "pg_dump {version}");
        let counts = (unfiltered.pruned, unfiltered.dropped, unfiltered.unread);
        assert_eq!(counts, (0, 0, 0), "pg_dump {version}: a state keeping every row");
    }
}

/// **A state that moves mid-block is read as the replay enters the next
/// group**: the groups read before it moved are emitted whole, the ones after
/// it rules out are skipped, and the filter is asked once per group entered
/// and chunk taken, not per row. `ordered` is one chunk, so the move is asked
/// for at the fourth group: the first chunk's read and three groups' entries
/// come before it.
#[tokio::test]
async fn a_state_moving_mid_block_prunes_from_the_next_group_it_enters() {
    for version in VERSIONS {
        let (_dir, dump, index) = gathered(version).await;
        let (below, groups) = groups_below(&index, 990);
        let at_least = || term("id", PredicateOp::Ge, "990");
        let serial = run(&dump, &index, 1, true, Some(Moving::constant(at_least()))).await;
        let moving = Moving::new(Expr::default(), at_least(), 4);
        let moved = run(&dump, &index, 1, true, Some(Arc::clone(&moving) as _)).await;
        // The three groups entered before the move are read whole, and from
        // the fourth on the replay reads what the moved state keeps.
        let jump = moved.ids.windows(2).position(|pair| pair[1] != pair[0] + 1).unwrap() + 1;
        let (head, tail) = moved.ids.split_at(jump);
        assert!(head.len() > 3 && head.iter().copied().eq(1..=head.len() as i32), "{head:?}");
        assert_eq!(tail, serial.ids, "pg_dump {version}");
        assert_eq!(moved.pruned, below - 3, "pg_dump {version}");
        let asked = moving.asked.load(Ordering::Relaxed);
        assert!(asked > 4 && asked <= groups as u64 + 1, "pg_dump {version}: asked {asked} times");
    }
}

/// `public.t`'s `id` from 1 to [`LONG_ROWS`], each row padded so that a
/// small read chunk holds a few of them.
fn long_block(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("long.sql");
    let rows: String = (1..=LONG_ROWS).map(|id| format!("{id}\t{:>40}\n", "pad")).collect();
    std::fs::write(
        &path,
        format!(
            "CREATE TABLE public.t (\n    id integer,\n    pad text\n);\n\n\
             COPY public.t (id, pad) FROM stdin;\n{rows}\\.\n"
        ),
    )
    .unwrap();
    path
}

/// The rows of [`long_block`].
const LONG_ROWS: i32 = 2000;

/// A read chunk holding a handful of [`long_block`]'s rows.
const SMALL_CHUNK: usize = 512;

/// `run`'s ids, split where they stop ascending by one: the rows read before
/// a state moved, and those after.
fn at_the_move(run: &Run) -> (&[i32], &[i32]) {
    let jump = run.ids.windows(2).position(|pair| pair[1] != pair[0] + 1).map_or(0, |at| at + 1);
    run.ids.split_at(jump)
}

/// **A state is read again at each chunk the replay takes, in every block**
/// (`docs/design/decisions.md`, "D93"): over one row group, which a replay
/// enters once, a state moving to `id > 1500` after a few reads drops every
/// row it rejects from the next chunk on — statistics or not, and serially
/// and split alike, each sub-stream from its own next chunk. Read only at
/// the group's entry, the move would never be seen.
#[tokio::test]
async fn a_state_moving_inside_a_group_is_read_at_the_next_chunk() {
    let dir = tempfile::tempdir().unwrap();
    let dump = long_block(dir.path());
    let index = mapped(&dump, 1 << 20).await;
    let block = index.blocks_for("public.t").next().unwrap();
    assert_eq!(block.statistics.as_deref().unwrap().groups.len(), 1);
    let scan = ScanOptions { chunk_size_bytes: SMALL_CHUNK, ..ScanOptions::default() };
    let past = || term("id", PredicateOp::Gt, "1500");

    for use_statistics in [true, false] {
        let moving = Moving::new(Expr::default(), past(), 4);
        let got =
            run_scanned(&dump, &index, "public.t", 1, use_statistics, scan.clone(), Some(moving))
                .await;
        let (head, tail) = at_the_move(&got);
        let read = head.len() as i32;
        assert!(read > 0 && read < 1500, "statistics {use_statistics}: {read} row(s) before");
        assert!(head.iter().copied().eq(1..=read), "statistics {use_statistics}");
        assert!(tail.iter().copied().eq(1501..=LONG_ROWS), "statistics {use_statistics}");
        let counts = (got.pruned, got.dropped);
        assert_eq!(counts, (0, (1500 - read) as u64), "statistics {use_statistics}");

        let moving = Moving::new(Expr::default(), past(), 4);
        let split = run_scanned(
            &dump,
            &index,
            "public.t",
            SPLIT_JOBS,
            use_statistics,
            scan.clone(),
            Some(moving),
        )
        .await;
        assert!(split.partitions.len() > 1, "statistics {use_statistics}: {split:?}");
        let kept: Vec<i32> = split.ids.iter().copied().filter(|&id| id > 1500).collect();
        assert!(kept.iter().copied().eq(1501..=LONG_ROWS), "statistics {use_statistics}");
        assert!(split.ids.len() < LONG_ROWS as usize, "statistics {use_statistics}: none dropped");
        assert_eq!(split.ids.len() as u64 + split.dropped, LONG_ROWS as u64);
    }
}

/// **A state read inside a group acts as one read at its entry does**
/// (`docs/design/decisions.md`, "D93"): moving to one ruling the group out,
/// the rest of it is skipped rather than read and dropped a row at a time,
/// and the group, part of it read, is not counted as pruned. Without
/// statistics nothing rules the group out, so the same move drops each row
/// after it.
#[tokio::test]
async fn a_state_read_inside_a_group_it_rules_out_skips_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let dump = long_block(dir.path());
    let index = mapped(&dump, 1 << 20).await;
    let scan = ScanOptions { chunk_size_bytes: SMALL_CHUNK, ..ScanOptions::default() };
    let beyond = || term("id", PredicateOp::Gt, "5000");

    let moving = Moving::new(Expr::default(), beyond(), 4);
    let got = run_scanned(&dump, &index, "public.t", 1, true, scan.clone(), Some(moving)).await;
    let read = got.ids.len() as i32;
    assert!(read > 0 && read < LONG_ROWS, "{read} row(s) before");
    assert!(got.ids.iter().copied().eq(1..=read));
    assert_eq!((got.pruned, got.dropped), (0, 0), "{got:?}");

    let moving = Moving::new(Expr::default(), beyond(), 4);
    let blind = run_scanned(&dump, &index, "public.t", 1, false, scan, Some(moving)).await;
    let read = blind.ids.len() as i32;
    assert!(read > 0 && read < LONG_ROWS, "{read} row(s) before");
    assert!(blind.ids.iter().copied().eq(1..=read));
    assert_eq!((blind.pruned, blind.dropped), (0, (LONG_ROWS - read) as u64), "{blind:?}");
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
        among("nowhere", &["5"]),
        Expr::Not(Box::new(among("id", &["1", "five"]))),
    ] {
        let got = run(&dump, &index, 1, true, Some(Moving::constant(state.clone()))).await;
        assert_eq!(got.ids, whole.ids, "{state:?}");
        assert_eq!((got.pruned, got.dropped, got.unread), (0, 0, 0), "{state:?}");
    }
    let beside = Expr::And(vec![lacking(), term("id", PredicateOp::Lt, "5")]);
    let got = run(&dump, &index, 1, true, Some(Moving::constant(beside))).await;
    assert_eq!(got.ids, [1, 2, 3, 4]);
}

/// `column IN (values)`, no NULL among them.
fn among(column: &str, values: &[&str]) -> Expr {
    Expr::In(Membership {
        column: column.into(),
        values: values.iter().map(|v| Some(v.to_string())).collect(),
    })
}

/// **A state holding a membership is read as its `Or` of `=` is**: the same
/// rows emitted, the same groups skipped and the same rows dropped before
/// they decode, serial and split alike.
#[tokio::test]
async fn a_membership_in_the_state_is_read_as_its_disjunction_is() {
    let (_dir, dump, index) = gathered(18).await;
    let values = ["3", "700", "990", "5000"];
    let disjunction = || Expr::Or(values.iter().map(|v| term("id", PredicateOp::Eq, v)).collect());
    for jobs in [1, SPLIT_JOBS] {
        let want = run(&dump, &index, jobs, true, Some(Moving::constant(disjunction()))).await;
        let got =
            run(&dump, &index, jobs, true, Some(Moving::constant(among("id", &values)))).await;
        assert_eq!(got.ids, [3, 700, 990], "{jobs} job(s)");
        assert_eq!(
            (got.ids, got.pruned, got.dropped, got.unread),
            (want.ids, want.pruned, want.dropped, want.unread),
            "{jobs} job(s)"
        );
    }
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

/// A dynamic filter whose state is `at_cut` when the cut reads it and keeps
/// every row after, under a generation of its own — so a replay reads each
/// group the cut handed it whole, and what a sub-stream emits is what it was
/// handed rather than the rows of it the cut's state keeps.
struct CutOnly {
    at_cut: Arc<Expr>,
    reads: AtomicU64,
}

impl CutOnly {
    fn new(at_cut: Expr) -> Arc<Self> {
        Arc::new(Self { at_cut: Arc::new(at_cut), reads: AtomicU64::new(0) })
    }
}

impl DynamicFilter for CutOnly {
    fn generation(&self) -> u64 {
        1
    }

    fn current(&self) -> (u64, Arc<Expr>) {
        match self.reads.fetch_add(1, Ordering::Relaxed) {
            0 => (0, Arc::clone(&self.at_cut)),
            _ => (1, Arc::new(Expr::default())),
        }
    }
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
/// The state keeps every row once the cut is made ([`CutOnly`]), so what each
/// sub-stream emits is what it was handed: the membership's own state would
/// drop the kept groups' rows outside it, and a join's bounds would add the
/// stop the sorted-block test above reads.
#[tokio::test]
async fn a_selective_state_on_a_clustered_key_is_cut_balanced_over_the_groups_it_keeps() {
    let build_side = || {
        let members = (400..500).map(|id| term("id", PredicateOp::Eq, &id.to_string()));
        CutOnly::new(Expr::Or(members.collect()))
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
/// no group by it — reading every row, and dropping each the state it read
/// rejects.
#[tokio::test]
async fn a_replay_takes_the_cut_s_verdicts_under_the_generation_it_read() {
    let (_dir, dump, index) = gathered(18).await;
    let filter = Arc::new(Unmoving {
        first: Arc::new(Expr::default()),
        then: Arc::new(term("id", PredicateOp::Ge, "990")),
        reads: AtomicU64::new(0),
    });
    let got = run(&dump, &index, SPLIT_JOBS, true, Some(filter)).await;
    assert_eq!(got.ids, (990..=1000).collect::<Vec<_>>());
    assert_eq!((got.pruned, got.dropped), (0, 989));
}

/// `public.t`'s `id` from 1 to 200 beside an integer `v` that is its `id`,
/// but text no `integer` decoder reads in the first fifty rows.
fn undecodable_head(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("undecodable.sql");
    let rows: String = (1..=200)
        .map(|id| if id <= 50 { format!("{id}\tnope\n") } else { format!("{id}\t{id}\n") })
        .collect();
    std::fs::write(
        &path,
        format!(
            "CREATE TABLE public.t (\n    id integer,\n    v integer\n);\n\n\
             COPY public.t (id, v) FROM stdin;\n{rows}\\.\n"
        ),
    )
    .unwrap();
    path
}

/// The ids a serial replay of `public.t`, projecting `id` and `v`, emitted
/// under `dynamic` and the rows it dropped — or the error it ended on.
async fn read_both(
    dump: &Path,
    index: &DumpIndex,
    dynamic: Option<Arc<dyn DynamicFilter>>,
) -> Result<(Vec<i32>, u64), String> {
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(dump).unwrap());
    let watch =
        Arc::new(SourceWatch::open(source.as_ref(), StrictIdentity::ADVISORY).await.unwrap());
    let table = index.tables().into_iter().find(|t| t.qualified() == "public.t").unwrap();
    let options = QueryOptions {
        projection: Some(vec!["id".into(), "v".into()]),
        parallelism: Parallelism::workers(1, 1 << 30),
        ..QueryOptions::default()
    };
    let plan = TablePartitions::plan(source, index, watch, &table, ScanOptions::default(), options)
        .await
        .unwrap();
    let plan = Arc::new(plan);
    let mut stream = match dynamic {
        Some(filter) => plan.under(filter).stream(0, 64),
        None => plan.stream(0, 64),
    };
    let mut ids = Vec::new();
    while let Some(batch) = stream.next().await {
        let batch: RecordBatch = batch.map_err(|e| e.to_string())?;
        ids.extend(batch.column(0).as_primitive::<Int32Type>().iter().map(Option::unwrap));
    }
    Ok((ids, stream.dynamic_filter_pruned_rows()))
}

/// **A row the state rejects is dropped before a column of it decodes**, in
/// a group the state keeps: under `id > 50`, over one group holding every
/// row, the fifty rows whose `v` no decoder reads are dropped and counted,
/// where read without the filter the first of them is the replay's error.
/// **A field of the state's own that does not decode keeps its row**, so
/// under `v > 100` the error is the one the unfiltered replay raises.
#[tokio::test]
async fn a_row_the_state_rejects_is_dropped_before_it_decodes() {
    let dir = tempfile::tempdir().unwrap();
    let dump = undecodable_head(dir.path());
    let index = mapped(&dump, 1 << 20).await;
    let block = index.blocks_for("public.t").next().unwrap();
    assert_eq!(block.statistics.as_deref().unwrap().groups.len(), 1);

    let unfiltered = read_both(&dump, &index, None).await;
    let refused = unfiltered.expect_err("the head's `v` decodes");

    let past_head = Moving::constant(term("id", PredicateOp::Gt, "50"));
    let (ids, dropped) = read_both(&dump, &index, Some(past_head)).await.unwrap();
    assert_eq!(ids, (51..=200).collect::<Vec<_>>());
    assert_eq!(dropped, 50);

    let on_v = Moving::constant(term("v", PredicateOp::Gt, "100"));
    assert_eq!(read_both(&dump, &index, Some(on_v)).await, Err(refused));
}
