use super::*;

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
