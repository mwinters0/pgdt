//! `pgdt`'s status output — the `tracing` lines `parse`, `info` and `query`
//! write to stderr for the two phases worth watching a long run for: an
//! `.xz` file's seek-table walk, which all three can pay, and the scan itself
//! starting and finishing, which only `parse` and `query` do — `info` never
//! scans (`info_announces_no_scan`)
//! (`docs/design/decisions.md`, "D64"). What only the binary can say is that
//! the subscriber is actually wired up and that these lines reach real
//! stderr — the library's own tests exercise the `tracing::info!` call sites
//! directly, with no subscriber installed, and see nothing.
//!
//! **Two mechanisms both scan, and both had to be named apart.** `parse`
//! internally runs `index::scan_preamble` (a bounded prepass, `"preamble scan
//! …"`) before `stream::map_forward` (the loop that actually reads the file,
//! `"scan …"`), and a single uninterrupted run reports both in sequence. Every
//! assertion here that touches a line spelling `"scan started"` or `"scan
//! complete"` is careful to say which of the two it means — a test written
//! against the ambiguous substring cannot tell the two apart.

use std::path::{Path, PathBuf};
use std::process::Command;

use pgdump_query::{
    ByteRangeSource, DumpIndex, DumpMetadata, LocalFileSource, ScanOptions, Span, SpanBody,
    XzSource, build_index, cache,
};

mod common;
use common::{fixture, run, stderr_of};

/// A small plain fixture with several `COPY` blocks — any one with more than
/// one block will do, since `an_interrupted_and_resumed_parse_is_told_apart_from_a_fresh_one`
/// needs a real mid-file resume point.
fn plain_dump() -> PathBuf {
    fixture("16/edge_cases/default.sql")
}

/// The same hand-written dump `xz_source.rs`/`parallelism.rs` build their
/// `.xz` fixtures from.
fn edge_cases_sql() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../pgdump_query/tests/data/edge_cases.sql")
}

/// A seekable multi-block `.xz` copy, in a directory of its own. `xz` is not
/// `mise`-pinned, so a missing binary fails loudly rather than the test
/// silently skipping (`docs/design/roadmap.md`, "A test may assume the tools
/// `mise` pins").
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

/// A cache holding everything up to `frontier` and an `Unscanned` tail —
/// the shape a real interrupted `parse` leaves — built directly rather than
/// by actually interrupting a process, the same construction
/// `partial_reporting.rs`'s `write_truncated_cache` uses and for the same
/// reason: deterministic, and it needs no signal timing.
async fn write_truncated_cache(dump: &Path, frontier: u64) -> PathBuf {
    let source = LocalFileSource::open(dump).unwrap();
    let size = std::fs::metadata(dump).unwrap().len();
    let full = build_index(&source, &ScanOptions::default()).await.unwrap();

    let mut spans: Vec<Span> = full.spans.iter().filter(|s| s.start < frontier).cloned().collect();
    if let Some(last) = spans.last_mut() {
        last.end = frontier;
    }
    spans.push(Span {
        start: frontier,
        end: size,
        database: None,
        text: None,
        toc: None,
        toc_owned: false,
        body: SpanBody::Unscanned,
    });
    let index = DumpIndex {
        spans,
        scanned_through: frontier,
        metadata: full.metadata.clone().map(|m| DumpMetadata { databases: m.databases }),
        roles: full.roles.clone(),
        tablespaces: full.tablespaces.clone(),
        diagnostics: Vec::new(),
    };
    let path = cache::colocated_path(dump);
    cache::save(&path, &source, &index).await.unwrap();
    path
}

/// The `end_offset` of the `n`-th `COPY` block — a real resumable watermark
/// rather than an arbitrary byte.
async fn block_frontier(dump: &Path, n: usize) -> u64 {
    let source = LocalFileSource::open(dump).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    index.blocks().nth(n).expect("the fixture has this many blocks").end_offset
}

/// `word` opens `line`, loosely enough to survive a subsecond-precision
/// change: `YYYY-MM-DDTHH:MM:SS`, then anything, ending `Z` before the first
/// space — exact enough to catch "not a timestamp at all" without pinning
/// `tracing_subscriber`'s exact rendering.
fn opens_with_rfc3339_stamp(line: &str) -> bool {
    let Some(stamp) = line.split_whitespace().next() else { return false };
    let b = stamp.as_bytes();
    b.len() >= 20
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && stamp.ends_with('Z')
}

