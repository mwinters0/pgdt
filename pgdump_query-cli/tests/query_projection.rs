//! `pgdq query --column` / `--no-columns` — projection's CLI surface
//! (`docs/design/architecture.md`, "Projection" and "CLI surface").
//!
//! `pgdump_query/tests/projection.rs` already holds what a projection *means*
//! against `table_stream`. What only the binary can say is the part these
//! pin: which flag combinations the parser refuses and at what exit status,
//! and what the rendered stream looks like — above all that a zero-column
//! query prints one line per row and no header, because
//! `--no-columns | wc -l` is the filtered-row-count idiom the phase's figure
//! is built on and an off-by-one there is silent.

use std::path::Path;
use std::process::{Command, Output};

mod common;
use common::{fixture, require_uv, run, scripts_dir, stderr_of, stdout_of};

/// `public.widgets` from the real `pg_dump` edge-case fixture: five rows over
/// `(id, name, description, is_active, created_at)`, one of which has a NULL
/// `description` and one an empty `name`.
fn widgets(extra: &[&str]) -> Output {
    let dump = fixture("16/edge_cases/default.sql");
    let mut args = vec![
        "query",
        "--source",
        dump.to_str().unwrap(),
        "--table",
        "public.widgets",
        "--dqcache",
        "none",
    ];
    args.extend_from_slice(extra);
    run(&args)
}

fn lines(out: &Output) -> Vec<String> {
    stdout_of(out).lines().map(str::to_string).collect()
}

