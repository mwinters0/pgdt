//! What a scan reports: what its plan settled, to the sink its table was
//! registered with, when it is planned; the filter it answers, on its
//! `EXPLAIN` line; and what it finds while reading — the groups statistics
//! pruned, the bytes an early stop left unread — as the plan node's metrics
//! under `EXPLAIN ANALYZE`.
//!
//! **Over the statistics fixture gathered at a small group size**, so
//! `public.ordered`'s ascending ids span many groups and a range filter on
//! them both prunes groups and stops early in the group it keeps, as the
//! library's own `tests/pruning.rs` has it.

use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arrow::util::pretty::pretty_format_batches;
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::physical_plan::metrics::MetricValue;
use datafusion::physical_plan::{ExecutionPlan, collect, displayable};
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion_pgdump::{
    AllowanceOrigin, BudgetAccount, BudgetedPlanNote, PgDump, PgDumpOptions, ScanBudget,
    register_dump,
};
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    DiagnosticSink, Finding, LocalFileSource, MEMORY_RESERVE, PlanNote, PlanNoteKind, ScanOptions,
    Severity, StatisticsRequest, StatisticsSelection, map_file,
};

/// A group size several `ordered` rows long.
const SMALL_GROUP: u64 = 1024;

const ORDERED: &str = "shop.public.ordered";

/// The statistics fixture copied into `dir` beside the complete cache a parse
/// gathering every column's statistics at [`SMALL_GROUP`] leaves.
async fn gathered_copy(dir: &Path) -> PathBuf {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/16/statistics/default.sql");
    let copy = dir.join("default.sql");
    std::fs::copy(fixture, &copy).unwrap();
    let source = LocalFileSource::open(&copy).unwrap();
    let request = StatisticsRequest {
        selection: StatisticsSelection::All,
        group_size: Some(NonZeroU64::new(SMALL_GROUP).unwrap()),
        ..StatisticsRequest::ALL
    };
    let cache = CacheMode::enabled(pgdump_query::cache::colocated_path(&copy));
    map_file(&source, &ScanOptions::default(), &cache, &request).await.unwrap();
    copy
}

/// What a sink heard of the plan-note channel: each note, with its severity
/// and sentence, and a budget-quoting one's account. Every other finding is
/// registration's, and dropped.
#[derive(Default)]
struct Plans(Mutex<Vec<(Severity, String, PlanNote)>>, Mutex<Vec<BudgetAccount>>);

impl DiagnosticSink for Plans {
    fn report(&self, finding: &dyn Finding) {
        let any = finding.as_any();
        let note = match any.downcast_ref::<BudgetedPlanNote>() {
            Some(budgeted) => {
                self.1.lock().unwrap().push(budgeted.account.clone());
                &budgeted.note
            }
            None => match any.downcast_ref::<PlanNote>() {
                Some(note) => {
                    assert_eq!(note.budget_bytes(), None, "a budget-quoting note arrives wrapped");
                    note
                }
                None => return,
            },
        };
        let heard = (finding.severity(), finding.message(), note.clone());
        self.0.lock().unwrap().push(heard);
    }
}

impl Plans {
    fn take(&self) -> Vec<(Severity, String, PlanNote)> {
        std::mem::take(&mut self.0.lock().unwrap())
    }

    fn accounts(&self) -> Vec<BudgetAccount> {
        std::mem::take(&mut self.1.lock().unwrap())
    }
}

fn session(partitions: usize) -> SessionContext {
    SessionContext::new_with_config(SessionConfig::new().with_target_partitions(partitions))
}

async fn plan(ctx: &SessionContext, sql: &str) -> Arc<dyn ExecutionPlan> {
    ctx.sql(sql).await.unwrap().create_physical_plan().await.unwrap()
}

/// The scan's own node in `plan`.
fn pgdump_exec(plan: &Arc<dyn ExecutionPlan>) -> Option<Arc<dyn ExecutionPlan>> {
    if plan.name() == "PgDumpExec" {
        return Some(Arc::clone(plan));
    }
    plan.children().into_iter().find_map(pgdump_exec)
}

/// The metric `name` on `plan`'s scan, summed over its partitions.
fn metric(plan: &Arc<dyn ExecutionPlan>, name: &str) -> Option<MetricValue> {
    let metrics = pgdump_exec(plan).unwrap().metrics().unwrap().aggregate_by_name();
    metrics.iter().find(|m| m.value().name() == name).map(|m| m.value().clone())
}

