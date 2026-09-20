//! `pgdt query --where` — the boolean expression grammar's CLI surface
//! (`docs/design/decisions.md`, "D60", and "Predicates").
//!
//! What an expression *means* is pinned against the library in
//! `pgdump_query/tests/batch.rs` and `pgdump_query/src/predicate.rs`; the
//! grammar's own shape is unit-tested in `pgdt/src/where_expr.rs`. What only
//! the binary can say is that the flag reaches the evaluator, that `OR` and
//! `NOT` change which rows come back, that `--where` and `--filter` compose,
//! and that no string means one thing under one flag and something else under
//! the other — a string that reads as structure is refused by both, and
//! quoting is the remedy under both.

use std::process::{Command, Output};

mod common;
use common::{fixture, require_uv, run, scripts_dir, stderr_of, stdout_of};

/// `public.widgets` from the real `pg_dump` edge-case fixture: five rows over
/// `(id, name, description, is_active, created_at)`. Row 2 has a NULL
/// `description`, row 3 a NULL `created_at`, row 5 a NULL `is_active`.
fn widgets(extra: &[&str]) -> Output {
    let dump = fixture("16/edge_cases/default.sql");
    let mut args = vec![
        "query",
        "--source",
        dump.to_str().unwrap(),
        "--table",
        "public.widgets",
        "--dtcache",
        "none",
        "--column",
        "name",
    ];
    args.extend_from_slice(extra);
    run(&args)
}

/// The `name` of every row surviving `extra`, in file order, without the
/// header line.
fn kept(extra: &[&str]) -> Vec<String> {
    let out = widgets(extra);
    assert!(out.status.success(), "{}", stderr_of(&out));
    stdout_of(&out).lines().skip(1).map(str::to_string).collect()
}

/// **`OR` is what the flag exists for**, and the terms either side are the
/// same terms `--filter` takes.
#[test]
fn a_disjunction_keeps_the_rows_of_either_side() {
    assert_eq!(kept(&["--where", "name=alpha or name=beta"]), ["alpha", "beta"]);
}

/// Parens override precedence, and the result is not the one the unparenthesised
/// reading gives — which is what says the grouping reached the evaluator
/// rather than being parsed and dropped.
#[test]
fn parens_group_and_change_the_answer() {
    assert_eq!(
        kept(&["--where", "(name=alpha or name=beta) and is_active=f"]),
        ["beta"],
        "the OR is evaluated first"
    );
    assert_eq!(
        kept(&["--where", "name=alpha or name=beta and is_active=f"]),
        ["alpha", "beta"],
        "unparenthesised, AND binds tighter"
    );
}

/// **`NOT` and `IS DISTINCT FROM` differ on the NULL row, and that is why
/// both exist.** `is_active` is `t` on rows 1, 3 and 4, `f` on row 2 and NULL
/// on row 5: `NOT is_active=t` is `NOT UNKNOWN` there, which is `UNKNOWN`, so
/// the row is dropped; `IS DISTINCT FROM` counts the NULL as a value and
/// keeps it.
#[test]
fn negation_drops_the_null_row_and_is_distinct_from_keeps_it() {
    assert_eq!(kept(&["--where", "not is_active=t"]), ["beta"]);
    assert_eq!(kept(&["--where", "is_active is distinct from t"]), ["beta", ""]);
}

/// The two flags compose, and the composition is a conjunction: the
/// expression and every term must hold.
#[test]
fn where_and_filter_are_anded() {
    assert_eq!(kept(&["--where", "name=alpha or name=beta"]), ["alpha", "beta"]);
    assert_eq!(
        kept(&["--where", "name=alpha or name=beta", "--filter", "is_active=t"]),
        ["alpha"],
        "the term narrows the expression"
    );
}

/// **No string means one thing under one flag and something else under the
/// other.** Both flags refuse this one — `--where` because `beta` is not a
/// term, `--filter` because an unquoted `AND` is what `--where` reads as
/// structure — and neither opens the dump to do it. This is the property the
/// two flags exist to have between them, and it cannot be seen from either
/// alone.
#[test]
fn a_string_that_reads_as_structure_is_refused_under_both_flags() {
    let refused = widgets(&["--where", "name=alpha and beta"]);
    assert!(!refused.status.success());
    let stderr = stderr_of(&refused);
    assert!(stderr.contains("--where"), "{stderr}");
    assert!(stderr.contains("`beta`"), "{stderr}");

    let also_refused = widgets(&["--filter", "name=alpha and beta"]);
    assert!(!also_refused.status.success(), "{}", stdout_of(&also_refused));
    let stderr = stderr_of(&also_refused);
    assert!(stderr.contains("`AND`"), "{stderr}");
    assert!(stderr.contains("--where"), "{stderr}");
}

