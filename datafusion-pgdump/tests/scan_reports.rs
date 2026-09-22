//! What a scan reports (`docs/design/roadmap-P6-datafusion.md`, "Diagnostics:
//! one sink"): what its plan settled, to the sink its table was registered
//! with, when it is planned; and what it finds while reading — the groups
//! statistics pruned, the bytes an early stop left unread — as the plan
//! node's metrics under `EXPLAIN ANALYZE`.
//!
//! **Over the statistics fixture gathered at a small group size**, so
//! `public.ordered`'s ascending ids span many groups and a range filter on
//! them both prunes groups and stops early in the group it keeps, as the
//! library's own `tests/pruning.rs` has it.

use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arrow::util::pretty::pretty_format_batches;
use datafusion::physical_plan::metrics::MetricValue;
use datafusion::physical_plan::{ExecutionPlan, collect};
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion_pgdump::{PgDump, PgDumpOptions, ScanBudget, register_dump};
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
/// and sentence. Every other finding is registration's, and dropped.
#[derive(Default)]
struct Plans(Mutex<Vec<(Severity, String, PlanNote)>>);

impl DiagnosticSink for Plans {
    fn report(&self, finding: &dyn Finding) {
        if let Some(note) = finding.as_any().downcast_ref::<PlanNote>() {
            let heard = (finding.severity(), finding.message(), note.clone());
            self.0.lock().unwrap().push(heard);
        }
    }
}

impl Plans {
    fn take(&self) -> Vec<(Severity, String, PlanNote)> {
        std::mem::take(&mut self.0.lock().unwrap())
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
