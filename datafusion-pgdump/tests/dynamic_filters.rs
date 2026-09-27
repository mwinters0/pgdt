//! DataFusion's dynamic filters, flags on against flags off, over the
//! statistics fixtures (`docs/design/roadmap-P27-dynamic-filters.md`,
//! "Evidence").
//!
//! **No row may be lost, and the flags are the oracle.** A hash join's build
//! side, a TopK's heap and an ungrouped `MIN`/`MAX` each publish a filter
//! into the scan below them, and every producer re-checks its own rows, so a
//! query answers the same with the three per-producer flags
//! (`optimizer.enable_{join,topk,aggregate}_dynamic_filter_pushdown`) on as
//! off. A scan that consumes a filter and drops a row the producer would
//! have kept is a different answer, and nothing else would say so: the join
//! simply never sees the row.
//!
//! **Each query is also held to the shape it is here for**, so the harness
//! cannot pass by exercising nothing. The scan holds whatever filter reaches
//! it, answers `No` for it, visits it in `apply_expressions` — which is what
//! makes a join compute its filter at all — and prints it in `EXPLAIN`, where
//! each filter's final state is read once the flags-on query has run. **And
//! the sweep is held to having consumed them**: the scan skips the row groups
//! a filter's state rules out and stops a sorted block at a bound it requires,
//! which its metrics count, so a sweep in which no filter pruned or stopped
//! anything fails.
//!
//! Every query selects its sort keys alone where it has a `LIMIT`, since
//! ties at the cut are the TopK's to break however it likes.

use std::collections::BTreeSet;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::RecordBatch;
use arrow::util::display::{ArrayFormatter, FormatOptions};
use datafusion::catalog::TableProvider;
use datafusion::common::tree_node::TreeNodeRecursion;
use datafusion::config::ConfigOptions;
use datafusion::execution::session_state::SessionStateBuilder;
use datafusion::physical_expr::PhysicalExpr;
use datafusion::physical_plan::execution_plan::reset_plan_states;
use datafusion::physical_plan::metrics::MetricValue;
use datafusion::physical_plan::{ExecutionPlan, ExecutionPlanProperties, collect, displayable};
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion_pgdump::{PgDump, PgDumpOptions, register_dump};
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    Finding, LocalFileSource, ScanOptions, StatisticsRequest, StatisticsSelection, map_file,
};

mod in_order;
use in_order::{Order, RunScansInOrder};

/// A sink for a registration whose findings this target is not about.
fn ignore(_: &dyn Finding) {}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

/// Every flag set of every major's `statistics` schema: the one holding
/// sorted, unsorted and gappy columns, an enum, special floats, and a table
/// that is three blocks under `--load-via-partition-root`.
fn statistics_fixtures() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for major in std::fs::read_dir(fixtures_root()).unwrap() {
        let schema = major.unwrap().path().join("statistics");
        let Ok(dumps) = std::fs::read_dir(&schema) else { continue };
        for dump in dumps {
            let dump = dump.unwrap().path();
            if dump.extension().is_some_and(|e| e == "sql") {
                found.push(dump);
            }
        }
    }
    found.sort();
    assert!(!found.is_empty(), "no statistics fixtures under {}", fixtures_root().display());
    found
}

/// A group size several rows of every fixture table long, so a scan that
/// prunes has groups to keep and groups to skip.
const SMALL_GROUP: u64 = 1024;

/// `fixture` copied into `dir` beside a complete cache holding every
/// statistic at [`SMALL_GROUP`], so the committed tree is never written into.
async fn parsed_copy(fixture: &Path, dir: &Path) -> PathBuf {
    let dir = tempfile::tempdir_in(dir).unwrap().keep();
    let copy = dir.join(fixture.file_name().unwrap());
    std::fs::copy(fixture, &copy).unwrap();
    let source = LocalFileSource::open(&copy).unwrap();
    let cache = CacheMode::enabled(pgdump_query::cache::colocated_path(&copy));
    let request = StatisticsRequest {
        selection: StatisticsSelection::All,
        group_size: Some(NonZeroU64::new(SMALL_GROUP).unwrap()),
        ..StatisticsRequest::ALL
    };
    map_file(&source, &ScanOptions::default(), &cache, &request).await.unwrap();
    copy
}

