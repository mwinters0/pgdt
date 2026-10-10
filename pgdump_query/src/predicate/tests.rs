use std::sync::Arc;

use arrow::datatypes::{DataType, Field, IntervalUnit, Schema, TimeUnit};

use super::*;
use crate::copy::encode_field;
use crate::index::UnrepresentableTier;
use crate::pgtype::NestedPlan;
use crate::pgtype::{bounds_set_keyed_by, comparison_for, text_grammar};
use crate::preamble::{CollationDef, ColumnDef, TypeDef, TypeKind};
use crate::resolve::ColumnNote;
use crate::statistics::BoundsSet;
use crate::unrepresentable::scalar_read;

/// [`super::resolve_term`] in PostgreSQL's semantics, which every test
/// but the DataFusion-semantics ones asks for.
fn resolve_term(
    predicate: &Predicate,
    index: usize,
    resolved: &ResolvedSchema,
    header_offset: u64,
) -> Result<ResolvedTerm> {
    super::resolve_term(predicate, index, resolved, header_offset, ComparisonSemantics::Postgres)
}

/// A term resolved against a column the register has no plan for — what
/// every `Eq`/`Ne` on an unresolved column falls back to, and what the
/// two NULL tests always are.
fn text_term(p: &Predicate, index: usize) -> ResolvedTerm {
    let compared = p.value.as_ref().map(|value| ComparedTerm {
        column: p.column.clone(),
        comparison: Comparison::Canonical(Spellings::one(value.clone())),
        declared_type: String::new(),
        divergences: Vec::new(),
        statistics: BelievedStatistics::NONE,
    });
    let compared = if p.op.is_ordering() { None } else { compared };
    ResolvedTerm {
        op: p.op,
        index,
        compared,
        unrepresentable: None,
        tested: None,
        invalid: PostgresInvalidValues::Default,
        not_null: None,
    }
}

/// The one note a term announces, or `None` — a scalar column has at most
/// one diverging position.
#[track_caller]
fn only_note(term: &ResolvedTerm) -> Option<ComparisonNote> {
    let notes = term.comparison_notes();
    assert!(notes.len() <= 1, "a scalar column announces at most one note: {notes:?}");
    notes.into_iter().next()
}

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

/// The type list every one-column schema below resolves against: one
/// enum, so a column declared `public.mood` reaches the register's enum
/// arm rather than its "no such type" one.
fn test_types() -> Vec<TypeDef> {
    vec![
        TypeDef {
            name: "public.mood".into(),
            kind: TypeKind::Enum { labels: vec!["sad".into(), "ok".into()], exact: true },
        },
        // A range whose DDL stated no `subtype` — the one shape the
        // resolver calls nested and the register refuses outright, there
        // being no bound type to name the refusal after.
        TypeDef {
            name: "public.opaquerange".into(),
            kind: TypeKind::Range { subtype: None, multirange_type_name: None, canonical: None },
        },
    ]
}

/// A one-column `ResolvedSchema` for `declared`/`data_type`, mapped and
/// scalar — the shape an ordering term is allowed on.
///
/// The comparison plan comes from the register itself rather than being
/// stated here, so these tests exercise the same `declared -> plan` walk
/// `resolve_columns` makes.
fn one_column(declared: &str, data_type: DataType) -> ResolvedSchema {
    ResolvedSchema {
        schema: Arc::new(Schema::new(vec![Field::new("v", data_type, true)])),
        columns: vec![ColumnResolution::Mapped],
        notes: vec![ColumnNote {
            column: "v".into(),
            declared: Some(declared.into()),
            resolution: ColumnResolution::Mapped,
        }],
        plans: vec![NestedPlan::Scalar],
        comparisons: vec![comparison_for(declared, None, &test_types(), &[])],
        not_null: vec![false],
    }
}

/// A one-column schema for a **nested** declared type, resolved against
/// `types` so the Arrow type, the nested plan and the comparison plan
/// agree the way they do in a real query.
fn nested_column(declared: &str, types: &[TypeDef]) -> ResolvedSchema {
    let crate::pgtype::TypeOutcome::Mapped(data_type, plan) =
        crate::pgtype::resolve_declared_type(declared, types)
    else {
        panic!("{declared} does not resolve")
    };
    ResolvedSchema {
        schema: Arc::new(Schema::new(vec![Field::new("v", data_type, true)])),
        columns: vec![ColumnResolution::Mapped],
        notes: vec![ColumnNote {
            column: "v".into(),
            declared: Some(declared.into()),
            resolution: ColumnResolution::Mapped,
        }],
        plans: vec![plan],
        comparisons: vec![comparison_for(declared, None, types, &[])],
        not_null: vec![false],
    }
}

/// One nested term's verdict over a single-field row holding `field`.
#[track_caller]
fn nested_verdict(
    declared: &str,
    types: &[TypeDef],
    op: PredicateOp,
    field: &str,
    literal: &str,
) -> Result<Truth> {
    let p = Predicate { column: "v".into(), op, value: Some(literal.into()) };
    resolve_term(&p, 0, &nested_column(declared, types), 0)?.eval(
        RawRow::unchecked(field.as_bytes()),
        &mut RowSplit::default(),
        "public.t",
        0,
    )
}

fn order_predicate(op: PredicateOp, value: &str) -> Predicate {
    Predicate { column: "v".into(), op, value: Some(value.into()) }
}

/// Resolve `op value` against a one-column schema and evaluate it over a
/// single-field row holding `field`.
fn ordered(
    declared: &str,
    data_type: DataType,
    op: PredicateOp,
    value: &str,
    field: &str,
) -> Result<bool> {
    let p = order_predicate(op, value);
    let term = resolve_term(&p, 0, &one_column(declared, data_type), 0)?;
    Ok(term
        .eval(RawRow::unchecked(field.as_bytes()), &mut RowSplit::default(), "public.t", 0)?
        .is_true())
}

#[test]
fn eq_matches_the_decoded_value() {
    let p = Predicate { column: "x".into(), op: PredicateOp::Eq, value: Some("a\tb".into()) };
    assert_eq!(truth(&p, b"other\ta\\tb", 1), Truth::True);
    assert_eq!(truth(&p, b"other\tc", 1), Truth::False);
}

#[test]
fn ne_matches_everything_but_the_decoded_value() {
    let p = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
    assert_eq!(truth(&p, b"other\tb", 1), Truth::True);
    assert_eq!(truth(&p, b"other\ta", 1), Truth::False);
}

/// A NULL field is *unknown* under `=` and `!=`, not false, and a row is
/// kept only where the root is true.
#[test]
fn null_is_unknown_under_eq_and_ne() {
    let eq = Predicate { column: "x".into(), op: PredicateOp::Eq, value: Some("a".into()) };
    let ne = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
    assert_eq!(truth(&eq, b"other\t\\N", 1), Truth::Unknown);
    assert_eq!(truth(&ne, b"other\t\\N", 1), Truth::Unknown);
    assert!(!matches_all(&[eq, ne], &[1, 1], b"other\t\\N"));
}

/// `IS DISTINCT FROM` is `!=` with NULL counted as a value, which is the
/// one thing `NOT` cannot express: `NOT (x = a)` drops a NULL row.
#[test]
fn is_distinct_from_counts_null_as_a_value() {
    let idf =
        Predicate { column: "x".into(), op: PredicateOp::IsDistinctFrom, value: Some("a".into()) };
    let indf = Predicate {
        column: "x".into(),
        op: PredicateOp::IsNotDistinctFrom,
        value: Some("a".into()),
    };
    assert_eq!(truth(&idf, b"other\t\\N", 1), Truth::True);
    assert_eq!(truth(&idf, b"other\tb", 1), Truth::True);
    assert_eq!(truth(&idf, b"other\ta", 1), Truth::False);
    assert_eq!(truth(&indf, b"other\t\\N", 1), Truth::False);
    assert_eq!(truth(&indf, b"other\tb", 1), Truth::False);
    assert_eq!(truth(&indf, b"other\ta", 1), Truth::True);

    // The half `NOT` gets wrong, stated as the pair it is.
    let eq = Predicate { column: "x".into(), op: PredicateOp::Eq, value: Some("a".into()) };
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
    let is_null = Predicate { column: "x".into(), op: PredicateOp::IsNull, value: None };
    let is_not_null = Predicate { column: "x".into(), op: PredicateOp::IsNotNull, value: None };
    assert_eq!(truth(&is_null, b"other\t\\N", 1), Truth::True);
    assert_eq!(truth(&is_null, b"other\ta", 1), Truth::False);
    assert_eq!(truth(&is_not_null, b"other\t\\N", 1), Truth::False);
    assert_eq!(truth(&is_not_null, b"other\ta", 1), Truth::True);
}

/// `op` over a `date` column, testing in the tiers `reach` reads and read
/// as the null mode reads it there, or tested by nothing.
fn date_test(op: PredicateOp, reach: Option<UnrepresentableTier>) -> ResolvedExpr {
    let predicate = Predicate { column: "v".into(), op, value: None };
    let resolved = one_column("date", DataType::Date32);
    let term = resolve_term(&predicate, 0, &resolved, 0).unwrap();
    let reads = [reach.and_then(|reach| scalar_read(&DataType::Date32, reach))];
    ResolvedExpr::Term(term).reading(&reads).testing(&reads)
}

fn row_truth(expr: &ResolvedExpr, field: &str) -> Truth {
    let row = encode_field((field != "\\N").then_some(field));
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

/// A group of `rows` rows whose count, where it kept one, is `count`.
struct Counted {
    rows: u64,
    count: Option<Unrepresentable>,
}

impl GroupStatistics for Counted {
    fn rows(&self) -> u64 {
        self.rows
    }
    // Every row NULL to the null mode's view, which tells the test
    // nothing: it reads the count alone.
    fn null_count(&self, _: usize) -> Option<u64> {
        Some(self.rows)
    }
    fn bounds(&self, _: usize, _: &CompareKind) -> Option<(&str, &str)> {
        None
    }
    fn dictionary(&self, _: usize) -> Option<impl Iterator<Item = &str>> {
        None::<std::iter::Empty<&str>>
    }
    fn unrepresentable(&self, _: usize) -> Option<Unrepresentable> {
        self.count
    }
}

/// **A group answers the test off its own count, in the tiers the test
/// reads**, never off its NULL count, which in the null mode's view
/// counts the values the test matches; a group that kept no count can go
/// either way, and a column its block's count says holds none matches
/// nowhere.
#[test]
fn a_group_answers_the_unrepresentable_test_off_its_count() {
    use UnrepresentableTier::{Engine, Format};
    let group =
        |rows, format, engine| Counted { rows, count: Some(Unrepresentable { format, engine }) };
    let (t, f) = (Truth::True, Truth::False);
    let is = |reach| date_test(PredicateOp::IsUnrepresentable, Some(reach));
    let is_not = |reach| date_test(PredicateOp::IsNotUnrepresentable, Some(reach));
    assert_eq!(is(Format).truths(&group(3, 0, 2)), TruthSet::of(f));
    assert_eq!(is(Engine).truths(&group(3, 0, 2)), sets(&[t, f]));
    assert_eq!(is(Engine).truths(&group(3, 1, 2)), TruthSet::of(t));
    assert_eq!(is_not(Engine).truths(&group(3, 1, 2)), TruthSet::of(f));
    assert_eq!(is_not(Format).truths(&group(3, 0, 2)), TruthSet::of(t));
    assert_eq!(is(Format).truths(&Counted { rows: 3, count: None }), sets(&[t, f]));
    assert_eq!(is(Format).truths(&group(0, 0, 0)), TruthSet::EMPTY);
    let untested = date_test(PredicateOp::IsUnrepresentable, None);
    assert_eq!(untested.truths(&Counted { rows: 3, count: None }), TruthSet::of(f));
    let negated = date_test(PredicateOp::IsNotUnrepresentable, None);
    assert_eq!(negated.truths(&Counted { rows: 3, count: None }), TruthSet::of(t));
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
        Predicate { column: "a".into(), op: PredicateOp::Eq, value: Some("1".into()) },
        Predicate { column: "b".into(), op: PredicateOp::Eq, value: Some("2".into()) },
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
        Predicate { column: "a".into(), op: PredicateOp::Ne, value: Some("1".into()) },
        Predicate { column: "a".into(), op: PredicateOp::Ne, value: Some("2".into()) },
    ];
    assert!(matches_all(&filters, &[0, 0], b"3"));
    assert!(!matches_all(&filters, &[0, 0], b"2"));
}

/// The row every tree test below is evaluated over: field 0 holds `1` and
/// field 1 is NULL, so a leaf of each truth value is built out of real
/// terms rather than out of a constant.
const THREE_VALUED_ROW: &[u8] = b"1\t\\N";

/// A leaf that evaluates to `want` over [`THREE_VALUED_ROW`]: `f0 = 1`,
/// `f0 = 2`, and `f1 = 1` against the NULL field.
fn leaf(want: Truth) -> ResolvedExpr {
    let (index, value) = match want {
        Truth::True => (0, "1"),
        Truth::False => (0, "2"),
        Truth::Unknown => (1, "1"),
    };
    let p = Predicate { column: "v".into(), op: PredicateOp::Eq, value: Some(value.into()) };
    ResolvedExpr::Term(text_term(&p, index))
}

