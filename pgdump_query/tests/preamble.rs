//! Preamble/metadata extraction against real `pg_dump` output — the
//! `fixture_schema_types.sql` fixtures across all three routine versions and
//! flag sets (`docs/design/architecture.md`, "The preamble grammar and `DumpMetadata`"). Unlike `pgdump_query/src/preamble.rs`'s unit tests (hand-written
//! statement text), this pins the parser against what `pg_dump` actually
//! emits.

use std::path::{Path, PathBuf};

use pgdump_query::preamble::{TypeDef, TypeKind};
use pgdump_query::{DatabaseMetadata, LocalFileSource, ScanOptions, build_index};

fn types_fixture(version: u32, flag_set: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(version.to_string())
        .join("types")
        .join(format!("{flag_set}.sql"))
}

fn edge_cases_fixture(version: u32, flag_set: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(version.to_string())
        .join("edge_cases")
        .join(format!("{flag_set}.sql"))
}

/// Two copies of `edge_cases/create.sql`, concatenated: a real
/// `\connect`-delimited multi-database dump, the shape `pg_dumpall` and
/// hand-concatenated dump files produce ("Multi-database dumps" in
/// `docs/design/architecture.md`). `--create` is the only
/// flag combination in the fixture matrix that emits a `\connect` at all
/// (plain `pg_dump` never does), so it's the only one two copies of can be
/// concatenated into this shape.
///
/// The second copy has its database name changed so the two `\connect`
/// targets are distinguishable: `pgdq_fixture` (the fixture generator's
/// fixed `DB_NAME`) appears nowhere in a `--create` dump except in the
/// `CREATE DATABASE`/`ALTER DATABASE`/`\connect` lines naming it, so a
/// literal string replace is safe and needs no real second Postgres
/// instance. The rest of the schema — every table, type, and row — is
/// identical between the two, which is deliberate: it means a table name
/// like `public.widgets` genuinely collides across databases, exercising
/// `resolve.rs::database_for`'s first-match behavior and `table_stream`'s
/// cross-database matching (both currently un-scoped by database — see
/// `docs/status/STATUS.md`, "Decisions worth a second look") against a real
/// dump instead of only hand-written unit input.
fn multidb_fixture(version: u32) -> (tempfile::TempDir, PathBuf) {
    let content = std::fs::read_to_string(edge_cases_fixture(version, "create")).unwrap();
    let renamed = content.replace("pgdq_fixture", "pgdq_fixture_2");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("multidb.sql");
    std::fs::write(&path, format!("{content}{renamed}")).unwrap();
    (dir, path)
}

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
                ("id".to_string(), "integer".to_string()),
                ("v_smallint".to_string(), "smallint".to_string()),
                ("v_integer".to_string(), "integer".to_string()),
                ("v_bigint".to_string(), "bigint".to_string()),
            ],
            "pg_dump {version}"
        );

        let numeric_cols = db.tables.get("public.t_numeric").unwrap();
        assert_eq!(
            numeric_cols,
            &vec![
                ("id".to_string(), "integer".to_string()),
                ("v_typed".to_string(), "numeric(38,10)".to_string()),
                ("v_typed39".to_string(), "numeric(39,10)".to_string()),
                ("v_small".to_string(), "numeric(10,2)".to_string()),
                ("v_untyped".to_string(), "numeric".to_string()),
            ],
            "pg_dump {version}"
        );

        let text_cols = db.tables.get("public.t_text").unwrap();
        assert_eq!(
            text_cols[1..],
            [
                ("v_text".to_string(), "text".to_string()),
                ("v_varchar".to_string(), "character varying(10)".to_string()),
                ("v_char".to_string(), "character(10)".to_string()),
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
            TypeKind::Domain { base_type: "integer".to_string() },
            "pg_dump {version}"
        );
        assert_eq!(
            find_type(&db, "public.derived_domain").kind,
            TypeKind::Domain { base_type: "public.base_domain".to_string() },
            "pg_dump {version}"
        );

        // Composite.
        assert_eq!(
            find_type(&db, "public.point2d").kind,
            TypeKind::Composite {
                fields: Some(vec![
                    ("x".to_string(), "integer".to_string()),
                    ("y".to_string(), "text".to_string()),
                ])
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
                    ("label".to_string(), "text".to_string()),
                    ("tags".to_string(), "text[]".to_string()),
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
            TypeKind::Domain { base_type: "box".to_string() },
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
                ("v_multidim".to_string(), "integer[]".to_string()),
                ("v_mixed_dim".to_string(), "integer[]".to_string()),
                ("v_lbound".to_string(), "integer[]".to_string()),
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
                ("v_box_domain".to_string(), "public.box_domain".to_string()),
                ("v_box_domain_array".to_string(), "public.box_domain[]".to_string()),
            ],
            "pg_dump {version}"
        );

        // A user-defined type used as a column's declared type is recorded
        // schema-qualified (I8), matching the type's own name.
        let enum_domain_cols = db.tables.get("public.t_enum_domain").unwrap();
        assert_eq!(enum_domain_cols[1].1, "public.mood");
        assert_eq!(enum_domain_cols[2].1, "public.derived_domain");
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
                ("id".to_string(), "integer".to_string()),
                ("name".to_string(), "text".to_string()),
                ("description".to_string(), "text".to_string()),
                ("is_active".to_string(), "boolean".to_string()),
                ("created_at".to_string(), "timestamp with time zone".to_string()),
            ],
            "pg_dump {version}"
        );

        // I5: a regular (non-binary-upgrade) dump omits a dropped column
        // from the DDL entirely — the DDL here has exactly the same 3 live
        // columns the COPY header lists, no dummy placeholder.
        assert_eq!(
            db.tables.get("public.dropped_column").unwrap(),
            &vec![
                ("id".to_string(), "integer".to_string()),
                ("keep_me".to_string(), "text".to_string()),
                ("also_keep".to_string(), "boolean".to_string()),
            ],
            "pg_dump {version}"
        );

        // Generated columns are declared in the DDL (unlike dropped ones)
        // but never appear in the COPY column list — the DDL's declared
        // type is still exactly what a by-name lookup should find.
        let generated = db.tables.get("public.generated_column").unwrap();
        assert_eq!(generated.last().unwrap(), &("total".to_string(), "integer".to_string()));
    }
}

#[tokio::test]
async fn edge_cases_binary_upgrade_dump_recreates_the_dropped_column_as_a_dummy() {
    for version in [13, 16, 18] {
        let db = single_database(&edge_cases_fixture(version, "binary-upgrade")).await;
        let cols = db.tables.get("public.dropped_column").unwrap();
        assert_eq!(cols[0], ("id".to_string(), "integer".to_string()));
        assert_eq!(cols[1], ("keep_me".to_string(), "text".to_string()));
        // I5: the mangled, quoted placeholder name and the C-comment-suffixed
        // dummy type, exactly as real `pg_dump --binary-upgrade` writes them.
        assert_eq!(cols[2], ("........pg.dropped.3........".to_string(), "INTEGER".to_string()));
        assert_eq!(cols[3], ("also_keep".to_string(), "boolean".to_string()));
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
