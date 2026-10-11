use std::sync::Arc;

use super::*;
use crate::compare::{Geometric, TextGrammar, Unchecked, comparison_for};
use pgdt_model::ColumnDef;

fn ty(name: &str, kind: TypeKind) -> TypeDef {
    TypeDef { name: name.to_string(), kind }
}

/// **A bit string's grammar is read off every spelling of its type**,
/// with the length its typmod hands `bit_in` or `varbit_in` (I73): `bit`
/// alone is `bit(1)` and `"bit"` has none (I8), `varbit` is `bit varying`,
/// and a domain hands down its base type's. An array's elements, and any
/// other type, have none here; a nested position carries its own.
#[test]
fn a_bit_string_s_grammar_is_read_off_every_spelling() {
    let bit = |length| Some(TextGrammar::Bit { varying: false, length });
    let varbit = |length| Some(TextGrammar::Bit { varying: true, length });
    let types = [
        ty("public.b3", TypeKind::domain("bit(3)")),
        ty("public.b3b", TypeKind::domain("public.b3")),
        ty("public.flags", TypeKind::Composite { fields: Some(vec![ColumnDef::new("f", "bit")]) }),
    ];
    for (declared, expected) in [
        ("bit", bit(Some(1))),
        ("BIT", bit(Some(1))),
        ("bit(3)", bit(Some(3))),
        ("\"bit\"", bit(None)),
        ("\"bit\"(5)", bit(Some(5))),
        ("bit varying", varbit(None)),
        ("BIT  VARYING(4)", varbit(Some(4))),
        ("varbit", varbit(None)),
        ("\"varbit\"(2)", varbit(Some(2))),
        ("public.b3", bit(Some(3))),
        ("public.b3b", bit(Some(3))),
        ("\"bit varying\"", None),
        ("\"BIT\"", None),
        ("bit(0)", None),
        ("bit(x)", None),
        ("bit(3)[]", None),
        ("public.b3[]", None),
        ("public.flags", None),
        ("text", None),
    ] {
        assert_eq!(text_grammar(declared, &types), expected, "`{declared}`");
    }
    let ComparisonPlan::Nested(NestedCompare::Array(element)) =
        comparison_for("public.b3[]", None, &types, &[])
    else {
        panic!("an array of a domain over `bit(3)` is nested");
    };
    assert!(
        matches!(element.plan, NestedCompare::Uncomparable { grammar, .. } if grammar == bit(Some(3)))
    );
    let ComparisonPlan::Nested(NestedCompare::Record(fields)) =
        comparison_for("public.flags", None, &types, &[])
    else {
        panic!("a composite is nested");
    };
    assert!(
        matches!(&fields[0].1.plan, NestedCompare::Uncomparable { grammar, .. } if *grammar == bit(Some(1)))
    );
}

/// **A geometric type's grammar is read off every spelling of it** (I74),
/// through a domain, and none off one written with a typmod, which no
/// geometric type takes. An array of `box` has a grammar of its own, its
/// elements split at `;` (I22); an array of any other geometric type is
/// read through its nested plan, whose element carries the grammar, as
/// does a `box` element a container holds.
#[test]
fn a_geometric_type_s_grammar_is_read_off_every_spelling() {
    let types = [
        ty("public.boxes", TypeKind::domain("box")),
        ty("public.boxlist", TypeKind::domain("box[]")),
        ty(
            "public.shapes",
            TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("b", "box[]"), ColumnDef::new("c", "\"circle\"")]),
            },
        ),
    ];
    let geometric = |g| Some(TextGrammar::Geometric(g));
    for (declared, expected) in [
        ("point", geometric(Geometric::Point)),
        ("LINE", geometric(Geometric::Line)),
        ("lseg", geometric(Geometric::Lseg)),
        ("\"box\"", geometric(Geometric::Box)),
        ("path", geometric(Geometric::Path)),
        ("polygon", geometric(Geometric::Polygon)),
        ("circle", geometric(Geometric::Circle)),
        ("public.boxes", geometric(Geometric::Box)),
        ("box[]", Some(TextGrammar::BoxArray { not_null: false })),
        ("\"box\"[][]", Some(TextGrammar::BoxArray { not_null: false })),
        ("public.boxes[]", Some(TextGrammar::BoxArray { not_null: false })),
        ("public.boxlist", Some(TextGrammar::BoxArray { not_null: false })),
        ("point(2)", None),
        ("\"Point\"", None),
        ("pg_catalog.point", None),
        ("point[]", None),
        ("public.shapes", None),
    ] {
        assert_eq!(text_grammar(declared, &types), expected, "`{declared}`");
    }
    let element_grammar = |declared| match comparison_for(declared, None, &types, &[]) {
        ComparisonPlan::Nested(NestedCompare::Array(element)) => match element.plan {
            NestedCompare::Uncomparable { grammar, .. } => grammar,
            other => panic!("`{declared}`'s element is {other:?}"),
        },
        other => panic!("`{declared}` is {other:?}"),
    };
    assert_eq!(element_grammar("point[]"), geometric(Geometric::Point));
    assert_eq!(element_grammar("box[]"), geometric(Geometric::Box));
    assert_eq!(element_grammar("public.boxes[]"), geometric(Geometric::Box));
    let ComparisonPlan::Nested(NestedCompare::Record(fields)) =
        comparison_for("public.shapes", None, &types, &[])
    else {
        panic!("a composite is nested");
    };
    assert!(matches!(
        &fields[0].1.plan,
        NestedCompare::Array(element) if matches!(element.plan, NestedCompare::Uncomparable {
            grammar: Some(TextGrammar::Geometric(Geometric::Box)), ..
        })
    ));
    assert!(matches!(
        &fields[1].1.plan,
        NestedCompare::Uncomparable { grammar, .. } if *grammar == geometric(Geometric::Circle)
    ));
}