/// What the dynamic filter reaching a scan must look like, read off its
/// final state as the scan prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Shape {
    /// A join's `k >= min AND k <= max` over the build side's keys.
    Bounds,
    /// A join's membership as an `IN` list: at most
    /// `hash_join_inlist_pushdown_max_distinct_values` build keys.
    InList,
    /// A join's membership past the `IN` list's limit.
    HashLookup,
    /// A `Partitioned` join's routing of each probe row to its partition's
    /// filter.
    Case,
    /// A join on several keys: `struct(k1, k2) IN (…)`.
    Struct,
    /// A null-equal join's `k IS NULL OR …`, or a TopK's NULL arm.
    IsNull,
    /// A TopK's lexicographic threshold, or an aggregate's `c < min OR
    /// c > max`: a strict comparison against a literal.
    Threshold,
    /// A TopK whose heap no further row can enter: `false`, which rules out
    /// every row the scan has left.
    Nothing,
}

impl Shape {
    /// Whether `text`, what one scan prints of the filters it holds, is of
    /// this shape.
    fn seen_in(self, text: &str) -> bool {
        match self {
            Shape::Bounds => text.contains(">=") && text.contains("<="),
            Shape::InList => text.contains(" IN (SET) (") || text.contains(" IN (["),
            Shape::HashLookup => text.contains("hash_lookup"),
            Shape::Case => text.contains("CASE "),
            Shape::Struct => text.contains("struct("),
            Shape::IsNull => text.contains("IS NULL"),
            Shape::Threshold => {
                text.contains(" < ") || text.contains(" > ") || text.contains(" IS NOT NULL")
            }
            Shape::Nothing => text.contains("DynamicFilter [ false ]"),
        }
    }
}

/// The session a query runs in, beside its flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Join {
    /// The planner's choice: a small build side is collected whole.
    Chosen,
    /// Every hash join `Partitioned`, so a join's filter is the `CASE` over
    /// its partitions'.
    Partitioned,
}

/// One query, the tables it reads, and the shapes the filter reaching its
/// scans must take under each join mode (`Chosen`, `Partitioned`); an empty
/// list asserts nothing, for a producer that publishes none there.
struct Query {
    sql: &'static str,
    tables: &'static [&'static str],
    chosen: &'static [Shape],
    partitioned: &'static [Shape],
}

const fn query(
    sql: &'static str,
    tables: &'static [&'static str],
    chosen: &'static [Shape],
    partitioned: &'static [Shape],
) -> Query {
    Query { sql, tables, chosen, partitioned }
}

use Shape::*;

