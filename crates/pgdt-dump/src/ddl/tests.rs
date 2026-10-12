use super::*;
use pgdt_model::ColumnDef;

/// Feed `lines` (joined with `\n`, the same way `crate::map::Builder`
/// accumulates a statement) to [`classify_statement`] and unwrap the
/// result — the DDL grammar exercised directly, nothing about it
/// depending on a database-segmenting builder.
fn parse(lines: &[&str]) -> StatementShape {
    classify_statement(&lines.join("\n")).unwrap()
}

pub(super) fn parse_table(lines: &[&str]) -> (String, Vec<ColumnDef>) {
    let (name, definition) = parse_table_def(lines);
    (name, definition.columns)
}

pub(super) fn parse_table_def(lines: &[&str]) -> (String, TableDef) {
    match parse(lines) {
        StatementShape::Table { name, definition } => (name, definition),
        other => panic!("expected a Table shape, got {other:?}"),
    }
}

pub(super) fn parse_type(lines: &[&str]) -> TypeDef {
    match parse(lines) {
        StatementShape::Type(def) => def,
        other => panic!("expected a Type shape, got {other:?}"),
    }
}

pub(super) fn parse_extension(lines: &[&str]) -> Extension {
    match parse(lines) {
        StatementShape::Extension(ext) => ext,
        other => panic!("expected an Extension shape, got {other:?}"),
    }
}

/// Both of `appendPsqlMetaConnect`'s forms name their database, the
/// connection string's escapes and the identifier's doubled `"` undone;
/// a connection string holding more than the name is no boundary.
#[test]
fn a_connect_names_its_database_in_either_form() {
    assert_eq!(parse_connect("\\connect koji").as_deref(), Some("koji"));
    assert_eq!(parse_connect("\\connect \"Koji\"").as_deref(), Some("Koji"));
    assert_eq!(
        parse_connect("\\connect -reuse-previous=on \"dbname='pgdt-emitters'\"").as_deref(),
        Some("pgdt-emitters")
    );
    assert_eq!(
        parse_connect(r#"\connect -reuse-previous=on "dbname='it\'s ""a\\b"" db'""#).as_deref(),
        Some(r#"it's "a\b" db"#)
    );
    assert_eq!(
        parse_connect("\\connect -reuse-previous=on \"dbname='a' host=b\"").as_deref(),
        None
    );
    assert_eq!(parse_connect("\\encoding SQL_ASCII"), None);
}

#[test]
fn classify_statement_recognizes_the_four_span_shapes() {
    assert!(matches!(
        classify_statement("CREATE TABLE public.t (id integer);"),
        Some(StatementShape::Table { .. })
    ));
    assert!(matches!(
        classify_statement("CREATE TYPE public.mood AS ENUM ('sad');"),
        Some(StatementShape::Type(_))
    ));
    assert!(matches!(
        classify_statement("CREATE EXTENSION pgcrypto;"),
        Some(StatementShape::Extension(_))
    ));
    assert!(matches!(
        classify_statement("CREATE COLLATION public.c (provider = libc, locale = 'C');"),
        Some(StatementShape::Collation(_))
    ));
    assert!(classify_statement("ALTER TABLE t OWNER TO postgres;").is_none());
    assert!(
        classify_statement("CREATE TABLESPACE fast OWNER postgres LOCATION '/srv/fast';").is_none()
    );
    assert!(classify_statement("CREATE TYPES x;").is_none());
    assert!(classify_statement("ALTER TYPE t ADD VALUE 'x';").is_none());
    // `ALTER COLLATION ... OWNER TO` follows every `CREATE COLLATION` in
    // a real dump and must not be mistaken for one.
    assert!(classify_statement("ALTER COLLATION public.c OWNER TO postgres;").is_none());
}