fn exact(expr: &ResolvedExpr) -> Truth {
    expr.eval(true, RawRow::unchecked(THREE_VALUED_ROW), &mut RowSplit::default(), "public.t", 0)
        .unwrap()
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

/// Every [`TruthSet`], the empty one included.
fn all_sets() -> impl Iterator<Item = TruthSet> {
    (0..8).map(TruthSet)
}

/// **The set operations are Kleene's tables lifted**, asserted against
/// the row evaluator itself rather than against a second copy of the
/// tables: over every pair of sets, `A AND B` is exactly what `a AND b`
/// evaluates to over real terms for some `a` in `A` and `b` in `B`, and so
/// for `OR` and `NOT`.
#[test]
fn truth_sets_combine_as_their_members_do() {
    for a in all_sets() {
        let negated = a.members().map(|x| exact(&ResolvedExpr::Not(Box::new(leaf(x)))));
        assert_eq!(a.not(), negated.fold(TruthSet::EMPTY, |s, t| s.union(TruthSet::of(t))));
        for b in all_sets() {
            let (mut and, mut or) = (TruthSet::EMPTY, TruthSet::EMPTY);
            for x in a.members() {
                for y in b.members() {
                    let pair = || vec![leaf(x), leaf(y)];
                    and = and.union(TruthSet::of(exact(&ResolvedExpr::And(pair()))));
                    or = or.union(TruthSet::of(exact(&ResolvedExpr::Or(pair()))));
                }
            }
            assert_eq!(a.and(b), and, "{a:?} AND {b:?}");
            assert_eq!(a.or(b), or, "{a:?} OR {b:?}");
        }
    }
}

/// One column's statistics for one row group, stated by hand — the
/// shape [`GroupStatistics`] reads, with every column index answered
/// alike.
#[derive(Debug, Clone, Default)]
struct Group {
    rows: u64,
    nulls: Option<u64>,
    bounds: Option<(String, String)>,
    dictionary: Option<Vec<String>>,
}

impl GroupStatistics for Group {
    fn rows(&self) -> u64 {
        self.rows
    }
    fn null_count(&self, _: usize) -> Option<u64> {
        self.nulls
    }
    fn bounds(&self, _: usize, _: &CompareKind) -> Option<(&str, &str)> {
        self.bounds.as_ref().map(|(min, max)| (min.as_str(), max.as_str()))
    }
    fn dictionary(&self, _: usize) -> Option<impl Iterator<Item = &str>> {
        self.dictionary.as_ref().map(|entries| entries.iter().map(String::as_str))
    }
    fn unrepresentable(&self, _: usize) -> Option<Unrepresentable> {
        None
    }
}

fn sets(truths: &[Truth]) -> TruthSet {
    truths.iter().fold(TruthSet::EMPTY, |set, t| set.union(TruthSet::of(*t)))
}

/// What a group's statistics let each kind of term say, one case per
/// rule, by hand: a column the statistics bound, a group with NULLs, a
/// `NOT` over both, the equality operators' one-sided reading of bounds,
/// a dictionary a decoded kind reads by key, and statistics a term's
/// comparison does not share.
#[test]
fn a_group_is_answered_from_its_statistics() {
    use Truth::{False, True, Unknown};
    let term = |declared: &str, op, literal: Option<&str>| {
        let p = Predicate { column: "v".into(), op, value: literal.map(str::to_string) };
        ResolvedExpr::Term(
            resolve_term(&p, 0, &one_column(declared, DataType::Utf8View), 0).unwrap(),
        )
    };
    let bounded = |min: &str, max: &str, nulls: u64| Group {
        rows: 4,
        nulls: Some(nulls),
        bounds: Some((min.into(), max.into())),
        dictionary: None,
    };
    let listed = |entries: &[&str]| Group {
        rows: 4,
        nulls: Some(0),
        bounds: None,
        dictionary: Some(entries.iter().map(|e| e.to_string()).collect()),
    };
    let lt5 = term("integer", PredicateOp::Lt, Some("5"));
    let not_lt5 = ResolvedExpr::Not(Box::new(lt5.clone()));
    let cases: Vec<(&str, &ResolvedExpr, Group, TruthSet)> = vec![
        ("above the literal", &lt5, bounded("6", "9", 0), sets(&[False])),
        ("straddling it", &lt5, bounded("4", "9", 0), sets(&[True, False])),
        ("with NULLs", &lt5, bounded("6", "9", 1), sets(&[False, Unknown])),
        // A "may match" flag would call this group empty.
        ("negated", &not_lt5, bounded("6", "9", 1), sets(&[True, Unknown])),
        ("no rows", &lt5, Group { rows: 0, nulls: Some(0), ..Group::default() }, sets(&[])),
        (
            "no statistics",
            &lt5,
            Group { rows: 4, ..Group::default() },
            sets(&[True, False, Unknown]),
        ),
        (
            "only NULLs",
            &lt5,
            Group { rows: 4, nulls: Some(4), ..Group::default() },
            sets(&[Unknown]),
        ),
    ];
    let eq5 = term("integer", PredicateOp::Eq, Some("5"));
    let ne5 = term("integer", PredicateOp::Ne, Some("5"));
    let distinct = term("integer", PredicateOp::IsDistinctFrom, Some("5"));
    let is_null = term("integer", PredicateOp::IsNull, None);
    let decimal = term("numeric", PredicateOp::Eq, Some("1.5"));
    let unknown_collation_lt = term("text", PredicateOp::Lt, Some("b"));
    let unknown_collation_eq = term("text", PredicateOp::Eq, Some("b"));
    let more: Vec<(&str, &ResolvedExpr, Group, TruthSet)> = vec![
        ("= outside the bounds", &eq5, bounded("6", "9", 0), sets(&[False])),
        // Bounds equal to the literal say nothing about spellings.
        ("!= on bounds equal to it", &ne5, bounded("5", "5", 0), sets(&[True, False])),
        ("!= on a dictionary of it", &ne5, listed(&["5"]), sets(&[False])),
        ("a dictionary without it", &eq5, listed(&["6", "7"]), sets(&[False])),
        (
            "both, agreeing",
            &eq5,
            Group { bounds: Some(("1".into(), "9".into())), ..listed(&["6"]) },
            sets(&[False]),
        ),
        (
            "DISTINCT over NULLs",
            &distinct,
            Group { rows: 2, nulls: Some(2), ..Group::default() },
            sets(&[True]),
        ),
        (
            "IS NULL with none",
            &is_null,
            Group { rows: 2, nulls: Some(0), ..Group::default() },
            sets(&[False]),
        ),
        ("a decoded kind's entry", &decimal, listed(&["1.50"]), sets(&[True])),
        (
            "bounds under an unknown collation",
            &unknown_collation_lt,
            bounded("c", "d", 0),
            sets(&[True, False]),
        ),
        ("its dictionary", &unknown_collation_eq, listed(&["a"]), sets(&[False])),
    ];
    for (case, expr, group, want) in cases.into_iter().chain(more) {
        assert_eq!(expr.truths(&group), want, "{case}");
    }
}

/// A two-column schema, both `integer`, for the tests about *where* a
/// decode failure surfaces.
fn two_integers() -> ResolvedSchema {
    let note = |name: &str| ColumnNote {
        column: name.into(),
        declared: Some("integer".into()),
        resolution: ColumnResolution::Mapped,
    };
    ResolvedSchema {
        schema: Arc::new(Schema::new(vec![
            Field::new("a", DataType::Int32, true),
            Field::new("b", DataType::Int32, true),
        ])),
        columns: vec![ColumnResolution::Mapped; 2],
        notes: vec![note("a"), note("b")],
        plans: vec![NestedPlan::Scalar; 2],
        comparisons: vec![comparison_for("integer", None, &[], &[]); 2],
        not_null: vec![false; 2],
    }
}

/// A decode failure surfaces only where evaluation reaches it, so which
/// rows error depends on where the term sits in the tree — and on whether
/// a `Not` sits above it, a `Not` being the one node that has to tell
/// `False` from `Unknown` and so cannot short-circuit an unknown away.
#[test]
fn a_decode_failure_surfaces_only_where_it_is_reached() {
    let schema = two_integers();
    let term = |column: &str, op, value: &str, index| {
        let p = Predicate { column: column.into(), op, value: Some(value.into()) };
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
        let p = Predicate { column: "c".into(), op: PredicateOp::Eq, value: Some("x".into()) };
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

/// `column IN (values)` over `schema`'s column `index`, in PostgreSQL's
/// semantics, `None` in `values` a NULL.
fn membership(
    schema: &ResolvedSchema,
    index: usize,
    values: &[Option<&str>],
) -> Result<ResolvedMembership> {
    let membership = Membership {
        column: schema.schema.field(index).name().clone(),
        values: values.iter().map(|v| v.map(str::to_string)).collect(),
    };
    resolve_membership(&membership, index, schema, 0, ComparisonSemantics::Postgres)
}

/// A tree's exact value over `row`, as a `Not` above it asks for it.
fn exact_over(expr: &ResolvedExpr, row: &[u8]) -> Result<Truth> {
    expr.eval(true, RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
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
                        let p = Predicate {
                            column: "a".into(),
                            op: PredicateOp::Eq,
                            value: Some((*v).into()),
                        };
                        ResolvedExpr::Term(resolve_term(&p, 0, &schema, 0).unwrap())
                    }
                    None => {
                        let p = Predicate {
                            column: "z".into(),
                            op: PredicateOp::Eq,
                            value: Some("0".into()),
                        };
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

/// **A row group is answered as the `Or` answers it**, its terms' sets
/// combined through Kleene's `OR` and a NULL in the list adding `Unknown`
/// over a group any row starts in — no tighter, though one row can equal
/// at most one value.
#[test]
fn a_membership_answers_a_group_as_its_disjunction_does() {
    let schema = two_integers();
    let groups = [
        Group { rows: 0, ..Group::default() },
        Group { rows: 4, nulls: Some(0), ..Group::default() },
        Group { rows: 4, nulls: Some(4), ..Group::default() },
        Group {
            rows: 4,
            nulls: Some(1),
            bounds: Some(("2".into(), "5".into())),
            ..Group::default()
        },
        Group {
            rows: 4,
            nulls: None,
            dictionary: Some(vec!["1".into(), "9".into()]),
            ..Group::default()
        },
    ];
    let lists: [&[Option<&str>]; 5] =
        [&[Some("1"), Some("3")], &[Some("1"), None], &[None], &[], &[Some("9"), Some("1")]];
    for list in lists {
        let resolved = membership(&schema, 0, list).unwrap();
        for group in &groups {
            let want = resolved
                .terms
                .iter()
                .fold(TruthSet::of(Truth::False), |set, term| set.or(term.truths(group)));
            let want = match (resolved.null, group.rows) {
                (false, _) => want,
                (true, 0) => TruthSet::EMPTY,
                (true, _) => want.or(TruthSet::of(Truth::Unknown)),
            };
            assert_eq!(resolved.truths(group), want, "{list:?} over {group:?}");
        }
    }
    let nulls_only = membership(&schema, 0, &[None]).unwrap();
    assert_eq!(nulls_only.truths(&groups[0]), TruthSet::EMPTY);
    assert_eq!(nulls_only.truths(&groups[1]), TruthSet::of(Truth::Unknown));
    // Every row of this group is `1` or `9`, so each is in the list, but
    // each term alone can be false of it, and so is the `Or` taken here.
    let dictionary = membership(&schema, 0, &[Some("9"), Some("1")]).unwrap();
    let all_listed = Group { nulls: Some(0), ..groups[4].clone() };
    assert_eq!(dictionary.truths(&all_listed), sets(&[Truth::True, Truth::False]));
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
            kind: CompareKind::Text { length: None },
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
    let term = |index: usize, op, value: &str| {
        let column = schema.schema.field(index).name().clone();
        let p = Predicate { column, op, value: Some(value.into()) };
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
    assert_eq!(rows(and(vec![term(1, Ge, "1"), list()])), "and(1>=, 0in)");

    // A decoded kind's `=` keys its field, so a bound is implied by key.
    let numeric = one_column("numeric", DataType::Utf8View);
    let numeric_term = |op, value: &str| {
        let p = Predicate { column: "v".into(), op, value: Some(value.into()) };
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
    let term = |index: usize, op, value: String| {
        let column = schema.schema.field(index).name().clone();
        let p = Predicate { column, op, value: Some(value) };
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

/// **A group's bounds are keyed once for every term reading them**: the
/// keys held answer a second ask of the same column and kind, a group's
/// bounds there being one pair of texts, and another column is keyed
/// afresh.
#[test]
fn a_group_s_bounds_are_keyed_once_per_column_and_kind() {
    let kind = CompareKind::Int { bytes: 8 };
    let mut keyed = KeyedBounds::default();
    let key = |text: &str| order_key(&kind, text).unwrap();
    let held = |keys: Option<(&OrderKey, &OrderKey)>| {
        keys.map(|(low, high)| (compare_keys(low, &key("1")), compare_keys(high, &key("9"))))
    };
    assert_eq!(held(keyed.keys(0, &kind, "1", "9")), Some((Ordering::Equal, Ordering::Equal)));
    assert_eq!(held(keyed.keys(0, &kind, "2", "8")), Some((Ordering::Equal, Ordering::Equal)));
    assert_eq!(held(keyed.keys(1, &kind, "2", "8")), Some((Ordering::Greater, Ordering::Less)));
    assert_eq!(keyed.keys(2, &kind, "x", "8"), None, "a bound that does not key");
}

#[test]
fn missing_column_index_is_treated_as_null() {
    // Can't happen once a caller resolves the index from the block's own
    // schema; the fallback is exercised anyway.
    let p = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
    assert_eq!(truth(&p, b"onlyone", 5), Truth::Unknown);
}

/// The four operators over the boundary itself — the case a `<` / `<=`
/// pair differs on.
#[test]
fn the_four_operators_differ_only_at_the_boundary() {
    for (op, below, at, above) in [
        (PredicateOp::Lt, true, false, false),
        (PredicateOp::Le, true, true, false),
        (PredicateOp::Gt, false, false, true),
        (PredicateOp::Ge, false, true, true),
    ] {
        for (field, expected) in [("4", below), ("5", at), ("6", above)] {
            assert_eq!(
                ordered("integer", DataType::Int32, op, "5", field).unwrap(),
                expected,
                "{} {} 5",
                field,
                op.symbol()
            );
        }
    }
}

/// Numbers order as numbers: the text comparison `Eq` uses would put `9`
/// after `10`.
#[test]
fn integers_order_numerically_not_lexicographically() {
    assert!(ordered("integer", DataType::Int32, PredicateOp::Gt, "9", "10").unwrap());
    assert!(!ordered("integer", DataType::Int32, PredicateOp::Lt, "9", "10").unwrap());
}

/// An `oid` is unsigned across its whole range: `4294967295` is above
/// `2147483648`, which an `Int32` reading would make two negative
/// numbers (I39).
#[test]
fn an_oid_orders_over_the_whole_unsigned_range() {
    let oid = |op, value, field| ordered("oid", DataType::UInt32, op, value, field).unwrap();
    assert!(oid(PredicateOp::Gt, "2147483648", "4294967295"));
    assert!(oid(PredicateOp::Lt, "2147483648", "0"));
    assert!(oid(PredicateOp::Ge, "4294967295", "4294967295"));
}

/// `oidin` reads a signed literal by wrapping it — `-1` is 4294967295 —
/// and this build does not implement that, so the literal is refused
/// rather than read as −1.
#[test]
fn a_signed_oid_literal_is_refused_rather_than_wrapped() {
    let p = order_predicate(PredicateOp::Lt, "-1");
    let err = resolve_term(&p, 0, &one_column("oid", DataType::UInt32), 0).unwrap_err();
    assert!(
        matches!(err, Error::PredicateValueDecode { ref column, .. } if column == "v"),
        "{err:?}"
    );
}

/// A `numeric(10,2)` literal and field are both taken to the column's
/// scale before comparing, so `1.5` and `1.50` are one value.
#[test]
fn a_decimal_compares_at_the_columns_scale() {
    let t = DataType::Decimal128(10, 2);
    assert!(ordered("numeric(10,2)", t.clone(), PredicateOp::Ge, "1.5", "1.50").unwrap());
    assert!(ordered("numeric(10,2)", t.clone(), PredicateOp::Le, "1.5", "1.50").unwrap());
    assert!(ordered("numeric(10,2)", t, PredicateOp::Lt, "1.5", "-1.50").unwrap());
}

/// A literal finer than the column's scale is refused rather than
/// rounded: the column's own decoder does not drop non-zero digits.
#[test]
fn a_literal_finer_than_the_columns_scale_is_refused() {
    let err =
        ordered("numeric(10,2)", DataType::Decimal128(10, 2), PredicateOp::Gt, "1.005", "1.00")
            .unwrap_err();
    assert!(
        matches!(err, Error::PredicateValueDecode { ref value, .. } if value == "1.005"),
        "{err:?}"
    );
}

/// PostgreSQL's NaN, not Rust's: greater than everything, equal to
/// itself, and never `None`.
#[test]
fn nan_is_the_largest_float_and_equals_itself() {
    let f = |op, value, field| {
        ordered("double precision", DataType::Float64, op, value, field).unwrap()
    };
    assert!(f(PredicateOp::Gt, "Infinity", "NaN"));
    assert!(f(PredicateOp::Ge, "NaN", "NaN"));
    assert!(f(PredicateOp::Le, "NaN", "NaN"));
    assert!(!f(PredicateOp::Gt, "NaN", "NaN"));
    assert!(f(PredicateOp::Lt, "NaN", "-Infinity"));
}

/// **A float literal past its type's range is refused, as `float4in` and
/// `float8in` refuse it** (I59) — overflowing, I57's rounded largest
/// finite value included, or a nonzero spelling underflowing to zero —
/// under `=`, an ordering operator and inside a nested literal alike,
/// while a subnormal, a zero spelled with any exponent and the server's
/// other spellings of a value in range are read by value. Each case was
/// cast on PostgreSQL 16.
#[test]
fn a_float_literal_past_its_range_is_refused_and_one_inside_it_read() {
    let refused = |declared: &str, data_type: DataType, literal: &str| {
        for op in [PredicateOp::Eq, PredicateOp::Ge] {
            let p = order_predicate(op, literal);
            let err = resolve_term(&p, 0, &one_column(declared, data_type.clone()), 0).unwrap_err();
            assert!(
                matches!(&err, Error::PredicateValueDecode { value, .. } if value == literal),
                "{declared} {op:?} {literal}: {err:?}"
            );
        }
    };
    for literal in [
        "1e400",
        "-1e400",
        "1e-400",
        "-1e-400",
        "2e-324",
        "1.79769313486232e+308",
        "-1.79769313486232e+308",
        "1.7976931348623159e+308",
    ] {
        refused("double precision", DataType::Float64, literal);
    }
    for literal in ["1e400", "1e-400", "1e-46", "3.403e+38", "-3.403e+38", "3.4028236e+38"] {
        refused("real", DataType::Float32, literal);
    }

    let double = |literal, field| {
        ordered("double precision", DataType::Float64, PredicateOp::Eq, literal, field).unwrap()
    };
    for (literal, field) in [
        ("1e-310", "1e-310"),
        ("3e-324", "5e-324"),
        ("1.7976931348623158e+308", "1.7976931348623157e+308"),
        ("0e-999", "0"),
        ("-0e-400", "0"),
        ("0.0000e-500", "-0"),
        ("+inf", "Infinity"),
        ("INFINITY", "Infinity"),
        ("-Inf", "-Infinity"),
        ("-nan", "NaN"),
        ("1E5", "100000"),
        (".5", "0.5"),
        ("5.", "5"),
    ] {
        assert!(double(literal, field), "{literal} = {field}");
    }
    let real = |literal, field| {
        ordered("real", DataType::Float32, PredicateOp::Eq, literal, field).unwrap()
    };
    assert!(real("1e-45", "1e-45"));
    assert!(real("3.4028235e+38", "3.4028235e+38"));
    // The field's reader is the literal's: I57's spelling in a field is
    // refused too, where it is read.
    let field = ordered(
        "double precision",
        DataType::Float64,
        PredicateOp::Eq,
        "1.7976931348623157e+308",
        "1.79769313486232e+308",
    );
    assert!(matches!(field, Err(Error::FieldDecode { .. })), "{field:?}");

    let nested = nested_verdict("double precision[]", &[], PredicateOp::Eq, "{1}", "{1,1e400}");
    assert!(matches!(nested, Err(Error::PredicateValueDecode { .. })), "{nested:?}");
    let nested = nested_verdict("double precision[]", &[], PredicateOp::Eq, "{1e-310}", "{1e-310}");
    assert_eq!(nested.unwrap(), Truth::True);
}

/// **A strict parse finds a refusal anywhere in a field** ([`field_refused`]):
/// a scalar where [`field_key`] refuses it, and a nested value at its
/// container's input grammar, at any element read as a field of its own
/// type — under a position this build does not order too — and at a
/// range's bound order or a discrete bound's successor (I44, I46, I47). A
/// spelling the input grammar reads and the output grammar never writes is
/// no refusal, nor is any text of a type read as its text but `json`, at
/// the top level or beneath a container, which `json_in`'s grammar reads
/// (I72), and `bit` and `bit varying`, whose grammar is read in place of
/// the order they lack, their typmod with it, through a domain too (I73),
/// as are the geometric types', a `box` array's split at `;` (I74, I22),
/// and a `character varying(n)`'s or `character(n)`'s length (I75).
#[test]
fn a_strict_check_finds_a_refusal_anywhere_in_a_field() {
    let types = vec![
        TypeDef {
            name: "public.pair".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("a", "integer"), ColumnDef::new("b", "smallint")]),
            },
        },
        TypeDef {
            name: "public.mood".into(),
            kind: TypeKind::Enum { labels: vec!["sad".into(), "ok".into()], exact: true },
        },
        TypeDef {
            name: "public.jsonpair".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("ok", "integer"), ColumnDef::new("doc", "json")]),
            },
        },
        TypeDef { name: "public.bits3".into(), kind: TypeKind::domain("bit(3)") },
        TypeDef {
            name: "public.flags".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("a", "bit"),
                    ColumnDef::new("b", "public.bits3"),
                    ColumnDef::new("c", "bit varying(2)"),
                ]),
            },
        },
        TypeDef { name: "public.ring".into(), kind: TypeKind::domain("circle") },
        TypeDef { name: "public.short".into(), kind: TypeKind::domain("character varying(2)") },
        TypeDef {
            name: "public.shapes".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("b", "box[]"), ColumnDef::new("p", "point")]),
            },
        },
        not_null_domain("public.nn", "integer"),
        not_null_domain("public.nnbox", "box"),
        not_null_domain("public.nnmoney", "money"),
        TypeDef {
            name: "public.nnpair".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("a", "public.nn"),
                    ColumnDef::new("b", "integer"),
                ]),
            },
        },
        TypeDef {
            name: "public.nnrange".into(),
            kind: TypeKind::Range {
                subtype: Some("public.nn".into()),
                multirange_type_name: None,
                canonical: None,
            },
        },
    ];
    // As `gather::field_checks` reads a column: a plan ordering nothing
    // by its declared type's grammar, where it has one.
    let refused = |declared: &str, text: &str| {
        let plan = comparison_for(declared, None, &types, &[]);
        match (&plan, text_grammar(declared, &types)) {
            (ComparisonPlan::Refused, Some(grammar)) => grammar_refuses(grammar, text),
            _ => field_refused(&plan, text),
        }
    };
    let cases: [(&str, &str, bool); 98] = [
        ("smallint", "70000", true),
        ("smallint", "7", false),
        ("smallint", " 7", false),
        ("text", "anything", false),
        ("character varying(3)", "abcd", true),
        ("character varying(3)", "abc   ", false),
        ("character(3)", "abc d", true),
        ("character(3)", "ab ", false),
        ("character", "ab", true),
        ("bpchar", "abcd", false),
        ("character varying(2)[]", "{ab,abc}", true),
        ("public.short", "abc", true),
        ("public.short[]", "{ab}", false),
        ("money", "not money", false),
        ("public.mood", "happy", true),
        ("public.mood", "ok", false),
        ("smallint[]", "{1,70000}", true),
        ("smallint[]", "{{1},{70000}}", true),
        ("smallint[]", "{1,2", true),
        ("smallint[]", "{ 1 , 2 }", false),
        ("smallint[]", "{1,NULL}", false),
        ("public.mood[]", "{ok,happy}", true),
        ("money[]", "{1", true),
        ("money[]", "{$1.00}", false),
        ("public.pair", "(1,70000)", true),
        ("public.pair", "(1)", true),
        ("public.pair", "(1,2)", false),
        ("public.pair[]", "{\"(1,2)\",\"(1,70000)\"}", true),
        ("int4range", "[5,1]", true),
        ("int4range", "[1,5)", false),
        ("int4range", "[1,2147483647]", true),
        ("int4range", "[1,70000000000)", true),
        ("int4multirange", "{[1,2), [5,1]}", true),
        ("int4multirange", "{[1,2), [5,6)}", false),
        ("int2vector", "1 70000", true),
        ("json", "{bad", true),
        ("json", "[1,]", true),
        ("json", r#""\u0000""#, false),
        ("json", r#" [1, {"a": null}] "#, false),
        ("json[]", r#"{1,"[1, 2]"}"#, false),
        ("json[]", "{1,x}", true),
        ("json[]", "{1,NULL}", false),
        ("public.jsonpair", r#"(1,"[1, 2]")"#, false),
        ("public.jsonpair", "(1,{bad)", true),
        ("public.jsonpair[]", r#"{"(1,{})","(2,[)"}"#, true),
        ("bit", "1", false),
        ("bit", "10", true),
        ("\"bit\"", "10", false),
        ("bit(3)", "x5", true),
        ("bit(4)", "x5", false),
        ("bit varying(2)", "111", true),
        ("varbit", "1012", true),
        ("public.bits3", "101", false),
        ("public.bits3", "10", true),
        ("bit(3)[]", "{101,10}", true),
        ("bit(3)[]", "{101,NULL}", false),
        ("varbit(2)[]", "{1,11}", false),
        ("public.bits3[]", "{1}", true),
        ("public.flags", "(1,101,11)", false),
        ("public.flags", "(11,101,11)", true),
        ("public.flags", "(1,10,11)", true),
        ("public.flags", "(1,101,111)", true),
        ("point", "(1,2)", false),
        ("point", "(1,2", true),
        ("line", "{0,0,1}", true),
        ("line", "[(2,0),(3,1.7976931348623157e308)]", false),
        ("lseg", "[(1,2),(3,4))", false),
        ("box", "[(1,2),(3,4)]", true),
        ("path", "(1,2),(3,4),", true),
        ("polygon", "(1,2)", false),
        ("circle", "<(1,2),-1>", true),
        ("public.ring", "<(1,2),1e400>", true),
        ("point[]", r#"{"(1,2)","(3,x)"}"#, true),
        ("box[]", "{(1,1),(0,0);(2,2),(1,1)}", false),
        ("box[]", r#"{"(1,1),(0,0)";(2,2),(1,1)}"#, false),
        ("box[]", "{(1,1),(0,0);(2,2)}", true),
        ("public.shapes", r#"("{(1,1),(0,0);(2,2),(1,1)}","(1,2)")"#, false),
        ("public.shapes", r#"("{""(1,1),(0,0)"";(2,2),(1,1)}","(1,2)")"#, false),
        ("public.shapes", r#"("{(1,1),(0,0);(2,2)}","(1,2)")"#, true),
        ("public.shapes", r#"("{(1,1),(0,0)}",(1))"#, true),
        ("pg_node_tree", "{QUERY}", true),
        ("pg_mcv_list[]", "{NULL}", false),
        ("pg_mcv_list[]", "{x}", true),
        ("\"char\"", "anything", false),
        ("refcursor", "", false),
        ("xid", "abc", false),
        ("cid", "4294967296", false),
        ("xid8[]", "{1,abc}", false),
        ("xid8[]", "{1,abc", true),
        ("public.nn[]", "{1,NULL}", true),
        ("public.nn[]", "{{1,2},{3,NULL}}", true),
        ("public.nn[]", "{1,2}", false),
        ("public.nnpair", "(,2)", true),
        ("public.nnpair", "(1,)", false),
        ("public.nnpair[]", r#"{"(,2)"}"#, true),
        ("public.nnpair[]", r#"{"(1,)",NULL}"#, false),
        ("public.nnmoney[]", "{NULL}", true),
        ("public.nnrange", "(,3)", false),
    ];
    for (declared, text, expected) in cases {
        assert_eq!(refused(declared, text), expected, "{declared} {text:?}");
    }
    // A `box[]` column resolves to text with its plan refused (I22), and is
    // read by its own grammar.
    for (text, expected) in [
        ("{(1,1),(0,0);(2,2),(1,1)}", false),
        (r#"{"(1,1),(0,0)";(2,2),(1,1)}"#, false),
        ("{{(1,1),(0,0)};{(2,2),(1,1)}}", false),
        ("{NULL;(1,1),(0,0)}", false),
        ("{(1,1),(0,0);(2,2)}", true),
        ("{(1,1),(0,0),(2,2),(1,1)}", true),
        ("{(1,1),(0,0)", true),
    ] {
        assert_eq!(
            grammar_refuses(TextGrammar::BoxArray { not_null: false }, text),
            expected,
            "{text:?}"
        );
    }
    assert!(refused("public.nnbox[]", "{NULL;(1,1),(0,0)}"));
    assert!(!refused("public.nnbox[]", "{(1,1),(0,0)}"));
}

/// A domain over `base` declaring `NOT NULL`.
fn not_null_domain(name: &str, base: &str) -> TypeDef {
    TypeDef {
        name: name.into(),
        kind: TypeKind::Domain {
            base_type: base.into(),
            collation: None,
            not_null: true,
            check: false,
        },
    }
}

/// **A base type's array beneath a container is split at the `DELIMITER`
/// its `CREATE TYPE` declares** (I22), through a domain and a typmod, so a
/// quoted element followed by it is read, as `array_in` reads it, and one
/// followed by `,` refused; a delimiter the preamble could not read, or one
/// `array_in` reads as syntax at some major, splits nothing and refuses
/// nothing but the container's own grammar.
#[test]
fn a_base_type_s_array_beneath_a_container_is_split_at_its_delimiter() {
    let base =
        |name: &str, delimiter| TypeDef { name: name.into(), kind: TypeKind::Base { delimiter } };
    let mut types = vec![
        base("public.semi", Some(b';')),
        base("public.pipe", Some(b'|')),
        base("public.plain", Some(b',')),
        base("public.unread", None),
        base("public.backslash", Some(b'\\')),
        TypeDef { name: "public.semid".into(), kind: TypeKind::domain("public.semi") },
    ];
    for (field, text, expected) in [
        ("public.semi[]", r#"("{""a b"";c}")"#, false),
        ("public.semi[]", r#"("{a,b;c}")"#, false),
        ("public.semi[]", r#"("{{a;b};{c;d}}")"#, false),
        ("public.semi[]", r#"("{{a;b};{c}}")"#, true),
        ("public.semi[]", r#"("{""a b"",c}")"#, true),
        ("public.semi(8)[]", r#"("{""a b"";c}")"#, false),
        ("public.semid[]", r#"("{""a b"";c}")"#, false),
        ("public.semid[]", r#"("{""a b"",c}")"#, true),
        ("public.pipe[]", r#"("{""a b""|c}")"#, false),
        ("public.plain[]", r#"("{""a b"",c}")"#, false),
        ("public.plain[]", r#"("{""a b"";c}")"#, true),
        ("public.unread[]", r#"("{""a b"";c}")"#, false),
        ("public.unread[]", r#"("{""a b"",c}")"#, false),
        ("public.unread[]", r#"("{a}""#, true),
        ("public.backslash[]", r#"("{""a b""\\c}")"#, false),
    ] {
        types.retain(|t| t.name != "public.holder");
        types.push(TypeDef {
            name: "public.holder".into(),
            kind: TypeKind::Composite { fields: Some(vec![ColumnDef::new("v", field)]) },
        });
        let plan = comparison_for("public.holder", None, &types, &[]);
        assert_eq!(field_refused(&plan, text), expected, "{field} {text:?}");
    }
}

/// **A field PostgreSQL refuses is told from one this build does not
/// read** ([`Unread`]): every check carrying a `pg-refuses` marker answers
/// `Refused`, which fails the parse keying the field, and a spelling the
/// server reads where this build does not answers `Unparsed`, which does
/// not — nor does a label an enum whose labels are not read exactly
/// lacks, nor a `macaddr` octet opening with a sign (`KD85`).
#[test]
fn a_field_postgresql_refuses_is_told_from_one_this_build_does_not_read() {
    use CompareKind as K;
    let bare = K::Numeric { infinities: true, typmod: None };
    let past_dscale = format!("1.{}", "0".repeat(decode::NUMERIC_DSCALE_MAX + 1));
    // `numeric_in` reads it at a scale within its bounds.
    let past_dscale_by_exponent = format!("{past_dscale}e5");
    // `NAMEDATALEN` less one is the longest label a type can hold (I70).
    let longest_label = "x".repeat(ENUM_LABEL_LIMIT - 1);
    let long_label = "x".repeat(ENUM_LABEL_LIMIT);
    let refused: Vec<(CompareKind, &str)> = vec![
        (K::Int { bytes: 2 }, "70000"),
        (K::Int { bytes: 4 }, "-2147483649"),
        (K::Int { bytes: 8 }, "9223372036854775808"),
        (K::Float64, "1.79769313486232e+308"),
        (K::Float64, "1e-400"),
        (K::Float32, "3.403e+38"),
        (K::Decimal { precision: 2, scale: 5 }, "0.001"),
        (
            K::Numeric {
                infinities: false,
                typmod: Some(crate::pgtype::NumericTypmod { precision: 3, scale: 0 }),
            },
            "1000",
        ),
        (bare.clone(), &past_dscale),
        (K::Date, "2020-02-30"),
        (K::Date, "0000-01-01"),
        (K::Time, "24:00:01"),
        (K::Timestamp { with_tz: false }, "294277-01-01 00:00:00"),
        (K::Timestamp { with_tz: true }, "2020-01-01 00:00:00+16"),
        (K::Interval { qualifier: None }, "00:90:00"),
        (K::IntervalFields { qualifier: None }, "1 day 1 day"),
        (K::TimeTz, "12:00:00+16"),
        (K::Network { cidr: true }, "10.0.0.1/8"),
        (K::Jsonb, "\"a\u{1}\""),
        (K::Jsonb, r#"["\u0000"]"#),
        (K::Jsonb, r#""\udc00""#),
        (K::Jsonb, "[01]"),
        (K::Jsonb, "1."),
        (K::Uuid, "-00000000-0000-0000-0000-000000000000"),
        (K::Uuid, "{00000000-0000-0000-0000-000000000000"),
        (K::Bool, "maybe"),
        (K::UnsignedInt, "4294967296"),
        (K::UnsignedInt, "08x"),
        (K::Network { cidr: false }, "10"),
        (K::Network { cidr: false }, "::1/08"),
        (K::Network { cidr: true }, "1.2.3.4/33"),
        (K::MacAddr { octets: 6 }, "08:00:2b:01:02:03x"),
        (K::MacAddr { octets: 6 }, "08:00:2b:01:02:100"),
        (K::MacAddr { octets: 8 }, "08:00:2b:01:02:03:04"),
        (K::Bytea, "\\x0"),
        (K::Bytea, "\\400"),
        (K::Enum { labels: Arc::from(["a".to_string()]), exact: true }, "b"),
        (K::Enum { labels: Arc::from(["a".to_string()]), exact: true }, " a"),
        (K::Enum { labels: Arc::from(["a".to_string()]), exact: true }, "A"),
        (K::Enum { labels: Arc::from(["a".to_string()]), exact: false }, &long_label),
    ];
    for (kind, text) in &refused {
        assert_eq!(field_key(kind, text).err(), Some(Unread::Refused), "{kind:?} {text:?}");
    }
    let unparsed: Vec<(CompareKind, &str)> = vec![
        (K::Int { bytes: 2 }, " 5"),
        (K::Int { bytes: 2 }, "0x10"),
        (K::Float64, " 1.5"),
        (K::Decimal { precision: 10, scale: 2 }, "1e5"),
        (K::Date, "20-01-01"),
        (K::Time, "12:-5:00"),
        (K::Timestamp { with_tz: true }, "2020-01-01 00:00:00+0530"),
        (K::Timestamp { with_tz: false }, "2020-01-01T00:00:00"),
        (K::Interval { qualifier: None }, "1 hour"),
        (K::TimeTz, "12:00:00+0530"),
        (bare.clone(), &past_dscale_by_exponent),
        (K::UnsignedInt, " 5"),
        (K::UnsignedInt, "0x1F"),
        (K::Bool, "yes"),
        (K::MacAddr { octets: 6 }, "08-00-2b-01-02-03"),
        (K::MacAddr { octets: 6 }, "08:00:2b:01:02:-1"),
        (K::MacAddr { octets: 8 }, "08002b0102030405"),
        (K::Bytea, "\\x ab"),
        (K::Bytea, "\\101"),
        (K::Enum { labels: Arc::from(["a".to_string()]), exact: false }, "b"),
        (K::Enum { labels: Arc::from(["a".to_string()]), exact: false }, &longest_label),
    ];
    for (kind, text) in &unparsed {
        assert_eq!(field_key(kind, text).err(), Some(Unread::Unparsed), "{kind:?} {text:?}");
    }
}

/// **A field no reader here reads is refused exactly where every major
/// refuses it** — an integer, a float, a `numeric` bare or held to a
/// typmod, a `jsonb` — and never where one reads it (I80, I81, I82, I41).
/// Each case was put to `int2in`, `int4in`, `int8in`, `float4in`,
/// `float8in`, `numeric_in` and `jsonb_in` in a `postgres:<major>-trixie`
/// container at each of 13.23, 14.24, 15.19, 16.15, 17.11 and 18.6 —
/// `numeric_in` handed the column's typmod, as `COPY` hands it, and a
/// negative scale only from 15 — and is marked read where any of them
/// read it. A case this build reads is one some major reads.
#[test]
fn a_field_no_reader_reads_is_refused_only_where_every_major_refuses_it() {
    use crate::pgtype::NumericTypmod;
    use CompareKind as K;
    let check = |kind: &CompareKind, text: &str, read: bool| {
        let key = field_key(kind, text);
        let shown = if text.len() > 40 { &text[..40] } else { text };
        if read {
            assert_ne!(key.err(), Some(Unread::Refused), "{kind:?} {shown:?} is read");
        } else {
            assert_eq!(key.err(), Some(Unread::Refused), "{kind:?} {shown:?} is refused");
        }
    };
    let integers: &[(&str, [bool; 3])] = &[
        ("abc", [false, false, false]),
        ("", [false, false, false]),
        (" ", [false, false, false]),
        ("-", [false, false, false]),
        ("+", [false, false, false]),
        ("1 2", [false, false, false]),
        (" 12 ", [true, true, true]),
        ("\t12\n", [true, true, true]),
        ("12abc", [false, false, false]),
        ("0x1F", [true, true, true]),
        ("0X1f", [true, true, true]),
        ("0o17", [true, true, true]),
        ("0O17", [true, true, true]),
        ("0b101", [true, true, true]),
        ("0B101", [true, true, true]),
        ("1_000", [true, true, true]),
        ("1__000", [false, false, false]),
        ("_1", [false, false, false]),
        ("1_", [false, false, false]),
        ("0x", [false, false, false]),
        ("0o", [false, false, false]),
        ("0b", [false, false, false]),
        ("0x_1F", [true, true, true]),
        ("0x1F_", [false, false, false]),
        ("0x1_F", [true, true, true]),
        ("0b2", [false, false, false]),
        ("0o8", [false, false, false]),
        ("+-1", [false, false, false]),
        ("--1", [false, false, false]),
        ("- 1", [false, false, false]),
        ("1.0", [false, false, false]),
        ("1e3", [false, false, false]),
        ("\u{661}\u{662}", [false, false, false]),
        ("12\u{a0}", [false, false, false]),
        ("\u{b}12", [true, true, true]),
        ("\u{c}12\r", [true, true, true]),
        ("0x80000000", [false, false, true]),
        ("0x7FFFFFFF", [false, true, true]),
        ("-0x80000000", [false, true, true]),
        ("-0x80000001", [false, false, true]),
        ("32768", [false, true, true]),
        ("99999999999999999999", [false, false, false]),
        ("1_000_000_000_000", [false, false, true]),
        (" -0x1_F ", [true, true, true]),
        ("0x1g", [false, false, false]),
        ("+0b1_1", [true, true, true]),
        ("0_1", [true, true, true]),
        ("00_1", [true, true, true]),
        ("1_0_0", [true, true, true]),
        ("0x__1", [false, false, false]),
        ("0x1__F", [false, false, false]),
        ("x1F", [false, false, false]),
    ];
    let narrow: &[(&str, u8, bool)] = &[
        ("0x8000", 2, false),
        ("-0x8000", 2, true),
        ("-0x8001", 2, false),
        ("1_000_000", 2, false),
        ("0x8000000000000000", 8, false),
        ("-0x8000000000000000", 8, true),
        ("-0x8000000000000001", 8, false),
        ("9_223_372_036_854_775_808", 8, false),
    ];
    let floats: &[(&str, [bool; 2])] = &[
        ("abc", [false, false]),
        ("", [false, false]),
        (" ", [false, false]),
        ("1e", [false, false]),
        ("1e+", [false, false]),
        ("1e-", [false, false]),
        ("1.5 ", [true, true]),
        (" 1.5", [true, true]),
        ("\t-1.5e3\n", [true, true]),
        ("0x1p3", [true, true]),
        ("0x1.8p1", [true, true]),
        ("0x", [false, false]),
        ("0x.8", [true, true]),
        ("0x.", [false, false]),
        ("0x1p", [false, false]),
        ("0x1p+", [false, false]),
        ("nan(123)", [true, true]),
        ("nan(", [false, false]),
        ("nan()", [true, true]),
        ("nan(abc_1)", [true, true]),
        ("nan(a-b)", [false, false]),
        ("infinit", [false, false]),
        ("infinity", [true, true]),
        ("INFINITY", [true, true]),
        ("+infinity", [true, true]),
        ("-inf", [true, true]),
        ("inf x", [false, false]),
        ("1_0", [false, false]),
        (".", [false, false]),
        ("-.", [false, false]),
        ("+.e1", [false, false]),
        (".e1", [false, false]),
        ("1e400", [false, false]),
        (" 1e400 ", [false, false]),
        ("0x1p1024", [false, false]),
        ("0x1p1023", [false, true]),
        ("0x1.fffffffffffffp1023", [false, true]),
        ("0x1.fffffffffffff8p1023", [false, false]),
        ("0x1.fffffffffffff7p1023", [false, true]),
        ("0x1p-1074", [false, true]),
        ("0x1p-1075", [false, false]),
        ("0x1.0000000000001p-1075", [false, true]),
        ("0x0p99999", [true, true]),
        ("0x1p-1076", [false, false]),
        (" nan ", [true, true]),
        ("1.5.5", [false, false]),
        ("1,5", [false, false]),
        ("e5", [false, false]),
        ("0x1.8", [true, true]),
        ("0X1P3", [true, true]),
        ("-0x1p3", [true, true]),
        ("0x1p99999999999999999999", [false, false]),
        ("0x1p-99999999999999999999", [false, false]),
        ("0x0.0p0", [true, true]),
        ("0xp1", [false, false]),
        ("1e5x", [false, false]),
        ("0x1p-1022", [false, true]),
        ("0x0.00000000000008p-1022", [false, false]),
        ("0x0.00000000000004p-1022", [false, false]),
        ("0x0.00000000000004000001p-1022", [false, false]),
        ("+-1", [false, false]),
        ("1 e5", [false, false]),
        ("infinityy", [false, false]),
        ("nanx", [false, false]),
        ("0x1pa", [false, false]),
        ("0x1.p1", [true, true]),
        ("0x.1p1", [true, true]),
        ("5.", [true, true]),
        ("1d5", [false, false]),
        ("1f", [false, false]),
    ];
    let reals: &[(&str, bool)] = &[
        ("0x1p128", false),
        ("0x1p127", true),
        ("0x1.fffffep127", true),
        ("0x1.ffffffp127", false),
        ("0x1.fffffefp127", true),
        ("0x1p-149", true),
        ("0x1p-150", false),
        ("0x1.000002p-150", true),
        ("0x1.0000000000001p-150", true),
        (" 3.403e38 ", false),
        (" 1e-46", false),
        ("0x1p-126", true),
    ];
    let bare: &[(&str, bool)] = &[
        ("abc", false),
        ("", false),
        (" ", false),
        (".", false),
        ("-", false),
        ("+", false),
        ("1e", false),
        ("1e+", false),
        ("1 e5", false),
        ("1e 5", true),
        ("1e\t5", true),
        ("1e -5", true),
        ("1e- 5", false),
        ("1e-5 ", true),
        ("1.2.3", false),
        ("1..2", false),
        (".5", true),
        ("5.", true),
        ("+5", true),
        (" 5 ", true),
        ("NaN", true),
        ("nan", true),
        (" NaN ", true),
        ("-NaN", false),
        ("+NaN", false),
        ("NaNx", false),
        ("Infinity", true),
        ("-Infinity", true),
        ("+Infinity", true),
        ("inf", true),
        ("-inf", true),
        ("+inf", true),
        ("infinity", true),
        ("infinit", false),
        ("INF", true),
        (" -inf ", true),
        ("- inf", false),
        ("0x1F", true),
        ("0x", false),
        ("0x_1F", true),
        ("0o17", true),
        ("0b101", true),
        ("0x1F_", false),
        ("0x1__F", false),
        ("-0x1F", true),
        ("1_000", true),
        ("1_000.000_1", true),
        ("1_.5", false),
        ("1._5", false),
        ("_1", false),
        ("._5", false),
        ("1e1_0", true),
        ("1e_10", false),
        ("1e10_", false),
        ("1e1073741823", false),
        ("1e-1073741823", false),
        ("0e1073741823", true),
        ("0e-1073741823", false),
        ("1e1073741824", false),
        ("1e-1073741824", false),
        ("0e1073741824", false),
        ("1e-16383", true),
        ("1e-16384", false),
        ("0.5e-16383", false),
        ("0e-16383", true),
        ("0e-16384", false),
        ("1e131071", true),
        ("1e131072", false),
        ("9.9e131071", true),
        ("0.1e131073", false),
        ("+ 5", false),
        ("1e+-5", false),
        ("1E5", true),
        ("\u{661}", false),
        ("5\u{a0}", false),
        ("1e99999999999999999999", false),
        ("1e-99999999999999999999", false),
        ("1.5e", false),
        ("1.e5", true),
        (".e5", false),
        ("0x1.5", false),
        ("0x1e5", true),
        ("0b", false),
        ("0o8", false),
        ("\u{661}\u{662}", false),
        ("1,5", false),
        ("0x80000000000000000000000000000000000", true),
    ];
    let ten_two: &[(&str, bool)] = &[
        ("Infinity", false),
        ("-inf", false),
        ("inf", false),
        ("NaN", true),
        (" nan ", true),
        ("1e5", true),
        ("1e7", true),
        ("1e8", false),
        ("0x1F", true),
        ("0x174876E800", false),
        ("0x5F5E0FF", true),
        ("0x5F5E100", false),
        ("1e-1073741823", true),
        ("1e-1073741824", false),
        ("1e-5", true),
        (" 1.5", true),
        ("1_0", true),
        ("9999999.995e1", true),
        ("9.9999999995e7", false),
        ("9.9999999994e7", true),
        ("abc", false),
        ("1e1073741823", false),
        ("0e1073741823", true),
        ("1e-16384", true),
        ("0.005e0", true),
        ("-0.005e0", true),
        ("0.0049e0", true),
        ("99999999.995e0", false),
        ("1e+7", true),
        ("+1e7", true),
        ("0b1111111111111111111111111111111111111111", false),
    ];
    let three_minus_two: &[(&str, bool)] = &[
        ("1e5", false),
        ("99949", true),
        ("99950", false),
        ("1e5 ", false),
        ("0x1869D", false),
        ("0x1869E", false),
        ("Infinity", false),
        ("NaN", true),
        ("abc", false),
        ("1.5e4", true),
    ];
    let jsonb: &[(&str, bool)] = &[
        ("abc", false),
        ("", false),
        (" ", false),
        ("{", false),
        ("[1,]", false),
        ("{\"a\":1}x", false),
        ("01", false),
        ("+1", false),
        (".5", false),
        ("NaN", false),
        ("\"\\u0000\"", false),
        ("[1 2]", false),
        ("{\"a\" 1}", false),
        ("\"\\x\"", false),
        ("tru", false),
        ("nul", false),
        ("1e1073741824", false),
        ("1e1073741823", false),
        ("0e1073741823", true),
        ("1e-16384", false),
        ("\"\\ud800\"", false),
        ("[1] [2]", false),
        ("1.", false),
        ("\"a\tb\"", false),
        ("{\"a\":1,}", false),
        ("-", false),
        ("-0", true),
        ("1E+2", true),
        ("\"\\u00\"", false),
        ("{1:2}", false),
        ("nulls", false),
        ("  true  ", true),
        ("\u{b}true", false),
    ];
    for (text, read) in integers {
        for (bytes, read) in [2, 4, 8].into_iter().zip(read) {
            check(&K::Int { bytes }, text, *read);
        }
    }
    for &(text, bytes, read) in narrow {
        check(&K::Int { bytes }, text, read);
    }
    for (text, [real, double]) in floats {
        check(&K::Float32, text, *real);
        check(&K::Float64, text, *double);
    }
    for &(text, read) in reals {
        check(&K::Float32, text, read);
    }
    let held = |precision, scale| {
        [
            K::Decimal { precision, scale: scale as i8 },
            K::Numeric { infinities: false, typmod: Some(NumericTypmod { precision, scale }) },
        ]
    };
    for (kinds, cases) in [
        (vec![K::Numeric { infinities: true, typmod: None }], bare),
        (held(10, 2).to_vec(), ten_two),
        (held(3, -2).to_vec(), three_minus_two),
        (vec![K::Jsonb], jsonb),
    ] {
        for kind in &kinds {
            for &(text, read) in cases {
                check(kind, text, read);
            }
        }
    }
    // `10^131072` and one less, the two sides of `NUMERIC_WEIGHT_MAX`,
    // which share a width in bits; and a document past `JSONB_MAX_DEPTH`.
    let bare = K::Numeric { infinities: true, typmod: None };
    check(&bare, &format!("0x{}", power_of_ten_in(131_072, 4, true)), true);
    check(&bare, &format!("0x{}", power_of_ten_in(131_072, 4, false)), false);
    check(&bare, &format!("0b{}", power_of_ten_in(131_072, 1, true)), true);
    check(&bare, &format!("0o{}", power_of_ten_in(131_072, 3, false)), false);
    let deep = |depth| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    check(&K::Jsonb, &deep(2000), true);
    check(&K::Jsonb, &deep(1001), true);
}

/// `10^n`, less one where `less_one` holds, in the radix of `bits` bits a
/// digit.
fn power_of_ten_in(n: u32, bits: usize, less_one: bool) -> String {
    // Little-endian 32-bit limbs, multiplied up nine decimal digits at a
    // time.
    let mut limbs: Vec<u64> = vec![1];
    let mut left = n;
    while left > 0 {
        let step = left.min(9);
        let multiplier = 10u64.pow(step);
        let mut carry = 0;
        for limb in &mut limbs {
            let value = *limb * multiplier + carry;
            *limb = value & 0xFFFF_FFFF;
            carry = value >> 32;
        }
        if carry > 0 {
            limbs.push(carry);
        }
        left -= step;
    }
    if less_one {
        // `10^n` ends in a zero bit for every `n` past zero, so the borrow
        // stops at the first set bit.
        for limb in &mut limbs {
            if *limb == 0 {
                *limb = 0xFFFF_FFFF;
            } else {
                *limb -= 1;
                break;
            }
        }
    }
    let binary: Vec<u8> = limbs
        .iter()
        .rev()
        .flat_map(|limb| (0..32).rev().map(move |i| (limb >> i & 1) as u8))
        .skip_while(|&bit| bit == 0)
        .collect();
    let pad = (bits - binary.len() % bits) % bits;
    let padded: Vec<u8> = std::iter::repeat_n(0, pad).chain(binary).collect();
    padded
        .chunks(bits)
        .map(|digit| {
            let value = digit.iter().fold(0u32, |n, &bit| n * 2 + u32::from(bit));
            char::from_digit(value, 16).expect("a digit")
        })
        .collect()
}

/// **A date, time, timestamp or `interval` field is refused exactly
/// where no supported major reads it** (I83, I84): text the readers do
/// not read is put to the grammar (`abc`, `KD90`'s case), and so is text
/// they refuse, split otherwise than `ParseDateTime` splits it — the time
/// zone `+99` in `12:34:56.789 t a/b-99:00` is part of a POSIX zone, and
/// `day` in `1 day  day` a unit with no count, which 13 to 16 read — and
/// an `interval` under its column's field qualifier. Each case's reading
/// by the servers is `datetime_in`'s tests'.
#[test]
fn a_date_or_time_field_is_refused_only_where_no_major_reads_it() {
    use CompareKind as K;
    use IntervalQualifier as Q;
    let tz = K::Timestamp { with_tz: true };
    let cases: &[(CompareKind, &str, bool)] = &[
        (K::Date, "abc", false),
        (K::Date, "2020-02-30", false),
        (K::Date, "2020-02-30x", false),
        (K::Date, "y2001m02d04", true),
        (K::Date, "2020-01-01 foo5", true),
        (K::Time, "12:00 m", true),
        (K::Time, "13:00 pm", false),
        (K::TimeTz, "12:34:56.789 t a/b-99:00", true),
        (K::TimeTz, "12:00:00+16", false),
        (K::Timestamp { with_tz: false }, "294277-01-01", false),
        (tz.clone(), "294277-01-01", true),
        (tz, "294277-01-01 +00", false),
        (K::Interval { qualifier: None }, "1 day  day", true),
        (K::IntervalFields { qualifier: None }, "1 year 1 mon 1 day ! year", true),
        (K::Interval { qualifier: None }, "1 day 1 day", false),
        (K::IntervalFields { qualifier: None }, "abc", false),
        // Under the column's qualifier: a bare number is a year in an
        // `interval year`, and `a:b` minutes and seconds in an `interval
        // minute to second`.
        (K::Interval { qualifier: None }, "3000000000", true),
        (K::Interval { qualifier: Some(Q::Year) }, "3000000000", false),
        (K::IntervalFields { qualifier: Some(Q::Year) }, "3000000000", false),
        (K::Interval { qualifier: Some(Q::Second) }, "3000000000", true),
        (K::Interval { qualifier: None }, "100:30", true),
        (K::Interval { qualifier: Some(Q::MinuteToSecond) }, "100:30", false),
        (K::IntervalFields { qualifier: Some(Q::MinuteToSecond) }, "100:30", false),
        (K::Interval { qualifier: Some(Q::MinuteToSecond) }, "59:30", true),
    ];
    for (kind, text, read) in cases {
        let refused = field_key(kind, text) == Err(Unread::Refused);
        assert_eq!(refused, !read, "{kind:?} {text:?}");
    }
}

/// **In DataFusion's semantics a float literal is not read on
/// PostgreSQL's terms** (`roadmap.md`, "A literal is guaranteed in
/// `*_out`'s form and never read past `*_in`'s"): one past the range is
/// the infinity the parse rounds it to.
#[test]
fn a_float_literal_past_its_range_is_not_refused_in_datafusion_semantics() {
    let p = order_predicate(PredicateOp::Ge, "1e400");
    let resolved = one_column("double precision", DataType::Float64);
    super::resolve_term(&p, 0, &resolved, 0, ComparisonSemantics::DataFusion).unwrap();
}

/// **An integer literal past its column's width is refused, as `int2in`,
/// `int4in` and `int8in` refuse it** (I60) — at either end, under `=`, an
/// ordering operator, inside an array literal and as a range's bound —
/// while a value at the edge of the width, and the server's `+` and
/// leading zeros, are read by value. Each refusal was cast on PostgreSQL
/// 16.
#[test]
fn an_integer_literal_past_its_width_is_refused_and_one_inside_it_read() {
    let refused = |declared: &str, data_type: DataType, literal: &str| {
        for op in [PredicateOp::Eq, PredicateOp::Ge] {
            let p = order_predicate(op, literal);
            let err = resolve_term(&p, 0, &one_column(declared, data_type.clone()), 0).unwrap_err();
            assert!(
                matches!(&err, Error::PredicateValueDecode { value, .. } if value == literal),
                "{declared} {op:?} {literal}: {err:?}"
            );
        }
    };
    for literal in ["32768", "-32769", "70000"] {
        refused("smallint", DataType::Int16, literal);
    }
    for literal in ["2147483648", "-2147483649", "3000000000"] {
        refused("integer", DataType::Int32, literal);
    }
    for literal in ["9223372036854775808", "-9223372036854775809"] {
        refused("bigint", DataType::Int64, literal);
    }

    let read = |declared: &str, data_type: DataType, literal: &str, field: &str| {
        ordered(declared, data_type, PredicateOp::Eq, literal, field).unwrap()
    };
    assert!(read("smallint", DataType::Int16, "-32768", "-32768"));
    assert!(read("smallint", DataType::Int16, "+0032767", "32767"));
    assert!(read("integer", DataType::Int32, "-2147483648", "-2147483648"));
    assert!(read("integer", DataType::Int32, "2147483647", "2147483647"));
    assert!(read("bigint", DataType::Int64, "-9223372036854775808", "-9223372036854775808"));
    assert!(read("bigint", DataType::Int64, "3000000000", "3000000000"));
    let p = order_predicate(PredicateOp::Lt, "70000");
    let message =
        resolve_term(&p, 0, &one_column("smallint", DataType::Int16), 0).unwrap_err().to_string();
    assert!(message.contains("from -32768 to 32767"), "{message}");

    let types = test_types();
    for (declared, field, literal) in [
        ("smallint[]", "{1}", "{1,70000}"),
        ("integer[]", "{1}", "{-2147483649}"),
        ("int4range", "[1,10)", "[1,3000000000)"),
        ("int4range", "[1,10)", "(-2147483649,1]"),
    ] {
        let verdict = nested_verdict(declared, &types, PredicateOp::Eq, field, literal);
        assert!(
            matches!(verdict, Err(Error::PredicateValueDecode { .. })),
            "{declared} {literal}: {verdict:?}"
        );
    }
    let verdict = nested_verdict("smallint[]", &types, PredicateOp::Eq, "{32767}", "{32767}");
    assert_eq!(verdict.unwrap(), Truth::True);

    let membership =
        Membership { column: "v".into(), values: vec![Some("1".into()), Some("70000".into())] };
    let err = resolve_membership(
        &membership,
        0,
        &one_column("smallint", DataType::Int16),
        0,
        ComparisonSemantics::Postgres,
    )
    .unwrap_err();
    assert!(matches!(&err, Error::PredicateValueDecode { op: "IN", .. }), "{err:?}");
}

/// **In DataFusion's semantics an integer literal is not read at the
/// column's width** (`roadmap.md`, "A literal is guaranteed in `*_out`'s
/// form and never read past `*_in`'s"): DataFusion widens the column to
/// the literal's type, so `70000` against a `smallint` matches nothing
/// rather than refusing.
#[test]
fn an_integer_literal_past_its_width_is_not_refused_in_datafusion_semantics() {
    let resolved = one_column("smallint", DataType::Int16);
    for op in [PredicateOp::Eq, PredicateOp::Ge] {
        let p = order_predicate(op, "70000");
        let term =
            super::resolve_term(&p, 0, &resolved, 0, ComparisonSemantics::DataFusion).unwrap();
        let truth = term
            .eval(RawRow::unchecked(b"32767"), &mut RowSplit::default(), "public.t", 0)
            .unwrap();
        assert_eq!(truth, Truth::False, "{op:?}");
    }
}

/// **A date, time or timestamp literal or field PostgreSQL refuses is
/// refused** (I61) — a day the calendar lacks, a time past
/// `time_overflows`, a zone past `±15:59:59`, an instant past the range —
/// under `=` and an ordering operator, inside an array and as a range's
/// bound, and a field so spelled raises `FieldDecode` where it is read
/// rather than being read as a neighbouring value. `12:-5:00`, which the server reads as
/// 12:00 at zone `-5`, is refused rather than read as 11:55. Each was
/// cast on PostgreSQL 16.
#[test]
fn a_date_or_time_postgresql_refuses_is_refused_as_literal_and_field() {
    let timestamptz = DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()));
    let cases = [
        ("date", DataType::Date32, "2020-02-30", "2020-03-01"),
        ("date", DataType::Date32, "0000-01-01", "0001-01-01"),
        ("date", DataType::Date32, "5874898-01-01", "5874897-12-31"),
        ("time", DataType::Time64(TimeUnit::Microsecond), "12:60:00", "13:00:00"),
        ("time", DataType::Time64(TimeUnit::Microsecond), "12:-5:00", "11:55:00"),
        ("time with time zone", DataType::Utf8View, "12:00:00+16", "12:00:00+00"),
        ("time with time zone", DataType::Utf8View, "24:00:01+00", "24:00:00+00"),
        (
            "timestamp",
            DataType::Timestamp(TimeUnit::Microsecond, None),
            "294277-01-01 00:00:00",
            "2020-01-01 00:00:00",
        ),
        (
            "timestamp with time zone",
            timestamptz,
            "2020-01-01 00:00:00+16",
            "2020-01-01 00:00:00+00",
        ),
    ];
    for (declared, data_type, refused, field) in &cases {
        for op in [PredicateOp::Eq, PredicateOp::Ge] {
            let p = order_predicate(op, refused);
            let err = resolve_term(&p, 0, &one_column(declared, data_type.clone()), 0).unwrap_err();
            assert!(
                matches!(&err, Error::PredicateValueDecode { value, .. } if value == refused),
                "{declared} {op:?} {refused}: {err:?}"
            );
        }
        // `=` compares a field's spelling with the literal's canonical one
        // and decodes no field, so an ordering is what reads one.
        let got = ordered(declared, data_type.clone(), PredicateOp::Ge, field, refused);
        assert!(matches!(got, Err(Error::FieldDecode { .. })), "{declared} {refused}: {got:?}");
    }

    let types = test_types();
    for (declared, field, literal) in [
        ("date[]", "{2020-01-01}", "{2020-02-30}"),
        ("daterange", "[2020-01-01,2020-01-02)", "[2020-01-01,2020-13-01)"),
        ("tsrange", "empty", "[2020-01-01 00:00:00,2020-01-01 25:00:00)"),
    ] {
        let verdict = nested_verdict(declared, &types, PredicateOp::Eq, field, literal);
        assert!(
            matches!(verdict, Err(Error::PredicateValueDecode { .. })),
            "{declared} {literal}: {verdict:?}"
        );
    }
}

/// **An interval literal or field `interval_in` refuses is refused**
/// (I62) — a minute past 59, a count past `int32`, a time past `int64`
/// microseconds, a unit given twice — under `=` and an ordering operator,
/// and inside an array, and a field so spelled raises `FieldDecode` where
/// it is read: `00:90:00` once read as `01:30:00`, and `3000000000 days`
/// was ordered. Each was cast on PostgreSQL 16.
#[test]
fn an_interval_postgresql_refuses_is_refused_as_literal_and_field() {
    let interval = || DataType::Interval(IntervalUnit::MonthDayNano);
    for (refused, field) in [
        ("00:90:00", "01:30:00"),
        ("3000000000 days", "1 day"),
        ("178956970 years 8 mons", "1 mon"),
        ("2562047788:00:54.775808", "01:00:00"),
        ("1 day 1 day", "2 days"),
    ] {
        for op in [PredicateOp::Eq, PredicateOp::Ge] {
            let p = order_predicate(op, refused);
            let err = resolve_term(&p, 0, &one_column("interval", interval()), 0).unwrap_err();
            assert!(
                matches!(&err, Error::PredicateValueDecode { value, .. } if value == refused),
                "{op:?} {refused}: {err:?}"
            );
        }
        let got = ordered("interval", interval(), PredicateOp::Ge, field, refused);
        assert!(matches!(got, Err(Error::FieldDecode { .. })), "{refused}: {got:?}");
    }
    let verdict =
        nested_verdict("interval[]", &test_types(), PredicateOp::Eq, "{01:30:00}", "{00:90:00}");
    assert!(matches!(verdict, Err(Error::PredicateValueDecode { .. })), "{verdict:?}");
}

/// **A `uuid` whose hyphen `uuid_in` refuses is refused as a literal and
/// as a field** (I64): under `=` and an ordering operator, and inside an
/// array; one in braces is read. Each was cast on PostgreSQL 16, and each
/// refused here once read, a hyphen having been dropped wherever it fell.
#[test]
fn a_uuid_postgresql_refuses_is_refused_as_literal_and_field() {
    let uuid = || DataType::FixedSizeBinary(16);
    let canonical = "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11";
    for refused in [
        "-a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11",
        "a0eebc99--9c0b-4ef8-bb6d-6bb9bd380a11",
        "a0eebc9-99c0b-4ef8-bb6d-6bb9bd380a11",
        "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11-",
    ] {
        for op in [PredicateOp::Eq, PredicateOp::Ge] {
            let p = order_predicate(op, refused);
            let err = resolve_term(&p, 0, &one_column("uuid", uuid()), 0).unwrap_err();
            assert!(
                matches!(&err, Error::PredicateValueDecode { value, .. } if value == refused),
                "{op:?} {refused}: {err:?}"
            );
        }
        let got = ordered("uuid", uuid(), PredicateOp::Ge, canonical, refused);
        assert!(matches!(got, Err(Error::FieldDecode { .. })), "{refused}: {got:?}");
    }
    let braced = "{a0eebc999c0b4ef8bb6d6bb9bd380a11}";
    assert!(ordered("uuid", uuid(), PredicateOp::Eq, braced, canonical).unwrap());
    assert!(ordered("uuid", uuid(), PredicateOp::Ge, canonical, braced).unwrap());
    let verdict = nested_verdict(
        "uuid[]",
        &test_types(),
        PredicateOp::Eq,
        &format!("{{{canonical}}}"),
        "{a0eebc99--9c0b-4ef8-bb6d-6bb9bd380a11}",
    );
    assert!(matches!(verdict, Err(Error::PredicateValueDecode { .. })), "{verdict:?}");
}

/// **A number past `numeric_in`'s storage bounds is refused as a literal
/// on every `numeric` kind, and as a field where the server reads it with
/// no typmod** (I63): a display scale past 16383 or more than 131072
/// integer digits, under `=` and an ordering operator and inside an
/// array. A `numeric(p,s)` field so spelled is read, the server rounding
/// it to the scale first, and each edge inside the bounds is read. Each
/// was cast on PostgreSQL 16; `1.` and 16384 zeros once matched `1`.
#[test]
fn a_numeric_past_numeric_ins_bounds_is_refused_as_literal_and_bare_field() {
    let fraction = |zeros: usize| format!("1.{}", "0".repeat(zeros));
    let integer = |digits: usize| format!("1{}", "0".repeat(digits - 1));
    let kinds = [
        ("numeric", DataType::Utf8View),
        ("numeric(10,2)", DataType::Decimal128(10, 2)),
        ("numeric(100,2)", DataType::Utf8View),
    ];
    for refused in [fraction(16384), format!("-{}", fraction(16384)), integer(131073)] {
        for (declared, data_type) in &kinds {
            for op in [PredicateOp::Eq, PredicateOp::Ge] {
                let p = order_predicate(op, &refused);
                let err =
                    resolve_term(&p, 0, &one_column(declared, data_type.clone()), 0).unwrap_err();
                assert!(
                    matches!(&err, Error::PredicateValueDecode { .. }),
                    "{declared} {op:?} {}: {err:?}",
                    refused.len()
                );
            }
        }
        let got = ordered("numeric", DataType::Utf8View, PredicateOp::Eq, "1", &refused);
        assert!(matches!(got, Err(Error::FieldDecode { .. })), "{}: {got:?}", refused.len());
    }
    let past_scale = fraction(16384);
    for (declared, data_type) in &kinds[1..] {
        let got = ordered(declared, data_type.clone(), PredicateOp::Ge, "1", &past_scale);
        assert!(got.unwrap(), "{declared}");
    }
    for read in [fraction(16383), integer(131072), format!("{}1", "0".repeat(131073))] {
        for (op, literal) in [(PredicateOp::Eq, read.as_str()), (PredicateOp::Ge, "1")] {
            let got = ordered("numeric", DataType::Utf8View, op, literal, &read);
            assert!(got.unwrap(), "{op:?} {}", read.len());
        }
    }
    let verdict = nested_verdict(
        "numeric[]",
        &test_types(),
        PredicateOp::Eq,
        "{1}",
        &format!("{{{}}}", fraction(16384)),
    );
    assert!(matches!(verdict, Err(Error::PredicateValueDecode { .. })), "{verdict:?}");
}

/// **A `numeric(p,s)` field is put through its typmod and a literal is
/// not** (I51, I63). A field finer than the scale is rounded half away
/// from zero, `1.005` keying as `1.01`, and one past the precision once
/// rounded is refused, on the typed arm and on one held as text for a
/// precision past 76 alike, inside an array too. A literal is read
/// exactly: `1.005` is refused on the typed arm as finer than its scale
/// and keys as itself on the text arm, and one past the precision
/// compares. Each field was cast on PostgreSQL 16.
#[test]
fn a_numeric_field_is_rounded_and_bounded_by_its_typmod_and_a_literal_is_not() {
    let typed = ("numeric(10,2)", DataType::Decimal128(10, 2));
    let text = ("numeric(100,2)", DataType::Utf8View);
    let past_text = format!("{}.995", "9".repeat(98));
    for ((declared, data_type), past) in
        [(&typed, "99999999.995"), (&typed, "123456789012"), (&text, &past_text)]
    {
        let read = |op, literal: &str, field: &str| {
            ordered(declared, data_type.clone(), op, literal, field)
        };
        assert!(read(PredicateOp::Ge, "1.01", "1.005").unwrap(), "{declared}");
        assert!(!read(PredicateOp::Lt, "1.01", "1.005").unwrap(), "{declared}");
        assert!(read(PredicateOp::Le, "-1.01", "-1.005").unwrap(), "{declared}");
        assert!(read(PredicateOp::Ge, "0", "-0.004").unwrap(), "{declared}");
        assert!(read(PredicateOp::Lt, "123456789012", "1.00").unwrap(), "{declared}");
        let got = read(PredicateOp::Ge, "1", past);
        assert!(matches!(got, Err(Error::FieldDecode { .. })), "{declared} {past}: {got:?}");
    }
    let got = ordered(typed.0, typed.1.clone(), PredicateOp::Gt, "1.005", "1.01");
    assert!(matches!(got, Err(Error::PredicateValueDecode { .. })), "{got:?}");
    assert!(ordered(text.0, text.1.clone(), PredicateOp::Gt, "1.005", "1.005").unwrap());
    assert!(ordered(text.0, text.1.clone(), PredicateOp::Eq, "1.01", "1.005").unwrap());
    let wide = format!("1{}", "0".repeat(120));
    assert!(ordered(text.0, text.1.clone(), PredicateOp::Lt, &wide, "1.00").unwrap());

    let array = |op, field: &str, literal: &str| {
        nested_verdict("numeric(10,2)[]", &test_types(), op, field, literal)
    };
    assert_eq!(array(PredicateOp::Eq, "{1.005,2}", "{1.01,2.00}").unwrap(), Truth::True);
    let got = array(PredicateOp::Eq, "{99999999.995}", "{1}");
    assert!(matches!(got, Err(Error::FieldDecode { .. })), "{got:?}");
}

/// **A `jsonb` number is bounded as `numeric_in` bounds it** (I63), its
/// exponent applied first: an exponent past `PG_INT32_MAX / 2`, a display
/// scale past 16383 — the fraction's digits less the exponent — or more
/// than 131072 integer digits is refused as a literal and as a field, and
/// each edge inside is read, a zero whatever its exponent, and `1e100000`
/// among them. Each was cast on PostgreSQL 16.
#[test]
fn a_jsonb_number_is_bounded_as_numeric_in_bounds_it() {
    let jsonb =
        |op, literal: &str, field: &str| ordered("jsonb", DataType::Utf8View, op, literal, field);
    for refused in ["1e131072", "10e131071", "0.001e131075", "1.5e-16383", "1.50e-16382"]
        .into_iter()
        .chain(["0e1073741824", "0e-16384", "[1e131072]"])
    {
        let err = jsonb(PredicateOp::Ge, refused, "1").unwrap_err();
        assert!(matches!(err, Error::PredicateValueDecode { .. }), "{refused}: {err:?}");
        let got = jsonb(PredicateOp::Eq, "1", refused);
        assert!(matches!(got, Err(Error::FieldDecode { .. })), "{refused}: {got:?}");
    }
    let largest = format!("1{}", "0".repeat(131071));
    for (read, field) in [
        ("1e131071", largest.as_str()),
        ("10e131070", largest.as_str()),
        ("0.001e131074", largest.as_str()),
        ("1e100000", &format!("1{}", "0".repeat(100000))),
        ("1.5e-16382", &format!("0.{}15", "0".repeat(16381))),
        ("0e1073741823", "0"),
        ("-0e-16383", "0"),
    ] {
        assert!(jsonb(PredicateOp::Eq, read, field).unwrap(), "{read}");
        assert!(jsonb(PredicateOp::Eq, field, read).unwrap(), "{read}");
    }
}

/// A NULL field is excluded by every ordering operator, as under
/// `Eq`/`Ne`.
#[test]
fn a_null_field_matches_no_ordering_operator() {
    for op in [PredicateOp::Lt, PredicateOp::Le, PredicateOp::Gt, PredicateOp::Ge] {
        assert!(!ordered("integer", DataType::Int32, op, "0", "\\N").unwrap());
    }
}

/// The three special values are ordered exactly, on whichever side they
/// appear: `-infinity` below every finite value, `infinity` above it, and
/// each equal to itself (I34).
#[test]
fn date_and_timestamp_infinities_are_ordered() {
    let date = |op, value, field| ordered("date", DataType::Date32, op, value, field).unwrap();
    assert!(date(PredicateOp::Gt, "2020-01-01", "infinity"));
    assert!(!date(PredicateOp::Lt, "2020-01-01", "infinity"));
    assert!(date(PredicateOp::Lt, "0044-01-01 BC", "-infinity"));
    assert!(date(PredicateOp::Lt, "infinity", "-infinity"));
    // Each special equals itself, so the boundary pair splits on it.
    assert!(date(PredicateOp::Ge, "infinity", "infinity"));
    assert!(!date(PredicateOp::Gt, "infinity", "infinity"));
    assert!(date(PredicateOp::Le, "-infinity", "-infinity"));

    let ts = |op, value, field| {
        ordered(
            "timestamp without time zone",
            DataType::Timestamp(TimeUnit::Microsecond, None),
            op,
            value,
            field,
        )
        .unwrap()
    };
    assert!(ts(PredicateOp::Gt, "2020-01-01 00:00:00", "infinity"));
    assert!(ts(PredicateOp::Lt, "2020-01-01 00:00:00", "-infinity"));
    let tstz = ordered(
        "timestamp with time zone",
        DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
        PredicateOp::Gt,
        "2020-01-01 00:00:00+00",
        "infinity",
    );
    assert!(tstz.unwrap());
}

/// A `numeric(p,s)` column can hold `NaN` — the typmod does not apply to
/// it — and PostgreSQL orders it above every other value, itself included
/// (I34). An *infinity* cannot reach this kind at all: any typmod rejects
/// one, and a `numeric` without a typmod is held as text.
#[test]
fn a_decimal_nan_is_the_largest_value_and_equals_itself() {
    let t = DataType::Decimal128(10, 2);
    let nan = |op, value, field| ordered("numeric(10,2)", t.clone(), op, value, field).unwrap();
    assert!(nan(PredicateOp::Gt, "999999.99", "NaN"));
    assert!(nan(PredicateOp::Gt, "-1.50", "NaN"));
    assert!(nan(PredicateOp::Ge, "NaN", "NaN"));
    assert!(!nan(PredicateOp::Gt, "NaN", "NaN"));
    assert!(nan(PredicateOp::Lt, "NaN", "0.00"));
}

/// The spelling is the one that type's own `*_out` writes and nothing
/// else, so a `date` reading `Infinity` is undecodable on either side. A
/// text column holding the word is unaffected.
#[test]
fn only_the_types_own_spelling_is_special() {
    let err =
        ordered("date", DataType::Date32, PredicateOp::Gt, "Infinity", "2020-01-01").unwrap_err();
    assert!(matches!(err, Error::PredicateValueDecode { .. }), "{err:?}");
    let err =
        ordered("date", DataType::Date32, PredicateOp::Gt, "2020-01-01", "Infinity").unwrap_err();
    assert!(matches!(err, Error::FieldDecode { .. }), "{err:?}");

    // A `text` column holding `infinity` holds the *word*: it compares
    // bytewise, below `zzz`, where a special value would sort above it.
    let text = |op, field| ordered("text", DataType::Utf8View, op, "zzz", field).unwrap();
    assert!(text(PredicateOp::Lt, "infinity"));
    assert!(!text(PredicateOp::Gt, "infinity"));
}

/// A field that is not a value of its mapped type is the same fault the
/// typed build path reports, with the same wording and the same escape —
/// the population the special values are separated *from*.
#[test]
fn an_undecodable_field_is_a_field_decode_error() {
    let err = ordered("integer", DataType::Int32, PredicateOp::Gt, "0", "twelve").unwrap_err();
    assert!(
        matches!(err, Error::FieldDecode { ref column, ref value, .. }
                if column == "v" && value == "twelve"),
        "{err:?}"
    );
}

/// **A value is answered with no row around it, and the answer is the
/// row's** — every [`Comparison`] arm and both NULL-counting families,
/// over a NULL, values either side of the literal, and a value that is
/// not of the column's type. The one thing a value cannot carry is where
/// it came from, so the row path's `FieldDecode` is the value path's
/// `None`, and nothing else is.
#[test]
fn a_value_outside_any_row_is_answered_as_the_row_holding_it_is() {
    use crate::copy::encode_field;
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
                &Predicate { column: "v".into(), op: PredicateOp::IsNull, value: None },
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
            let row = encode_field(value);
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

/// A literal that is not a value of the column's type is refused when the
/// block's schema resolves, once rather than per row.
#[test]
fn an_undecodable_literal_is_refused_at_resolution() {
    let p = order_predicate(PredicateOp::Gt, "twelve");
    let err = resolve_term(&p, 0, &one_column("integer", DataType::Int32), 0).unwrap_err();
    assert!(
        matches!(err, Error::PredicateValueDecode { ref column, .. } if column == "v"),
        "{err:?}"
    );
}

/// Refusal is by resolution and plan, and the four reasons are distinct
/// facts about the column.
#[test]
fn an_ordering_operator_is_refused_off_a_mapped_scalar_column() {
    let p = order_predicate(PredicateOp::Gt, "1");

    let mut unmapped = one_column("mystery", DataType::Utf8View);
    unmapped.columns[0] = ColumnResolution::UnknownType;
    assert!(matches!(
        resolve_term(&p, 0, &unmapped, 7).unwrap_err(),
        Error::UnorderedPredicateColumn { reason, header_offset: 7, .. } if reason == NOT_MAPPED
    ));

    // A nested column the register compares not at all — a range whose
    // DDL stated no subtype, so there is no bound type to compare by.
    let mut nested = one_column("public.opaquerange", DataType::Utf8View);
    nested.plans[0] = NestedPlan::Range(Box::new(NestedPlan::Scalar));
    assert!(matches!(
        resolve_term(&p, 0, &nested, 0).unwrap_err(),
        Error::UnorderedPredicateColumn { reason, .. } if reason == NESTED
    ));

    // A nested column whose *shape* compares here and one of whose
    // positions does not: refused too, and the sentence names the
    // position and the type rather than the nesting.
    let mut inherited = one_column("json[]", DataType::Utf8View);
    inherited.plans[0] = NestedPlan::array(NestedPlan::Scalar);
    let err = resolve_term(&p, 0, &inherited, 0).unwrap_err();
    let Error::UnorderedPredicateColumn { reason, .. } = &err else { panic!("{err:?}") };
    assert!(reason.contains("`[]` inside it is `json`"), "{reason}");

    // A `Mapped` scalar column whose *declared* type the register
    // refuses: unreachable from the mapping table, everything it maps to
    // a scalar having a comparison in the same arm, and refused anyway.
    assert!(matches!(
        resolve_term(&p, 0, &one_column("mystery", DataType::UInt8), 0).unwrap_err(),
        Error::UnorderedPredicateColumn { reason, .. } if reason == NO_ORDER
    ));
}

/// **The ordering refusal says the server refuses too only where it
/// does.** A `json` position takes the server's order away (I45), so its
/// sentence says so; a position this build merely models no order for —
/// `money`, `bit`, `tsvector`, a user base type — makes no claim about
/// PostgreSQL, which orders `money[]`, `bit[]` and `tsvector[]` through
/// their btree opclasses. The whole message is pinned: the manual prints
/// both for a user to recognise.
#[test]
fn a_nested_ordering_refusal_claims_the_server_refuses_only_for_json() {
    let types = vec![
        TypeDef { name: "public.gtype".into(), kind: TypeKind::base() },
        TypeDef {
            name: "public.jsonpair".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("ok", "integer"), ColumnDef::new("doc", "json")]),
            },
        },
        TypeDef {
            name: "public.tagged".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("ok", "integer"),
                    ColumnDef::new("tag", "public.gtype"),
                ]),
            },
        },
    ];
    let message = |declared: &str| {
        let p = order_predicate(PredicateOp::Lt, "{}");
        resolve_term(&p, 0, &nested_column(declared, &types), 0).unwrap_err().to_string()
    };
    let server = |path: &str, at: &str| {
        format!(
            "`<` on column `v` in the COPY block at offset 0: the column is nested and \
                 `{path}` inside it is `{at}`, which has no order here — PostgreSQL refuses the \
                 same comparison, since a container is ordered by its element type's own \
                 comparison and this type has none; use `=` or `!=` for a text comparison"
        )
    };
    let unmodelled = |path: &str, at: &str| {
        format!(
            "`<` on column `v` in the COPY block at offset 0: the column is nested and \
                 `{path}` inside it is `{at}`, for which this build models no order — a \
                 container is ordered by its element type's own comparison, so the column has \
                 none here either; use `=` or `!=` for a text comparison"
        )
    };
    for (declared, path) in [("json[]", "[]"), ("public.jsonpair", ".doc")] {
        assert_eq!(message(declared), server(path, "json"), "{declared}");
    }
    for (declared, path, at) in [
        ("money[]", "[]", "money"),
        ("bit(3)[]", "[]", "bit(3)"),
        ("tsvector[]", "[]", "tsvector"),
        ("public.tagged", ".tag", "public.gtype"),
    ] {
        let found = message(declared);
        assert_eq!(found, unmodelled(path, at), "{declared}");
        assert!(!found.contains("PostgreSQL"), "{found}");
    }
}

/// Under DataFusion's semantics every comparing operator on a nested column is
/// refused — `=` too, which PostgreSQL's answers structurally or as text
/// — and a column with no plan still answers `=` bytewise, which is
/// Arrow's `=` over the `Utf8View` it emits, announcing nothing. Its
/// ordering is `datafusion_semantics_orders_a_column_with_no_plan_bytewise`'s.
#[test]
fn datafusion_semantics_refuses_a_nested_column_under_every_comparing_operator() {
    let arrow = |p: &Predicate, resolved: &ResolvedSchema| {
        super::resolve_term(p, 0, resolved, 0, ComparisonSemantics::DataFusion)
    };
    let mut opaque = one_column("public.opaquerange", DataType::Utf8View);
    opaque.plans[0] = NestedPlan::Range(Box::new(NestedPlan::Scalar));
    let array = nested_column("integer[]", &[]);
    let mut inherited = one_column("json[]", DataType::Utf8View);
    inherited.plans[0] = NestedPlan::array(NestedPlan::Scalar);
    for resolved in [&opaque, &array, &inherited] {
        for op in [PredicateOp::Eq, PredicateOp::IsDistinctFrom, PredicateOp::Lt] {
            let err = arrow(&order_predicate(op, "{}"), resolved).unwrap_err();
            assert!(
                matches!(&err, Error::UncomparablePredicateColumn { reason, .. }
                        if reason == NESTED_IN_DATAFUSION),
                "{err:?}"
            );
        }
        assert!(arrow(&order_predicate(PredicateOp::IsNull, "{}"), resolved).is_ok());
    }
    let mut unknown = one_column("box", DataType::Utf8View);
    unknown.columns[0] = ColumnResolution::UnknownType;
    let term = arrow(&order_predicate(PredicateOp::Eq, "(1,1),(0,0)"), &unknown).unwrap();
    assert!(term.comparison_notes().is_empty());
    assert_eq!(term.eval_value(Some("(1,1),(0,0)")), Some(Truth::True));
}

/// **A column's divergences depend on the semantics asked for**, and
/// Arrow's are read off what the column emits rather than off which terms
/// this build answers: a nested column refused every Arrow term still
/// reports DataFusion's order of it, and each position inside it its own
/// divergence under its path — `numeric[]` its element's text, `text[]`
/// its element's collation, `real[]` its element's unnormalized zero; a
/// float column reports nothing in either.
/// A column that fell back to text reports nothing in DataFusion's semantics,
/// its column note being the finding. One schema, so the order of the
/// report is the columns'.
#[test]
fn a_column_s_divergences_follow_the_semantics_asked_for() {
    use ComparisonDivergence as D;
    let columns = [
        one_column("real", DataType::Float32),
        one_column("text", DataType::Utf8View),
        one_column("interval", DataType::Interval(IntervalUnit::MonthDayNano)),
        nested_column("integer[]", &[]),
        {
            let mut undeclared = one_column("text", DataType::Utf8View);
            undeclared.columns[0] = ColumnResolution::NotDeclared;
            undeclared.notes[0].declared = None;
            undeclared.comparisons[0] = ComparisonPlan::Refused;
            undeclared
        },
        {
            let mut unknown = one_column("box", DataType::Utf8View);
            unknown.columns[0] = ColumnResolution::UnknownType;
            unknown.comparisons[0] = ComparisonPlan::Refused;
            unknown
        },
        nested_column("numeric[]", &[]),
        nested_column("text[]", &[]),
        nested_column("real[]", &[]),
    ];
    let mut resolved = ResolvedSchema::default();
    for (i, c) in columns.into_iter().enumerate() {
        resolved.columns.extend(c.columns);
        resolved
            .notes
            .extend(c.notes.into_iter().map(|n| ColumnNote { column: format!("c{i}"), ..n }));
        resolved.plans.extend(c.plans);
        resolved.comparisons.extend(c.comparisons);
    }
    let report = |semantics| {
        column_divergences(&resolved, semantics)
            .into_iter()
            .map(|n| (n.column, n.path, n.divergence))
            .collect::<Vec<_>>()
    };
    let c = |i: usize| format!("c{i}");
    let element = || Some("[]".to_string());
    assert_eq!(
        report(ComparisonSemantics::Postgres),
        [
            (c(1), None, D::UnknownCollation),
            (c(5), None, D::UnmodelledType),
            (c(7), element(), D::UnknownCollation),
        ]
    );
    assert_eq!(
        report(ComparisonSemantics::DataFusion),
        [
            (c(1), None, D::UnknownCollation),
            (c(2), None, D::IntervalFields),
            (c(3), None, D::NestedOrder),
            (c(6), None, D::NestedOrder),
            (c(6), element(), D::ValueAsText),
            (c(7), None, D::NestedOrder),
            (c(7), element(), D::UnknownCollation),
            (c(8), None, D::NestedOrder),
            (c(8), element(), D::UnnormalizedZero),
        ]
    );
    let notes = column_divergences(&resolved, ComparisonSemantics::DataFusion);
    let order = notes.iter().find(|n| n.column == c(3)).unwrap();
    assert!(!order.divergence.affects_equality());
    assert!(
        order.message().starts_with("`c3` (integer[]) is ordered as DataFusion orders"),
        "{}",
        order.message()
    );
    let zero = notes.iter().find(|n| n.divergence == D::UnnormalizedZero).unwrap();
    assert!(zero.divergence.affects_equality());
    assert!(zero.message().starts_with("`c8[]` (real) is compared"), "{}", zero.message());
}

/// **A text-emitted kind's equality warning is about a literal's
/// spelling**: DataFusion compares a literal bytewise with the emitted
/// text, so a `timetz` literal the server reads as the value it wrote
/// matches nothing in DataFusion's semantics where PostgreSQL's matches it.
/// The column says so, and names such a spelling.
#[test]
fn a_literal_not_spelled_as_the_server_writes_misses_in_datafusion_semantics() {
    let resolved = one_column("time with time zone", DataType::Utf8View);
    let arrow = |literal| {
        let p = order_predicate(PredicateOp::Eq, literal);
        super::resolve_term(&p, 0, &resolved, 0, ComparisonSemantics::DataFusion).unwrap()
    };
    assert_eq!(arrow("12:00+00").eval_value(Some("12:00:00+00")), Some(Truth::False));
    // One this build's PostgreSQL semantics also reads, answering as the
    // server does.
    let p = order_predicate(PredicateOp::Eq, "12:00:00.0+00");
    let postgres = resolve_term(&p, 0, &resolved, 0).unwrap();
    assert_eq!(postgres.eval_value(Some("12:00:00+00")), Some(Truth::True));
    assert_eq!(arrow("12:00:00.0+00").eval_value(Some("12:00:00+00")), Some(Truth::False));
    let [note] = &column_divergences(&resolved, ComparisonSemantics::DataFusion)[..] else {
        panic!("one note")
    };
    assert_eq!(note.divergence, ComparisonDivergence::ValueAsText);
    assert!(note.divergence.affects_equality());
    assert!(note.message().contains("`'12:00+00'` misses `12:00:00+00`"), "{}", note.message());

    // `macaddr`'s text orders as its octets do, and an uppercase literal,
    // which PostgreSQL reads as the same octets, still misses.
    let resolved = one_column("macaddr", DataType::Utf8View);
    let p = order_predicate(PredicateOp::Eq, "08:00:2B:01:02:03");
    let postgres = resolve_term(&p, 0, &resolved, 0).unwrap();
    assert_eq!(postgres.eval_value(Some("08:00:2b:01:02:03")), Some(Truth::True));
    let arrow = super::resolve_term(&p, 0, &resolved, 0, ComparisonSemantics::DataFusion).unwrap();
    assert_eq!(arrow.eval_value(Some("08:00:2b:01:02:03")), Some(Truth::False));
    let [note] = &column_divergences(&resolved, ComparisonSemantics::DataFusion)[..] else {
        panic!("one note")
    };
    assert_eq!(note.divergence, ComparisonDivergence::ValueAsText);
}

/// **A float nested in a list is compared with no `-0` made `0`**, as
/// `make_comparator` compares it and as [`ComparisonDivergence::UnnormalizedZero`]
/// says — where a float column's `-0` equals `0` in DataFusion's semantics
/// (DataFusion's `apply_cmp`). Read off the arrays a batch builds.
#[test]
fn a_nested_float_s_negative_zero_is_not_zero_to_datafusion() {
    let resolved = nested_column("real[]", &[]);
    let array = crate::batch::column_of(
        resolved.schema.field(0).data_type(),
        &resolved.plans[0],
        &["{-0}", "{0}"],
    )
    .unwrap();
    let cmp = arrow::array::make_comparator(&array, &array, Default::default()).unwrap();
    assert_eq!(cmp(0, 1), Ordering::Less);
}

/// **A column with no plan is ordered bytewise in DataFusion's semantics**,
/// under every ordering operator, where PostgreSQL's refuses them: the
/// column emits the file's text as `Utf8View`, which DataFusion orders by
/// its bytes. Both roads there — a type this build does not map, and a
/// column no DDL declared, which is every column under
/// `--schema-mode strings` — answer alike and announce nothing. Both read
/// bounds by `Text`; which stored set that names is the block's to say,
/// gathering having resolved the column under whatever type its DDL gives
/// it (`crate::pgtype::bounds_set_keyed_by`).
#[test]
fn datafusion_semantics_orders_a_column_with_no_plan_bytewise() {
    let mut unknown = one_column("box", DataType::Utf8View);
    unknown.columns[0] = ColumnResolution::UnknownType;
    let mut undeclared = one_column("text", DataType::Utf8View);
    undeclared.columns[0] = ColumnResolution::NotDeclared;
    undeclared.comparisons[0] = ComparisonPlan::Refused;
    // Bytewise, so an upper-case letter is below every lower-case one and
    // a multi-byte character above both.
    let cases = [
        (PredicateOp::Lt, "a", [true, false, false, false]),
        (PredicateOp::Le, "a", [true, true, false, false]),
        (PredicateOp::Gt, "a", [false, false, true, true]),
        (PredicateOp::Ge, "b", [false, false, true, true]),
    ];
    let values = ["Z", "a", "b", "é"];
    for resolved in [&unknown, &undeclared] {
        for (op, literal, want) in cases {
            let p = order_predicate(op, literal);
            assert!(matches!(
                resolve_term(&p, 0, resolved, 0),
                Err(Error::UnorderedPredicateColumn { .. })
            ));
            let term =
                super::resolve_term(&p, 0, resolved, 0, ComparisonSemantics::DataFusion).unwrap();
            assert!(term.comparison_notes().is_empty());
            let statistics = &term.compared.as_ref().unwrap().statistics;
            let kind = statistics.bounds.as_deref().map(|(kind, _)| kind.clone());
            assert_eq!(kind, Some(CompareKind::Text { length: None }));
            assert!(!statistics.dictionary);
            for (value, want) in values.iter().zip(want) {
                assert_eq!(
                    term.eval_value(Some(value)),
                    Some(Truth::of(want)),
                    "{value:?} {} {literal:?}",
                    op.symbol()
                );
            }
        }
    }
}

/// Which statistics a term reads, per semantics: `(the set of bounds
/// under <, dictionary under =)`. **Each semantics reads the set gathered
/// in its own order** — the primary set where one order serves both, the
/// DataFusion set where the register's exact order is another, and a
/// divergent column's one set, DataFusion's, in DataFusion's alone — and a dictionary
/// wherever its entries are the field's own text, which `character(n)`'s,
/// stored unpadded, are not.
#[test]
fn datafusion_semantics_reads_only_the_statistics_that_hold_there() {
    use BoundsSet::{DataFusion as F, Primary as P};
    let read = |declared: &str, literal: &str, semantics| {
        let believed = |op| {
            let p = order_predicate(op, literal);
            let resolved = one_column(declared, DataType::Utf8View);
            let term = super::resolve_term(&p, 0, &resolved, 0, semantics).unwrap();
            (term.compared.unwrap().statistics, resolved.bounds_kinds(0))
        };
        let (lt, stored) = believed(PredicateOp::Lt);
        let set = lt.bounds.and_then(|b| bounds_set_keyed_by(&stored, &b.0));
        (set, believed(PredicateOp::Eq).0.dictionary)
    };
    for (declared, literal, postgres, datafusion) in [
        ("integer", "1", (Some(P), true), (Some(P), true)),
        ("uuid", "00000000-0000-0000-0000-000000000000", (Some(P), true), (Some(P), true)),
        ("real", "1", (Some(P), true), (Some(P), true)),
        ("macaddr", "08:00:2b:01:02:03", (Some(P), true), (Some(P), true)),
        ("numeric", "1", (Some(P), true), (Some(F), true)),
        ("interval", "1 day", (Some(P), true), (Some(F), true)),
        ("public.mood", "ok", (Some(P), true), (Some(F), true)),
        // On the database's collation, so bounded bytewise, which is
        // DataFusion's order and not known to be the server's.
        ("text", "a", (None, true), (Some(P), true)),
        ("json", "{}", (None, false), (Some(P), false)),
        // Diverging, so bounded in DataFusion's order alone, as text.
        ("jsonb", "{}", (None, true), (Some(P), true)),
        ("character(3)", "a", (None, true), (Some(P), false)),
    ] {
        assert_eq!(read(declared, literal, ComparisonSemantics::Postgres), postgres, "{declared}");
        assert_eq!(
            read(declared, literal, ComparisonSemantics::DataFusion),
            datafusion,
            "{declared}"
        );
    }
}

/// A range type declaring a `canonical` function refuses **every**
/// comparing operator, `=` and `!=` included, through an error of its own
/// — in PostgreSQL's semantics the one refusal that cannot end by
/// offering the text comparison,
/// that comparison being what the file says is not the server's.
///
/// The two NULL tests still answer: they read no value and consult no
/// plan.
#[test]
fn a_range_declaring_a_canonical_function_refuses_every_operator() {
    let types = vec![TypeDef {
        name: "public.canonrange".into(),
        kind: TypeKind::Range {
            subtype: Some("integer".into()),
            multirange_type_name: None,
            canonical: Some("public.canonrange_canonical".into()),
        },
    }];
    let schema = nested_column("public.canonrange", &types);
    for op in [
        PredicateOp::Lt,
        PredicateOp::Le,
        PredicateOp::Gt,
        PredicateOp::Ge,
        PredicateOp::Eq,
        PredicateOp::Ne,
        PredicateOp::IsDistinctFrom,
        PredicateOp::IsNotDistinctFrom,
    ] {
        let p = Predicate { column: "v".into(), op, value: Some("[1,10]".into()) };
        let err = resolve_term(&p, 0, &schema, 7).unwrap_err();
        let Error::UncomparablePredicateColumn { reason, header_offset: 7, .. } = &err else {
            panic!("{op:?}: {err:?}")
        };
        // The sentence names the range type and its function, which is
        // what a user needs to go and look at the DDL.
        assert!(reason.contains("`public.canonrange`"), "{op:?}: {reason}");
        assert!(reason.contains("`public.canonrange_canonical`"), "{op:?}: {reason}");
    }
    for op in [PredicateOp::IsNull, PredicateOp::IsNotNull] {
        let p = Predicate { column: "v".into(), op, value: None };
        let term = resolve_term(&p, 0, &schema, 0).expect("a NULL test consults no plan");
        assert!(term.compared.is_none(), "{op:?}");
    }
}

/// Only a divergent classification produces a note, and the sentence is
/// chosen from the declared type rather than the Arrow one.
#[test]
fn a_divergent_comparison_produces_a_note_naming_why() {
    let note = |declared, data_type, literal| {
        let p = order_predicate(PredicateOp::Gt, literal);
        only_note(&resolve_term(&p, 0, &one_column(declared, data_type), 0).unwrap())
    };

    assert_eq!(note("integer", DataType::Int32, "1"), None, "an agreeing type says nothing");

    let text = note("character varying(10)", DataType::Utf8View, "a").unwrap();
    assert_eq!(text.divergence, ComparisonDivergence::UnknownCollation);
    assert!(text.message().contains("collation"), "{}", text.message());

    // `character(n)` asks the same collation question over its own
    // trimming comparison, so a bare column gets the collation sentence
    // and one declaring `COLLATE "C"` gets no note at all.
    let padded = note("character(10)", DataType::Utf8View, "a").unwrap();
    assert_eq!(padded.divergence, ComparisonDivergence::UnknownCollation);
    assert!(padded.message().contains("collation"), "{}", padded.message());

    // `json` is what `AsText` has left: the server defines no comparison
    // for it at all.
    let other = note("json", DataType::Utf8View, "1").unwrap();
    assert_eq!(other.divergence, ComparisonDivergence::AsText);
    assert!(other.message().contains("no comparison"), "{}", other.message());

    // Types that say nothing: a bare `numeric` and the three the
    // text-held row lost, sharing `Utf8View` with the rows above, and an
    // enum, emitted as a dictionary.
    assert_eq!(note("numeric", DataType::Utf8View, "10"), None);
    for (declared, literal) in [
        ("time with time zone", "00:00:00+00"),
        ("inet", "10.0.0.1"),
        ("macaddr", "08:00:2b:01:02:03"),
    ] {
        assert_eq!(note(declared, DataType::Utf8View, literal), None, "{declared}");
    }
    assert_eq!(
        note(
            "public.mood",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            "sad",
        ),
        None
    );
}

/// `jsonb` orders by kind before value, and the kind order is
/// `JsonbValue`'s own type codes: an object above an array, an array above
/// every scalar, and a boolean above a number above a string above JSON
/// `null` (I41). The first three pairs are cells of
/// `fixtures/16/oracle/comparisons.tsv`; the rest are the probe I41
/// records, no oracle case carrying a boolean or a string leaf.
///
/// `holds` reads `field <op> literal`, the direction a filter asks in.
#[test]
fn jsonb_orders_by_kind_before_value() {
    let holds =
        |field, op, literal| ordered("jsonb", DataType::Utf8View, op, literal, field).unwrap();
    assert!(holds("1", PredicateOp::Gt, "null"));
    assert!(holds("[1, 2]", PredicateOp::Gt, "1"));
    assert!(holds(r#"{"a": 1}"#, PredicateOp::Gt, "[1, 2]"));
    assert!(holds("true", PredicateOp::Gt, "1"));
    assert!(holds("1", PredicateOp::Gt, r#""a""#));
    assert!(holds("false", PredicateOp::Lt, "true"));
    // A bytewise comparison of the same text gets the first one
    // backwards, `1` being below `n`, and this one right: `{` is above.
    assert!(holds(r#"{"a": 1}"#, PredicateOp::Gt, "null"));
}

/// A container is ordered by its **size** before any member of it and
/// only then member-wise, so a one-pair object sorts below a two-pair one
/// whatever the keys say.
#[test]
fn a_jsonb_container_is_ordered_by_size_first() {
    let holds =
        |field, op, literal| ordered("jsonb", DataType::Utf8View, op, literal, field).unwrap();
    assert!(holds("[1, 2]", PredicateOp::Gt, "[3]"));
    assert!(holds(r#"{"z": 1}"#, PredicateOp::Lt, r#"{"a": 1, "b": 2}"#));
    assert!(holds("[]", PredicateOp::Lt, "[[]]"));
    // Same size: the members decide, keys before values.
    assert!(holds(r#"{"z": 1}"#, PredicateOp::Gt, r#"{"a": 2}"#));
    assert!(holds(r#"{"a": 2}"#, PredicateOp::Gt, r#"{"a": 1}"#));
    // A container element outranks a scalar one at the same position.
    assert!(holds("[[1], 2]", PredicateOp::Gt, "[3, [4]]"));
}

/// The pairs of an object are walked in **storage** order — key length
/// first, then bytes — while the keys themselves are compared as strings.
/// The two orders disagree here: stored, this is `z` against `y`, and
/// sorted alphabetically it would be `aa` against `y`, which answers the
/// other way (I41).
#[test]
fn jsonb_object_pairs_are_walked_in_storage_order() {
    assert!(
        ordered(
            "jsonb",
            DataType::Utf8View,
            PredicateOp::Gt,
            r#"{"y": 3, "zz": 4}"#,
            r#"{"z": 1, "aa": 2}"#,
        )
        .unwrap()
    );
}

/// A top-level scalar is stored in a one-element pseudo-array, and
/// `compareJsonbContainers` lets the element count *overwrite* the
/// `rawScalar` answer — so a scalar sorts below a one- or many-element
/// array and **above an empty one** (I41).
#[test]
fn a_top_level_scalar_outranks_an_empty_array() {
    let holds =
        |field, op, literal| ordered("jsonb", DataType::Utf8View, op, literal, field).unwrap();
    assert!(holds("1", PredicateOp::Gt, "[]"));
    assert!(holds("null", PredicateOp::Gt, "[]"));
    assert!(holds("1", PredicateOp::Lt, "[1]"));
    assert!(holds("1", PredicateOp::Lt, "[1, 2]"));
    // An object is above a scalar whatever its size: the type order
    // decides before any count.
    assert!(holds("1", PredicateOp::Lt, "{}"));
    assert!(holds("[]", PredicateOp::Lt, "{}"));
}

/// A `jsonb` number is a `numeric`, so `1`, `1.0` and `1e0` are one value
/// and `9` is below `10`. Whitespace, key order and a duplicate key are
/// normalized on the way in, the last of them to the **last** value
/// written, which is what the server stores.
#[test]
fn a_jsonb_literal_is_canonicalized_the_way_the_server_stores_it() {
    fn holds(field: &str, op: PredicateOp, literal: &str) -> bool {
        ordered("jsonb", DataType::Utf8View, op, literal, field).unwrap()
    }
    fn equal(field: &str, literal: &str) {
        assert!(holds(field, PredicateOp::Ge, literal), "{field} >= {literal}");
        assert!(holds(field, PredicateOp::Le, literal), "{field} <= {literal}");
    }
    for spelling in ["1", "1.0", "1e0", "1.00", "0.1e1", "100e-2"] {
        equal("1", spelling);
    }
    assert!(holds("10", PredicateOp::Gt, "9"));
    equal("-0", "0");
    equal(r#"{"a": 1}"#, r#"{  "a" : 1  }"#);
    equal(r#"{"a": 1}"#, r#"{"a":1}"#);
    equal(r#"{"a": 2}"#, r#"{"a":1,"a":2}"#);
    equal(r#"{"a": 1, "b": 2}"#, r#"{"b":2,"a":1}"#);
    // `escape_json` writes a tab as `\t`, and the `COPY` row then doubles
    // that backslash — so the field below is four characters of escaping
    // deep and the literal, which is COPY-decoded already, is two. Both
    // spellings a JSON escape has for the character are one value;
    // *unescaped* it is not a JSON string at all.
    equal("\"a\\\\tb\"", "\"a\\u0009b\"");
    // A character above the BMP, which `escape_json` writes as itself and
    // a literal may write as a surrogate pair.
    equal("\"\u{1f600}\"", "\"\\ud83d\\ude00\"");
}

/// The literal grammar is `jsonb_in`'s and nothing wider: every one of
/// these is text the server itself refuses (I41), so it is
/// `Error::PredicateValueDecode` naming the value.
#[test]
fn a_jsonb_literal_outside_the_input_grammar_is_refused() {
    for literal in [
        "01",
        "+1",
        ".5",
        "1.",
        "NaN",
        "1 2",
        "[1,]",
        "{a:1}",
        "{",
        "{\"a\":1",
        "'a'",
        "",
        "\"\\x41\"",
        "\"\\ud83d\"",
        "\"\\u0000\"",
        "\"a\nb\"",
        "1e999999",
        "1e9223372036854775807",
        "12e9223372036854775806",
        "1e9223372036854775808",
    ] {
        let err = ordered("jsonb", DataType::Utf8View, PredicateOp::Gt, literal, "1").unwrap_err();
        assert!(matches!(err, Error::PredicateValueDecode { .. }), "{literal:?}: {err:?}");
    }
    // A field the parser refuses is the other fault.
    let err = ordered("jsonb", DataType::Utf8View, PredicateOp::Gt, "1", "{oops}").unwrap_err();
    assert!(matches!(err, Error::FieldDecode { .. }), "{err:?}");
}

/// A `jsonb` column announces that its structure is compared exactly and
/// only a string leaf is left, on the database's collation (I32, I41).
/// `json` keeps the text-held sentence, the server defining no order for
/// it at all.
#[test]
fn jsonb_announces_its_string_leaves_and_json_stays_text_held() {
    let note = |declared, literal| {
        let p = order_predicate(PredicateOp::Gt, literal);
        only_note(&resolve_term(&p, 0, &one_column(declared, DataType::Utf8View), 0).unwrap())
    };
    let jsonb = note("jsonb", "1").unwrap();
    assert_eq!(jsonb.divergence, ComparisonDivergence::JsonbStringCollation);
    assert!(jsonb.message().contains("structurally"), "{}", jsonb.message());
    assert!(jsonb.message().contains("object key"), "{}", jsonb.message());

    let json = note("json", "anything").unwrap();
    assert_eq!(json.divergence, ComparisonDivergence::AsText);
    assert!(json.message().contains("no comparison"), "{}", json.message());
}

/// A bare `numeric` orders as a decimal: `9` is below `10` where the text
/// it is held as puts it above, and trailing zeros are not part of the
/// value (I33).
#[test]
fn a_bare_numeric_orders_by_value_not_by_its_text() {
    let n = |op, value, field| ordered("numeric", DataType::Utf8View, op, value, field).unwrap();
    assert!(n(PredicateOp::Gt, "9", "10"));
    assert!(!n(PredicateOp::Lt, "9", "10"));
    // Equal by value, written two ways — a bare `numeric` preserves the
    // scale it was written with.
    assert!(n(PredicateOp::Ge, "1.5", "1.50"));
    assert!(n(PredicateOp::Le, "1.5", "1.50"));
    assert!(!n(PredicateOp::Gt, "1.5", "1.50"));
    // Sign, then magnitude, then the fraction.
    assert!(n(PredicateOp::Lt, "0", "-0.001"));
    assert!(n(PredicateOp::Gt, "-2", "-1.9"));
    assert!(n(PredicateOp::Gt, "0.45", "0.5"));
    assert!(n(PredicateOp::Lt, "0.55", "0.5"));
    // One zero, however either side spells it.
    assert!(n(PredicateOp::Ge, "-0", "0.000"));
    assert!(n(PredicateOp::Le, "-0", "0.000"));
    // Past every fixed-width integer, which is why the key is digits.
    let big = "1".repeat(200);
    assert!(n(PredicateOp::Gt, &big, &format!("{big}0")));
}

/// All three of `numeric`'s specials are ordered, and only a *bare*
/// column admits the two infinities: any typmod rejects them (I34), so on
/// a `numeric(77,0)` the literal is refused.
#[test]
fn a_bare_numeric_carries_all_three_specials() {
    let n = |op, value, field| ordered("numeric", DataType::Utf8View, op, value, field).unwrap();
    assert!(n(PredicateOp::Gt, "1.5", "Infinity"));
    assert!(n(PredicateOp::Lt, "1.5", "-Infinity"));
    assert!(n(PredicateOp::Gt, "Infinity", "NaN"));
    assert!(n(PredicateOp::Ge, "NaN", "NaN"));
    assert!(!n(PredicateOp::Gt, "NaN", "NaN"));
    assert!(n(PredicateOp::Gt, "-Infinity", "Infinity"));
    // `numeric_out`'s spelling and nothing else.
    let err = ordered("numeric", DataType::Utf8View, PredicateOp::Gt, "inf", "0").unwrap_err();
    assert!(matches!(err, Error::PredicateValueDecode { .. }), "{err:?}");

    // Past `Decimal256`: a `NaN` is reachable, an infinity is not.
    let wide = |op, value, field| ordered("numeric(77,0)", DataType::Utf8View, op, value, field);
    assert!(wide(PredicateOp::Gt, "0", "NaN").unwrap());
    let err = wide(PredicateOp::Gt, "Infinity", "0").unwrap_err();
    assert!(matches!(err, Error::PredicateValueDecode { .. }), "{err:?}");
}

/// An enum orders by declaration order (I33), which is the reverse of the
/// label text here: `sad` is declared first and sorts last alphabetically.
#[test]
fn an_enum_orders_by_declaration_order() {
    let dict = || DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8));
    let e = |op, value, field| ordered("public.mood", dict(), op, value, field).unwrap();
    assert!(e(PredicateOp::Gt, "sad", "ok"));
    assert!(!e(PredicateOp::Lt, "sad", "ok"));
    assert!(e(PredicateOp::Ge, "sad", "sad"));
    assert!(!e(PredicateOp::Gt, "sad", "sad"));

    // A label the type does not declare is not a value of the column, on
    // either side.
    let err = ordered("public.mood", dict(), PredicateOp::Gt, "nope", "sad").unwrap_err();
    assert!(matches!(err, Error::PredicateValueDecode { .. }), "{err:?}");
    let err = ordered("public.mood", dict(), PredicateOp::Gt, "sad", "SAD").unwrap_err();
    assert!(matches!(err, Error::FieldDecode { .. }), "{err:?}");
}

/// An `interval`'s months collapse to 30 days and its days to 86400
/// seconds, so `1 mon`, `30 days` and `720:00:00` are one value written
/// three ways — every answer here read out of
/// `fixtures/17/oracle/comparisons.tsv`.
///
/// `holds` reads `field <op> literal`, the direction a filter asks in:
/// the row's own value on the left.
#[test]
fn an_interval_orders_by_its_collapsed_span() {
    let holds = |field, op, literal| {
        ordered("interval", DataType::Interval(IntervalUnit::MonthDayNano), op, literal, field)
            .unwrap()
    };
    for spelling in ["30 days", "720:00:00"] {
        assert!(holds(spelling, PredicateOp::Ge, "1 mon"), "{spelling}");
        assert!(holds(spelling, PredicateOp::Le, "1 mon"), "{spelling}");
    }
    // `-1 days` is below `00:00:00`.
    assert!(holds("-1 days", PredicateOp::Lt, "00:00:00"));
    // The full `interval_out` form, with a year part and a time tail.
    let full = "1 year 2 mons 3 days 04:05:06";
    assert!(holds(full, PredicateOp::Lt, "2 years"));
    assert!(holds(full, PredicateOp::Gt, "1 mon"));
    // The sign on a time tail belongs to the whole tail, and a `+` on a
    // part that follows a negative one is `AddPostgresIntPart`'s.
    assert!(holds("-1 days -04:00:00", PredicateOp::Lt, "-1 days"));
    assert!(holds("-1 days +04:00:00", PredicateOp::Gt, "-1 days"));
    // The two infinities are read on every file, in `date_out`'s
    // spellings rather than `numeric_out`'s.
    assert!(holds("infinity", PredicateOp::Gt, "1 mon"));
    assert!(holds("-infinity", PredicateOp::Lt, "1 mon"));
    assert!(holds("infinity", PredicateOp::Ge, "infinity"));
    assert!(matches!(
        ordered(
            "interval",
            DataType::Interval(IntervalUnit::MonthDayNano),
            PredicateOp::Gt,
            "Infinity",
            "1 mon"
        )
        .unwrap_err(),
        Error::PredicateValueDecode { .. }
    ));
}

/// The `interval` grammar is `interval_out`'s under `IntervalStyle =
/// postgres` (I4) and nothing wider, so a spelling the server's *input*
/// function takes is refused. The first three are literals
/// `fixtures/17/oracle/literals.tsv` records the server accepting.
#[test]
fn an_interval_literal_outside_the_output_grammar_is_refused() {
    for literal in ["1.5 hours", "P1Y2M", "1 century", "1 month", "@ 1 day", "", "1 day "] {
        let err = ordered(
            "interval",
            DataType::Interval(IntervalUnit::MonthDayNano),
            PredicateOp::Gt,
            literal,
            "1 mon",
        )
        .unwrap_err();
        assert!(matches!(err, Error::PredicateValueDecode { .. }), "{literal:?}: {err:?}");
    }
}

/// A `time with time zone` sorts by the UTC instant and breaks a tie on
/// the zone, so `00:00:00-05` is above `00:00:00+00` — five hours later,
/// though it reads earlier — and two spellings of one instant are still
/// unequal (`fixtures/16/oracle/comparisons.tsv`).
#[test]
fn a_timetz_orders_by_the_utc_instant_then_the_zone() {
    let holds = |field, op, literal| {
        ordered("time with time zone", DataType::Utf8View, op, literal, field).unwrap()
    };
    assert!(holds("00:00:00-05", PredicateOp::Gt, "00:00:00+00"));
    // Same instant, different zone: ordered, and not equal. The stored
    // zone is seconds *west* of GMT, so the value displaying `-05` ranks
    // above the one displaying `+00`.
    assert!(holds("00:00:00-05", PredicateOp::Gt, "05:00:00+00"));
    assert!(!holds("00:00:00-05", PredicateOp::Le, "05:00:00+00"));
    // `24:00:00` is a real boundary value, above everything finite here.
    assert!(holds("24:00:00+00", PredicateOp::Gt, "05:00:00+00"));
    // A `timetz` always carries an offset; one without is not a value.
    assert!(matches!(
        ordered("time with time zone", DataType::Utf8View, PredicateOp::Gt, "00:00:00", "\\N")
            .unwrap_err(),
        Error::PredicateValueDecode { .. }
    ));
}

/// `network_cmp_internal`: family first, then the shorter netmask's worth
/// of address bits, then the netmask, then the whole address (I40). The
/// last pair is the one no address-then-netmask key can get right.
#[test]
fn inet_orders_by_family_then_prefix_then_netmask() {
    let holds =
        |field, op, literal| ordered("inet", DataType::Utf8View, op, literal, field).unwrap();
    // IPv4 below every IPv6 address, whatever the bytes say.
    assert!(holds("192.168.1.1", PredicateOp::Lt, "::1"));
    assert!(holds("192.168.1.1", PredicateOp::Gt, "10.0.0.1"));
    // Same first eight bits, different netmask: the netmask decides,
    // *before* the host bits below it are looked at.
    assert!(holds("10.1.0.0/8", PredicateOp::Lt, "10.0.0.0/16"));
    assert!(holds("10.0.0.0/16", PredicateOp::Gt, "10.1.0.0/8"));
}

/// `cidr` compares exactly as `inet` does; the only difference is that
/// `cidr_in` refuses a value with a bit set below its netmask, and so
/// does this (`fixtures/16/oracle/literals.tsv`).
#[test]
fn a_cidr_refuses_a_literal_with_host_bits_set() {
    let holds =
        |field, op, literal| ordered("cidr", DataType::Utf8View, op, literal, field).unwrap();
    assert!(holds("10.0.0.0/8", PredicateOp::Lt, "192.168.1.0/24"));
    assert!(holds("192.168.1.0/24", PredicateOp::Lt, "::/0"));
    assert!(matches!(
        ordered("cidr", DataType::Utf8View, PredicateOp::Gt, "192.168.1.1/24", "10.0.0.0/8")
            .unwrap_err(),
        Error::PredicateValueDecode { .. }
    ));
    // The same value is an ordinary `inet`.
    assert!(
        ordered("inet", DataType::Utf8View, PredicateOp::Lt, "192.168.1.1/24", "10.0.0.0/8")
            .unwrap()
    );
}

/// The two MAC types compare as their bytes and differ only in width, so
/// a six-octet literal is not a `macaddr8` value and the reverse holds
/// too. The colon form is the only one read, `macaddr_out` writing it.
#[test]
fn a_macaddr_compares_as_its_octets_at_its_own_width() {
    let holds = |declared, field, op, literal| {
        ordered(declared, DataType::Utf8View, op, literal, field).unwrap()
    };
    assert!(holds("macaddr", "08:00:2b:01:02:03", PredicateOp::Lt, "08:00:2b:01:02:04"));
    assert!(holds(
        "macaddr8",
        "08:00:2b:01:02:03:04:05",
        PredicateOp::Lt,
        "08:00:2b:01:02:03:04:06"
    ));
    for (declared, literal) in [
        ("macaddr", "08:00:2b:01:02:03:04:05"),
        ("macaddr8", "08:00:2b:01:02:03"),
        // A separator the server takes and `macaddr_out` never writes.
        ("macaddr", "08-00-2b-01-02-03"),
        // `from_str_radix` would take the sign; the digit check does not.
        ("macaddr", "+8:00:2b:01:02:03"),
    ] {
        let err =
            ordered(declared, DataType::Utf8View, PredicateOp::Gt, literal, "\\N").unwrap_err();
        assert!(matches!(err, Error::PredicateValueDecode { .. }), "{declared} {literal}: {err:?}");
    }
}

/// `uuid` and `bytea` compare as their bytes, which is what
/// `uuid_internal_cmp` and `byteacmp` do.
#[test]
fn uuid_and_bytea_compare_bytewise() {
    assert!(
        ordered(
            "uuid",
            DataType::FixedSizeBinary(16),
            PredicateOp::Gt,
            "00000000-0000-0000-0000-000000000000",
            "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11",
        )
        .unwrap()
    );
    // `\x00ff` is COPY-escaped in the row and sorts after `\x00`.
    assert!(ordered("bytea", DataType::Binary, PredicateOp::Gt, "\\x00", "\\\\x00ff").unwrap());
}

/// **A `bytea` literal matches its value in whichever form the dump
/// wrote it**, `hex` or `escape` (I4, I56), written in either form
/// itself: under `=` and `!=`, in an `IN` list, through a group's
/// dictionary, and under an ordering operator.
#[test]
fn a_bytea_literal_matches_its_value_in_either_output_form() {
    use Truth::{False, True};
    // Bytes `00 41 5c` and `00 41 5d`, each as a literal and as a field
    // COPY-escaped on disk.
    let literals = ["\\x00415c", "\\000A\\\\"];
    let fields = ["\\\\x00415c", "\\\\000A\\\\\\\\"];
    let others = ["\\\\x00415d", "\\\\000A]"];
    let compare =
        |op, literal, field| ordered("bytea", DataType::Binary, op, literal, field).unwrap();
    for literal in literals {
        for field in fields {
            assert!(compare(PredicateOp::Eq, literal, field), "{literal:?} = {field:?}");
            assert!(!compare(PredicateOp::Ne, literal, field), "{literal:?} != {field:?}");
            assert!(compare(PredicateOp::Le, literal, field), "{literal:?} <= {field:?}");
        }
        for field in others {
            assert!(!compare(PredicateOp::Eq, literal, field), "{literal:?} = {field:?}");
            assert!(compare(PredicateOp::Ne, literal, field), "{literal:?} != {field:?}");
            assert!(compare(PredicateOp::Gt, literal, field), "{literal:?} < {field:?}");
        }
    }
    let schema = one_column("bytea", DataType::Binary);
    for list in [[Some(literals[0]), Some("\\x01")], [Some("\\001"), Some(literals[1])]] {
        let resolved = ResolvedExpr::In(membership(&schema, 0, &list).unwrap());
        for field in fields {
            assert_eq!(exact_over(&resolved, field.as_bytes()).unwrap(), True, "{list:?}");
        }
        for field in others {
            assert_eq!(exact_over(&resolved, field.as_bytes()).unwrap(), False, "{list:?}");
        }
    }
    for literal in literals {
        let p = order_predicate(PredicateOp::Eq, literal);
        let term = ResolvedExpr::Term(resolve_term(&p, 0, &schema, 0).unwrap());
        for (entry, want) in [("\\000A\\\\", True), ("\\x00415c", True), ("\\000A]", False)] {
            let group = Group {
                rows: 1,
                nulls: Some(0),
                bounds: None,
                dictionary: Some(vec![entry.into()]),
            };
            assert_eq!(term.truths(&group), sets(&[want]), "{literal:?} over {entry:?}");
        }
    }
}

/// Dates and timestamps compare as the instant they decode to, so a
/// BC date is below every AD one however its text sorts.
#[test]
fn dates_compare_as_instants() {
    assert!(
        ordered("date", DataType::Date32, PredicateOp::Lt, "2024-01-01", "0044-01-01 BC").unwrap()
    );
    assert!(
        ordered(
            "timestamp with time zone",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            PredicateOp::Lt,
            "2024-01-01 00:00:00+00",
            "2023-12-31 18:30:00+00",
        )
        .unwrap()
    );
}

/// `=` renders the filter's literal into the spelling the file holds and
/// compares bytes, so a literal that is the same *value* written
/// differently matches.
#[test]
fn equality_canonicalizes_the_literal_once() {
    let eq = |declared, data_type, literal, field| {
        ordered(declared, data_type, PredicateOp::Eq, literal, field).unwrap()
    };
    // The scale the column declares, which the file writes out in full.
    assert!(eq("numeric(10,2)", DataType::Decimal128(10, 2), "1.5", "1.50"));
    assert!(!eq("numeric(10,2)", DataType::Decimal128(10, 2), "1.5", "1.51"));
    // A leading zero, a leading plus, a case difference, an input-only
    // separator: each is the same value the file spells one way.
    assert!(eq("integer", DataType::Int32, "+007", "7"));
    assert!(eq("oid", DataType::UInt32, "0042", "42"));
    assert!(eq(
        "uuid",
        DataType::FixedSizeBinary(16),
        "0AF1C2D3-0000-0000-0000-000000000000",
        "0af1c2d3-0000-0000-0000-000000000000"
    ));
    assert!(eq("macaddr", DataType::Utf8View, "08:00:2B:01:02:03", "08:00:2b:01:02:03"));
    // Both `numeric` kinds spell `NaN` the way their own `*_out` does,
    // so it renders to itself.
    assert!(eq("numeric(10,2)", DataType::Decimal128(10, 2), "NaN", "NaN"));
    assert!(eq("date", DataType::Date32, "infinity", "infinity"));
    assert!(!eq("date", DataType::Date32, "infinity", "-infinity"));
}

/// `=` on a `text` or `varchar` column is a byte comparison:
/// `CompareKind::Text { length: None }` renders the literal to itself, so the commonest
/// column there is carries no per-row work.
#[test]
fn equality_on_a_text_column_is_unchanged() {
    for declared in ["text", "character varying(10)", "name", "json"] {
        assert!(ordered(declared, DataType::Utf8View, PredicateOp::Eq, "a b", "a b").unwrap());
        assert!(!ordered(declared, DataType::Utf8View, PredicateOp::Eq, "a b", "a  b").unwrap());
    }
}

/// `character(n)` is the third category — the *field* narrowed per row.
/// The dump writes every value padded to `n` and `bpchareq` strips the
/// padding from both sides (I38), so an unpadded literal matches.
#[test]
fn equality_on_a_char_column_trims_both_sides() {
    let eq = |literal, field| {
        ordered("character(10)", DataType::Utf8View, PredicateOp::Eq, literal, field).unwrap()
    };
    assert!(eq("a", "a         "));
    assert!(eq("a         ", "a"));
    // A tab is a value byte, not padding. The *field* side is COPY TEXT,
    // so the tab is written escaped; the literal side is a `Predicate`'s
    // own value and is not.
    assert!(!eq("a", "a\\t        "));
    assert!(eq("a\t", "a\\t        "));
}

/// The five kinds whose `*_out` is not injective over the values one file
/// can hold: no rendering of the literal makes them bytewise, so both
/// sides are decoded per row.
#[test]
fn the_decoded_kinds_call_two_spellings_of_one_value_equal() {
    let eq = |declared, literal, field| {
        ordered(declared, DataType::Utf8View, PredicateOp::Eq, literal, field).unwrap()
    };
    // A bare `numeric` keeps its display scale (I33).
    assert!(eq("numeric", "1.5", "1.50"));
    assert!(eq("numeric", "1.50", "1.5"));
    assert!(!eq("numeric", "1.5", "1.51"));
    // An `interval` collapses months to 30 days and days to 86400 s (I40).
    assert!(eq("interval", "1 mon", "30 days"));
    assert!(eq("interval", "30 days", "720:00:00"));
    // `float8eq` has one zero, and a dump can write `-0`.
    assert!(ordered("double precision", DataType::Float64, PredicateOp::Eq, "0", "-0").unwrap());
    // ... and `NaN = NaN` is true on the server, which is neither IEEE's
    // answer nor Rust's (I33).
    assert!(ordered("double precision", DataType::Float64, PredicateOp::Eq, "NaN", "NaN").unwrap());
    // `jsonb` prints its numbers through `numeric_out` and compares them
    // by value (I41), so one document is written two ways.
    assert!(eq("jsonb", "{\"a\": 1.50}", "{\"a\": 1.5}"));
    assert!(!eq("jsonb", "{\"a\": 1.5}", "{\"a\": 2}"));
    // The two that decode for a reason about this build rather than
    // about PostgreSQL.
    assert!(eq("inet", "10.0.0.1/32", "10.0.0.1"));
    assert!(eq("time with time zone", "00:00:00+00", "00:00:00+00"));
    assert!(!eq("time with time zone", "00:00:00+00", "01:00:00+01"));
}

/// `!=` is `=` inverted, and a NULL field matches neither.
#[test]
fn typed_inequality_inverts_and_a_null_matches_neither() {
    let ne = |literal, field| {
        ordered("numeric(10,2)", DataType::Decimal128(10, 2), PredicateOp::Ne, literal, field)
            .unwrap()
    };
    assert!(!ne("1.5", "1.50"));
    assert!(ne("1.5", "1.51"));
    for op in [PredicateOp::Eq, PredicateOp::Ne] {
        assert!(
            !ordered("integer", DataType::Int32, op, "1", "\\N").unwrap(),
            "a NULL field matches no comparison"
        );
    }
}

/// A literal that is not a value of the column's type is
/// `Error::PredicateValueDecode` before a row is read — the same refusal
/// an ordering operator makes, on the same output-form-only grammar.
#[test]
fn an_undecodable_equality_literal_is_refused_at_resolution() {
    for (declared, data_type, literal) in [
        ("integer", DataType::Int32, "abc"),
        ("boolean", DataType::Boolean, "true"),
        ("oid", DataType::UInt32, "-1"),
        ("bytea", DataType::Binary, "\\101"),
        ("public.mood", DataType::Utf8View, "furious"),
        ("numeric(10,2)", DataType::Decimal128(10, 2), "1.005"),
        ("interval", DataType::Interval(IntervalUnit::MonthDayNano), "1 month"),
    ] {
        let p = order_predicate(PredicateOp::Eq, literal);
        assert!(
            matches!(
                resolve_term(&p, 0, &one_column(declared, data_type), 0).unwrap_err(),
                Error::PredicateValueDecode { value, .. } if value == literal
            ),
            "{declared} = {literal}"
        );
    }
}

/// The refusal names the form the column's `CompareKind` reads, not only
/// the value it turned down — so a `boolean` is told how a `boolean` is
/// written, and `interval`, `inet` and `macaddr` are answered by the same
/// sentence.
#[test]
fn a_refused_literal_names_the_form_the_column_accepts() {
    for (declared, data_type, literal, accepted) in [
        ("boolean", DataType::Boolean, "true", "`t` or `f`"),
        (
            "interval",
            DataType::Interval(IntervalUnit::MonthDayNano),
            "1 month",
            "the way `interval` prints it",
        ),
        ("inet", DataType::Utf8View, "10", "an IPv4 or IPv6 address"),
        ("macaddr", DataType::Utf8View, "08-00-2b-01-02-03", "six colon-separated hex pairs"),
        // The two arms where the kind's own payload is the answer: the
        // enum's declared labels, and the `numeric` scale.
        ("public.mood", DataType::Utf8View, "furious", "declared labels: 'sad', 'ok'"),
        (
            "numeric(10,2)",
            DataType::Decimal128(10, 2),
            "1.005",
            "as a number with at most 2 decimal places, or `NaN`",
        ),
    ] {
        let p = order_predicate(PredicateOp::Eq, literal);
        let message =
            resolve_term(&p, 0, &one_column(declared, data_type), 0).unwrap_err().to_string();
        assert!(message.contains(literal), "{declared} = {literal}: {message}");
        assert!(message.contains(accepted), "{declared} = {literal}: {message}");
    }
}

/// The enum clause names the labels themselves, quoted the way the dump
/// writes them and a `--filter` value reads them back, and stops at
/// [`ENUM_LABELS_SHOWN`] with a pointer at the output that carries the
/// rest.
#[test]
fn the_enum_clause_quotes_its_labels_and_caps_the_list() {
    let kind = |labels: &[&str]| CompareKind::Enum {
        labels: labels.iter().map(|l| (*l).to_string()).collect::<Arc<[String]>>(),
        exact: true,
    };
    assert_eq!(
        accepted_form(&kind(&["sad", "has space", "has'quote"])),
        "as one of the type's declared labels: 'sad', 'has space', 'has''quote'"
    );

    let many: Vec<String> = (0..ENUM_LABELS_SHOWN + 3).map(|i| format!("l{i}")).collect();
    let clause =
        accepted_form(&CompareKind::Enum { labels: many.iter().cloned().collect(), exact: true });
    assert!(clause.contains("'l0'"), "{clause}");
    assert!(clause.contains(&format!("'l{}'", ENUM_LABELS_SHOWN - 1)), "{clause}");
    assert!(!clause.contains(&format!("'l{ENUM_LABELS_SHOWN}'")), "{clause}");
    assert!(clause.ends_with("and 3 more; see `info --detail`"), "{clause}");
}

/// The `numeric(p,s)` clause branches on the sign of the scale, as what
/// the column refuses does: a positive scale bounds the fraction, a zero
/// scale admits no fraction, and a negative one admits only multiples of
/// a power of ten.
#[test]
fn the_numeric_clause_branches_on_the_sign_of_the_scale() {
    for (scale, expected) in [
        (2i8, "as a number with at most 2 decimal places, or `NaN`"),
        (1, "as a number with at most 1 decimal place, or `NaN`"),
        (0, "as a whole number, or `NaN`"),
        (-2, "as a whole number that is a multiple of 100, or `NaN`"),
    ] {
        let kind = CompareKind::Decimal { precision: 10, scale };
        assert_eq!(accepted_form(&kind), expected, "scale {scale}");
    }
    // The scale-free arm keeps the scale-free clause: a `p > 76` column
    // is held as text and refuses a literal only past `numeric_in`'s
    // bounds (I63), which the clause states.
    assert!(NUMERIC_DIGITS.contains(&decode::NUMERIC_INTEGER_DIGITS_MAX.to_string()));
    assert!(NUMERIC_DIGITS.contains(&decode::NUMERIC_DSCALE_MAX.to_string()));
    assert_eq!(
        accepted_form(&CompareKind::Numeric {
            infinities: false,
            typmod: Some(crate::pgtype::NumericTypmod { precision: 77, scale: 0 }),
        }),
        "as a number of at most 131072 digits before the point and 16383 after it, or `NaN`"
    );
}

/// A divergence is operator-conditional, and a collation's reaches
/// ordering alone: a libc collation is deterministic, so `texteq` is a
/// byte comparison whatever the collation is.
#[test]
fn a_collation_divergence_does_not_reach_equality() {
    let note = |declared, op, literal| {
        let p = order_predicate(op, literal);
        only_note(&resolve_term(&p, 0, &one_column(declared, DataType::Utf8View), 0).unwrap())
    };
    for declared in ["text", "character varying(10)", "character(10)"] {
        assert_eq!(
            note(declared, PredicateOp::Gt, "a").map(|n| n.divergence),
            Some(ComparisonDivergence::UnknownCollation),
            "{declared} under an ordering operator"
        );
        assert_eq!(note(declared, PredicateOp::Eq, "a"), None, "{declared} under `=`");
    }
    assert_eq!(
        note("jsonb", PredicateOp::Gt, "1").map(|n| n.divergence),
        Some(ComparisonDivergence::JsonbStringCollation)
    );
    assert_eq!(note("jsonb", PredicateOp::Eq, "1"), None);
    // `json` reaches both, the server defining neither operator for it.
    for op in [PredicateOp::Gt, PredicateOp::Eq] {
        assert_eq!(note("json", op, "1").map(|n| n.divergence), Some(ComparisonDivergence::AsText));
    }
}

/// A column the register has no comparison for still answers `=`, as
/// text, and says so where the column is a resolved scalar: `box_eq`
/// compares areas and a byte comparison does not.
#[test]
fn a_column_with_no_comparison_answers_equality_and_says_when_that_is_a_guess() {
    let p = order_predicate(PredicateOp::Eq, "(1,1),(0,0)");

    // `box` is `ColumnResolution::UnknownType` in a real query, this
    // build mapping no Arrow type for it, and that is the outcome the
    // announcement is keyed on.
    let mut scalar = one_column("box", DataType::Utf8View);
    scalar.columns[0] = ColumnResolution::UnknownType;
    let term = resolve_term(&p, 0, &scalar, 0).unwrap();
    let note = only_note(&term).expect("an unmodelled scalar announces");
    assert_eq!(note.divergence, ComparisonDivergence::UnmodelledType);
    assert!(note.message().contains("`box` compares areas"), "{}", note.message());
    assert!(
        term.eval(RawRow::unchecked(b"(1,1),(0,0)"), &mut RowSplit::default(), "public.t", 0)
            .unwrap()
            .is_true()
    );
    assert!(
        !term
            .eval(RawRow::unchecked(b"(3,3),(2,2)"), &mut RowSplit::default(), "public.t", 0)
            .unwrap()
            .is_true()
    );

    // A nested column the register does not compare is the other
    // plan-less population and is silent: its `range_out` text renders
    // the value, so a byte comparison of two canonical spellings is the
    // server's answer.
    let mut nested = one_column("public.opaquerange", DataType::Utf8View);
    nested.plans[0] = NestedPlan::Range(Box::new(NestedPlan::Scalar));
    assert_eq!(only_note(&resolve_term(&p, 0, &nested, 0).unwrap()), None);

    // So is a column no DDL explained — `--data-only`, or
    // `--schema-mode strings`. There is no declared type to qualify, and
    // `ResolvedSchema::notes` has already said so.
    let mut undeclared = one_column("mystery", DataType::Utf8View);
    undeclared.columns[0] = ColumnResolution::NotDeclared;
    assert_eq!(only_note(&resolve_term(&p, 0, &undeclared, 0).unwrap()), None);
}

/// A nested column one of whose positions has no order here still
/// answers `=`, over the container's whole text, and **says what that
/// costs**: `array_cmp` and `record_cmp` look up the position type's
/// comparison proc and raise when there is none, so a `json` element
/// takes the server's `=` away as surely as it takes its order. That is
/// [`ComparisonDivergence::AsText`]'s own sentence one level down, and
/// the note names the position exactly as the ordering refusal does.
#[test]
fn a_nested_uncomparable_position_announces_under_equality() {
    let types = vec![TypeDef {
        name: "public.jsonpair".into(),
        kind: TypeKind::Composite {
            fields: Some(vec![ColumnDef::new("ok", "integer"), ColumnDef::new("doc", "json")]),
        },
    }];
    for (declared, path) in
        [("json[]", "[]"), ("public.jsonpair", ".doc"), ("public.jsonpair[]", "[].doc")]
    {
        let resolved = nested_column(declared, &types);
        for op in [PredicateOp::Eq, PredicateOp::Ne] {
            let term = resolve_term(&order_predicate(op, "{}"), 0, &resolved, 0).unwrap();
            let note =
                only_note(&term).unwrap_or_else(|| panic!("{declared} under {}", op.symbol()));
            assert_eq!(note.column, "v", "{declared}");
            assert_eq!(note.path.as_deref(), Some(path), "{declared}");
            assert_eq!(note.declared_type, "json", "{declared}");
            assert_eq!(note.divergence, ComparisonDivergence::AsText, "{declared}");
            assert!(note.message().contains("no equality"), "{}", note.message());
        }
        // `=` is the byte comparison of the container's own canonical
        // text.
        let term =
            resolve_term(&order_predicate(PredicateOp::Eq, "{1,2}"), 0, &resolved, 0).unwrap();
        assert!(
            term.eval(RawRow::unchecked(b"{1,2}"), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
                .is_true(),
            "{declared}"
        );
        assert!(
            !term
                .eval(RawRow::unchecked(b"{1,3}"), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
                .is_true(),
            "{declared}"
        );
        // The announcement is under `=`; `<` still names the same
        // position in a refusal.
        let Err(err) = resolve_term(&order_predicate(PredicateOp::Lt, "{}"), 0, &resolved, 0)
        else {
            panic!("{declared} is ordered");
        };
        assert!(err.to_string().contains(&format!("`{path}`")), "{err}");
    }
}

/// A column the **resolver** declined announces off its resolution and
/// never off its tree — the boundary worth pinning, since the tree such a
/// column carries does hold an `Uncomparable` position and the
/// announcement above must not reach it. Both shapes resolve the column
/// itself to text (I22, I26), so `resolve_term` drops the plan one branch
/// earlier than the nested one.
///
/// Silence is the right answer for `public.intarr[]`: PostgreSQL orders it
/// through `array_ops` and its `array_out` text renders the value. For
/// `box[]` it is `KD10` one level down, `box_eq` comparing areas; closing
/// that is a question about which *resolutions* announce, not about this
/// tree.
#[test]
fn a_position_the_resolver_declined_announces_nothing() {
    let types = vec![TypeDef { name: "public.intarr".into(), kind: TypeKind::domain("integer[]") }];
    for (declared, resolution) in [
        ("box[]", ColumnResolution::OpaqueElementType),
        ("public.intarr[]", ColumnResolution::NestedArrayElement),
    ] {
        let mut resolved = one_column(declared, DataType::Utf8View);
        resolved.comparisons = vec![comparison_for(declared, None, &types, &[])];
        assert!(
            matches!(resolved.comparisons[0], ComparisonPlan::Nested(NestedCompare::Array(_))),
            "{declared} carries a tree with an uncomparable position"
        );
        resolved.columns[0] = resolution;
        let term = resolve_term(&order_predicate(PredicateOp::Eq, "{}"), 0, &resolved, 0).unwrap();
        assert_eq!(only_note(&term), None, "{declared}");
    }
}

/// A column stating a collation the dump declares `deterministic = false`
/// announces under **both** operator families (I42) — the only collation
/// divergence that reaches `=`.
///
/// The rows still come back bytewise, which is the defect the note names:
/// under a non-deterministic collation two values spelled differently can
/// be equal to the server, so `=` is a weaker filter than the server's and
/// `<` is a different order.
#[test]
fn a_non_deterministic_collation_announces_under_equality_and_ordering() {
    let collations = [CollationDef { name: "public.icu_ci".to_string(), deterministic: false }];
    let schema = |collation: Option<&str>, declared: &[CollationDef]| {
        let mut resolved = one_column("text", DataType::Utf8View);
        resolved.comparisons = vec![comparison_for("text", collation, &[], declared)];
        resolved
    };

    for op in [PredicateOp::Gt, PredicateOp::Eq, PredicateOp::Ne] {
        let p = order_predicate(op, "a");
        let resolved = schema(Some("public.icu_ci"), &collations);
        let note = only_note(&resolve_term(&p, 0, &resolved, 0).unwrap())
            .unwrap_or_else(|| panic!("{op:?} announces"));
        assert_eq!(note.divergence, ComparisonDivergence::NonDeterministicCollation);
        assert!(note.message().contains("non-deterministic"), "{}", note.message());
    }

    // The same clause with nothing declared about it is the ordinary
    // named-collation divergence, which `=` does not see.
    let eq = order_predicate(PredicateOp::Eq, "a");
    let plain = schema(Some("public.icu_ci"), &[]);
    assert_eq!(only_note(&resolve_term(&eq, 0, &plain, 0).unwrap()), None);

    // The comparison itself is `Text`, so the term evaluates bytewise.
    let term = resolve_term(&eq, 0, &schema(Some("public.icu_ci"), &collations), 0).unwrap();
    assert!(only_note(&term).is_some());
    assert!(
        term.eval(RawRow::unchecked(b"a"), &mut RowSplit::default(), "public.t", 0)
            .unwrap()
            .is_true()
    );
    assert!(
        !term
            .eval(RawRow::unchecked(b"A"), &mut RowSplit::default(), "public.t", 0)
            .unwrap()
            .is_true()
    );
}

/// The four tie-breaks `array_cmp` reaches only when the elements agree,
/// in the order it reaches them: element count, then dimension count,
/// then the dimensions themselves, then the lower bounds (I45).
///
/// The dimension pair is the one no oracle case carries: `{{1,2,3,4}}` and
/// `{{1,2},{3,4}}` hold the same four elements in the same order, are both
/// two-dimensional, and differ only in `dims`.
#[test]
fn an_array_falls_back_to_its_shape_only_when_the_elements_agree() {
    let types = test_types();
    let lt = |field: &str, literal: &str| {
        nested_verdict("integer[]", &types, PredicateOp::Lt, field, literal).unwrap()
    };
    // Elements first: element 0 settles `{1,9}` against `{2,0}` before
    // any count is looked at.
    assert_eq!(lt("{1,9}", "{2,0}"), Truth::True);
    // Element count, once the shorter array's elements agree.
    assert_eq!(lt("{1,2}", "{1,2,3}"), Truth::True);
    // Dimension count, at equal element counts.
    assert_eq!(lt("{{1},{2}}", "{1,2}"), Truth::False);
    // The dimensions, at equal element and dimension counts.
    assert_eq!(lt("{{1,2,3,4}}", "{{1,2},{3,4}}"), Truth::True);
    // The lower bounds, last of all.
    assert_eq!(lt("[0:1]={1,2}", "{1,2}"), Truth::True);
    // Two NULLs are equal and a NULL is above every value, at every
    // level and under every operator.
    assert_eq!(
        nested_verdict("integer[]", &types, PredicateOp::Eq, "{NULL}", "{NULL}").unwrap(),
        Truth::True
    );
    assert_eq!(lt("{NULL}", "{2147483647}"), Truth::False);
}

/// A zero-field composite is written `()`, and so is a one-field
/// composite holding NULL — the literal cannot tell them apart and the
/// declared field list decides (I23). The strict field decoder cannot ask
/// that question for itself, so the comparison asks it.
#[test]
fn a_zero_field_composite_compares_equal_to_itself() {
    let types = [
        TypeDef {
            name: "public.empty_comp".into(),
            kind: TypeKind::Composite { fields: Some(Vec::new()) },
        },
        TypeDef {
            name: "public.one".into(),
            kind: TypeKind::Composite { fields: Some(vec![ColumnDef::new("x", "integer")]) },
        },
    ];
    assert_eq!(
        nested_verdict("public.empty_comp", &types, PredicateOp::Eq, "()", "()").unwrap(),
        Truth::True
    );
    assert_eq!(
        nested_verdict("public.one", &types, PredicateOp::Eq, "()", "()").unwrap(),
        Truth::True,
        "one field, holding NULL, on both sides"
    );
    // A field count the composite does not declare is not a value of the
    // type: `Error::FieldDecode` for a field, and the literal is refused
    // at resolution.
    assert!(matches!(
        nested_verdict("public.one", &types, PredicateOp::Eq, "(1,2)", "(1)"),
        Err(Error::FieldDecode { .. })
    ));
    assert!(matches!(
        nested_verdict("public.one", &types, PredicateOp::Eq, "(1)", "(1,2)"),
        Err(Error::PredicateValueDecode { .. })
    ));
}

/// `int2vector` names no operator of its own, so the server compares it
/// through `anyarray` polymorphism — element-wise, not over the text
/// (I47). `'2' < '10'` is where the two answers part company, and is what
/// the committed oracle case is built around.
#[test]
fn an_int2vector_compares_element_wise_and_not_as_text() {
    let lt = |field: &str, literal: &str| {
        nested_verdict("int2vector", &[], PredicateOp::Lt, field, literal).unwrap()
    };
    assert_eq!(lt("2", "10"), Truth::True, "element-wise; bytewise would say false");
    assert_eq!(lt("10", "2"), Truth::False);
    // `array_cmp`: the elements first, then the shorter one.
    assert_eq!(lt("1 2", "1 2 3"), Truth::True);
    assert_eq!(lt("", "0"), Truth::True);
    assert_eq!(lt("-32768 32767", "0"), Truth::True);
    assert_eq!(
        nested_verdict("int2vector", &[], PredicateOp::Eq, "1 2 3", "1 2 3").unwrap(),
        Truth::True
    );
    // The literal takes `int2vectorin`'s superset, the type having no
    // element input function for the leaf rule to apply to.
    assert_eq!(
        nested_verdict("int2vector", &[], PredicateOp::Eq, "1 2", "  +1   02  ").unwrap(),
        Truth::True
    );
    // And the array spelling is not this type's, on either side.
    assert!(matches!(
        nested_verdict("int2vector", &[], PredicateOp::Eq, "1 2", "{1,2}"),
        Err(Error::PredicateValueDecode { .. })
    ));
    assert!(matches!(
        nested_verdict("int2vector", &[], PredicateOp::Eq, "{1,2}", "1 2"),
        Err(Error::FieldDecode { .. })
    ));
}

/// The literal side reads the container's `*_in` superset and each leaf
/// its own `*_out` form — one rule at every depth. So `{ 1 , 2 }` is
/// `{1,2}` (`array_in` drops whitespace around an element) while
/// `( 1 ,a)` is refused (`record_in` keeps it, and `1 ` is not what
/// `int4out` writes).
#[test]
fn a_nested_literal_is_lenient_about_the_container_and_strict_about_the_leaf() {
    let types = [TypeDef {
        name: "public.point2d".into(),
        kind: TypeKind::Composite {
            fields: Some(vec![ColumnDef::new("x", "integer"), ColumnDef::new("y", "text")]),
        },
    }];
    let ints = test_types();
    for literal in ["{ 1 , 2 }", "{1,2}", "[1:2]={1,2}", "{1,\"2\"}"] {
        assert_eq!(
            nested_verdict("integer[]", &ints, PredicateOp::Eq, "{1,2}", literal).unwrap(),
            Truth::True,
            "{literal}"
        );
    }
    assert_eq!(
        nested_verdict("public.point2d", &types, PredicateOp::Eq, "(1,a)", "(1,a)").unwrap(),
        Truth::True
    );
    let err =
        nested_verdict("public.point2d", &types, PredicateOp::Eq, "(1,a)", "( 1 ,a)").unwrap_err();
    let Error::PredicateValueDecode { accepted, .. } = &err else { panic!("{err:?}") };
    assert!(accepted.contains("spelled as the dump spells it"), "{accepted}");
}

/// A range's bounds settle infinity, then the held value, then
/// inclusivity — and an exclusive bound's answer depends on *which end*
/// it is (I46).
///
/// `empty` below everything is the fourth rule, and it is not recoverable
/// from the bounds: `empty` and `(,)` both have two absent bounds and sit
/// at opposite ends of the order.
#[test]
fn a_range_bound_settles_infinity_then_value_then_inclusivity() {
    let types = test_types();
    let lt = |field: &str, literal: &str| {
        nested_verdict("numrange", &types, PredicateOp::Lt, field, literal).unwrap()
    };
    assert_eq!(lt("empty", "(,)"), Truth::True);
    assert_eq!(lt("(,)", "empty"), Truth::False);
    assert_eq!(
        nested_verdict("numrange", &types, PredicateOp::Eq, "empty", "empty").unwrap(),
        Truth::True
    );
    // An absent bound is the extreme of its own end.
    assert_eq!(lt("(,5)", "[1,10)"), Truth::True);
    assert_eq!(lt("[1,10)", "[1,)"), Truth::True);
    // Same value, different inclusivity, and the two ends disagree about
    // which way it goes.
    assert_eq!(lt("[1,10)", "(1,10)"), Truth::True);
    assert_eq!(lt("[1,10)", "[1,10]"), Truth::True);
}

/// A discrete range is rewritten into canonical form on the way in and a
/// continuous one is not, so `[1,10]` and `[1,10)` are one value of
/// `int4range` and two of `numrange` (I46).
///
/// Two rewrites, and the second is not a successor: bounds that end up
/// equal without both ends including the point collapse to `empty`, which
/// is why `int4range '(1,2)'` holds nothing. It runs before the canonical
/// function *and* after it, which is the only way `(1,2)` reaches it.
#[test]
fn a_discrete_range_is_canonicalized_and_a_continuous_one_is_not() {
    let types = test_types();
    let eq = |declared: &str, field: &str, literal: &str| {
        nested_verdict(declared, &types, PredicateOp::Eq, field, literal).unwrap()
    };
    for literal in ["[1,10)", "[1,9]", "(0,10)", "(0,9]"] {
        assert_eq!(eq("int4range", "[1,10)", literal), Truth::True, "{literal}");
    }
    assert_eq!(eq("int4range", "empty", "(1,2)"), Truth::True);
    assert_eq!(eq("int4range", "empty", "[1,1)"), Truth::True);
    assert_eq!(eq("numrange", "empty", "[1,1)"), Truth::True, "the collapse is not discrete");
    assert_eq!(eq("numrange", "[1,10)", "[1,10]"), Truth::False);
    // `daterange_canonical` skips a bound that is not a finite date, so
    // an infinity keeps the inclusivity it was written with (I34) — and
    // the *other* bound is still rewritten.
    assert_eq!(eq("daterange", "[2020-01-01,infinity]", "[2020-01-01,infinity]"), Truth::True);
    assert_eq!(eq("daterange", "[-infinity,2020-01-02)", "[-infinity,2020-01-01]"), Truth::True);
    // The successor can leave the subtype's range, which the server
    // raises on — at the subtype's own width (`Discrete::largest`).
    // Under every operator, and from either bound; one below it is a
    // value. A bound past the width is refused before that
    // (`an_integer_literal_past_its_width_is_refused_and_one_inside_it_read`).
    let refused = |declared: &str, op: PredicateOp, literal: &str| {
        let field = if declared.starts_with("date") { "[2020-01-01,2020-01-02)" } else { "[1,10)" };
        let field =
            if declared.contains("multi") { format!("{{{field}}}") } else { field.to_string() };
        matches!(
            nested_verdict(declared, &types, op, &field, literal),
            Err(Error::PredicateValueDecode { .. })
        )
    };
    for (declared, literal) in [
        ("int4range", "[1,2147483647]"),
        ("int4range", "(2147483647,)"),
        ("int4multirange", "{[1,2147483647]}"),
        ("int8range", "[1,9223372036854775807]"),
        ("int8range", "(9223372036854775807,)"),
        ("daterange", "[2020-01-01,5874897-12-31]"),
        ("daterange", "(5874897-12-31,)"),
        ("datemultirange", "{[2020-01-01,5874897-12-31]}"),
    ] {
        for op in [PredicateOp::Eq, PredicateOp::Lt, PredicateOp::Ge] {
            assert!(refused(declared, op, literal), "{declared} {op:?} {literal}");
        }
    }
    assert_eq!(eq("int4range", "[1,2147483647)", "[1,2147483646]"), Truth::True);
    assert_eq!(eq("int4range", "[1,2147483647)", "[1,2147483647)"), Truth::True);
    assert_eq!(
        eq("daterange", "[2020-01-01,5874897-12-31)", "[2020-01-01,5874897-12-30]"),
        Truth::True
    );
    assert_eq!(
        decode::decode_date32("5874897-12-31").map(i64::from),
        Some(Discrete::Date.largest())
    );
    // A lower bound above its upper is `22000` on the server — a fault
    // the container grammar cannot see, since the text is well formed.
    assert!(matches!(
        nested_verdict("int4range", &types, PredicateOp::Gt, "[1,10)", "[10,1)"),
        Err(Error::PredicateValueDecode { .. })
    ));
    assert!(matches!(
        nested_verdict("int4multirange", &types, PredicateOp::Gt, "{[1,10)}", "{[10,1)}"),
        Err(Error::PredicateValueDecode { .. })
    ));
}

/// A multirange's members are sorted, emptied out and merged before
/// anything is compared, so several spellings are one value and the
/// comparison itself is a plain sequence walk (I46).
///
/// Whether two members merge is the range type's question, not the
/// bounds': `{[1,5),[6,10)}` stays two members even in `int4range`, while
/// `{[1,5],[6,10)}` becomes one, canonicalization having already made the
/// first `[1,6)`. The same pair in `numrange` never merges.
#[test]
fn a_multirange_is_sorted_coalesced_and_emptied_before_it_is_compared() {
    let types = test_types();
    let eq = |declared: &str, field: &str, literal: &str| {
        nested_verdict(declared, &types, PredicateOp::Eq, field, literal).unwrap()
    };
    for literal in ["{[1,10)}", "{[1,5),[5,10)}", "{[5,10),[1,5)}", "{[1,1),[1,10)}"] {
        assert_eq!(eq("int4multirange", "{[1,10)}", literal), Truth::True, "{literal}");
    }
    assert_eq!(eq("int4multirange", "{[1,10)}", "{empty,[1,10)}"), Truth::True);
    assert_eq!(eq("int4multirange", "{[1,5)}", "{[1,3),[2,5)}"), Truth::True, "overlapping");
    assert_eq!(eq("int4multirange", "{[1,5),[6,10)}", "{[1,5),[6,10)}"), Truth::True);
    assert_eq!(eq("int4multirange", "{[1,10)}", "{[1,5],[6,10)}"), Truth::True, "adjacent");
    assert_eq!(eq("nummultirange", "{[1,5),[6,10)}", "{[1,5),[6,10)}"), Truth::True);
    assert_eq!(eq("nummultirange", "{[1,5),(5,10)}", "{[1,5),(5,10)}"), Truth::True);
    // Member-wise, then the shorter one first.
    let lt = |field: &str, literal: &str| {
        nested_verdict("int4multirange", &types, PredicateOp::Lt, field, literal).unwrap()
    };
    assert_eq!(lt("{}", "{[1,10)}"), Truth::True);
    assert_eq!(lt("{[1,5),[6,10)}", "{[1,10)}"), Truth::True);
}

/// The comparison register against the committed comparison oracle:
/// every cell of `fixtures/<13-18>/oracle/comparisons.tsv`, answered by
/// the same `resolve_term`/`matches` path a `--filter` takes, and compared
/// with what the server itself said (`docs/design/decisions.md`, "D70").
///
/// This is the check the oracle exists for: `oracle_register.py` and
/// `oracle_differences.py` are about which rows exist, and nothing else
/// compares an answer to an answer.
mod oracle {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::cache::CacheMode;
    use crate::copy::{decode_field, encode_field, split_fields};
    use crate::index::preamble_only;
    use crate::io::LocalFileSource;
    use crate::preamble::{ColumnDef, DatabaseMetadata, DumpMetadata, TableDef};
    use crate::resolve::{SchemaMode, resolve_columns};
    use crate::scan::ScanOptions;

    /// The majors `scripts/generate_fixtures.py` generates, which is what
    /// `fixtures/` holds a directory per.
    const MAJORS: [u32; 6] = [13, 14, 15, 16, 17, 18];

    /// The six operators this asserts, in `comparisons.tsv`'s own column
    /// order, paired with their cell offset after the four key columns.
    ///
    /// **`=` and `<>` are asked over a wider population than the other
    /// four**: a column the register has no order for still answers `=`
    /// as text, so a case the register does not order is skipped for the
    /// ordering operators and asserted for these two.
    ///
    /// What they add over `<=`/`>=`, which already carried the register's
    /// equality, is the **canonicalization** — the three-way choice
    /// `equality_comparison` makes. Both operands of a cell are values the
    /// server itself stored, so for a canonicalized kind the assertion is
    /// that the file's own spelling compares byte for byte; the content is
    /// in the kinds where it cannot, and every one of them has a case
    /// here: `real`'s `-0` against `0`, a bare `numeric`'s `1.5` against
    /// `1.50` and a `jsonb` number written two ways. `character(10)`'s
    /// trimmed comparison is reached only over values the file pads
    /// alike.
    const ASSERTED: [(usize, PredicateOp); 6] = [
        (0, PredicateOp::Lt),
        (1, PredicateOp::Le),
        (2, PredicateOp::Gt),
        (3, PredicateOp::Ge),
        (4, PredicateOp::Eq),
        (5, PredicateOp::Ne),
    ];

    /// The declared types whose columns the register refuses an ordering
    /// operator on, so none of their ordering cells is asserted: `xml`, an enum with
    /// no labels, a user-defined base type with no operator class, a domain
    /// over a type the register has no comparison for, the built-ins it
    /// has no comparison for (`line`, `xid`, `xid8`, `cid`), and the two
    /// nested shapes that are refused for reasons of their own. They are
    /// here because the register refuses them, not because PostgreSQL
    /// orders them: it orders the domain over `box` (by area), `xid8`, the
    /// nested array and the multirange from v14, and refuses `xml`, the
    /// base type itself, `line`, `xid` and `cid` (`E42883`).
    ///
    /// **`json` is not here**, and the difference is the point: this build
    /// *does* compare it, bytewise, and every cell of it is `E42883`
    /// because PostgreSQL defines no comparison at all — so the walk skips
    /// it as a server refusal rather than as a refusal of ours.
    ///
    /// It is asserted as an exact set, so a type that quietly stops
    /// comparing fails here rather than passing as one more skip.
    const REFUSED: [&str; 10] = [
        // I26: an array whose element is itself an array. The column
        // resolves to text, and the register agrees rather than claiming
        // an order the resolver has already declined.
        "public.intarr[]",
        // A multirange companion **before v14**, where the type does not
        // exist and the range's DDL carries no `multirange_type_name` to
        // find it through (I10). Its v14+ cells are asserted; a case is
        // recorded here if it was refused at *any* major.
        "public.myrange_multi",
        // Scalars with no comparison in the register at all.
        "public.box_domain",
        "public.mybase",
        "public.empty_enum",
        "xml",
        "line",
        "xid",
        "xid8",
        "cid",
    ];

    /// The cases where this build's answer is knowingly not
    /// PostgreSQL's, as `(type, collation, left, right)` — the exception
    /// set, and it is **met**: every entry is a real disagreement in the
    /// committed files, and every disagreement is an entry.
    ///
    /// **Met means met everywhere it is permitted.** An entry is keyed by
    /// the case, so it has to disagree in every major that carries the
    /// case and under every operator its divergence reaches — for a
    /// divergence of order 24 cells, four ordering operators by six
    /// majors. Taking
    /// *somewhere in the walk* as met would instead leave the block
    /// passing after a collation moved for one major, or after the
    /// comparator stopped being antisymmetric under one operator.
    ///
    /// **`=` and `<>` are not among those cells, and that is the
    /// assertion rather than an omission.** Every divergence in this list
    /// but `box`'s, below, is a divergence of *order*
    /// ([`ComparisonDivergence::affects_equality`]) — a libc collation is
    /// deterministic, so `texteq` is a byte comparison whatever the
    /// collation is — so a disagreement under `=` would fail the
    /// announcement check below rather than count toward an entry.
    ///
    /// Each entry's column also has to *announce* its divergence through
    /// [`ComparisonDivergence`], **under this operator**, which is
    /// asserted alongside — so an exception cannot be claimed for a
    /// column the register tells the user it is confident about. **Three
    /// of its populations are one statement asked at three depths** — a
    /// collation the file does not carry (I32), reached through a column,
    /// through an array's element and through a string inside a document:
    ///
    /// - **A `text[]` element**, ordered by `varstr_cmp` under the
    ///   database's collation exactly as a `text` value is, one level
    ///   down.
    /// - **A `jsonb` string leaf.** `compareJsonbScalarValue` passes
    ///   `DEFAULT_COLLATION_OID` to `varstr_cmp`, so a leaf is ordered by
    ///   the database's collation, which a plain dump does not record
    ///   (I32); everything structural above it — the kind order, a
    ///   container's size, storage order, the raw-scalar wrapper — is
    ///   asserted rather than excepted, which is what makes this list two
    ///   entries rather than the arm.
    /// - **`box`'s area equality**, the population apart and the only
    ///   one this build could close by writing code. `box_eq`
    ///   compares the two rectangles' *areas*, so the server calls
    ///   `(1,1),(0,0)` and `(3,3),(2,2)` equal where a byte comparison
    ///   does not — and `box` has no comparison in the register, so the
    ///   four ordering operators are refused on it and `=` falls back to
    ///   text. `<>` is not among the cells because PostgreSQL defines no
    ///   `box <> box` at all, which is why each entry is met over six
    ///   cells rather than twenty-four. The column announces
    ///   `ComparisonDivergence::UnmodelledType`, and the deficiency is
    ///   `KD10`.
    /// - **`text` under the database's own collation**, which is glibc's
    ///   `en_US.utf8` on this apparatus: case is a lower-weight
    ///   difference than letter, an accent sorts with its base letter
    ///   rather than after `z`, and punctuation is ignored at the primary
    ///   level, so `_x` sorts where `x` does. The five unordered pairs the
    ///   case table chose for those reasons are here in both directions,
    ///   and `_x` reaches every letter in the alphabet rather than only
    ///   `ax`.
    const EXCEPTIONS: &[(&str, &str, &str, &str)] = &[
        // A `text[]` column inherits its element's collation boundary:
        // the elements are ordered by `varstr_cmp` under the database's
        // collation exactly as a `text` column's values are, one level
        // down. `b` is below `B` under `en_US.utf8` and above it
        // bytewise, and `NULL` is below `a` bytewise and above it under
        // the locale — the same two disagreements the `text` rows below
        // carry, reached through an element.
        ("text[]", "\\N", "{a,b}", "{a,B}"),
        ("text[]", "\\N", "{a,B}", "{a,b}"),
        ("text[]", "\\N", "{\"NULL\"}", "{a,b}"),
        ("text[]", "\\N", "{a,b}", "{\"NULL\"}"),
        ("text[]", "\\N", "{\"NULL\"}", "{a,B}"),
        ("text[]", "\\N", "{a,B}", "{\"NULL\"}"),
        // `box_eq` is an area comparison, not a value one.
        ("public.box_domain", "\\N", "(1,1),(0,0)", "(3,3),(2,2)"),
        ("public.box_domain", "\\N", "(3,3),(2,2)", "(1,1),(0,0)"),
        ("jsonb", "\\N", "{\"a\": \"a\"}", "{\"a\": \"A\"}"),
        ("jsonb", "\\N", "{\"a\": \"A\"}", "{\"a\": \"a\"}"),
        // Case is a lower-weight difference than letter.
        ("text", "default", "A", "a"),
        ("text", "default", "a", "A"),
        ("text", "default", "a", "B"),
        ("text", "default", "B", "a"),
        ("text", "default", "ax", "B"),
        ("text", "default", "B", "ax"),
        // An accent sorts with its base letter, not after `z`.
        ("text", "default", "é", "f"),
        ("text", "default", "f", "é"),
        ("text", "default", "é", "hello"),
        ("text", "default", "hello", "é"),
        // Punctuation is ignored at the primary level, so `_x` sorts
        // where `x` does — above every letter in the alphabet.
        ("text", "default", "_x", "a"),
        ("text", "default", "a", "_x"),
        ("text", "default", "_x", "ax"),
        ("text", "default", "ax", "_x"),
        ("text", "default", "_x", "co-op"),
        ("text", "default", "co-op", "_x"),
        ("text", "default", "_x", "coop"),
        ("text", "default", "coop", "_x"),
        ("text", "default", "_x", "de luge"),
        ("text", "default", "de luge", "_x"),
        ("text", "default", "_x", "deluge"),
        ("text", "default", "deluge", "_x"),
        ("text", "default", "_x", "e"),
        ("text", "default", "e", "_x"),
        ("text", "default", "_x", "é"),
        ("text", "default", "é", "_x"),
        ("text", "default", "_x", "f"),
        ("text", "default", "f", "_x"),
        ("text", "default", "_x", "hello"),
        ("text", "default", "hello", "_x"),
    ];

    fn fixture(major: u32, rest: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../fixtures/{major}/{rest}"))
    }

    /// Split one COPY TEXT line of a committed oracle file into its
    /// decoded fields — the same L1 decoder that reads a dump, because
    /// the server wrote these files with `COPY ... TO STDOUT`.
    fn fields(line: &[u8]) -> Vec<Option<String>> {
        split_fields(line).map(|f| decode_field(f).unwrap().map(|v| v.into_owned())).collect()
    }

    fn rows(path: &Path) -> Vec<Vec<Option<String>>> {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        bytes.split(|&b| b == b'\n').filter(|l| !l.is_empty()).map(fields).collect()
    }

    /// The `COLLATE` clause a dump would carry for a case labelled with
    /// this collation. `C` is written; `default` is *not*, because
    /// `pg_dump` writes a clause only where the column's collation is not
    /// its type's default (I37) — so the dump-realistic form of the
    /// oracle's `COLLATE "default"` case is a bare column, which is the
    /// register's `UnknownCollation` arm. Both reach the same comparison.
    fn clause(collation: Option<&String>) -> Option<&'static str> {
        match collation.map(String::as_str) {
            Some("C") => Some("pg_catalog.\"C\""),
            _ => None,
        }
    }

    /// A one-column schema for a case's declared type and collation,
    /// built by handing `resolve_columns` a one-table `DumpMetadata` —
    /// the same function a real block resolves through.
    ///
    /// **Not a hand-built `ResolvedSchema`**, and the difference is
    /// load-bearing now that `=` is asserted: the resolution, the nested
    /// plan and the comparison plan have to agree the way they do in a
    /// real query, because which of them `resolve_term` consults decides
    /// whether an equality term announces a divergence. A schema claiming
    /// every case is `Mapped` and `Scalar` would put `integer[]` and
    /// `box` in one population that no dump ever produces.
    fn schema(declared: &str, collation: Option<&str>, types: &[TypeDef]) -> ResolvedSchema {
        let column = ColumnDef {
            name: "v".into(),
            declared_type: declared.into(),
            collation: collation.map(str::to_string),
            not_null: None,
        };
        let metadata = DumpMetadata {
            databases: vec![DatabaseMetadata {
                name: None,
                preamble_complete: true,
                server_version: None,
                pg_dump_version: None,
                extensions: Vec::new(),
                types: types.to_vec(),
                collations: Vec::new(),
                tables: [("public.t".to_string(), TableDef::with_columns(vec![column]))]
                    .into_iter()
                    .collect(),
            }],
        };
        resolve_columns(
            "public.t",
            &["v".to_string()],
            Some(&metadata),
            None,
            SchemaMode::Typed,
            &[],
        )
    }

    /// The [`Truth`] this build answers a cell with, or the fault it
    /// raised instead — a literal it will not decode, or a field it will
    /// not. `field` is `None` for a SQL NULL one, which reaches the row
    /// as `\N` like any other, and answers `Unknown` exactly where the
    /// server's cell reads `u`.
    ///
    /// The second half of the pair is whether the resolved term
    /// **announces** a divergence, read off `ComparisonNote` — the channel
    /// a user actually sees — rather than off the register's plan. Which
    /// divergences reach which operator is the term's own conclusion
    /// ([`ComparisonDivergence::affects_equality`]), and asking the plan
    /// would be a second copy of that rule living in the test.
    fn answer(
        declared: &str,
        collation: Option<&str>,
        types: &[TypeDef],
        op: PredicateOp,
        field: Option<&str>,
        bound: &str,
    ) -> (std::result::Result<Truth, String>, bool) {
        let resolved = schema(declared, collation, types);
        let p = Predicate { column: "v".into(), op, value: Some(bound.into()) };
        let term = match resolve_term(&p, 0, &resolved, 0) {
            Ok(term) => term,
            Err(e) => return (Err(e.to_string()), false),
        };
        let announced = !term.comparison_notes().is_empty();
        let got = term
            .eval(RawRow::unchecked(&encode_field(field)), &mut RowSplit::default(), "public.t", 0)
            .map_err(|e| e.to_string());
        (got, announced)
    }

    /// Every major's `CREATE TYPE`/`CREATE DOMAIN` list, read from the
    /// `types` fixture that major's oracle database was loaded from — the
    /// same DDL, so a case's declared type resolves against the list the
    /// server itself had.
    async fn types_of(major: u32) -> Vec<TypeDef> {
        let source = LocalFileSource::open(fixture(major, "types/default.sql")).unwrap();
        let (metadata, _) =
            preamble_only(&source, &ScanOptions::default(), &CacheMode::DISABLED).await.unwrap();
        metadata.databases.into_iter().next().expect("a dump names a database").types
    }

    /// **A float literal the server refuses is refused, and one it reads
    /// that this build reads means what it does to the server**: every
    /// `real` and `double precision` row of `literals.tsv` at every
    /// major, the range's edges among them (I59).
    #[test]
    fn a_float_literal_is_read_as_the_server_reads_it() {
        literals_are_read_as_the_server_reads_them(&["real", "double precision"]);
    }

    /// **An integer literal the server refuses is refused, and one it
    /// reads that this build reads means what it does to the server**:
    /// every `smallint`, `integer` and `bigint` row of `literals.tsv` at
    /// every major, a value one past each width among them (I60).
    #[test]
    fn an_integer_literal_is_read_as_the_server_reads_it() {
        literals_are_read_as_the_server_reads_them(&["smallint", "integer", "bigint"]);
    }

    /// **A date, time or timestamp literal the server refuses is refused,
    /// and one it reads that this build reads means what it does to the
    /// server**: every row of the five types and their ranges in
    /// `literals.tsv` at every major, an impossible month, a time past
    /// `24:00:00` and a timestamp past the range among them (I61).
    #[test]
    fn a_date_or_time_literal_is_read_as_the_server_reads_it() {
        literals_are_read_as_the_server_reads_them(&[
            "date",
            "time without time zone",
            "time with time zone",
            "timestamp without time zone",
            "timestamp with time zone",
            "daterange",
            "tsrange",
            "tstzrange",
        ]);
    }

    /// **A `numeric` or `jsonb` literal the server refuses is refused, and
    /// one it reads that this build reads means what it does to the
    /// server**: every bare `numeric` and `jsonb` row of `literals.tsv` at
    /// every major. The `numeric(p,s)` rows are cast with the typmod, which
    /// a comparison's literal is not (I63).
    #[test]
    fn a_numeric_literal_is_read_as_the_server_reads_it() {
        literals_are_read_as_the_server_reads_them(&["numeric", "jsonb"]);
    }

    /// **A `json` field the server refuses a strict parse refuses, and one
    /// it reads it does not**: every `json` row of `literals.tsv` at every
    /// major, read as a field (I72).
    #[test]
    fn a_json_field_is_refused_as_the_server_refuses_it() {
        let plan = comparison_for("json", None, &[], &[]);
        let mut asserted = 0usize;
        for major in MAJORS {
            for row in rows(&fixture(major, "oracle/literals.tsv")) {
                let (Some("json"), Some(text)) = (row[0].as_deref(), row[1].as_deref()) else {
                    continue;
                };
                let reads = row[2].as_deref() == Some("ok");
                assert_eq!(field_refused(&plan, text), !reads, "{major} {text:?}");
                asserted += 1;
            }
        }
        assert!(asserted >= MAJORS.len() * 4, "{asserted} rows");
    }

    /// **A `uuid` literal the server refuses is refused, and one it reads
    /// this build reads to the same value**: every `uuid` row of
    /// `literals.tsv` at every major, a value in braces among them (I64).
    #[test]
    fn a_uuid_literal_is_read_as_the_server_reads_it() {
        literals_are_read_as_the_server_reads_them(&["uuid"]);
    }

    /// **A `boolean`, `oid`, `inet`, `cidr`, `macaddr`, `macaddr8` or
    /// `bytea` literal the server refuses is refused, and one it reads
    /// that this build reads means what it does to the server**: every row
    /// of the seven in `literals.tsv` at every major (I65–I69).
    #[test]
    fn a_literal_of_a_kind_with_an_input_function_port_is_read_as_the_server_reads_it() {
        literals_are_read_as_the_server_reads_them(&[
            "boolean", "oid", "inet", "cidr", "macaddr", "macaddr8", "bytea",
        ]);
    }

    /// Each row of `literals.tsv` declaring one of `declared`, at every
    /// major, put to `=` against a field holding the server's output for
    /// it: one the server refuses is refused, and one it reads is either
    /// read to the same value or refused as a shortfall (D55). A literal an
    /// older major refuses and the newest reads is the newest's (I35), as
    /// a v13 `numeric`'s `Infinity` is. One two majors read apart — a
    /// non-additive literal row of `fixtures/oracle-differences.tsv`, each
    /// exempt there beside its invariant — is read to the value some
    /// major reads it to, or refused.
    fn literals_are_read_as_the_server_reads_them(declared: &[&str]) {
        let apart: BTreeSet<(Option<String>, Option<String>)> =
            rows(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/oracle-differences.tsv"))
                .into_iter()
                .filter(|row| {
                    row[2].as_deref() == Some("literals")
                        && row[10].as_deref() == Some("non-additive")
                })
                .map(|row| (row[3].clone(), row[4].clone()))
                .collect();
        let mut read_apart: BTreeMap<(String, String), (bool, Vec<u32>)> = BTreeMap::new();
        let newest = MAJORS[MAJORS.len() - 1];
        let newest_reads: BTreeSet<(Option<String>, Option<String>)> =
            rows(&fixture(newest, "oracle/literals.tsv"))
                .into_iter()
                .filter(|row| row[2].as_deref() == Some("ok"))
                .map(|row| (row[0].clone(), row[1].clone()))
                .collect();
        let mut asserted = 0usize;
        for major in MAJORS {
            for row in rows(&fixture(major, "oracle/literals.tsv")) {
                let ty = row[0].as_deref().expect("a case names a type");
                if !declared.contains(&ty) {
                    continue;
                }
                let Some(literal) = row[1].as_deref() else { continue };
                let status = row[2].as_deref().expect("a row has a status");
                let (got, _) = answer(ty, None, &[], PredicateOp::Eq, row[3].as_deref(), literal);
                if apart.contains(&(row[0].clone(), row[1].clone())) {
                    let (met, readers) =
                        read_apart.entry((ty.to_string(), literal.to_string())).or_default();
                    match &got {
                        Ok(truth) if status == "ok" => {
                            *met |= *truth == Truth::True;
                            readers.push(major);
                        }
                        Ok(_) => {}
                        Err(e) => {
                            assert!(e.contains("filter value"), "{major} {ty} {literal}: {e}");
                            *met = true;
                        }
                    }
                } else if status == "ok" {
                    // A spelling this build does not read is a shortfall,
                    // not a disagreement (D55).
                    if let Err(e) = &got {
                        assert!(e.contains("filter value"), "{major} {ty} {literal}: {e}");
                    } else {
                        assert_eq!(got, Ok(Truth::True), "{major} {ty} {literal}");
                    }
                } else if !newest_reads.contains(&(row[0].clone(), row[1].clone())) {
                    let e = got.expect_err(&format!("{major} {ty} {literal} is refused"));
                    assert!(e.contains("filter value"), "{major} {ty} {literal}: {e}");
                }
                asserted += 1;
            }
        }
        for ((ty, literal), (met, readers)) in read_apart {
            assert!(met, "{ty} {literal}: read to no value {readers:?} read it to");
        }
        assert!(asserted > 0, "no row of {declared:?} was read");
    }

    #[tokio::test]
    async fn the_register_answers_every_committed_oracle_cell() {
        // Keyed by case so one disagreement is reported as the pair it
        // is, not as four cells; the value is the majors and operators
        // it showed up under.
        let mut disagreed: BTreeMap<(String, String, String, String), BTreeSet<String>> =
            BTreeMap::new();
        // How many cells each case was asserted over at all, so "met"
        // can mean met everywhere rather than met somewhere.
        let mut walked: BTreeMap<(String, String, String, String), usize> = BTreeMap::new();
        let mut refused: BTreeSet<String> = BTreeSet::new();
        let mut asserted = 0usize;

        for major in MAJORS {
            let types = types_of(major).await;
            // The `*_out` text the server canonicalizes each accepted
            // literal to, keyed by type and literal. It is what a dump
            // *holds*, so every value below is put to the register in
            // this form rather than as the case's input spelling: a
            // `character(10)`'s blank padding, a `jsonb`'s single space
            // after each `:`, a `numeric(10,2)`'s rounded scale.
            let outputs: BTreeMap<(String, String), String> =
                rows(&fixture(major, "oracle/literals.tsv"))
                    .into_iter()
                    .filter_map(|r| Some(((r[0].clone()?, r[1].clone()?), r[3].clone()?)))
                    .collect();

            for row in rows(&fixture(major, "oracle/comparisons.tsv")) {
                let declared = row[0].clone().expect("a case names a type");
                let collation = clause(row[3].as_ref());
                let plan = comparison_for(&declared, collation, &types, &[]);
                // A column the register has no comparison for is skipped
                // for the four ordering operators, which it refuses, and
                // still asserted for `=`/`<>`, which fall back to text.
                let ordered = plan.orders();
                if !ordered {
                    refused.insert(declared.clone());
                }
                // How the case names itself in a report and in
                // `EXCEPTIONS`: the raw cells, so an entry can be copied
                // out of a failure straight into the table.
                let case = |left: &str, right: &str| {
                    (
                        declared.clone(),
                        row[3].clone().unwrap_or_else(|| "\\N".into()),
                        left.to_string(),
                        right.to_string(),
                    )
                };
                let output =
                    |literal: &str| outputs.get(&(declared.clone(), literal.to_string())).cloned();
                for (offset, op) in ASSERTED {
                    if op.is_ordering() && !ordered {
                        continue;
                    }
                    let cell = row[4 + offset].as_deref().expect("a cell is never NULL");
                    // A cell recording what the server *refused* says
                    // nothing about how it compares.
                    if cell.starts_with('E') {
                        continue;
                    }
                    // A NULL *right* operand has no spelling in the
                    // filter grammar at all — `IS NULL` is how a filter
                    // asks for one — so there is no question to put to
                    // the register.
                    let Some(right) = row[2].as_deref() else { continue };
                    let bound = output(right).expect("an accepted literal has an output");
                    // A NULL *left* operand is the field, and this
                    // build answers `Unknown` for every comparing
                    // operator — the same value the server's `u` records,
                    // asserted as itself rather than as the "excluded"
                    // it collapses to at the root.
                    let Some(left) = row[1].as_deref() else {
                        assert_eq!(cell, "u", "{major} {declared}: a NULL operand");
                        assert_eq!(
                            answer(&declared, collation, &types, op, None, &bound).0,
                            Ok(Truth::Unknown),
                            "{major} {declared}: a NULL field is unknown"
                        );
                        continue;
                    };
                    let field = output(left).expect("an accepted literal has an output");
                    asserted += 1;
                    // `announced` is whether a disagreement is *permitted*
                    // here — the term announces a divergence that reaches
                    // this operator. It is what an exception is checked
                    // against and what the cell count is taken over, so an
                    // entry is never asked to be met under an operator its
                    // divergence does not claim.
                    let (got, announced) =
                        answer(&declared, collation, &types, op, Some(&field), &bound);
                    if announced {
                        *walked.entry(case(left, right)).or_default() += 1;
                    }
                    let expected = match cell {
                        "t" => Truth::True,
                        "f" => Truth::False,
                        other => panic!("{major} {declared}: unexpected cell {other:?}"),
                    };
                    if got != Ok(expected) {
                        // An exception is only ever claimable where the
                        // register has already told the user its answer
                        // may differ *under this operator*.
                        assert!(
                            announced,
                            "{major} {declared} {}: disagrees under {} while announcing no \
                                 divergence that reaches it",
                            row[1].as_deref().unwrap_or("\\N"),
                            op.symbol()
                        );
                        disagreed
                            .entry(case(left, right))
                            .or_default()
                            .insert(format!("{major} {} {got:?}", op.symbol()));
                    }
                }
            }
        }

        let expected_exceptions: BTreeSet<_> = EXCEPTIONS
            .iter()
            .map(|(t, c, l, r)| (t.to_string(), c.to_string(), l.to_string(), r.to_string()))
            .collect();
        let found: BTreeSet<_> = disagreed.keys().cloned().collect();
        let unexpected: Vec<_> = found.difference(&expected_exceptions).collect();
        let stale: Vec<_> = expected_exceptions.difference(&found).collect();
        assert!(
            unexpected.is_empty(),
            "{} cases disagree with the server and are not in the exception set:\n{}",
            unexpected.len(),
            unexpected
                .iter()
                .map(|k| format!("  {k:?} {:?}", disagreed[*k]))
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert!(stale.is_empty(), "exceptions that no longer disagree: {stale:?}");
        // An entry is keyed by the case, so "met" has to mean met over
        // every cell the case has: a collation that moves for one major,
        // or a comparator that stops being antisymmetric under one
        // operator, would otherwise leave the entry satisfied by the
        // majors and operators it still disagrees under.
        let partial: Vec<_> = expected_exceptions
            .iter()
            .filter(|case| disagreed[*case].len() != walked[*case])
            .map(|case| format!("  {case:?} {:?} of {} cells", disagreed[case], walked[case]))
            .collect();
        assert!(
            partial.is_empty(),
            "{} exceptions disagree in only part of the walk:\n{}",
            partial.len(),
            partial.join("\n")
        );
        assert_eq!(
            refused,
            REFUSED.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
            "the set of declared types the register refuses an ordering operator on"
        );
        // A floor, not a count: the walk skips a cell for four good
        // reasons, and a bug in any of them would leave it asserting
        // almost nothing while passing.
        assert!(asserted > 45_000, "only {asserted} cells asserted");
    }

    /// **The key a filter compares by is a total order over every value
    /// the server wrote**, per declared type the register compares as a
    /// scalar: reflexive, antisymmetric and transitive over the output
    /// spelling of every literal it accepted. A single comparison needs
    /// none of that, and a running minimum, maximum or sortedness flag
    /// needs all of it — two values that never meet in a filter still
    /// have to agree with a third about where they sit.
    ///
    /// Whether that order is *the server's* is the cell walk above; this
    /// is the property that walk cannot see, since it asks each pair
    /// once against a literal.
    #[tokio::test]
    async fn the_key_is_a_total_order_over_every_committed_value() {
        let mut kinds = 0usize;
        let mut triples = 0usize;
        for major in MAJORS {
            let types = types_of(major).await;
            let mut outputs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for row in rows(&fixture(major, "oracle/literals.tsv")) {
                if let (Some(declared), Some("ok"), Some(output)) =
                    (row[0].clone(), row[2].as_deref(), row[3].clone())
                {
                    outputs.entry(declared).or_default().insert(output);
                }
            }
            for (declared, values) in outputs {
                let ComparisonPlan::Compared { kind, .. } =
                    comparison_for(&declared, None, &types, &[])
                else {
                    continue;
                };
                kinds += 1;
                let keys: Vec<(&String, OrderKey)> = values
                    .iter()
                    .map(|v| {
                        let key = order_key(&kind, v).unwrap_or_else(|| {
                            panic!("{major} {declared}: the server wrote {v:?}")
                        });
                        (v, key)
                    })
                    .collect();
                for (a, ka) in &keys {
                    assert_eq!(compare_keys(ka, ka), Ordering::Equal, "{major} {declared} {a}");
                    for (b, kb) in &keys {
                        let ab = compare_keys(ka, kb);
                        assert_eq!(
                            ab,
                            compare_keys(kb, ka).reverse(),
                            "{major} {declared}: {a:?} against {b:?}"
                        );
                        for (c, kc) in &keys {
                            triples += 1;
                            let bc = compare_keys(kb, kc);
                            if ab != Ordering::Greater && bc != Ordering::Greater {
                                let ac = compare_keys(ka, kc);
                                assert_ne!(
                                    ac,
                                    Ordering::Greater,
                                    "{major} {declared}: {a:?} <= {b:?} <= {c:?}"
                                );
                                if ab == Ordering::Equal && bc == Ordering::Equal {
                                    assert_eq!(ac, Ordering::Equal);
                                }
                            }
                        }
                    }
                }
            }
        }
        // Floors, not counts: the scalar kinds of six majors.
        assert!(kinds > 150, "only {kinds} declared types walked");
        assert!(triples > 50_000, "only {triples} triples asserted");
    }

    /// The persisted format version and the ordering digest it was pinned
    /// beside, re-pinned together (`golden_order_is_pinned_to_the_format_version`).
    const GOLDEN_ORDER: (u32, u64) = (68, 18_361_034_400_210_632_537);

    /// **Every committed oracle value, sorted under its declared type's
    /// comparison kind and under each kind a set of its bounds is stored
    /// by, digests to the value pinned beside the cache's
    /// `CACHE_FORMAT_VERSION`.** A stored bound, sortedness or dictionary means
    /// what this build's comparison says, so a change to how any kind
    /// orders or equates values is a persisted reshape that bumps the
    /// version (`docs/design/decisions.md`, "D22"); this fails until it
    /// is bumped and the digest re-pinned. A regenerated oracle moves the
    /// digest with no change to any comparison, and is re-pinned alone.
    ///
    /// The digest is FNV-1a over each major, each declared type and its
    /// values in stored order ([`ValueKey::stored_order`]), ties broken by
    /// text, with whether each value keys equal to the one before it, and
    /// whether only the stored order tells the two apart — so equality
    /// moves it as well as order.
    #[tokio::test]
    async fn golden_order_is_pinned_to_the_format_version() {
        fn fnv(hash: &mut u64, bytes: &[u8]) {
            for byte in bytes {
                *hash ^= u64::from(*byte);
                *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        let mut digest = 0xcbf2_9ce4_8422_2325u64;
        let mut values_sorted = 0usize;
        for major in MAJORS {
            let types = types_of(major).await;
            let mut outputs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for row in rows(&fixture(major, "oracle/literals.tsv")) {
                if let (Some(declared), Some("ok"), Some(output)) =
                    (row[0].clone(), row[2].as_deref(), row[3].clone())
                {
                    outputs.entry(declared).or_default().insert(output);
                }
            }
            for (declared, values) in outputs {
                let plan = comparison_for(&declared, None, &types, &[]);
                let ComparisonPlan::Compared { kind, .. } = &plan else {
                    continue;
                };
                // The register's kind, and each kind a set of bounds is
                // stored under.
                let mut kinds = vec![kind.clone()];
                for stored in plan.bounds_kinds().into_iter().flatten() {
                    if !kinds.contains(&stored) {
                        kinds.push(stored);
                    }
                }
                for kind in kinds {
                    let mut keyed: Vec<(ValueKey, &String)> = values
                        .iter()
                        .map(|v| (ValueKey::of(&kind, v).expect("the server wrote it"), v))
                        .collect();
                    keyed.sort_by(|(ka, a), (kb, b)| ka.stored_order(kb).then_with(|| a.cmp(b)));
                    let name = kind_name(&kind);
                    fnv(&mut digest, format!("{major}\t{declared}\t{name}\n").as_bytes());
                    let mut previous: Option<&ValueKey> = None;
                    for (key, value) in &keyed {
                        let tie = previous.map(|p| (p.compare(key), p.stored_order(key)));
                        fnv(
                            &mut digest,
                            match tie {
                                Some((Ordering::Equal, Ordering::Equal)) => b"=",
                                Some((Ordering::Equal, _)) => b"~",
                                _ => b"<",
                            },
                        );
                        fnv(&mut digest, value.as_bytes());
                        previous = Some(key);
                        values_sorted += 1;
                    }
                }
            }
        }
        assert!(values_sorted > 800, "only {values_sorted} values sorted");
        assert_eq!(
            (crate::cache::CACHE_FORMAT_VERSION, digest),
            GOLDEN_ORDER,
            "the comparison order moved, or CACHE_FORMAT_VERSION did: bump CACHE_FORMAT_VERSION if any \
                 kind orders or equates differently, then re-pin both here"
        );
    }

    /// Whether each [`CompareKind`] orders the values its column emits as
    /// DataFusion orders the emitted array ([`arrow_against`]), and whether
    /// the bounds gathering stores under it are that order's minimum and
    /// maximum of the group, as `(kind, order agrees, bounds agree)`. Asserted as the exact table,
    /// every kind present, so a kind moving either way fails here.
    ///
    /// Every kind whose key is the decoded Arrow value agrees by
    /// construction — a float's too, its `-0` made `0` before the
    /// kernels see it; `MacAddr` agrees because `macaddr_out` writes
    /// fixed-width lowercase hex, whose bytes order as the octets do, and
    /// is compared as text in DataFusion semantics all the same, a user's
    /// literal not being bound to that spelling. The seven that do not,
    /// each with the pair the walk first meets:
    ///
    /// - `Enum`, emitted `Dictionary`: Arrow compares the label text, not
    ///   the declaration order (I33).
    /// - `Interval`, emitted `Interval(MonthDayNano)`: Arrow compares
    ///   months, days and nanoseconds field by field, so `30 days` is
    ///   below `1 mon` where `interval_cmp_value` equates them (I40).
    /// - `Numeric`, `TimeTz`, `Network`, `Jsonb`, emitted `Utf8View`: Arrow
    ///   compares the text bytewise.
    /// - `PaddedText`, emitted `Utf8View` with its padding: Arrow compares
    ///   the padded text, so a byte below the blank orders differently —
    ///   and a stored bound, being unpadded, is no value of the column.
    const ARROW_AGREEMENT: &[(&str, bool, bool)] = &[
        ("Bool", true, true),
        ("Bytea", true, true),
        ("Date", true, true),
        ("Decimal", true, true),
        ("Enum", false, false),
        ("Float32", true, true),
        ("Float64", true, true),
        ("Int", true, true),
        ("Interval", false, false),
        ("Jsonb", false, false),
        ("MacAddr", true, true),
        ("Network", false, false),
        ("Numeric", false, false),
        ("PaddedText", false, false),
        ("Text", true, true),
        ("Time", true, true),
        ("TimeTz", false, false),
        ("Timestamp", true, true),
        ("UnsignedInt", true, true),
        ("Uuid", true, true),
    ];

    /// Values added to the oracle's for a kind whose committed population
    /// happens to order alike under both comparisons, each in the form its
    /// type's `*_out` writes: every IPv4 `inet`/`cidr` case there has a
    /// first octet of two digits or more, so none reaches where a
    /// network's order and its text's part — `9.` against `10.`.
    const SUPPLEMENT: &[(&str, &str)] = &[("inet", "9.0.0.1"), ("cidr", "9.0.0.0/8")];

    /// A stable name per [`CompareKind`] variant, exhaustive so a new kind
    /// cannot go unrecorded in [`ARROW_AGREEMENT`]. The one only
    /// [`CompareKind::datafusion_order`] produces is named and never walked.
    fn kind_name(kind: &CompareKind) -> &'static str {
        match kind {
            CompareKind::Bool => "Bool",
            CompareKind::Int { .. } => "Int",
            CompareKind::UnsignedInt => "UnsignedInt",
            CompareKind::Float32 => "Float32",
            CompareKind::Float64 => "Float64",
            CompareKind::Decimal { .. } => "Decimal",
            CompareKind::Date => "Date",
            CompareKind::Time => "Time",
            CompareKind::Timestamp { .. } => "Timestamp",
            CompareKind::Uuid => "Uuid",
            CompareKind::Bytea => "Bytea",
            CompareKind::Text { .. } => "Text",
            CompareKind::PaddedText { .. } => "PaddedText",
            CompareKind::Numeric { .. } => "Numeric",
            CompareKind::Enum { .. } => "Enum",
            CompareKind::Interval { .. } => "Interval",
            CompareKind::TimeTz => "TimeTz",
            CompareKind::Network { .. } => "Network",
            CompareKind::MacAddr { .. } => "MacAddr",
            CompareKind::Jsonb => "Jsonb",
            CompareKind::IntervalFields { .. } => "IntervalFields",
        }
    }

    /// DataFusion's order of every element of `array` against element
    /// `j`, read off the `lt`, `eq` and `gt` kernels, or why it has none: a
    /// kernel refusing the type, or an element answering other than
    /// exactly one of the three.
    ///
    /// DataFusion's `apply_cmp` makes a float's `-0` into `0` before the
    /// kernels run (`normalize_float_zero`, `datafusion-common` 55), so
    /// this does too; the kernels alone order `-0` below `0`. The check
    /// against DataFusion itself is the provider crate's, the library
    /// taking no DataFusion dependency.
    fn arrow_against(
        array: &arrow::array::ArrayRef,
        j: usize,
    ) -> std::result::Result<Vec<Ordering>, String> {
        use arrow::array::{Array, AsArray, Scalar};
        use arrow::compute::kernels::cmp::{eq, gt, lt};
        use arrow::datatypes::{DataType, Float32Type, Float64Type};
        let array: arrow::array::ArrayRef = match array.data_type() {
            DataType::Float32 => std::sync::Arc::new(
                array.as_primitive::<Float32Type>().unary::<_, Float32Type>(|v| v + 0.0),
            ),
            DataType::Float64 => std::sync::Arc::new(
                array.as_primitive::<Float64Type>().unary::<_, Float64Type>(|v| v + 0.0),
            ),
            _ => array.clone(),
        };
        let array = &array;
        let scalar = Scalar::new(array.slice(j, 1));
        let kernel = |r: std::result::Result<arrow::array::BooleanArray, _>| {
            r.map_err(|e: arrow::error::ArrowError| e.to_string())
        };
        let (lt, eq, gt) =
            (kernel(lt(array, &scalar))?, kernel(eq(array, &scalar))?, kernel(gt(array, &scalar))?);
        (0..array.len())
            .map(|i| match (lt.value(i), eq.value(i), gt.value(i)) {
                (true, false, false) => Ok(Ordering::Less),
                (false, true, false) => Ok(Ordering::Equal),
                (false, false, true) => Ok(Ordering::Greater),
                other => Err(format!("element {i} against {j} answers lt/eq/gt {other:?}")),
            })
            .collect()
    }

    /// Whether `stored` are Arrow's minimum and maximum of `group`, over
    /// `array`, the group's values followed by the two bounds: each bound
    /// at or below (above) every value and equal to one of them — the
    /// upper one only where it is exact.
    fn arrow_extremes(
        array: &arrow::array::ArrayRef,
        group: &[&str],
        stored: &crate::statistics::Bounds,
    ) -> std::result::Result<(), String> {
        let n = group.len();
        let (min, max) = (arrow_against(array, n)?, arrow_against(array, n + 1)?);
        let below = min[..n].iter().all(|o| o.is_ge());
        let above = max[..n].iter().all(|o| o.is_le());
        let min_held = min[..n].contains(&Ordering::Equal);
        let max_held = max[..n].contains(&Ordering::Equal);
        if below && above && min_held && (max_held || !stored.max_exact) {
            Ok(())
        } else {
            Err(format!(
                "{:?} and {:?} over {group:?}: below {below}, above {above}, min held \
                     {min_held}, max held {max_held}",
                stored.min, stored.max
            ))
        }
    }

    /// One kind's findings over every declared type compared by it.
    #[derive(Default)]
    struct Agreement {
        declared: BTreeSet<String>,
        arrow_types: BTreeSet<String>,
        pairs: usize,
        groups: usize,
        /// The first pair the two orders answer differently, or the
        /// first reason Arrow gave none.
        order: Option<String>,
        /// The first group whose stored bounds are not Arrow's extremes.
        bounds: Option<String>,
    }

    /// **Which kinds already order as DataFusion orders the emitted
    /// array**, over every value the server wrote for every declared type
    /// the register compares as a scalar, on six majors: each pair's
    /// [`ValueKey`] order against [`arrow_against`]'s answer on the array
    /// a batch builds from the same texts, and each gathered group's
    /// stored bounds against the group's Arrow minimum and maximum.
    ///
    /// A value the emitted type cannot hold — a `date`'s `infinity`, a
    /// `numeric(p,s)`'s `NaN` (D96) — is left out: no array carries it
    /// for Arrow to order.
    #[tokio::test]
    async fn each_kind_s_order_and_bounds_against_arrow_s() {
        let mut rng = Seeded(0x5EED_0006_0001);
        let mut found: BTreeMap<&'static str, Agreement> = BTreeMap::new();
        for major in MAJORS {
            let types = types_of(major).await;
            let mut outputs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for row in rows(&fixture(major, "oracle/literals.tsv")) {
                if let (Some(declared), Some("ok"), Some(output)) =
                    (row[0].clone(), row[2].as_deref(), row[3].clone())
                {
                    outputs.entry(declared).or_default().insert(output);
                }
            }
            for (declared, value) in SUPPLEMENT {
                outputs.entry(declared.to_string()).or_default().insert(value.to_string());
            }
            for (declared, values) in outputs {
                let resolved = schema(&declared, None, &types);
                let ComparisonPlan::Compared { kind, .. } = &resolved.comparisons[0] else {
                    continue;
                };
                let data_type = resolved.schema.field(0).data_type();
                let plan = &resolved.plans[0];
                let held: Vec<&str> = values
                    .iter()
                    .map(String::as_str)
                    .filter(|v| crate::batch::column_of(data_type, plan, &[v]).is_ok())
                    .collect();
                if held.is_empty() {
                    continue;
                }
                let agreement = found.entry(kind_name(kind)).or_default();
                agreement.declared.insert(declared.clone());
                agreement.arrow_types.insert(format!("{data_type}"));
                let key = |v: &str| {
                    ValueKey::of(kind, v)
                        .unwrap_or_else(|| panic!("{major} {declared}: the server wrote {v:?}"))
                };
                let array = crate::batch::column_of(data_type, plan, &held).unwrap();
                for (j, b) in held.iter().enumerate() {
                    let arrow = match arrow_against(&array, j) {
                        Ok(arrow) => arrow,
                        Err(e) => {
                            agreement.order.get_or_insert(format!("{major} {declared}: {e}"));
                            break;
                        }
                    };
                    for (i, a) in held.iter().enumerate() {
                        agreement.pairs += 1;
                        let ours = key(a).compare(&key(b));
                        if ours != arrow[i] {
                            agreement.order.get_or_insert(format!(
                                "{major} {declared}: {a:?} against {b:?} is {ours:?} here and \
                                     {:?} in Arrow",
                                arrow[i]
                            ));
                        }
                    }
                }
                // The whole set as one group, and seeded small ones.
                let mut groups = vec![held.clone()];
                groups.extend(
                    (0..30).map(|_| (0..1 + rng.below(5)).map(|_| *rng.pick(&held)).collect()),
                );
                for group in groups {
                    let Some(stored) = crate::gather::one_group_bounds(kind.clone(), &group) else {
                        continue;
                    };
                    agreement.groups += 1;
                    let mut with_bounds = group.clone();
                    with_bounds.extend([stored.min.as_str(), stored.max.as_str()]);
                    let array = match crate::batch::column_of(data_type, plan, &with_bounds) {
                        Ok(array) => array,
                        Err(v) => {
                            agreement.bounds.get_or_insert(format!(
                                "{major} {declared}: stored bound {v:?} is not a value of \
                                     {data_type}"
                            ));
                            continue;
                        }
                    };
                    let problem = arrow_extremes(&array, &group, &stored);
                    if let Err(e) = problem {
                        agreement.bounds.get_or_insert(format!("{major} {declared}: {e}"));
                    }
                }
            }
        }
        let table: Vec<(&str, bool, bool)> =
            found.iter().map(|(name, a)| (*name, a.order.is_none(), a.bounds.is_none())).collect();
        let report: Vec<String> = found
                .iter()
                .map(|(name, a)| {
                    format!(
                        "  {name}: {} types as {:?}, {} pairs, {} groups\n    order: {}\n    bounds: {}",
                        a.declared.len(),
                        a.arrow_types,
                        a.pairs,
                        a.groups,
                        a.order.as_deref().unwrap_or("agrees"),
                        a.bounds.as_deref().unwrap_or("agree"),
                    )
                })
                .collect();
        assert_eq!(table, ARROW_AGREEMENT, "\n{}", report.join("\n"));
    }

    /// **Under [`ComparisonSemantics::DataFusion`] every comparing operator
    /// answers as DataFusion does** ([`arrow_against`]) over the array a
    /// batch builds, over every value the server wrote for every declared
    /// type the register compares as a scalar, on six majors, each value
    /// in turn the literal — and announces nothing. A nested column refuses
    /// every comparing operator instead.
    ///
    /// It also holds the rule for which stored bounds DataFusion's semantics
    /// reads ([`ComparisonPlan::bounds_ordered_in`]) to the evidence
    /// [`ARROW_AGREEMENT`] records: a kind [`CompareKind::datafusion_order`]
    /// leaves alone is one whose order and gathered bounds were found
    /// to be DataFusion's. And **every kind emitted as text —
    /// `Utf8View` or a `Dictionary` of `Utf8` — is [`CompareKind::Text { length: None }`]
    /// there**: the walk's literals are the server's own spellings, so
    /// agreeing over them cannot show that a user's literal is compared
    /// as DataFusion compares it, and only bytewise is.
    ///
    /// **The set of bounds DataFusion's semantics reads is DataFusion's minimum
    /// and maximum**, gathered under its kind over every value and over
    /// runs of three, on six majors; and an undivergent column reads its one set there
    /// exactly where [`ARROW_AGREEMENT`] records that set agreeing —
    /// `macaddr` among them — and a second set everywhere else.
    #[tokio::test]
    async fn datafusion_semantics_answers_as_arrow_s_kernels_do() {
        const OPS: [PredicateOp; 8] = [
            PredicateOp::Eq,
            PredicateOp::Ne,
            PredicateOp::Lt,
            PredicateOp::Le,
            PredicateOp::Gt,
            PredicateOp::Ge,
            PredicateOp::IsDistinctFrom,
            PredicateOp::IsNotDistinctFrom,
        ];
        let arrow_term = |p: &Predicate, resolved: &ResolvedSchema| {
            crate::predicate::resolve_term(p, 0, resolved, 0, ComparisonSemantics::DataFusion)
        };
        let mut kinds: BTreeMap<&'static str, CompareKind> = BTreeMap::new();
        let (mut answered, mut nested_refused, mut text_emitted) = (0usize, 0usize, 0usize);
        let (mut datafusion_bounds_checked, mut primary_in_both) = (0usize, 0usize);
        for major in MAJORS {
            let types = types_of(major).await;
            let mut outputs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for row in rows(&fixture(major, "oracle/literals.tsv")) {
                if let (Some(declared), Some("ok"), Some(output)) =
                    (row[0].clone(), row[2].as_deref(), row[3].clone())
                {
                    outputs.entry(declared).or_default().insert(output);
                }
            }
            for (declared, value) in SUPPLEMENT {
                outputs.entry(declared.to_string()).or_default().insert(value.to_string());
            }
            for (declared, values) in outputs {
                let resolved = schema(&declared, None, &types);
                let kind = match &resolved.comparisons[0] {
                    ComparisonPlan::Compared { kind, .. } => kind.clone(),
                    ComparisonPlan::Nested(_) => {
                        let literal = values.first().cloned();
                        for op in OPS {
                            let p = Predicate { column: "v".into(), op, value: literal.clone() };
                            assert!(
                                matches!(
                                    arrow_term(&p, &resolved),
                                    Err(Error::UncomparablePredicateColumn { .. })
                                ),
                                "{major} {declared} {op:?}"
                            );
                            nested_refused += 1;
                        }
                        continue;
                    }
                    _ => continue,
                };
                let data_type = resolved.schema.field(0).data_type();
                let emitted_as_text = match data_type {
                    DataType::Utf8View => true,
                    DataType::Dictionary(_, value) => value.as_ref() == &DataType::Utf8,
                    _ => false,
                };
                assert!(
                    !emitted_as_text || matches!(kind.datafusion_order(), CompareKind::Text { .. }),
                    "{major} {declared}: emitted as text, compared as {:?}",
                    kind.datafusion_order()
                );
                text_emitted += usize::from(emitted_as_text);
                let plan = &resolved.plans[0];
                let held: Vec<&str> = values
                    .iter()
                    .map(String::as_str)
                    .filter(|v| crate::batch::column_of(data_type, plan, &[v]).is_ok())
                    .collect();
                if held.is_empty() {
                    continue;
                }
                kinds.insert(kind_name(&kind), kind.clone());
                let set = resolved.comparisons[0].bounds_in(ComparisonSemantics::DataFusion);
                if matches!(
                    resolved.comparisons[0],
                    ComparisonPlan::Compared { divergence: None, .. }
                ) {
                    let agrees = ARROW_AGREEMENT
                        .iter()
                        .any(|(name, _, bounds)| *name == kind_name(&kind) && *bounds);
                    let one = set == Some(BoundsSet::Primary);
                    assert_eq!(one, agrees, "{major} {declared}: one set read in both");
                    primary_in_both += usize::from(one);
                }
                let set = set.unwrap_or_else(|| panic!("{major} {declared}: no DataFusion bounds"));
                let stored_kind = resolved.bounds_kinds(0)[match set {
                    BoundsSet::Primary => 0,
                    BoundsSet::DataFusion => 1,
                }]
                .clone()
                .expect("a set read is gathered");
                let groups = std::iter::once(held.clone()).chain(held.chunks(3).map(<[_]>::to_vec));
                for group in groups {
                    let Some(stored) = crate::gather::one_group_bounds(stored_kind.clone(), &group)
                    else {
                        continue;
                    };
                    let mut with_bounds = group.clone();
                    with_bounds.extend([stored.min.as_str(), stored.max.as_str()]);
                    let array = crate::batch::column_of(data_type, plan, &with_bounds)
                        .unwrap_or_else(|v| panic!("{major} {declared}: bound {v:?}"));
                    arrow_extremes(&array, &group, &stored)
                        .unwrap_or_else(|e| panic!("{major} {declared}: {e}"));
                    datafusion_bounds_checked += 1;
                }
                let array = crate::batch::column_of(data_type, plan, &held).unwrap();
                for (j, b) in held.iter().enumerate() {
                    let arrow = arrow_against(&array, j)
                        .unwrap_or_else(|e| panic!("{major} {declared}: {e}"));
                    for op in OPS {
                        let p = Predicate { column: "v".into(), op, value: Some(b.to_string()) };
                        let term = arrow_term(&p, &resolved)
                            .unwrap_or_else(|e| panic!("{major} {declared} {op:?} {b:?}: {e}"));
                        assert!(term.comparison_notes().is_empty(), "{major} {declared}");
                        for (i, a) in held.iter().enumerate() {
                            let ord = arrow[i];
                            let want = match op {
                                PredicateOp::Eq | PredicateOp::IsNotDistinctFrom => ord.is_eq(),
                                PredicateOp::Ne | PredicateOp::IsDistinctFrom => ord.is_ne(),
                                PredicateOp::Lt => ord.is_lt(),
                                PredicateOp::Le => ord.is_le(),
                                PredicateOp::Gt => ord.is_gt(),
                                _ => ord.is_ge(),
                            };
                            assert_eq!(
                                term.eval_value(Some(a)),
                                Some(Truth::of(want)),
                                "{major} {declared}: {a:?} {} {b:?}",
                                op.symbol()
                            );
                            answered += 1;
                        }
                    }
                }
            }
        }
        let walked: Vec<&str> = kinds.keys().copied().collect();
        let recorded: Vec<&str> = ARROW_AGREEMENT.iter().map(|(name, ..)| *name).collect();
        assert_eq!(walked, recorded, "every kind the register compares is walked");
        for (name, order, bounds) in ARROW_AGREEMENT {
            assert_eq!(
                kinds[name].datafusion_divergence().is_none(),
                kinds[name].datafusion_order() == kinds[name],
                "{name}: a kind DataFusion semantics moves reports a divergence, and only one"
            );
            if kinds[name].datafusion_order() == kinds[name] {
                assert!(*order && *bounds, "{name} keeps its kind and disagrees with Arrow");
            }
        }
        assert!(
            answered > 40_000 && nested_refused > 0 && text_emitted > 0,
            "{answered} {nested_refused} {text_emitted}"
        );
        assert!(
            datafusion_bounds_checked > 400 && primary_in_both > 0,
            "{datafusion_bounds_checked} {primary_in_both}"
        );
    }

    /// Every `(declared type, COLLATE clause)` case of every major's
    /// `comparisons.tsv`, resolved as a one-column schema, with the output
    /// spelling of every literal the server accepted for its type.
    async fn cases(major: u32) -> Vec<(String, Option<&'static str>, ResolvedSchema)> {
        let types = types_of(major).await;
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for row in rows(&fixture(major, "oracle/comparisons.tsv")) {
            let declared = row[0].clone().expect("a case names a type");
            let collation = clause(row[3].as_ref());
            if seen.insert((declared.clone(), collation)) {
                let resolved = schema(&declared, collation, &types);
                out.push((declared, collation, resolved));
            }
        }
        out
    }

    /// **Under PostgreSQL's semantics a column reports exactly what a term
    /// on it would announce**, under `=` and under `<` — the two operator
    /// families — over every case the oracle holds on six majors. The
    /// report is written apart from [`resolve_term`], so this is what
    /// holds the two to one answer.
    #[tokio::test]
    async fn a_column_reports_what_its_terms_announce() {
        let mut compared = 0usize;
        for major in MAJORS {
            let mut outputs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for row in rows(&fixture(major, "oracle/literals.tsv")) {
                if let (Some(declared), Some("ok"), Some(output)) =
                    (row[0].clone(), row[2].as_deref(), row[3].clone())
                {
                    outputs.entry(declared).or_default().insert(output);
                }
            }
            for (declared, collation, resolved) in cases(major).await {
                // The first literal both families decode: a refusal of
                // the operator is an answer, a literal that does not key
                // is not.
                let announced = outputs.get(&declared).into_iter().flatten().find_map(|v| {
                    let mut notes = Vec::new();
                    for op in [PredicateOp::Eq, PredicateOp::Lt] {
                        let p = Predicate { column: "v".into(), op, value: Some(v.clone()) };
                        match resolve_term(&p, 0, &resolved, 0) {
                            Ok(term) => notes.extend(term.comparison_notes()),
                            Err(Error::PredicateValueDecode { .. }) => return None,
                            Err(_) => {}
                        }
                    }
                    Some(notes)
                });
                let Some(announced) = announced else { continue };
                let reported = column_divergences(&resolved, ComparisonSemantics::Postgres);
                let context = format!("{major} {declared} {collation:?}");
                assert!(
                    announced.iter().all(|n| reported.contains(n)),
                    "{context}: {announced:?} not all in {reported:?}"
                );
                assert!(
                    reported.iter().all(|n| announced.contains(n)),
                    "{context}: {reported:?} not all announced ({announced:?})"
                );
                compared += 1;
            }
        }
        assert!(compared > 300, "only {compared} cases compared");
    }

    /// **Under DataFusion's semantics a column that reports nothing answers as
    /// the server does**: every oracle cell a term in DataFusion's semantics
    /// answers differently from PostgreSQL falls on a column reporting a
    /// divergence that reaches the cell's operator, or on one that fell
    /// back to text, whose column note is its finding. A nested column,
    /// whose every term is refused here, is answered as DataFusion
    /// answers it — `make_comparator` over the arrays a batch builds —
    /// and asserted to report its order, so its equality cells hold the
    /// order note to order alone. The other direction is not asserted — a
    /// report is allowed to announce a divergence this population never
    /// exercises.
    #[tokio::test]
    async fn a_column_reporting_no_datafusion_divergence_answers_as_the_server() {
        let (mut asserted, mut disagreed) = (0usize, BTreeSet::new());
        let mut nested_asserted = 0usize;
        for major in MAJORS {
            let outputs: BTreeMap<(String, String), String> =
                rows(&fixture(major, "oracle/literals.tsv"))
                    .into_iter()
                    .filter_map(|r| Some(((r[0].clone()?, r[1].clone()?), r[3].clone()?)))
                    .collect();
            let resolved: BTreeMap<(String, Option<&str>), ResolvedSchema> = cases(major)
                .await
                .into_iter()
                .map(|(declared, collation, resolved)| ((declared, collation), resolved))
                .collect();
            for row in rows(&fixture(major, "oracle/comparisons.tsv")) {
                let declared = row[0].clone().expect("a case names a type");
                let collation = clause(row[3].as_ref());
                let schema = &resolved[&(declared.clone(), collation)];
                let report = column_divergences(schema, ComparisonSemantics::DataFusion);
                if matches!(schema.comparisons[0], ComparisonPlan::Nested(_)) {
                    assert!(
                        report.iter().any(|n| n.divergence == ComparisonDivergence::NestedOrder),
                        "{major} {declared}: {report:?}"
                    );
                }
                let output = |literal: &str| outputs.get(&(declared.clone(), literal.to_string()));
                let (Some(left), Some(right)) = (row[1].as_deref(), row[2].as_deref()) else {
                    continue;
                };
                let (Some(field), Some(bound)) = (output(left), output(right)) else {
                    continue;
                };
                // A nested column's every term is refused here, so its
                // answer is DataFusion's own: `make_comparator` over the
                // two values as a batch emits them.
                let nested = (!schema.plans[0].is_scalar()
                    && schema.columns[0] == ColumnResolution::Mapped)
                    .then(|| {
                        let array = crate::batch::column_of(
                            schema.schema.field(0).data_type(),
                            &schema.plans[0],
                            &[field, bound],
                        )
                        .ok()?;
                        let cmp = arrow::array::make_comparator(&array, &array, Default::default())
                            .ok()?;
                        Some(cmp(0, 1))
                    });
                for (offset, op) in ASSERTED {
                    let cell = row[4 + offset].as_deref().expect("a cell is never NULL");
                    let expected = match cell {
                        "t" => Truth::True,
                        "f" => Truth::False,
                        _ => continue,
                    };
                    let got = match nested {
                        Some(None) => continue,
                        Some(Some(ord)) => {
                            nested_asserted += 1;
                            Truth::of(match op {
                                PredicateOp::Eq => ord.is_eq(),
                                PredicateOp::Ne => ord.is_ne(),
                                PredicateOp::Lt => ord.is_lt(),
                                PredicateOp::Le => ord.is_le(),
                                PredicateOp::Gt => ord.is_gt(),
                                _ => ord.is_ge(),
                            })
                        }
                        None => {
                            let p =
                                Predicate { column: "v".into(), op, value: Some(bound.clone()) };
                            let Ok(term) = crate::predicate::resolve_term(
                                &p,
                                0,
                                schema,
                                0,
                                ComparisonSemantics::DataFusion,
                            ) else {
                                continue;
                            };
                            let Ok(got) = term.eval(
                                RawRow::unchecked(&encode_field(Some(field))),
                                &mut RowSplit::default(),
                                "public.t",
                                0,
                            ) else {
                                continue;
                            };
                            got
                        }
                    };
                    asserted += 1;
                    if got != expected {
                        // A column that fell back to text is reported by
                        // its `Warning` column note, and nothing else.
                        let fell_back = schema.columns[0] != ColumnResolution::Mapped;
                        assert!(
                            fell_back
                                || report.iter().any(|n| {
                                    op.is_ordering() || n.divergence.affects_equality()
                                }),
                            "{major} {declared} {collation:?}: {left:?} {} {right:?} is \
                                 {got:?} in DataFusion semantics and {cell} on the server, and the \
                                 column reports {report:?}",
                            op.symbol()
                        );
                        disagreed.insert(declared.clone());
                    }
                }
            }
        }
        // A floor on both counts: a walk that asserted nothing, or found
        // no disagreement at all, would prove nothing about the report.
        assert!(asserted > 40_000, "only {asserted} cells asserted");
        assert!(nested_asserted > 1_000, "only {nested_asserted} nested cells asserted");
        assert!(disagreed.len() >= 8, "only {disagreed:?} disagreed");
    }

    /// A range or multirange literal is put into the form the server
    /// stores it in *before* it is compared, checked against the server's
    /// own rewriting of every such literal in `literals.tsv`.
    ///
    /// **The cell walk above cannot reach this**, and that is why this
    /// test exists rather than one more assertion inside it: it puts both
    /// operands of every cell through `outputs` by construction, so both
    /// sides arrive already canonical and a build that rewrote nothing
    /// would pass. The input spellings are only in `literals.tsv` — and
    /// they are exactly the interesting ones, `int4range '[1,10]'` being
    /// the value the file holds as `[1,11)`.
    ///
    /// So each accepted row is asked `<output> = <input>`, which is
    /// `True` only where the two are one value here as they are on the
    /// server, and each refused row is asked for the refusal — which is
    /// where `[10,1)` lands, a `22000` the container grammar cannot see
    /// (I44) and [`make_range`] raises.
    #[tokio::test]
    async fn a_range_literal_is_canonicalized_before_it_is_compared() {
        let mut asked = 0usize;
        let mut rewritten = 0usize;
        for major in MAJORS {
            let types = types_of(major).await;
            for row in rows(&fixture(major, "oracle/literals.tsv")) {
                let declared = row[0].clone().expect("a case names a type");
                if !matches!(
                    comparison_for(&declared, None, &types, &[]),
                    ComparisonPlan::Nested(
                        NestedCompare::Range { .. } | NestedCompare::Multirange { .. }
                    )
                ) {
                    continue;
                }
                // A SQL NULL input is the `\N` field, not a literal.
                let Some(input) = row[1].clone() else { continue };
                let status = row[2].as_deref().expect("a case records its status");
                // The type does not exist at this major — a user range's
                // multirange companion before v14 (I10).
                if status == "E42704" {
                    continue;
                }
                asked += 1;
                let ask = |field: &str| {
                    answer(&declared, None, &types, PredicateOp::Eq, Some(field), &input).0
                };
                let Some(output) = row[3].as_deref().filter(|_| status == "ok") else {
                    // The field is never read: the literal is refused
                    // when the block's schema resolves, before a row.
                    assert!(
                        ask("empty").is_err(),
                        "{major} {declared}: accepted {input:?}, which the server refused \
                             with {status}"
                    );
                    continue;
                };
                assert_eq!(
                    ask(output),
                    Ok(Truth::True),
                    "{major} {declared}: {input:?} is not the value the server stored it as \
                         ({output:?})"
                );
                if input != output {
                    rewritten += 1;
                }
            }
        }
        // Floors, not counts, and the second is the one that matters: a
        // build that canonicalized nothing would still satisfy the first,
        // most rows being already written the way the server stores them.
        assert!(asked > 150, "only {asked} range literals asserted");
        assert!(rewritten > 30, "only {rewritten} of them needed rewriting");
    }

    /// SplitMix64, seeded, so a failing group is the same group on every
    /// run and on every machine.
    struct Seeded(u64);

    impl Seeded {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }

        fn chance(&mut self, one_in: usize) -> bool {
            self.below(one_in) == 0
        }

        fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
            &items[self.below(items.len())]
        }
    }

    const OPERATORS: [PredicateOp; 10] = [
        PredicateOp::Eq,
        PredicateOp::Ne,
        PredicateOp::IsNull,
        PredicateOp::IsNotNull,
        PredicateOp::Lt,
        PredicateOp::Le,
        PredicateOp::Gt,
        PredicateOp::Ge,
        PredicateOp::IsDistinctFrom,
        PredicateOp::IsNotDistinctFrom,
    ];

    /// A random tree of at most `depth` levels over `terms`, `And`/`Or`
    /// of arity zero to three.
    fn random_tree(rng: &mut Seeded, depth: usize, terms: &[ResolvedTerm]) -> ResolvedExpr {
        let arity = |rng: &mut Seeded| rng.below(4);
        match if depth == 0 { 0 } else { rng.below(4) } {
            0 => ResolvedExpr::Term(rng.pick(terms).clone()),
            1 => ResolvedExpr::And(
                (0..arity(rng)).map(|_| random_tree(rng, depth - 1, terms)).collect(),
            ),
            2 => ResolvedExpr::Or(
                (0..arity(rng)).map(|_| random_tree(rng, depth - 1, terms)).collect(),
            ),
            _ => ResolvedExpr::Not(Box::new(random_tree(rng, depth - 1, terms))),
        }
    }

    /// **The truth-set evaluator never rules out an answer a row gives**,
    /// over every value the server wrote for every declared type in the
    /// committed oracle, on six majors: seeded random row groups — NULLs,
    /// repeats, no rows at all — with their statistics taken off the rows
    /// and then weakened the ways a stored statistic can be (a bound
    /// loosened to another value on its side, a count or a dictionary
    /// missing), under random terms and random `And`/`Or`/`Not` trees.
    /// Every row's exact answer through the row evaluator has to be in
    /// the group's set.
    ///
    /// **And it rules out everything it can where the statistics are
    /// exact**, which is what keeps the first half from passing on an
    /// evaluator that answers every set: a single ordering term over
    /// unloosened bounds on a column that orders exactly, and a single
    /// equality term over a dictionary on a column that equates exactly,
    /// each have to answer precisely the set its rows produce.
    ///
    /// Bounds are offered to every column they can be keyed for, exact or
    /// not; which ones a term believes is the evaluator's to decide.
    #[tokio::test]
    async fn a_groups_truths_hold_every_rows_answer() {
        let mut rng = Seeded(0x5EED_0010_0003);
        let (mut answers, mut ruled_out, mut exact_sets) = (0usize, 0usize, 0usize);
        for major in MAJORS {
            let types = types_of(major).await;
            let mut values: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            let mut literals: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for row in rows(&fixture(major, "oracle/literals.tsv")) {
                let (Some(declared), Some("ok")) = (row[0].clone(), row[2].as_deref()) else {
                    continue;
                };
                if let Some(output) = row[3].clone() {
                    values.entry(declared.clone()).or_default().insert(output.clone());
                    literals.entry(declared.clone()).or_default().insert(output);
                }
                if let Some(input) = row[1].clone() {
                    literals.entry(declared).or_default().insert(input);
                }
            }
            let cases: BTreeSet<(String, Option<&str>)> =
                rows(&fixture(major, "oracle/comparisons.tsv"))
                    .into_iter()
                    .map(|row| {
                        (row[0].clone().expect("a case names a type"), clause(row[3].as_ref()))
                    })
                    .collect();
            for (declared, collation) in cases {
                let Some(pool) = values.get(&declared) else { continue };
                let pool: Vec<&str> = pool.iter().map(String::as_str).collect();
                let literals: Vec<&str> = literals[&declared].iter().map(String::as_str).collect();
                let resolved = schema(&declared, collation, &types);
                let (key_kind, orders_exactly) = match &resolved.comparisons[0] {
                    ComparisonPlan::Compared { kind, divergence } => {
                        let keys = pool.iter().all(|v| order_key(kind, v).is_some());
                        (keys.then_some(kind.clone()), divergence.is_none())
                    }
                    _ => (None, false),
                };
                let mut terms = Vec::new();
                for _ in 0..40 {
                    let op = *rng.pick(&OPERATORS);
                    let value = match op {
                        PredicateOp::IsNull | PredicateOp::IsNotNull => None,
                        _ => Some(rng.pick(&literals).to_string()),
                    };
                    let p = Predicate { column: "v".into(), op, value };
                    if let Ok(term) = resolve_term(&p, 0, &resolved, 0) {
                        terms.push(term);
                    }
                }
                if terms.is_empty() {
                    continue;
                }
                // A `NOT NULL` domain's column holds no NULL a dump can
                // restore, and a read refuses one.
                let nullable = !resolved.not_null[0];
                for _ in 0..30 {
                    let group_rows: Vec<Option<&str>> = (0..rng.below(6))
                        .map(|_| (!(nullable && rng.chance(4))).then(|| *rng.pick(&pool)))
                        .collect();
                    let present: Vec<&str> = group_rows.iter().flatten().copied().collect();
                    let nulls = group_rows.len() - present.len();
                    let key = |kind: &CompareKind, v: &str| order_key(kind, v).unwrap();
                    let mut loosened = false;
                    let bounds = key_kind.as_ref().filter(|_| !present.is_empty()).map(|kind| {
                        let cmp = |a: &str, b: &str| compare_keys(&key(kind, a), &key(kind, b));
                        let mut min = present
                            .iter()
                            .copied()
                            .reduce(|a, b| if cmp(b, a).is_lt() { b } else { a })
                            .unwrap();
                        let mut max = present
                            .iter()
                            .copied()
                            .reduce(|a, b| if cmp(b, a).is_gt() { b } else { a })
                            .unwrap();
                        if rng.chance(3) {
                            let below: Vec<&str> =
                                pool.iter().copied().filter(|v| cmp(v, min).is_lt()).collect();
                            if !below.is_empty() {
                                min = rng.pick(&below);
                                loosened = true;
                            }
                        }
                        if rng.chance(3) {
                            let above: Vec<&str> =
                                pool.iter().copied().filter(|v| cmp(v, max).is_gt()).collect();
                            if !above.is_empty() {
                                max = rng.pick(&above);
                                loosened = true;
                            }
                        }
                        (min.to_string(), max.to_string())
                    });
                    let dictionary: Option<Vec<String>> = (!present.is_empty()).then(|| {
                        present
                            .iter()
                            .map(|v| v.to_string())
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect()
                    });
                    let full = Group {
                        rows: group_rows.len() as u64,
                        nulls: Some(nulls as u64),
                        bounds: bounds.clone(),
                        dictionary: dictionary.clone(),
                    };
                    let weakened = Group {
                        rows: full.rows,
                        nulls: full.nulls.filter(|_| !rng.chance(8)),
                        bounds: full.bounds.clone().filter(|_| !rng.chance(4)),
                        dictionary: full.dictionary.clone().filter(|_| !rng.chance(4)),
                    };
                    let row_answers = |expr: &ResolvedExpr| {
                        group_rows
                            .iter()
                            .map(|v| {
                                expr.eval(
                                    true,
                                    RawRow::unchecked(&encode_field(*v)),
                                    &mut RowSplit::default(),
                                    "public.t",
                                    0,
                                )
                                .unwrap_or_else(|e| {
                                    panic!("{major} {declared}: the server wrote {v:?}: {e}")
                                })
                            })
                            .fold(TruthSet::EMPTY, |set, t| set.union(TruthSet::of(t)))
                    };
                    let trees: Vec<ResolvedExpr> = terms
                        .iter()
                        .cloned()
                        .map(ResolvedExpr::Term)
                        .chain((0..6).map(|_| random_tree(&mut rng, 3, &terms)))
                        .collect();
                    for expr in &trees {
                        let produced = row_answers(expr);
                        for group in [&full, &weakened] {
                            let set = expr.truths(group);
                            assert_eq!(
                                set.union(produced),
                                set,
                                "{major} {declared} {collation:?}: {expr:?} over {group_rows:?} \
                                     with {group:?} answered {set:?}"
                            );
                            answers += group_rows.len();
                            if !set.contains(Truth::True) && !group_rows.is_empty() {
                                ruled_out += 1;
                            }
                        }
                        let ResolvedExpr::Term(term) = expr else { continue };
                        let exact = if term.op.is_ordering() {
                            orders_exactly && !loosened && bounds.is_some()
                        } else {
                            !matches!(term.op, PredicateOp::IsNull | PredicateOp::IsNotNull)
                                && matches!(
                                    resolved.comparisons[0],
                                    ComparisonPlan::Compared { .. }
                                )
                                && term.comparison_notes().is_empty()
                                && dictionary.is_some()
                        };
                        if exact {
                            let only = Group {
                                bounds: full.bounds.clone().filter(|_| term.op.is_ordering()),
                                dictionary: full
                                    .dictionary
                                    .clone()
                                    .filter(|_| !term.op.is_ordering()),
                                ..full.clone()
                            };
                            assert_eq!(
                                expr.truths(&only),
                                produced,
                                "{major} {declared} {collation:?}: {expr:?} over {group_rows:?} is not exact"
                            );
                            exact_sets += 1;
                        }
                    }
                }
            }
        }
        // Floors, not counts: the first is the property's reach, the
        // other two are what keep it from being met by answering
        // everything.
        assert!(answers > 1_000_000, "only {answers} row answers held");
        assert!(ruled_out > 75_000, "only {ruled_out} groups ruled out");
        assert!(exact_sets > 50_000, "only {exact_sets} sets asserted exact");
    }
}
