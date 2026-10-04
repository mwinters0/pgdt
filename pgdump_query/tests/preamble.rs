//! Preamble/metadata extraction against real `pg_dump` output — the
//! `fixture_schema_types.sql` fixtures across all three routine versions and
//! flag sets (`docs/design/decisions.md`, "D36"). Unlike `pgdump_query/src/preamble.rs`'s unit tests (hand-written
//! statement text), this pins the parser against what `pg_dump` actually
//! emits.

use std::path::Path;

use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::preamble::{CollationDef, ColumnDef, NotNull, TypeDef, TypeKind};
use pgdump_query::resolve::ColumnResolution;
use pgdump_query::{
    DatabaseMetadata, DumpMetadata, LocalFileSource, QueryOptions, ResolvedSchema, ScanOptions,
    StatisticsRequest, build_index, cache, map_file, table_stream,
};

mod common;
use common::{VERSIONS, edge_cases_fixture, fixture, multidb_fixture, sandboxed, types_fixture};

/// `column`, declared `NOT NULL` with a constraint its children take.
fn not_null(column: ColumnDef) -> ColumnDef {
    ColumnDef { not_null: Some(NotNull::Inherited), ..column }
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

        let int_cols = db
            .tables
            .get("public.t_int")
            .map(|t| &t.columns)
            .unwrap_or_else(|| panic!("pg_dump {version}"));
        assert_eq!(
            int_cols,
            &vec![
                not_null(ColumnDef::new("id", "integer")),
                ColumnDef::new("v_smallint", "smallint"),
                ColumnDef::new("v_integer", "integer"),
                ColumnDef::new("v_bigint", "bigint"),
            ],
            "pg_dump {version}"
        );

        let numeric_cols = db.tables.get("public.t_numeric").map(|t| &t.columns).unwrap();
        assert_eq!(
            numeric_cols,
            &vec![
                not_null(ColumnDef::new("id", "integer")),
                ColumnDef::new("v_typed", "numeric(38,10)"),
                ColumnDef::new("v_typed39", "numeric(39,10)"),
                ColumnDef::new("v_small", "numeric(10,2)"),
                ColumnDef::new("v_untyped", "numeric"),
            ],
            "pg_dump {version}"
        );

        let text_cols = db.tables.get("public.t_text").map(|t| &t.columns).unwrap();
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
            TypeKind::exact_enum([
                "sad".to_string(),
                "ok".to_string(),
                "happy".to_string(),
                "has space".to_string(),
                "has,comma".to_string(),
                "has'quote".to_string(),
            ]),
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
            TypeKind::Domain {
                base_type: "public.base_domain".to_string(),
                collation: None,
                not_null: true,
                check: false,
            },
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
                // No fixture range declares a `canonical` function: one must
                // be written against the shell type, which a SQL function
                // cannot take, so it would need a C or internal-language
                // function in the fixture schema (I46).
                canonical: None,
            },
            "pg_dump {version}"
        );

        // I21, as pg_dump actually writes it: `integer[][]` in the DDL comes
        // back as plain `integer[]`, indistinguishable from the column beside
        // it that holds 1-D values. This is the whole reason an array
        // column's Arrow type cannot be settled from the declared type.
        let shape_cols = db.tables.get("public.t_array_shape").map(|t| &t.columns).unwrap();
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
        let spelling_cols = db.tables.get("public.t_array_spelling").map(|t| &t.columns).unwrap();
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
        let delim_cols = db.tables.get("public.t_delimiter").map(|t| &t.columns).unwrap();
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
        let enum_domain_cols = db.tables.get("public.t_enum_domain").map(|t| &t.columns).unwrap();
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
            TypeKind::exact_enum([
                "sad".to_string(),
                "ok".to_string(),
                "happy".to_string(),
                "has space".to_string(),
                "has,comma".to_string(),
                "has'quote".to_string(),
            ]),
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

