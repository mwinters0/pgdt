//! The `pgdump.` session settings (`docs/design/roadmap-P6-datafusion.md`,
//! "Workers and memory"): what `pgdt query` takes as `--memory`,
//! `--chunk-size` and `--max-line-bytes`, set with `SET` and read when a scan
//! is planned.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arrow::array::RecordBatch;
use arrow::util::pretty::pretty_format_batches;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion_pgdump::{
    PgDump, PgDumpOptions, PgDumpSettings, ScanBudget, register_dump, register_table_factory,
};
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    DiagnosticSink, Finding, LocalFileSource, MEMORY_RESERVE, PlanNote, PlanNoteKind, ScanOptions,
    StatisticsRequest, map_file,
};

const EVENTS: &str = "shop.logs.events";

/// The `edge_cases` fixture copied into `dir` beside the complete cache a
/// parse leaves.
async fn parsed_copy(dir: &Path) -> PathBuf {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/16/edge_cases/default.sql");
    let copy = dir.join("default.sql");
    std::fs::copy(fixture, &copy).unwrap();
    let source = LocalFileSource::open(&copy).unwrap();
    let cache = CacheMode::enabled(pgdump_query::cache::colocated_path(&copy));
    map_file(&source, &ScanOptions::default(), &cache, &StatisticsRequest::NONE).await.unwrap();
    copy
}

/// The plan notes a sink heard; every other finding is registration's.
#[derive(Default)]
struct Plans(Mutex<Vec<PlanNote>>);

impl DiagnosticSink for Plans {
    fn report(&self, finding: &dyn Finding) {
        if let Some(note) = finding.as_any().downcast_ref::<PlanNote>() {
            self.0.lock().unwrap().push(note.clone());
        }
    }
}

impl Plans {
    /// Whether a note since the last call says the budget seated no reader.
    fn floored(&self) -> bool {
        std::mem::take(&mut *self.0.lock().unwrap())
            .iter()
            .any(|note| matches!(note.kind, PlanNoteKind::AllocationBelowFloor { .. }))
    }
}

async fn run(ctx: &SessionContext, sql: &str) -> Result<Vec<RecordBatch>, String> {
    match ctx.sql(sql).await {
        Ok(frame) => frame.collect().await.map_err(|e| e.to_string()),
        Err(err) => Err(err.to_string()),
    }
}

async fn plan(ctx: &SessionContext, sql: &str) -> Arc<dyn ExecutionPlan> {
    ctx.sql(sql).await.unwrap().create_physical_plan().await.unwrap()
}

fn printed(batches: &[RecordBatch]) -> String {
    pretty_format_batches(batches).unwrap().to_string()
}

/// A session over the fixture whose budget holds `allowance`, registered.
async fn session(copy: &Path, allowance: u64, plans: &Arc<Plans>) -> SessionContext {
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let budget = Arc::new(ScanBudget::new(allowance));
    let ctx = SessionContext::new_with_config(
        SessionConfig::new().with_target_partitions(4).with_extension(budget),
    );
    register_dump(&ctx, Some("shop"), &dump, Arc::clone(plans) as _).unwrap();
    ctx
}

/// **`SET pgdump.memory` overrides the budget's allowance for every scan
/// planned after it, and none planned before**: under an allowance the
/// reserve takes whole the plan runs at its floor and says so, and a larger
/// one stated seats it; stated small again, the next plan is floored while a
/// plan already made keeps its draw.
#[tokio::test(flavor = "multi_thread")]
async fn a_stated_allowance_binds_the_scans_planned_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let copy = parsed_copy(dir.path()).await;
    let plans = Arc::new(Plans::default());
    let ctx = session(&copy, MEMORY_RESERVE, &plans).await;
    let query = format!("SELECT * FROM {EVENTS}");

    let _floored = plan(&ctx, &query).await;
    assert!(plans.floored(), "the budget's own allowance seats no reader");

    run(&ctx, &format!("SET pgdump.memory = {}", 8u64 << 30)).await.unwrap();
    let seated = plan(&ctx, &query).await;
    assert!(!plans.floored(), "a stated allowance seats one");
    let budget = ctx.state().config().get_extension::<ScanBudget>().unwrap();
    let drawn = budget.drawn();
    assert!(drawn > 0);

    run(&ctx, &format!("SET pgdump.memory = {MEMORY_RESERVE}")).await.unwrap();
    let _floored = plan(&ctx, &query).await;
    assert!(plans.floored(), "the next plan is under the one stated now");
    assert_eq!(budget.drawn(), drawn, "the live plan keeps what it drew");
    drop(seated);
    assert_eq!(budget.drawn(), 0);
}

