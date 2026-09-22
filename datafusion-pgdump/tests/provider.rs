//! The provider against the library it wraps, over the committed fixtures
//! (`docs/design/roadmap-P6-datafusion.md`, "Verification").
//!
//! **The library's own serial stream is the oracle.** A scan through
//! DataFusion is the library's partitioned replay under DataFusion's
//! scheduler, so what it must return is exactly what `table_stream` returns:
//! the same rows, in the same order once its partitions are read in order, of
//! the same Arrow type — or the same refusal.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::RecordBatch;
use arrow::compute::concat_batches;
use arrow::datatypes::SchemaRef;
use datafusion::physical_plan::{ExecutionPlan, execute_stream_partitioned};
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion_pgdump::{Error, PgDump, PgDumpOptions, ScanBudget, register_dump};
use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    ComparisonSemantics, Finding, LocalFileSource, QueryOptions, ScanOptions, SchemaMode,
    StatisticsRequest, TableName, map_file, table_stream,
};

/// A sink for a registration whose findings this target is not about.
fn ignore(_: &dyn Finding) {}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

/// Every generated dump: `fixtures/<major>/<schema>/<flag-set>.sql`.
fn every_fixture() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for major in std::fs::read_dir(fixtures_root()).unwrap() {
        let major = major.unwrap().path();
        if !major.is_dir() {
            continue;
        }
        for schema in std::fs::read_dir(&major).unwrap() {
            let schema = schema.unwrap().path();
            if !schema.is_dir() {
                continue;
            }
            for dump in std::fs::read_dir(&schema).unwrap() {
                let dump = dump.unwrap().path();
                if dump.extension().is_some_and(|e| e == "sql") {
                    found.push(dump);
                }
            }
        }
    }
    found.sort();
    assert!(!found.is_empty(), "no fixtures under {}", fixtures_root().display());
    found
}

/// `fixture` copied into `dir` beside the complete cache a parse leaves, so
/// the committed tree is never written into.
async fn parsed_copy(fixture: &Path, dir: &Path) -> PathBuf {
    // One directory per fixture: every major has a `default.sql`, and a cache
    // left by another's copy describes another file.
    let dir = tempfile::tempdir_in(dir).unwrap().keep();
    let copy = dir.join(fixture.file_name().unwrap());
    std::fs::copy(fixture, &copy).unwrap();
    let source = LocalFileSource::open(&copy).unwrap();
    let cache = CacheMode::enabled(pgdump_query::cache::colocated_path(&copy));
    map_file(&source, &ScanOptions::default(), &cache, &StatisticsRequest::NONE).await.unwrap();
    copy
}

fn session(partitions: usize, batch_size: usize) -> SessionContext {
    SessionContext::new_with_config(
        SessionConfig::new().with_target_partitions(partitions).with_batch_size(batch_size),
    )
}

/// A SQL identifier, quoted.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The catalog each of `dump`'s databases registered under, by
/// [`register_dump`]'s rule, with `dump` standing in where the file names
/// none.
fn register(ctx: &SessionContext, dump: &Arc<PgDump>) -> Vec<(Option<String>, String)> {
    let databases = dump.databases();
    let name = matches!(databases.as_slice(), [None] | []).then_some("dump");
    let catalogs = register_dump(ctx, name, dump, &ignore).unwrap();
    databases.into_iter().zip(catalogs).collect()
}

fn catalog_of(catalogs: &[(Option<String>, String)], table: &TableName) -> String {
    catalogs.iter().find(|(database, _)| *database == table.database).unwrap().1.clone()
}

fn select(catalog: &str, table: &TableName, columns: &str) -> String {
    format!(
        "SELECT {columns} FROM {}.{}.{}",
        quoted(catalog),
        quoted(table.schema.as_deref().unwrap_or(datafusion_pgdump::UNQUALIFIED_SCHEMA)),
        quoted(&table.table)
    )
}

/// Every batch of `plan`, partition by partition in partition order.
async fn partition_batches(
    ctx: &SessionContext,
    plan: Arc<dyn ExecutionPlan>,
) -> datafusion::error::Result<Vec<RecordBatch>> {
    let mut batches = Vec::new();
    for mut stream in execute_stream_partitioned(plan, ctx.task_ctx())? {
        while let Some(batch) = stream.next().await {
            batches.push(batch?);
        }
    }
    Ok(batches)
}

