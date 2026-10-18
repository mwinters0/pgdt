//! What the CLI's integration tests need before they can assert anything:
//! how to run the binary, where the fixtures are, and where `scripts/` is.
//!
//! Each `tests/*.rs` file is its own crate, so without a shared module each one
//! carries its own copy. The fixture vocabulary is `pgdt-fixtures`'s,
//! re-exported here.

// Each test binary compiles the whole module and uses a subset of it.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The oracle: a misbehaving HTTP origin server, in-process. It sits here
/// rather than in its own test file for the reason this module exists at all — the remote
/// slices that follow each need it, and a `tests/*.rs` file cannot import
/// another one.
pub mod oracle;

#[allow(unused_imports)]
pub use pgdt_fixtures::*;

/// The built `pgdt` binary, ready to take arguments. Cargo hands us the exact
/// path, so this never resolves through `PATH` and never runs a stale install.
pub fn pgdt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pgdt"))
}

/// Run `pgdt` and hand back the whole [`Output`] — for a test whose subject is
/// the exit status or the stderr text, not just what was printed.
pub fn run(args: &[&str]) -> Output {
    pgdt().args(args).output().expect("the pgdt binary runs")
}

/// Run `pgdt`, require it to succeed, and hand back stdout — for a test that
/// treats a non-zero exit as a failure of the test rather than as its subject.
pub fn run_ok(args: &[&str]) -> String {
    succeeded(args, run(args))
}

/// [`run`] with `dir` as the working directory — which is where a remote
/// dump's default cache goes, there being no place beside the object for one
/// to sit.
pub fn run_in(dir: impl AsRef<Path>, args: &[&str]) -> Output {
    pgdt().current_dir(dir).args(args).output().expect("the pgdt binary runs")
}

/// [`run_ok`] with `dir` as the working directory ([`run_in`]).
pub fn run_ok_in(dir: impl AsRef<Path>, args: &[&str]) -> String {
    succeeded(args, run_in(dir, args))
}

/// `out`'s stdout, once its status says `pgdt` succeeded. **A failure shows
/// both streams**: `pgdt sql` prints its errors to stdout, as upstream's
/// `datafusion-cli` does, and every other command to stderr, so a message
/// showing only one of them is empty for part of the binary.
fn succeeded(args: &[&str], out: Output) -> String {
    assert!(
        out.status.success(),
        "pgdt {args:?} failed\n--- stdout\n{}\n--- stderr\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    stdout_of(&out)
}

pub fn stdout_of(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("pgdt writes UTF-8")
}

pub fn stderr_of(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("pgdt writes UTF-8")
}

/// `scripts/`, which is the working directory every `uv run` here needs.
pub fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts")
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

/// A `parse` listing without the one line that names the cache path, which
/// differs between two runs by construction — so a remote run's report and a
/// local one's can be compared whole.
pub fn strip_cache_line(output: &str) -> String {
    output
        .lines()
        .filter(|line| !line.starts_with("wrote cache to "))
        .collect::<Vec<_>>()
        .join("\n")
}