/// **A scan's plan notes reach its table's sink when it is planned**, named
/// as SQL reaches the table and carrying the library's severity: before a row
/// is read, a pruning filter is an `Info`, and a budget that affords no reader
/// a `Warning`. A table registered by hand hears them once it is reported.
#[tokio::test(flavor = "multi_thread")]
async fn a_scan_s_plan_notes_reach_its_table_s_sink_when_it_is_planned() {
    let dir = tempfile::tempdir().unwrap();
    let copy = gathered_copy(dir.path()).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let pruning = format!("SELECT id FROM {ORDERED} WHERE id < 500");

    let plans = Arc::new(Plans::default());
    let ctx = session(1);
    register_dump(&ctx, Some("shop"), &dump, Arc::clone(&plans) as _).unwrap();
    assert!(plans.take().is_empty(), "registration plans nothing");
    let _plan = plan(&ctx, &pruning).await;
    let heard = plans.take();
    let [(severity, message, note)] = heard.as_slice() else { panic!("{heard:#?}") };
    let PlanNoteKind::StatisticsPruned { skipped_groups, groups, .. } = note.kind else {
        panic!("{heard:#?}")
    };
    assert!(0 < skipped_groups && skipped_groups < groups, "{message}");
    assert_eq!(*severity, Severity::Info);
    assert!(message.starts_with(&format!("{ORDERED}: row-group statistics rule out")), "{message}");

    // A budget carved from an allowance the reserve takes whole affords no
    // reader, which the plan says rather than refusing.
    let tight = Arc::new(ScanBudget::new(MEMORY_RESERVE));
    let ctx = SessionContext::new_with_config(
        SessionConfig::new().with_target_partitions(4).with_extension(tight),
    );
    register_dump(&ctx, Some("shop"), &dump, Arc::clone(&plans) as _).unwrap();
    let _plan = plan(&ctx, &format!("SELECT * FROM {ORDERED}")).await;
    let heard = plans.take();
    assert!(
        heard.iter().any(|(severity, message, _)| *severity == Severity::Warning
            && message.starts_with(&format!("{ORDERED}: "))),
        "{heard:#?}"
    );

    // By hand: silent until reported, then under the name it was given.
    let table = dump.table(None, Some("public"), "ordered").unwrap();
    let ctx = session(1);
    ctx.register_table("o", Arc::clone(&table) as _).unwrap();
    let _plan = plan(&ctx, "SELECT id FROM o WHERE id < 500").await;
    assert!(plans.take().is_empty());
    table.report("o", Arc::clone(&plans) as _);
    let _plan = plan(&ctx, "SELECT id FROM o WHERE id < 500").await;
    let heard = plans.take();
    assert!(
        matches!(heard.as_slice(), [(_, message, _)] if message.starts_with("o: row-group")),
        "{heard:#?}"
    );
}

