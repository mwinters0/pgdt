use arrow::datatypes::DataType;

use super::*;
use crate::predicate::test_support::*;
use crate::predicate::{Expr, Predicate};
use pgdt_codec::ResolvedSchema;
use pgdt_codec::compare::CompareKind;
use pgdt_core::escape_field;
use pgdt_model::UnrepresentableTier;

/// One text term's own truth value over `raw_row`.
fn truth(p: &Predicate, raw_row: &[u8], index: usize) -> Truth {
    text_term(p, index)
        .eval(RawRow::unchecked(raw_row), &mut RowSplit::default(), "public.t", 0)
        .unwrap()
}

/// Whether a row survives a conjunction of text terms.
fn matches_all(filters: &[Predicate], indices: &[usize], raw_row: &[u8]) -> bool {
    let expr = ResolvedExpr::And(
        filters.iter().zip(indices).map(|(p, &i)| ResolvedExpr::Term(text_term(p, i))).collect(),
    );
    expr.matches(RawRow::unchecked(raw_row), &mut RowSplit::default(), "public.t", 0).unwrap()
}

#[test]
fn eq_matches_the_decoded_value() {
    let p = Predicate { column: "x".into(), op: PredicateOp::Eq("a\tb".into()) };
    assert_eq!(truth(&p, b"other\ta\\tb", 1), Truth::True);
    assert_eq!(truth(&p, b"other\tc", 1), Truth::False);
}

#[test]
fn ne_matches_everything_but_the_decoded_value() {
    let p = Predicate { column: "x".into(), op: PredicateOp::Ne("a".into()) };
    assert_eq!(truth(&p, b"other\tb", 1), Truth::True);
    assert_eq!(truth(&p, b"other\ta", 1), Truth::False);
}

/// A NULL field is *unknown* under `=` and `!=`, not false, and a row is
/// kept only where the root is true.
#[test]
fn null_is_unknown_under_eq_and_ne() {
    let eq = Predicate { column: "x".into(), op: PredicateOp::Eq("a".into()) };
    let ne = Predicate { column: "x".into(), op: PredicateOp::Ne("a".into()) };
    assert_eq!(truth(&eq, b"other\t\\N", 1), Truth::Unknown);
    assert_eq!(truth(&ne, b"other\t\\N", 1), Truth::Unknown);
    assert!(!matches_all(&[eq, ne], &[1, 1], b"other\t\\N"));
}

/// `IS DISTINCT FROM` is `!=` with NULL counted as a value, which is the
/// one thing `NOT` cannot express: `NOT (x = a)` drops a NULL row.
#[test]
fn is_distinct_from_counts_null_as_a_value() {
    let idf = Predicate { column: "x".into(), op: PredicateOp::IsDistinctFrom("a".into()) };
    let indf = Predicate { column: "x".into(), op: PredicateOp::IsNotDistinctFrom("a".into()) };
    assert_eq!(truth(&idf, b"other\t\\N", 1), Truth::True);
    assert_eq!(truth(&idf, b"other\tb", 1), Truth::True);
    assert_eq!(truth(&idf, b"other\ta", 1), Truth::False);
    assert_eq!(truth(&indf, b"other\t\\N", 1), Truth::False);
    assert_eq!(truth(&indf, b"other\tb", 1), Truth::False);
    assert_eq!(truth(&indf, b"other\ta", 1), Truth::True);

    // The half `NOT` gets wrong, stated as the pair it is.
    let eq = Predicate { column: "x".into(), op: PredicateOp::Eq("a".into()) };
    let negated = ResolvedExpr::Not(Box::new(ResolvedExpr::Term(text_term(&eq, 1))));
    assert!(
        !negated
            .matches(RawRow::unchecked(b"other\t\\N"), &mut RowSplit::default(), "public.t", 0)
            .unwrap()
    );
    assert!(
        ResolvedExpr::Term(text_term(&idf, 1))
            .matches(RawRow::unchecked(b"other\t\\N"), &mut RowSplit::default(), "public.t", 0)
            .unwrap()
    );
}