/// Every shape the spec's "Evidence" names, each in a query of its own. The
/// build side of a join is the smaller one, as the planner picks it from the
/// provider's row counts; a static filter on it keeps it smaller.
const QUERIES: &[Query] = &[
    // Joins. An `IN` list over nine integer build keys.
    query(
        "SELECT o.id, o.low_card FROM moods m JOIN ordered o ON o.id = m.id",
        &["moods", "ordered"],
        &[Bounds, InList],
        &[Case, InList],
    ),
    // A build side past the `IN` list's 150 distinct keys.
    query(
        "SELECT a.id, b.id FROM ordered a JOIN (SELECT id, unsorted FROM ordered WHERE stepped < 50) b \
         ON a.unsorted = b.unsorted",
        &["ordered"],
        &[Bounds, HashLookup],
        &[Case, HashLookup],
    ),
    // Two keys: `struct(…) IN`.
    query(
        "SELECT o.id FROM (SELECT id, stepped FROM ordered WHERE id <= 20) b \
         JOIN ordered o ON o.id = b.id AND o.stepped = b.stepped",
        &["ordered"],
        &[Bounds, Struct],
        &[Case, Struct],
    ),
    // Text keys, and a build side holding NULLs that `=` never matches.
    query(
        "SELECT a.id FROM (SELECT high_card FROM ordered WHERE id <= 5) b \
         JOIN ordered a ON a.high_card = b.high_card",
        &["ordered"],
        &[Bounds, InList],
        &[Case, InList],
    ),
    query(
        "SELECT a.id FROM (SELECT gappy FROM ordered WHERE id <= 30) b \
         JOIN ordered a ON a.gappy = b.gappy",
        &["ordered"],
        &[Bounds, InList],
        &[Case, InList],
    ),
    // A null-equal join: NULL keys match, and the filter says so.
    query(
        "SELECT a.id FROM (SELECT gappy FROM ordered WHERE id <= 30) b \
         JOIN ordered a ON a.gappy IS NOT DISTINCT FROM b.gappy",
        &["ordered"],
        &[IsNull],
        &[Case, IsNull],
    ),
    // An enum's keys, compared by value against its dictionary. Nine rows
    // are one partition, so even a `Partitioned` join routes nothing.
    query(
        "SELECT a.id, a.m FROM (SELECT m FROM moods WHERE id <= 3) b JOIN moods a ON a.m = b.m",
        &["moods"],
        &[Bounds, InList],
        &[Bounds, InList],
    ),
    // Floats: -0 beside 0, NaN, both infinities.
    query(
        "SELECT a.id, a.f8_unsorted FROM (SELECT f8 FROM specials WHERE id IN (3, 4, 6, 7)) b \
         JOIN specials a ON a.f8_unsorted = b.f8",
        &["specials"],
        &[InList],
        &[Case, InList],
    ),
    // A probe table several blocks long, under `--load-via-partition-root`.
    query(
        "SELECT s.id, s.local FROM (SELECT id FROM ordered WHERE id BETWEEN 480 AND 520) b \
         JOIN spans s ON s.id = b.id",
        &["ordered", "spans"],
        &[Bounds, InList],
        &[Case, InList],
    ),
    // TopK, the sort keys alone.
    query(
        "SELECT unsorted FROM ordered ORDER BY unsorted LIMIT 7",
        &["ordered"],
        &[Threshold],
        &[Threshold],
    ),
    query(
        "SELECT unsorted FROM ordered ORDER BY unsorted DESC LIMIT 7",
        &["ordered"],
        &[Threshold],
        &[Threshold],
    ),
    // A column descending in the file, asked for ascending.
    query(
        "SELECT reversed FROM ordered ORDER BY reversed LIMIT 5",
        &["ordered"],
        &[Threshold],
        &[Threshold],
    ),
    // NULL placement, all four ways.
    // NULLs first, and more NULLs than the limit: the heap fills with them.
    query(
        "SELECT gappy FROM ordered ORDER BY gappy NULLS FIRST LIMIT 9",
        &["ordered"],
        &[Nothing],
        &[Nothing],
    ),
    query(
        "SELECT gappy FROM ordered ORDER BY gappy ASC NULLS LAST LIMIT 9",
        &["ordered"],
        &[Threshold],
        &[Threshold],
    ),
    query(
        "SELECT gappy FROM ordered ORDER BY gappy DESC NULLS FIRST LIMIT 200",
        &["ordered"],
        &[Threshold],
        &[Threshold],
    ),
    query(
        "SELECT gappy FROM ordered ORDER BY gappy DESC NULLS LAST LIMIT 9",
        &["ordered"],
        &[Threshold],
        &[Threshold],
    ),
    // Several keys: the lexicographic chain.
    query(
        "SELECT low_card, unsorted FROM ordered ORDER BY low_card DESC, unsorted LIMIT 7",
        &["ordered"],
        &[Threshold],
        &[Threshold],
    ),
    query(
        "SELECT c_text FROM ordered ORDER BY c_text DESC LIMIT 5",
        &["ordered"],
        &[Threshold],
        &[Threshold],
    ),
    // The enum is declared sorted by its labels' text, so ascending is a
    // limit with no TopK at all, and descending is a TopK.
    query("SELECT m FROM moods ORDER BY m LIMIT 4", &["moods"], &[], &[]),
    query("SELECT m FROM moods ORDER BY m DESC LIMIT 4", &["moods"], &[Threshold], &[Threshold]),
    query(
        "SELECT f8_unsorted FROM specials ORDER BY f8_unsorted LIMIT 3",
        &["specials"],
        &[Threshold],
        &[Threshold],
    ),
    query(
        "SELECT f8_unsorted FROM specials ORDER BY f8_unsorted DESC LIMIT 3",
        &["specials"],
        &[Threshold],
        &[Threshold],
    ),
    query(
        "SELECT local FROM spans ORDER BY local DESC LIMIT 5",
        &["spans"],
        &[Threshold],
        &[Threshold],
    ),
    // Ungrouped `MIN`/`MAX`, a static filter keeping the statistics from
    // answering them.
    query(
        "SELECT MIN(unsorted), MAX(unsorted) FROM ordered WHERE low_card = 'amber'",
        &["ordered"],
        &[Threshold],
        &[Threshold],
    ),
    query("SELECT MAX(gappy) FROM ordered WHERE id > 1", &["ordered"], &[Threshold], &[Threshold]),
    query(
        "SELECT MIN(f8_unsorted) FROM specials WHERE id > 0",
        &["specials"],
        &[Threshold],
        &[Threshold],
    ),
    // An enum's argument is coerced to its value type, so it gets none.
    query("SELECT MIN(m) FROM moods WHERE id > 1", &["moods"], &[], &[]),
];