/// **A type whose input function refuses every value, or none, has a
/// grammar saying so**: the internal types I78 names and an enum whose
/// labels are exactly none (I70) refuse every field, and `"char"`,
/// `refcursor`, `xid`, `xid8` and `cid` none (I79) — bare `char` being
/// `character`, a typmod none of them takes no grammar, and a nested
/// position carrying its own.
#[test]
fn a_type_refusing_every_value_or_none_has_a_grammar_saying_so() {
    let types = [
        ty("public.none", TypeKind::exact_enum::<&str>([])),
        ty("public.unread", TypeKind::Enum { labels: vec![], exact: false }),
        ty("public.tree", TypeKind::domain("pg_node_tree")),
    ];
    let all = Some(TextGrammar::RefusesAll);
    let nothing = Some(TextGrammar::RefusesNothing);
    for (declared, expected) in [
        ("pg_node_tree", all),
        ("pg_ndistinct", all),
        ("pg_dependencies", all),
        ("pg_mcv_list", all),
        ("pg_brin_bloom_summary", all),
        ("pg_brin_minmax_multi_summary", all),
        ("\"gtsvector\"", all),
        ("public.tree", all),
        ("public.none", all),
        ("public.unread", None),
        ("\"char\"", nothing),
        ("refcursor", nothing),
        ("xid", nothing),
        ("XID8", nothing),
        ("cid", nothing),
        ("char", None),
        ("xid(4)", None),
        ("xid[]", None),
        ("pg_catalog.xid", None),
    ] {
        assert_eq!(text_grammar(declared, &types), expected, "`{declared}`");
    }
    let ComparisonPlan::Nested(NestedCompare::Array(element)) =
        comparison_for("xid8[]", None, &types, &[])
    else {
        panic!("an array of `xid8` is nested");
    };
    assert!(
        matches!(element.plan, NestedCompare::Uncomparable { grammar, .. } if grammar == nothing)
    );
}

