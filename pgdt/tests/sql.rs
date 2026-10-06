//! `pgdt sql` as a user runs it: `--dump`, `STORED AS PGDUMP` and what
//! reaches stderr.

mod common;

use std::path::{Path, PathBuf};
use std::process::Output;

use pgdump_query::cache::{self, CacheMode, CacheStatus};
use pgdump_query::{
    DataBlock, LocalFileSource, ScanOptions, SpanBody, StatisticsRequest, map_file,
};

fn fixture(schema: &str) -> PathBuf {
    common::fixture(&format!("16/{schema}/default.sql"))
}

/// `fixture` copied into `dir` beside a complete cache holding no statistics
/// ([`map_ungathered`]).
async fn parsed_copy(fixture: &Path, dir: &Path) -> PathBuf {
    let copy = dir.join(fixture.file_name().unwrap());
    std::fs::copy(fixture, &copy).unwrap();
    let source = LocalFileSource::open(&copy).unwrap();
    map_ungathered(&source, &cache::colocated_path(&copy)).await;
    copy
}

/// `pgdt sql`, its rows as CSV.
fn run(args: &[&str]) -> Output {
    common::pgdt().args(["sql", "--format", "csv"]).args(args).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// **`--dump NAME=SOURCE` is a catalog, and `STORED AS PGDUMP` a table**, both
/// answering SQL.
#[tokio::test]
async fn a_dump_answers_through_both_registrations() {
    let dir = tempfile::tempdir().unwrap();
    let copy = parsed_copy(&fixture("edge_cases"), dir.path()).await;
    let dump = copy.to_str().unwrap();

    let out = run(&[
        "-q",
        "--dump",
        &format!("shop={dump}"),
        "-c",
        "SELECT count(*) AS n FROM shop.logs.events",
    ]);
    assert!(out.status.success(), "{}{}", text(&out.stdout), text(&out.stderr));
    assert_eq!(text(&out.stdout).trim(), "n\n3");

    let out = run(&[
        "-q",
        "-c",
        &format!(
            "CREATE EXTERNAL TABLE ev STORED AS PGDUMP LOCATION '{dump}' \
             OPTIONS ('pgdump.table' 'events')"
        ),
        "SELECT max(event_id) AS m FROM ev",
    ]);
    assert!(out.status.success(), "{}{}", text(&out.stdout), text(&out.stderr));
    assert!(text(&out.stdout).trim().ends_with("m\n102"), "{}", text(&out.stdout));
}

/// **Registration's warnings reach stderr, named, and `--quiet` keeps them
/// off**; an `Info` never prints. The columns whose collation the dump does
/// not record are one line, naming the dump, after the rest.
#[tokio::test]
async fn registration_warnings_reach_stderr_unless_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let copy = parsed_copy(&fixture("types"), dir.path()).await;
    let dump = format!("shop={}", copy.display());

    let out = run(&["--dump", &dump, "-c", "SELECT 1"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stderr = text(&out.stderr);
    let lines: Vec<&str> = stderr.lines().collect();
    let (folded, columns) = lines.split_last().unwrap();
    assert!(!columns.is_empty());
    assert!(columns.iter().all(|line| line.starts_with("warning: shop.")), "{stderr}");
    assert!(columns.iter().any(|line| line.contains("public.mood")), "{stderr}");
    assert!(folded.starts_with(&format!("warning: {}: ", copy.display())), "{stderr}");
    assert!(folded.contains(" table(s) are each compared bytewise: "), "{stderr}");
    assert_eq!(lines.iter().filter(|line| line.contains("no COLLATE clause")).count(), 1);

    let out = run(&["-q", "--dump", &dump, "-c", "SELECT 1"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stderr), "");
}

/// **A `STORED AS PGDUMP` statement's uncollated columns are one line**,
/// naming the table as the statement does.
#[tokio::test]
async fn a_statement_s_uncollated_columns_are_one_line() {
    let dir = tempfile::tempdir().unwrap();
    let copy = parsed_copy(&fixture("edge_cases"), dir.path()).await;
    let out = run(&[
        "-c",
        &format!(
            "CREATE EXTERNAL TABLE ev STORED AS PGDUMP LOCATION '{}' \
             OPTIONS ('pgdump.table' 'events')",
            copy.display()
        ),
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stderr = text(&out.stderr);
    let collation: Vec<&str> =
        stderr.lines().filter(|line| line.contains("no COLLATE clause")).collect();
    assert_eq!(collation.len(), 1, "{stderr}");
    assert!(collation[0].starts_with("warning: ev: 1 column(s) in 1 table(s) "), "{stderr}");
}

/// **A scan's warnings reach stderr when it is planned, named**, and
/// `--quiet` keeps them off: a `--memory-limit` beyond any allowance a host
/// reports leaves the scans nothing, and the plan says it runs at its floor —
/// and what that budget was carved from, the pool's grant among it.
#[tokio::test]
async fn a_scan_s_warnings_reach_stderr_when_it_is_planned() {
    let dir = tempfile::tempdir().unwrap();
    let copy = parsed_copy(&fixture("edge_cases"), dir.path()).await;
    let dump = format!("shop={}", copy.display());
    let args = ["--memory-limit", "1024T", "--dump", &dump, "-c", "SELECT * FROM shop.logs.events"];
    let floor = |stderr: &str| {
        stderr
            .lines()
            .filter(|line| {
                line.starts_with("warning: shop.logs.events: ") && line.contains("one-slot floor")
            })
            .count()
    };

    let out = run(&args);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(floor(&text(&out.stderr)), 1, "{}", text(&out.stderr));
    // The provider's clause, printed unchanged: the pool's grant is named by
    // the setting that moves it.
    assert!(
        text(&out.stderr).lines().any(|line| line.contains("one-slot floor")
            && line.contains(&format!("pool is granted {} byte(s)", 1024u64 << 40))
            && line.contains("datafusion.runtime.memory_limit")),
        "{}",
        text(&out.stderr)
    );

    let out = run(&[&["-q"], &args[..]].concat());
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stderr), "");
}

/// **The scan settings are `SET` like DataFusion's own and listed beside
/// them**: an allowance stated too small for one reader floors the next scan,
/// whatever the host reports and saying the setting stated it, and `SHOW ALL`
/// names every `pgdump.` key.
#[tokio::test]
async fn a_scan_setting_is_set_by_sql() {
    let dir = tempfile::tempdir().unwrap();
    let copy = parsed_copy(&fixture("edge_cases"), dir.path()).await;
    let dump = format!("shop={}", copy.display());

    let out = run(&[
        "--dump",
        &dump,
        "-c",
        "SET pgdump.memory = 1",
        "SELECT count(*) AS n FROM (SELECT * FROM shop.logs.events)",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("n\n3\n"), "{}", text(&out.stdout));
    let stderr = text(&out.stderr);
    assert!(
        stderr.lines().any(|line| {
            line.starts_with("warning: shop.logs.events: ")
                && line.contains("floor")
                && line.contains("an allowance of 1 resident byte(s), stated by pgdump.memory")
        }),
        "{stderr}"
    );

    let out = run(&["-q", "-c", "SHOW ALL"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let shown = text(&out.stdout);
    for key in [
        "pgdump.memory",
        "pgdump.chunk_size",
        "pgdump.max_line_bytes",
        "pgdump.dynamic_filter_rows",
    ] {
        assert!(shown.contains(key), "{key}: {shown}");
    }
}

/// **An ungrouped aggregate's dynamic filter is off unless the environment
/// states it**, and `SET` turns it on like any other setting.
// upstream: UF1
#[test]
fn the_aggregate_dynamic_filter_is_off_unless_stated() {
    const KEY: &str = "datafusion.optimizer.enable_aggregate_dynamic_filter_pushdown";
    let show = format!("SHOW {KEY}");
    let value = |out: Output| {
        assert!(out.status.success(), "{}", text(&out.stderr));
        text(&out.stdout).lines().last().unwrap().to_owned()
    };

    assert_eq!(value(run(&["-q", "-c", &show])), format!("{KEY},false"));
    assert_eq!(
        value(run(&["-q", "-c", &format!("SET {KEY} = true"), "-c", &show])),
        format!("{KEY},true")
    );
    let stated = common::pgdt()
        .env("DATAFUSION_OPTIMIZER_ENABLE_AGGREGATE_DYNAMIC_FILTER_PUSHDOWN", "true")
        .args(["sql", "--format", "csv", "-q", "-c", &show])
        .output()
        .unwrap();
    assert_eq!(value(stated), format!("{KEY},true"));
}

/// **The shell is built with the guard**: `pgdump_unrepresentable` answers
/// where a scan answers it, and a query DataFusion would have to evaluate it
/// in is refused at planning, by the guard, not when it is evaluated.
#[tokio::test]
async fn the_unrepresentable_function_is_guarded_at_planning() {
    let dir = tempfile::tempdir().unwrap();
    let copy = parsed_copy(&fixture("types"), dir.path()).await;
    let dump = format!("shop={}", copy.display());

    let out = run(&[
        "-q",
        "--dump",
        &dump,
        "-c",
        "SELECT id FROM shop.public.t_date \
         WHERE v_date IS NULL AND NOT pgdump_unrepresentable(v_date)",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout).trim(), "id\n7");

    let out = run(&[
        "-q",
        "--dump",
        &dump,
        "-c",
        "SELECT pgdump_unrepresentable(v_date) FROM shop.public.t_date",
    ]);
    let said = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(said.contains("pgdump_unrepresentable_guard"), "{said}");
    assert!(said.contains("DataFusion would have to evaluate it"), "{said}");
    assert!(!said.contains("with_unrepresentable_guard"), "{said}");
}

/// **A dump with no complete cache ends the run before any SQL**, naming the
/// parse that builds it.
#[tokio::test]
async fn a_dump_with_no_cache_names_the_parse() {
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("bare.sql");
    std::fs::copy(fixture("edge_cases"), &bare).unwrap();
    let out = run(&["-q", "--dump", &format!("shop={}", bare.display()), "-c", "SELECT 1"]);
    assert!(!out.status.success());
    let said = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(said.contains(&format!("pgdt parse --source {}", bare.display())), "{said}");
}

/// **`--strict-identity` is `pgdt`'s, and binds every registration stating
/// none of its own**: a dump touched since its parse opens with a warning
/// unless `time` is stated, and then neither `--dump` nor `STORED AS PGDUMP`
/// opens it.
#[tokio::test]
async fn strict_identity_binds_both_registrations() {
    let dir = tempfile::tempdir().unwrap();
    let copy = parsed_copy(&fixture("edge_cases"), dir.path()).await;
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    std::fs::File::options().write(true).open(&copy).unwrap().set_modified(future).unwrap();
    let dump = copy.to_str().unwrap();
    let catalog = format!("shop={dump}");

    let out = run(&["-q", "--dump", &catalog, "-c", "SELECT 1"]);
    assert!(out.status.success(), "{}", text(&out.stderr));

    let out = run(&["-q", "--strict-identity=time", "--dump", &catalog, "-c", "SELECT 1"]);
    assert!(!out.status.success());
    let said = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(said.contains("`--strict-identity=time`"), "{said}");

    let create = format!(
        "CREATE EXTERNAL TABLE ev STORED AS PGDUMP LOCATION '{dump}' \
         OPTIONS ('pgdump.table' 'events')"
    );
    let out = run(&["-q", "--strict-identity", "-c", &create]);
    let said = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(said.contains("`--strict-identity=time`"), "{said}");
}

/// **A dump's own strictness overrides the session's**, from either front
/// door: `--dump …:strict-identity=` and `pgdump.strict_identity` open a
/// touched dump under `none` or `advisory` where the session binds `time`, and
/// refuse it under `time` where the session is advisory.
#[tokio::test]
async fn a_dump_s_own_strictness_overrides_the_session_s() {
    let dir = tempfile::tempdir().unwrap();
    let copy = parsed_copy(&fixture("edge_cases"), dir.path()).await;
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    std::fs::File::options().write(true).open(&copy).unwrap().set_modified(future).unwrap();
    let dump = copy.to_str().unwrap();
    let count = "SELECT count(*) AS n FROM shop.logs.events";
    let refused = |out: &Output| {
        let said = format!("{}{}", text(&out.stdout), text(&out.stderr));
        assert!(said.contains("`--strict-identity=time`"), "{said}");
    };

    for loose in ["none", "advisory"] {
        let loose = format!("shop={dump}:strict-identity={loose}");
        let out = run(&["-q", "--strict-identity=time", "--dump", &loose, "-c", count]);
        assert!(out.status.success(), "{loose}: {}", text(&out.stderr));
        assert_eq!(text(&out.stdout).trim(), "n\n3");
    }
    let out = run(&["-q", "--dump", &format!("shop={dump}:strict-identity=time"), "-c", count]);
    assert!(!out.status.success());
    refused(&out);

    let create = |strict: &str| {
        format!(
            "CREATE EXTERNAL TABLE ev STORED AS PGDUMP LOCATION '{dump}' \
             OPTIONS ('pgdump.table' 'events', 'pgdump.strict_identity' '{strict}')"
        )
    };
    let out = run(&[
        "-q",
        "--strict-identity",
        "-c",
        &create("none"),
        "-c",
        "SELECT count(*) AS n FROM ev",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("n\n3"), "{}", text(&out.stdout));
    let out = run(&[
        "-q",
        "--strict-identity",
        "-c",
        &create("advisory"),
        "-c",
        "SELECT count(*) AS n FROM ev",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("n\n3"), "{}", text(&out.stdout));
    refused(&run(&["-q", "-c", &create("time")]));
}

/// Map `source` whole into the cache at `path` at the data level and keep no
/// statistics: the census a typed plan needs and nothing gathered, as a
/// query's own mapping pass leaves a cache (`docs/design/decisions.md`,
/// "D35").
async fn map_ungathered(source: &LocalFileSource, path: &Path) {
    let cache = CacheMode::enabled(path.to_path_buf());
    map_file(source, &ScanOptions::default(), &cache, &StatisticsRequest::DATA).await.unwrap();
    let CacheStatus::Valid { mut index, .. } = cache::load(path, source).await.unwrap() else {
        panic!("the parse left a complete cache")
    };
    for span in &mut index.spans {
        if let SpanBody::Data(DataBlock::Copy(block)) = &mut span.body {
            block.statistics = None;
        }
    }
    cache::save(path, source, &index).await.unwrap();
}