#[test]
fn is_null_and_is_not_null() {
    let is_null = Predicate { column: "x".into(), op: PredicateOp::IsNull };
    let is_not_null = Predicate { column: "x".into(), op: PredicateOp::IsNotNull };
    assert_eq!(truth(&is_null, b"other\t\\N", 1), Truth::True);
    assert_eq!(truth(&is_null, b"other\ta", 1), Truth::False);
    assert_eq!(truth(&is_not_null, b"other\t\\N", 1), Truth::False);
    assert_eq!(truth(&is_not_null, b"other\ta", 1), Truth::True);
}

fn row_truth(expr: &ResolvedExpr, field: &str) -> Truth {
    let row = escape_field((field != "\\N").then_some(field));
    expr.eval(true, RawRow::unchecked(&row), &mut RowSplit::default(), "public.t", 0).unwrap()
}

/// **`IS UNREPRESENTABLE` asks of the text, in the tiers its front end
/// reads, and tells apart exactly the NULLs the null mode made there**:
/// an infinity in both, a date past the calendar in DataFusion's alone, a
/// NULL the dump holds and a finite date in neither — two-valued, and
/// its negation the complement, NULL included (`docs/design/decisions.md`,
/// "D101").
#[test]
fn the_unrepresentable_test_tells_the_null_mode_s_nulls_from_the_dump_s() {
    use UnrepresentableTier::{Engine, Format};
    let cases: [(&str, bool, bool); 5] = [
        // (field, IS UNREPRESENTABLE at Format, at Engine)
        ("infinity", true, true),
        ("-infinity", true, true),
        ("262143-01-01", false, true),
        ("2024-01-01", false, false),
        ("\\N", false, false),
    ];
    for (field, format, engine) in cases {
        for (reach, want) in [(Format, format), (Engine, engine)] {
            let is = date_test(PredicateOp::IsUnrepresentable, Some(reach));
            let is_not = date_test(PredicateOp::IsNotUnrepresentable, Some(reach));
            assert_eq!(row_truth(&is, field), Truth::of(want), "{field} at {reach:?}");
            assert_eq!(row_truth(&is_not, field), Truth::of(!want), "{field} at {reach:?}");
            // The null mode reads as NULL exactly what the test matches,
            // so `IS NULL` is the test's or the dump's.
            let is_null = date_test(PredicateOp::IsNull, Some(reach));
            assert_eq!(row_truth(&is_null, field), Truth::of(want || field == "\\N"));
        }
        // A column the block's count says holds none is tested by
        // nothing, and never matches.
        let untested = date_test(PredicateOp::IsUnrepresentable, None);
        assert_eq!(row_truth(&untested, field), Truth::False);
    }
}

#[test]
fn an_empty_conjunction_matches_every_row() {
    assert!(matches_all(&[], &[], b"a\tb"));
    // And the default filter *is* that conjunction.
    assert!(matches!(Expr::default(), Expr::And(ref children) if children.is_empty()));
}

#[test]
fn every_term_must_match() {
    let filters = [
        Predicate { column: "a".into(), op: PredicateOp::Eq("1".into()) },
        Predicate { column: "b".into(), op: PredicateOp::Eq("2".into()) },
    ];
    assert!(matches_all(&filters, &[0, 1], b"1\t2"));
    assert!(!matches_all(&filters, &[0, 1], b"1\t3"));
    assert!(!matches_all(&filters, &[0, 1], b"9\t2"));
}

/// Two terms on one column are an ordinary conjunction, and a
/// contradictory pair matches nothing — no term is special-cased.
#[test]
fn two_terms_may_name_the_same_column() {
    let filters = [
        Predicate { column: "a".into(), op: PredicateOp::Ne("1".into()) },
        Predicate { column: "a".into(), op: PredicateOp::Ne("2".into()) },
    ];
    assert!(matches_all(&filters, &[0, 0], b"3"));
    assert!(!matches_all(&filters, &[0, 0], b"2"));
}

const VALUES: [Truth; 3] = [Truth::True, Truth::False, Truth::Unknown];