/// **What a strict parse leaves unchecked is named at every position,
/// with its reason** (`unchecked_positions`): a type the restoring server
/// decides the input of, at the column or beneath it, a type no reader
/// here reads, a range declaring a canonical function, an enum whose
/// labels are inexact and a domain's `CHECK` anywhere — and nothing for
/// a type a strict parse reads, `json` and a geometric type among them,
/// nor for a `NOT NULL` domain beneath a container, whose NULL it refuses.
#[test]
fn what_a_strict_parse_leaves_unchecked_is_named_where_it_lies() {
    let domain = |base: &str, not_null, check| TypeKind::Domain {
        base_type: base.into(),
        collation: None,
        not_null,
        check,
    };
    let types = [
        ty("public.gtype", TypeKind::base()),
        ty(
            "public.canon",
            TypeKind::Range {
                subtype: Some("integer".into()),
                multirange_type_name: None,
                canonical: Some("public.canon_fn".into()),
            },
        ),
        ty(
            "public.doc",
            TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("id", "integer"),
                    ColumnDef::new("body", "xml"),
                    ColumnDef::new("n", "public.pos"),
                ]),
            },
        ),
        ty("public.mood", TypeKind::Enum { labels: vec!["ok".into()], exact: false }),
        ty("public.none", TypeKind::exact_enum::<&str>([])),
        ty("public.shell", TypeKind::Shell),
        ty("public.unparsed", TypeKind::Composite { fields: None }),
        ty(
            "public.nosub",
            TypeKind::Range {
                subtype: None,
                multirange_type_name: Some("public.nosubmulti".into()),
                canonical: None,
            },
        ),
        ty("public.ints", TypeKind::domain("integer[]")),
        ty(
            "public.grid",
            TypeKind::Composite { fields: Some(vec![ColumnDef::new("rows", "public.ints[]")]) },
        ),
        ty("public.pos", domain("integer", false, true)),
        ty("public.over_pos", domain("public.pos", false, false)),
        ty("public.nn", domain("integer", true, false)),
        ty(
            "public.posrange",
            TypeKind::Range {
                subtype: Some("public.pos".into()),
                multirange_type_name: Some("public.posmulti".into()),
                canonical: None,
            },
        ),
    ];
    let named = |declared: &str| -> Vec<(String, String, Unchecked)> {
        let plan = comparison_for(declared, None, &types, &[]);
        unchecked_positions(declared, &plan, column_grammar(&plan, declared, &types), &types)
    };
    let at = |path: &str, declared: &str, why| (path.to_string(), declared.to_string(), why);
    let canon = || Unchecked::RangeCanonical { function: "public.canon_fn".into() };
    for (declared, expected) in [
        ("integer", vec![]),
        ("json", vec![]),
        ("json[]", vec![]),
        ("box", vec![]),
        ("box[]", vec![]),
        ("bit(3)", vec![]),
        ("public.nn", vec![]),
        ("xml", vec![at("", "xml", Unchecked::Xml)]),
        ("money", vec![at("", "money", Unchecked::Money)]),
        ("money[]", vec![at("[]", "money", Unchecked::Money)]),
        ("tsvector", vec![at("", "tsvector", Unchecked::NoReader)]),
        ("pg_lsn[]", vec![at("[]", "pg_lsn", Unchecked::NoReader)]),
        ("regclass", vec![at("", "regclass", Unchecked::Catalog)]),
        ("aclitem[]", vec![at("[]", "aclitem", Unchecked::Catalog)]),
        ("\"char\"", vec![]),
        ("xid[]", vec![]),
        ("pg_node_tree", vec![]),
        ("public.none", vec![]),
        ("public.nowhere", vec![at("", "public.nowhere", Unchecked::UndeclaredType)]),
        ("nowhere", vec![at("", "nowhere", Unchecked::UndeclaredType)]),
        ("public.nowhere[]", vec![at("[]", "public.nowhere", Unchecked::UndeclaredType)]),
        ("public.shell", vec![at("", "public.shell", Unchecked::BaseType)]),
        ("public.unparsed", vec![at("", "public.unparsed", Unchecked::Unparsed)]),
        ("public.nosub", vec![at("", "public.nosub", Unchecked::Unparsed)]),
        ("public.nosubmulti", vec![at("", "public.nosubmulti", Unchecked::Unparsed)]),
        ("bit(0)", vec![at("", "bit(0)", Unchecked::Unparsed)]),
        ("public.ints[]", vec![at("[]", "public.ints", Unchecked::ArrayShape)]),
        ("public.grid", vec![at(".rows[]", "public.ints", Unchecked::ArrayShape)]),
        ("public.gtype", vec![at("", "public.gtype", Unchecked::BaseType)]),
        ("public.gtype[]", vec![at("[]", "public.gtype", Unchecked::BaseType)]),
        ("public.canon", vec![at("", "public.canon", canon())]),
        ("public.canon[]", vec![at("", "public.canon", canon())]),
        ("public.mood", vec![at("", "public.mood", Unchecked::LabelsInexact)]),
        ("public.mood[]", vec![at("[]", "public.mood", Unchecked::LabelsInexact)]),
        (
            "public.doc",
            vec![
                at(".body", "xml", Unchecked::Xml),
                at(".n", "public.pos", Unchecked::DomainCheck),
            ],
        ),
        ("public.pos", vec![at("", "public.pos", Unchecked::DomainCheck)]),
        ("public.over_pos", vec![at("", "public.pos", Unchecked::DomainCheck)]),
        ("public.pos[]", vec![at("[]", "public.pos", Unchecked::DomainCheck)]),
        ("public.nn[]", vec![]),
        ("public.posrange", vec![at(".bound", "public.pos", Unchecked::DomainCheck)]),
        ("public.posmulti", vec![at("[].bound", "public.pos", Unchecked::DomainCheck)]),
    ] {
        assert_eq!(named(declared), expected, "{declared}");
    }
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