/// What the library's serial stream answers for `table` — every batch, or its
/// refusal's words.
async fn library_answer(
    dump: &Path,
    table: &TableName,
    schema_mode: SchemaMode,
) -> Result<(SchemaRef, Vec<RecordBatch>), String> {
    let source = LocalFileSource::open(dump).unwrap();
    let options = QueryOptions {
        database: table.database.clone(),
        schema_mode,
        semantics: ComparisonSemantics::Arrow,
        ..QueryOptions::default()
    };
    let mut stream = table_stream(
        &source,
        &table.qualified(),
        ScanOptions::default(),
        options,
        None,
        CacheMode::DISABLED,
    );
    let mut batches = Vec::new();
    while let Some(batch) = stream.next().await {
        batches.push(batch.map_err(|e| e.to_string())?);
    }
    Ok((stream.resolved_schema().schema, batches))
}

/// **Every fixture table's `SELECT *` through the provider is the library's
/// own stream, value and type**, typed and as text, cut into several
/// partitions and small batches so the cuts land inside blocks.
#[tokio::test(flavor = "multi_thread")]
async fn every_fixture_table_reads_as_the_library_reads_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut tables_compared = 0;
    for fixture in every_fixture() {
        let copy = parsed_copy(&fixture, dir.path()).await;
        for schema_mode in [SchemaMode::Typed, SchemaMode::Strings] {
            let options = PgDumpOptions { schema_mode, ..PgDumpOptions::default() };
            let dump = PgDump::open(copy.to_str().unwrap(), options).await.unwrap();
            let ctx = session(4, 3);
            let catalogs = register(&ctx, &dump);
            for table in dump.tables() {
                let what =
                    format!("{} in {} ({schema_mode:?})", table.qualified(), fixture.display());
                let expected = library_answer(&copy, table, schema_mode).await;
                let query = select(&catalog_of(&catalogs, table), table, "*");
                let plan = ctx.sql(&query).await.unwrap().create_physical_plan().await.unwrap();
                let got = partition_batches(&ctx, Arc::clone(&plan)).await;
                match (expected, got) {
                    (Ok((schema, expected)), Ok(got)) => {
                        assert_eq!(plan.schema(), schema, "{what}: schema");
                        let expected = concat_batches(&schema, &expected).unwrap();
                        let got = concat_batches(&schema, &got).unwrap();
                        assert_eq!(got, expected, "{what}: rows");
                    }
                    (Err(expected), Err(got)) => {
                        assert!(got.to_string().contains(&expected), "{what}: {got} / {expected}");
                    }
                    (expected, got) => panic!(
                        "{what}: the library answered {:?} and the provider {:?}",
                        expected.map(|(_, b)| b.len()),
                        got.map(|b| b.len())
                    ),
                }
                tables_compared += 1;
            }
        }
    }
    assert!(tables_compared > 0);
}

async fn fixture_dump(schema: &str, flag_set: &str, dir: &Path) -> (PathBuf, Arc<PgDump>) {
    let fixture = fixtures_root().join("16").join(schema).join(format!("{flag_set}.sql"));
    let copy = parsed_copy(&fixture, dir).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    (copy, dump)
}

