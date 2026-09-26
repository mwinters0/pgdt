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
//! cannot pass by exercising nothing. A third session carries each table
//! under a recording node that holds whatever filter reaches the scan's
//! position, answers `No` for it, and visits it in `apply_expressions` —
//! which is what makes a join compute its filter at all — and the filters'
//! final state is read after the query ran. The recording node filters
//! nothing: its answers are checked against the flags-off session too.
//!
//! Every query selects its sort keys alone where it has a `LIMIT`, since
//! ties at the cut are the TopK's to break however it likes.

use std::collections::BTreeSet;
use std::fmt;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arrow::array::RecordBatch;
use arrow::util::display::{ArrayFormatter, FormatOptions};
use async_trait::async_trait;
use datafusion::catalog::{Session, TableProvider};
use datafusion::common::tree_node::TreeNodeRecursion;
use datafusion::common::{Result, Statistics};
use datafusion::config::ConfigOptions;
use datafusion::datasource::TableType;
use datafusion::execution::{SendableRecordBatchStream, TaskContext};
use datafusion::logical_expr::{Expr, TableProviderFilterPushDown};
use datafusion::physical_expr::PhysicalExpr;
use datafusion::physical_plan::execution_plan::{ChildrenPropertiesMode, ReplaceChildrenOptions};
use datafusion::physical_plan::filter_pushdown::{
    ChildPushdownResult, FilterPushdownPhase, FilterPushdownPropagation, PushedDown,
};
use datafusion::physical_plan::statistics::{ChildStats, StatisticsArgs};
use datafusion::physical_plan::{
    DisplayAs, DisplayFormatType, ExecutionPlan, PlanProperties, apply_expression_roots,
};
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion_pgdump::{PgDump, PgDumpOptions, register_dump};
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    Finding, LocalFileSource, ScanOptions, StatisticsRequest, StatisticsSelection, map_file,
};

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
/// final state's display.
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
    /// Whether `text`, one held filter's final display, is of this shape.
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
    let mut config = SessionConfig::new().with_target_partitions(3).with_batch_size(8);
    for flag in FLAGS {
        config = config.set_bool(flag, on);
    }
    if join == Join::Partitioned {
        config = config
            .set_usize("datafusion.optimizer.hash_join_single_partition_threshold", 0)
            .set_usize("datafusion.optimizer.hash_join_single_partition_threshold_rows", 0);
    }
    SessionContext::new_with_config(config)
}

