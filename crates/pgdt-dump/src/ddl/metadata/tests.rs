use super::*;
use crate::ddl::parse_alter_type_drop_attribute_body;
use crate::ddl::tests::parse_type;
use pgdt_model::{CollationDef, ColumnDef, NotNull, TableDef, TableReference};

/// The `--binary-upgrade` fold (I6) is [`dump_metadata_from_spans`]'s
/// job, not `classify`'s, needing an already-open `TypeDef` to
/// fold the label into: a `TypeDef` span for the empty enum, an unrelated
/// `Unparsed` span for the `binary_upgrade_set_next_pg_enum_oid` noise
/// every real dump interleaves (I6), and two `AlterTypeAddValue` spans.
#[test]
fn binary_upgrade_enum_labels_arrive_via_alter_type() {
    let spans = vec![
        span(SpanBody::TypeDef {
            name: "public.mood".to_string(),
            kind: TypeKind::Enum { labels: Vec::new(), exact: true },
        }),
        span(SpanBody::Unparsed),
        span(SpanBody::AlterTypeAddValue {
            type_name: "public.mood".to_string(),
            label: "sad".to_string(),
        }),
        span(SpanBody::Unparsed),
        span(SpanBody::AlterTypeAddValue {
            type_name: "public.mood".to_string(),
            label: "has'quote".to_string(),
        }),
    ];
    let meta = dump_metadata_from_spans(&spans);
    assert_eq!(
        meta.databases[0].types[0].kind,
        TypeKind::Enum { labels: vec!["sad".to_string(), "has'quote".to_string()], exact: true }
    );
}

/// `--binary-upgrade`'s placeholder for a composite's dropped attribute is
/// dropped again by name, quoted as `fmtId` quotes it; the statement
/// carrying anything more is no such drop.
#[test]
fn a_dropped_attribute_s_placeholder_leaves_its_composite() {
    assert_eq!(
        parse_alter_type_drop_attribute_body(
            "emitters.trio DROP ATTRIBUTE \"........pg.dropped.2........\";"
        ),
        Some(("emitters.trio".to_string(), "........pg.dropped.2........".to_string()))
    );
    assert_eq!(parse_alter_type_drop_attribute_body("public.t DROP ATTRIBUTE b CASCADE;"), None);
    assert_eq!(parse_alter_type_drop_attribute_body("public.t ADD VALUE 'x';"), None);

    let TypeDef { name, kind } = parse_type(&[
        "CREATE TYPE emitters.trio AS (",
        "\ta integer,",
        "\t\"........pg.dropped.2........\" INTEGER /* dummy */,",
        "\tc date",
        ");",
    ]);
    let spans = vec![
        span(SpanBody::TypeDef { name, kind }),
        span(SpanBody::Unparsed),
        span(SpanBody::AlterTypeDropAttribute {
            type_name: "emitters.trio".to_string(),
            attribute: "........pg.dropped.2........".to_string(),
        }),
    ];
    let meta = dump_metadata_from_spans(&spans);
    assert_eq!(
        meta.databases[0].types[0].kind,
        TypeKind::Composite {
            fields: Some(vec![ColumnDef::new("a", "integer"), ColumnDef::new("c", "date")])
        }
    );
}

/// `--binary-upgrade`'s references arrive after the full column list, as
/// a `SET NOT NULL` before v18 arrives after a list not printing its
/// column, and fold into the table they name, a parent named twice once;
/// one naming a table not declared is dropped rather than declaring one.
#[test]
fn binary_upgrade_table_references_arrive_via_alter_table() {
    let reference = |table: &str, reference: TableReference| {
        span(SpanBody::AlterTableReference { table: table.to_string(), reference })
    };
    let parent = || TableReference::Parent("public.p".to_string());
    let spans = vec![
        span(SpanBody::Table {
            name: "public.c".to_string(),
            definition: TableDef::with_columns(vec![ColumnDef::new("id", "integer")]),
        }),
        span(SpanBody::Unparsed),
        reference("public.c", parent()),
        reference("public.c", parent()),
        reference("public.c", TableReference::OfType("public.t".to_string())),
        reference("public.c", TableReference::NotNull("label".to_string())),
        reference("public.elsewhere", parent()),
    ];
    let meta = dump_metadata_from_spans(&spans);
    let tables = &meta.databases[0].tables;
    assert_eq!(tables.len(), 1);
    assert_eq!(
        tables["public.c"],
        TableDef {
            columns: vec![ColumnDef::new("id", "integer")],
            parents: vec!["public.p".to_string()],
            of_type: Some("public.t".to_string()),
            not_null: vec![("label".to_string(), NotNull::Inherited)],
            checks: Vec::new(),
            partition_of: None,
        }
    );
}

