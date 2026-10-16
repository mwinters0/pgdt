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

/// A composite `public.<name>` of the given column types.
fn composite(name: &str, columns: &[&str]) -> TypeDef {
    let columns =
        columns.iter().enumerate().map(|(i, ty)| ColumnDef::new(format!("f{i}"), *ty)).collect();
    TypeDef { name: format!("public.{name}"), kind: TypeKind::Composite { fields: Some(columns) } }
}

/// The types the cross-plan cases read: records, two enum types of one
/// label list, a domain, and composites of one shape and of two.
fn cross_plan_types() -> Vec<TypeDef> {
    let mut mood2 = mood();
    mood2.name = "public.mood2".to_string();
    vec![
        record(1),
        record(2),
        mood(),
        mood2,
        TypeDef { name: "public.posint".to_string(), kind: TypeKind::domain("integer") },
        composite("pair", &["integer", "integer"]),
        composite("labelled", &["integer", "text"]),
        composite("varied", &["integer", "character varying"]),
    ]
}

/// **Keys whose orders the engine does not relate never compare, and never
/// panic** — each pair probed on a PostgreSQL 16 server, which resolves no
/// operator for it, resolves one only under `TimeZone`, or refuses the cast
/// it resolves: a scalar against a nested value, a `date` against an
/// `integer`, two enum types, a `macaddr` against a `macaddr8`, a `bigint`
/// past an `oid`'s range, a `numeric` past a double's, two array or range
/// types, two records of different column types, and two keys read in two
/// semantics. And the pairs `KD119` and `KD120` name.
#[test]
fn keys_no_order_relates_answer_none() {
    let types = cross_plan_types();
    let key = |declared: &str, text: &str| field(&plan(declared, &types), text);
    let datafusion = |declared: &str, text: &str| {
        ValueKey::of(&plan(declared, &types), ComparisonSemantics::DataFusion, text).unwrap()
    };
    let pairs = [
        (key("integer", "1"), key("text", "1")),
        (key("integer", "1"), key("integer[]", "{1}")),
        (key("public.r1", "(1)"), key("public.r2", "(1,2)")),
        (key("date", "2020-01-01"), key("integer", "18262")),
        (key("date", "-infinity"), key("text", "a")),
        (key("date", "infinity"), key("interval", "infinity")),
        (key("public.mood", "ok"), key("integer", "1")),
        (key("public.mood", "ok"), key("public.mood2", "ok")),
        (key("boolean", "t"), key("integer", "1")),
        (key("uuid", "00000000-0000-0000-0000-000000000000"), key("bytea", "\\x00")),
        (key("timestamp", "2020-01-01 00:00:00"), key("timestamptz", "2020-01-01 00:00:00+00")),
        (key("date", "2020-01-01"), key("timestamptz", "2020-01-01 00:00:00+00")),
        (key("time", "01:00:00"), key("timetz", "01:00:00+00")),
        (key("date", "2020-01-01"), key("time", "01:00:00")),
        (key("macaddr", "08:00:2b:01:02:03"), key("macaddr8", "08:00:2b:ff:fe:01:02:03")),
        (key("bigint", "-1"), key("oid", "5")),
        (key("bigint", "5000000000"), key("oid", "5")),
        (key("oid", "1"), key("numeric", "2")),
        (key("oid", "1"), key("double precision", "2")),
        (key("double precision", "1"), key("numeric", &format!("1{}", "0".repeat(400)))),
        (key("double precision", "0"), key("numeric", &format!("0.{}1", "0".repeat(400)))),
        (key("integer[]", "{1}"), key("text[]", "{1}")),
        (key("integer[]", "{1}"), key("bigint[]", "{1}")),
        (key("integer[]", "{1}"), key("public.posint[]", "{1}")),
        (key("text[]", "{a}"), key("character varying[]", "{a}")),
        (key("int4range", "[1,2)"), key("int8range", "[1,2)")),
        (key("int4range", "[1,2)"), key("numrange", "[1.5,2)")),
        (key("public.labelled", "(1,a)"), key("public.varied", "(1,a)")),
        (key("integer", "1"), datafusion("integer", "1")),
        // `KD120`: a `text`'s trailing blank, which a `character
        // varying`'s cast to `bpchar` would trim.
        (key("text", "ab "), key("character(3)", "ab ")),
        // `KD119`: DataFusion widens both.
        (datafusion("smallint", "7"), datafusion("bigint", "7")),
        (datafusion("numeric(5,1)", "1.5"), datafusion("numeric(6,2)", "1.50")),
    ];
    for (a, b) in pairs {
        assert_eq!(a.compare(&b), None, "{a:?} against {b:?}");
        assert_eq!(b.compare(&a), None, "{b:?} against {a:?}");
    }
}

