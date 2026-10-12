use super::*;
use crate::ddl::tests::{parse_extension, parse_table, parse_type};
use crate::ddl::{StatementShape, classify_statement};
use pgdt_model::ColumnDef;

/// A domain carries its own `COLLATE`, which is its *type default* — the
/// thing a column-level clause exists to override — and its `NOT NULL`,
/// which a `CHECK` beside it does not hide.
#[test]
fn a_domain_keeps_its_own_collate_clause() {
    let plain = parse_type(&["CREATE DOMAIN public.d AS text;"]);
    assert_eq!(plain.kind, TypeKind::domain("text"));

    let collated = parse_type(&["CREATE DOMAIN public.dc AS text COLLATE pg_catalog.\"C\";"]);
    assert_eq!(
        collated.kind,
        TypeKind::Domain {
            base_type: "text".to_string(),
            collation: Some("pg_catalog.\"C\"".to_string()),
            not_null: false,
            check: false,
        }
    );

    let constrained = parse_type(&[
        "CREATE DOMAIN public.dn AS character varying(10) COLLATE pg_catalog.\"C\" NOT NULL",
        "    CONSTRAINT dn_check CHECK ((VALUE <> ''::text));",
    ]);
    assert_eq!(
        constrained.kind,
        TypeKind::Domain {
            base_type: "character varying(10)".to_string(),
            collation: Some("pg_catalog.\"C\"".to_string()),
            not_null: true,
            check: true,
        }
    );
}

#[test]
fn parses_an_enum() {
    let def = parse_type(&[
        "CREATE TYPE public.mood AS ENUM (",
        "    'sad',",
        "    'has space',",
        "    'has,comma',",
        "    'has''quote'",
        ");",
    ]);
    assert_eq!(def.name, "public.mood");
    assert_eq!(
        def.kind,
        TypeKind::exact_enum([
            "sad".to_string(),
            "has space".to_string(),
            "has,comma".to_string(),
            "has'quote".to_string(),
        ])
    );
}

#[test]
fn parses_a_domain_over_a_domain() {
    let def = parse_type(&["CREATE DOMAIN public.derived AS public.base_domain NOT NULL;"]);
    assert_eq!(
        def,
        TypeDef {
            name: "public.derived".to_string(),
            kind: TypeKind::Domain {
                base_type: "public.base_domain".to_string(),
                collation: None,
                not_null: true,
                check: false,
            },
        }
    );
}

#[test]
fn parses_a_composite_type() {
    let def = parse_type(&["CREATE TYPE public.point2d AS (", "\tx integer,", "\ty text", ");"]);
    assert_eq!(
        def.kind,
        TypeKind::Composite {
            fields: Some(vec![ColumnDef::new("x", "integer"), ColumnDef::new("y", "text")])
        }
    );
}

/// The field list is all-or-nothing: `record_out` is positional, so a
/// type parsed with one field missing would refuse every valid row of it
/// (see [`TypeKind::Composite`]). An empty body stays a real zero-field
/// composite, which is a different thing entirely (I23).
#[test]
fn a_composite_with_an_unparseable_fragment_keeps_no_fields_at_all() {
    let def = parse_type(&["CREATE TYPE public.broken AS (", "\tx integer,", "\t?!", ");"]);
    assert_eq!(def.kind, TypeKind::Composite { fields: None });

    let empty = parse_type(&["CREATE TYPE public.empty_comp AS (", ");"]);
    assert_eq!(empty.kind, TypeKind::Composite { fields: Some(vec![]) });
}

#[test]
fn parses_a_shell_type() {
    let def = parse_type(&["CREATE TYPE public.shelly;"]);
    assert_eq!(def.kind, TypeKind::Shell);
}

#[test]
fn parses_a_base_type() {
    let def = parse_type(&[
        "CREATE TYPE public.mytype (",
        "    INPUT = mytype_in,",
        "    OUTPUT = mytype_out",
        ");",
    ]);
    assert_eq!(def.kind, TypeKind::base());
}

