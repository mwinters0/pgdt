//! What the CLI's integration tests need before they can assert anything:
//! how to run the binary, where the fixtures are, and where `scripts/` is.
//!
//! Same reason as `pgdump_query/tests/common/mod.rs`: each `tests/*.rs` file
//! is its own crate, so without a shared module each one carries its own copy.

// Each test binary compiles the whole module and uses a subset of it.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The oracle: a misbehaving HTTP origin server, in-process. It sits here
/// rather than in its own test file for the reason this module exists at all — the remote
/// slices that follow each need it, and a `tests/*.rs` file cannot import
/// another one.
pub mod oracle;

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
    let out = run(args);
    assert!(out.status.success(), "pgdt {args:?} failed: {}", stderr_of(&out));
    stdout_of(&out)
}

pub fn stdout_of(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("pgdt writes UTF-8")
}

pub fn stderr_of(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("pgdt writes UTF-8")
}

/// A generated fixture, addressed relative to the `fixtures/` root
/// (`"16/edge_cases/create.sql"`).
pub fn fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures").join(relative)
}

/// Every real `pg_dump` output file the fixture generator produced, across all
/// six routine versions and every schema — read off the tree rather than
/// listed, so a new schema or flag set is swept the moment the generator
/// writes it. Includes the degenerate shapes: `data-only`, `schema-only`,
/// `inserts`/`column-inserts` (no `COPY` blocks at all), and `dumpall`
/// (concatenated, multi-`\connect`).
///
/// A deliberate second copy of `pgdump_query/tests/common/mod.rs`'s function of
/// the same name, for the reason stated at the top of this file: the two test
/// crates cannot share a module, and a path helper is what each one needs
/// before it can address a fixture at all.
pub fn all_fixtures() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures");
    let mut out = Vec::new();
    for version in std::fs::read_dir(&root).unwrap() {
        let version = version.unwrap().path();
        if !version.is_dir() {
            continue;
        }
        for schema in std::fs::read_dir(&version).unwrap() {
            let schema = schema.unwrap().path();
            if !schema.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(&schema).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_some_and(|e| e == "sql") {
                    out.push(path);
                }
            }
        }
    }
    assert!(!out.is_empty(), "fixture discovery found nothing — did the tree move?");
    out.sort();
    out
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

/// A private copy of a fixture in a fresh tempdir, named `name`, so a
/// colocated `.dtcache` can be written beside it without touching the
/// checked-in tree. The `TempDir` is returned because dropping it deletes the
/// copy.
pub fn sandboxed(relative: &str, name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join(name);
    std::fs::copy(fixture(relative), &dump).unwrap();
    (dir, dump)
}
