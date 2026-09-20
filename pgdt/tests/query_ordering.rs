//! `pgdt query --filter` with the four ordering operators — their CLI
//! surface (`docs/design/decisions.md`, "Predicates", the ordering
//! register, and "The CLI").
//!
//! What an ordering comparison *means* is pinned against the library in
//! `pgdump_query/src/predicate.rs` and `pgdump_query/tests/ordering.rs`.
//! What only the binary can say is how a term is split into column, operator
//! and value, and that a divergent comparison is announced on stderr — the
//! signal that exists nowhere else, since it is neither a `Diagnostic` nor a
//! `ColumnNote`.

use std::process::Output;

mod common;
use common::{fixture, run, stderr_of, stdout_of};

/// One table of the generated `types` fixture, projected to one column so a
/// sibling column's undecodable value cannot sink the query.
fn query(table: &str, column: &str, extra: &[&str]) -> Output {
    let dump = fixture("16/types/default.sql");
    let mut args = vec![
        "query",
        "--source",
        dump.to_str().unwrap(),
        "--table",
        table,
        "--dtcache",
        "none",
        "--column",
        column,
    ];
    args.extend_from_slice(extra);
    run(&args)
}

/// The values of `column` that survive `extra`, in file order, without the
/// header line.
fn kept(table: &str, column: &str, extra: &[&str]) -> Vec<String> {
    let out = query(table, column, extra);
    assert!(out.status.success(), "{}", stderr_of(&out));
    stdout_of(&out).lines().skip(1).map(str::to_string).collect()
}

/// **The point of this test.** `public.t_int` holds `-32768`, `32767`
/// and `0`; compared as text `-32768` would sort last, so the answer
/// distinguishes a typed comparison from the string one `=` uses.
#[test]
fn an_ordering_operator_compares_numerically() {
    assert_eq!(kept("public.t_int", "v_smallint", &["--filter", "v_smallint>0"]), ["32767"]);
    assert_eq!(kept("public.t_int", "v_smallint", &["--filter", "v_smallint<0"]), ["-32768"]);
}

/// `>=` is one operator, not `>` with a stray `=` in the value — the
/// longest spelling at a position wins, exactly as `!=` has always beaten
/// `=`. `>` and `>=` differ on the boundary row and nothing else.
#[test]
fn the_two_character_spellings_are_not_split_at_their_first_byte() {
    assert_eq!(kept("public.t_int", "v_smallint", &["--filter", "v_smallint>=0"]), ["32767", "0"]);
    assert_eq!(kept("public.t_int", "v_smallint", &["--filter", "v_smallint<=0"]), ["-32768", "0"]);
}

