use super::*;
use crate::ddl::tests::parse_table;

/// A column `name` of `declared_type` carrying a `NOT NULL` its children
/// take.
fn not_null(name: &str, declared_type: &str) -> ColumnDef {
    ColumnDef { not_null: Some(NotNull::Inherited), ..ColumnDef::new(name, declared_type) }
}

#[test]
fn captures_multi_word_and_parameterized_types() {
    let (_, cols) = parse_table(&[
        "CREATE TABLE public.t (",
        "    a character varying(16) NOT NULL,",
        "    b numeric(38,10),",
        "    c timestamp with time zone DEFAULT now(),",
        "    d public.mood",
        ");",
    ]);
    assert_eq!(cols[0], not_null("a", "character varying(16)"));
    assert_eq!(cols[1], ColumnDef::new("b", "numeric(38,10)"));
    assert_eq!(cols[2], ColumnDef::new("c", "timestamp with time zone"));
    assert_eq!(cols[3], ColumnDef::new("d", "public.mood"));
}

#[test]
fn binary_upgrade_dummy_column_comment_and_quoted_identifier() {
    // I5, confirmed against `fixtures/*/edge_cases/binary-upgrade.sql`:
    // a `--binary-upgrade` dump recreates a dropped column as a dummy
    // typed column, quoting its mangled name and annotating the type
    // with a C-style comment neither of which is ordinary SQL syntax
    // elsewhere in this grammar.
    let (_, cols) = parse_table(&[
        "CREATE TABLE public.dropped_column (",
        "    id integer NOT NULL,",
        "    keep_me text,",
        "    \"........pg.dropped.3........\" INTEGER /* dummy */,",
        "    also_keep boolean",
        ");",
    ]);
    assert_eq!(cols[2], ColumnDef::new("........pg.dropped.3........", "INTEGER"));
}

/// The `COLLATE` clause, in the shape `pg_dump` actually writes it (I37):
/// schema-qualified, the collation name double-quoted, and **after**
/// `DEFAULT` and `NOT NULL` rather than beside the type. That placement
/// is why the clause is found by a scan of the whole fragment rather than
/// by looking at the token after the type words.
#[test]
fn captures_the_collate_clause_pg_dump_writes() {
    let (_, cols) = parse_table(&[
        "CREATE TABLE public.t_collate (",
        "    v_text text,",
        "    v_text_c text COLLATE pg_catalog.\"C\",",
        "    v_text_db text COLLATE pg_catalog.\"en_US.utf8\",",
        "    v_char_c character(10) DEFAULT 'x'::bpchar NOT NULL COLLATE pg_catalog.\"C\",",
        "    v_user text COLLATE public.mycoll",
        ");",
    ]);
    assert_eq!(cols[0], ColumnDef::new("v_text", "text"));
    assert_eq!(cols[1].collation.as_deref(), Some("pg_catalog.\"C\""));
    assert_eq!(cols[2].collation.as_deref(), Some("pg_catalog.\"en_US.utf8\""));
    // The type words still stop at `COLLATE`, wherever the clause sits.
    assert_eq!(cols[3].declared_type, "character(10)");
    assert_eq!(cols[3].collation.as_deref(), Some("pg_catalog.\"C\""));
    assert_eq!(cols[4].collation.as_deref(), Some("public.mycoll"));
}

/// The scan is top-level and quote-aware, so neither a string literal
/// containing the word nor a parenthesized `CHECK` expression using the
/// operator can be mistaken for the column's own clause — `pg_dump`
/// writes both inline.
#[test]
fn a_collate_inside_a_literal_or_an_expression_is_not_the_column_s() {
    let (_, cols) = parse_table(&[
        "CREATE TABLE public.t (",
        "    a text DEFAULT 'collate pg_catalog.\"C\"'::text,",
        "    b text CHECK ((b COLLATE pg_catalog.\"C\") > 'a'::text),",
        "    c text GENERATED ALWAYS AS (upper(a COLLATE pg_catalog.\"C\")) STORED",
        ");",
    ]);
    assert!(cols.iter().all(|c| c.collation.is_none()), "{cols:?}");
}
