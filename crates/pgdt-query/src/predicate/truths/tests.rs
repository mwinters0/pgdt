use std::cmp::Ordering;

use arrow::datatypes::DataType;

use super::*;
use crate::predicate::Predicate;
use crate::predicate::test_support::*;
use pgdt_model::UnrepresentableTier;

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