/// Lines reporting the bounded preamble prepass (`index::scan_preamble`,
/// through `crate::scan::scan`).
fn preamble_lines(stderr: &str) -> Vec<&str> {
    stderr.lines().filter(|l| l.contains("preamble scan")).collect()
}

/// Lines reporting the real read loop (`stream::map_forward`) — deliberately
/// excludes anything spelling `"preamble scan"`, since that also contains the
/// substring `"scan started"`/`"scan complete"` and a test matching on the
/// bare substring would not be able to tell the two apart, which is
/// defect 1 exactly.
fn main_scan_lines(stderr: &str) -> Vec<&str> {
    stderr.lines().filter(|l| l.contains("scan") && !l.contains("preamble")).collect()
}

/// The CLI's **first** report line — what was stated or discovered, printed
/// before the dump is opened. Keyed on the mode sentence, which is the half of
/// the resolution that needs no source.
fn stated_report(stderr: &str) -> &str {
    stderr
        .lines()
        .find(|l| l.contains("memory limit") || l.contains("memory allocation"))
        .unwrap_or_else(|| panic!("no stated report: {stderr}"))
}

/// The CLI's **second** report line — what the source's recommendation and the
/// allowance fitted to, printed once the file has been opened and asked.
fn resolved_report(stderr: &str) -> &str {
    stderr
        .lines()
        .find(|l| l.contains("resolved the arrangement"))
        .unwrap_or_else(|| panic!("no resolved report: {stderr}"))
}

/// A fresh `parse` of a plain file states the arrangement once and says when
/// each of its two passes finishes — the pair the manual shows
/// (`docs/manual/dump-inspection.md`, "Status on stderr").
#[test]
fn a_fresh_parse_announces_the_preamble_pass_and_the_scan_separately() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("out.dtcache");
    let out = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dtcache",
        cache.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);

    let preamble = preamble_lines(&stderr);
    let main = main_scan_lines(&stderr);
    assert_eq!(preamble.len(), 2, "expected a preamble started/complete pair: {stderr}");
    assert_eq!(main.len(), 2, "expected a scan started/complete pair: {stderr}");
    assert!(preamble[0].contains("started") && preamble[1].contains("complete"), "{stderr}");
    assert!(main[0].contains("started") && main[1].contains("complete"), "{stderr}");

    // An RFC3339 timestamp opens each one, so a line correlates with anything
    // else read off the same clock — a `dmesg` entry, a cgroup sample, an
    // orchestrator's own log.
    for line in preamble.iter().chain(main.iter()) {
        assert!(opens_with_rfc3339_stamp(line), "{line:?}");
    }
}

/// **The regression this file exists to catch.** A fresh, uninterrupted
/// `parse` runs the preamble pass then the real scan once each — it must
/// never look like an interrupted scan that got resumed, which is what two
/// mechanisms sharing the name "scan" produced: a "scan complete" with
/// `reached_eof=false` immediately followed by a second "scan started" naming
/// a nonzero `resumed_from`, indistinguishable from a real interruption and
/// resume. With the two passes named apart, the real "scan" line appears
/// exactly once — never twice, and never paired with an early, non-EOF
/// completion of itself.
#[test]
fn an_uninterrupted_parse_never_looks_like_a_resumed_one() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("out.dtcache");
    let out = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dtcache",
        cache.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);

    let main = main_scan_lines(&stderr);
    assert_eq!(main.len(), 2, "the real scan reports itself exactly once: {stderr}");
    assert!(main[0].contains("scan started"), "{stderr}");
    assert!(main[1].contains("scan complete"), "{stderr}");
    // The one "scan complete" is the true end of the file, not an early stop
    // that a second "scan started" would then appear to resume from.
    assert!(main[1].contains("reached_eof=true"), "{stderr}");
}