/// Kleene's tables, written out rather than derived, over trees whose
/// leaves are real terms.
#[test]
fn and_or_and_not_are_three_valued() {
    use Truth::{False, True, Unknown};

    assert_eq!(exact(&ResolvedExpr::And(vec![])), True, "an empty conjunction is true");
    assert_eq!(exact(&ResolvedExpr::Or(vec![])), False, "an empty disjunction is false");

    for a in VALUES {
        assert_eq!(exact(&ResolvedExpr::Not(Box::new(leaf(a)))), a.not());
        for b in VALUES {
            let pair = || vec![leaf(a), leaf(b)];
            let and = match (a, b) {
                (False, _) | (_, False) => False,
                (Unknown, _) | (_, Unknown) => Unknown,
                _ => True,
            };
            let or = match (a, b) {
                (True, _) | (_, True) => True,
                (Unknown, _) | (_, Unknown) => Unknown,
                _ => False,
            };
            assert_eq!(exact(&ResolvedExpr::And(pair())), and, "{a:?} AND {b:?}");
            assert_eq!(exact(&ResolvedExpr::Or(pair())), or, "{a:?} OR {b:?}");
        }
    }
}

/// Every tree of height at most three over the three truth values, with
/// `And`/`Or` arity two.
fn trees(height: usize) -> Vec<ResolvedExpr> {
    let mut out: Vec<ResolvedExpr> = VALUES.iter().copied().map(leaf).collect();
    if height == 0 {
        return out;
    }
    let sub = trees(height - 1);
    for a in &sub {
        out.push(ResolvedExpr::Not(Box::new(a.clone())));
        for b in &sub {
            out.push(ResolvedExpr::And(vec![a.clone(), b.clone()]));
            out.push(ResolvedExpr::Or(vec![a.clone(), b.clone()]));
        }
    }
    out
}

/// The short-circuit is verdict-preserving: `matches` evaluates the root
/// inexactly — an `And` may report `False` for an `Unknown` and stop —
/// and nothing above the root tells the two apart. Asserted over every
/// tree of height three.
#[test]
fn the_root_verdict_survives_the_short_circuit() {
    let all = trees(2);
    assert!(all.len() > 1000, "only {} trees", all.len());
    for expr in &all {
        assert_eq!(
            expr.matches(
                RawRow::unchecked(THREE_VALUED_ROW),
                &mut RowSplit::default(),
                "public.t",
                0
            )
            .unwrap(),
            exact(expr).is_true(),
            "{expr:?}"
        );
    }
}

/// A decode failure surfaces only where evaluation reaches it, so which
/// rows error depends on where the term sits in the tree — and on whether
/// a `Not` sits above it, a `Not` being the one node that has to tell
/// `False` from `Unknown` and so cannot short-circuit an unknown away.
#[test]
fn a_decode_failure_surfaces_only_where_it_is_reached() {
    let schema = two_integers();
    let term = |column: &str, op: Comparing, value: &str, index| {
        let p = Predicate { column: column.into(), op: op(value.into()) };
        ResolvedExpr::Term(resolve_term(&p, index, &schema, 0).unwrap())
    };
    // `b` holds text no `integer` decoder will read, so any term over it
    // is `Error::FieldDecode` the moment it is evaluated.
    let row: &[u8] = b"1\tnope";
    let corrupt = || term("b", PredicateOp::Gt, "0", 1);
    let ok = |value| term("a", PredicateOp::Eq, value, 0);

    assert!(
        corrupt().matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0).is_err()
    );
    // Settled by a field that did decode, in both directions.
    assert!(
        ResolvedExpr::Or(vec![ok("1"), corrupt()])
            .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
            .unwrap()
    );
    assert!(
        !ResolvedExpr::And(vec![ok("2"), corrupt()])
            .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
            .unwrap()
    );
    // Not settled: the walk reaches the corrupt field.
    assert!(
        ResolvedExpr::And(vec![ok("1"), corrupt()])
            .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
            .is_err()
    );

    // An unknown conjunct settles the *root*, so the walk stops there
    // until a `Not` above it makes `Unknown` and `False` different
    // answers. Field 2 does not exist in this row, so the leading term
    // reads NULL and is unknown.
    let unknown = || {
        let p = Predicate { column: "c".into(), op: PredicateOp::Eq("x".into()) };
        ResolvedExpr::And(vec![ResolvedExpr::Term(text_term(&p, 2)), corrupt()])
    };
    assert!(
        !unknown()
            .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
            .unwrap()
    );
    assert!(
        ResolvedExpr::Not(Box::new(unknown()))
            .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
            .is_err()
    );
}