/// Every producer's flag.
const FLAGS: [&str; 3] = [
    "datafusion.optimizer.enable_join_dynamic_filter_pushdown",
    "datafusion.optimizer.enable_topk_dynamic_filter_pushdown",
    "datafusion.optimizer.enable_aggregate_dynamic_filter_pushdown",
];

/// Several partitions and small batches, so a TopK's filter tightens while
/// the scan still streams and a partition's cut lands inside a block.
fn session(join: Join, on: bool) -> SessionContext {
    SessionContext::new_with_config(config(join, on))
}

/// [`session`], running each scan's partitions one at a time in `order`.
fn session_in(join: Join, on: bool, order: Order) -> SessionContext {
    SessionContext::new_with_state(
        SessionStateBuilder::new_with_default_features()
            .with_config(config(join, on))
            .with_physical_optimizer_rule(Arc::new(RunScansInOrder(order)))
            .build(),
    )
}

fn config(join: Join, on: bool) -> SessionConfig {
    let mut config = SessionConfig::new().with_target_partitions(3).with_batch_size(8);
    for flag in FLAGS {
        config = config.set_bool(flag, on);
    }
    if join == Join::Partitioned {
        config = config
            .set_usize("datafusion.optimizer.hash_join_single_partition_threshold", 0)
            .set_usize("datafusion.optimizer.hash_join_single_partition_threshold_rows", 0);
    }
    config
}

/// `dump`'s tables registered in `ctx` under their bare names.
fn register(ctx: &SessionContext, dump: &Arc<PgDump>) {
    // The catalog is what installs the session's budget and settings.
    register_dump(ctx, Some("dump"), dump, Arc::new(ignore)).unwrap();
    for name in dump.tables() {
        let table: Arc<dyn TableProvider> = dump.table(None, None, &name.table).unwrap();
        ctx.register_table(&name.table, table).unwrap();
    }
}

/// Each row of `batches` as text, sorted: the answer as a multiset.
fn rows(batches: &[RecordBatch]) -> Vec<String> {
    let options = FormatOptions::default().with_null("NULL");
    let mut out = Vec::new();
    for batch in batches {
        let columns: Vec<_> = batch
            .columns()
            .iter()
            .map(|c| ArrayFormatter::try_new(c.as_ref(), &options).unwrap())
            .collect();
        for row in 0..batch.num_rows() {
            out.push(
                columns.iter().map(|c| c.value(row).to_string()).collect::<Vec<_>>().join("|"),
            );
        }
    }
    out.sort();
    out
}

/// `sql`'s answer, and the plan it ran, whose scans now hold each filter in
/// its final state.
async fn run(ctx: &SessionContext, sql: &str) -> (Vec<String>, Arc<dyn ExecutionPlan>) {
    let df = ctx.sql(sql).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    let plan = df.create_physical_plan().await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    let batches = collect(Arc::clone(&plan), ctx.task_ctx()).await;
    (rows(&batches.unwrap_or_else(|e| panic!("{sql}: {e}"))), plan)
}

/// Every scan in `plan`.
fn scans(plan: &Arc<dyn ExecutionPlan>) -> Vec<Arc<dyn ExecutionPlan>> {
    let mut found: Vec<_> = plan.children().into_iter().flat_map(scans).collect();
    if plan.name() == "PgDumpExec" {
        found.push(Arc::clone(plan));
    }
    found
}

