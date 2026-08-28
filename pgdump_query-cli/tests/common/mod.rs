//! What the CLI's integration tests need before they can assert anything:
//! how to run the binary, where the fixtures are, and where `scripts/` is.
//!
//! Same reason as `pgdump_query/tests/common/mod.rs`: each `tests/*.rs` file
//! is its own crate, so without a shared module each one carries its own copy.

// Each test binary compiles the whole module and uses a subset of it.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The built `pgdq` binary, ready to take arguments. Cargo hands us the exact
/// path, so this never resolves through `PATH` and never runs a stale install.
pub fn pgdq() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pgdq"))
}

/// Run `pgdq` and hand back the whole [`Output`] — for a test whose subject is
/// the exit status or the stderr text, not just what was printed.
pub fn run(args: &[&str]) -> Output {
    pgdq().args(args).output().expect("the pgdq binary runs")
}

/// Run `pgdq`, require it to succeed, and hand back stdout — for a test that
/// treats a non-zero exit as a failure of the test rather than as its subject.
pub fn run_ok(args: &[&str]) -> String {
    let out = run(args);
    assert!(out.status.success(), "pgdq {args:?} failed: {}", stderr_of(&out));
    stdout_of(&out)
}

pub fn stdout_of(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("pgdq writes UTF-8")
}

pub fn stderr_of(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("pgdq writes UTF-8")
}

/// A generated fixture, addressed relative to the `fixtures/` root
/// (`"16/edge_cases/create.sql"`).
pub fn fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures").join(relative)
}

/// `scripts/`, which is the working directory every `uv run` here needs.
pub fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts")
}

/// **Failed, not skipped, when `uv` is absent.** `uv` is the one tool
/// `mise.toml` pins, and `mise install` is the remedy — which is the point of
/// pinning tools at all (`docs/design/roadmap.md`, "A test may assume the
/// tools `mise` pins"). A skip here is invisible in a green suite and takes
/// the guarded thing's only drift guard with it.
pub fn require_uv(guarding: &str) {
    let ok = Command::new("uv").arg("--version").output().is_ok_and(|o| o.status.success());
    assert!(
        ok,
        "`uv` is not runnable, so {guarding} cannot run. \
         `mise.toml` pins it: run `mise install`."
    );
}

/// A private copy of a fixture in a fresh tempdir, named `name`, so a
/// colocated `.dqcache` can be written beside it without touching the
/// checked-in tree. The `TempDir` is returned because dropping it deletes the
/// copy.
pub fn sandboxed(relative: &str, name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join(name);
    std::fs::copy(fixture(relative), &dump).unwrap();
    (dir, dump)
}