/// **A membership answers exactly what the `Or` of its `=` terms does**,
/// in all three values: a NULL in the list is a disjunct that is unknown
/// on every row — here an `=` over a field the row does not have — so it
/// never makes the membership true and turns a miss into `Unknown`; a
/// NULL field is unknown against any list with a value in it; and the
/// empty list is the empty disjunction, false on every row.
#[test]
fn a_membership_answers_as_its_disjunction_of_equalities() {
    let schema = two_integers();
    let lists: [&[Option<&str>]; 7] = [
        &[Some("1")],
        &[Some("3"), Some("1"), Some("1")],
        &[Some("1"), None],
        &[None, Some("7")],
        &[None],
        &[None, None],
        &[],
    ];
    let rows: [&[u8]; 4] = [b"1\t2", b"3\t\\N", b"\\N\t1", b"7"];
    for list in lists {
        let resolved = ResolvedExpr::In(membership(&schema, 0, list).unwrap());
        let disjunction = ResolvedExpr::Or(
            list.iter()
                .map(|value| match value {
                    Some(v) => {
                        let p = Predicate { column: "a".into(), op: PredicateOp::Eq((*v).into()) };
                        ResolvedExpr::Term(resolve_term(&p, 0, &schema, 0).unwrap())
                    }
                    None => {
                        let p = Predicate { column: "z".into(), op: PredicateOp::Eq("0".into()) };
                        ResolvedExpr::Term(text_term(&p, 99))
                    }
                })
                .collect(),
        );
        for row in rows {
            let want = exact_over(&disjunction, row).unwrap();
            assert_eq!(exact_over(&resolved, row).unwrap(), want, "{list:?} over {row:?}");
            let negated = |e: &ResolvedExpr| ResolvedExpr::Not(Box::new(e.clone()));
            assert_eq!(
                exact_over(&negated(&resolved), row).unwrap(),
                exact_over(&negated(&disjunction), row).unwrap(),
                "NOT {list:?} over {row:?}"
            );
        }
    }
    let over = |list: &[Option<&str>], row: &[u8]| {
        exact_over(&ResolvedExpr::In(membership(&schema, 0, list).unwrap()), row).unwrap()
    };
    assert_eq!(over(&[Some("1"), None], b"1\t2"), Truth::True);
    assert_eq!(over(&[Some("1"), None], b"3\t2"), Truth::Unknown);
    assert_eq!(over(&[Some("1")], b"\\N\t2"), Truth::Unknown);
    assert_eq!(over(&[None], b"1\t2"), Truth::Unknown);
    assert_eq!(over(&[], b"\\N\t2"), Truth::False);
}

/// **It raises where its first `=` would, and reads no field where none
/// is compared**: a value that does not decode is `FieldDecode` once any
/// value is listed, and a list of NULLs alone, like `= NULL`, never
/// reaches the field. A literal refused at resolution is the first one
/// in list order, the operator named `IN`.
#[test]
fn a_membership_raises_where_its_first_equality_would() {
    // A bare `numeric` decodes its field to compare it, where an
    // `integer`'s `=` compares the text as the file spells it.
    let numeric = one_column("numeric", DataType::Utf8View);
    let row: &[u8] = b"nope";
    let raised = |list: &[Option<&str>]| {
        exact_over(&ResolvedExpr::In(membership(&numeric, 0, list).unwrap()), row)
    };
    assert!(matches!(raised(&[Some("1"), None]), Err(Error::FieldDecode { .. })));
    assert!(matches!(raised(&[None, Some("1")]), Err(Error::FieldDecode { .. })));
    assert_eq!(raised(&[None]).unwrap(), Truth::Unknown);
    assert_eq!(raised(&[]).unwrap(), Truth::False);
    let schema = two_integers();
    let refused = membership(&schema, 0, &[Some("1"), None, Some("x"), Some("y")]).unwrap_err();
    assert!(
        matches!(&refused, Error::PredicateValueDecode { op: "IN", value, .. } if value == "x"),
        "{refused:?}"
    );
}

