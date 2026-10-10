use std::sync::Arc;

use super::*;
use crate::pgtype_impl::{comparison_for, text_grammar};
use pgdt_model::{ColumnDef, TypeDef, TypeKind};

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
                typmod: Some(crate::pgtype_impl::NumericTypmod { precision: 3, scale: 0 }),
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
    use crate::pgtype_impl::NumericTypmod;
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
            typmod: Some(crate::pgtype_impl::NumericTypmod { precision: 77, scale: 0 }),
        }),
        "as a number of at most 131072 digits before the point and 16383 after it, or `NaN`"
    );
}