/// A span with placeholder offsets — `dump_metadata_from_spans` never
/// reads `start`/`end`/`database`, only `body`.
fn span(body: SpanBody) -> pgdt_model::Span {
    pgdt_model::Span {
        start: 0,
        end: 0,
        database: None,
        text: None,
        toc: None,
        toc_owned: false,
        body,
    }
}

fn dummy_data_span() -> pgdt_model::Span {
    span(SpanBody::Data(pgdt_model::DataBlock::Copy(pgdt_model::CopyBlock {
        header: pgdt_core::CopyHeader {
            schema: None,
            table: "placeholder".to_string(),
            columns: Vec::new(),
        },
        database: None,
        header_offset: 0,
        data_offset: 0,
        terminator_offset: 0,
        end_offset: 0,
        row_count: 0,
        partition_root: None,
        statistics: None,
        statistics_declined: None,
        ignored_refusals: None,
        checked_in_full: false,
        array_shapes: Some(Vec::new()),
        unrepresentable: Some(Vec::new()),
    })))
}

#[test]
fn plain_dump_has_no_connect_and_names_no_database() {
    let meta = dump_metadata_from_spans(&[span(SpanBody::Table {
        name: "public.t".to_string(),
        definition: TableDef::with_columns(vec![ColumnDef::new("id", "integer")]),
    })]);
    assert_eq!(meta.databases.len(), 1);
    assert_eq!(meta.databases[0].name, None);
    assert!(meta.databases[0].preamble_complete);
}

#[test]
fn connect_starts_a_new_named_database_and_drops_the_preconnect_segment() {
    // `CREATE DATABASE koji ...;` isn't a shape `classify` reads
    // (I5: it never carries a real table or type), so it's `Unparsed`
    // like any other statement this module doesn't model.
    let meta = dump_metadata_from_spans(&[
        span(SpanBody::VersionHeader {
            server_version: Some("16.14".to_string()),
            pg_dump_version: None,
        }),
        span(SpanBody::Unparsed),
        span(SpanBody::Connect { database: "koji".to_string() }),
        span(SpanBody::Table {
            name: "public.t".to_string(),
            definition: TableDef::with_columns(vec![ColumnDef::new("id", "integer")]),
        }),
    ]);

    assert_eq!(meta.databases.len(), 1, "the pre-connect segment must be dropped");
    assert_eq!(meta.databases[0].name.as_deref(), Some("koji"));
    assert!(meta.databases[0].tables.contains_key("public.t"));
    assert_eq!(
        meta.databases[0].server_version.as_deref(),
        Some("16.14"),
        "the version header must survive onto the real database, even though \
             the segment it was read in gets dropped"
    );
}

#[test]
fn a_copy_block_completes_the_current_database_and_a_later_connect_starts_a_new_one() {
    // Realistic shape: a `\connect` always precedes a database's first
    // real table (you can't create one before connecting to it), so the
    // first database here starts via `Connect`, same as the second.
    let meta = dump_metadata_from_spans(&[
        span(SpanBody::Connect { database: "one".to_string() }),
        span(SpanBody::Table { name: "public.a".to_string(), definition: TableDef::default() }),
        dummy_data_span(),
        // Per I1, nothing more should be captured for this database now.
        span(SpanBody::Table {
            name: "public.ignored".to_string(),
            definition: TableDef::default(),
        }),
        span(SpanBody::Connect { database: "two".to_string() }),
        span(SpanBody::Table { name: "public.b".to_string(), definition: TableDef::default() }),
    ]);

    assert_eq!(meta.databases.len(), 2);
    assert_eq!(meta.databases[0].name.as_deref(), Some("one"));
    assert!(meta.databases[0].tables.contains_key("public.a"));
    assert!(!meta.databases[0].tables.contains_key("public.ignored"));
    assert_eq!(meta.databases[1].name.as_deref(), Some("two"));
    assert!(meta.databases[1].tables.contains_key("public.b"));
}