/// **A base type's `DELIMITER` is the first byte of its literal**, as
/// `DefineType` takes it, and `,` where it states none (I22); a value
/// that is no plain string literal is a delimiter this build cannot read.
#[test]
fn a_base_type_s_delimiter_is_read_off_its_literal() {
    let delimiter = |clause: &str| {
        let def = parse_type(&[
            "CREATE TYPE public.mytype (",
            "    INPUT = mytype_in,",
            "    OUTPUT = mytype_out,",
            "    CATEGORY = 'S',",
            clause,
            "    ALIGNMENT = int4",
            ");",
        ]);
        match def.kind {
            TypeKind::Base { delimiter } => delimiter,
            other => panic!("{clause}: {other:?}"),
        }
    };
    for (clause, expected) in [
        ("    DELIMITER = ';',", Some(b';')),
        ("    delimiter = '|',", Some(b'|')),
        ("    DELIMITER = '''',", Some(b'\'')),
        ("    DELIMITER = '\\',", Some(b'\\')),
        ("    DELIMITER = ';;',", Some(b';')),
        ("    DELIMITER = '',", Some(0)),
        ("    DELIMITER = '\u{e9}',", Some(0xc3)),
        ("    DEFAULT = 'a, DELIMITER = x',", Some(b',')),
        ("    DELIMITER = E';',", None),
        ("    DELIMITER = semicolon,", None),
        ("    DELIMITER = ';'::text,", None),
    ] {
        assert_eq!(delimiter(clause), expected, "{clause}");
    }
}

#[test]
fn parses_a_range_type() {
    let def = parse_type(&[
        "CREATE TYPE public.myrange AS RANGE (subtype = int4, subtype_diff = int4mi);",
    ]);
    assert_eq!(
        def.kind,
        TypeKind::Range {
            subtype: Some("int4".to_string()),
            multirange_type_name: None,
            canonical: None,
        }
    );
}

/// PG14+ real output (`fixtures/{14..18}/types/default.sql`): multi-line
/// body, multi-word subtype, and the `multirange_type_name` parameter
/// naming the auto-created companion type nothing else in the dump ever
/// declares (I10).
#[test]
fn parses_a_range_type_with_a_multirange_companion() {
    let def = parse_type(&[
        "CREATE TYPE public.myrange AS RANGE (",
        "    subtype = double precision,",
        "    multirange_type_name = public.myrange_multi",
        ");",
    ]);
    assert_eq!(
        def.kind,
        TypeKind::Range {
            subtype: Some("double precision".to_string()),
            multirange_type_name: Some("public.myrange_multi".to_string()),
            canonical: None,
        }
    );
}

/// `pg_dump` writes `canonical = <function>` for any range type whose
/// `pg_range.rngcanonical` is set (I10), in the same `key = value` body
/// every other parameter is in; the value is kept verbatim, like
/// `subtype`, nothing reading it beyond its presence.
///
/// The body here is every parameter `dumpRangeType` can append, in order,
/// so the two the grammar keeps are found past the three it steps over —
/// `collation` in particular, whose value carries a quoted identifier.
#[test]
fn parses_a_range_type_declaring_a_canonical_function() {
    let def = parse_type(&[
        "CREATE TYPE public.canonrange AS RANGE (",
        "    subtype = integer,",
        "    multirange_type_name = public.canonrange_multi,",
        "    collation = pg_catalog.\"C\",",
        "    canonical = public.canonrange_canonical,",
        "    subtype_diff = public.canonrange_diff",
        ");",
    ]);
    assert_eq!(
        def.kind,
        TypeKind::Range {
            subtype: Some("integer".to_string()),
            multirange_type_name: Some("public.canonrange_multi".to_string()),
            canonical: Some("public.canonrange_canonical".to_string()),
        }
    );
}

#[test]
fn parses_an_extension() {
    let ext = parse_extension(&["CREATE EXTENSION IF NOT EXISTS pgcrypto WITH SCHEMA public;"]);
    assert_eq!(ext, Extension { name: "pgcrypto".to_string(), schema: Some("public".to_string()) });
}

#[test]
fn extension_without_a_schema_clause() {
    let ext = parse_extension(&["CREATE EXTENSION IF NOT EXISTS pgcrypto;"]);
    assert_eq!(ext, Extension { name: "pgcrypto".to_string(), schema: None });
}

