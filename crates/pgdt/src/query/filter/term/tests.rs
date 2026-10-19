use super::*;

/// One parsed comparison term, or the message it was refused with.
fn filter(spec: &str) -> Result<(String, PredicateOp), String> {
    match parse_filter(spec) {
        Ok(Expr::Term(p)) => Ok((p.column, p.op)),
        Ok(other) => panic!("`{spec}` should be a comparison, parsed as {other:?}"),
        Err(e) => Err(e.to_string()),
    }
}

/// The column and values of a membership term that must parse.
fn membership(spec: &str) -> (String, Vec<String>) {
    match parse_filter(spec) {
        Ok(Expr::In(m)) => (m.column, m.values.into_iter().map(Option::unwrap).collect()),
        other => panic!("`{spec}` should be a membership, got {other:?}"),
    }
}

/// The message a membership term was refused with.
fn membership_err(spec: &str) -> String {
    match parse_filter(spec) {
        Err(e) => e.to_string(),
        Ok(parsed) => panic!("`{spec}` should be refused, parsed as {parsed:?}"),
    }
}

/// **`IN` is a worded operator before a list**, found where the others
/// are, the earliest winning; each value is read as `=`'s is — trimmed,
/// or quoted to keep its spaces, its commas or its parens — and `null`
/// is the text `null`, `=` having no NULL either.
#[test]
fn in_reads_a_list_of_values_as_equality_reads_one() {
    let own = |c: &str, vs: &[&str]| (c.to_string(), vs.iter().map(|v| v.to_string()).collect());
    assert_eq!(membership("id in (1, 2,3)"), own("id", &["1", "2", "3"]));
    assert_eq!(membership("id IN(7)"), own("id", &["7"]));
    assert_eq!(membership("id   In   ( 7 )  "), own("id", &["7"]));
    assert_eq!(
        membership(r#"note in (' a, b ', "it""s", '(1,2)', null, '')"#),
        own("note", &[" a, b ", r#"it"s"#, "(1,2)", "null", ""])
    );
    // An unquoted value keeps a quote that does not open it, as `=` does.
    assert_eq!(membership("note in (don't, x)"), own("note", &["don't", "x"]));
    assert_eq!(membership(r#""a in (b)" in (c)"#), own("a in (b)", &["c"]));
    // The earliest operator wins: an `=` before the word keeps the term
    // a comparison, and the word needs whitespace before it and a paren
    // after it to be one.
    assert_eq!(ok("note=x in (y)"), ("note".into(), PredicateOp::Eq("x in (y)".into())));
    assert!(err("tin (y)").contains("--filter must be"));
    assert!(err("x in y").contains("--filter must be"));
    for (spec, says) in [
        ("x in ()", "at least one value"),
        ("x in ( )", "at least one value"),
        ("x in (1,,2)", "empty value"),
        ("x in (1,)", "empty value"),
        ("x in (1, 2", "never closed"),
        ("x in ('1)", "never closed"),
        ("x in (1) 2", "follows the `IN` list"),
        ("x in ((1,2))", "holds a `(`"),
        ("x in ('a'b)", "unbalanced `'` quote"),
    ] {
        let message = membership_err(spec);
        assert!(message.contains(says), "`{spec}`: {message}");
    }
}

/// `column`, `op` and `value` of a term that must parse.
fn ok(spec: &str) -> (String, PredicateOp) {
    filter(spec).unwrap_or_else(|e| panic!("`{spec}` should parse: {e}"))
}

/// The message a term that must not parse was refused with.
fn err(spec: &str) -> String {
    match filter(spec) {
        Err(message) => message,
        Ok(parsed) => panic!("`{spec}` should be refused, parsed as {parsed:?}"),
    }
}

/// The bare spelling: no whitespace anywhere means nothing to trim and no
/// quote to strip.
#[test]
fn a_bare_term_parses_as_it_reads() {
    assert_eq!(ok("name=alpha"), ("name".into(), PredicateOp::Eq("alpha".into())));
    assert_eq!(ok("v>=5"), ("v".into(), PredicateOp::Ge("5".into())));
    assert_eq!(ok("v!=5"), ("v".into(), PredicateOp::Ne("5".into())));
}

/// Whitespace outside quotes is not data, on **both** sides of the
/// operator — untrimmed, the value side fails loudly on a typed column and
/// silently on a text one.
#[test]
fn whitespace_outside_quotes_is_trimmed_on_both_sides() {
    assert_eq!(ok(" name = alpha "), ("name".into(), PredicateOp::Eq("alpha".into())));
    assert_eq!(ok("v >= 5"), ("v".into(), PredicateOp::Ge("5".into())));
}

/// Whitespace is `str::trim`'s, not ASCII space's, so a non-breaking
/// space pasted out of a web page is caught rather than searched for.
#[test]
fn trimming_is_unicode_whitespace() {
    assert_eq!(
        ok("name\u{a0}=\u{a0}alpha\u{a0}"),
        ("name".into(), PredicateOp::Eq("alpha".into()))
    );
}

/// An all-whitespace value collapses to the empty string, which needs no
/// special handling: it is a legitimate value to search for and the
/// quoted spelling is there for anyone who meant the spaces.
#[test]
fn an_all_whitespace_value_collapses_to_empty() {
    assert_eq!(ok("name=   "), ("name".into(), PredicateOp::Eq(String::new())));
}

/// A quoted value is taken exactly as written, which is what keeps a
/// space-padded `char(n)` value askable from the command line.
#[test]
fn a_quoted_value_is_taken_as_written() {
    assert_eq!(ok(r#"name = " x""#), ("name".into(), PredicateOp::Eq(" x".into())));
    assert_eq!(ok("name = 'x '"), ("name".into(), PredicateOp::Eq("x ".into())));
    assert_eq!(ok("name=''"), ("name".into(), PredicateOp::Eq(String::new())));
}

/// Both quote characters open a value: which one a user reaches for is
/// decided by the shell, the term normally being inside shell single
/// quotes already.
#[test]
fn both_quote_characters_open_a_value() {
    let (_, double) = ok(r#"name="the answer""#);
    let (_, single) = ok("name='the answer'");
    assert_eq!(double.value(), Some("the answer"));
    assert_eq!(single, double);
}

/// A quote inside a quoted part is doubled, as SQL does it. Two quote
/// characters also give a lazier escape for free: a value holding one can
/// be written in the other, with no doubling at all.
#[test]
fn an_interior_quote_is_doubled_or_written_in_the_other_quote() {
    assert_eq!(ok("note='it''s'").1.value(), Some("it's"));
    assert_eq!(ok(r#"note="say ""hi""""#).1.value(), Some(r#"say "hi""#));
    assert_eq!(ok(r#"note="it's""#).1.value(), Some("it's"));
}

/// A quote that does not open the value is ordinary data, to this parser
/// and to `--where`'s tokenizer alike.
#[test]
fn a_quote_inside_an_unquoted_value_is_data() {
    assert_eq!(ok("note=don't").1.value(), Some("don't"));
    assert_eq!(ok(r#"note=a"b"#).1.value(), Some(r#"a"b"#));
}

/// Quotes work on the column side too, and the operator split skips them,
/// which is what makes a column named `a=b` askable.
#[test]
fn a_quoted_column_name_survives_the_split() {
    assert_eq!(ok(r#""my column"=x"#).0, "my column");
    assert_eq!(ok(r#""a=b"=x"#), ("a=b".into(), PredicateOp::Eq("x".into())));
    assert_eq!(ok(r#" "a=b" = "y=z" "#).1.value(), Some("y=z"));
}

/// The split rule is otherwise unchanged: earliest position, longest
/// spelling, so a value carrying an operator byte still cannot steal it.
#[test]
fn the_earliest_operator_outside_quotes_still_wins() {
    assert_eq!(ok("name=alpha>x"), ("name".into(), PredicateOp::Eq("alpha>x".into())));
}

/// The two worded infix operators, in every case and with any run of
/// whitespace between their words — the spelling `PredicateOp::symbol`
/// names them by, so the grammar and every refusal message agree.
#[test]
fn the_distinct_from_forms_parse() {
    assert_eq!(ok("v is distinct from 1"), ("v".into(), PredicateOp::IsDistinctFrom("1".into())));
    assert_eq!(
        ok("v IS NOT DISTINCT FROM 1"),
        ("v".into(), PredicateOp::IsNotDistinctFrom("1".into()))
    );
    assert_eq!(
        ok("v Is  Not   Distinct\tFrom  ' x'"),
        ("v".into(), PredicateOp::IsNotDistinctFrom(" x".into()))
    );
}

/// **A worded operator is a candidate at a position, not a pass of its
/// own**, so the earliest operator still wins in both directions: the
/// punctuation one when it is to the left, the phrase when it is.
#[test]
fn the_earliest_operator_wins_against_a_worded_one_too() {
    assert_eq!(
        ok("note=a is distinct from b"),
        ("note".into(), PredicateOp::Eq("a is distinct from b".into()))
    );
    assert_eq!(
        ok("a is distinct from b=c"),
        ("a".into(), PredicateOp::IsDistinctFrom("b=c".into()))
    );
}

/// The phrase needs whitespace on both sides: a column named
/// `is distinct from` is still askable unquoted, what follows the phrase
/// there being `=` rather than a space.
#[test]
fn a_worded_operator_needs_whitespace_around_it() {
    assert_eq!(ok("is distinct from=x"), ("is distinct from".into(), PredicateOp::Eq("x".into())));
    // Only the exact words, and only with a value after them: a prefix
    // match on `distinctly`, or a `FROM` with nothing behind it, falls
    // through to the usage message rather than splitting.
    for spec in ["v is distinctly from x", "v is distinct from"] {
        let message = err(spec);
        assert!(message.contains("--filter must be"), "`{spec}`: {message}");
    }
}

/// **The `IS` forms are the fallback**: an operator outside quotes is
/// looked for first, so this is the equality it plainly reads as rather
/// than an `IS NULL` on a column `note=this`.
#[test]
fn a_value_ending_in_is_null_is_not_an_is_null_term() {
    assert_eq!(ok("note=this is null"), ("note".into(), PredicateOp::Eq("this is null".into())));
}

/// The `IS` forms parse case-insensitively and take a quoted column name,
/// which is what lets a column called `is null` be named.
#[test]
fn the_is_forms_parse_on_a_term_with_no_operator() {
    assert_eq!(ok("created_at IS NULL"), ("created_at".into(), PredicateOp::IsNull));
    assert_eq!(ok("created_at is not null"), ("created_at".into(), PredicateOp::IsNotNull));
    assert_eq!(ok(r#" "my column" Is Null "#).0, "my column");
    assert_eq!(ok("v_date IS UNREPRESENTABLE"), ("v_date".into(), PredicateOp::IsUnrepresentable));
    assert_eq!(
        ok("v_date is not unrepresentable"),
        ("v_date".into(), PredicateOp::IsNotUnrepresentable)
    );
    assert_eq!(ok(r#""is null" = x"#), ("is null".into(), PredicateOp::Eq("x".into())));
}

/// A malformed quote is refused, never reinterpreted. Unterminated and
/// trailing-text are one fault with one message.
#[test]
fn a_malformed_quote_is_refused() {
    for spec in ["name='x", "name='x'y", "'name=x", r#"name = "x'"#, "'name' 'is null"] {
        let message = err(spec);
        assert!(message.contains("unbalanced"), "`{spec}`: {message}");
        assert!(message.contains(spec), "`{spec}`: {message}");
    }
}

/// A term with neither an operator nor an `IS` form is a usage fault, and
/// the message quotes the term back.
#[test]
fn a_term_with_no_operator_at_all_is_a_usage_fault() {
    let message = err("nonsense");
    assert!(message.contains("--filter must be"), "{message}");
    assert!(message.contains("nonsense"), "{message}");
}
