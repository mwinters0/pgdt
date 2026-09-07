//! `pgdq parse|query --jobs / --parallel-memory` — the CLI surface a caller
//! states its concurrency and its memory budget in
//! (`docs/design/architecture.md`, "Execution model and API surface").
//!
//! The library holds what the two numbers *mean*: `io.rs`'s pool tests drive
//! the budget's split between a source's two read units, its floor, and the
//! block-decode line it draws. What only the binary can say is that the flags
//! reach `ScanOptions`/`QueryOptions` at all — a flag parsed and dropped on
//! the floor leaves every one of those passing — and that neither number can
//! change an answer.
//!
//! **`.xz` is where the budget actually decides something**, so the parity
//! assertion here runs against a compressed source as well as a plain one: a
//! budget below the file's block unit sends every read through the streaming
//! decoder instead of the block path, and the rows must not notice.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;
use common::{fixture, run, stderr_of, stdout_of};

/// The same hand-written dump `xz_source.rs` builds its fixtures from — no
/// `CREATE TABLE` DDL, so every column resolves `Utf8View` and a byte-for-byte
/// comparison is exactly what parity means here.
fn edge_cases_sql() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../pgdump_query/tests/data/edge_cases.sql")
}

/// A seekable multi-block `.xz` copy of it, in a directory of its own.
/// `xz` is not `mise`-pinned, so a missing binary fails loudly rather than
/// skipping (`docs/design/roadmap.md`, "A test may assume the tools `mise`
/// pins").
fn seekable_xz() -> (tempfile::TempDir, PathBuf) {
    let out = Command::new("xz")
        .args(["--block-size=512", "-c"])
        .arg(edge_cases_sql())
        .output()
        .expect("`xz` is not runnable, so this test cannot build its fixture; install it.");
    assert!(out.status.success(), "xz failed: {}", String::from_utf8_lossy(&out.stderr));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge_cases.sql.xz");
    std::fs::write(&path, &out.stdout).unwrap();
    (dir, path)
}

fn query(dump: &Path, table: &str, extra: &[&str]) -> Output {
    let mut args =
        vec!["query", "--source", dump.to_str().unwrap(), "--table", table, "--dqcache", "none"];
    args.extend_from_slice(extra);
    run(&args)
}

/// Every setting of the two flags answers the same rows on a plain file. The
/// budgets bracket the read chunk: 1 MiB is one chunk buffer's worth, which is
/// the pool's one-slot floor, and 512 MiB is more than anything here can use.
///
/// **The reference is `--jobs 1`, not the default**, since `--jobs` now cuts
/// the replay: an omitted flag is this machine's available parallelism, so a
/// default reference would compare one partitioned run against another and
/// pass however the merge ordered them.
#[test]
fn a_query_reads_the_same_rows_at_any_stated_parallelism() {
    let dump = fixture("16/edge_cases/default.sql");
    let reference = query(&dump, "public.widgets", &["--jobs", "1"]);
    assert!(reference.status.success(), "{}", stderr_of(&reference));
    for extra in [
        vec!["--jobs", "1"],
        vec!["--jobs", "8"],
        vec!["--parallel-memory", "1048576"],
        vec!["--jobs", "8", "--parallel-memory", "536870912"],
        vec!["--jobs", "2", "--parallel-memory", "1048576"],
    ] {
        let out = query(&dump, "public.widgets", &extra);
        assert!(out.status.success(), "{extra:?}: {}", stderr_of(&out));
        assert_eq!(stdout_of(&out), stdout_of(&reference), "{extra:?} changed the rows");
    }
}

