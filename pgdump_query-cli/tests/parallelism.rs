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
/// **The reference is `--jobs 1` stated, not the default**, since `--jobs` cuts
/// the replay. The default is 1 today, so the two coincide — but a reference
/// that inherited it would follow the default wherever it goes next, and a
/// parallel one would compare one partitioned run against another and pass
/// however the merge ordered them.
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
        // Below this fixture's 512-byte block unit, so no whole block can be
        // held: the streaming reader, on a file that has boundaries to seek
        // by. `--jobs 2` beside it is incidental: a stated budget reaches the
        // source at `--jobs 1` too.
        vec!["--jobs", "2", "--parallel-memory", "400"],
        vec!["--parallel-memory", "65536"],
        vec!["--jobs", "8", "--parallel-memory", "536870912"],
    ] {
        let out = query(&compressed, "public.widgets", &extra);
        assert!(out.status.success(), "{extra:?}: {}", stderr_of(&out));
        assert_eq!(stdout_of(&out), stdout_of(&reference), "{extra:?} changed the rows");
    }
}

/// The byte count a decline's message names as the recourse — *raise the
/// memory budget to N byte(s) or more* — pulled back out of the sentence,
/// because the number is the source's and this crate has no second copy of it.
fn recourse_bytes(stderr: &str) -> u64 {
    let (_, tail) = stderr.split_once("raise the memory budget to ").expect(stderr);
    let (number, _) = tail.split_once(" byte(s) or more").expect(stderr);
    number.parse().expect(number)
}

