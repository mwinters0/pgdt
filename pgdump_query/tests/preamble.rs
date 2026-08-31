//! Preamble/metadata extraction against real `pg_dump` output — the
//! `fixture_schema_types.sql` fixtures across all three routine versions and
//! flag sets (`docs/design/architecture.md`, "The preamble grammar and `DumpMetadata`"). Unlike `pgdump_query/src/preamble.rs`'s unit tests (hand-written
//! statement text), this pins the parser against what `pg_dump` actually
//! emits.

use std::path::Path;

use pgdump_query::preamble::{ColumnDef, TypeDef, TypeKind};
use pgdump_query::{DatabaseMetadata, LocalFileSource, ScanOptions, build_index};

mod common;
use common::{edge_cases_fixture, multidb_fixture, types_fixture};

async fn single_database(path: &Path) -> DatabaseMetadata {
    let source = LocalFileSource::open(path).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    let mut metadata = index.metadata.expect("build_index always populates metadata");
    assert_eq!(metadata.databases.len(), 1, "no fixture here is a multi-database dump");
    metadata.databases.pop().unwrap()
}

fn find_type<'a>(db: &'a DatabaseMetadata, name: &str) -> &'a TypeDef {
    db.types.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no type named {name}"))
}

#[tokio::test]
async fn default_dump_declares_every_mapped_column_type() {
    for version in [13, 16, 18] {
        let db = single_database(&types_fixture(version, "default")).await;
        assert!(db.preamble_complete);
        assert_eq!(db.name, None, "pg_dump {version}: no --create, no \\connect");
        assert!(db.server_version.is_some(), "pg_dump {version}");
        assert!(db.pg_dump_version.is_some(), "pg_dump {version}");
        assert!(db.extensions.is_empty(), "pg_dump {version}: the types schema defines none");

        let int_cols = db.tables.get("public.t_int").unwrap_or_else(|| panic!("pg_dump {version}"));
        assert_eq!(
            int_cols,
            &vec![
                ColumnDef::new("id", "integer"),
                ColumnDef::new("v_smallint", "smallint"),
                ColumnDef::new("v_integer", "integer"),
                ColumnDef::new("v_bigint", "bigint"),
            ],
            "pg_dump {version}"
        );

        let numeric_cols = db.tables.get("public.t_numeric").unwrap();
        assert_eq!(
            numeric_cols,
            &vec![
                ColumnDef::new("id", "integer"),
                ColumnDef::new("v_typed", "numeric(38,10)"),
                ColumnDef::new("v_typed39", "numeric(39,10)"),
                ColumnDef::new("v_small", "numeric(10,2)"),
                ColumnDef::new("v_untyped", "numeric"),
            ],
            "pg_dump {version}"
        );

        let text_cols = db.tables.get("public.t_text").unwrap();
        assert_eq!(
            text_cols[1..],
            [
                ColumnDef::new("v_text", "text"),
                ColumnDef::new("v_varchar", "character varying(10)"),
                ColumnDef::new("v_char", "character(10)"),
            ],
            "pg_dump {version}"
        );

        // Enum: fully determined by the DDL body in the non-binary-upgrade
        // shape.
        assert_eq!(
            find_type(&db, "public.mood").kind,
            TypeKind::Enum {
                labels: vec![
                    "sad".to_string(),
                    "ok".to_string(),
                    "happy".to_string(),
                    "has space".to_string(),
                    "has,comma".to_string(),
                    "has'quote".to_string(),
                ]
            },
            "pg_dump {version}"
        );

        // Domain over a domain (transitive resolution is `pgtype.rs`'s job;
        // this just needs the immediate base type recorded).
        assert_eq!(
            find_type(&db, "public.base_domain").kind,
            TypeKind::domain("integer"),
            "pg_dump {version}"
        );
        assert_eq!(
            find_type(&db, "public.derived_domain").kind,
            TypeKind::domain("public.base_domain"),
            "pg_dump {version}"
        );

        // Composite.
        assert_eq!(
            find_type(&db, "public.point2d").kind,
            TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("x", "integer"), ColumnDef::new("y", "text"),])
            },
            "pg_dump {version}"
        );

        // A composite whose own field is an array — the nested shape the
        // nested-literal codec is parameterized for. The field's declared
        // type keeps its `[]` exactly as `format_type` wrote it.
        assert_eq!(
            find_type(&db, "public.tagged").kind,
            TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("label", "text"),
                    ColumnDef::new("tags", "text[]"),
                ])
            },
            "pg_dump {version}"
        );

        // A composite with no fields at all. `pg_dump` writes the body as an
        // empty parenthesized block over two lines, which reaches the grammar
        // as an empty field list rather than as a parse failure — the
        // distinction the field list's `Option` carries, and what lets this
        // map to a zero-field `Struct` while an unparseable body refuses the
        // column.
        assert_eq!(
            find_type(&db, "public.empty_comp").kind,
            TypeKind::Composite { fields: Some(vec![]) },
            "pg_dump {version}"
        );

        // A domain over `box`, the one built-in whose array delimiter is `;`
        // (I22). The DDL records the base type's *name* and nothing about its
        // delimiter, which is exactly why an element-type refusal has to run
        // after the domain walk rather than over the declared spelling.
        assert_eq!(
            find_type(&db, "public.box_domain").kind,
            TypeKind::domain("box"),
            "pg_dump {version}"
        );

        // A user-defined range over a non-numeric subtype, carrying a
        // `collation` parameter the range grammar must step over without
        // mistaking it for the subtype. On PG14+ a `multirange_type_name`
        // parameter sits between the two (I10).
        assert_eq!(
            find_type(&db, "public.textrange").kind,
            TypeKind::Range {
                subtype: Some("text".to_string()),
                multirange_type_name: (version >= 14).then(|| "public.textmultirange".to_string()),
            },
            "pg_dump {version}"
        );

        // I21, as pg_dump actually writes it: `integer[][]` in the DDL comes
        // back as plain `integer[]`, indistinguishable from the column beside
        // it that holds 1-D values. This is the whole reason an array
        // column's Arrow type cannot be settled from the declared type.
        let shape_cols = db.tables.get("public.t_array_shape").unwrap();
        assert_eq!(
            shape_cols[1..],
            [
                ColumnDef::new("v_multidim", "integer[]"),
                ColumnDef::new("v_mixed_dim", "integer[]"),
                ColumnDef::new("v_lbound", "integer[]"),
            ],
            "pg_dump {version}"
        );

        // I28, as pg_dump actually writes it: the four array-declaration
        // spellings nothing else in the tree covers all come back plain
        // `integer[]`. The bracket count and the bounds are discarded by the
        // parser, never reach `pg_type`, and so cannot reach the file — which
        // is why the spellings themselves are pinned by unit test over a
        // hand-built type list, and this table pins the collapse.
        let spelling_cols = db.tables.get("public.t_array_spelling").unwrap();
        assert_eq!(
            spelling_cols[1..],
            [
                ColumnDef::new("v_bounded", "integer[]"),
                ColumnDef::new("v_bounded_2d", "integer[]"),
                ColumnDef::new("v_array_kw", "integer[]"),
                ColumnDef::new("v_array_kw_n", "integer[]"),
            ],
            "pg_dump {version}"
        );

        // Neither the domain-over-`box` column nor its array says anything
        // about the `;` delimiter its values are actually written with — the
        // declared strings are indistinguishable from any other domain's.
        let delim_cols = db.tables.get("public.t_delimiter").unwrap();
        assert_eq!(
            delim_cols[1..],
            [
                ColumnDef::new("v_box_domain", "public.box_domain"),
                ColumnDef::new("v_box_domain_array", "public.box_domain[]"),
            ],
            "pg_dump {version}"
        );

        // A user-defined type used as a column's declared type is recorded
        // schema-qualified (I8), matching the type's own name.
        let enum_domain_cols = db.tables.get("public.t_enum_domain").unwrap();
        assert_eq!(enum_domain_cols[1].declared_type, "public.mood");
        assert_eq!(enum_domain_cols[2].declared_type, "public.derived_domain");
    }
}