/// `public.t_collate`'s whole column list, at every major, with each
/// `COLLATE` clause exactly as the dump wrote it.
///
/// This is where the *displaced* clause is asserted. `pg_dump` appends
/// `COLLATE` after `DEFAULT`/`GENERATED` and after `NOT NULL` (I37), whatever
/// the input said, so a parser that read the token following the type words
/// would find `DEFAULT` on `v_text_def` and `GENERATED` on `v_gen_nn`. All
/// six majors write these twelve lines byte for byte alike.
///
/// **`v_gen_nn` can be asserted nowhere else.** It is `STORED` generated, so
/// its DDL is written and the `COPY` column list omits it — a declared column
/// with no data column, which no `TableStream` can filter or project. It is
/// also the tree's only fragment stacking all three displacers, and the only
/// real-dump stress on `extract_collation`'s paren- and quote-aware scan: the
/// clause sits outside `upper(COALESCE(v_src, ''::text))`, past two nesting
/// levels and a quoted literal.
///
/// `v_user` and `v_nd` are the user-collation reference form —
/// schema-qualified, unquoted, outside `pg_catalog`. Kept verbatim, because
/// that is L1's rule and because `pg_catalog."en_US.utf8"` above them has a
/// dot *inside* the quoted name, so a pre-split `schema.name` pair would be
/// ambiguous. The two are spelled alike here and answer differently only
/// because of what the *statements* beside them say, which is the join
/// [`the_types_schema_declares_one_deterministic_and_one_non_deterministic_collation`]
/// pins the other half of.
#[tokio::test]
async fn t_collate_carries_its_collate_clause_wherever_pg_dump_displaced_it() {
    fn collated(name: &str, declared_type: &str, collation: &str) -> ColumnDef {
        ColumnDef {
            name: name.to_string(),
            declared_type: declared_type.to_string(),
            collation: Some(collation.to_string()),
            not_null: None,
        }
    }
    let not_null = |column: ColumnDef| ColumnDef { not_null: Some(NotNull::Inherited), ..column };

    for version in VERSIONS {
        let db = single_database(&types_fixture(version, "default")).await;
        assert_eq!(
            db.tables.get("public.t_collate").map(|t| &t.columns).unwrap(),
            &vec![
                not_null(ColumnDef::new("id", "integer")),
                collated("v_text_c", "text", r#"pg_catalog."C""#),
                collated("v_text_locale", "text", r#"pg_catalog."en_US.utf8""#),
                collated("v_text_ucs", "text", "pg_catalog.ucs_basic"),
                ColumnDef::new("v_name", "name"),
                ColumnDef::new("v_domain_c", "public.text_c"),
                ColumnDef::new("v_pair", "public.collated_pair"),
                collated("v_text_def", "text", r#"pg_catalog."C""#),
                collated("v_user", "text", "public.c_collation"),
                collated("v_nd", "text", "public.nd_collation"),
                ColumnDef::new("v_src", "text"),
                not_null(collated("v_gen_nn", "text", r#"pg_catalog."C""#)),
            ],
            "pg_dump {version}"
        );
    }
}

/// `public.t_v18_columns`, the `types` schema's v18 sidecar: the three shapes
/// v18 writes between a column's type and its displaced `COLLATE` clause —
/// `CONSTRAINT <name> NOT NULL`, `NOT NULL NO INHERIT`, and a virtual
/// `GENERATED ALWAYS AS (expr)` with no `STORED` (I37) — each read to its
/// type and its clause. The virtual columns are declared and never in `COPY`
/// (I5), so, like `v_gen_nn` above, this is the one place they are asserted;
/// below 18 the table does not exist.
#[tokio::test]
async fn v18_s_column_shapes_keep_their_type_and_their_displaced_collation() {
    let c = r#"pg_catalog."C""#;
    let collated = |name: &str, not_null| ColumnDef {
        name: name.to_string(),
        declared_type: "text".to_string(),
        collation: Some(c.to_string()),
        not_null,
    };
    for version in VERSIONS {
        let db = single_database(&types_fixture(version, "default")).await;
        let table = db.tables.get("public.t_v18_columns").map(|t| &t.columns);
        if version < 18 {
            assert_eq!(table, None, "pg_dump {version}: the sidecar loads at 18 alone");
            continue;
        }
        assert_eq!(
            table.unwrap(),
            &vec![
                ColumnDef { not_null: Some(NotNull::Inherited), ..ColumnDef::new("id", "integer") },
                collated("v_named", Some(NotNull::Inherited)),
                collated("v_no_inherit", Some(NotNull::NoInherit)),
                collated("v_virtual", None),
                ColumnDef::new("v_virtual_len", "integer"),
                ColumnDef::new("v_after", "date"),
            ],
            "pg_dump {version}"
        );
    }
}

/// The two `CREATE COLLATION`s the `types` schema declares, read at every
/// major — and **both determinism answers come off committed bytes**.
///
/// `pg_dump` writes `, deterministic = false` only where the catalog says the
/// collation is non-deterministic, and unconditionally where it does (I42).
/// So the pair is the whole of what a dump can state: `public.c_collation` is
/// `libc`, which the server refuses to make anything but deterministic, and
/// `public.nd_collation` is `provider = icu, deterministic = false`, which no
/// other provider can spell. All six majors write both statements byte for
/// byte alike, option order included — provider, determinism, locale — which
/// is `dumpCollation`'s own append order rather than the fixture's.
///
/// **Neither line carries a `version =`** — the append is inside
/// `if (dopt->binary_upgrade)` (I42), which is why an ICU collation could
/// enter the tree at all. Which flag set does carry it, and that the six
/// majors agree on the value, is
/// [`the_icu_collversion_reaches_binary_upgrade_alone_and_agrees_across_majors`]
/// rather than a claim made here.
///
/// Each name is kept verbatim and schema-qualified, exactly as `v_user`'s and
/// `v_nd`'s `COLLATE` clauses above spell them — which is the join
/// `crate::pgtype`'s register makes between the two.
#[tokio::test]
async fn the_types_schema_declares_one_deterministic_and_one_non_deterministic_collation() {
    for version in VERSIONS {
        let db = single_database(&types_fixture(version, "default")).await;
        assert_eq!(
            db.collations,
            vec![
                CollationDef { name: "public.c_collation".to_string(), deterministic: true },
                CollationDef { name: "public.nd_collation".to_string(), deterministic: false },
            ],
            "pg_dump {version}"
        );
    }
}

/// The ICU `collversion` in the fixture tree, **guarded rather than merely
/// tolerated** — read off the fixture text, since the parser deliberately
/// drops the field (`CollationDef` keeps a name and a determinism answer and
/// nothing else).
///
/// `dumpCollation` appends `version = '<collversion>'` inside
/// `if (dopt->binary_upgrade)` (I42), so one committed fixture byte per major
/// is an environment release number that moves when an image pin moves. That
/// much is established practice here — every fixture header already carries a
/// Debian package revision, and `fixtures/<v>/oracle/meta.tsv` commits glibc's
/// `default_collversion` on purpose — and this test is what watches this
/// byte the same way.
///
/// **The assertion is agreement, never the literal**, which is what
/// `scripts/oracle_differences.py` already demands of `default_collversion`.
/// All six majors read one version because all six image pins are `-trixie`
/// and so resolve to one libicu; the documented drift is *across* base images
/// (`und-x-icu` is `153.128` on `13.23-alpine` against `153.136` on
/// `18.6-alpine`), so a split here means the pins have drifted apart, which is
/// the fault worth reporting. Naming the value would instead fire on every
/// routine image bump — a deliberate regeneration whose diff already shows the
/// change, and a check that fires on the expected event is a signal that is
/// always on.
///
/// The shape claim is the other half, made concrete here rather than in
/// [`the_types_schema_declares_one_deterministic_and_one_non_deterministic_collation`]'s
/// prose: the version is absent from `default`, absent from
/// `data-only` — which emits no `CREATE COLLATION` at all — and absent from
/// `public.c_collation` under every flag set, a `C` libc collation having no
/// `collversion` to record.
#[test]
fn the_icu_collversion_reaches_binary_upgrade_alone_and_agrees_across_majors() {
    /// Each `CREATE COLLATION` in one fixture, as (name, everything after it).
    fn create_collations(version: u32, flag_set: &str) -> Vec<(String, String)> {
        let path = types_fixture(version, flag_set);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        text.lines()
            .filter_map(|line| line.strip_prefix("CREATE COLLATION "))
            .map(|rest| {
                let (name, options) = rest.split_once(' ').unwrap_or((rest, ""));
                (name.to_string(), options.to_string())
            })
            .collect()
    }

    /// The `version = '…'` option's value, wherever the option is present.
    fn collversion(options: &str) -> Option<&str> {
        let after = options.split_once("version = '")?.1;
        Some(after.split_once('\'').expect("pg_dump closes the quote it opened").0)
    }

    let mut found: Vec<(u32, String)> = Vec::new();
    for version in VERSIONS {
        for flag_set in ["default", "data-only", "binary-upgrade"] {
            let statements = create_collations(version, flag_set);
            if flag_set == "data-only" {
                assert!(
                    statements.is_empty(),
                    "pg_dump {version} data-only: a data-only dump emits no DDL, so there is \
                     no statement for a collversion to reach"
                );
                continue;
            }

            let names: Vec<&str> = statements.iter().map(|(name, _)| name.as_str()).collect();
            assert_eq!(
                names,
                ["public.c_collation", "public.nd_collation"],
                "pg_dump {version} {flag_set}"
            );
            assert_eq!(
                collversion(&statements[0].1),
                None,
                "pg_dump {version} {flag_set}: a `C` libc collation has no collversion"
            );

            match (flag_set, collversion(&statements[1].1)) {
                ("default", None) => {}
                ("binary-upgrade", Some(version_option)) => {
                    found.push((version, version_option.to_string()));
                }
                (_, got) => panic!(
                    "pg_dump {version} {flag_set}: nd_collation's collversion is {got:?} — \
                     the append is gated on --binary-upgrade (I42)"
                ),
            }
        }
    }

    assert_eq!(found.len(), VERSIONS.len(), "one binary-upgrade fixture per major");
    let (first_major, first) = &found[0];
    for (major, other) in &found[1..] {
        assert_eq!(
            other, first,
            "the ICU collversion is {first} on pg_dump {first_major} and {other} on {major} — \
             the base-image pins have drifted apart"
        );
    }
}

#[tokio::test]
async fn edge_cases_default_dump_declares_widgets_and_the_dropped_generated_tables() {
    for version in [13, 16, 18] {
        let db = single_database(&edge_cases_fixture(version, "default")).await;
        assert_eq!(
            db.tables.get("public.widgets").map(|t| &t.columns).unwrap(),
            &vec![
                not_null(ColumnDef::new("id", "integer")),
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
            db.tables.get("public.dropped_column").map(|t| &t.columns).unwrap(),
            &vec![
                not_null(ColumnDef::new("id", "integer")),
                ColumnDef::new("keep_me", "text"),
                ColumnDef::new("also_keep", "boolean"),
            ],
            "pg_dump {version}"
        );

        // Generated columns are declared in the DDL (unlike dropped ones)
        // but never appear in the COPY column list — the DDL's declared
        // type is still exactly what a by-name lookup should find.
        let generated = db.tables.get("public.generated_column").map(|t| &t.columns).unwrap();
        assert_eq!(generated.last().unwrap(), &ColumnDef::new("total", "integer"));
    }
}

/// A `CREATE TABLE` list holds fragments that are no column, and a comma that
/// is no separator: the parent's inline `CHECK`, at 18 the child's table-level
/// `NOT NULL label` (`dumpTableSchema` writes one for an inherited column with
/// a local not-null constraint), and a default's `ARRAY[now(), now()]`, whose
/// far side once read as a column `now` that the real one resolved through.
/// Each table declares exactly the columns `pg_dump` wrote, at every major.
#[tokio::test]
async fn emitters_tables_declare_their_columns_and_nothing_else() {
    for version in VERSIONS {
        let db = single_database(&fixture(version, "emitters", "default")).await;
        let declared = |table: &str| db.tables.get(table).unwrap().columns.clone();
        assert_eq!(
            declared("emitters.parent"),
            vec![
                not_null(ColumnDef::new("id", "integer")),
                ColumnDef::new("label", "text"),
                ColumnDef::new("born", "date"),
            ],
            "pg_dump {version}"
        );
        assert_eq!(
            declared("emitters.child"),
            vec![ColumnDef::new("extra", "numeric(6,2)")],
            "pg_dump {version}"
        );
        assert_eq!(
            declared("emitters.stamped"),
            vec![
                not_null(ColumnDef::new("id", "integer")),
                ColumnDef::new("stamps", "timestamp with time zone[]"),
                ColumnDef::new("now", "integer"),
            ],
            "pg_dump {version}"
        );
    }
}

/// An inheritance child and a typed table declare every column they hold,
/// wherever `pg_dump` wrote it: the parent's and the type's through the
/// `INHERITS` and `OF` clauses, and under `--binary-upgrade`, where the full
/// list is the table's own, through the `ALTER TABLE ONLY` forms after it.
/// Each column in the server's order, as its `COPY` header lists it, and
/// each reads typed. **Each is `NOT NULL` exactly where the server holds it
/// so** (I76): the child's `id` through its parent, its `label` by the `SET
/// NOT NULL` before 18 and the table-level `NOT NULL label` from 18, the
/// typed table's `name` by its column option — or, under `--binary-upgrade`,
/// on each column.
#[tokio::test]
async fn inherited_and_typed_columns_are_declared_through_their_references() {
    let child = [("id", "integer"), ("label", "text"), ("born", "date"), ("extra", "numeric(6,2)")];
    let people = [("name", "text"), ("born", "date"), ("height", "integer")];
    let not_null = [
        ("emitters.child", &[true, true, false, false][..]),
        ("emitters.people", &[true, false, false]),
    ];
    for version in VERSIONS {
        for flag_set in ["default", "binary-upgrade"] {
            let path = fixture(version, "emitters", flag_set);
            let db = single_database(&path).await;
            let label = format!("pg_dump {version} {flag_set}");
            assert_eq!(db.tables["emitters.child"].parents, ["emitters.parent"], "{label}");
            assert_eq!(
                db.tables["emitters.people"].of_type.as_deref(),
                Some("emitters.person"),
                "{label}"
            );
            for (table, expected) in [("emitters.child", &child[..]), ("emitters.people", &people)]
            {
                let declared: Vec<(&str, &str)> = db
                    .declared_columns(table)
                    .iter()
                    .map(|c| (c.name.as_str(), c.declared_type.as_str()))
                    .collect();
                assert_eq!(declared, expected, "{label}: {table}");
                let resolved = typed_schema(&path, table).await;
                let names: Vec<&str> =
                    resolved.schema.fields().iter().map(|f| f.name().as_str()).collect();
                let expected_names: Vec<&str> = expected.iter().map(|(name, _)| *name).collect();
                assert_eq!(names, expected_names, "{label}: {table}");
                assert!(
                    resolved.columns.iter().all(|c| *c == ColumnResolution::Mapped),
                    "{label}: {table} resolves {:?}",
                    resolved.columns
                );
                let (_, expected) = not_null.iter().find(|(t, _)| *t == table).unwrap();
                assert_eq!(resolved.not_null, *expected, "{label}: {table}");
            }
        }
    }
}

/// The schema a typed read of `table` commits to, every row read.
async fn typed_schema(path: &Path, table: &str) -> ResolvedSchema {
    let source = LocalFileSource::open(path).unwrap();
    let mut stream = table_stream(
        &source,
        table,
        ScanOptions::default(),
        QueryOptions::default(),
        None,
        CacheMode::DISABLED,
    );
    while let Some(batch) = stream.next().await {
        batch.unwrap();
    }
    stream.resolved_schema()
}

#[tokio::test]
async fn edge_cases_binary_upgrade_dump_recreates_the_dropped_column_as_a_dummy() {
    for version in [13, 16, 18] {
        let db = single_database(&edge_cases_fixture(version, "binary-upgrade")).await;
        let cols = db.tables.get("public.dropped_column").map(|t| &t.columns).unwrap();
        assert_eq!(cols[0], not_null(ColumnDef::new("id", "integer")));
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
            Some("pgdt_fixture"),
            "pg_dump {version}"
        );
        assert_eq!(
            metadata.databases[1].name.as_deref(),
            Some("pgdt_fixture_2"),
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

/// Each database of a real `pg_dumpall` keeps the version pair its own
/// `pg_dump` wrote (I9), read from the raw file: the segment's `\connect` and
/// the `-- Dumped from`/`-- Dumped by` lines nearest it, before it under
/// `--create` and after it for `template1` and `postgres`. `template1` holds no
/// `COPY` block, so the database after it is the case a segment's data once
/// decided. Asserted over the eager index and over `map_file`, the scan `pgdt
/// parse` saves and `pgdt info` reads.
#[tokio::test]
async fn every_dumpall_database_keeps_its_own_version_headers() {
    fn expected(text: &str) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        let (mut server, mut by) = (None::<String>, None::<String>);
        let mut awaiting: Option<String> = None;
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("-- Dumped from database version ") {
                server = Some(v.to_string());
            } else if let Some(v) = line.strip_prefix("-- Dumped by pg_dump version ") {
                by = Some(v.to_string());
                if let Some(db) = awaiting.take() {
                    out.push((db, server.take().unwrap(), by.take().unwrap()));
                }
            } else if let Some(db) = line.strip_prefix("\\connect ") {
                let db = db.split_whitespace().next().unwrap().to_string();
                match (server.take(), by.take()) {
                    (Some(s), Some(b)) => out.push((db, s, b)),
                    _ => awaiting = Some(db),
                }
            }
        }
        out
    }
    fn versions(metadata: &DumpMetadata) -> Vec<(String, String, String)> {
        metadata
            .databases
            .iter()
            .map(|db| {
                (
                    db.name.clone().unwrap(),
                    db.server_version.clone().unwrap_or_default(),
                    db.pg_dump_version.clone().unwrap_or_default(),
                )
            })
            .collect()
    }

    for version in VERSIONS {
        let fixture = edge_cases_fixture(version, "dumpall");
        let want = expected(&std::fs::read_to_string(&fixture).unwrap());
        assert_eq!(want.len(), 4, "pg_dump {version}: four databases, each with its pair");
        assert_eq!(want[0].0, "template1", "pg_dump {version}");

        let source = LocalFileSource::open(&fixture).unwrap();
        let index = build_index(&source, &ScanOptions::default()).await.unwrap();
        assert_eq!(versions(index.metadata.as_ref().unwrap()), want, "pg_dump {version}: eager");

        let (_dir, dump) = sandboxed(&fixture, "dumpall.sql");
        let source = LocalFileSource::open(&dump).unwrap();
        let mode = CacheMode::enabled(cache::colocated_path(&dump));
        let mapped = map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA)
            .await
            .unwrap()
            .index;
        assert_eq!(versions(mapped.metadata.as_ref().unwrap()), want, "pg_dump {version}: mapped");
    }
}

/// `dumpTableSchema` writes an unlogged table and a foreign table as `CREATE
/// UNLOGGED TABLE` and `CREATE FOREIGN TABLE`, and each declares its columns
/// as a plain table does: the unlogged one, with and without
/// `--binary-upgrade`, and the foreign one where `--include-foreign-data`
/// writes its rows, each reading typed; and the foreign one with no rows,
/// whose `--binary-upgrade` list carries its dropped column's placeholder as a
/// table's does (I5).
#[tokio::test]
async fn unlogged_and_foreign_tables_declare_their_columns() {
    let scratch = [("id", "integer"), ("at", "date"), ("ok", "boolean")];
    let imported = [("id", "integer"), ("born", "date"), ("label", "text")];
    for version in VERSIONS {
        for (schema, flag_set, table, expected) in [
            ("emitters", "default", "emitters.scratch", &scratch[..]),
            ("emitters", "binary-upgrade", "emitters.scratch", &scratch),
            ("objects", "include-foreign-data", "objects.imported", &imported),
        ] {
            let path = fixture(version, schema, flag_set);
            let label = format!("pg_dump {version} {schema}/{flag_set}: {table}");
            let db = single_database(&path).await;
            let declared: Vec<(&str, &str)> = db
                .declared_columns(table)
                .iter()
                .map(|c| (c.name.as_str(), c.declared_type.as_str()))
                .collect();
            assert_eq!(declared, expected, "{label}");
            let resolved = typed_schema(&path, table).await;
            assert!(
                resolved.columns.iter().all(|c| *c == ColumnResolution::Mapped),
                "{label} resolves {:?}",
                resolved.columns
            );
        }
        for (flag_set, expected) in [
            ("default", &[("id", "integer"), ("label", "text")][..]),
            (
                "binary-upgrade",
                &[
                    ("id", "integer"),
                    ("........pg.dropped.2........", "INTEGER"),
                    ("label", "text"),
                ],
            ),
        ] {
            let db = single_database(&fixture(version, "emitters", flag_set)).await;
            let declared: Vec<(&str, &str)> = db
                .declared_columns("emitters.external")
                .iter()
                .map(|c| (c.name.as_str(), c.declared_type.as_str()))
                .collect();
            assert_eq!(declared, expected, "pg_dump {version} {flag_set}");
        }
    }
}

/// `--binary-upgrade` recreates a composite's dropped attribute as a
/// placeholder field and drops it with `ALTER TYPE … DROP ATTRIBUTE`, so the
/// type holds the fields `record_out` writes, and a column of it reads every
/// row typed, as it does without the flag.
#[tokio::test]
async fn a_composite_s_dropped_attribute_is_dropped_under_binary_upgrade() {
    for version in VERSIONS {
        for flag_set in ["default", "binary-upgrade"] {
            let path = fixture(version, "emitters", flag_set);
            let label = format!("pg_dump {version} {flag_set}");
            let db = single_database(&path).await;
            assert_eq!(
                find_type(&db, "emitters.trio").kind,
                TypeKind::Composite {
                    fields: Some(
                        vec![ColumnDef::new("a", "integer"), ColumnDef::new("c", "date"),]
                    )
                },
                "{label}"
            );
            let resolved = typed_schema(&path, "emitters.trios").await;
            assert!(
                resolved.columns.iter().all(|c| *c == ColumnResolution::Mapped),
                "{label} resolves {:?}",
                resolved.columns
            );
        }
    }
}

/// A database whose name holds a byte outside `[A-Za-z0-9_.]` is entered by
/// `\connect -reuse-previous=on "dbname='…'"`, and is a database of its own:
/// listed under its name, with its version pair, holding its table and its
/// table's block, none of which the database before it holds.
#[tokio::test]
async fn a_connection_string_connect_opens_its_own_database() {
    for version in VERSIONS {
        let source = LocalFileSource::open(fixture(version, "emitters", "dumpall")).unwrap();
        let index = build_index(&source, &ScanOptions::default()).await.unwrap();
        let metadata = index.metadata.as_ref().unwrap();
        let names: Vec<Option<&str>> =
            metadata.databases.iter().map(|db| db.name.as_deref()).collect();
        let at = names
            .iter()
            .position(|n| *n == Some("pgdt-emitters"))
            .unwrap_or_else(|| panic!("pg_dump {version}: databases {names:?}"));
        let db = &metadata.databases[at];
        assert!(db.server_version.is_some() && db.pg_dump_version.is_some(), "pg_dump {version}");
        let declared: Vec<(&str, &str)> = db
            .declared_columns("public.named")
            .iter()
            .map(|c| (c.name.as_str(), c.declared_type.as_str()))
            .collect();
        assert_eq!(
            declared,
            [("id", "integer"), ("label", "text"), ("born", "date")],
            "pg_dump {version}"
        );
        assert!(metadata.databases[at - 1].tables.is_empty(), "pg_dump {version}: {names:?}");
        let blocks: Vec<Option<&str>> =
            index.blocks_for("public.named").map(|b| b.database.as_deref()).collect();
        assert_eq!(blocks, [Some("pgdt-emitters")], "pg_dump {version}");
    }
}

/// A `--create` dump `\connect`s its database again after its `DATABASE
/// PROPERTIES` entry (I58), and that reconnect continues the database rather
/// than opening a second, empty one of the same name ahead of it: each name is
/// listed once, and a table declared after the reconnect resolves typed.
/// `dumpall-binary-upgrade` reconnects to every `--create` database,
/// `dumpall-clean` to `template1`.
#[tokio::test]
async fn a_reconnect_after_database_properties_continues_its_database() {
    for version in VERSIONS {
        for flags in ["dumpall-binary-upgrade", "dumpall-clean"] {
            let path = fixture(version, "emitters", flags);
            let source = LocalFileSource::open(&path).unwrap();
            let index = build_index(&source, &ScanOptions::default()).await.unwrap();
            let names: Vec<Option<&str>> = index
                .metadata
                .as_ref()
                .unwrap()
                .databases
                .iter()
                .map(|db| db.name.as_deref())
                .collect();
            let mut unique = names.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), names.len(), "pg_dump {version} {flags}: {names:?}");
            let resolved = typed_schema(&path, "emitters.tuned").await;
            assert!(
                resolved.columns.iter().all(|c| *c == ColumnResolution::Mapped),
                "pg_dump {version} {flags}: {:?}",
                resolved.columns
            );
        }
    }
}

/// **What a strict parse leaves unchecked in the `emitters` schema is named
/// per table, at every major and in both forms a dump declares it**
/// (`strict_unchecked`; I77): each base type's column, the range declaring a
/// canonical function, the domain declaring a `CHECK`, the parent's `CHECK`
/// and the child's, inherited by default and re-declared under
/// `--binary-upgrade` — and nothing in any other table. A `--data-only` dump
/// declares no table, so no field of any is checked.
#[tokio::test]
async fn a_strict_parse_names_what_it_leaves_unchecked_in_the_emitters_schema() {
    use pgdump_query::{Unchecked, strict_unchecked};
    fn base<'a>(column: &'a str, declared: &str) -> (&'a str, String, Unchecked) {
        (column, declared.to_string(), Unchecked::BaseType)
    }
    for version in VERSIONS {
        for flag_set in ["default", "binary-upgrade", "data-only"] {
            let path = fixture(version, "emitters", flag_set);
            let source = LocalFileSource::open(&path).unwrap();
            let index = build_index(&source, &ScanOptions::default()).await.unwrap();
            let label = format!("pg_dump {version} {flag_set}");
            assert!(index.blocks().next().is_some(), "{label}");
            for block in index.blocks() {
                let table = block.header.qualified_name();
                let unchecked = strict_unchecked(
                    &block.header,
                    index.metadata.as_ref(),
                    block.database.as_deref(),
                );
                if flag_set == "data-only" {
                    assert!(!unchecked.declared, "{label}: {table}");
                    assert!(unchecked.columns.is_empty() && unchecked.checks.is_empty());
                    continue;
                }
                assert!(unchecked.declared, "{label}: {table}");
                let columns: Vec<(&str, String, Unchecked)> = unchecked
                    .columns
                    .iter()
                    .map(|c| {
                        assert_eq!(c.path, "", "{label}: {table}");
                        (c.column.as_str(), c.declared.clone(), c.why.clone())
                    })
                    .collect();
                let checks: Vec<(Option<&str>, Option<&str>)> = unchecked
                    .checks
                    .iter()
                    .map(|c| (c.name.as_deref(), c.inherited_from.as_deref()))
                    .collect();
                let positive = Some("parent_id_positive");
                let (expected_columns, expected_checks) = match table.as_str() {
                    "emitters.base_values" => (
                        vec![
                            base("v_varchar", "emitters.bt_varchar(8)"),
                            base("v_char", "emitters.bt_char"),
                            base("v_int2", "emitters.bt_int2"),
                            base("v_main", "emitters.bt_text_main"),
                            base("v_pair", "emitters.bt_pair"),
                        ],
                        vec![],
                    ),
                    "emitters.range_values" => (
                        vec![(
                            "v_canon",
                            "emitters.r_canon".to_string(),
                            Unchecked::RangeCanonical {
                                function: "emitters.r_canon_canonical".into(),
                            },
                        )],
                        vec![],
                    ),
                    "emitters.domain_values" => (
                        vec![(
                            "v_positive",
                            "emitters.positive".to_string(),
                            Unchecked::DomainCheck,
                        )],
                        vec![],
                    ),
                    "emitters.parent" => (vec![], vec![(positive, None)]),
                    "emitters.child" if flag_set == "binary-upgrade" => {
                        (vec![], vec![(positive, None)])
                    }
                    "emitters.child" => (vec![], vec![(positive, Some("emitters.parent"))]),
                    _ => (vec![], vec![]),
                };
                assert_eq!(columns, expected_columns, "{label}: {table}");
                assert_eq!(checks, expected_checks, "{label}: {table}");
            }
        }
    }
}
