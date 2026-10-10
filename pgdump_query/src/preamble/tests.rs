use super::*;
use crate::model::{
    CheckConstraint, CollationDef, ColumnDef, DatabaseMetadata, Extension, NotNull, PartitionOf,
    SpanBody, TableDef, TableReference, TypeDef, TypeKind,
};

/// Feed `lines` (joined with `\n`, the same way `crate::map::Builder`
/// accumulates a statement) to [`classify_statement`] and unwrap the
/// result — the DDL grammar exercised directly, nothing about it
/// depending on a database-segmenting builder.
fn parse(lines: &[&str]) -> StatementShape {
    classify_statement(&lines.join("\n")).unwrap()
}

fn parse_table(lines: &[&str]) -> (String, Vec<ColumnDef>) {
    let (name, definition) = parse_table_def(lines);
    (name, definition.columns)
}

fn parse_table_def(lines: &[&str]) -> (String, TableDef) {
    match parse(lines) {
        StatementShape::Table { name, definition } => (name, definition),
        other => panic!("expected a Table shape, got {other:?}"),
    }
}

fn parse_type(lines: &[&str]) -> TypeDef {
    match parse(lines) {
        StatementShape::Type(def) => def,
        other => panic!("expected a Type shape, got {other:?}"),
    }
}

fn parse_extension(lines: &[&str]) -> Extension {
    match parse(lines) {
        StatementShape::Extension(ext) => ext,
        other => panic!("expected an Extension shape, got {other:?}"),
    }
}

/// A column `name` of `declared_type` carrying a `NOT NULL` its children
/// take.
fn not_null(name: &str, declared_type: &str) -> ColumnDef {
    ColumnDef { not_null: Some(NotNull::Inherited), ..ColumnDef::new(name, declared_type) }
}