/// What each scan in `plan` prints of the dynamic filters it holds, for each
/// that holds one: its `predicate=` from the first of them, the static
/// filter it answers standing ahead of them there.
fn held(plan: &Arc<dyn ExecutionPlan>) -> Vec<String> {
    scans(plan)
        .iter()
        .filter_map(|scan| {
            let line = displayable(scan.as_ref()).one_line().to_string();
            let (_, predicate) = line.split_once(", predicate=")?;
            let first = predicate.find("DynamicFilter [")?;
            Some(predicate[first..].trim().to_string())
        })
        .collect()
}

/// The filters `plan`'s scans visit in `apply_expressions`.
fn visited(plan: &Arc<dyn ExecutionPlan>) -> Vec<Arc<dyn PhysicalExpr>> {
    let mut visited = Vec::new();
    for scan in scans(plan) {
        scan.apply_expressions(&mut |filter| {
            visited.push(Arc::clone(filter));
            Ok(TreeNodeRecursion::Continue)
        })
        .unwrap();
    }
    visited
}

/// What `plan`'s scans counted under the metric `name`, summed.
fn counted(plan: &Arc<dyn ExecutionPlan>, name: &str) -> usize {
    scans(plan)
        .iter()
        .filter_map(|scan| scan.metrics()?.sum_by_name(name))
        .map(|value| value.as_usize())
        .sum()
}

/// The metric counting the row groups a dynamic filter pruned.
const PRUNED_DYNAMIC: &str = "row_groups_pruned_dynamic_filter";

/// The metric counting the rows a dynamic filter dropped before decoding them.
const DROPPED_DYNAMIC: &str = "rows_pruned_dynamic_filter";

/// The metric counting the bytes an early stop left unread.
const UNREAD: &str = "bytes_unread_early_stop";