/// The one option this parse reads, in the four shapes a file can put it
/// in — and the two that must not be mistaken for it.
#[test]
fn create_collation_reads_determinism_and_nothing_else() {
    fn parse(stmt: &str) -> CollationDef {
        match classify_statement(stmt) {
            Some(StatementShape::Collation(def)) => def,
            other => panic!("{stmt:?} classified as {other:?}"),
        }
    }

    // The committed fixture shape: an option list with no determinism
    // clause, which is the server's default and so a stated `true`.
    assert_eq!(
        parse("CREATE COLLATION public.c_collation (provider = libc, locale = 'C');"),
        CollationDef { name: "public.c_collation".to_string(), deterministic: true }
    );
    // What `pg_dump` writes for a non-deterministic one (I42): the clause
    // sits between the provider and the locale, so the scan cannot key on
    // position.
    assert_eq!(
        parse(
            "CREATE COLLATION public.nd (provider = icu, deterministic = false,                  locale = 'und-u-ks-level2');"
        ),
        CollationDef { name: "public.nd".to_string(), deterministic: false }
    );
    // Keyword and value are both case-insensitive, as every other
    // keyword in this grammar is.
    assert!(
        !parse("CREATE COLLATION public.nd (PROVIDER = icu, DETERMINISTIC = FALSE);").deterministic
    );
    // The copy form carries no option list at all.
    assert_eq!(
        parse(r#"CREATE COLLATION public.c FROM "C";"#),
        CollationDef { name: "public.c".to_string(), deterministic: true }
    );

    // An ICU locale is an arbitrary string the server does not re-quote,
    // so the option split has to respect quoting or a locale could spell
    // the clause. `split_top_level_commas` is what makes this hold.
    assert!(
            parse(
                "CREATE COLLATION public.tricky (provider = icu,                  locale = 'und, deterministic = false');"
            )
            .deterministic
        );
    // `deterministic = true` is spellable by hand and is not the clause.
    assert!(
        parse("CREATE COLLATION public.plain (provider = icu, deterministic = true);")
            .deterministic
    );
}

/// I29's own dump, and the four characters a name may hold that the
/// statement grammar also uses — a paren, a comma, a run of spaces and a
/// comment opener — plus a constraint keyword: each declaration is kept
/// whole, its definition under the one spelling a lookup compares.
#[test]
fn a_type_name_needing_quotes_survives_the_statement_grammar() {
    for (statement, name) in [
        (r#"CREATE DOMAIN s."d[3]" AS integer;"#, r#"s."d[3]""#),
        (r#"CREATE DOMAIN s."my type" AS integer;"#, r#"s."my type""#),
        (r#"CREATE TYPE s."weird[]" AS ENUM ('a', 'b');"#, r#"s."weird[]""#),
        (r#"CREATE DOMAIN s."x ARRAY" AS integer;"#, r#"s."x ARRAY""#),
        (r#"CREATE DOMAIN s."Mood" AS integer;"#, r#"s."Mood""#),
        (r#"CREATE DOMAIN S.Mood AS integer;"#, "s.mood"),
    ] {
        assert_eq!(parse_type(&[statement]).name, name, "{statement}");
    }
    let (_, columns) = parse_table(&[
        "CREATE TABLE s.t (",
        r#"    a s."weird[]","#,
        r#"    b s."my type","#,
        r#"    c s."x ARRAY","#,
        r#"    d s."d[3]","#,
        r#"    e s."weird[]"[],"#,
        r#"    f s."my type"[],"#,
        r#"    g s."x ARRAY"[],"#,
        r#"    h s."a)" NOT NULL,"#,
        r#"    i s."a,b" DEFAULT 'x',"#,
        r#"    j s."two  spaces","#,
        r#"    k s."a NOT b","#,
        r#"    l s."a/*b*/c" /* dummy */"#,
        ");",
    ]);
    let declared: Vec<&str> = columns.iter().map(|c| c.declared_type.as_str()).collect();
    assert_eq!(
        declared,
        [
            r#"s."weird[]""#,
            r#"s."my type""#,
            r#"s."x ARRAY""#,
            r#"s."d[3]""#,
            r#"s."weird[]"[]"#,
            r#"s."my type"[]"#,
            r#"s."x ARRAY"[]"#,
            r#"s."a)""#,
            r#"s."a,b""#,
            r#"s."two  spaces""#,
            r#"s."a NOT b""#,
            r#"s."a/*b*/c""#,
        ]
    );
    let composite = parse_type(&[r#"CREATE TYPE s.c AS (x s."a,b", y s."p(q)");"#]);
    assert_eq!(
        composite.kind,
        TypeKind::Composite {
            fields: Some(vec![
                ColumnDef::new("x", r#"s."a,b""#),
                ColumnDef::new("y", r#"s."p(q)""#),
            ])
        }
    );
    let range = parse_type(&[
        r#"CREATE TYPE s."R" AS RANGE (subtype = integer, multirange_type_name = S."R_multi");"#,
    ]);
    assert_eq!(range.name, r#"s."R""#);
    assert!(matches!(
        range.kind,
        TypeKind::Range { multirange_type_name: Some(ref m), .. } if m == r#"s."R_multi""#
    ));
}

/// A collation's name is kept as a type's is, so a quoted one — `pg_dump`
/// quotes any name that is not a plain lower-case identifier — reads back
/// as the name the column's `COLLATE` clause states (I42).
#[test]
fn a_quoted_collation_name_keeps_its_case() {
    let Some(StatementShape::Collation(def)) = classify_statement(
        r#"CREATE COLLATION public."CI" (provider = icu, deterministic = false, locale = 'und-u-ks-level2');"#,
    ) else {
        panic!("not a collation");
    };
    assert_eq!(def, CollationDef { name: r#"public."CI""#.to_string(), deterministic: false });
}