/// The budget at the CLI: the one that sent a compressed read down the
/// streaming path says so on stderr, once, naming the file's largest block and
/// what one reader of it would have held — a block slot, the chunk buffer, the
/// decoder's own retention and the pool's own slots — so the flag that says
/// *raise it* also
/// says what to raise it to. The rows go to stdout and are untouched by it.
#[test]
fn a_declined_block_path_is_announced_once_on_stderr() {
    let (_xz_dir, compressed) = seekable_xz();

    // 400 bytes is below the fixture's 512-byte block unit.
    let out = query(&compressed, "public.widgets", &["--jobs", "2", "--parallel-memory", "400"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let err = stderr_of(&out);
    assert_eq!(err.matches("streaming decoder").count(), 1, "said once, not per sub-stream: {err}");
    assert!(err.contains("memory budget of 400"), "the budget that declined it is named: {err}");
    // The recourse is the whole of what a reader costs the rule, so it is past
    // the four 512-byte blocks the pool keeps by the decoder's own dictionary —
    // read out of the sentence rather than restated, this being the CLI's view
    // of a number the library owns.
    assert!(recourse_bytes(&err) > 4 * 512, "{err}");

    // A budget that affords a whole block says nothing at all about the read
    // path.
    let quiet =
        query(&compressed, "public.widgets", &["--jobs", "2", "--parallel-memory", "536870912"]);
    assert!(quiet.status.success(), "{}", stderr_of(&quiet));
    assert!(!stderr_of(&quiet).contains("streaming decoder"), "{}", stderr_of(&quiet));

    // Neither does a plain file, which has no container to decline.
    let plain_dir = tempfile::tempdir().unwrap();
    let plain = plain_dir.path().join("edge_cases.sql");
    std::fs::copy(edge_cases_sql(), &plain).unwrap();
    let plain_out = query(&plain, "public.widgets", &["--jobs", "2", "--parallel-memory", "400"]);
    assert!(plain_out.status.success(), "{}", stderr_of(&plain_out));
    assert!(!stderr_of(&plain_out).contains("streaming decoder"), "{}", stderr_of(&plain_out));
}

/// **The budget is stated with no `--jobs` beside it**, which is the whole of
/// what a caller who does not want to think about workers can ask for: the
/// worker count is filled in from the source and the stated bytes ride through
/// whatever it comes back as — so the same decline and the same silence follow
/// from `--parallel-memory` alone, with no `--jobs 2` typed
/// (`docs/design/architecture.md`, "Execution model and API surface").
///
/// Driven through the CLI rather than through the value, because what this
/// pins is that the flag reaches `ScanOptions`/`QueryOptions` and then the
/// source: a budget carried in the value and dropped on the way down would
/// leave the unit tests passing.
#[test]
fn a_stated_budget_decides_the_read_path_with_no_jobs_flag() {
    let (_xz_dir, compressed) = seekable_xz();

    let declined = query(&compressed, "public.widgets", &["--parallel-memory", "400"]);
    assert!(declined.status.success(), "{}", stderr_of(&declined));
    let err = stderr_of(&declined);
    assert!(err.contains("memory budget of 400"), "the stated budget declined it: {err}");
    assert!(recourse_bytes(&err) > 2 * 512, "{err}");

    // And raising it alone takes the block path back — the recourse the
    // message names, with nothing else stated beside it.
    let quiet = query(&compressed, "public.widgets", &["--parallel-memory", "536870912"]);
    assert!(quiet.status.success(), "{}", stderr_of(&quiet));
    assert!(!stderr_of(&quiet).contains("streaming decoder"), "{}", stderr_of(&quiet));

    // The rows are the same either way, which is the standing promise: the
    // budget decides a read path and never an answer.
    assert_eq!(stdout_of(&declined), stdout_of(&quiet));
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

/// A dump whose one table holds `rows` integers, with two values that are not
/// integers at all: `zzzEARLY` at row `early` and `zzzLATE` at row `late`.
///
/// Row counts rather than byte offsets, because what the test needs is only
/// that one bad value is earlier in the file than the other and that they land
/// in **different** sub-streams at a low job count — 40,000 rows split in two
/// puts the byte midpoint near row 20,900, so 18,000 and 22,000 sit either side
/// of it with a margin of a thousand rows each, and 18,000 is past the
/// 8,192-row batch that sub-stream 0 fills first.
fn dump_with_two_bad_rows(dir: &Path, rows: u32, early: u32, late: u32) -> PathBuf {
    let mut file = String::from("SET client_encoding = 'UTF8';\n\n");
    file.push_str("CREATE TABLE public.t_bad (\n    id integer\n);\n\n");
    file.push_str("COPY public.t_bad (id) FROM stdin;\n");
    for i in 1..=rows {
        match i {
            _ if i == early => file.push_str("zzzEARLY\n"),
            _ if i == late => file.push_str("zzzLATE\n"),
            _ => file.push_str(&format!("{i}\n")),
        }
    }
    file.push_str("\\.\n\n");
    let path = dir.join("two_bad_rows.sql");
    std::fs::write(&path, file).unwrap();
    path
}

/// **The lowest-offset error is the one raised, at every job count**
/// (`docs/design/architecture.md`, "`pgdq query` merges the sub-streams back
/// into file order"). Two rows fail to decode; the later one is in a sub-stream that
/// reaches it in its very first batch, while the earlier one is three batches
/// into the sub-stream before it. A merge that raised whichever failure arrived
/// first would name `zzzLATE` at `--jobs 2` and `zzzEARLY` serially, so a user
/// re-running to confirm the failure would be told about a different row.
///
/// The serial run is the reference, named rather than defaulted to, and the
/// assertion is on the *value* rather than the offset so a failure says which
/// row was reported.
#[test]
fn the_lowest_offset_error_is_the_one_the_merge_raises() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dump_with_two_bad_rows(dir.path(), 40_000, 18_000, 22_000);
    for jobs in ["1", "2", "3", "4", "8"] {
        let out = query(&dump, "public.t_bad", &["--jobs", jobs]);
        assert!(!out.status.success(), "--jobs {jobs}: the bad rows must fail the query");
        let err = stderr_of(&out);
        assert!(err.contains("zzzEARLY"), "--jobs {jobs} named the wrong row: {err}");
        assert!(!err.contains("zzzLATE"), "--jobs {jobs} named the later row: {err}");
        // And nothing past the error prints. Every sub-stream is filled in the
        // first round, so the ones after the failing one are holding a batch
        // when it fails; left in their slots those batches would print as soon
        // as the sub-streams before them drained, which is rows a serial
        // replay never reached.
        let printed = stdout_of(&out);
        for line in printed.lines().skip(1) {
            let id: u32 = line.trim().parse().unwrap_or_else(|_| panic!("--jobs {jobs}: {line}"));
            assert!(id < 18_000, "--jobs {jobs} printed row {id}, past the error at 18000");
        }
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