/// **A genuine resume, contrasted with the fresh case above.** `map_forward`
/// skips the preamble pass whenever it is not starting at byte 0
/// (`stream::map_file`'s `resumed_from == 0` gate), so a cache built to look
/// like an interrupted scan's produces no `"preamble scan"` line at all —
/// only the real scan, naming the frontier it picked up from. That is the
/// shape that actually distinguishes a resume from a cold start once the two
/// mechanisms are named apart: not the presence of `resumed_from` (a fresh
/// parse's real scan always names one — the preamble's own end), but the
/// *absence* of a preceding preamble pass.
#[tokio::test]
async fn a_genuine_resume_reports_no_preamble_pass_and_names_the_frontier() {
    let (_dir, dump) = common::sandboxed("16/edge_cases/default.sql", "dump.sql");
    let frontier = block_frontier(&dump, 0).await;
    assert!(frontier > 0, "a real watermark, not byte 0");
    let cache = write_truncated_cache(&dump, frontier).await;

    let out =
        run(&["parse", "--source", dump.to_str().unwrap(), "--dtcache", cache.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);

    assert!(preamble_lines(&stderr).is_empty(), "a resume re-runs no preamble pass: {stderr}");
    let main = main_scan_lines(&stderr);
    assert_eq!(main.len(), 2, "{stderr}");
    assert!(
        main[0].contains(&format!("resumed_from={frontier}")),
        "expected resumed_from={frontier}: {stderr}"
    );
}

/// `--jobs`/`--memory` are the arrangement a "scan started" line
/// names — stated once, in the library's own vocabulary, not the flags', and
/// as a plain byte count rather than `Option`'s debug spelling.
///
/// The number on the line is the **read-buffer budget** carved out of the
/// stated allowance, never the allowance itself
/// (`docs/design/decisions.md`, "D83"): a plain source recommends no
/// per-reader memory, so a 640 MiB allowance leaves it the library's own
/// 64 MiB constant under a 256 MiB cap.
#[test]
fn scan_started_names_jobs_and_the_stated_memory_budget_as_a_quantity() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("out.dtcache");
    let out = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dtcache",
        cache.to_str().unwrap(),
        "--jobs",
        "4",
        "--memory",
        "671088640",
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    let started = main_scan_lines(&stderr)
        .into_iter()
        .find(|l| l.contains("started"))
        .unwrap_or_else(|| panic!("no main \"scan started\" line: {stderr}"));
    assert!(started.contains("jobs=4"), "{started}");
    // A bare quantity — not `Some(67108864)`, `Option`'s debug spelling
    // reaching a user-facing line.
    assert!(started.contains("memory_bytes=67108864"), "{started}");
    assert!(!started.contains("Some("), "{started}");
    assert!(!started.contains("None"), "{started}");
}

/// **The default case of the same defect.** Stating no byte budget still leaves
/// a real one governing every read — `None` said nothing a reader could act on;
/// the line states the number actually in force, and marks it `(default)` on
/// the one arrangement that carries no number at all.
///
/// **Which arrangement that is now depends on the machine.** A plain file
/// recommends no budget, so a flagless scan reads `67108864 (default)` where no
/// memory limit is discovered and the discovered allowance, bare, where one is
/// — so the assertion is written against `discover_memory_limit` rather than
/// against the constant, which would pass on a bare host and fail in exactly
/// the container this default exists for.
#[test]
fn scan_started_names_the_default_memory_budget_when_none_was_stated() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("out.dtcache");
    let out = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dtcache",
        cache.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    let started = main_scan_lines(&stderr)
        .into_iter()
        .find(|l| l.contains("started"))
        .unwrap_or_else(|| panic!("no main \"scan started\" line: {stderr}"));
    assert!(started.contains("jobs=1"), "{started}");
    match pgdump_query::discover_memory_limit() {
        None => assert!(started.contains("memory_bytes=67108864 (default)"), "{started}"),
        Some(limit) => {
            let budget = pgdump_query::DEFAULT_MEMORY_BUDGET
                .min(limit.bytes.saturating_sub(pgdump_query::MEMORY_RESERVE));
            assert!(started.contains(&format!("memory_bytes={budget}")), "{started}");
            assert!(!started.contains("(default)"), "a discovered budget is not it: {started}");
        }
    }
}