/// I11: `pg_dump` writes a completed C-level base type as *two* statements
/// under one name — the shell first, then the definition. The metadata is
/// keyed on the type, not the statement, so the completion wins and the pair
/// leaves one entry. A hand-built type list cannot show this; only a real
/// dump emits the pair at all, which is why this test reads the fixture.
#[tokio::test]
async fn a_completed_base_type_leaves_one_entry_and_the_shell_does_not_win() {
    for version in [13, 16, 18] {
        let db = single_database(&types_fixture(version, "default")).await;

        let mybase: Vec<&TypeDef> = db.types.iter().filter(|t| t.name == "public.mybase").collect();
        assert_eq!(
            mybase.len(),
            1,
            "pg_dump {version}: the SHELL TYPE/TYPE pair must leave one entry, got {mybase:?}"
        );
        assert_eq!(
            mybase[0].kind,
            TypeKind::Base,
            "pg_dump {version}: the completion must win over the shell"
        );

        // A type that never got a completion keeps its shell kind.
        assert_eq!(find_type(&db, "public.shellonly").kind, TypeKind::Shell, "pg_dump {version}");

        // The rule is per name, not special-cased to `mybase`.
        let mut names: Vec<&str> = db.types.iter().map(|t| t.name.as_str()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "pg_dump {version}: one entry per type, not per statement");
    }
}

#[tokio::test]
async fn binary_upgrade_dump_yields_the_same_enum_labels_via_alter_type() {
    for version in [13, 16, 18] {
        let db = single_database(&types_fixture(version, "binary-upgrade")).await;
        assert_eq!(
            find_type(&db, "public.mood").kind,
            TypeKind::Enum {
                labels: vec![
                    "sad".to_string(),
                    "ok".to_string(),
                    "happy".to_string(),
                    "has space".to_string(),
                    "has,comma".to_string(),
                    "has'quote".to_string(),
                ]
            },
            "pg_dump {version}: binary-upgrade's split CREATE TYPE/ALTER TYPE \
             ADD VALUE shape (I6) must fold back to the same labels, same order, \
             as the plain form"
        );
        // The binary-upgrade OID-preservation noise between every object's
        // TOC comment and its real statement (I6) must not corrupt the
        // table declarations that follow it.
        assert!(db.tables.contains_key("public.t_int"));
    }
}

