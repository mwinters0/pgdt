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

/// An enum, `public.mood`, whose labels sort the other way as text.
fn mood() -> TypeDef {
    let labels = vec!["sad".to_string(), "ok".to_string()];
    TypeDef { name: "public.mood".to_string(), kind: TypeKind::Enum { labels, exact: true } }
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

/// **Keys no order relates never compare, and never panic**: a scalar
/// against a nested value, two scalars of unrelated types — a `date`'s
/// against an `integer`'s, a special value against another type's — two
/// arrays whose elements differ, two records of different arity, and a pair
/// the server orders only under its `TimeZone`.
#[test]
fn keys_no_order_relates_answer_none() {
    let types = [record(1), record(2), mood()];
    let key = |declared: &str, text: &str| field(&plan(declared, &types), text);
    let pairs = [
        (key("integer", "1"), key("text", "1")),
        (key("integer", "1"), key("integer[]", "{1}")),
        (key("integer[]", "{1}"), key("text[]", "{1}")),
        (key("public.r1", "(1)"), key("public.r2", "(1,2)")),
        (key("int4range", "[1,2)"), key("numrange", "[1.5,2)")),
        (key("date", "2020-01-01"), key("integer", "18262")),
        (key("date", "-infinity"), key("text", "a")),
        (key("date", "infinity"), key("interval", "infinity")),
        (key("numeric", "NaN"), key("double precision", "NaN")),
        (key("public.mood", "ok"), key("integer", "1")),
        (key("oid", "1"), key("integer", "1")),
        (key("uuid", "00000000-0000-0000-0000-000000000000"), key("bytea", "\\x00")),
        (key("timestamp", "2020-01-01 00:00:00"), key("timestamptz", "2020-01-01 00:00:00+00")),
        (key("date", "2020-01-01"), key("timestamptz", "2020-01-01 00:00:00+00")),
    ];
    for (a, b) in pairs {
        assert_eq!(a.compare(&b), None, "{a:?} against {b:?}");
        assert_eq!(b.compare(&a), None, "{b:?} against {a:?}");
    }
}

/// **Keys an order relates compare correctly, whatever plans they were read
/// under**: two integer widths, two `numeric` scales in either semantics, a
/// `numeric(p,s)` against a bare one, a `date` against a `timestamp` at its
/// midnight, a `macaddr` against a `macaddr8` as the server widens it, and a
/// special value against its own domain — `date`'s `infinity` above a
/// `timestamp` and equal to a `timestamptz`'s.
#[test]
fn keys_an_order_relates_compare_across_plans() {
    let types = [];
    let key = |declared: &str, semantics, text: &str| {
        ValueKey::of(&plan(declared, &types), semantics, text).expect("a value of the type")
    };
    let postgres = ComparisonSemantics::Postgres;
    let datafusion = ComparisonSemantics::DataFusion;
    let ordered = [
        (key("smallint", postgres, "7"), key("bigint", postgres, "70000"), Ordering::Less),
        (
            key("numeric(5,1)", datafusion, "1.5"),
            key("numeric(6,2)", datafusion, "1.50"),
            Ordering::Equal,
        ),
        (
            key("numeric(5,1)", datafusion, "1.5"),
            key("numeric(6,2)", datafusion, "1.49"),
            Ordering::Greater,
        ),
        (key("numeric(5,1)", postgres, "-1.5"), key("numeric", postgres, "-1.50"), Ordering::Equal),
        (key("numeric(5,1)", postgres, "NaN"), key("numeric", postgres, "NaN"), Ordering::Equal),
        (
            key("numeric(5,1)", postgres, "9999.9"),
            key("numeric", postgres, "Infinity"),
            Ordering::Less,
        ),
        (
            key("date", postgres, "2020-01-02"),
            key("timestamp", postgres, "2020-01-01 23:59:59"),
            Ordering::Greater,
        ),
        (
            key("date", postgres, "2020-01-01"),
            key("timestamp", postgres, "2020-01-01 00:00:00"),
            Ordering::Equal,
        ),
        (
            key("date", datafusion, "1999-12-31"),
            key("timestamp", datafusion, "2000-01-01 00:00:00"),
            Ordering::Less,
        ),
        (
            key("macaddr", postgres, "08:00:2b:01:02:03"),
            key("macaddr8", postgres, "08:00:2b:ff:fe:01:02:03"),
            Ordering::Equal,
        ),
        (
            key("macaddr", postgres, "08:00:2b:01:02:03"),
            key("macaddr8", postgres, "08:00:2b:ff:fe:01:02:04"),
            Ordering::Less,
        ),
        (
            key("date", postgres, "infinity"),
            key("timestamp", postgres, "2020-01-01 00:00:00"),
            Ordering::Greater,
        ),
        (
            key("date", postgres, "infinity"),
            key("timestamptz", postgres, "infinity"),
            Ordering::Equal,
        ),
        (
            key("date", postgres, "-infinity"),
            key("timestamptz", postgres, "2020-01-01 00:00:00+00"),
            Ordering::Less,
        ),
        (key("character(3)", postgres, "ab "), key("text", postgres, "ab"), Ordering::Equal),
        (key("inet", postgres, "10.0.0.0/8"), key("cidr", postgres, "10.0.0.0/8"), Ordering::Equal),
    ];
    for (a, b, expected) in ordered {
        assert_eq!(a.compare(&b), Some(expected), "{a:?} against {b:?}");
        assert_eq!(b.compare(&a), Some(expected.reverse()), "{b:?} against {a:?}");
    }
    // A special value is ranked within its own type too.
    let date = |text| key("date", postgres, text);
    assert_eq!(date("infinity").compare(&date("2020-01-01")), Some(Ordering::Greater));
}

/// **A key is ordered as the semantics orders the column**: PostgreSQL's
/// order where the register gives one, DataFusion's over the value the
/// column emits — an enum by its label's text, a column with no plan as its
/// text — and no key where the semantics has no order for the column: a
/// column the register gives none in PostgreSQL's, a nested one in
/// DataFusion's.
#[test]
fn the_semantics_decides_the_order_a_key_is_read_in() {
    let types = [mood()];
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
/// equal to a filter, through the API's key as through the scalar key
/// beneath it, and `-0` is first in the order a bound is kept in, which is
/// `internal`'s alone (`order::compare_keys_stored`).
#[test]
fn the_stored_order_breaks_a_float_s_zeros() {
    let float = plan("double precision", &[]);
    let (negative, positive) = (field(&float, "-0"), field(&float, "0"));
    assert_eq!(negative.compare(&positive), Some(Ordering::Equal));
    let (Key::Scalar(a), Key::Scalar(b)) = (&negative.0, &positive.0) else {
        panic!("a float's key is a scalar's");
    };
    assert_eq!(order::compare_keys(a, b), Ordering::Equal);
    assert_eq!(order::compare_keys_stored(a, b), Ordering::Less);
    assert_eq!(order::compare_keys_stored(b, a), Ordering::Greater);
}
