//! `pgdq`'s status output — the `tracing` lines `parse`, `info` and `query`
//! write to stderr for the two phases worth watching a long run for: an
//! `.xz` file's seek-table walk, and the scan itself starting and finishing
//! (`docs/design/architecture.md`, "Status output"). What only the binary can say is that
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
//! against the ambiguous substring would have passed the day this file's
//! first defect shipped.

use std::path::{Path, PathBuf};
use std::process::Command;

use pgdump_query::{
    DumpIndex, DumpMetadata, LocalFileSource, ScanOptions, Span, SpanBody, build_index, cache,
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

/// A fresh `parse` of a plain file states the arrangement once and says when
/// each of its two passes finishes — the pair the manual shows
/// (`docs/manual/dump-inspection.md`, "Status on stderr").
#[test]
fn a_fresh_parse_announces_the_preamble_pass_and_the_scan_separately() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("out.dqcache");
    let out = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dqcache",
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
    let cache = dir.path().join("out.dqcache");
    let out = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dqcache",
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
        run(&["parse", "--source", dump.to_str().unwrap(), "--dqcache", cache.to_str().unwrap()]);
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

/// `--jobs`/`--parallel-memory` are the arrangement a "scan started" line
/// names — stated once, in the library's own vocabulary, not the flags', and
/// as a plain byte count rather than `Option`'s debug spelling.
#[test]
fn scan_started_names_jobs_and_the_stated_memory_budget_as_a_quantity() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("out.dqcache");
    let out = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dqcache",
        cache.to_str().unwrap(),
        "--jobs",
        "4",
        "--parallel-memory",
        "268435456",
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    let started = main_scan_lines(&stderr)
        .into_iter()
        .find(|l| l.contains("started"))
        .unwrap_or_else(|| panic!("no main \"scan started\" line: {stderr}"));
    assert!(started.contains("jobs=4"), "{started}");
    // A bare quantity — not `Some(268435456)`, `Option`'s debug spelling
    // reaching a user-facing line.
    assert!(started.contains("memory_bytes=268435456"), "{started}");
    assert!(!started.contains("Some("), "{started}");
    assert!(!started.contains("None"), "{started}");
}

/// **The default case of the same defect.** At the shipped defaults nobody
/// stated a byte budget, but a real one still governs every read
/// (`DEFAULT_MEMORY_BUDGET`) — `None` said nothing a reader could act on;
/// the fix states the number actually in force and says in words that it is
/// the default rather than something asked for.
#[test]
fn scan_started_names_the_default_memory_budget_when_none_was_stated() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("out.dqcache");
    let out = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dqcache",
        cache.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    let started = main_scan_lines(&stderr)
        .into_iter()
        .find(|l| l.contains("started"))
        .unwrap_or_else(|| panic!("no main \"scan started\" line: {stderr}"));
    assert!(started.contains("jobs=1"), "{started}");
    assert!(started.contains("memory_bytes=67108864 (default)"), "{started}");
}

/// **`(default)` says nobody asked, not that nobody could ask.** The serial
/// path carries a stated budget like any other, so `--parallel-memory` over a
/// plain file — whose own recommendation is the serial path — prints the number
/// that was asked for, bare, which is what
/// makes the flag's own recourse ("raise the memory budget") readable from the
/// log without a second worker being stated beside it
/// (`docs/design/architecture.md`, "Execution model and API surface").
#[test]
fn a_stated_memory_budget_at_a_serial_job_count_is_not_marked_default() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("out.dqcache");
    let out = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dqcache",
        cache.to_str().unwrap(),
        "--parallel-memory",
        "268435456",
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    let started = main_scan_lines(&stderr)
        .into_iter()
        .find(|l| l.contains("started"))
        .unwrap_or_else(|| panic!("no main \"scan started\" line: {stderr}"));
    assert!(started.contains("jobs=1"), "{started}");
    assert!(started.contains("memory_bytes=268435456"), "{started}");
    assert!(!started.contains("(default)"), "a stated budget is not the default: {started}");
    assert!(!started.contains("None"), "{started}");
    assert!(!started.contains("Some("), "{started}");
}