#[tokio::test]
async fn data_only_dump_has_no_ddl_but_still_reports_versions() {
    for version in [13, 16, 18] {
        let db = single_database(&types_fixture(version, "data-only")).await;
        assert!(db.tables.is_empty(), "pg_dump {version}: --data-only has no DDL at all");
        assert!(db.types.is_empty(), "pg_dump {version}");
        assert!(db.server_version.is_some(), "pg_dump {version}");
        assert!(db.pg_dump_version.is_some(), "pg_dump {version}");
    }
}

#[tokio::test]
async fn edge_cases_default_dump_declares_widgets_and_the_dropped_generated_tables() {
    for version in [13, 16, 18] {
        let db = single_database(&edge_cases_fixture(version, "default")).await;
        assert_eq!(
            db.tables.get("public.widgets").unwrap(),
            &vec![
                ColumnDef::new("id", "integer"),
                ColumnDef::new("name", "text"),
                ColumnDef::new("description", "text"),
                ColumnDef::new("is_active", "boolean"),
                ColumnDef::new("created_at", "timestamp with time zone"),
            ],
            "pg_dump {version}"
        );

        // I5: a regular (non-binary-upgrade) dump omits a dropped column
        // from the DDL entirely — the DDL here has exactly the same 3 live
        // columns the COPY header lists, no dummy placeholder.
        assert_eq!(
            db.tables.get("public.dropped_column").unwrap(),
            &vec![
                ColumnDef::new("id", "integer"),
                ColumnDef::new("keep_me", "text"),
                ColumnDef::new("also_keep", "boolean"),
            ],
            "pg_dump {version}"
        );

        // Generated columns are declared in the DDL (unlike dropped ones)
        // but never appear in the COPY column list — the DDL's declared
        // type is still exactly what a by-name lookup should find.
        let generated = db.tables.get("public.generated_column").unwrap();
        assert_eq!(generated.last().unwrap(), &ColumnDef::new("total", "integer"));
    }
}

#[tokio::test]
async fn edge_cases_binary_upgrade_dump_recreates_the_dropped_column_as_a_dummy() {
    for version in [13, 16, 18] {
        let db = single_database(&edge_cases_fixture(version, "binary-upgrade")).await;
        let cols = db.tables.get("public.dropped_column").unwrap();
        assert_eq!(cols[0], ColumnDef::new("id", "integer"));
        assert_eq!(cols[1], ColumnDef::new("keep_me", "text"));
        // I5: the mangled, quoted placeholder name and the C-comment-suffixed
        // dummy type, exactly as real `pg_dump --binary-upgrade` writes them.
        assert_eq!(cols[2], ColumnDef::new("........pg.dropped.3........", "INTEGER"));
        assert_eq!(cols[3], ColumnDef::new("also_keep", "boolean"));
    }
}

/// Real fixture coverage for a concatenated/`pg_dumpall`-shaped
/// multi-database dump, as opposed to the hand-written `\connect` input in
/// `pgdump_query/src/preamble.rs`'s unit tests.
#[tokio::test]
async fn concatenated_create_dumps_yield_two_named_databases_each_fully_parsed() {
    for version in [13, 16, 18] {
        let (_dir, path) = multidb_fixture(version);
        let source = LocalFileSource::open(&path).unwrap();
        let index = build_index(&source, &ScanOptions::default()).await.unwrap();

        // I2: this is the only shape in which one qualified table name owns
        // more than one `COPY` block — one per database, both present in the
        // flat block list.
        let widget_blocks: Vec<_> = index.blocks_for("public.widgets").collect();
        assert_eq!(widget_blocks.len(), 2, "pg_dump {version}");
        assert_eq!(index.total_rows(), 2 * 144, "pg_dump {version}: doubled row count");

        let metadata = index.metadata.expect("build_index always populates metadata");
        assert_eq!(metadata.databases.len(), 2, "pg_dump {version}");
        assert_eq!(
            metadata.databases[0].name.as_deref(),
            Some("pgdq_fixture"),
            "pg_dump {version}"
        );
        assert_eq!(
            metadata.databases[1].name.as_deref(),
            Some("pgdq_fixture_2"),
            "pg_dump {version}"
        );

        for db in &metadata.databases {
            assert!(db.preamble_complete, "pg_dump {version}: {:?}", db.name);
            assert!(db.server_version.is_some(), "pg_dump {version}: {:?}", db.name);
            assert!(db.pg_dump_version.is_some(), "pg_dump {version}: {:?}", db.name);
        }
        // Same source schema copied verbatim into both `\connect` segments —
        // the DDL itself (everything but the database name) must come out
        // identical.
        assert_eq!(metadata.databases[0].tables, metadata.databases[1].tables, "pg_dump {version}");
        assert!(metadata.databases[0].tables.contains_key("public.widgets"), "pg_dump {version}");
    }
}
