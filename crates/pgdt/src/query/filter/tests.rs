use super::*;

/// **The structural refusal runs before the term grammar**, so a term
/// both structural and unparseable is told which of the two it is:
/// `and is null` is refused for the reserved spelling, and the remedy is
/// the quoting the grammar already teaches.
#[test]
fn the_flag_refuses_structure_before_it_parses_a_term() {
    let structural = parse_filter_flag("and is null").expect_err("a reserved spelling");
    let message = format!("{structural:#}");
    assert!(message.contains("`AND`"), "{message}");
    assert!(!message.contains("--filter must be"), "{message}");
    assert_eq!(
        column_of(parse_filter_flag(r#""and" is null"#).expect("the quoted column is askable")),
        "and"
    );
    // A term with no structure still reaches the term grammar, refusal
    // and all.
    assert!(
        format!("{:#}", parse_filter_flag("nonsense").expect_err("no operator"))
            .contains("--filter must be")
    );
    let Expr::Term(term) = parse_filter_flag("name=alpha").expect("a plain term") else {
        panic!("a comparison")
    };
    assert_eq!(term.op.value(), Some("alpha"));
    // A membership is one term under either flag, its paren no structure.
    let Expr::In(membership) = parse_filter_flag("name in (alpha, 'and')").expect("a list") else {
        panic!("a membership")
    };
    assert_eq!(membership.values, [Some("alpha".to_string()), Some("and".to_string())]);
}

/// The column a comparison term names.
fn column_of(expr: Expr) -> String {
    match expr {
        Expr::Term(term) => term.column,
        other => panic!("a comparison, not {other:?}"),
    }
}