/// **A catalog per database, named after it, and a name only where the file
/// names none** (`docs/design/roadmap-P6-datafusion.md`, "How a dump appears
/// in SQL").
#[tokio::test(flavor = "multi_thread")]
async fn a_dump_registers_one_catalog_per_database_named_as_the_file_names_it() {
    let dir = tempfile::tempdir().unwrap();

    // A plain dump names no database: a name is required.
    let (_, plain) = fixture_dump("edge_cases", "default", dir.path()).await;
    let refused = register_dump(&session(1, 8192), None, &plain, &ignore).unwrap_err();
    assert!(matches!(refused, Error::CatalogName(_)), "{refused}");
    let ctx = session(1, 8192);
    assert_eq!(register_dump(&ctx, Some("shop"), &plain, &ignore).unwrap(), ["shop"]);
    let catalog = ctx.catalog("shop").unwrap();
    assert!(catalog.schema_names().contains(&"public".to_string()));
    assert!(catalog.schema("logs").unwrap().table_names().contains(&"events".to_string()));

    // A file of several databases takes their names, and refuses one given.
    let dir = tempfile::tempdir().unwrap();
    let (_, all) = fixture_dump("edge_cases", "dumpall", dir.path()).await;
    let refused = register_dump(&session(1, 8192), Some("shop"), &all, &ignore).unwrap_err();
    assert!(matches!(refused, Error::CatalogName(_)), "{refused}");
    let ctx = session(1, 8192);
    let names = register_dump(&ctx, None, &all, &ignore).unwrap();
    assert!(names.contains(&"pgdt_fixture".to_string()), "{names:?}");
    assert!(names.contains(&"pgdt_tenant".to_string()), "{names:?}");
    let tenant = ctx.catalog("pgdt_tenant").unwrap();
    assert!(tenant.schema("tenant").unwrap().table_exist("ledger"));
    let rows = ctx
        .sql("SELECT count(*) AS n FROM pgdt_tenant.tenant.ledger")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows[0].num_rows(), 1);
}

/// **A missing or partial cache is an error naming the parse that builds
/// it**; the provider never maps.
#[tokio::test(flavor = "multi_thread")]
async fn only_a_complete_cache_opens() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = fixtures_root().join("16/edge_cases/default.sql");
    let copy = dir.path().join("default.sql");
    std::fs::copy(&fixture, &copy).unwrap();
    let location = copy.to_str().unwrap();

    let missing = PgDump::open(location, PgDumpOptions::default()).await.unwrap_err();
    assert!(matches!(missing, Error::CacheNotComplete { .. }), "{missing}");
    assert!(missing.to_string().contains(&format!("pgdt parse --source {location}")), "{missing}");

    // A preamble-only scan leaves a cache that stops short of the end.
    let source = LocalFileSource::open(&copy).unwrap();
    let cache = CacheMode::enabled(pgdump_query::cache::colocated_path(&copy));
    pgdump_query::preamble_only(&source, &ScanOptions::default(), &cache).await.unwrap();
    let partial = PgDump::open(location, PgDumpOptions::default()).await.unwrap_err();
    assert!(partial.to_string().contains("covers"), "{partial}");
    let before = std::fs::read(pgdump_query::cache::colocated_path(&copy)).unwrap();

    // A named cache path is the one the remedy names.
    let elsewhere = dir.path().join("elsewhere.dtcache");
    let options = PgDumpOptions { cache_path: Some(elsewhere.clone()), ..PgDumpOptions::default() };
    let named = PgDump::open(location, options).await.unwrap_err();
    assert!(named.to_string().contains(&format!("--dtcache {}", elsewhere.display())), "{named}");

    // Nothing was written by the refusals.
    assert_eq!(std::fs::read(pgdump_query::cache::colocated_path(&copy)).unwrap(), before);
    assert!(!elsewhere.exists());
}

/// **Projection is the library's, `COUNT(*)` is the empty projection, and a
/// limit stops the plan**, over a table of several blocks read in several
/// partitions.
#[tokio::test(flavor = "multi_thread")]
async fn projection_count_and_limit_reach_the_scan() {
    let dir = tempfile::tempdir().unwrap();
    let (copy, dump) = fixture_dump("partitions", "load-via-partition-root", dir.path()).await;
    let ctx = session(4, 2);
    let catalogs = register(&ctx, &dump);
    for table in dump.tables() {
        let catalog = catalog_of(&catalogs, table);
        let Ok((schema, expected)) = library_answer(&copy, table, SchemaMode::Typed).await else {
            continue;
        };
        let rows: usize = expected.iter().map(RecordBatch::num_rows).sum();

        let count = ctx
            .sql(&select(&catalog, table, "count(*) AS n"))
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        let n = count[0].column(0).as_any().downcast_ref::<arrow::array::Int64Array>().unwrap();
        assert_eq!(n.value(0) as usize, rows, "{}", table.qualified());

        let limited = ctx
            .sql(&format!("{} LIMIT 1", select(&catalog, table, "*")))
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        assert_eq!(
            limited.iter().map(RecordBatch::num_rows).sum::<usize>(),
            rows.min(1),
            "{}",
            table.qualified()
        );

        // The last column alone, and the plan reads nothing else.
        let Some(last) = schema.fields().last() else { continue };
        let query = select(&catalog, table, &quoted(last.name()));
        let plan = ctx.sql(&query).await.unwrap().create_physical_plan().await.unwrap();
        assert_eq!(plan.schema().fields().len(), 1);
        let got = partition_batches(&ctx, plan).await.unwrap();
        assert!(got.iter().all(|b| b.num_rows() <= 2), "batch size is the session's");
        let expected = concat_batches(&schema, &expected).unwrap();
        let expected = expected.project(&[schema.fields().len() - 1]).unwrap();
        let got = concat_batches(&expected.schema(), &got).unwrap();
        assert_eq!(got, expected, "{}", table.qualified());
    }
}