/// **Each comparison shape has its lookup**, answering as the `=` it
/// replaces: a spelling the file would not write matches a decoded kind
/// by value, a `character(n)` field matches past its padding, `NaN`
/// equals itself, and a nested value matches by structure.
#[test]
fn each_comparison_shape_is_looked_up_as_its_equality_compares() {
    let over = |schema: &ResolvedSchema, list: &[&str], field: &str| {
        let list: Vec<_> = list.iter().map(|v| Some(*v)).collect();
        let resolved = membership(schema, 0, &list).unwrap();
        (resolved.eval_value(Some(field)), std::mem::discriminant(&resolved.lookup))
    };
    let numeric = one_column("numeric", DataType::Utf8View);
    let (answer, shape) = over(&numeric, &["2", "1.50", "NaN"], "1.5");
    assert_eq!(answer, Some(Truth::True));
    assert_eq!(
        shape,
        std::mem::discriminant(&Lookup::Decoded {
            kind: CompareKind::Text { length: None, varying: false },
            keys: vec![]
        })
    );
    assert_eq!(over(&numeric, &["2", "1.50", "NaN"], "NaN").0, Some(Truth::True));
    assert_eq!(over(&numeric, &["2", "1.50"], "1.51").0, Some(Truth::False));
    assert_eq!(over(&numeric, &["2"], "x").0, None, "not a numeric: `=` would raise");
    let float = one_column("double precision", DataType::Float64);
    assert_eq!(over(&float, &["-Infinity", "NaN", "0"], "NaN").0, Some(Truth::True));
    assert_eq!(over(&float, &["-Infinity", "NaN", "0"], "-0").0, Some(Truth::True));
    let padded = one_column("character(4)", DataType::Utf8View);
    let (answer, shape) = over(&padded, &["b ", "a"], "a   ");
    assert_eq!(answer, Some(Truth::True));
    assert_eq!(shape, std::mem::discriminant(&Lookup::Trimmed(Default::default())));
    let integer = one_column("integer", DataType::Int32);
    let (answer, shape) = over(&integer, &["+7", "3"], "7");
    assert_eq!(answer, Some(Truth::True));
    assert_eq!(shape, std::mem::discriminant(&Lookup::Canonical(Default::default())));
    let types = test_types();
    let ranges = nested_column("int4multirange", &types);
    assert_eq!(over(&ranges, &["{[1,5),[5,10)}", "{}"], "{[1,10)}").0, Some(Truth::True));
    assert_eq!(over(&ranges, &["{[1,5)}", "{}"], "{[1,10)}").0, Some(Truth::False));
    assert!(matches!(membership(&ranges, 0, &[Some("{}")]).unwrap().lookup, Lookup::Nested { .. }));
}

/// A tree's shape, each leaf named by its operator and column: what
/// [`ResolvedExpr::for_rows`] leaves of it.
fn shape(expr: &ResolvedExpr) -> String {
    let list =
        |children: &[ResolvedExpr]| children.iter().map(shape).collect::<Vec<_>>().join(", ");
    match expr {
        ResolvedExpr::Term(term) => format!("{}{}", term.index, term.op.symbol()),
        ResolvedExpr::In(membership) => format!("{}in", membership.index),
        ResolvedExpr::And(children) => format!("and({})", list(children)),
        ResolvedExpr::Or(children) => format!("or({})", list(children)),
        ResolvedExpr::Not(inner) => format!("not({})", shape(inner)),
    }
}

