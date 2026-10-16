use std::cmp::Ordering;

use pgdt_model::{ColumnDef, TypeDef, TypeKind};

use super::*;
use crate::compare::comparison_for;

fn plan(declared: &str, types: &[TypeDef]) -> ComparisonPlan {
    comparison_for(declared, None, types, &[])
}

/// A record of `fields` integers, `public.r<fields>`.
fn record(fields: usize) -> TypeDef {
    let columns = (0..fields).map(|i| ColumnDef::new(format!("f{i}"), "integer")).collect();
    TypeDef {
        name: format!("public.r{fields}"),
        kind: TypeKind::Composite { fields: Some(columns) },
    }
}

fn field(plan: &ComparisonPlan, text: &str) -> ValueKey {
    ValueKey::of(plan, ComparisonSemantics::Postgres, text).expect("a value of the type")
}

/// **A nested value has a key**, read from its field as a scalar's is and
/// from a literal in its input grammar, the server's own form and order:
/// an array's elements before its shape (I45), a range through
/// `make_range` (I46), a record field-wise.
#[test]
fn a_nested_value_is_keyed_from_its_plan() {
    let types = [record(2)];
    let array = plan("integer[]", &types);
    let range = plan("int4range", &types);
    let composite = plan("public.r2", &types);
    let postgres = ComparisonSemantics::Postgres;
    let ordered = [
        (&array, "{1,2}", "{1,10}", Ordering::Less),
        (&array, "{1,2}", "[0:1]={1,2}", Ordering::Greater),
        (&array, "{1,2}", "{{1,2},{3,4}}", Ordering::Less),
        (&range, "[1,11)", "[2,3)", Ordering::Less),
        (&range, "empty", "[1,2)", Ordering::Less),
        (&composite, "(1,)", "(1,2)", Ordering::Greater),
    ];
    for (plan, a, b, expected) in ordered {
        assert_eq!(field(plan, a).compare(&field(plan, b)), Some(expected), "{a} against {b}");
        assert_eq!(field(plan, b).compare(&field(plan, a)), Some(expected.reverse()));
    }
    // A literal in the `array_in`/`range_in` superset is the value its
    // field form writes.
    let literals = [(&array, "{ 1 , 2 }", "{1,2}"), (&range, "[1,10]", "[1,11)")];
    for (plan, literal, written) in literals {
        let literal = ValueKey::literal(plan, postgres, literal).expect("a literal of the type");
        assert_eq!(literal.compare(&field(plan, written)), Some(Ordering::Equal));
    }
    // The field grammar is the strict one: `array_out` writes no blank.
    assert!(ValueKey::of(&array, postgres, "{ 1 , 2 }").is_none());
    assert!(field(&array, "{1,2}").heap_bytes() > 0);
}

/// **Keys of different representations never compare, and never panic**: a
/// scalar against a nested value, two scalars of different kinds, two
/// arrays whose elements differ, two records of different arity.
#[test]
fn keys_of_different_representations_answer_none() {
    let types = [record(1), record(2)];
    let integer = plan("integer", &types);
    let text = plan("text", &types);
    let integers = plan("integer[]", &types);
    let texts = plan("text[]", &types);
    let pairs = [
        (field(&integer, "1"), field(&text, "1")),
        (field(&integer, "1"), field(&integers, "{1}")),
        (field(&integers, "{1}"), field(&texts, "{1}")),
        (field(&plan("public.r1", &types), "(1)"), field(&plan("public.r2", &types), "(1,2)")),
        (field(&plan("int4range", &types), "[1,2)"), field(&plan("numrange", &types), "[1.5,2)")),
    ];
    for (a, b) in pairs {
        assert_eq!(a.compare(&b), None, "{a:?} against {b:?}");
        assert_eq!(b.stored_order(&a), None, "{b:?} against {a:?}");
    }
    // A special value is ranked, not represented: `infinity` is above a
    // date in any kind holding one.
    let date = plan("date", &types);
    assert_eq!(
        field(&date, "infinity").compare(&field(&date, "2020-01-01")),
        Some(Ordering::Greater)
    );
}