/// I58: a `--create` dump `\connect`s its database again after `DATABASE
/// PROPERTIES`, before any data and with no header pair of its own, and
/// that continues the database rather than opening an empty one.
#[test]
fn a_reconnect_to_the_current_database_continues_its_segment() {
    let meta = dump_metadata_from_spans(&[
        span(SpanBody::VersionHeader {
            server_version: Some("18.6".to_string()),
            pg_dump_version: Some("18.6".to_string()),
        }),
        span(SpanBody::Connect { database: "one".to_string() }),
        span(SpanBody::Table { name: "public.a".to_string(), definition: TableDef::default() }),
        span(SpanBody::Unparsed),
        span(SpanBody::Connect { database: "one".to_string() }),
        span(SpanBody::Table { name: "public.b".to_string(), definition: TableDef::default() }),
        dummy_data_span(),
    ]);

    assert_eq!(meta.databases.len(), 1);
    let db = &meta.databases[0];
    assert_eq!(db.name.as_deref(), Some("one"));
    assert_eq!(db.server_version.as_deref(), Some("18.6"));
    assert!(db.tables.contains_key("public.a") && db.tables.contains_key("public.b"));
}

/// Two dumps of one database concatenated stay two databases: the second
/// `\connect` follows a header pair of its own (I9), or the first's data,
/// and neither is a reconnect inside one invocation (I58).
#[test]
fn a_connect_to_the_current_database_from_another_invocation_opens_a_segment() {
    let header = || {
        span(SpanBody::VersionHeader {
            server_version: Some("18.6".to_string()),
            pg_dump_version: Some("18.6".to_string()),
        })
    };
    let table = |name: &str| {
        span(SpanBody::Table { name: name.to_string(), definition: TableDef::default() })
    };
    let connect = || span(SpanBody::Connect { database: "one".to_string() });
    let after_header = dump_metadata_from_spans(&[
        header(),
        connect(),
        table("public.a"),
        header(),
        connect(),
        table("public.b"),
    ]);
    let after_data = dump_metadata_from_spans(&[
        connect(),
        table("public.a"),
        dummy_data_span(),
        connect(),
        table("public.b"),
    ]);

    for meta in [after_header, after_data] {
        let tables: Vec<Vec<&str>> = meta
            .databases
            .iter()
            .map(|db| db.tables.keys().map(String::as_str).collect())
            .collect();
        assert_eq!(tables, [["public.a"], ["public.b"]]);
    }
}

/// I9: every `--create` segment in a real `pg_dumpall`/concatenated dump
/// carries its own version-header pair ahead of its own `\connect`, not
/// just the first one. Guards against a second database's headers being
/// dropped because they land while `current` is still the first
/// database's already-`preamble_complete` segment
/// (`fixtures/*/edge_cases/create.sql` x2).
#[test]
fn a_later_connect_segment_keeps_its_own_version_headers_too() {
    let meta = dump_metadata_from_spans(&[
        span(SpanBody::VersionHeader {
            server_version: Some("16.14".to_string()),
            pg_dump_version: Some("16.14".to_string()),
        }),
        span(SpanBody::Connect { database: "one".to_string() }),
        span(SpanBody::Table { name: "public.a".to_string(), definition: TableDef::default() }),
        dummy_data_span(),
        span(SpanBody::VersionHeader {
            server_version: Some("16.15".to_string()),
            pg_dump_version: Some("16.15".to_string()),
        }),
        span(SpanBody::Connect { database: "two".to_string() }),
        span(SpanBody::Table { name: "public.b".to_string(), definition: TableDef::default() }),
    ]);

    assert_eq!(meta.databases.len(), 2);
    assert_eq!(meta.databases[0].server_version.as_deref(), Some("16.14"));
    assert_eq!(meta.databases[0].pg_dump_version.as_deref(), Some("16.14"));
    assert_eq!(meta.databases[1].server_version.as_deref(), Some("16.15"));
    assert_eq!(meta.databases[1].pg_dump_version.as_deref(), Some("16.15"));
}