/// **A run says which of two arrangements it is in, before it opens the
/// scan.** The mode is a fact about the *environment* and nothing downstream
/// can recover it: `Parallelism` carries the number and not where it came
/// from, so a discovered budget and a stated one reach the library identical
/// (`docs/design/decisions.md`, "D64"). What only the binary can
/// say is that the lines are emitted at all, on both scanning commands, and
/// that they agree with what this machine actually reports.
///
/// **The report is two lines, and which fact is on which is the point.** The
/// mode, the limit and the flags as typed need no source, so they go first;
/// only the second line can name a count the allowance lowered, since the
/// lowering is the source's recommendation meeting the budget.
///
/// The mode itself is asserted against `discover_memory_limit` rather than
/// against either wording, for the same reason the budget assertions are:
/// the suite runs on bare hosts and in containers, and a test that pinned one
/// of them would pass on this machine and fail in the container the default
/// exists for. Which *arm* is exercised is pinned instead over the committed
/// runtime roots, in `main.rs`'s own resolution tests.
#[test]
fn a_scanning_command_reports_the_arrangement_it_resolved() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("out.dtcache");
    let out = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dtcache",
        cache.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    let stated = stated_report(&stderr);
    let resolved = resolved_report(&stderr);
    // Nothing was typed, and the line says so rather than leaving the reader
    // to infer it from a number's absence.
    assert!(stated.contains("jobs_flag=(not stated)"), "{stated}");
    assert!(stated.contains("memory_flag=(not stated)"), "{stated}");
    // A flagless plain scan takes the source's own recommendation, which is
    // the serial path — and says so as a recommendation rather than as
    // something a person typed.
    assert!(resolved.contains("jobs=1 (recommended by the source)"), "{resolved}");
    match pgdump_query::discover_memory_limit() {
        None => {
            assert!(stated.contains("no memory limit found"), "{stated}");
            assert!(resolved.contains("(default: no limit found)"), "{resolved}");
        }
        Some(limit) => {
            assert!(stated.contains("running inside a stated memory allocation"), "{stated}");
            assert!(stated.contains(&format!("limit_bytes={}", limit.bytes)), "{stated}");
            assert!(
                stated.contains(&format!("limit_read_from={}", limit.read_from.display())),
                "{stated}"
            );
            assert!(resolved.contains("(discovered:"), "{resolved}");
        }
    }
    // The stated half comes first: everything on it was true before the file
    // was touched, and on an `.xz` source touching the file is the expensive
    // part.
    assert!(
        stderr.find(stated).unwrap() < stderr.find(resolved).unwrap(),
        "the stated half precedes the resolved one: {stderr}"
    );

    // The same pair on `query`, which resolves the same two numbers for its
    // mapping pass and its replay and must announce them once.
    let out = run(&[
        "query",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dtcache",
        "none",
        "--table",
        "public.widgets",
        "--no-columns",
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    let stated: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("memory limit") || l.contains("memory allocation"))
        .collect();
    assert_eq!(stated.len(), 1, "discovered once, announced once: {stderr}");
    let resolved: Vec<&str> =
        stderr.lines().filter(|l| l.contains("resolved the arrangement")).collect();
    assert_eq!(resolved.len(), 1, "resolved once, announced once: {stderr}");
}