/// **One budget per session, drawn by every scan alive and returned when its
/// plan is gone** (`docs/design/roadmap-P6-datafusion.md`, "Workers and
/// memory").
#[tokio::test(flavor = "multi_thread")]
async fn scans_draw_on_the_session_budget_and_return_it() {
    let dir = tempfile::tempdir().unwrap();
    let (_, dump) = fixture_dump("edge_cases", "default", dir.path()).await;
    let budget = Arc::new(ScanBudget::new(4 << 30));
    let ctx = SessionContext::new_with_config(
        SessionConfig::new().with_target_partitions(4).with_extension(Arc::clone(&budget)),
    );
    register_dump(&ctx, Some("shop"), &dump, &ignore).unwrap();
    let query = "SELECT * FROM shop.public.widgets";

    let first = ctx.sql(query).await.unwrap().create_physical_plan().await.unwrap();
    let one = budget.drawn();
    assert!(one > 0, "a planned scan holds part of the budget");
    let second = ctx.sql(query).await.unwrap().create_physical_plan().await.unwrap();
    assert!(budget.drawn() > one, "a second scan draws beside the first");
    assert!(budget.drawn() <= 4 << 30);

    let rows = partition_batches(&ctx, Arc::clone(&first)).await.unwrap();
    assert!(!rows.is_empty());
    drop(first);
    drop(second);
    assert_eq!(budget.drawn(), 0, "every draw is returned once its plan is dropped");

    // The session's own, not a process-wide one.
    let other = session(4, 8192);
    register_dump(&other, Some("shop"), &dump, &ignore).unwrap();
    let plan = other.sql(query).await.unwrap().create_physical_plan().await.unwrap();
    assert_eq!(budget.drawn(), 0);
    drop(plan);
}