#[test]
fn parses_a_simple_table() {
    let (_, columns) = parse_table(&[
        "CREATE TABLE public.t_int (",
        "    id integer NOT NULL,",
        "    v_smallint smallint,",
        "    v_bigint bigint",
        ");",
    ]);
    assert_eq!(
        columns,
        vec![
            not_null("id", "integer"),
            ColumnDef::new("v_smallint", "smallint"),
            ColumnDef::new("v_bigint", "bigint"),
        ]
    );
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

/// Every `TableConstraint` and `TableLikeClause` gram.y admits in a column
/// list is told from a column and dropped, the forms `pg_dump` writes
/// (an inline `CHECK`, 18's table-level `NOT NULL`) and the ones only a
/// hand-written file holds; a column whose name is one of those words
/// stays a column wherever it can be one — quoted, or `exclude` bare,
/// that word being unreserved.
#[test]
fn a_table_constraint_or_like_is_no_column() {
    let (_, cols) = parse_table(&[
        "CREATE TABLE public.t (",
        "    LIKE public.src INCLUDING ALL,",
        "    id integer NOT NULL,",
        "    exclude integer,",
        "    \"constraint\" text,",
        "    \"not\" boolean,",
        "    NOT NULL inherited NO INHERIT,",
        "    CONSTRAINT named_nn NOT NULL inherited,",
        "    CONSTRAINT t_id_positive CHECK ((id > 0)),",
        "    CHECK (id < 10) NO INHERIT,",
        "    UNIQUE (id),",
        "    PRIMARY KEY (id),",
        "    FOREIGN KEY (id) REFERENCES public.other(id),",
        "    EXCLUDE USING gist (id WITH =),",
        "    exclude (id WITH =)",
        ");",
    ]);
    assert_eq!(
        cols,
        vec![
            not_null("id", "integer"),
            ColumnDef::new("exclude", "integer"),
            ColumnDef::new("constraint", "text"),
            ColumnDef::new("not", "boolean"),
        ]
    );
}

/// A comma inside an `ARRAY[…]` default splits nothing, so a later column
/// named like a word inside the brackets keeps its own type.
#[test]
fn a_comma_inside_brackets_splits_no_column() {
    let (_, cols) = parse_table(&[
        "CREATE TABLE public.t (",
        "    stamps timestamp with time zone[] DEFAULT ARRAY[now(), now()],",
        "    counts integer[] DEFAULT ARRAY[1, 2],",
        "    now integer",
        ");",
    ]);
    assert_eq!(
        cols,
        vec![
            ColumnDef::new("stamps", "timestamp with time zone[]"),
            ColumnDef::new("counts", "integer[]"),
            ColumnDef::new("now", "integer"),
        ]
    );
}

/// An inheritance child as `dumpTableSchema` writes it, its own columns
/// in the list and its parents after it, each parent qualified as a table
/// key is — an unquoted part folded, a quoted one kept.
#[test]
fn an_inherits_clause_records_the_parents_in_order() {
    let (name, table) = parse_table_def(&[
        "CREATE TABLE emitters.child (",
        "    NOT NULL label,",
        "    extra numeric(6,2)",
        ")",
        "INHERITS (emitters.parent, \"Other\".Second);",
    ]);
    assert_eq!(name, "emitters.child");
    assert_eq!(
        table,
        TableDef {
            columns: vec![ColumnDef::new("extra", "numeric(6,2)")],
            parents: vec!["emitters.parent".to_string(), "Other.second".to_string()],
            of_type: None,
            not_null: vec![("label".to_string(), NotNull::Inherited)],
            checks: Vec::new(),
            partition_of: None,
        }
    );
    // A child declaring nothing of its own keeps its empty list and its
    // parents.
    let (_, table) = parse_table_def(&["CREATE TABLE public.c (", ")", "INHERITS (public.p);"]);
    assert_eq!((table.columns.len(), table.parents), (0, vec!["public.p".to_string()]));
}

/// A typed table's list is a column's options, never its type — the
/// `NOT NULL` or default `pg_dump` writes beside the name alone — so it
/// declares no column of its own, its type naming them all, and a `NOT
/// NULL` among them is the table's; with no such option there is no list
/// at all.
#[test]
fn a_typed_table_records_its_type_and_declares_no_column() {
    let (name, table) = parse_table_def(&[
        "CREATE TABLE emitters.people OF emitters.person (",
        "    name NOT NULL,",
        "    born DEFAULT '2000-01-01'::date",
        ");",
    ]);
    assert_eq!(name, "emitters.people");
    assert_eq!(
        table,
        TableDef {
            columns: Vec::new(),
            parents: Vec::new(),
            of_type: Some("emitters.person".to_string()),
            not_null: vec![("name".to_string(), NotNull::Inherited)],
            checks: Vec::new(),
            partition_of: None,
        }
    );
    let (_, table) = parse_table_def(&["CREATE TABLE public.t OF PUBLIC.\"Person\";"]);
    assert_eq!(table.of_type.as_deref(), Some("public.\"Person\""));
}

/// The two references `--binary-upgrade` writes after a full column list,
/// the `SET NOT NULL` a dump before v18 writes for a column its list does
/// not print (I76), an added `CHECK` (tested apart), and nothing else an
/// `ALTER TABLE` says: the removals,
/// a statement holding more than the one subcommand, and every other
/// subcommand are none of them.
#[test]
fn only_an_added_parent_type_or_not_null_is_a_table_reference() {
    let parent = |p: &str| Some(TableReference::Parent(p.to_string()));
    let reference = |stmt: &str| parse_alter_table_reference(stmt).map(|(t, r)| (t, Some(r)));
    assert_eq!(
        reference("ALTER TABLE ONLY emitters.child INHERIT emitters.parent;\n"),
        Some(("emitters.child".to_string(), parent("emitters.parent")))
    );
    assert_eq!(
        reference("ALTER FOREIGN TABLE ONLY public.f INHERIT \"P\".p;"),
        Some(("public.f".to_string(), parent("P.p")))
    );
    assert_eq!(
        reference("ALTER TABLE ONLY emitters.people OF emitters.person;"),
        Some((
            "emitters.people".to_string(),
            Some(TableReference::OfType("emitters.person".to_string()))
        ))
    );
    assert_eq!(
        reference("alter table public.c inherit public.p;"),
        Some(("public.c".to_string(), parent("public.p")))
    );
    let not_null = |c: &str| Some(TableReference::NotNull(c.to_string()));
    assert_eq!(
        reference("ALTER TABLE ONLY emitters.child ALTER COLUMN label SET NOT NULL;\n"),
        Some(("emitters.child".to_string(), not_null("label")))
    );
    assert_eq!(
        reference("ALTER FOREIGN TABLE ONLY public.f ALTER \"Label\" SET NOT NULL;"),
        Some(("public.f".to_string(), not_null("Label")))
    );
    for other in [
        "ALTER TABLE ONLY public.c ALTER COLUMN label DROP NOT NULL;",
        "ALTER TABLE ONLY public.c ALTER COLUMN label SET DEFAULT 1;",
        "ALTER TABLE ONLY public.c ALTER COLUMN label SET NOT NULL, ADD COLUMN x integer;",
        "ALTER TABLE ONLY public.c NO INHERIT public.p;",
        "ALTER TABLE ONLY public.t NOT OF;",
        "ALTER TABLE ONLY public.c INHERIT public.p, ADD COLUMN x integer;",
        "ALTER TABLE ONLY public.c ADD CONSTRAINT c_pkey PRIMARY KEY (id);",
        "ALTER TABLE public.c OWNER TO postgres;",
        "ALTER TABLE ONLY public.c INHERIT public.p",
    ] {
        assert_eq!(parse_alter_table_reference(other), None, "{other}");
    }
}

/// A table as [`with_tables`] takes it: name, own columns, parents, `OF`
/// type.
type TableSpec<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a [&'a str], Option<&'a str>);

/// `metadata` holding `tables` and `types`, one database.
fn with_tables(tables: &[TableSpec<'_>], types: Vec<TypeDef>) -> DatabaseMetadata {
    let mut db = DatabaseMetadata::empty(None);
    db.types = types;
    for (name, columns, parents, of_type) in tables {
        db.tables.insert(
            name.to_string(),
            TableDef {
                columns: columns.iter().map(|(c, t)| ColumnDef::new(*c, *t)).collect(),
                parents: parents.iter().map(|p| p.to_string()).collect(),
                of_type: of_type.map(str::to_string),
                not_null: Vec::new(),
                checks: Vec::new(),
                partition_of: None,
            },
        );
    }
    db
}

/// A column the table lacks is found through its references: a parent's,
/// a grandparent's, the first parent's of two declaring it, its own where
/// it declares one too, and its type's field; and in the server's order —
/// each parent's, then the table's own, a column two declare at its first
/// place.
#[test]
fn a_column_is_found_through_parents_and_a_type() {
    let person = TypeDef {
        name: "public.person".to_string(),
        kind: TypeKind::Composite {
            fields: Some(vec![ColumnDef::new("name", "text"), ColumnDef::new("born", "date")]),
        },
    };
    let db = with_tables(
        &[
            ("public.g", &[("gid", "bigint")], &[], None),
            ("public.p1", &[("id", "integer"), ("shared", "text")], &["public.g"], None),
            ("public.p2", &[("shared", "varchar"), ("other", "date")], &[], None),
            (
                "public.c",
                &[("other", "date"), ("own", "boolean")],
                &["public.p1", "public.p2"],
                None,
            ),
            ("public.people", &[], &[], Some("public.person")),
        ],
        vec![person],
    );
    let declared = |table: &str, column: &str| {
        db.declared_column(table, column).map(|c| c.declared_type.as_str())
    };
    assert_eq!(declared("public.c", "own"), Some("boolean"));
    assert_eq!(declared("public.c", "id"), Some("integer"));
    assert_eq!(declared("public.c", "gid"), Some("bigint"));
    assert_eq!(declared("public.c", "shared"), Some("text"));
    assert_eq!(declared("public.c", "absent"), None);
    assert_eq!(declared("public.people", "born"), Some("date"));
    assert_eq!(declared("public.people", "height"), None);
    assert_eq!(declared("public.missing", "id"), None);

    let names = |table: &str| -> Vec<&str> {
        db.declared_columns(table).iter().map(|c| c.name.as_str()).collect()
    };
    assert_eq!(names("public.c"), ["gid", "id", "shared", "other", "own"]);
    assert_eq!(names("public.people"), ["name", "born"]);
    assert!(names("public.missing").is_empty());
    // Each column of the order is the declaration the lookup returns.
    for column in db.declared_columns("public.c") {
        assert_eq!(db.declared_column("public.c", &column.name), Some(column));
    }
}

/// **Every form a `NOT NULL` is declared in is read, and nothing that only
/// spells the words** (I76): on the column, plain, named and v18's `NO
/// INHERIT`, and implied by an inline `PRIMARY KEY`, an identity or a
/// `serial` type; at the
/// table, v18's element and a `PRIMARY KEY` list; on a domain, plain and
/// named. A `CHECK`, a generated expression, a literal, a quoted name and
/// a qualified type's `not` declare none.
#[test]
fn every_form_of_not_null_is_read_and_nothing_else() {
    let (_, table) = parse_table_def(&[
        "CREATE TABLE public.t (",
        "    a integer NOT NULL,",
        "    b text CONSTRAINT t_b_not_null NOT NULL COLLATE pg_catalog.\"C\",",
        "    c text NOT NULL NO INHERIT,",
        "    d integer PRIMARY KEY,",
        "    e bigint GENERATED ALWAYS AS IDENTITY (START WITH 1),",
        "    f boolean CHECK ((f IS NOT NULL)),",
        "    g boolean GENERATED ALWAYS AS ((a IS NOT NULL)) STORED,",
        "    h text DEFAULT 'NOT NULL'::text,",
        "    i public.\"not\" /* NOT NULL */,",
        "    j public.not,",
        "    k integer NULL,",
        "    l bigserial,",
        "    NOT NULL inherited,",
        "    CONSTRAINT own_nn NOT NULL \"Own\" NO INHERIT,",
        "    CONSTRAINT t_pkey PRIMARY KEY (a, \"Key\"),",
        "    CONSTRAINT t_check CHECK ((k IS NOT NULL))",
        ");",
    ]);
    let declared: Vec<(&str, Option<NotNull>)> =
        table.columns.iter().map(|c| (c.name.as_str(), c.not_null)).collect();
    let inherited = Some(NotNull::Inherited);
    assert_eq!(
        declared,
        [
            ("a", inherited),
            ("b", inherited),
            ("c", Some(NotNull::NoInherit)),
            ("d", inherited),
            ("e", inherited),
            ("f", None),
            ("g", None),
            ("h", None),
            ("i", None),
            ("j", None),
            ("k", None),
            ("l", inherited),
        ]
    );
    let at_table = |name: &str, not_null| (name.to_string(), not_null);
    assert_eq!(
        table.not_null,
        [
            at_table("inherited", NotNull::Inherited),
            at_table("Own", NotNull::NoInherit),
            at_table("a", NotNull::Inherited),
            at_table("Key", NotNull::Inherited),
        ]
    );
    for (stmt, not_null) in [
        ("CREATE DOMAIN public.d AS integer NOT NULL;", true),
        ("CREATE DOMAIN public.d AS integer CONSTRAINT d_nn NOT NULL DEFAULT 0;", true),
        (
            "CREATE DOMAIN public.d AS integer CONSTRAINT d_check CHECK ((VALUE IS NOT NULL));",
            false,
        ),
        ("CREATE DOMAIN public.d AS text DEFAULT 'NOT NULL'::text;", false),
    ] {
        let kind = parse_type(&[stmt]).kind;
        assert!(
            matches!(kind, TypeKind::Domain { not_null: n, .. } if n == not_null),
            "{stmt}: {kind:?}"
        );
    }
}

/// **A column is `NOT NULL` where its table declares it so, or any
/// ancestor passes it on** (I76): its own definition, the table's list —
/// v18's element, a typed table's option, a `SET NOT NULL` — and each
/// parent's, a grandparent's through the parent, but for a `NO INHERIT`
/// one, which binds its own table alone. A cycle ends.
#[test]
fn a_not_null_is_found_on_the_table_and_through_its_ancestors() {
    let mut db = with_tables(
        &[
            ("public.g", &[("gid", "bigint")], &[], None),
            ("public.p", &[("id", "integer"), ("own", "text")], &["public.g"], None),
            ("public.c", &[("extra", "date")], &["public.p"], None),
            ("public.people", &[], &[], Some("public.person")),
            ("public.loop", &[("x", "integer")], &["public.loop"], None),
        ],
        Vec::new(),
    );
    let tables = &mut db.tables;
    tables.get_mut("public.g").unwrap().columns[0].not_null = Some(NotNull::Inherited);
    tables.get_mut("public.p").unwrap().not_null.push(("own".into(), NotNull::NoInherit));
    tables.get_mut("public.c").unwrap().not_null.push(("id".into(), NotNull::Inherited));
    tables.get_mut("public.people").unwrap().not_null.push(("name".into(), NotNull::Inherited));
    assert!(db.column_not_null("public.g", "gid"));
    assert!(db.column_not_null("public.p", "gid"));
    assert!(db.column_not_null("public.c", "gid"));
    assert!(db.column_not_null("public.p", "own"));
    assert!(!db.column_not_null("public.c", "own"));
    assert!(db.column_not_null("public.c", "id"));
    assert!(!db.column_not_null("public.p", "id"));
    assert!(!db.column_not_null("public.c", "extra"));
    assert!(db.column_not_null("public.people", "name"));
    assert!(!db.column_not_null("public.people", "born"));
    assert!(!db.column_not_null("public.loop", "x"));
    assert!(!db.column_not_null("public.missing", "id"));
}

/// **Every form a `CHECK` is declared in is read, and nothing that only
/// spells the word** (I77): a table constraint, named, unnamed and `NO
/// INHERIT`; a column's, named or not; a typed table's column option; a
/// domain's; and the `ALTER TABLE … ADD CONSTRAINT … CHECK` a dump writes
/// apart, alone. A literal, a quoted name, a default calling a function
/// named like it and a view's `CHECK OPTION` declare none; nor does an
/// `ALTER TABLE` adding any other constraint, or two.
#[test]
fn every_form_of_check_is_read_and_nothing_else() {
    let check = |name: Option<&str>, no_inherit| CheckConstraint {
        name: name.map(str::to_string),
        no_inherit,
    };
    let (_, table) = parse_table_def(&[
        "CREATE TABLE public.t (",
        "    a integer CHECK ((a > 0)),",
        "    b integer CONSTRAINT \"B_pos\" CHECK ((b > 0)) NOT NULL,",
        "    \"check\" text DEFAULT 'CHECK'::text,",
        "    d integer DEFAULT public.check_d(),",
        "    CONSTRAINT t_a_b CHECK ((a < b)),",
        "    CHECK ((a <> 7)) NO INHERIT,",
        "    CONSTRAINT t_pkey PRIMARY KEY (a)",
        ");",
    ]);
    assert_eq!(
        table.checks,
        [
            check(None, false),
            check(Some("B_pos"), false),
            check(Some("t_a_b"), false),
            check(None, true),
        ]
    );
    assert_eq!(table.columns.len(), 4, "a CHECK costs no column: {:?}", table.columns);
    let (_, typed) = parse_table_def(&[
        "CREATE TABLE public.people OF public.person (",
        "    height WITH OPTIONS CONSTRAINT tall CHECK ((height > 100))",
        ");",
    ]);
    assert_eq!(typed.checks, [check(Some("tall"), false)]);

    let added = |stmt: &str| parse_alter_table_reference(stmt).map(|(_, reference)| reference);
    assert_eq!(
        added(
            "ALTER TABLE ONLY emitters.child ADD CONSTRAINT parent_id_positive CHECK ((id > 0));"
        ),
        Some(TableReference::Check(check(Some("parent_id_positive"), false)))
    );
    assert_eq!(
        added("ALTER TABLE public.t\n    ADD CONSTRAINT t_x CHECK ((x > 0)) NOT VALID;"),
        Some(TableReference::Check(check(Some("t_x"), false)))
    );
    assert_eq!(
        added("ALTER TABLE public.t ADD CHECK ((x > 0)) NO INHERIT;"),
        Some(TableReference::Check(check(None, true)))
    );
    for stmt in [
        "ALTER TABLE ONLY public.t ADD CONSTRAINT t_pkey PRIMARY KEY (id);",
        "ALTER TABLE public.t ADD CONSTRAINT a CHECK ((x > 0)), ADD CONSTRAINT b CHECK ((y > 0));",
        "ALTER TABLE public.t ADD COLUMN \"check\" integer;",
    ] {
        assert_eq!(added(stmt), None, "{stmt}");
    }

    for (stmt, check) in [
        ("CREATE DOMAIN public.d AS integer CONSTRAINT d_check CHECK ((VALUE > 0));", true),
        ("CREATE DOMAIN public.d AS integer CHECK ((VALUE > 0)) NOT NULL;", true),
        ("CREATE DOMAIN public.d AS text DEFAULT 'CHECK'::text;", false),
        ("CREATE DOMAIN public.d AS integer NOT NULL;", false),
    ] {
        let kind = parse_type(&[stmt]).kind;
        assert!(
            matches!(kind, TypeKind::Domain { check: c, .. } if c == check),
            "{stmt}: {kind:?}"
        );
    }
}

/// **A table's rows are held to its own `CHECK`s and every one an ancestor
/// passes on** (I77), each named with the ancestor declaring it: a
/// grandparent's through the parent, but for a `NO INHERIT` one; a name
/// met twice, as `--binary-upgrade`'s child re-declares its parent's, is
/// one constraint, and an unnamed one is never merged. A cycle ends.
#[test]
fn a_check_is_found_on_the_table_and_through_its_ancestors() {
    let mut db = with_tables(
        &[
            ("public.g", &[("gid", "bigint")], &[], None),
            ("public.p", &[("id", "integer")], &["public.g"], None),
            ("public.c", &[("extra", "date")], &["public.p"], None),
            ("public.loop", &[("x", "integer")], &["public.loop"], None),
        ],
        Vec::new(),
    );
    let check = |name: Option<&str>, no_inherit| CheckConstraint {
        name: name.map(str::to_string),
        no_inherit,
    };
    let tables = &mut db.tables;
    tables.get_mut("public.g").unwrap().checks =
        vec![check(Some("g_pos"), false), check(None, false)];
    tables.get_mut("public.p").unwrap().checks = vec![check(Some("p_own"), true)];
    tables.get_mut("public.c").unwrap().checks = vec![check(Some("g_pos"), false)];
    tables.get_mut("public.loop").unwrap().checks = vec![check(Some("l"), false)];
    let named = |table: &str| -> Vec<(Option<String>, Option<String>)> {
        db.table_checks(table)
            .into_iter()
            .map(|(c, from)| (c.name.clone(), from.map(str::to_string)))
            .collect()
    };
    let s = |s: &str| Some(s.to_string());
    assert_eq!(named("public.g"), [(s("g_pos"), None), (None, None)]);
    assert_eq!(
        named("public.p"),
        [(s("p_own"), None), (s("g_pos"), s("public.g")), (None, s("public.g"))]
    );
    assert_eq!(named("public.c"), [(s("g_pos"), None), (None, s("public.g"))]);
    assert_eq!(named("public.loop"), [(s("l"), None)]);
    assert_eq!(named("public.absent"), []);
}

/// Each of `PartitionBoundSpec`'s four forms is read whole, spacing
/// collapsed outside a literal and a block comment dropped, with what
/// follows it left alone; anything else is no bound.
#[test]
fn a_partition_bound_is_read_in_each_of_its_forms() {
    let bound = |s: &str| partition_bound(s).map(|(b, after)| (b, after.trim().to_string()));
    let read = |b: &str, after: &str| Some((b.to_string(), after.to_string()));
    assert_eq!(bound(" FOR VALUES IN ('a');"), read("FOR VALUES IN ('a')", ";"));
    assert_eq!(
        bound("for values in ('a  b', 'c'')')\n PARTITION BY RANGE (id);"),
        read("for values in ('a  b', 'c'')')", "PARTITION BY RANGE (id);")
    );
    assert_eq!(
        bound("FOR VALUES FROM (MINVALUE)\n    TO ('2020-01-01') ;"),
        read("FOR VALUES FROM (MINVALUE) TO ('2020-01-01')", ";")
    );
    assert_eq!(
        bound("FOR VALUES WITH (modulus 2, /* x */ remainder 0);"),
        read("FOR VALUES WITH (modulus 2, remainder 0)", ";")
    );
    assert_eq!(bound("DEFAULT;"), read("DEFAULT", ";"));
    for none in [
        "FOR VALUES FROM (1);",
        "FOR VALUES IN 'a';",
        "FOR VALUES;",
        "DEFAULTS;",
        "/* x */ DEFAULT;",
    ] {
        assert_eq!(partition_bound(none), None, "{none}");
    }
}

/// `ATTACH PARTITION` alters the parent and is read into the partition
/// it names, the bound ending the statement; a bound none of the forms
/// reads keeps the parent, and a statement not ending there is none.
/// `CREATE TABLE … PARTITION OF` reads its parent and bound, and its
/// list as a typed table's — options and constraints, no column.
#[test]
fn a_partition_s_parent_and_bound_are_read_from_either_statement() {
    let of = |parent: &str, bound: Option<&str>| PartitionOf {
        parent: parent.to_string(),
        bound: bound.map(str::to_string),
    };
    assert_eq!(
        parse_alter_table_reference(
            "ALTER TABLE ONLY public.evt ATTACH PARTITION public.evt_a FOR VALUES IN ('a');"
        ),
        Some((
            "public.evt_a".to_string(),
            TableReference::PartitionOf(of("public.evt", Some("FOR VALUES IN ('a')")))
        ))
    );
    assert_eq!(
        parse_alter_table_reference("alter table \"P\".t attach partition \"P\".\"T1\" default;"),
        Some(("P.T1".to_string(), TableReference::PartitionOf(of("P.t", Some("default")))))
    );
    assert_eq!(
        parse_alter_table_reference(
            "ALTER TABLE ONLY public.t ATTACH PARTITION public.t1 FOR VALUES /* x */ IN (1);"
        ),
        Some(("public.t1".to_string(), TableReference::PartitionOf(of("public.t", None))))
    );
    for none in [
        "ALTER TABLE ONLY public.t ATTACH PARTITION public.t1 FOR VALUES IN (1)",
        "ALTER TABLE ONLY public.t DETACH PARTITION public.t1;",
    ] {
        assert_eq!(parse_alter_table_reference(none), None, "{none}");
    }

    let (name, table) = parse_table_def(&[
        "CREATE TABLE public.r_2020 PARTITION OF public.r (",
        "    id NOT NULL,",
        "    CONSTRAINT r_pos CHECK ((id > 0))",
        ")",
        "FOR VALUES FROM ('2020-01-01') TO ('2021-01-01')",
        "PARTITION BY LIST (region);",
    ]);
    assert_eq!(name, "public.r_2020");
    assert_eq!(
        table,
        TableDef {
            not_null: vec![("id".to_string(), NotNull::Inherited)],
            checks: vec![CheckConstraint { name: Some("r_pos".to_string()), no_inherit: false }],
            partition_of: Some(of(
                "public.r",
                Some("FOR VALUES FROM ('2020-01-01') TO ('2021-01-01')")
            )),
            ..TableDef::default()
        }
    );
    let (_, table) = parse_table_def(&["CREATE TABLE public.r_d PARTITION OF public.r DEFAULT;"]);
    assert_eq!(table.partition_of, Some(of("public.r", Some("DEFAULT"))));
    let (_, table) = parse_table_def(&["CREATE TABLE public.r_x PARTITION OF public.r;"]);
    assert_eq!(table.partition_of, Some(of("public.r", None)));
}

/// A partition's bounds run up through every ancestor that is a
/// partition too, and stop at a cycle; a table any partition names as
/// its parent has partitions, whether or not it is one itself.
#[test]
fn a_partition_s_bounds_run_up_its_ancestors() {
    let mut db = with_tables(
        &[
            ("public.top", &[("id", "integer")], &[], None),
            ("public.mid", &[("id", "integer")], &[], None),
            ("public.leaf", &[("id", "integer")], &[], None),
            ("public.loop", &[("id", "integer")], &[], None),
        ],
        Vec::new(),
    );
    let of = |parent: &str, bound: &str| {
        Some(PartitionOf { parent: parent.to_string(), bound: Some(bound.to_string()) })
    };
    let tables = &mut db.tables;
    tables.get_mut("public.mid").unwrap().partition_of = of("public.top", "FOR VALUES IN (1)");
    tables.get_mut("public.leaf").unwrap().partition_of = of("public.mid", "DEFAULT");
    tables.get_mut("public.loop").unwrap().partition_of = of("public.loop", "DEFAULT");
    let bounds = |table: &str| -> Vec<(String, String)> {
        db.partition_bounds(table)
            .into_iter()
            .map(|(partition, of)| (partition.to_string(), of.parent.clone()))
            .collect()
    };
    let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
    assert_eq!(
        bounds("public.leaf"),
        [pair("public.leaf", "public.mid"), pair("public.mid", "public.top")]
    );
    assert_eq!(bounds("public.mid"), [pair("public.mid", "public.top")]);
    assert_eq!(bounds("public.top"), []);
    assert_eq!(bounds("public.loop"), [pair("public.loop", "public.loop")]);
    assert_eq!(bounds("public.absent"), []);
    let partitioned: Vec<bool> = ["public.top", "public.mid", "public.leaf", "public.absent"]
        .into_iter()
        .map(|t| db.has_partitions(t))
        .collect();
    assert_eq!(partitioned, [true, true, false, false]);
}

/// A reference reaching nothing — a parent or type the preamble does not
/// hold, a composite whose list did not parse, a cycle only a
/// hand-written file can hold — answers `None` rather than guessing or
/// looping.
#[test]
fn a_reference_reaching_nothing_declares_nothing() {
    let unparsed =
        TypeDef { name: "public.opaque".to_string(), kind: TypeKind::Composite { fields: None } };
    let db = with_tables(
        &[
            ("public.orphan", &[], &["public.gone"], None),
            ("public.typed", &[], &[], Some("public.opaque")),
            ("public.untyped", &[], &[], Some("public.nowhere")),
            ("public.a", &[("x", "integer")], &["public.b"], None),
            ("public.b", &[], &["public.a"], None),
        ],
        vec![unparsed],
    );
    assert_eq!(db.declared_column("public.orphan", "id"), None);
    assert_eq!(db.declared_column("public.typed", "id"), None);
    assert_eq!(db.declared_column("public.untyped", "id"), None);
    assert_eq!(db.declared_column("public.b", "x").map(|c| c.name.as_str()), Some("x"));
    assert_eq!(db.declared_column("public.b", "y"), None);
    assert_eq!(db.declared_columns("public.b").len(), 1);
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
fn table_with_no_column_list_registers_with_zero_columns() {
    // I5: a typed table (`CREATE TABLE x OF t`) has no column list.
    let (_, cols) = parse_table(&["CREATE TABLE public.typed OF public.point2d;"]);
    assert_eq!(cols, Vec::new());
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

/// The `--binary-upgrade` fold (I6) is [`dump_metadata_from_spans`]'s
/// job, not `classify_statement`'s, needing an already-open `TypeDef` to
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

/// `dumpTableSchema`'s `CREATE %s%s %s` writes three kinds of table; each
/// is read as a table, a foreign one's `SERVER` clause after its list.
#[test]
fn unlogged_and_foreign_tables_are_tables() {
    let (name, cols) = parse_table(&[
        "CREATE UNLOGGED TABLE emitters.scratch (",
        "    id integer,",
        "    at date",
        ");",
    ]);
    assert_eq!(name, "emitters.scratch");
    assert_eq!(cols, [ColumnDef::new("id", "integer"), ColumnDef::new("at", "date")]);
    let (name, table) = parse_table_def(&[
        "CREATE FOREIGN TABLE objects.imported (",
        "    id integer NOT NULL,",
        "    born date",
        ")",
        "INHERITS (objects.base)",
        "SERVER objects_files",
        "OPTIONS (",
        "    filename '/tmp/objects_imported.tsv'",
        ");",
    ]);
    assert_eq!(name, "objects.imported");
    assert_eq!(table.columns, [not_null("id", "integer"), ColumnDef::new("born", "date")]);
    assert_eq!(table.parents, ["objects.base"]);
    assert!(classify_statement("CREATE UNLOGGED SEQUENCE public.s;").is_none());
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

/// A span with placeholder offsets — `dump_metadata_from_spans` never
/// reads `start`/`end`/`database`, only `body`.
fn span(body: SpanBody) -> crate::model::Span {
    crate::model::Span {
        start: 0,
        end: 0,
        database: None,
        text: None,
        toc: None,
        toc_owned: false,
        body,
    }
}

fn dummy_data_span() -> crate::model::Span {
    span(SpanBody::Data(crate::model::DataBlock::Copy(crate::model::CopyBlock {
        header: crate::copy::CopyHeader {
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
    // `CREATE DATABASE koji ...;` isn't one of `classify`'s four shapes
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

fn complete(buf: &str) -> bool {
    statement_complete(buf, true)
}

/// Guards against an apostrophe in comment prose opening a string that
/// never closes.
#[test]
fn statement_complete_is_not_confused_by_an_apostrophe_in_a_line_comment() {
    assert!(complete("CREATE TABLE t (id integer);\n-- it's here\nSELECT 1;"));
    // Not complete until a real `;` follows the comment.
    assert!(!complete("SELECT 1\n-- it's here"));
}

/// Guards against an apostrophe inside a double-quoted identifier
/// opening a string that never closes.
#[test]
fn statement_complete_is_not_confused_by_an_apostrophe_in_a_double_quoted_identifier() {
    assert!(complete(r#"CREATE TABLE public."it's" (id integer);"#));
}

#[test]
fn statement_complete_handles_doubled_double_quotes() {
    assert!(complete(r#"CREATE TABLE public."a""b" (id integer);"#));
    assert!(!complete(r#"CREATE TABLE public."a""b (id integer);"#));
}

#[test]
fn statement_complete_a_comment_run_to_end_of_buffer_is_never_complete() {
    // No trailing `\n` to close the comment — matches a statement scan
    // whose last fed line is itself a bare comment.
    assert!(!complete("SELECT 1;\n-- trailing comment, no newline after it"));
}

/// A statement ends where the scanner's lexer says its regions do
/// (`crate::lex`): an escape string's `\'`, a `;` or paren in a block
/// comment, and a plain string's backslash under the dump's own setting.
#[test]
fn statement_complete_lexes_as_the_scanner_does() {
    assert!(complete("COMMENT ON TABLE t IS E'it\\'s; here';"));
    assert!(!complete("COMMENT ON TABLE t IS E'it\\';"));
    assert!(complete("COMMENT ON TABLE t IS 'it\\';"));
    assert!(!statement_complete("COMMENT ON TABLE t IS 'it\\';", false));
    assert!(complete("SELECT 1 /* ( ; */;"));
    assert!(!complete("SELECT 1; /* a\n/* b */"));
    assert!(in_open_quote("SELECT 1; /* a\n/* b */", true));
    assert!(complete("SELECT 'a'\n'b' /* $$ */;"));
}

/// [`StatementScan::feed_line`] answers what [`statement_complete`] does
/// over the same lines joined by [`push_stmt_line`], so a buffer whose
/// last line is a bare comment stays open, which is what [`crate::map`]'s
/// dangling-close arm relies on.
#[test]
fn feeding_lines_matches_joining_them_with_newlines() {
    let cases: [&[&str]; 6] = [
        &["INSERT INTO t VALUES (1);"],
        &["INSERT INTO t VALUES (", "  'a''b'", ");"],
        &["SELECT 1;", "-- trailing comment"],
        &["INSERT INTO t VALUES ('a", "-- still string content", "');"],
        &["", "SELECT 1;"],
        &["INSERT INTO t VALUES (E'a'", "", "'b\\'', 1);"],
    ];
    for lines in cases {
        let mut scan = StatementScan::new(true);
        for line in lines {
            scan.feed_line(line.as_bytes());
        }
        let mut joined = String::new();
        for line in lines {
            push_stmt_line(&mut joined, line);
        }
        assert_eq!(scan.complete(), complete(&joined), "for {joined:?}");
        assert_eq!(scan.in_quote(), in_open_quote(&joined, true), "for {joined:?}");
    }
}

/// `is_empty` is what [`crate::map`]'s `INSERT` run reads as "a fresh
/// statement starts here", and it must survive a whitespace-only line
/// exactly as `buf.is_empty()` did: `push_stmt_line` appends such a line,
/// so the buffer stops being empty.
#[test]
fn a_statement_scan_is_empty_only_before_anything_is_fed() {
    let mut scan = StatementScan::new(true);
    assert!(scan.is_empty());
    scan.feed_line(b"");
    assert!(scan.is_empty(), "an empty line appends nothing, as `push_stmt_line` does not");
    scan.feed_line(b"   ");
    assert!(!scan.is_empty());
    scan.reset();
    assert!(scan.is_empty());
}

/// A reset keeps the dump's setting: the next statement of an `INSERT`
/// run is written under the same one.
#[test]
fn a_reset_keeps_the_standard_strings_setting() {
    let mut scan = StatementScan::new(false);
    scan.feed_line(b"SELECT 1;");
    scan.reset();
    scan.feed_line(br"INSERT INTO t VALUES ('a\');");
    assert!(scan.in_quote());
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

/// [`extract_statement_cross_refs`]'s five recognized shapes, in lines
/// shaped as `fixtures/16/objects/default.sql`'s.
fn refs_of(stmt: &str) -> (Vec<String>, Vec<String>) {
    let mut roles = BTreeSet::new();
    let mut tablespaces = BTreeSet::new();
    extract_statement_cross_refs(stmt, &mut roles, &mut tablespaces);
    (roles.into_iter().collect(), tablespaces.into_iter().collect())
}

#[test]
fn owner_to_is_recognized_across_every_ownable_object_kind() {
    assert_eq!(
        refs_of("ALTER FUNCTION objects.rgb_to_cmyk(objects.color_rgb) OWNER TO postgres;").0,
        vec!["postgres".to_string()]
    );
    assert_eq!(refs_of("ALTER LARGE OBJECT 16490 OWNER TO postgres;").0, vec!["postgres"]);
}

#[test]
fn a_grant_records_its_grantee_but_not_public() {
    assert_eq!(
        refs_of("GRANT SELECT ON TABLE objects.events TO fixture_reader;").0,
        vec!["fixture_reader".to_string()]
    );
    assert!(refs_of("GRANT SELECT ON TABLE objects.widgets TO PUBLIC;").0.is_empty());
}

#[test]
fn a_revoke_records_its_grantee_from_from() {
    assert_eq!(
        refs_of("REVOKE ALL ON TABLE objects.events FROM fixture_reader;").0,
        vec!["fixture_reader".to_string()]
    );
}

/// `ALTER DEFAULT PRIVILEGES` is one statement carrying two role
/// references — the role the default applies to, and the grant's own
/// grantee — both recorded from the single combined line
/// `buildDefaultACLCommands` writes.
#[test]
fn alter_default_privileges_records_both_its_for_role_and_its_grantee() {
    let (roles, _) = refs_of(
        "ALTER DEFAULT PRIVILEGES FOR ROLE postgres IN SCHEMA objects GRANT SELECT ON TABLES \
             TO fixture_reader;",
    );
    assert_eq!(roles, vec!["fixture_reader".to_string(), "postgres".to_string()]);
}

#[test]
fn set_default_tablespace_records_a_real_tablespace_but_not_a_reset() {
    assert_eq!(refs_of("SET default_tablespace = fastspace;").1, vec!["fastspace".to_string()]);
    assert!(refs_of("SET default_tablespace = '';").1.is_empty());
}

#[test]
fn pg_default_is_never_recorded_even_if_named_explicitly() {
    assert!(refs_of("SET default_tablespace = pg_default;").1.is_empty());
}

/// `fmtId` quotes a name holding a capital, so a role `PUBLIC` and a
/// tablespace `PG_DEFAULT` are written quoted and are kept; `public`,
/// quoted or not, is the pseudo-role to the server (I85), and is not.
#[test]
fn a_quoted_public_role_and_pg_default_tablespace_are_kept() {
    assert_eq!(
        refs_of("GRANT SELECT ON TABLE objects.widgets TO \"PUBLIC\";").0,
        vec!["PUBLIC".to_string()]
    );
    assert_eq!(refs_of("ALTER TABLE objects.t OWNER TO \"PUBLIC\";").0, vec!["PUBLIC"]);
    assert!(refs_of("REVOKE ALL ON TABLE objects.t FROM \"public\";").0.is_empty());
    assert_eq!(
        refs_of("SET default_tablespace = \"PG_DEFAULT\";").1,
        vec!["PG_DEFAULT".to_string()]
    );
}

/// `fmtId()`'s quoted form (a role/tablespace name that needs quoting,
/// with `""` doubling any literal `"`) is parsed the same way an
/// unquoted one is.
#[test]
fn a_quoted_role_name_is_unquoted() {
    assert_eq!(
        refs_of("ALTER SCHEMA public OWNER TO \"has a \"\"quote\"\" and space\";").0,
        vec!["has a \"quote\" and space".to_string()]
    );
}

#[test]
fn a_plain_ddl_statement_records_nothing() {
    let (roles, tablespaces) = refs_of("CREATE TABLE public.t (id integer);");
    assert!(roles.is_empty());
    assert!(tablespaces.is_empty());
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