/// **The stated half is printed before the seek-table walk, which is the whole
/// of why it is a line of its own.** Opening a fresh `.xz` source walks every
/// stream footer before it can advise anything, so a mistyped
/// `--memory` would otherwise go unconfirmed until after a wait it
/// had no bearing on (`docs/design/decisions.md`, "D64").
///
/// Asserted on a fixture whose walk is instant, since what is being pinned is
/// the **order** of two lines and not the duration between them.
#[test]
fn the_stated_half_is_reported_before_the_seek_table_walk() {
    let (_dir, xz) = seekable_xz();
    let out_dir = tempfile::tempdir().unwrap();
    let cache = out_dir.path().join("out.dtcache");
    let out = run(&[
        "parse",
        "--source",
        xz.to_str().unwrap(),
        "--dtcache",
        cache.to_str().unwrap(),
        "--memory",
        "268435456",
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    let stated = stderr.find(stated_report(&stderr)).unwrap();
    let walk = stderr.find("seek table build started").expect(&stderr);
    let resolved = stderr.find(resolved_report(&stderr)).unwrap();
    assert!(stated < walk, "the flags are confirmed before the walk: {stderr}");
    assert!(walk < resolved, "the arrangement waits on the source: {stderr}");
    // And the typed value is there to be checked against what was meant.
    assert!(stderr[stated..walk].contains("memory_flag=268435456"), "the flag as typed: {stderr}");
}

/// **A stated flag is reported as stated, on both numbers.** The provenance is
/// the whole point of the line: the same two readers can appear under
/// `jobs=2` and under `jobs=24` depending on which of them a person typed, and
/// only a recommended count is telling the user what will run.
#[test]
fn the_mode_report_marks_a_stated_flag_as_stated() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("out.dtcache");
    let out = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dtcache",
        cache.to_str().unwrap(),
        "--jobs",
        "3",
        "--memory",
        "671088640",
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    let stated = stated_report(&stderr);
    assert!(stated.contains("jobs_flag=3"), "{stated}");
    assert!(stated.contains("memory_flag=671088640"), "{stated}");
    let resolved = resolved_report(&stderr);
    assert!(resolved.contains("jobs=3 (stated)"), "{resolved}");
    // The budget beside it is carved, so the line carries both numbers rather
    // than the typed one twice (`docs/design/decisions.md`, "D83").
    assert!(
        resolved.contains("memory_bytes=67108864 (stated: --memory allows 671088640 resident"),
        "{resolved}"
    );
    assert!(!resolved.contains("recommended"), "{resolved}");
}

/// **`(default)` says nobody asked, not that nobody could ask.** The serial
/// path carries a carved budget like any other, so `--memory` over a plain
/// file — whose own recommendation is the serial path — prints that budget
/// bare, which is what makes the flag's own recourse ("raise the memory
/// budget") readable from the log without a second worker being stated beside
/// it (`docs/design/decisions.md`, "I/O, memory and parallelism").
///
/// **The number here is the library's constant and the marking is still
/// absent**, which is the whole assertion: a plain source recommends nothing,
/// so an allowance leaves it `DEFAULT_MEMORY_BUDGET` under the cap, and a run
/// that had stated nothing would print that same number marked `(default)`.
#[test]
fn a_stated_memory_budget_at_a_serial_job_count_is_not_marked_default() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("out.dtcache");
    let out = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dtcache",
        cache.to_str().unwrap(),
        "--memory",
        "671088640",
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    let started = main_scan_lines(&stderr)
        .into_iter()
        .find(|l| l.contains("started"))
        .unwrap_or_else(|| panic!("no main \"scan started\" line: {stderr}"));
    assert!(started.contains("jobs=1"), "{started}");
    assert!(started.contains("memory_bytes=67108864"), "{started}");
    assert!(!started.contains("(default)"), "a stated allowance is not the default: {started}");
    assert!(!started.contains("None"), "{started}");
    assert!(!started.contains("Some("), "{started}");
}

/// No line anywhere ever reaches a reader as `Option`'s own debug spelling —
/// a blanket guard, over every command this file exercises, for the shape of
/// defect 2 recurring at a call site a narrower test does not reach.
#[test]
fn no_status_line_ever_prints_nones_or_somes() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("out.dtcache");
    let out = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dtcache",
        cache.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    assert!(!stderr.contains("None"), "{stderr}");
    assert!(!stderr.contains("Some("), "{stderr}");
}

/// Re-running `parse` against a cache that already covers the file is not a
/// scan (`docs/manual/dump-inspection.md`, "`parse`: reading the dump" —
/// "costs nothing and says so"), and the status output agrees: no
/// "scan started"/"scan complete" line of either kind the second time.
#[test]
fn a_fully_cached_reparse_announces_no_scan() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("out.dtcache");
    let first = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dtcache",
        cache.to_str().unwrap(),
    ]);
    assert!(first.status.success(), "{}", stderr_of(&first));
    assert!(!main_scan_lines(&stderr_of(&first)).is_empty());

    let second = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dtcache",
        cache.to_str().unwrap(),
    ]);
    assert!(second.status.success(), "{}", stderr_of(&second));
    let stderr = stderr_of(&second);
    assert!(preamble_lines(&stderr).is_empty(), "{stderr}");
    assert!(main_scan_lines(&stderr).is_empty(), "{stderr}");
}

/// `info` never scans, so it never announces either pass — its output is
/// read entirely from the cache
/// (`docs/design/decisions.md`, "D61").
#[test]
fn info_announces_no_scan() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("out.dtcache");
    let parsed = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dtcache",
        cache.to_str().unwrap(),
    ]);
    assert!(parsed.status.success(), "{}", stderr_of(&parsed));

    let info = run(&[
        "info",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dtcache",
        cache.to_str().unwrap(),
    ]);
    assert!(info.status.success(), "{}", stderr_of(&info));
    let stderr = stderr_of(&info);
    assert!(preamble_lines(&stderr).is_empty(), "{stderr}");
    assert!(main_scan_lines(&stderr).is_empty(), "{stderr}");
    assert!(!stderr.contains("seek table build"), "{stderr}");
}