/// **The remedy is quoting, and it is one the grammar already taught.** Row
/// 4's description holds both a paren and a bare `not`, so it is exactly the
/// value the refusal reaches — and quoted it is the equality it always was.
#[test]
fn a_value_that_holds_structure_is_asked_for_quoted() {
    let value = "contains a COPY-like phrase: COPY public.widgets (id, name) TO stdout; \
                 -- not a real directive";
    let quoted = format!("description='{value}'");
    assert_eq!(kept(&["--filter", &quoted]), ["delta"]);

    let bare = format!("description={value}");
    let refused = widgets(&["--filter", &bare]);
    assert!(!refused.status.success(), "{}", stdout_of(&refused));
    assert!(stderr_of(&refused).contains("`(`"), "{}", stderr_of(&refused));
}

/// **The refusal is narrow**, which is the half a rule restated beside the
/// tokenizer would have got wrong. Only whitespace or a paren can put a
/// keyword next to structure, so an unquoted multi-word value still needs no
/// quotes and the cheap `--filter` spelling survives.
#[test]
fn a_value_with_no_reserved_spelling_still_needs_no_quotes() {
    assert_eq!(kept(&["--filter", "description=a simple widget"]), ["alpha"]);
    assert_eq!(kept(&["--filter", "name=nota"]), Vec::<String>::new());
}

/// A structural fault is a usage fault: reported without a scan, naming the
/// whole expression.
#[test]
fn a_structural_fault_is_refused_before_the_dump_is_opened() {
    let out = widgets(&["--where", "(name=alpha"]);
    assert!(!out.status.success());
    let stderr = stderr_of(&out);
    assert!(stderr.contains("never closed"), "{stderr}");
    assert!(stderr.contains("(name=alpha"), "{stderr}");
}

/// The `IS NOT NULL` form survives the tokenizer: its `NOT` belongs to the
/// term, not to the expression, so this is a null test and not a negation of
/// a leaf called `NULL`.
#[test]
fn an_is_not_null_term_is_not_split_at_its_not() {
    assert_eq!(kept(&["--where", "description is not null"]), ["alpha", "gamma", "delta", ""]);
}

/// `IS DISTINCT FROM` reaches `--filter` too — one term grammar, two flags —
/// and its answer there is the same as inside an expression.
#[test]
fn the_filter_flag_takes_the_worded_operators() {
    assert_eq!(kept(&["--filter", "is_active is distinct from t"]), ["beta", ""]);
    assert_eq!(
        kept(&["--filter", "is_active is not distinct from t"]),
        ["alpha", "gamma", "delta"]
    );
}

/// **The `predicate-terms` figure's six command shapes actually run, and none
/// of them keeps a row.**
///
/// The same guard `query_projection.rs` puts on the projection figure, plus
/// the property this figure's subtraction rests on. Every shape asks an
/// all-false disjunction, so `Or` evaluates every term on every row and
/// nothing survives to be decoded, built or rendered — which is what makes the
/// difference between two of the table's rows the predicate alone. A literal
/// that ever matched would put the whole emit path inside one row of the
/// table and nothing else would notice
/// (`docs/design/measurements.md`, "What a filter term costs").
///
/// The expressions come from `predicate_expr` rather than being transcribed,
/// so a shape added to the harness is run here without anyone remembering to.
///
/// **Failed, not skipped, when `uv` is absent** — see `common::require_uv`.
#[test]
fn the_registered_predicate_shapes_are_executable() {
    require_uv("the predicate figure's command shapes");

    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("perf.sql");
    let status = Command::new("uv")
        .current_dir(scripts_dir())
        .args(["run", "generate_perf_data.py", "--size-mb", "2", "--seed", "42"])
        .arg(&dump)
        .status()
        .expect("uv runs");
    assert!(status.success(), "generating the perf input failed");

    for (shape, expr) in registered_predicates() {
        let out = run(&[
            "query",
            "--source",
            dump.to_str().unwrap(),
            "--table",
            "public.perf",
            "--dtcache",
            "none",
            "--schema-mode",
            "strings",
            "--where",
            &expr,
        ]);
        assert!(
            out.status.success(),
            "the {shape} shape `{expr}` does not run: {}",
            stderr_of(&out)
        );
        assert_eq!(
            stdout_of(&out).lines().count(),
            0,
            "the {shape} shape `{expr}` kept rows, so its reading is not the predicate alone"
        );
    }
}

/// `PREDICATE_SHAPES` and the expression `measure.py` derives for each, read
/// out of the harness rather than restated.
fn registered_predicates() -> Vec<(String, String)> {
    let out = Command::new("uv")
        .args([
            "run",
            "python",
            "-c",
            "import measure\n\
             for s in measure.PREDICATE_SHAPES:\n\
             \x20   print(f'{s}\\t{measure.predicate_expr(s)}')",
        ])
        .current_dir(scripts_dir())
        .output()
        .expect("uv runs");
    assert!(out.status.success(), "reading the registered predicates failed: {}", stderr_of(&out));
    let shapes: Vec<(String, String)> = stdout_of(&out)
        .lines()
        .map(|line| {
            let (shape, expr) = line.split_once('\t').expect("shape and expression are tabbed");
            (shape.to_string(), expr.to_string())
        })
        .collect();
    assert!(!shapes.is_empty(), "measure.py registers at least one predicate shape");
    shapes
}