/// No line anywhere ever reaches a reader as `Option`'s own debug spelling —
/// a blanket guard, over every command this file exercises, for the shape of
/// defect 2 recurring at a call site a narrower test does not reach.
#[test]
fn no_status_line_ever_prints_nones_or_somes() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("out.dqcache");
    let out = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dqcache",
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
    let cache = dir.path().join("out.dqcache");
    let first = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dqcache",
        cache.to_str().unwrap(),
    ]);
    assert!(first.status.success(), "{}", stderr_of(&first));
    assert!(!main_scan_lines(&stderr_of(&first)).is_empty());

    let second = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dqcache",
        cache.to_str().unwrap(),
    ]);
    assert!(second.status.success(), "{}", stderr_of(&second));
    let stderr = stderr_of(&second);
    assert!(preamble_lines(&stderr).is_empty(), "{stderr}");
    assert!(main_scan_lines(&stderr).is_empty(), "{stderr}");
}

/// `info` never scans, so it never announces either pass — its output is
/// read entirely from the cache
/// (`docs/design/architecture.md`, "CLI surface").
#[test]
fn info_announces_no_scan() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("out.dqcache");
    let parsed = run(&[
        "parse",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dqcache",
        cache.to_str().unwrap(),
    ]);
    assert!(parsed.status.success(), "{}", stderr_of(&parsed));

    let info = run(&[
        "info",
        "--source",
        plain_dump().to_str().unwrap(),
        "--dqcache",
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
/// (`XzSource::with_table`) and earns no line — the walk this phase names is
/// the one a cached table exists to skip (`docs/design/architecture.md`,
/// "The compressed source").
#[test]
fn the_seek_table_walk_is_announced_once_and_only_on_a_fresh_open() {
    let (_dir, xz_path) = seekable_xz();
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = cache_dir.path().join("out.dqcache");

    let first = run(&[
        "parse",
        "--source",
        xz_path.to_str().unwrap(),
        "--dqcache",
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
        "--dqcache",
        cache.to_str().unwrap(),
    ]);
    assert!(second.status.success(), "{}", stderr_of(&second));
    assert!(!stderr_of(&second).contains("seek table build"), "{}", stderr_of(&second));
}

/// **The shipped default is the source's, and the status line is where a user
/// sees which one they got.** A plain `parse` with no `--jobs` says `jobs=1`
/// (above); the same command over an `.xz` file says the cores this process was
/// given, because decode is the one shape that scales
/// (`docs/design/architecture.md`, "Execution model and API surface").
///
/// Asserted against `available_parallelism()` rather than a literal — the count
/// is the machine's, and on a one-CPU runner it legitimately *is* 1, which is
/// why the plain leg is asserted beside it rather than the `.xz` leg alone: what
/// this pins is that the two commands can differ, and by which number.
#[test]
fn an_xz_parse_defaults_to_the_cores_and_a_plain_one_to_serial() {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let (_dir, xz_path) = seekable_xz();
    let cache_dir = tempfile::tempdir().unwrap();

    let started_of = |source: &str, cache: PathBuf| -> String {
        let out = run(&["parse", "--source", source, "--dqcache", cache.to_str().unwrap()]);
        assert!(out.status.success(), "{}", stderr_of(&out));
        let stderr = stderr_of(&out);
        main_scan_lines(&stderr)
            .into_iter()
            .find(|l| l.contains("started"))
            .unwrap_or_else(|| panic!("no main \"scan started\" line: {stderr}"))
            .to_string()
    };

    let compressed = started_of(xz_path.to_str().unwrap(), cache_dir.path().join("xz.dqcache"));
    assert!(compressed.contains(&format!("jobs={cores}")), "{compressed}");

    let plain = started_of(plain_dump().to_str().unwrap(), cache_dir.path().join("plain.dqcache"));
    assert!(plain.contains("jobs=1"), "{plain}");
}