/// **Every join, TopK and aggregate shape answers alike with the producers'
/// filters on and off**, under either join mode, and each scan held the
/// filter it is here for; **and some filter pruned a group, some dropped a
/// row of a group it read, and some stopped a block its static filter alone
/// did not.**
#[tokio::test(flavor = "multi_thread")]
async fn a_dynamic_filter_changes_no_answer() {
    let dir = tempfile::tempdir().unwrap();
    let mut ran = vec![BTreeSet::new(); QUERIES.len()];
    let mut unshaped = Vec::new();
    let (mut pruning, mut dropping, mut stopping) =
        (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for fixture in statistics_fixtures() {
        let copy = parsed_copy(&fixture, dir.path()).await;
        let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
        let present: BTreeSet<&str> = dump.tables().iter().map(|t| t.table.as_str()).collect();
        for join in [Join::Chosen, Join::Partitioned] {
            let (off, on) = (session(join, false), session(join, true));
            register(&off, &dump);
            register(&on, &dump);
            for (i, query) in QUERIES.iter().enumerate() {
                if !query.tables.iter().all(|t| present.contains(t)) {
                    continue;
                }
                let at = format!("{} ({join:?}): {}", fixture.display(), query.sql);
                let (expected, unfiltered) = run(&off, query.sql).await;
                assert!(!expected.is_empty(), "{at}: returned nothing, so it proves nothing");
                let (got, plan) = run(&on, query.sql).await;
                assert_eq!(got, expected, "{at}: flags on");
                assert_eq!(counted(&unfiltered, PRUNED_DYNAMIC), 0, "{at}: flags off");
                assert_eq!(counted(&unfiltered, DROPPED_DYNAMIC), 0, "{at}: flags off");
                if counted(&plan, PRUNED_DYNAMIC) > 0 {
                    pruning.insert(query.sql);
                }
                if counted(&plan, DROPPED_DYNAMIC) > 0 {
                    dropping.insert(query.sql);
                }
                if counted(&plan, UNREAD) > counted(&unfiltered, UNREAD) {
                    stopping.insert(query.sql);
                }
                let seen = held(&plan);
                let shapes = match join {
                    Join::Chosen => query.chosen,
                    Join::Partitioned => query.partitioned,
                };
                for shape in shapes {
                    if !seen.iter().any(|f| shape.seen_in(f)) {
                        unshaped.push(format!("{at}: none is {shape:?}; held: {seen:?}"));
                    }
                }
                ran[i].insert(join as u8);
            }
        }
    }
    assert!(
        unshaped.is_empty(),
        "filters reaching a scan not of the shape asked:\n{}",
        unshaped.join("\n")
    );
    for (query, joins) in QUERIES.iter().zip(&ran) {
        assert_eq!(joins.len(), 2, "never ran under both join modes: {}", query.sql);
    }
    assert!(pruning.len() > 10, "queries whose filter pruned a group: {pruning:#?}");
    assert!(dropping.len() > 12, "queries whose filter dropped a row: {dropping:#?}");
    assert!(stopping.len() > 2, "queries whose filter stopped a block: {stopping:#?}");
}

/// `zeros`' columns, each holding both zeros at an extreme, in both orders of
/// appearance (`scripts/fixture_schema_statistics.sql`).
const ZEROS: [&str; 6] = [
    "min_pos_first",
    "min_neg_first",
    "max_neg_first",
    "max_pos_first",
    "r_min_pos_first",
    "r_max_neg_first",
];

/// **A float's zeros at its extremes answer alike with the producers'
/// filters on and off, whichever partition runs first.** A TopK and an
/// ungrouped `MIN`/`MAX` order `-0` below `0` while their thresholds'
/// evaluation calls them equal, so a scan pruning by such a threshold loses
/// the zero it rules out only where the partition holding the other zero has
/// already published it — which a schedule decides. Each order is run, so
/// the translator's rule for a zero (`dynamic_filter.rs`, `comparison`) is
/// held to both.
#[tokio::test(flavor = "multi_thread")]
async fn a_float_s_zeros_at_its_extremes_answer_alike_in_either_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut split = false;
    for fixture in statistics_fixtures() {
        let copy = parsed_copy(&fixture, dir.path()).await;
        let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
        if !dump.tables().iter().any(|t| t.table == "zeros") {
            continue;
        }
        for order in Order::ALL {
            let (off, on) =
                (session_in(Join::Chosen, false, order), session_in(Join::Chosen, true, order));
            register(&off, &dump);
            register(&on, &dump);
            for column in ZEROS {
                // A static filter keeps the statistics from answering the
                // aggregates.
                for sql in [
                    format!("SELECT MIN({column}) FROM zeros WHERE id > 0"),
                    format!("SELECT MAX({column}) FROM zeros WHERE id > 0"),
                    format!("SELECT {column} FROM zeros ORDER BY {column} LIMIT 1"),
                    format!("SELECT {column} FROM zeros ORDER BY {column} DESC LIMIT 1"),
                ] {
                    let at = format!("{} ({order:?}): {sql}", fixture.display());
                    let (expected, _) = run(&off, &sql).await;
                    let (got, plan) = run(&on, &sql).await;
                    assert_eq!(got, expected, "{at}: flags on");
                    // A column declared sorted is read under a limit with
                    // no TopK, so only the aggregates are sure to publish.
                    let aggregate = !sql.contains("LIMIT");
                    assert!(
                        !aggregate || !held(&plan).is_empty(),
                        "{at}: no filter reached the scan"
                    );
                    let [scan] = scans(&plan).try_into().unwrap();
                    split |= scan.output_partitioning().partition_count() > 1;
                }
            }
        }
    }
    assert!(split, "`zeros` was never read as more than one partition, so no order was run");
}

/// The newest major's `statistics` fixture, parsed, and a session over it
/// with every producer's flag on.
async fn statistics_session(dir: &Path) -> SessionContext {
    let fixture = fixtures_root().join("18/statistics/default.sql");
    let copy = parsed_copy(&fixture, dir).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let ctx = session(Join::Chosen, true);
    register(&ctx, &dump);
    ctx
}

const JOIN: &str = "SELECT o.id, o.low_card FROM moods m JOIN ordered o ON o.id = m.id";
const TOPK: &str = "SELECT unsorted FROM ordered ORDER BY unsorted LIMIT 7";

async fn planned(ctx: &SessionContext, sql: &str) -> Arc<dyn ExecutionPlan> {
    ctx.sql(sql).await.unwrap().create_physical_plan().await.unwrap()
}

