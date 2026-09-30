//! `scripts/generate_perf_data.py` writes what `pg_dump` writes — the drift
//! guard for the benchmark generator.
//!
//! The generator's charter is that its output is "shaped closely enough that
//! pgdt can read it back" (its module docstring). Nothing else checks that a
//! generated column resolves the way real `pg_dump` output would, or that a
//! generated value round-trips through both schema modes identically.
//!
//! **Two assertions, because one does not imply the other.** A column whose
//! declared type pgdt cannot map stays `Utf8View` and is therefore rendered
//! identically by both schema modes — so the byte comparison alone is blind
//! to a type spelling `pg_dump` never writes. The resolution check catches
//! those; the byte comparison catches a value pgdt re-renders differently
//! from what the file holds.
//!
//! **Failed, not skipped, when `uv` is absent.** `uv` is the one tool
//! `mise.toml` pins, and `mise install` is the remedy — which is the point of
//! pinning tools at all (`docs/design/roadmap.md`, "A test may assume the
//! tools `mise` pins"). A skip here is invisible in a green suite and takes
//! the generator's only drift guard with it.

use std::path::Path;
use std::process::Command;

mod common;
use common::{require_uv, run, run_ok as pgdt, scripts_dir, stderr_of};

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

/// Every column of the generated file must resolve `Mapped`. `pgdt parse`
/// prints a summary line only when at least one did not, so its absence is
/// the assertion — and it covers every way a column can fail to be typed, the
/// unmapped declaration this test was written for and a census refusal on a
/// nested column alike (`ResolvedSchema::unmapped_count`).
fn assert_every_column_maps(dump: &Path, cache: &Path) {
    let dump = dump.to_str().unwrap();
    let cache = cache.to_str().unwrap();
    let report = pgdt(&["parse", "--source", dump, "--dtcache", cache]);
    if report.contains("columns unmapped") {
        // `--detail` is what names the columns and says why, which is the
        // half a failure needs.
        let detail = pgdt(&["info", "--dtcache", cache, "--detail"]);
        panic!(
            "the generator declared a column pgdt does not map, so a typed benchmark is \
             measuring it untyped.\n{detail}"
        );
    }
}