/// **What the session holds besides its scans is billed to its budget**: each
/// registered dump's resident statistics, once per budget and returned when the
/// dump is dropped, and a finite memory pool's limit read at every draw. Both
/// come off the margin's ceiling, and what its room cannot absorb at the
/// count a scan resolves comes off that scan's budget
/// (`docs/design/roadmap-P6-datafusion.md`, "Workers and memory"): a plain
/// source, which recommends no per-reader memory and so keeps its count,
/// draws only what they leave under the ceiling, and still reads. What they
/// do to a source that recommends one is `budget.rs`'s unit tests.
#[tokio::test(flavor = "multi_thread")]
async fn the_resident_statistics_are_billed_and_lower_a_budget_the_margin_cannot_hold() {
    use datafusion::execution::runtime_env::RuntimeEnvBuilder;
    use pgdump_query::{
        DEFAULT_MEMORY_BUDGET, MEMORY_MARGIN_PERCENT, MEMORY_RESERVE, MEMORY_UNPOOLED_BOUND,
    };

    let dir = tempfile::tempdir().unwrap();
    let fixture = fixtures_root().join("16/edge_cases/default.sql");
    let dir = tempfile::tempdir_in(dir.path()).unwrap().keep();
    let copy = dir.join("default.sql");
    std::fs::copy(&fixture, &copy).unwrap();
    let source = LocalFileSource::open(&copy).unwrap();
    let cache = CacheMode::enabled(pgdump_query::cache::colocated_path(&copy));
    map_file(&source, &ScanOptions::default(), &cache, &StatisticsRequest::ALL).await.unwrap();
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let statistics = dump.statistics_bytes();
    assert!(statistics > 0, "a parse gathering statistics leaves some resident");

    // An allowance whose cap past the reserve holds the plain scan's
    // default, and whose margin, once the statistics and the pool are off
    // it, does not.
    let pool = 128u64 << 20;
    let allowance = MEMORY_RESERVE + statistics + pool;
    assert!(allowance - MEMORY_RESERVE > DEFAULT_MEMORY_BUDGET);
    let ceiling = (allowance / 100 * (100 - MEMORY_MARGIN_PERCENT))
        .saturating_sub(MEMORY_UNPOOLED_BOUND)
        .saturating_sub(statistics + pool);
    assert!(0 < ceiling && ceiling < DEFAULT_MEMORY_BUDGET);
    let query = "SELECT * FROM shop.public.widgets";

    let budget = Arc::new(ScanBudget::new(allowance));
    let config = SessionConfig::new().with_target_partitions(4).with_extension(Arc::clone(&budget));
    let runtime =
        RuntimeEnvBuilder::new().with_memory_limit(pool as usize, 1.0).build_arc().unwrap();
    let ctx = SessionContext::new_with_config_rt(config, runtime);
    register_dump(&ctx, Some("shop"), &dump, &ignore).unwrap();
    assert_eq!(budget.resident(), statistics, "registration bills the resident statistics");
    register_dump(&ctx, Some("again"), &dump, &ignore).unwrap();
    assert_eq!(budget.resident(), statistics, "once per budget, however often registered");
    let plan = ctx.sql(query).await.unwrap().create_physical_plan().await.unwrap();
    assert_eq!(budget.drawn(), ceiling, "what the margin cannot hold comes off the budget");
    let rows = partition_batches(&ctx, Arc::clone(&plan)).await.unwrap();
    assert!(rows.iter().map(RecordBatch::num_rows).sum::<usize>() > 0);
    drop(plan);

    // A table registered by hand is billed at its first scan.
    let by_hand = Arc::new(ScanBudget::new(allowance));
    let ctx_by_hand =
        SessionContext::new_with_config(SessionConfig::new().with_extension(Arc::clone(&by_hand)));
    ctx_by_hand.register_table("w", dump.table(None, None, "widgets").unwrap()).unwrap();
    assert_eq!(by_hand.resident(), 0);
    let plan =
        ctx_by_hand.sql("SELECT * FROM w").await.unwrap().create_physical_plan().await.unwrap();
    assert_eq!(by_hand.resident(), statistics);
    drop(plan);

    // Returned when the dump is dropped, and not before.
    drop(ctx);
    assert_eq!(budget.resident(), statistics, "the dump is still alive");
    drop((ctx_by_hand, dump));
    for budget in [budget, by_hand] {
        assert_eq!(budget.resident(), 0);
        assert_eq!(budget.drawn(), 0);
    }
}

/// **One table on its own is the provider its catalog hands out**, and a name
/// matching tables in several databases is refused rather than picked.
#[tokio::test(flavor = "multi_thread")]
async fn a_single_table_registers_on_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let (_, all) = fixture_dump("edge_cases", "dumpall", dir.path()).await;
    let refused = all.table(None, Some("public"), "widgets").unwrap_err();
    assert!(matches!(refused, Error::Table(_)), "{refused}");
    assert!(refused.to_string().contains("pgdt_tenant"), "{refused}");

    let widgets = all.table(Some("pgdt_tenant"), None, "widgets").unwrap();
    // One partition, so `collect` answers in file order both times.
    let ctx = session(1, 8192);
    ctx.register_table("w", widgets).unwrap();
    let alone = ctx.sql("SELECT * FROM w").await.unwrap().collect().await.unwrap();
    register_dump(&ctx, None, &all, &ignore).unwrap();
    let catalogued =
        ctx.sql("SELECT * FROM pgdt_tenant.public.widgets").await.unwrap().collect().await.unwrap();
    let schema = alone[0].schema();
    assert_eq!(
        concat_batches(&schema, &alone).unwrap(),
        concat_batches(&schema, &catalogued).unwrap()
    );
}