/// I9: `pg_dumpall` writes `\connect template1` itself, its `pg_dump`
/// child's pair following, and `template1` holds no `COPY` block; the
/// next database's pair, ahead of its own `\connect`, is that
/// database's and leaves `template1`'s alone.
#[test]
fn a_segment_with_no_data_keeps_its_headers_and_the_next_gets_its_own() {
    let header = |v: &str| {
        span(SpanBody::VersionHeader {
            server_version: Some(format!("{v}-server")),
            pg_dump_version: Some(format!("{v}-pg_dump")),
        })
    };
    let meta = dump_metadata_from_spans(&[
        span(SpanBody::Connect { database: "template1".to_string() }),
        header("t1"),
        span(SpanBody::Unparsed),
        header("app"),
        span(SpanBody::Unparsed),
        span(SpanBody::Connect { database: "app".to_string() }),
        span(SpanBody::Table { name: "public.a".to_string(), definition: TableDef::default() }),
        dummy_data_span(),
        span(SpanBody::Connect { database: "postgres".to_string() }),
        header("pg"),
    ]);

    let got: Vec<_> = meta
        .databases
        .iter()
        .map(|db| (db.name.as_deref(), db.server_version.as_deref(), db.pg_dump_version.as_deref()))
        .collect();
    assert_eq!(
        got,
        [
            (Some("template1"), Some("t1-server"), Some("t1-pg_dump")),
            (Some("app"), Some("app-server"), Some("app-pg_dump")),
            (Some("postgres"), Some("pg-server"), Some("pg-pg_dump")),
        ]
    );
}

#[test]
fn data_only_dump_has_no_tables_but_still_reports_versions() {
    let meta = dump_metadata_from_spans(&[span(SpanBody::VersionHeader {
        server_version: Some("16.15".to_string()),
        pg_dump_version: Some("16.15".to_string()),
    })]);
    let db = &meta.databases[0];
    assert!(db.tables.is_empty());
    assert!(db.types.is_empty());
    assert_eq!(db.server_version.as_deref(), Some("16.15"));
    assert!(db.preamble_complete);
}

/// A `CREATE COLLATION` reaches [`DumpMetadata`] through its own span, in
/// DDL order and per database — the same route a `CREATE TYPE` takes.
///
/// The leading `\connect` is what makes this two databases rather than
/// one: the *first* one replaces the pre-`\connect` segment rather than
/// closing it, which is [`dump_metadata_from_spans`]'s `--create` rule.
#[test]
fn collations_are_collected_per_database_in_ddl_order() {
    let meta = dump_metadata_from_spans(&[
        span(SpanBody::Connect { database: "one".to_string() }),
        span(SpanBody::Collation {
            collation: CollationDef { name: "public.a".to_string(), deterministic: true },
        }),
        span(SpanBody::Collation {
            collation: CollationDef { name: "public.b".to_string(), deterministic: false },
        }),
        span(SpanBody::Connect { database: "two".to_string() }),
        span(SpanBody::Collation {
            collation: CollationDef { name: "other.c".to_string(), deterministic: false },
        }),
    ]);
    assert_eq!(
        meta.databases[0].collations,
        vec![
            CollationDef { name: "public.a".to_string(), deterministic: true },
            CollationDef { name: "public.b".to_string(), deterministic: false },
        ]
    );
    assert_eq!(
        meta.databases[1].collations,
        vec![CollationDef { name: "other.c".to_string(), deterministic: false }]
    );
}