/// The header and every row are cut to the projection, in the order the flags
/// give — which is not the file's order here, so this also says the CLI
/// passes the names through as a sequence rather than as a set.
#[test]
fn repeated_column_flags_cut_the_output_in_the_order_given() {
    let out = widgets(&["--column", "name", "--column", "id"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let lines = lines(&out);
    assert_eq!(lines[0], "name\tid");
    assert_eq!(lines[1], "alpha\t1");
    assert_eq!(lines.len(), 6, "a header and five rows: {lines:?}");
    assert!(stderr_of(&out).contains("5 row(s)"));
}

/// No flag is every column, unchanged — the projection code path must not
/// change what a plain `pgdq query` prints.
#[test]
fn no_projection_flag_still_prints_every_column() {
    let out = widgets(&[]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert_eq!(lines(&out)[0], "id\tname\tdescription\tis_active\tcreated_at");
}

/// **The row-count idiom.** A zero-column projection prints one empty line
/// per row and *no* header, so the line count is the row count; a header
/// would make it `rows + 1`. The count on stderr is still reported, which is
/// what says the "no rows found" branch is keyed off batches rather than off
/// the header having been printed.
#[test]
fn no_columns_prints_one_empty_line_per_row_and_no_header() {
    let out = widgets(&["--no-columns"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let lines = lines(&out);
    assert_eq!(lines.len(), 5, "five rows, no header: {lines:?}");
    assert!(lines.iter().all(|l| l.is_empty()), "{lines:?}");
    assert!(stderr_of(&out).contains("5 row(s)"), "{}", stderr_of(&out));
}

/// A table with no matching rows still says so rather than reporting zero —
/// the branch `--no-columns` moved off the header flag.
#[test]
fn a_table_that_yields_nothing_still_says_no_rows_were_found() {
    let out = widgets(&["--no-columns", "--filter", "name=nothing matches this"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert_eq!(stdout_of(&out), "");
    assert!(stderr_of(&out).contains("no rows found"), "{}", stderr_of(&out));
}

/// The filtered row count: the filter names a column the projection does not,
/// which is the shape the phase's figure floor row is built on.
#[test]
fn a_filter_may_name_a_column_no_column_flag_projects() {
    let out = widgets(&["--no-columns", "--filter", "is_active=t"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert_eq!(lines(&out).len(), 3, "three active widgets");
}

/// The two flags are mutually exclusive, and the parser is what refuses them
/// — exit 2 and a usage message, before the file is opened.
#[test]
fn no_columns_and_column_cannot_both_be_given() {
    let out = widgets(&["--no-columns", "--column", "id"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr_of(&out));
    assert!(stderr_of(&out).contains("cannot be used with"), "{}", stderr_of(&out));
}

/// A name the table does not carry, and a repeated name, are the library's
/// two refusals reaching the command line intact.
#[test]
fn an_unknown_or_repeated_column_name_is_refused() {
    let unknown = widgets(&["--column", "nope"]);
    assert!(!unknown.status.success());
    assert!(stderr_of(&unknown).contains("`nope`"), "{}", stderr_of(&unknown));

    let repeated = widgets(&["--column", "id", "--column", "id"]);
    assert!(!repeated.status.success());
    assert!(stderr_of(&repeated).contains("more than once"), "{}", stderr_of(&repeated));
}

/// **Projecting a column away is the per-column escape from a hard decode
/// failure**, and it is reachable from the CLI. `t_numeric.v_small` holds a
/// `NaN` its mapped `numeric(10,2)` cannot represent, so the unprojected
/// query fails; dropping that one column leaves every other column typed,
/// where `--schema-mode strings` would have untyped all of them.
#[test]
fn projecting_a_column_away_escapes_its_decode_failure() {
    let dump = fixture("16/types/default.sql");
    let query = |extra: &[&str]| {
        let mut args = vec![
            "query",
            "--source",
            dump.to_str().unwrap(),
            "--table",
            "public.t_numeric",
            "--dqcache",
            "none",
        ];
        args.extend_from_slice(extra);
        run(&args)
    };
    let whole = query(&[]);
    assert!(stderr_of(&whole).contains("v_small"), "{}", stderr_of(&whole));

    let projected = query(&["--column", "id", "--column", "v_typed"]);
    assert!(projected.status.success(), "{}", stderr_of(&projected));
    assert_eq!(lines(&projected)[0], "id\tv_typed");
    assert!(stderr_of(&projected).contains("7 row(s)"), "{}", stderr_of(&projected));
}

/// **The `projection-widths` figure's five command shapes actually run.**
///
/// `scripts/measure.py` registered them before the flags existed, on the
/// understanding that this slice would make them executable, and the failure
/// they were guarding against has no other guard: a flag the CLI does not
/// accept is discovered only when a sweep runs the query — minutes into a
/// figure, with the figure lost. So the flags come from `projection_flags`
/// itself rather than being transcribed here, and every width is run against
/// a real generated input.
///
/// **Failed, not skipped, when `uv` is absent** — see `common::require_uv`.
#[test]
fn the_registered_projection_widths_are_executable() {
    require_uv("the projection figure's command shapes");

    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("perf.sql");
    generate_perf(&dump, &["--arrays", "--composite"]);

    for (width, flags) in registered_widths() {
        let mut args = vec![
            "query",
            "--source",
            dump.to_str().unwrap(),
            "--table",
            "public.perf",
            "--dqcache",
            "none",
            "--schema-mode",
            "typed",
        ];
        args.extend(flags.split_whitespace());
        let out = run(&args);
        assert!(
            out.status.success(),
            "the {width}-column shape `{flags}` does not run: {}",
            stderr_of(&out)
        );
        // The width the table's row label claims, read off the output the
        // figure would have timed.
        let header = lines(&out).first().cloned().unwrap_or_default();
        let columns = if width == 0 { 0 } else { header.split('\t').count() };
        assert_eq!(columns, width, "the {width}-column shape printed header `{header}`");
    }
}

/// `PROJECTION_WIDTHS` and the flags `measure.py` derives for each, read out
/// of the harness rather than restated — a width added there is then run here
/// without anyone remembering to.
fn registered_widths() -> Vec<(usize, String)> {
    let out = Command::new("uv")
        .args([
            "run",
            "python",
            "-c",
            "import measure\n\
             for w in measure.PROJECTION_WIDTHS:\n\
             \x20   print(f'{w}\\t{measure.projection_flags(w)}')",
        ])
        .current_dir(scripts_dir())
        .output()
        .expect("uv runs");
    assert!(out.status.success(), "reading the registered widths failed: {}", stderr_of(&out));
    let widths: Vec<(usize, String)> = stdout_of(&out)
        .lines()
        .map(|line| {
            let (w, flags) = line.split_once('\t').expect("width and flags are tab-separated");
            (w.parse().expect("the width is a number"), flags.to_string())
        })
        .collect();
    assert!(!widths.is_empty(), "measure.py registers at least one projection width");
    widths
}

/// A ~2 MiB perf dump — the same size and reasoning as
/// `perf_generator_fidelity.rs`: small enough to generate per test run, large
/// enough that every column carries real values.
fn generate_perf(out: &Path, extra: &[&str]) {
    let status = Command::new("uv")
        .current_dir(scripts_dir())
        .args(["run", "generate_perf_data.py", "--size-mb", "2", "--seed", "42"])
        .args(extra)
        .arg(out)
        .status()
        .expect("uv runs");
    assert!(status.success(), "generate_perf_data.py {extra:?} failed");
}