/// A fresh `.xz` open walks the footers and says so; a second `parse` against
/// the cache that walk left behind opens from the persisted table instead
/// (`XzSource::with_table`) and earns no line — the walk that gets no line is
/// the one a cached table exists to skip (`docs/design/decisions.md`,
/// "D18").
#[test]
fn the_seek_table_walk_is_announced_once_and_only_on_a_fresh_open() {
    let (_dir, xz_path) = seekable_xz();
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = cache_dir.path().join("out.dtcache");

    let first = run(&[
        "parse",
        "--source",
        xz_path.to_str().unwrap(),
        "--dtcache",
        cache.to_str().unwrap(),
    ]);
    assert!(first.status.success(), "{}", stderr_of(&first));
    let first_stderr = stderr_of(&first);
    assert_eq!(first_stderr.matches("seek table build started").count(), 1, "{first_stderr}");
    assert_eq!(first_stderr.matches("seek table build complete").count(), 1, "{first_stderr}");

    // The cache already covers the file, so this is the "nothing to do" case
    // above and opens through `with_table` regardless — no walk either way.
    let second = run(&[
        "parse",
        "--source",
        xz_path.to_str().unwrap(),
        "--dtcache",
        cache.to_str().unwrap(),
    ]);
    assert!(second.status.success(), "{}", stderr_of(&second));
    assert!(!stderr_of(&second).contains("seek table build"), "{}", stderr_of(&second));
}

/// **The shipped default is the source's, and the status line is where a user
/// sees which one they got.** A plain `parse` with no `--jobs` says `jobs=1`
/// (above); the same command over an `.xz` file says the cores this process was
/// given, capped at the blocks the file offers to cut at, because decode is the
/// one shape that scales and a seam past the last block does not exist
/// (`docs/design/decisions.md`, "D2").
///
/// Asserted against `available_parallelism()` and the file's own table rather
/// than a literal — the count is the machine's and the cap is the fixture's,
/// and on a one-CPU runner the answer legitimately *is* 1, which is why the
/// plain leg is asserted beside it rather than the `.xz` leg alone: what this
/// pins is that the two commands can differ, and by which number. This
/// fixture has a handful of blocks, so on any ordinary workstation it is the
/// **cap** that is being read here.
#[test]
fn an_xz_parse_defaults_to_the_cores_and_a_plain_one_to_serial() {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let (_dir, xz_path) = seekable_xz();
    let blocks = XzSource::open(&xz_path)
        .unwrap()
        .seek_table()
        .expect("an XzSource always has a table")
        .block_count();
    let cores = cores.min(blocks);
    let cache_dir = tempfile::tempdir().unwrap();

    let started_of = |source: &str, cache: PathBuf| -> String {
        let out = run(&["parse", "--source", source, "--dtcache", cache.to_str().unwrap()]);
        assert!(out.status.success(), "{}", stderr_of(&out));
        let stderr = stderr_of(&out);
        main_scan_lines(&stderr)
            .into_iter()
            .find(|l| l.contains("started"))
            .unwrap_or_else(|| panic!("no main \"scan started\" line: {stderr}"))
            .to_string()
    };

    let compressed = started_of(xz_path.to_str().unwrap(), cache_dir.path().join("xz.dtcache"));
    assert!(compressed.contains(&format!("jobs={cores}")), "{compressed}");

    let plain = started_of(plain_dump().to_str().unwrap(), cache_dir.path().join("plain.dtcache"));
    assert!(plain.contains("jobs=1"), "{plain}");
}

/// Lines correcting the arrangement (`stream::report_shortfall`).
fn arrangement_lines(stderr: &str) -> Vec<&str> {
    stderr.lines().filter(|l| l.contains("scan arrangement")).collect()
}