/// **Row evaluation drops each ordering term a membership in the same
/// conjunction implies, and nothing else**: a bound every listed value
/// meets goes, one a value misses stays, as does one over another column
/// or beneath a `Not`; a conjunction nested in another is read as one, a
/// NULL in the list implies nothing less, and an empty list nothing.
#[test]
fn row_evaluation_drops_only_the_bounds_a_membership_implies() {
    let schema = two_integers();
    let term = |index: usize, op: Comparing, value: &str| {
        let column = schema.schema.field(index).name().clone();
        let p = Predicate { column, op: op(value.into()) };
        ResolvedExpr::Term(resolve_term(&p, index, &schema, 0).unwrap())
    };
    let listed =
        |values: &[Option<&str>]| ResolvedExpr::In(membership(&schema, 0, values).unwrap());
    let list = || listed(&[Some("1"), Some("3"), Some("5")]);
    let and = ResolvedExpr::And;
    let rows = |expr: ResolvedExpr| shape(&expr.for_rows());
    use PredicateOp::{Ge, Gt, Le, Lt};

    assert_eq!(rows(and(vec![term(0, Ge, "1"), term(0, Le, "5"), list()])), "0in");
    assert_eq!(
        rows(and(vec![and(vec![term(0, Gt, "0"), term(0, Lt, "6")]), list()])),
        "0in",
        "a join's filter nests its bounds"
    );
    assert_eq!(
        rows(and(vec![term(0, Ge, "1"), term(0, Le, "4"), list(), term(1, Gt, "0")])),
        "and(0<=, 0in, 1>)"
    );
    assert_eq!(rows(and(vec![term(0, Gt, "1"), list()])), "and(0>, 0in)");
    assert_eq!(
        rows(and(vec![term(0, Ge, "1"), listed(&[Some("3"), None])])),
        "0in",
        "a NULL in the list"
    );
    assert_eq!(rows(and(vec![term(0, Ge, "1"), listed(&[None])])), "and(0>=, 0in)");
    assert_eq!(rows(and(vec![term(0, Ge, "1"), listed(&[])])), "and(0>=, 0in)");
    assert_eq!(
        rows(ResolvedExpr::Or(vec![and(vec![term(0, Ge, "1"), list()]), term(1, Gt, "0")])),
        "or(0in, 1>)"
    );
    let negated = ResolvedExpr::Not(Box::new(and(vec![term(0, Ge, "1"), list()])));
    assert_eq!(rows(negated), "not(and(0>=, 0in))");
    let values = vec![Some("1".to_string()), Some("3".to_string())];
    let excluded = Expr::Term(Predicate { column: "a".into(), op: PredicateOp::NotIn(values) });
    let excluded = crate::predicate::resolve_expr(
        &excluded,
        &schema,
        0,
        pgdt_codec::compare::ComparisonSemantics::Postgres,
    )
    .unwrap();
    assert_eq!(rows(and(vec![term(0, Ge, "1"), excluded])), "and(0>=, not(0in))", "`NOT IN`");
    assert_eq!(rows(and(vec![term(1, Ge, "1"), list()])), "and(1>=, 0in)");

    // A decoded kind's `=` keys its field, so a bound is implied by key.
    let numeric = one_column("numeric", DataType::Utf8View);
    let numeric_term = |op: Comparing, value: &str| {
        let p = Predicate { column: "v".into(), op: op(value.into()) };
        ResolvedExpr::Term(resolve_term(&p, 0, &numeric, 0).unwrap())
    };
    let decoded = ResolvedExpr::In(membership(&numeric, 0, &[Some("1.50"), Some("2")]).unwrap());
    let tree = and(vec![numeric_term(Ge, "1.5"), numeric_term(Lt, "2"), decoded]);
    assert_eq!(rows(tree), "and(0<, 0in)");
}

/// **A tree and its row form answer `True` on the same rows**, wherever
/// the tree answers at all: every bound between two edges of a list,
/// over every list of up to three values from a small range, a NULL
/// among them or not, alone and beside a term on another column, under
/// an `Or`, and beneath a `Not`, over rows holding each value, a NULL
/// and a field no `integer` reads.
#[test]
fn a_tree_and_its_row_form_keep_the_same_rows() {
    let schema = two_integers();
    let term = |index: usize, op: Comparing, value: String| {
        let column = schema.schema.field(index).name().clone();
        let p = Predicate { column, op: op(value) };
        ResolvedExpr::Term(resolve_term(&p, index, &schema, 0).unwrap())
    };
    let values = ["0", "2", "4"];
    let mut lists: Vec<Vec<Option<&str>>> = vec![vec![], vec![None]];
    for mask in 1..8u32 {
        let chosen: Vec<_> =
            (0..3).filter(|bit| mask & (1 << bit) != 0).map(|bit| Some(values[bit])).collect();
        lists.push(chosen.clone());
        lists.push(chosen.into_iter().chain([None]).collect());
    }
    let rows: Vec<Vec<u8>> = ["-1", "0", "1", "2", "3", "4", "5", "\\N", "nope"]
        .iter()
        .flat_map(|a| ["1", "\\N"].map(|b| format!("{a}\t{b}").into_bytes()))
        .collect();
    let (mut compared, mut dropped) = (0, 0);
    for list in &lists {
        let listed = ResolvedExpr::In(membership(&schema, 0, list).unwrap());
        for op in [PredicateOp::Lt, PredicateOp::Le, PredicateOp::Gt, PredicateOp::Ge] {
            for edge in -1..=5 {
                let bound = term(0, op, edge.to_string());
                let conjunction = ResolvedExpr::And(vec![bound.clone(), listed.clone()]);
                let beside = ResolvedExpr::And(vec![
                    bound.clone(),
                    term(1, PredicateOp::Ge, "1".into()),
                    listed.clone(),
                ]);
                let trees = [
                    conjunction.clone(),
                    beside.clone(),
                    ResolvedExpr::Or(vec![beside, term(1, PredicateOp::Lt, "0".into())]),
                    ResolvedExpr::Not(Box::new(conjunction)),
                ];
                for tree in trees {
                    let row_form = tree.for_rows();
                    dropped += usize::from(shape(&row_form) != shape(&tree));
                    for row in &rows {
                        let at = RawRow::unchecked(row);
                        let full = tree.matches(at, &mut RowSplit::default(), "public.t", 0);
                        let Ok(full) = full else { continue };
                        let got = row_form.matches(at, &mut RowSplit::default(), "public.t", 0);
                        assert_eq!(
                            got.ok(),
                            Some(full),
                            "{} as {} over {:?}",
                            shape(&tree),
                            shape(&row_form),
                            String::from_utf8_lossy(row)
                        );
                        compared += 1;
                    }
                }
            }
        }
    }
    assert!(dropped > 100 && compared > 10_000, "{dropped} dropped, {compared} compared");
}