/// **A scan prints each dynamic filter it holds, `empty` until its first
/// update**, and visits it where a join looks for its consumer; once a join
/// has run, its filter is printed in its final state.
#[tokio::test(flavor = "multi_thread")]
async fn a_scan_prints_each_filter_it_holds_empty_until_its_first_update() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = statistics_session(dir.path()).await;
    for sql in [JOIN, TOPK] {
        let plan = planned(&ctx, sql).await;
        assert_eq!(held(&plan), ["DynamicFilter [ empty ]"], "{sql}");
        assert_eq!(visited(&plan).len(), 1, "{sql}");
    }
    let (_, plan) = run(&ctx, JOIN).await;
    let [filter] = held(&plan).try_into().unwrap();
    assert!(Shape::Bounds.seen_in(&filter) && Shape::InList.seen_in(&filter), "{filter}");
    let tree = displayable(plan.as_ref()).tree_render().to_string();
    assert!(tree.contains("predicate") && tree.contains("DynamicFilter"), "{tree}");
}

/// **A join's filter prunes its probe side and stops it**: `moods` holds ids
/// 1 to 9, so `ordered`, whose `id` ascends, is read no further than its first
/// row past 9 and none of the groups after it, which the scan's metrics count
/// under `EXPLAIN ANALYZE` — and with the producers' flags off, not.
#[tokio::test(flavor = "multi_thread")]
async fn a_join_s_filter_prunes_its_probe_side_and_stops_it() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = statistics_session(dir.path()).await;
    let (rows, plan) = run(&ctx, JOIN).await;
    assert_eq!(rows.len(), 9, "{rows:?}");
    assert!(counted(&plan, PRUNED_DYNAMIC) > 0, "{}", displayable(plan.as_ref()).indent(true));
    assert!(counted(&plan, UNREAD) > 0, "{}", displayable(plan.as_ref()).indent(true));
    let explained = ctx.sql(&format!("EXPLAIN ANALYZE {JOIN}")).await.unwrap();
    let explained = explained.collect().await.unwrap();
    let explained = arrow::util::pretty::pretty_format_batches(&explained).unwrap().to_string();
    assert!(explained.contains(PRUNED_DYNAMIC), "{explained}");
    assert!(explained.contains(DROPPED_DYNAMIC), "{explained}");

    let off = session(Join::Chosen, false);
    let fixture = fixtures_root().join("18/statistics/default.sql");
    let copy = parsed_copy(&fixture, dir.path()).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    register(&off, &dump);
    let (unfiltered, plan) = run(&off, JOIN).await;
    assert_eq!(unfiltered, rows);
    let counts = [PRUNED_DYNAMIC, DROPPED_DYNAMIC, UNREAD].map(|name| counted(&plan, name));
    assert_eq!(counts, [0, 0, 0]);
}

/// The rows each output partition of `plan`'s hash join emitted.
fn joined_per_partition(plan: &Arc<dyn ExecutionPlan>) -> Vec<usize> {
    fn join(plan: &Arc<dyn ExecutionPlan>) -> Option<Arc<dyn ExecutionPlan>> {
        if plan.name() == "HashJoinExec" {
            return Some(Arc::clone(plan));
        }
        plan.children().into_iter().find_map(join)
    }
    let join = join(plan).expect("a hash join");
    let mut rows = vec![0; join.properties().partitioning.partition_count()];
    for metric in join.metrics().unwrap().iter() {
        if let (MetricValue::OutputRows(count), Some(partition)) =
            (metric.value(), metric.partition())
        {
            rows[partition] += count.value();
        }
    }
    rows
}