/// **`parse` says what ran, not only what was asked for.** A compressed
/// source whose largest block the budget cannot hold
/// reads through the streaming decoder and is therefore serial whatever
/// `--jobs` says — a decline `query` announces on a plan note and `parse` has
/// no plan notes to carry. The correction is on the status channel both
/// commands already have, once per scan, naming the delivered count beside
/// the announced one and the budget that would buy the path back
/// (`docs/design/decisions.md`, "D64").
///
/// What only the binary can say is that the line reaches real stderr and that
/// the two numbers on it actually disagree with `scan started`'s.
#[test]
fn a_parse_a_declined_source_runs_serially_says_so_once() {
    let (_xz_dir, compressed) = seekable_xz();
    let dir = tempfile::tempdir().unwrap();

    // 400 bytes is below the fixture's 512-byte block unit, so the block path
    // is declined and the source advises one partition over the whole file —
    // the same arrangement `parallelism.rs` asserts the `query` decline in.
    let out = run(&[
        "parse",
        "--source",
        compressed.to_str().unwrap(),
        "--dtcache",
        dir.path().join("declined.dtcache").to_str().unwrap(),
        "--statistics",
        "none",
        "--jobs",
        "2",
        "--memory",
        "400",
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    let started = main_scan_lines(&stderr)
        .into_iter()
        .find(|l| l.contains("started"))
        .unwrap_or_else(|| panic!("no main \"scan started\" line: {stderr}"));
    assert!(started.contains("jobs=2"), "the line that says what was asked for: {started}");

    let corrections = arrangement_lines(&stderr);
    assert_eq!(corrections.len(), 1, "said once for the scan, not once a block: {stderr}");
    let line = corrections[0];
    assert!(line.contains("jobs=1"), "the delivered count: {line}");
    assert!(line.contains("asked=2"), "beside the announced one: {line}");
    assert!(line.contains("source"), "the source is what refused, not the divisor: {line}");
    // The recourse is the whole of what one reader of the declined path holds
    // — four 512-byte block units with the pool's retention list, the chunk
    // and the decoder's own retention — so it is past two blocks whatever the
    // exact sum, and it is the source's own number, which is why nothing here
    // restates it.
    let (_, tail) = line.split_once("would_hold_bytes=").expect(line);
    let bytes: u64 = tail.split_whitespace().next().expect(line).parse().expect(line);
    assert!(bytes > 2 * 512, "{line}");

    // A budget that affords a whole block corrects nothing: the source would
    // be split, and this fixture's whole file being shorter than one reader's
    // charge is a fact about where the leader stands rather than about the
    // arrangement.
    let out = run(&[
        "parse",
        "--source",
        compressed.to_str().unwrap(),
        "--dtcache",
        dir.path().join("afforded.dtcache").to_str().unwrap(),
        "--statistics",
        "none",
        "--jobs",
        "2",
        "--memory",
        "536870912",
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(arrangement_lines(&stderr_of(&out)).is_empty(), "{}", stderr_of(&out));

    // And a plain file, which has no container to decline, is corrected by the
    // budget instead — the other of the two rules, reported by name.
    let out = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dtcache",
        dir.path().join("plain.dtcache").to_str().unwrap(),
        "--statistics",
        "none",
        "--jobs",
        "2",
        "--memory",
        "400",
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    let corrections = arrangement_lines(&stderr);
    assert_eq!(corrections.len(), 1, "{stderr}");
    assert!(corrections[0].contains("jobs=1"), "{}", corrections[0]);
    assert!(corrections[0].contains("budget"), "{}", corrections[0]);
}

/// **Gathering statistics corrects nothing**: a gathered table's interior is
/// split across the workers like any other, so a gathering `parse` at a count
/// the leader cuts at prints no `scan arrangement` line, exactly as one under
/// `--statistics none` does. The chunk is stated so the leader really cuts.
#[test]
fn a_gathering_parse_is_not_corrected() {
    let dir = tempfile::tempdir().unwrap();
    for (leg, statistics) in [("gathered", "all"), ("none", "none")] {
        let out = run(&[
            "parse",
            "--source",
            plain_dump().to_str().unwrap(),
            "--dtcache",
            dir.path().join(format!("{leg}.dtcache")).to_str().unwrap(),
            "--jobs",
            "2",
            "--chunk-size",
            "64",
            "--statistics",
            statistics,
        ]);
        assert!(out.status.success(), "{}", stderr_of(&out));
        assert!(arrangement_lines(&stderr_of(&out)).is_empty(), "{leg}: {}", stderr_of(&out));
    }
}
