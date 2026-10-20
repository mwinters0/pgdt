use super::*;
use pgdt_query::PredicateOp;

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
    assert_eq!(term.op, PredicateOp::Eq("alpha".into()));
    // A membership is one term under either flag, its paren no structure.
    let Expr::Term(membership) = parse_filter_flag("name in (alpha, 'and')").expect("a list")
    else {
        panic!("a membership")
    };
    let values = vec![Some("alpha".to_string()), Some("and".to_string())];
    assert_eq!(membership.op, PredicateOp::In(values));
}

/// The column a comparison term names.
fn column_of(expr: Expr) -> String {
    match expr {
        Expr::Term(term) => term.column,
        other => panic!("a comparison, not {other:?}"),
    }
}

/// SplitMix64, seeded, so a failing term is the same term on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn pick<'a>(&mut self, pieces: &[&'a str]) -> &'a str {
        pieces[(self.next() % pieces.len() as u64) as usize]
    }
}

/// **Whatever `--filter` accepts, `--where` reads as the same term**: the
/// property `refuse_where_structure` claims (`docs/design/decisions.md`,
/// "D60"), over terms built from the grammar's pieces — every operator
/// spelling, the worded ones in either case, both quotes, parens, the
/// keywords, and ASCII and Unicode whitespace in every gap — some with a
/// piece of structure before or after. A string `--where` refuses, or reads
/// otherwise, is one `--filter` should have refused.
#[test]
fn every_term_the_filter_flag_accepts_means_the_same_under_where() {
    const SPACES: [&str; 6] = ["", " ", "  ", "\t", "\u{a0}", "\u{3000}"];
    const COLUMNS: [&str; 8] = ["", "x", "\"a b\"", "\"a=b\"", "is", "and", "in", "not"];
    const OPERATORS: [&str; 16] = [
        "=",
        "!=",
        "<>",
        "<",
        "<=",
        ">",
        ">=",
        "is distinct from",
        "is not distinct from",
        "IS  Distinct\tFROM",
        "in",
        "IN",
        "is null",
        "is not null",
        "IS UNREPRESENTABLE",
        "is not unrepresentable",
    ];
    const VALUES: [&str; 12] = [
        "",
        "1",
        "'a''b'",
        "\"q\"",
        "don't",
        "'open",
        "(1, 2)",
        "('a', \"b\")",
        "a and b",
        "x=1",
        "null",
        ")",
    ];
    const STRUCTURE: [&str; 9] = ["", "", "", "(", ")", "and", "or", "not", "x=1 and"];

    let mut rng = Rng(0x00f1_17e2);
    let mut accepted = 0usize;
    for _ in 0..50_000 {
        let spec = [
            rng.pick(&STRUCTURE),
            rng.pick(&SPACES),
            rng.pick(&COLUMNS),
            rng.pick(&SPACES),
            rng.pick(&OPERATORS),
            rng.pick(&SPACES),
            rng.pick(&VALUES),
            rng.pick(&SPACES),
            rng.pick(&STRUCTURE),
        ]
        .join(if rng.next().is_multiple_of(4) { "" } else { " " });
        let Ok(filter) = parse_filter_flag(&spec) else { continue };
        accepted += 1;
        let filter = format!("{filter:?}");
        let r#where = parse_where(&spec).map(|expr| format!("{expr:?}"));
        assert_eq!(
            r#where.as_ref().ok(),
            Some(&filter),
            "--filter `{spec:?}` accepted as {filter}, --where read {where:?}"
        );
    }
    assert!(accepted > 1_000, "only {accepted} terms were accepted: the sweep asks too little");
}

/// A worded infix operator after leading whitespace, read under `--filter`
/// as `--where` reads it: refused, there being no column before it.
#[test]
fn leading_whitespace_before_a_worded_operator_reads_alike_under_both_flags() {
    for spec in
        [" is distinct from x", "\t is not distinct from x", " is distinct from don't and x"]
    {
        let filter =
            parse_filter_flag(spec).map(|e| format!("{e:?}")).map_err(|e| format!("{e:#}"));
        let r#where = parse_where(spec).map(|e| format!("{e:?}")).map_err(|e| format!("{e:#}"));
        assert_eq!(filter.is_ok(), r#where.is_ok(), "{spec:?}: {filter:?} / {where:?}");
        assert!(filter.is_err(), "{spec:?} names no column: {filter:?}");
    }
}
