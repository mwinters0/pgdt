//! `scripts/generate_perf_data.py` writes what `pg_dump` writes — the drift
//! guard for the benchmark generator, landed with `M10`
//! (`docs/design/roadmap.md`, out-of-band ledger).
//!
//! The generator's charter is that its output is "shaped closely enough that
//! pgdq can read it back" (its module docstring). Nothing checked that, and
//! three infidelities survived in it until someone read its output beside a
//! real dump: three type spellings `pg_dump` never writes, fractional seconds
//! PostgreSQL would have trimmed, and a float64 `repr()` in a `real` column.
//! A benchmark for the *typed* path was measuring 13 of its 16 scalar columns
//! untyped.
//!
//! **Two assertions, because one does not imply the other.** A column whose
//! declared type pgdq cannot map stays `Utf8View` and is therefore rendered
//! identically by both schema modes — so the byte comparison alone is blind
//! to the spellings, which is exactly how they survived. The resolution check
//! catches those; the byte comparison catches a value pgdq re-renders
//! differently from what the file holds.
//!
//! **Failed, not skipped, when `uv` is absent.** `uv` is the one tool
//! `mise.toml` pins, and `mise install` is the remedy — which is the point of
//! pinning tools at all (`docs/design/roadmap.md`, "A test may assume the
//! tools `mise` pins"). A skip here is invisible in a green suite and takes
//! the generator's only drift guard with it, which is how three infidelities
//! survived in the first place.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts")
}

fn require_uv() {
    let ok = Command::new("uv").arg("--version").output().is_ok_and(|o| o.status.success());
    assert!(
        ok,
        "`uv` is not runnable, so the generator's only drift guard cannot run. \
         `mise.toml` pins it: run `mise install`."
    );
}

/// A ~2 MiB dump — a few hundred rows, which is enough for every value shape
/// the generator draws from to appear many times over, and small enough that
/// generating two of them per test run is not felt.
fn generate(out: &Path, extra: &[&str]) {
    let mut cmd = Command::new("uv");
    cmd.current_dir(scripts_dir())
        .args(["run", "generate_perf_data.py", "--size-mb", "2", "--seed", "42"])
        .args(extra)
        .arg(out);
    let status = cmd.status().expect("uv runs");
    assert!(status.success(), "generate_perf_data.py {extra:?} failed");
}

fn pgdq(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_pgdq")).args(args).output().expect("pgdq runs");
    assert!(out.status.success(), "pgdq {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("pgdq writes UTF-8")
}

/// Every column of the generated file must resolve `Mapped`. `pgdq parse`
/// prints a summary line only when at least one did not, so its absence is
/// the assertion — and it covers every way a column can fail to be typed, the
/// unmapped declaration this test was written for and a census refusal on a
/// nested column alike (`ResolvedSchema::unmapped_count`).
fn assert_every_column_maps(dump: &Path, cache: &Path) {
    let dump = dump.to_str().unwrap();
    let cache = cache.to_str().unwrap();
    let report = pgdq(&["parse", "--source", dump, "--dqcache", cache]);
    if report.contains("columns unmapped") {
        // `--verbose` is what names the columns and says why, which is the
        // half a failure needs.
        let detail = pgdq(&["info", "--dqcache", cache, "--verbose"]);
        panic!(
            "the generator declared a column pgdq does not map, so a typed benchmark is \
             measuring it untyped.\n{detail}"
        );
    }
}

fn assert_modes_agree(dump: &Path) {
    let dump = dump.to_str().unwrap();
    let query = |mode: &str| {
        pgdq(&[
            "query",
            "--source",
            dump,
            "--table",
            "public.perf",
            "--dqcache",
            "none",
            "--schema-mode",
            mode,
        ])
    };
    let typed = query("typed");
    let strings = query("strings");
    assert!(!typed.is_empty(), "the generated file has rows");
    if typed != strings {
        // Report the first differing byte rather than two multi-megabyte
        // strings, which no test runner renders usefully.
        let at = typed
            .as_bytes()
            .iter()
            .zip(strings.as_bytes())
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| typed.len().min(strings.len()));
        let window = |s: &str| {
            let lo = at.saturating_sub(80);
            s.get(lo..(at + 80).min(s.len())).unwrap_or("").to_string()
        };
        panic!(
            "`typed` and `strings` disagree at byte {at} — the generator wrote a value pgdq \
             re-renders differently, which no pg_dump output can hold.\n  typed:   {}\n  \
             strings: {}",
            window(&typed),
            window(&strings)
        );
    }
}

#[test]
fn the_perf_generator_writes_what_pgdq_reads_back() {
    require_uv();
    let dir = tempfile::tempdir().unwrap();

    let control = dir.path().join("control.sql");
    generate(&control, &[]);
    assert_every_column_maps(&control, &dir.path().join("control.dqcache"));
    assert_modes_agree(&control);

    // The nested columns, which are the reason the two flags exist and are
    // separate — and the shapes where a census refusal, not an unmapped
    // declaration, is what would silently drop a column to `Utf8View`.
    let nested = dir.path().join("nested.sql");
    generate(&nested, &["--arrays", "--composite"]);
    assert_every_column_maps(&nested, &dir.path().join("nested.dqcache"));
    assert_modes_agree(&nested);

    // `--composite` alone, because a recorded figure is taken on exactly that
    // input (`docs/design/measurements.md`, "A typed query over nested
    // columns") — the flags are independent, so covering the pair does not
    // cover either one by itself. `--arrays` alone backs no figure and is
    // left uncovered rather than guarded on principle.
    let composite = dir.path().join("composite.sql");
    generate(&composite, &["--composite"]);
    assert_every_column_maps(&composite, &dir.path().join("composite.dqcache"));
    assert_modes_agree(&composite);
}