#[test]
fn missing_column_index_is_treated_as_null() {
    // Can't happen once a caller resolves the index from the block's own
    // schema; the fallback is exercised anyway.
    let p = Predicate { column: "x".into(), op: PredicateOp::Ne("a".into()) };
    assert_eq!(truth(&p, b"onlyone", 5), Truth::Unknown);
}

/// **A value is answered with no row around it, and the answer is the
/// row's** — every [`Comparison`] arm and both NULL-counting families,
/// over a NULL, values either side of the literal, and a value that is
/// not of the column's type. The one thing a value cannot carry is where
/// it came from, so the row path's `FieldDecode` is the value path's
/// `None`, and nothing else is.
#[test]
fn a_value_outside_any_row_is_answered_as_the_row_holding_it_is() {
    use pgdt_core::escape_field;
    let scalar = |declared: &str, op, literal: &str| {
        let schema = one_column(declared, DataType::Utf8View);
        resolve_term(&order_predicate(op, literal), 0, &schema, 0).unwrap()
    };
    let terms = [
        ("ordered", scalar("integer", PredicateOp::Le, "10")),
        ("canonical", scalar("integer", PredicateOp::Eq, "10")),
        ("trimmed", scalar("character(4)", PredicateOp::Ne, "10")),
        ("decoded", scalar("numeric", PredicateOp::IsNotDistinctFrom, "10")),
        ("distinct", scalar("integer", PredicateOp::IsDistinctFrom, "10")),
        (
            "null test",
            resolve_term(
                &Predicate { column: "v".into(), op: PredicateOp::IsNull },
                0,
                &one_column("integer", DataType::Int32),
                0,
            )
            .unwrap(),
        ),
        (
            "nested",
            resolve_term(
                &order_predicate(PredicateOp::Lt, "{10}"),
                0,
                &nested_column("integer[]", &[]),
                0,
            )
            .unwrap(),
        ),
    ];
    let values = [None, Some("9"), Some("10"), Some("10.0"), Some("10  "), Some("{9}"), Some("x")];
    let mut undecodable = 0;
    for (arm, term) in &terms {
        for value in values {
            let row = escape_field(value);
            let by_row =
                term.eval(RawRow::unchecked(&row), &mut RowSplit::default(), "public.t", 0);
            match (term.eval_value(value), by_row) {
                (Some(answer), Ok(by_row)) => assert_eq!(answer, by_row, "{arm} {value:?}"),
                (None, Err(Error::FieldDecode { .. })) => undecodable += 1,
                (answer, by_row) => panic!("{arm} {value:?}: {answer:?} against {by_row:?}"),
            }
        }
    }
    // The two decoding arms refuse `x` and the nested one every scalar
    // spelling, so the `None` half is reached rather than vacuous.
    assert!(undecodable >= 3, "only {undecodable} values refused");
}