/// `dump`'s tables registered in `ctx` under their bare names, each under a
/// [`Recorded`] where `held` is given.
fn register(ctx: &SessionContext, dump: &Arc<PgDump>, held: Option<&Held>) {
    // The catalog is what installs the session's budget and settings.
    register_dump(ctx, Some("dump"), dump, Arc::new(ignore)).unwrap();
    for name in dump.tables() {
        let table: Arc<dyn TableProvider> = dump.table(None, None, &name.table).unwrap();
        let table = match held {
            Some(held) => Arc::new(Recorded { inner: table, held: Arc::clone(held) }),
            None => table,
        };
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

async fn answer(ctx: &SessionContext, sql: &str) -> Vec<String> {
    let df = ctx.sql(sql).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    rows(&df.collect().await.unwrap_or_else(|e| panic!("{sql}: {e}")))
}

/// **Every join, TopK and aggregate shape answers alike with the producers'
/// filters on and off**, under either join mode, and each published the
/// filter it is here for.
#[tokio::test(flavor = "multi_thread")]
async fn a_dynamic_filter_changes_no_answer() {
    let dir = tempfile::tempdir().unwrap();
    let mut ran = vec![BTreeSet::new(); QUERIES.len()];
    let mut unshaped = Vec::new();
    for fixture in statistics_fixtures() {
        let copy = parsed_copy(&fixture, dir.path()).await;
        let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
        let present: BTreeSet<&str> = dump.tables().iter().map(|t| t.table.as_str()).collect();
        for join in [Join::Chosen, Join::Partitioned] {
            let (off, on, recording) =
                (session(join, false), session(join, true), session(join, true));
            let held: Held = Arc::default();
            register(&off, &dump, None);
            register(&on, &dump, None);
            register(&recording, &dump, Some(&held));
            for (i, query) in QUERIES.iter().enumerate() {
                if !query.tables.iter().all(|t| present.contains(t)) {
                    continue;
                }
                let at = format!("{} ({join:?}): {}", fixture.display(), query.sql);
                let expected = answer(&off, query.sql).await;
                assert!(!expected.is_empty(), "{at}: returned nothing, so it proves nothing");
                assert_eq!(answer(&on, query.sql).await, expected, "{at}: flags on");
                held.lock().unwrap().clear();
                assert_eq!(answer(&recording, query.sql).await, expected, "{at}: recorded");
                let seen: Vec<String> =
                    held.lock().unwrap().iter().map(|f| f.to_string()).collect();
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

/// The filters a [`Recorded`] scan has held while it executed.
type Held = Arc<Mutex<Vec<Arc<dyn PhysicalExpr>>>>;

/// A table whose scan is wrapped in a [`Recording`] node.
#[derive(Debug)]
struct Recorded {
    inner: Arc<dyn TableProvider>,
    held: Held,
}

#[async_trait]
impl TableProvider for Recorded {
    fn schema(&self) -> arrow::datatypes::SchemaRef {
        self.inner.schema()
    }

    fn table_type(&self) -> TableType {
        self.inner.table_type()
    }

    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> Result<Vec<TableProviderFilterPushDown>> {
        self.inner.supports_filters_pushdown(filters)
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let input = self.inner.scan(state, projection, filters, limit).await?;
        Ok(Arc::new(Recording { input, filters: Vec::new(), held: Arc::clone(&self.held) }))
    }
}

/// Holds every filter pushed to it in the `Post` phase and answers `No` for
/// each, passing its input through untouched: the scan's position in the
/// plan, as a consumer would occupy it, without consuming anything.
#[derive(Debug)]
struct Recording {
    input: Arc<dyn ExecutionPlan>,
    filters: Vec<Arc<dyn PhysicalExpr>>,
    held: Held,
}

impl DisplayAs for Recording {
    fn fmt_as(&self, _: DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Recording: filters={}", self.filters.len())
    }
}

impl ExecutionPlan for Recording {
    fn name(&self) -> &str {
        "Recording"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        self.input.properties()
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![&self.input]
    }

    /// The held filters: what a join searches its probe side for before it
    /// computes one.
    fn apply_expressions(
        &self,
        f: &mut dyn FnMut(&Arc<dyn PhysicalExpr>) -> Result<TreeNodeRecursion>,
    ) -> Result<TreeNodeRecursion> {
        apply_expression_roots(&self.filters, f)
    }

    fn maintains_input_order(&self) -> Vec<bool> {
        vec![true]
    }

    fn benefits_from_input_partitioning(&self) -> Vec<bool> {
        vec![false]
    }

    fn replace_children(
        self: Arc<Self>,
        mut children: Vec<Arc<dyn ExecutionPlan>>,
        _: ReplaceChildrenOptions,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        Ok(Arc::new(Recording {
            input: children.swap_remove(0),
            filters: self.filters.clone(),
            held: Arc::clone(&self.held),
        }))
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        self.replace_children(
            children,
            ReplaceChildrenOptions::new(ChildrenPropertiesMode::Recompute),
        )
    }

    fn handle_child_pushdown_result(
        &self,
        phase: FilterPushdownPhase,
        child_pushdown_result: ChildPushdownResult,
        _config: &ConfigOptions,
    ) -> Result<FilterPushdownPropagation<Arc<dyn ExecutionPlan>>> {
        let pushed: Vec<_> =
            child_pushdown_result.parent_filters.into_iter().map(|f| f.filter).collect();
        let declined = FilterPushdownPropagation::with_parent_pushdown_result(vec![
            PushedDown::No;
            pushed.len()
        ]);
        if phase != FilterPushdownPhase::Post || pushed.is_empty() {
            return Ok(declined);
        }
        let mut filters = self.filters.clone();
        filters.extend(pushed);
        Ok(declined.with_updated_node(Arc::new(Recording {
            input: Arc::clone(&self.input),
            filters,
            held: Arc::clone(&self.held),
        })))
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        if partition == 0 {
            self.held.lock().unwrap().extend(self.filters.iter().cloned());
        }
        self.input.execute(partition, context)
    }

    fn child_stats_requests(&self, partition: Option<usize>) -> Vec<ChildStats> {
        vec![ChildStats::At(partition)]
    }

    fn statistics_from_inputs(
        &self,
        input_stats: &[Arc<Statistics>],
        _args: &StatisticsArgs,
    ) -> Result<Arc<Statistics>> {
        Ok(Arc::clone(&input_stats[0]))
    }
}