/// **Keys whose orders the engine relates compare as it answers, whatever
/// plans they were read under** — each pair's answer the one a PostgreSQL
/// 16 server gives: through the cross-type operators of the integer widths
/// and of `real` and `double precision`; through the cast to `oid`, which
/// wraps a negative `integer`; to `double precision`, rounding a `bigint`
/// and a `numeric`; to `numeric`, exactly; `date2timestamp`; a `time` as an
/// `interval`; a `character(n)` as `text`; `cidr` as `inet`; special values
/// by their rank; two arrays and two records of one type at every leaf; and
/// in DataFusion's semantics, an enum's label as text.
#[test]
fn keys_an_order_relates_compare_across_plans() {
    use Ordering::{Equal, Greater, Less};
    let types = cross_plan_types();
    let key = |declared: &str, text: &str| field(&plan(declared, &types), text);
    let datafusion = |declared: &str, text: &str| {
        ValueKey::of(&plan(declared, &types), ComparisonSemantics::DataFusion, text).unwrap()
    };
    let ordered = [
        (key("smallint", "7"), key("bigint", "70000"), Less),
        (key("integer", "-1"), key("oid", "5"), Greater),
        (key("smallint", "-1"), key("oid", "4294967295"), Equal),
        (key("bigint", "4294967295"), key("oid", "5"), Greater),
        (key("integer", "16777217"), key("real", "16777216"), Greater),
        (key("bigint", "9007199254740993"), key("double precision", "9007199254740992"), Equal),
        (key("real", "1"), key("double precision", "2"), Less),
        (key("integer", "2"), key("numeric(5,1)", "1.5"), Greater),
        (key("bigint", "1"), key("numeric", "NaN"), Less),
        (key("integer", "1"), key("numeric", "Infinity"), Less),
        (key("real", "0.1"), key("numeric", "0.1"), Greater),
        (key("double precision", "0.1"), key("numeric", "0.1"), Equal),
        (key("double precision", "Infinity"), key("numeric", "Infinity"), Equal),
        (key("double precision", "NaN"), key("numeric(5,1)", "NaN"), Equal),
        (key("double precision", "1"), key("numeric", "NaN"), Less),
        (key("numeric(5,1)", "-1.5"), key("numeric", "-1.50"), Equal),
        (key("numeric(5,1)", "1.5"), key("numeric(6,2)", "1.50"), Equal),
        (key("numeric(5,1)", "1.5"), key("numeric(6,2)", "1.49"), Greater),
        (key("numeric(5,1)", "NaN"), key("numeric", "NaN"), Equal),
        (key("numeric(5,1)", "9999.9"), key("numeric", "Infinity"), Less),
        (key("date", "2020-01-02"), key("timestamp", "2020-01-01 23:59:59"), Greater),
        (key("date", "2020-01-01"), key("timestamp", "2020-01-01 00:00:00"), Equal),
        (key("date", "5874897-12-31"), key("timestamp", "294276-12-31 00:00:00"), Greater),
        (key("date", "infinity"), key("timestamp", "2020-01-01 00:00:00"), Greater),
        (key("date", "infinity"), key("timestamp", "infinity"), Equal),
        (key("time", "01:00:00"), key("interval", "01:00:00"), Equal),
        (key("time", "01:00:00"), key("interval", "1 day"), Less),
        (key("time", "01:00:00"), key("interval", "infinity"), Less),
        (key("character(3)", "ab "), key("text", "ab"), Equal),
        (key("character(5)", "ab   "), key("name", "abc"), Less),
        (key("character varying(3)", "abc"), key("text", "abc"), Equal),
        (key("inet", "10.0.0.0/8"), key("cidr", "10.0.0.0/8"), Equal),
        (key("public.mood", "sad"), field(&plan("public.mood", &[mood()]), "ok"), Less),
        (key("character varying(3)[]", "{a}"), key("character varying(5)[]", "{b}"), Less),
        (key("public.pair", "(1,2)"), key("public.r2", "(1,3)"), Less),
        (datafusion("public.mood", "ok"), datafusion("text", "ok"), Equal),
    ];
    for (a, b, expected) in ordered {
        assert_eq!(a.compare(&b), Some(expected), "{a:?} against {b:?}");
        assert_eq!(b.compare(&a), Some(expected.reverse()), "{b:?} against {a:?}");
    }
    // A special value is ranked within its own type too.
    assert_eq!(key("date", "infinity").compare(&key("date", "2020-01-01")), Some(Greater));
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
    let (Key::Scalar { key: a, .. }, Key::Scalar { key: b, .. }) = (&negative.key, &positive.key)
    else {
        panic!("a float's key is a scalar's");
    };
    assert_eq!(order::compare_keys(a, b), Ordering::Equal);
    assert_eq!(order::compare_keys_stored(a, b), Ordering::Less);
    assert_eq!(order::compare_keys_stored(b, a), Ordering::Greater);
}