fn assert_modes_agree(dump: &Path) {
    let dump = dump.to_str().unwrap();
    let query = |mode: &str| {
        pgdt(&[
            "query",
            "--source",
            dump,
            "--table",
            "public.perf",
            "--dtcache",
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
            "`typed` and `strings` disagree at byte {at} — the generator wrote a value pgdt \
             re-renders differently, which no pg_dump output can hold.\n  typed:   {}\n  \
             strings: {}",
            window(&typed),
            window(&strings)
        );
    }
}

#[test]
fn the_perf_generator_writes_what_pgdt_reads_back() {
    require_uv("the generator's only drift guard");
    let dir = tempfile::tempdir().unwrap();

    let control = dir.path().join("control.sql");
    generate(&control, &[]);
    assert_every_column_maps(&control, &dir.path().join("control.dtcache"));
    assert_modes_agree(&control);

    // The nested columns, which are the reason the two flags exist and are
    // separate — and the shapes where a census refusal, not an unmapped
    // declaration, is what would silently drop a column to `Utf8View`.
    let nested = dir.path().join("nested.sql");
    generate(&nested, &["--arrays", "--composite"]);
    assert_every_column_maps(&nested, &dir.path().join("nested.dtcache"));
    assert_modes_agree(&nested);

    // `--composite` alone, because a recorded figure is taken on exactly that
    // input (`docs/design/measurements.md`, "A typed query over nested
    // columns") — the flags are independent, so covering the pair does not
    // cover either one by itself. `--arrays` alone backs no figure and is
    // left uncovered rather than guarded on principle.
    let composite = dir.path().join("composite.sql");
    generate(&composite, &["--composite"]);
    assert_every_column_maps(&composite, &dir.path().join("composite.dtcache"));
    assert_modes_agree(&composite);
}

/// `scripts/generate_pruning_bench.py` exists for what `statistics-pruning`
/// prices, so its guard is that the statistics that figure reads are the ones
/// a `parse` stores: `id` ascending with bounds in every group, `v_category`
/// with a dictionary in every group — its bytewise bounds are read in DataFusion's
/// semantics alone, never by `pgdt`'s (`docs/design/decisions.md`, "D79"), so
/// the equality leg is pruned by the dictionary it is named for — and
/// `v_smallint` bounded in every group with no dictionary in any, so the
/// figure's filter on it consults every group's bounds and skips none — and,
/// against a cache `parse --statistics-level metadata` wrote, asks for statistics and
/// consults none, returning the same rows, which is what the figure's leg over
/// that cache is refused without.
///
/// At the figure's own group size, `measure.GATHER_STATISTICS`'s, because the
/// last of those rests on how many rows a group holds: a group of at most a
/// dictionary's cap in rows keeps one, and a dictionary lacking the literal
/// skips the group. A whole number of MiB, so the last group is full.
#[test]
fn the_pruning_generator_writes_the_statistics_its_figure_prices() {
    require_uv("the pruning generator's only drift guard");
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("pruning.sql");
    let status = Command::new("uv")
        .current_dir(scripts_dir())
        .args(["run", "generate_pruning_bench.py", "--size-mb", "24", "--seed", "42"])
        .arg(&dump)
        .status()
        .expect("uv runs");
    assert!(status.success(), "generate_pruning_bench.py failed");
    let cache = dir.path().join("pruning.dtcache");
    let (dump, cache) = (dump.to_str().unwrap(), cache.to_str().unwrap());
    assert_every_column_maps(Path::new(dump), Path::new(cache));
    std::fs::remove_file(cache).unwrap();
    pgdt(&["parse", "--source", dump, "--dtcache", cache, "--row-group-size", "1048576"]);
    let detail = pgdt(&["info", "--dtcache", cache, "--detail"]);
    let line = |column: &str| {
        detail
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("{column}: over")))
            .unwrap_or_else(|| panic!("no statistics line for {column}:\n{detail}"))
            .to_string()
    };
    let groups = detail
        .split(" group(s)")
        .next()
        .and_then(|head| head.rsplit(' ').next())
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or_else(|| panic!("no group count:\n{detail}"));
    assert!(groups > 16, "{groups} group(s) prove little:\n{detail}");
    let id = line("id");
    assert!(id.contains("ascending"), "{id}");
    assert!(id.contains(&format!("bounds in {groups} of {groups} group(s)")), "{id}");
    let category = line("v_category");
    assert!(
        category.contains(&format!("dictionary in {groups} of {groups} group(s)")),
        "{category}"
    );
    let smallint = line("v_smallint");
    assert!(smallint.contains("unsorted"), "{smallint}");
    assert!(smallint.contains(&format!("bounds in {groups} of {groups} group(s)")), "{smallint}");
    assert!(smallint.contains("dictionary in 0 of"), "{smallint}");
    // The filter `measure.PRUNING_FILTERS` states, consulted and skipping
    // nothing: the note is printed only where statistics were consulted.
    let query = |cache: &str| {
        let queried = run(&[
            "query",
            "--source",
            dump,
            "--table",
            "public.perf",
            "--dtcache",
            cache,
            "--schema-mode",
            "typed",
            "--where",
            "v_smallint=0",
            "--statistics",
            "all",
        ]);
        assert!(queried.status.success(), "{}", stderr_of(&queried));
        stderr_of(&queried)
    };
    let queried = query(cache);
    let skipped_none = format!("rule out 0 of {groups} group(s)");
    assert!(queried.contains(&skipped_none), "{queried}");
    assert!(!queried.contains("reading stopped early"), "{queried}");
    assert!(
        queried.lines().any(|l| l.ends_with(" row(s)") || l.starts_with("no rows found for ")),
        "{queried}"
    );
}
