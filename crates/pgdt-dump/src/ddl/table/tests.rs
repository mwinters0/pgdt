use super::*;
use crate::ddl::classify_create;
use crate::ddl::tests::{parse_table, parse_table_def, parse_type};
use pgdt_model::TypeKind;

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

#[test]
fn table_with_no_column_list_registers_with_zero_columns() {
    // I5: a typed table (`CREATE TABLE x OF t`) has no column list.
    let (_, cols) = parse_table(&["CREATE TABLE public.typed OF public.point2d;"]);
    assert_eq!(cols, Vec::new());
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
    assert!(classify_create("CREATE UNLOGGED SEQUENCE public.s;").is_none());
}
