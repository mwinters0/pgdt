use pgdt_model::{ColumnDef, TypeDef, TypeKind};

use super::*;
use crate::compare::comparison_for;

fn ty(name: &str, kind: TypeKind) -> TypeDef {
    TypeDef { name: name.to_string(), kind }
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