/// **`SET pgdump.memory = 0` returns to the allowance the budget discovered**,
/// as `target_partitions = 0` returns to the machine's parallelism: a plan
/// seated by a stated allowance is followed by one floored under the budget's
/// own, and the setting reads as unstated again. `RESET` still reaches none
/// of it, which is why `0` is the way back; `pgdt --memory 0` stays refused
/// (`pgdt/tests/parallelism.rs`), absence being how a flag asks.
#[tokio::test(flavor = "multi_thread")]
async fn a_zero_allowance_returns_to_the_discovered_one() {
    let dir = tempfile::tempdir().unwrap();
    let copy = parsed_copy(dir.path()).await;
    let plans = Arc::new(Plans::default());
    let ctx = session(&copy, MEMORY_RESERVE, &plans).await;
    let query = format!("SELECT * FROM {EVENTS}");

    run(&ctx, &format!("SET pgdump.memory = {}", 8u64 << 30)).await.unwrap();
    drop(plan(&ctx, &query).await);
    assert!(!plans.floored(), "a stated allowance seats a reader");

    run(&ctx, "SET pgdump.memory = 0").await.unwrap();
    drop(plan(&ctx, &query).await);
    assert!(plans.floored(), "the budget's own allowance seats none");
    let state = ctx.state();
    let settings = state.config().options().extensions.get::<PgDumpSettings>().unwrap();
    assert_eq!(settings.memory, None, "the allowance is unstated again");

    let reset = run(&ctx, "RESET pgdump.memory").await.unwrap_err();
    assert!(reset.contains("pgdump"), "{reset}");
}

/// **`pgdump.chunk_size` sizes the reads a scan makes and
/// `pgdump.max_line_bytes` bounds the lines it carries across them**, as
/// `--chunk-size` and `--max-line-bytes` do: a line limit shorter than the
/// table's rows binds nothing while every row fits one read, refuses the scan
/// once reads are shorter than a row, and a generous one over those short
/// reads answers what the defaults do.
#[tokio::test(flavor = "multi_thread")]
async fn the_read_settings_reach_the_scan() {
    let dir = tempfile::tempdir().unwrap();
    let copy = parsed_copy(dir.path()).await;
    let plans = Arc::new(Plans::default());
    let ctx = session(&copy, 1 << 30, &plans).await;
    let query = format!("SELECT * FROM {EVENTS} ORDER BY event_id");
    let expected = printed(&run(&ctx, &query).await.unwrap());

    run(&ctx, "SET pgdump.max_line_bytes = 8").await.unwrap();
    assert_eq!(printed(&run(&ctx, &query).await.unwrap()), expected);
    run(&ctx, "SET pgdump.chunk_size = 3").await.unwrap();
    let refused = run(&ctx, &query).await.unwrap_err();
    assert!(refused.contains("exceeds the 8-byte line limit"), "{refused}");
    run(&ctx, "SET pgdump.max_line_bytes = 1048576").await.unwrap();
    assert_eq!(printed(&run(&ctx, &query).await.unwrap()), expected);
}

/// **The settings are the session's, listed by `SHOW ALL`, and a value
/// `pgdt` would refuse is refused by `SET`**, save the `0` that un-states an
/// allowance — as is a key that is none of them. `STORED AS PGDUMP` alone
/// installs them too.
#[tokio::test(flavor = "multi_thread")]
async fn the_settings_are_listed_and_checked() {
    let ctx = SessionContext::new_with_config(SessionConfig::new().with_information_schema(true));
    register_table_factory(&ctx, Arc::new(Plans::default()));
    run(&ctx, "SET pgdump.chunk_size = 65536").await.unwrap();
    let listed = printed(
        &run(&ctx, "SELECT name, value FROM information_schema.df_settings WHERE name LIKE 'pgdump.%' ORDER BY name")
            .await
            .unwrap(),
    );
    for line in ["pgdump.chunk_size", "65536", "pgdump.max_line_bytes", "pgdump.memory"] {
        assert!(listed.contains(line), "{listed}");
    }
    for bad in [
        "SET pgdump.chunk_size = 0",
        "SET pgdump.max_line_bytes = 0",
        "SET pgdump.memory = '4G'",
        "SET pgdump.jobs = 4",
    ] {
        assert!(run(&ctx, bad).await.is_err(), "{bad}");
    }
}
