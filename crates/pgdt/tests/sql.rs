//! `pgdt sql` as a user runs it: `--source`, `STORED AS PGDUMP` and what
//! reaches stderr.

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Output;

use pgdt_codec::compare::{ComparisonDivergence, ComparisonSemantics};
use std::sync::Arc;

use pgdt_dump::cache::{self, CacheMode};
use pgdt_io::{ByteRangeSource, LocalFileSource};
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_query::{Dump, QueryOptions};

fn fixture(schema: &str) -> PathBuf {
    common::fixture(16, schema, "default")
}

/// `pgdt sql`, its rows as CSV.
fn run(args: &[&str]) -> Output {
    common::pgdt().args(["sql", "--format", "csv"]).args(args).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// **`--source NAME=SOURCE` is a catalog, and `STORED AS PGDUMP` a table**, both
/// answering SQL.
#[tokio::test]
async fn a_dump_answers_through_both_registrations() {
    let dir = tempfile::tempdir().unwrap();
    let copy =
        common::parsed_copy(&fixture("edge_cases"), dir.path(), &StatisticsRequest::DATA).await;
    common::ungathered(&copy).await;
    let dump = copy.to_str().unwrap();

    let out = run(&[
        "-q",
        "--source",
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

/// How many columns, in how many tables, a registration of `dump` compares
/// bytewise for want of a recorded collation, counted off the library's own
/// per-column divergences rather than the provider's finding.
async fn uncollated(dump: &Path) -> (usize, usize) {
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(dump).unwrap());
    let mode = CacheMode::enabled(cache::colocated_path(dump));
    let dump = Dump::opened(mode.open_source(source).await.unwrap()).await.unwrap();
    let options =
        QueryOptions { semantics: ComparisonSemantics::DataFusion, ..QueryOptions::default() };
    let (mut columns, mut tables) = (0, 0);
    for name in dump.tables() {
        let table = dump.table(&name.into()).unwrap().unwrap();
        let Ok(divergences) = table.divergences(&options) else { continue };
        let named: BTreeSet<String> = divergences
            .into_iter()
            .filter(|note| note.divergence == ComparisonDivergence::UnknownCollation)
            .map(|note| note.column)
            .collect();
        columns += named.len();
        tables += usize::from(!named.is_empty());
    }
    (columns, tables)
}

/// **Registration's warnings reach stderr, named, and `--quiet` keeps them
/// off**; an `Info` never prints. The columns whose collation the dump does
/// not record are one line, naming the dump, after the rest, counting them
/// and the tables apart.
#[tokio::test]
async fn registration_warnings_reach_stderr_unless_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let copy = common::parsed_copy(&fixture("types"), dir.path(), &StatisticsRequest::DATA).await;
    common::ungathered(&copy).await;
    let dump = format!("shop={}", copy.display());

    let out = run(&["--source", &dump, "-c", "SELECT 1"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stderr = text(&out.stderr);
    let lines: Vec<&str> = stderr.lines().collect();
    let (folded, columns) = lines.split_last().unwrap();
    assert!(!columns.is_empty());
    assert!(columns.iter().all(|line| line.starts_with("warning: shop.")), "{stderr}");
    assert!(columns.iter().any(|line| line.contains("public.mood")), "{stderr}");
    let (columns, tables) = uncollated(&copy).await;
    assert!(tables > 1, "the fixture's uncollated columns span tables");
    let counted = format!(
        "warning: {}: {columns} column(s) in {tables} table(s) are each compared bytewise: ",
        copy.display()
    );
    assert!(folded.starts_with(&counted), "{counted}\n{stderr}");
    assert_eq!(lines.iter().filter(|line| line.contains("no COLLATE clause")).count(), 1);

    let out = run(&["-q", "--source", &dump, "-c", "SELECT 1"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stderr), "");
}

/// **A `STORED AS PGDUMP` statement's uncollated columns are one line**,
/// naming the table as the statement does.
#[tokio::test]
async fn a_statement_s_uncollated_columns_are_one_line() {
    let dir = tempfile::tempdir().unwrap();
    let copy =
        common::parsed_copy(&fixture("edge_cases"), dir.path(), &StatisticsRequest::DATA).await;
    common::ungathered(&copy).await;
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
    let copy =
        common::parsed_copy(&fixture("edge_cases"), dir.path(), &StatisticsRequest::DATA).await;
    common::ungathered(&copy).await;
    let dump = format!("shop={}", copy.display());
    let args =
        ["--memory-limit", "1024T", "--source", &dump, "-c", "SELECT * FROM shop.logs.events"];
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
    let copy =
        common::parsed_copy(&fixture("edge_cases"), dir.path(), &StatisticsRequest::DATA).await;
    common::ungathered(&copy).await;
    let dump = format!("shop={}", copy.display());

    let out = run(&[
        "--source",
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
/// states it**, and moves under `SET` and `RESET` as the manual says
/// DataFusion 55.1 moves it: `SET` of it or of
/// `enable_dynamic_filter_pushdown` turns it on, `RESET` of it restores
/// DataFusion's default, on, and `RESET` of `enable_dynamic_filter_pushdown`
/// leaves it as it is.
// upstream: UF1
#[test]
fn the_aggregate_dynamic_filter_is_off_unless_stated() {
    const KEY: &str = "datafusion.optimizer.enable_aggregate_dynamic_filter_pushdown";
    const UMBRELLA: &str = "datafusion.optimizer.enable_dynamic_filter_pushdown";
    let show = format!("SHOW {KEY}");
    let value = |out: Output| {
        assert!(out.status.success(), "{}", text(&out.stderr));
        text(&out.stdout).lines().last().unwrap().to_owned()
    };
    // The option's value once `statements` have run in one shell.
    let after = |statements: &[&str]| {
        let mut args = vec!["-q"];
        for statement in statements {
            args.extend(["-c", statement]);
        }
        args.extend(["-c", &show]);
        value(run(&args))
    };
    let (off, on) = (format!("{KEY},false"), format!("{KEY},true"));

    assert_eq!(value(run(&["-q", "-c", &show])), off);
    assert_eq!(after(&[&format!("SET {KEY} = true")]), on);
    assert_eq!(after(&[&format!("SET {UMBRELLA} = true")]), on);
    assert_eq!(after(&[&format!("RESET {KEY}")]), on);
    assert_eq!(after(&[&format!("RESET {UMBRELLA}")]), off);
    assert_eq!(after(&[&format!("SET {KEY} = true"), &format!("RESET {UMBRELLA}")]), on);
    let stated = common::pgdt()
        .env("DATAFUSION_OPTIMIZER_ENABLE_AGGREGATE_DYNAMIC_FILTER_PUSHDOWN", "true")
        .args(["sql", "--format", "csv", "-q", "-c", &show])
        .output()
        .unwrap();
    assert_eq!(value(stated), format!("{KEY},true"));
}

/// **The shell's session is readied, so built with the guard**:
/// `pgdump_unrepresentable` answers
/// where a scan answers it, and a query DataFusion would have to evaluate it
/// in is refused at planning, by the guard, not when it is evaluated.
#[tokio::test]
async fn the_unrepresentable_function_is_guarded_at_planning() {
    let dir = tempfile::tempdir().unwrap();
    let copy = common::parsed_copy(&fixture("types"), dir.path(), &StatisticsRequest::DATA).await;
    common::ungathered(&copy).await;
    let dump = format!("shop={}", copy.display());

    let out = run(&[
        "-q",
        "--source",
        &dump,
        "-c",
        "SELECT id FROM shop.public.t_date \
         WHERE v_date IS NULL AND NOT pgdump_unrepresentable(v_date)",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout).trim(), "id\n7");

    let out = run(&[
        "-q",
        "--source",
        &dump,
        "-c",
        "SELECT pgdump_unrepresentable(v_date) FROM shop.public.t_date",
    ]);
    let said = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(said.contains("pgdump_unrepresentable_guard"), "{said}");
    assert!(said.contains("DataFusion would have to evaluate it"), "{said}");
    assert!(!said.contains("ready_session"), "{said}");
}

/// **A refusal names its remedy in the grammar that registered its dump,
/// wherever it is printed**: a statement `-f` runs, its refusal printed by
/// DataFusion's CLI itself, is spelled in `--source`'s suffixes for a dump
/// `--source` registered, and in `pgdump.` options for one a statement
/// registered.
#[tokio::test]
async fn a_refusal_is_spelled_in_the_grammar_that_registered_its_dump() {
    let dir = tempfile::tempdir().unwrap();
    let copy = common::parsed_copy(&fixture("types"), dir.path(), &StatisticsRequest::DATA).await;
    common::ungathered(&copy).await;
    let file = |name: &str, sql: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, sql).unwrap();
        path.to_str().unwrap().to_string()
    };

    let select = file("select.sql", "SELECT v_date FROM shop.public.t_date;\n");
    let dump = format!("shop={}:unrepresentable=refuse", copy.display());
    let out = run(&["-q", "--source", &dump, "-f", &select]);
    let said = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(said.contains("(`:unrepresentable=null`; `:unrepresentable=text`)"), "{said}");

    let statement = file(
        "statement.sql",
        &format!(
            "CREATE EXTERNAL TABLE d STORED AS PGDUMP LOCATION '{}' \
             OPTIONS ('pgdump.table' 't_date', 'pgdump.unrepresentable' 'refuse');\n\
             SELECT v_date FROM d;\n",
            copy.display()
        ),
    );
    let out = run(&["-q", "-f", &statement]);
    let said = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(
        said.contains("(`'pgdump.unrepresentable' 'null'`; `'pgdump.unrepresentable' 'text'`)"),
        "{said}"
    );
}

/// **A dump with no complete cache ends the run before any SQL**, spelled
/// with the parse that builds it.
#[tokio::test]
async fn a_dump_with_no_cache_names_the_parse() {
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("bare.sql");
    std::fs::copy(fixture("edge_cases"), &bare).unwrap();
    let out = run(&["-q", "--source", &format!("shop={}", bare.display()), "-c", "SELECT 1"]);
    assert!(!out.status.success());
    let said = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(said.contains(&format!("pgdt parse --source {}", bare.display())), "{said}");
}

/// **`--strict-identity` is `pgdt`'s, and binds every registration stating
/// none of its own**: a dump touched since its parse opens with a warning
/// unless `time` is stated, and then neither `--source` nor `STORED AS PGDUMP`
/// opens it.
#[tokio::test]
async fn strict_identity_binds_both_registrations() {
    let dir = tempfile::tempdir().unwrap();
    let copy =
        common::parsed_copy(&fixture("edge_cases"), dir.path(), &StatisticsRequest::DATA).await;
    common::ungathered(&copy).await;
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    std::fs::File::options().write(true).open(&copy).unwrap().set_modified(future).unwrap();
    let dump = copy.to_str().unwrap();
    let catalog = format!("shop={dump}");

    let out = run(&["-q", "--source", &catalog, "-c", "SELECT 1"]);
    assert!(out.status.success(), "{}", text(&out.stderr));

    let out = run(&["-q", "--strict-identity=time", "--source", &catalog, "-c", "SELECT 1"]);
    assert!(!out.status.success());
    let said = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(said.contains("with `time` bound"), "{said}");

    let create = format!(
        "CREATE EXTERNAL TABLE ev STORED AS PGDUMP LOCATION '{dump}' \
         OPTIONS ('pgdump.table' 'events')"
    );
    let out = run(&["-q", "--strict-identity", "-c", &create]);
    let said = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(said.contains("with `time` bound"), "{said}");
}

/// **A dump's own strictness overrides the session's**, from either front
/// door: `--source …:strict-identity=` and `pgdump.strict_identity` open a
/// touched dump under `none` or `advisory` where the session binds `time`, and
/// refuse it under `time` where the session is advisory.
#[tokio::test]
async fn a_dump_s_own_strictness_overrides_the_session_s() {
    let dir = tempfile::tempdir().unwrap();
    let copy =
        common::parsed_copy(&fixture("edge_cases"), dir.path(), &StatisticsRequest::DATA).await;
    common::ungathered(&copy).await;
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    std::fs::File::options().write(true).open(&copy).unwrap().set_modified(future).unwrap();
    let dump = copy.to_str().unwrap();
    let count = "SELECT count(*) AS n FROM shop.logs.events";
    let refused = |out: &Output| {
        let said = format!("{}{}", text(&out.stdout), text(&out.stderr));
        assert!(said.contains("with `time` bound"), "{said}");
    };

    for loose in ["none", "advisory"] {
        let loose = format!("shop={dump}:strict-identity={loose}");
        let out = run(&["-q", "--strict-identity=time", "--source", &loose, "-c", count]);
        assert!(out.status.success(), "{loose}: {}", text(&out.stderr));
        assert_eq!(text(&out.stdout).trim(), "n\n3");
    }
    let out = run(&["-q", "--source", &format!("shop={dump}:strict-identity=time"), "-c", count]);
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