/// **A note quoting a budget says what that budget was carved from**: the
/// allowance and its origin, the session pool's limit, the dumps' resident
/// statistics and what live scans had drawn, each in the sentence the sink
/// hears beside the setting keys that move them; a note quoting none — a
/// pruning filter's — reaches the sink as the library's own.
#[tokio::test(flavor = "multi_thread")]
async fn a_budget_quoting_note_carries_the_scan_s_account() {
    let dir = tempfile::tempdir().unwrap();
    let copy = gathered_copy(dir.path()).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let pool = 64usize << 20;
    let runtime = RuntimeEnvBuilder::new().with_memory_limit(pool, 1.0).build_arc().unwrap();
    let budget = Arc::new(ScanBudget::new(MEMORY_RESERVE));
    let ctx = SessionContext::new_with_config_rt(
        SessionConfig::new().with_target_partitions(4).with_extension(Arc::clone(&budget)),
        runtime,
    );
    let plans = Arc::new(Plans::default());
    register_dump(&ctx, Some("shop"), &dump, Arc::clone(&plans) as _).unwrap();
    let resident = budget.resident();
    assert!(resident > 0, "the gathered statistics are billed");

    let _plan = plan(&ctx, &format!("SELECT * FROM {ORDERED}")).await;
    let heard = plans.take();
    let floors: Vec<&String> = heard
        .iter()
        .filter(|(_, _, note)| matches!(note.kind, PlanNoteKind::AllocationBelowFloor { .. }))
        .map(|(_, message, _)| message)
        .collect();
    let [floor] = floors.as_slice() else { panic!("{heard:#?}") };
    let accounts = plans.accounts();
    let expected = BudgetAccount {
        allowance: Some(MEMORY_RESERVE),
        origin: AllowanceOrigin::Stated,
        pool_limit: pool as u64,
        resident,
        drawn: 0,
    };
    // Every note of that plan quotes its budget, each with the one account.
    assert_eq!(accounts, vec![expected; heard.len()], "{heard:#?}");
    for term in [
        format!("an allowance of {MEMORY_RESERVE} resident byte(s)"),
        format!("granted {pool} byte(s)"),
        format!("statistics hold {resident} byte(s)"),
        "pgdump.memory".to_string(),
        "datafusion.runtime.memory_limit".to_string(),
    ] {
        assert!(floor.contains(&term), "{term}: {floor}");
    }
    // A zero budget is below every reader however small its chunk, so the
    // library lists no chunk lever and the clause names no chunk key.
    assert!(!floor.contains("pgdump.chunk_size"), "{floor}");

    let _pruned = plan(&ctx, &format!("SELECT id FROM {ORDERED} WHERE id < 500")).await;
    let heard = plans.take();
    assert!(
        heard.iter().any(|(_, _, note)| matches!(note.kind, PlanNoteKind::StatisticsPruned { .. })),
        "{heard:#?}"
    );
}

/// **A narrowed batch span on a plain source names the read chunk and not
/// the allowance**: the budget stops at `DEFAULT_MEMORY_BUDGET` however large
/// an allowance is stated (`docs/design/decisions.md`, "D83"), and each
/// reader's charge is a multiple of the chunk, so a smaller one leaves every
/// seated sub-stream a wider batch.
#[tokio::test(flavor = "multi_thread")]
async fn a_narrowed_span_on_a_plain_source_names_the_chunk() {
    let dir = tempfile::tempdir().unwrap();
    let copy = gathered_copy(dir.path()).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let budget = Arc::new(ScanBudget::new(4 << 30));
    let ctx = SessionContext::new_with_config(
        SessionConfig::new().with_target_partitions(4).with_extension(budget),
    );
    let plans = Arc::new(Plans::default());
    register_dump(&ctx, Some("shop"), &dump, Arc::clone(&plans) as _).unwrap();
    let _plan = plan(&ctx, &format!("SELECT * FROM {ORDERED}")).await;
    let heard = plans.take();
    let [(severity, message, note)] = heard.as_slice() else { panic!("{heard:#?}") };
    assert!(matches!(note.kind, PlanNoteKind::BatchSpanNarrowed { .. }), "{heard:#?}");
    assert_eq!(*severity, Severity::Info);
    let (_, keys) = message.split_once("the settings that move it: ").expect(message);
    for key in ["pgdump.chunk_size", "datafusion.execution.target_partitions"] {
        assert!(keys.contains(key), "{key}: {message}");
    }
    assert!(!keys.contains("pgdump.memory"), "{message}");
}

