//! `--strict-identity` on `pgdt info`: one flag, one meaning on every command
//! (`docs/design/decisions.md`, "D21").
//!
//! `info` is the command that *reports* — it reads the whole `CacheStatus`
//! rather than going through `CacheMode::load`, where the refusal for the two
//! commands that scan lives — so the two cases below are the ones nothing else
//! in the tree can observe: that a bound-but-moved signal stops the report, and
//! that the flag is a usage error where there is no source to bind it to.
//! `pgdump_query/tests/identity.rs` pins what the refusal itself compares; this
//! file pins that `info` asks for it.
//!
//! One selection meaning one thing includes how the refusal *reads*, so the
//! third case pins the same refusal on a command that scans: the source's name
//! leads it on both, the classifier being shared rather than copied
//! (`main.rs`'s `about_the_source`).

use std::time::{Duration, SystemTime};

mod common;
use common::{fixture, run, run_ok, stderr_of};

/// A writable copy of a real dump, so a test can move its modification time.
fn sandboxed() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("dump.sql");
    std::fs::copy(fixture("16/edge_cases/create.sql"), &dump).unwrap();
    (dir, dump)
}

/// Move the dump's modification time an hour on, leaving every byte alone —
/// the ambiguous case the default is advisory about and `time` refuses.
fn touch_forward(dump: &std::path::Path) {
    let future = SystemTime::now() + Duration::from_secs(3600);
    std::fs::File::options().write(true).open(dump).unwrap().set_modified(future).unwrap();
}

/// `pgdt info --strict-identity=time` refuses a cache whose modification
/// signal has moved, and says which cache and why — a refusal *is* the
/// explanation, so the reporting command loses nothing by being able to stop.
/// Without the flag the same run reports.
#[test]
fn info_refuses_a_moved_signal_under_strict_time_and_reports_without_it() {
    let (_dir, dump) = sandboxed();
    let dump = dump.to_str().unwrap();
    run_ok(&["parse", "--source", dump]);

    // Strict first: a `parse` re-run would re-record the mtime it now sees,
    // and `info` never writes, so the order here is only about the cache
    // staying the one `parse` wrote.
    run_ok(&["info", "--source", dump, "--strict-identity=time"]);

    touch_forward(std::path::Path::new(dump));
    let refused = run(&["info", "--source", dump, "--strict-identity=time"]);
    assert!(!refused.status.success(), "`time` binds the modification time on `info` too");
    let said = stderr_of(&refused);
    assert!(said.contains("dump.sql.dtcache"), "it names the cache it refused: {said}");
    assert!(said.contains(dump), "and the source it refused it about: {said}");
    assert!(said.contains("modification time has moved"), "and why: {said}");
    assert!(
        said.contains("the cache recorded") && said.contains("the source now reports"),
        "and what it saw, not only that it looked: {said}"
    );

    // The default is unchanged: the same moved signal is reported.
    let reported = run_ok(&["info", "--source", dump]);
    assert!(
        reported.contains("mtime has changed since the cache was saved"),
        "without the flag the moved signal is a diagnostic: {reported}"
    );
}

/// Cache-only `info` has no source at all, so its identity is *null* rather
/// than absent: asking for a guarantee about a source nobody named is
/// malformed, not a refusal. clap answers it, before anything is opened.
#[test]
fn strict_identity_without_a_source_is_a_usage_error() {
    let (_dir, dump) = sandboxed();
    let dump = dump.to_str().unwrap();
    run_ok(&["parse", "--source", dump]);
    let cache = format!("{dump}.dtcache");

    // The same invocation is fine without the flag — cache-only mode is the
    // supported entry point, and it is the *flag* that is refused.
    run_ok(&["info", "--dtcache", &cache]);

    let refused = run(&["info", "--dtcache", &cache, "--strict-identity=time"]);
    assert!(!refused.status.success(), "the flag needs a source to bind");
    let said = stderr_of(&refused);
    assert!(said.contains("--source"), "and the usage error names what is missing: {said}");
}

/// The same refusal on a command that *scans*, reading the same way: a cache
/// path is derived on a remote source and says nothing about which dump was
/// asked for, so the source's name leads the sentence wherever the refusal
/// surfaces, not only on the command that happened to be written first.
#[test]
fn a_scanning_command_names_the_source_in_a_strict_identity_refusal() {
    let (_dir, dump) = sandboxed();
    let dump = dump.to_str().unwrap();
    run_ok(&["parse", "--source", dump]);
    touch_forward(std::path::Path::new(dump));

    let refused =
        run(&["query", "--source", dump, "--table", "public.widgets", "--strict-identity=time"]);
    assert!(!refused.status.success(), "`time` binds the modification time on `query` too");
    let said = stderr_of(&refused);
    assert!(said.contains(&format!("Error: {dump}: the cache at")), "{said}");
    assert!(said.contains("dump.sql.dtcache"), "the cache is still named: {said}");
}