/// **The budget decides a compressed source's read path, and never its
/// answer.** Below the file's block unit `XzSource` falls back to the
/// streaming decoder; above it the same read is a slice of a decoded block.
/// Both must produce the plain file's rows byte for byte.
#[test]
fn a_compressed_query_agrees_across_the_budget_that_changes_its_read_path() {
    let (_xz_dir, compressed) = seekable_xz();
    let plain_dir = tempfile::tempdir().unwrap();
    let plain = plain_dir.path().join("edge_cases.sql");
    std::fs::copy(edge_cases_sql(), &plain).unwrap();

    // As above: the serial path is the oracle, named rather than defaulted to.
    let reference = query(&plain, "public.widgets", &["--jobs", "1"]);
    assert!(reference.status.success(), "{}", stderr_of(&reference));
    for extra in [
        vec![],
        // Small enough that a decoded block does not fit beside the chunk
        // pool's own ceiling: the streaming reader, on a file that has
        // boundaries to seek by.
        vec!["--parallel-memory", "65536"],
        vec!["--jobs", "8", "--parallel-memory", "536870912"],
    ] {
        let out = query(&compressed, "public.widgets", &extra);
        assert!(out.status.success(), "{extra:?}: {}", stderr_of(&out));
        assert_eq!(stdout_of(&out), stdout_of(&reference), "{extra:?} changed the rows");
    }
}

/// **The merge prints file order, not arrival order.** `pgdq query` holds one
/// batch per sub-stream and emits the one that begins earliest in the file
/// (`docs/design/architecture.md`, "Partitioned replay"), so the `id` column
/// of a table written 1..5 reads 1..5 at every job count — where a printer
/// that emitted whatever finished first would interleave them.
///
/// Asserted against the file's own order rather than against another run, so
/// it fails on a merge that is consistently wrong as well as on one that is
/// unstable. `public.widgets` is five rows in a 371-byte data region, which
/// eight workers cut into eight pieces, so the ids really do come from
/// different sub-streams.
#[test]
fn the_merge_prints_file_order_at_every_job_count() {
    let dump = fixture("16/edge_cases/default.sql");
    for jobs in ["1", "2", "3", "5", "8", "24"] {
        let out = query(&dump, "public.widgets", &["--jobs", jobs, "--column", "id"]);
        assert!(out.status.success(), "--jobs {jobs}: {}", stderr_of(&out));
        let printed = stdout_of(&out);
        let ids: Vec<&str> = printed.lines().skip(1).collect();
        assert_eq!(ids, ["1", "2", "3", "4", "5"], "--jobs {jobs} printed out of file order");
    }
}

/// A `parse` under a stated budget writes the cache a default one writes: the
/// listing does not depend on which read path the budget chose.
#[test]
fn a_parse_reports_the_same_listing_at_any_stated_parallelism() {
    let mut reference: Option<String> = None;
    for extra in [vec![], vec!["--jobs", "8"], vec!["--parallel-memory", "1048576"]] {
        let dir = tempfile::tempdir().unwrap();
        let dump = dir.path().join("parallel.sql");
        std::fs::copy(fixture("16/edge_cases/default.sql"), &dump).unwrap();
        let mut args = vec!["parse", "--source", dump.to_str().unwrap()];
        args.extend_from_slice(&extra);
        let out = run(&args);
        assert!(out.status.success(), "{extra:?}: {}", stderr_of(&out));
        // Everything but the trailing `wrote cache to <path>`, which names
        // this run's own tempdir.
        let printed = stdout_of(&out);
        let got = printed
            .lines()
            .filter(|line| !line.starts_with("wrote cache to "))
            .collect::<Vec<_>>()
            .join("\n");
        match &reference {
            None => reference = Some(got),
            Some(want) => assert_eq!(&got, want, "{extra:?} changed the listing"),
        }
    }
}

/// Zero is refused for both, before the file is opened: `--jobs 0` would read
/// as the serial path through `Parallelism::workers`, which is a surprise
/// rather than an answer, and a budget of zero leaves no room for a buffer of
/// any unit.
#[test]
fn zero_is_refused_for_both_flags() {
    let dump = fixture("16/edge_cases/default.sql");
    let jobs = query(&dump, "public.widgets", &["--jobs", "0"]);
    assert!(!jobs.status.success());
    assert!(stderr_of(&jobs).contains("--jobs 1 is the serial path"), "{}", stderr_of(&jobs));

    let memory = query(&dump, "public.widgets", &["--parallel-memory", "0"]);
    assert!(!memory.status.success());
    assert!(stderr_of(&memory).contains("no room for a read buffer"), "{}", stderr_of(&memory));
}