/// **The groups statistics pruned and the bytes an early stop left unread
/// are the scan's metrics**, as the plan note counts the first, and
/// `EXPLAIN ANALYZE` shows both. A filter no sorted order closes stops
/// nowhere, and one ruling out no group prunes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn pruned_groups_and_early_stop_bytes_are_the_scan_s_metrics() {
    let dir = tempfile::tempdir().unwrap();
    let copy = gathered_copy(dir.path()).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let plans = Arc::new(Plans::default());
    let ctx = session(1);
    register_dump(&ctx, Some("shop"), &dump, Arc::clone(&plans) as _).unwrap();
    plans.take();

    let query = format!("SELECT id FROM {ORDERED} WHERE id < 500");
    let planned = plan(&ctx, &query).await;
    let rows = collect(Arc::clone(&planned), ctx.task_ctx()).await.unwrap();
    assert_eq!(rows.iter().map(|b| b.num_rows()).sum::<usize>(), 499);
    let heard = plans.take();
    let [(_, _, note)] = heard.as_slice() else { panic!("{heard:#?}") };
    let PlanNoteKind::StatisticsPruned { skipped_groups, groups, .. } = note.kind else {
        panic!("{heard:#?}")
    };
    let Some(MetricValue::PruningMetrics { pruning_metrics, .. }) =
        metric(&planned, "row_groups_pruned_statistics")
    else {
        panic!("no pruning metric")
    };
    assert_eq!(pruning_metrics.pruned() as u64, skipped_groups);
    assert_eq!((pruning_metrics.pruned() + pruning_metrics.matched()) as u64, groups);
    let unread = metric(&planned, "bytes_unread_early_stop").unwrap().as_usize();
    assert!(unread > 0, "the kept group's rows past 499 are not read");

    // `unsorted` has no order to stop on; `id <= 1000` rules no group out,
    // and its bound is passed only by the last row, which saves nothing.
    for (query, prunes) in [
        (format!("SELECT id FROM {ORDERED} WHERE unsorted < 20"), None),
        (format!("SELECT id FROM {ORDERED} WHERE id <= 1000"), Some(0)),
    ] {
        let planned = plan(&ctx, &query).await;
        collect(Arc::clone(&planned), ctx.task_ctx()).await.unwrap();
        if let Some(prunes) = prunes {
            let Some(MetricValue::PruningMetrics { pruning_metrics, .. }) =
                metric(&planned, "row_groups_pruned_statistics")
            else {
                panic!("{query}: no pruning metric")
            };
            assert_eq!(pruning_metrics.pruned(), prunes, "{query}");
        }
        let unread = metric(&planned, "bytes_unread_early_stop").unwrap().as_usize();
        assert_eq!(unread, 0, "{query}");
    }

    let explained = ctx.sql(&format!("EXPLAIN ANALYZE {query}")).await.unwrap();
    let explained = pretty_format_batches(&explained.collect().await.unwrap()).unwrap().to_string();
    assert!(explained.contains("row_groups_pruned_statistics"), "{explained}");
    assert!(explained.contains("bytes_unread_early_stop"), "{explained}");
}

/// **A scan's `EXPLAIN` line prints the filter it answers as its
/// `predicate=`**, by bare column name however the query qualified it and
/// whether or not the scan emits the column, ahead of the dynamic filters it
/// holds, and so does its tree rendering; a filter it cannot answer is the
/// `FilterExec`'s above it, and the scan prints none.
#[tokio::test(flavor = "multi_thread")]
async fn a_scan_prints_the_filter_it_answers_as_its_predicate() {
    let dir = tempfile::tempdir().unwrap();
    let copy = gathered_copy(dir.path()).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let ctx = session(1);
    register_dump(&ctx, Some("shop"), &dump, Arc::new(Plans::default()) as _).unwrap();
    for (query, printed) in [
        (format!("SELECT id FROM {ORDERED} WHERE id < 50"), Some("id < 50")),
        (
            format!(
                "SELECT o.id FROM {ORDERED} o WHERE o.stepped > 3 AND (o.id < 5 OR o.id > 100)"
            ),
            Some("stepped > 3 AND (id < 5 OR id > 100)"),
        ),
        (
            format!(
                "SELECT o.id FROM {ORDERED} o WHERE o.id < 5 OR o.stepped > 3 AND o.low_card = 'amber'"
            ),
            Some("id < 5 OR stepped > 3 AND low_card = amber"),
        ),
        (
            format!("SELECT unsorted FROM {ORDERED} WHERE id > 1 ORDER BY unsorted LIMIT 7"),
            Some("id > 1 AND DynamicFilter [ empty ]"),
        ),
        (format!("SELECT id FROM {ORDERED} WHERE id + 1 > 5"), None),
    ] {
        let planned = plan(&ctx, &query).await;
        let line = displayable(pgdump_exec(&planned).unwrap().as_ref()).one_line().to_string();
        let predicate = line.split_once(", predicate=").map(|(_, predicate)| predicate.trim());
        assert_eq!(predicate, printed, "{query}");
    }
    let planned = plan(&ctx, &format!("SELECT id FROM {ORDERED} WHERE id < 50")).await;
    let tree = displayable(planned.as_ref()).tree_render().to_string();
    assert!(tree.contains("predicate: id < 50"), "{tree}");
}