/// **A selective join on a clustered key spreads its probe side over every
/// partition**: a build side of `ordered`'s ids 400 to 499 publishes a filter
/// keeping a run of groups that the planned cut leaves to one of the probe's
/// three partitions, and the cut made at the probe's first poll, where the
/// filter is complete, gives each partition a third of that run, so each
/// joins a share of the hundred rows — where with the producers' flags off
/// one partition joins them all.
#[tokio::test(flavor = "multi_thread")]
async fn a_selective_join_s_probe_side_is_cut_over_what_its_filter_keeps() {
    const SELECTIVE: &str = "SELECT o.id FROM (SELECT id FROM ordered WHERE id BETWEEN 400 AND 499) b \
                             JOIN ordered o ON o.id = b.id";
    let dir = tempfile::tempdir().unwrap();
    let ctx = statistics_session(dir.path()).await;
    let (rows, plan) = run(&ctx, SELECTIVE).await;
    assert_eq!(rows.len(), 100);
    let spread = joined_per_partition(&plan);
    assert_eq!(spread.len(), 3, "{}", displayable(plan.as_ref()).indent(true));
    assert!(spread.iter().all(|&rows| rows > 0), "{spread:?}");

    let off = session(Join::Chosen, false);
    let fixture = fixtures_root().join("18/statistics/default.sql");
    let copy = parsed_copy(&fixture, dir.path()).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    register(&off, &dump);
    let (unfiltered, plan) = run(&off, SELECTIVE).await;
    assert_eq!(unfiltered, rows);
    let planned = joined_per_partition(&plan);
    assert_eq!(planned.iter().filter(|&&rows| rows > 0).count(), 1, "{planned:?}");
}

/// **A static filter is not held**: one the provider could not answer is a
/// `FilterExec`'s, pushed to the scan in the `Pre` phase and declined there.
#[tokio::test(flavor = "multi_thread")]
async fn a_scan_holds_no_static_filter() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = statistics_session(dir.path()).await;
    let plan = planned(&ctx, "SELECT id FROM ordered WHERE id + 1 > 5").await;
    let shown = displayable(plan.as_ref()).indent(true).to_string();
    assert!(shown.contains("FilterExec"), "{shown}");
    assert!(held(&plan).is_empty() && visited(&plan).is_empty(), "{shown}");
}

/// **A second `Post` pass holds nothing twice**: a TopK pushes the filter it
/// already published again, and a fetch set on the scan afterwards keeps
/// what it holds.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_pass_holds_nothing_twice_and_a_fetch_keeps_it() {
    use datafusion::physical_optimizer::PhysicalOptimizerRule;
    use datafusion::physical_optimizer::filter_pushdown::FilterPushdown;

    let dir = tempfile::tempdir().unwrap();
    let ctx = statistics_session(dir.path()).await;
    let plan = planned(&ctx, TOPK).await;
    let options = ctx.state().config_options().as_ref().clone();
    let again = FilterPushdown::new_post_optimization().optimize(plan, &options).unwrap();
    assert_eq!(visited(&again).len(), 1, "{}", displayable(again.as_ref()).indent(true));
    let [scan] = scans(&again).try_into().unwrap();
    let fetched = scan.with_fetch(Some(3)).unwrap();
    assert_eq!(visited(&fetched).len(), 1);
}

/// **A reset drops every filter a scan holds**, its producers' having been
/// replaced or discarded by their own resets, and the reset plan runs to the
/// same answer.
#[tokio::test(flavor = "multi_thread")]
async fn a_reset_drops_every_filter_a_scan_holds() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = statistics_session(dir.path()).await;
    for sql in [JOIN, TOPK] {
        let (expected, plan) = run(&ctx, sql).await;
        assert_eq!(visited(&plan).len(), 1, "{sql}");
        let reset = reset_plan_states(plan).unwrap();
        assert!(held(&reset).is_empty() && visited(&reset).is_empty(), "{sql}");
        let batches = collect(reset, ctx.task_ctx()).await.unwrap();
        assert_eq!(rows(&batches), expected, "{sql}");
    }
}

/// **The figures' generator mirrors the largest `IN` list a join publishes**,
/// so its costing join is published as one list of that many values rather
/// than as a `hash_lookup` the scan cannot read
/// (`scripts/generate_dynamic_filter_bench.py`, `BUCKETS`).
#[test]
fn the_costing_input_is_the_largest_in_list_a_join_publishes() {
    let generator =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/generate_dynamic_filter_bench.py");
    let text = std::fs::read_to_string(generator).unwrap();
    let buckets: usize = text
        .lines()
        .find_map(|line| line.strip_prefix("BUCKETS = "))
        .expect("the generator states BUCKETS")
        .trim()
        .parse()
        .unwrap();
    let optimizer = ConfigOptions::default().optimizer;
    assert_eq!(buckets, optimizer.hash_join_inlist_pushdown_max_distinct_values);
}