/// **A key is ordered as the semantics orders the column**: PostgreSQL's
/// order where the register gives one, DataFusion's over the value the
/// column emits — an enum by its label's text, a column with no plan as its
/// text — and no key where the semantics has no order for the column: a
/// column the register gives none in PostgreSQL's, a nested one in
/// DataFusion's.
#[test]
fn the_semantics_decides_the_order_a_key_is_read_in() {
    let labels = vec!["sad".to_string(), "ok".to_string()];
    let types =
        [TypeDef { name: "public.mood".to_string(), kind: TypeKind::Enum { labels, exact: true } }];
    let mood = plan("public.mood", &types);
    let order = |semantics| {
        let key = |text| ValueKey::of(&mood, semantics, text).expect("a declared label");
        key("sad").compare(&key("ok"))
    };
    assert_eq!(order(ComparisonSemantics::Postgres), Some(Ordering::Less));
    assert_eq!(order(ComparisonSemantics::DataFusion), Some(Ordering::Greater));

    let money = plan("money", &types);
    assert_eq!(money, ComparisonPlan::Refused);
    assert!(ValueKey::of(&money, ComparisonSemantics::Postgres, "$1.00").is_none());
    let text = |text| ValueKey::of(&money, ComparisonSemantics::DataFusion, text).unwrap();
    assert_eq!(text("$10.00").compare(&text("$9.00")), Some(Ordering::Less));

    let array = plan("integer[]", &types);
    assert!(ValueKey::of(&array, ComparisonSemantics::DataFusion, "{1}").is_none());
    assert!(ValueKey::literal(&array, ComparisonSemantics::DataFusion, "{1}").is_none());
    // An array of a type the register orders by nothing has no key either,
    // though its empty value would read.
    assert!(ValueKey::of(&plan("json[]", &types), ComparisonSemantics::Postgres, "{}").is_none());
}

/// **A literal is read as the server coerces one**, with no typmod (I63): a
/// `numeric(2,1)` field is rounded to its scale and refused past its
/// precision, where its literal is read exactly and is never past one; and
/// a `smallint` literal in DataFusion's semantics is read as DataFusion
/// reads it, an `i64`.
#[test]
fn a_literal_is_read_in_its_input_grammar() {
    let decimal = plan("numeric(2,1)", &[]);
    let postgres = ComparisonSemantics::Postgres;
    assert_eq!(field(&decimal, "1.04").compare(&field(&decimal, "1.0")), Some(Ordering::Equal));
    assert!(ValueKey::literal(&decimal, postgres, "1.04").is_none());
    assert!(ValueKey::of(&decimal, postgres, "12.5").is_none());
    let literal = ValueKey::literal(&decimal, postgres, "12.5").expect("a numeric literal");
    assert_eq!(literal.compare(&field(&decimal, "9.9")), Some(Ordering::Greater));

    let smallint = plan("smallint", &[]);
    assert!(ValueKey::literal(&smallint, postgres, "70000").is_none());
    let wide = ValueKey::literal(&smallint, ComparisonSemantics::DataFusion, "70000");
    assert!(wide.is_some());
}

/// **The stored order refines the filter's**: a float's `-0` and `0` are
/// equal to a filter and `-0` is first in the order a bound is kept in,
/// through the API's key as through the scalar key the gatherer holds
/// (`order::compare_keys_stored`).
#[test]
fn the_stored_order_breaks_a_float_s_zeros() {
    let float = plan("double precision", &[]);
    let (negative, positive) = (field(&float, "-0"), field(&float, "0"));
    assert_eq!(negative.compare(&positive), Some(Ordering::Equal));
    assert_eq!(negative.stored_order(&positive), Some(Ordering::Less));
    assert_eq!(positive.stored_order(&negative), Some(Ordering::Greater));
    let (Key::Scalar(a), Key::Scalar(b)) = (&negative.0, &positive.0) else {
        panic!("a float's key is a scalar's");
    };
    assert_eq!(order::compare_keys(a, b), Ordering::Equal);
    assert_eq!(order::compare_keys_stored(a, b), Ordering::Less);
}