/// The **earliest** operator position wins, so a value containing an
/// operator byte is not stolen by it: `name=alpha>x` is `name` equal to
/// `alpha>x`, which no row carries — not `name=alpha` greater than `x`,
/// which would name a column that does not exist.
#[test]
fn a_value_containing_an_operator_byte_does_not_steal_the_split() {
    let dump = fixture("16/edge_cases/default.sql");
    let out = run(&[
        "query",
        "--source",
        dump.to_str().unwrap(),
        "--table",
        "public.widgets",
        "--dtcache",
        "none",
        "--no-columns",
        "--filter",
        "name=alpha>x",
    ]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert_eq!(stdout_of(&out), "", "no row has that name");
    assert!(stderr_of(&out).contains("no rows found"), "{}", stderr_of(&out));
}

/// A column that resolved `Mapped` but whose order is not PostgreSQL's is
/// announced once, on stderr, naming the column and what the order is not.
/// `text` is the collation case (I32); nothing in the file could close it.
#[test]
fn a_divergent_comparison_is_announced_on_stderr() {
    let out = query("public.t_text", "v_text", &["--filter", "v_text>a"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    assert_eq!(stderr.matches("`v_text`").count(), 1, "announced once: {stderr}");
    assert!(stderr.contains("collation"), "{stderr}");
    assert!(!stdout_of(&out).contains("warning"), "the announcement stays off stdout");
}

/// A query that selects no rows still resolved a schema, so it still owes the
/// announcement — otherwise the one case where a user most wants to know why
/// their filter matched nothing is the case that says nothing.
#[test]
fn a_query_with_no_matching_rows_still_announces() {
    let out = query("public.t_text", "v_text", &["--filter", "v_text>zzz"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(stderr_of(&out).contains("collation"), "{}", stderr_of(&out));
}

/// A comparison that agrees with PostgreSQL says nothing at all: the
/// announcement is a divergence report, not a "you used an operator" notice.
#[test]
fn an_agreeing_comparison_is_silent() {
    let out = query("public.t_int", "v_integer", &["--filter", "v_integer>0"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(!stderr_of(&out).contains("warning"), "{}", stderr_of(&out));
}

/// A nested column whose *shape* compares here and one of whose positions
/// does not keeps `=`/`!=` and refuses an ordering operator — and the refusal
/// names the column, the position, and the operators that do work.
///
/// `public.arr_holder` is `(label text, arr public.intarr[])`, and
/// `public.intarr` is a domain over `integer[]`, so its `arr` field is an
/// array whose element is an array (I26) — a shape this build declines to
/// order rather than mis-decode.
#[test]
fn an_ordering_operator_on_a_nested_column_is_refused() {
    let out = query("public.t_nested_array", "v_arr_holder", &["--filter", "v_arr_holder>1"]);
    assert!(!out.status.success());
    let stderr = stderr_of(&out);
    assert!(stderr.contains("`v_arr_holder`"), "{stderr}");
    assert!(stderr.contains("nested"), "{stderr}");
    assert!(stderr.contains("`.arr[]`"), "{stderr}");
    assert!(stderr.contains("`=` or `!=`"), "{stderr}");
}

/// Under `--schema-mode strings` nothing resolves, so every ordering
/// operator is refused — a consequence of the rule rather than a case of its
/// own, and the message says which flag caused it.
#[test]
fn strings_mode_refuses_every_ordering_operator() {
    let out = query(
        "public.t_int",
        "v_integer",
        &["--schema-mode", "strings", "--filter", "v_integer>0"],
    );
    assert!(!out.status.success());
    assert!(stderr_of(&out).contains("--schema-mode strings"), "{}", stderr_of(&out));
}

/// PostgreSQL's `infinity` is a value with an order, not a decode failure, so
/// a filter over a `date` column holding one answers instead of erroring. The
/// meaning is pinned against the library; what this adds is that the binary
/// exits 0 on a dump every `pg_dump` can produce.
#[test]
fn a_special_value_is_ordered_rather_than_ending_the_query() {
    assert_eq!(kept("public.t_date", "id", &["--filter", "v_date>9999-12-31"]), ["1", "6"]);
    assert_eq!(kept("public.t_numeric", "id", &["--filter", "v_small>0.00"]), ["3"]);
}

/// A literal that is not a value of the column's type is refused by name,
/// once, rather than silently matching nothing.
#[test]
fn a_literal_that_is_not_of_the_columns_type_is_refused() {
    let out = query("public.t_int", "v_integer", &["--filter", "v_integer>abc"]);
    assert!(!out.status.success());
    let stderr = stderr_of(&out);
    assert!(stderr.contains("`abc`"), "{stderr}");
    assert!(stderr.contains("integer"), "{stderr}");
}

/// The message and the parser agree on the operator set, so a term the parser
/// cannot split lists every operator that would have worked — the worded ones
/// included, since a user who reaches this message has already been refused
/// once.
#[test]
fn the_usage_message_names_every_operator() {
    let out = query("public.t_int", "v_integer", &["--filter", "nonsense"]);
    assert!(!out.status.success());
    let stderr = stderr_of(&out);
    for op in
        ["!=", "<", "<=", ">", ">=", "IS DISTINCT FROM", "IS NOT DISTINCT FROM", "IS NOT NULL"]
    {
        assert!(stderr.contains(op), "usage does not name `{op}`: {stderr}");
    }
}

/// Whitespace before the operator belongs to nobody: it is trimmed off the
/// column name, the same way the `IS NULL` forms already trim it.
#[test]
fn trailing_whitespace_on_the_column_name_is_trimmed() {
    assert_eq!(kept("public.t_int", "v_smallint", &["--filter", "v_smallint >0"]), ["32767"]);
}
