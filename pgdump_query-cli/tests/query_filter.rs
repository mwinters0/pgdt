//! `pgdq query --filter`, repeated — the conjunction's CLI surface
//! (`docs/design/architecture.md`, "Predicates" and "CLI surface").
//!
//! What a conjunction *means* is pinned against the library in
//! `pgdump_query/tests/batch.rs`. What only the binary can say is that the
//! flag accumulates rather than replacing, that each repetition is parsed as
//! its own term, and that a fault in any term reaches the user naming that
//! term's column.

use std::process::Output;

mod common;
use common::{fixture, run, stderr_of, stdout_of};

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
        "--dqcache",
        "none",
        "--no-columns",
    ];
    args.extend_from_slice(extra);
    run(&args)
}

/// Rows surviving `extra`, counted through the `--no-columns | wc -l` idiom.
fn kept(extra: &[&str]) -> usize {
    let out = widgets(extra);
    assert!(out.status.success(), "{}", stderr_of(&out));
    stdout_of(&out).lines().count()
}

/// **The conjunction.** Each term is satisfied by a different four- or
/// three-row subset; together they keep only the two rows in both. Asserting
/// each term alone in the same test is what says the flag accumulates —
/// a second `--filter` that replaced the first would give 4, and one that
/// were ignored would give 3.
#[test]
fn repeated_filter_flags_are_anded() {
    assert_eq!(kept(&["--filter", "is_active=t"]), 3);
    assert_eq!(kept(&["--filter", "created_at IS NOT NULL"]), 4);
    assert_eq!(
        kept(&["--filter", "is_active=t", "--filter", "created_at IS NOT NULL"]),
        2,
        "only the rows in both"
    );
}

/// No `--filter` at all is every row: the conjunction's empty case needs no
/// spelling of its own and must not change what a plain query prints.
#[test]
fn no_filter_flag_yields_every_row() {
    assert_eq!(kept(&[]), 5);
}

/// Terms are independent comparisons — nothing folds two on one column
/// together — so a pair no row can satisfy is not an error, it is a query
/// with no rows.
#[test]
fn a_contradictory_pair_of_terms_finds_no_rows() {
    let out = widgets(&["--filter", "name=alpha", "--filter", "name=beta"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert_eq!(stdout_of(&out), "");
    assert!(stderr_of(&out).contains("no rows found"), "{}", stderr_of(&out));
}

/// Every term is parsed before the dump is opened, so a malformed one is
/// reported as a usage fault rather than after a scan.
#[test]
fn a_malformed_term_is_refused() {
    let out = widgets(&["--filter", "is_active=t", "--filter", "nonsense"]);
    assert!(!out.status.success());
    assert!(stderr_of(&out).contains("--filter must be"), "{}", stderr_of(&out));
    assert!(stderr_of(&out).contains("nonsense"), "{}", stderr_of(&out));
}

/// A column the block does not carry is the library's refusal, and it names
/// the offending term's column wherever in the conjunction that term sits.
#[test]
fn an_unknown_column_in_a_later_term_names_that_column() {
    let out = widgets(&["--filter", "is_active=t", "--filter", "nope=x"]);
    assert!(!out.status.success());
    assert!(stderr_of(&out).contains("`nope`"), "{}", stderr_of(&out));
}

/// The two flag families are independent: a term may name a column no
/// `--column` projects, and the projection still decides what is printed.
#[test]
fn a_conjunction_composes_with_a_projection() {
    let dump = fixture("16/edge_cases/default.sql");
    let out = run(&[
        "query",
        "--source",
        dump.to_str().unwrap(),
        "--table",
        "public.widgets",
        "--dqcache",
        "none",
        "--column",
        "name",
        "--filter",
        "is_active=t",
        "--filter",
        "created_at IS NOT NULL",
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stdout = stdout_of(&out);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["name", "alpha", "delta"]);
}
