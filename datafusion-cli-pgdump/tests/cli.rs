//! The binary as a user runs it: `--dump`, `STORED AS PGDUMP` and what reaches
//! stderr (`docs/design/roadmap-P6-datafusion.md`, "The binary:
//! `datafusion-cli-pgdump`").

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use pgdump_query::cache::CacheMode;
use pgdump_query::{LocalFileSource, ScanOptions, StatisticsRequest, map_file};

fn fixture(schema: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/16").join(schema).join("default.sql")
}

/// `fixture` copied into `dir` beside the complete cache `pgdt parse` leaves.
async fn parsed_copy(fixture: &Path, dir: &Path) -> PathBuf {
    let copy = dir.join(fixture.file_name().unwrap());
    std::fs::copy(fixture, &copy).unwrap();
    let source = LocalFileSource::open(&copy).unwrap();
    let cache = CacheMode::enabled(pgdump_query::cache::colocated_path(&copy));
    map_file(&source, &ScanOptions::default(), &cache, &StatisticsRequest::NONE).await.unwrap();
    copy
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_datafusion-cli-pgdump"))
        .args(["--format", "csv"])
        .args(args)
        .output()
        .unwrap()
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
/// off**; an `Info` never prints.
#[tokio::test]
async fn registration_warnings_reach_stderr_unless_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let copy = parsed_copy(&fixture("types"), dir.path()).await;
    let dump = format!("shop={}", copy.display());

    let out = run(&["--dump", &dump, "-c", "SELECT 1"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stderr = text(&out.stderr);
    let lines: Vec<&str> = stderr.lines().collect();
    assert!(!lines.is_empty());
    assert!(lines.iter().all(|line| line.starts_with("warning: shop.")), "{stderr}");
    assert!(lines.iter().any(|line| line.contains("public.mood")), "{stderr}");

    let out = run(&["-q", "--dump", &dump, "-c", "SELECT 1"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stderr), "");
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
                line.starts_with("warning: shop.logs.events: ") && line.contains("floor")
            })
            .count()
    };

    let out = run(&args);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(floor(&text(&out.stderr)), 1, "{}", text(&out.stderr));
    // The provider's clause, printed unchanged: the pool's grant is named by
    // the setting that moves it.
    assert!(
        text(&out.stderr).lines().any(|line| line.contains("floor")
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
    for key in ["pgdump.memory", "pgdump.chunk_size", "pgdump.max_line_bytes"] {
        assert!(shown.contains(key), "{key}: {shown}");
    }
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
