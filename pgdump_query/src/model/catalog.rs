//! What the preamble declares, per database: the tables, types, collations
//! and extensions its DDL wrote, kept as the words written — a declared type's
//! text, never a parsed `(base, typmod)` pair, the codec drawing every
//! conclusion from them (`docs/design/decisions.md`, "D74"). The grammar that
//! reads them is `crate::preamble`'s.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::lex::canonical_type_name;

/// Everything the preamble pass recovered, per database — see
/// `docs/design/decisions.md`, "The file map and the preamble".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpMetadata {
    pub databases: Vec<DatabaseMetadata>,
}

/// One database's preamble, in DDL order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseMetadata {
    /// `None` for a plain `pg_dump` output, which has no `\connect` and so
    /// names no database. Never guessed: a `--create` dump's pre-`\connect`
    /// segment names no real database either, and is told apart by being
    /// *replaced* rather than pushed when the first `\connect` arrives, its
    /// version headers carried over ([`crate::preamble::dump_metadata_from_spans`]'s `Connect`
    /// arm).
    pub name: Option<String>,
    /// Whether this database's preamble was read to completion. Always
    /// `true` for every entry a [`crate::stream::build_index`] full scan or a
    /// [`crate::load::scan_preamble`] prepass produces, neither ever leaving
    /// a segment half-read. The *first* database's metadata is present after
    /// any scan that persists a cache (the preamble prepass,
    /// `docs/design/decisions.md`, "D30") and every later `\connect`ed
    /// database's once the mapping pass reaches its first `COPY` block, so a
    /// database the scan has not reached is absent from
    /// `DumpIndex::metadata` rather than present with this `false`.
    pub preamble_complete: bool,
    pub server_version: Option<String>,
    pub pg_dump_version: Option<String>,
    pub extensions: Vec<Extension>,
    pub types: Vec<TypeDef>,
    /// The `CREATE COLLATION` statements this database's preamble declared,
    /// in DDL order. Only *user-defined* collations appear: `pg_dump` emits no
    /// definition for the ones `initdb` created in `pg_catalog`, so a
    /// `COLLATE "en_US.utf8"` clause resolves to nothing here (I42).
    pub collations: Vec<CollationDef>,
    /// Qualified table name (`schema.table`, folded the same way
    /// [`crate::copy::CopyHeader::qualified_name`] is) -> what its `CREATE
    /// TABLE` declared, and the `--binary-upgrade` `ALTER TABLE` forms that
    /// add a reference to it. A column is looked up through
    /// [`DatabaseMetadata::declared_column`], which reaches the columns the
    /// table takes from elsewhere.
    ///
    /// Deficiency register: `deficiency: KD14` — this is the structure a scan
    /// holds per table, and peak resident set grows with the table count while
    /// staying flat in dump bytes, about a fifth of it live structure the
    /// preamble alone pays (`measurements.md`, `peak-rss` and `rss-attribution`).
    /// **(c) unowned**; promoted by a dump with tens of thousands of tables,
    /// nothing in hand being one. It is also why every "resident set" claim
    /// about this system is the *one-block* reading and says so.
    pub tables: BTreeMap<String, TableDef>,
}

impl DatabaseMetadata {
    /// The declaration of column `column` of table `table`: the table's own,
    /// else, for a typed table, its `OF` type's field alone, else the first
    /// parent's, in `INHERITS` order, that declares it — each parent asked the same way, so a
    /// grandparent's column is found. `None` where the table is not declared
    /// here or nothing it reaches declares the column.
    ///
    /// Read against the preamble's final state, never folded into the
    /// table's own list (`docs/design/decisions.md`, "D36"). A reference that
    /// names a table or type the preamble does not hold, or a composite whose
    /// field list did not parse, reaches nothing; a cycle, which no server
    /// can hold and a hand-written file can, ends at the table it returns to.
    ///
    /// The server merges a column declared on two of these into one of a
    /// single type and collation, refusing the table otherwise, so which one
    /// is found first decides nothing `pg_dump` can write.
    pub fn declared_column(&self, table: &str, column: &str) -> Option<&ColumnDef> {
        self.declared_column_from(table, column, &mut BTreeSet::new())
    }

    fn declared_column_from<'a>(
        &'a self,
        table: &str,
        column: &str,
        visited: &mut BTreeSet<&'a str>,
    ) -> Option<&'a ColumnDef> {
        let (name, def) = self.tables.get_key_value(table)?;
        if !visited.insert(name) {
            return None;
        }
        if let Some(found) = def.columns.iter().find(|c| c.name == column) {
            return Some(found);
        }
        if let Some(of_type) = &def.of_type {
            return self.composite_fields(of_type)?.iter().find(|f| f.name == column);
        }
        def.parents.iter().find_map(|parent| self.declared_column_from(parent, column, visited))
    }

    /// Every column of `table`, in the order the server gives them — each
    /// parent's, in `INHERITS` order, then the `OF` type's fields, then the
    /// table's own — a column two of them declare appearing once, at its
    /// first place, as the declaration [`declared_column`](Self::declared_column)
    /// returns — bar a table both typed and inheriting, which no server holds,
    /// whose parents that lookup never asks. Empty where the table is not
    /// declared here.
    pub fn declared_columns(&self, table: &str) -> Vec<&ColumnDef> {
        let mut columns = Vec::new();
        self.declared_columns_from(table, &mut BTreeSet::new(), &mut columns);
        columns
    }

    fn declared_columns_from<'a>(
        &'a self,
        table: &str,
        visited: &mut BTreeSet<&'a str>,
        columns: &mut Vec<&'a ColumnDef>,
    ) {
        let Some((name, def)) = self.tables.get_key_value(table) else { return };
        if !visited.insert(name) {
            return;
        }
        for parent in &def.parents {
            let mut inherited = Vec::new();
            self.declared_columns_from(parent, visited, &mut inherited);
            for column in inherited {
                if !columns.iter().any(|c| c.name == column.name) {
                    columns.push(column);
                }
            }
        }
        let typed = def.of_type.as_deref().and_then(|t| self.composite_fields(t));
        for column in typed.into_iter().flatten().chain(&def.columns) {
            match columns.iter_mut().find(|c| c.name == column.name) {
                Some(merged) => *merged = column,
                None => columns.push(column),
            }
        }
    }

    /// Whether table `table` declares column `column` `NOT NULL` — on the
    /// column, at the table, or by an `ALTER TABLE … SET NOT NULL` — or takes
    /// it from a parent, in `INHERITS` order and each parent asked the same
    /// way, which passes on every `NOT NULL` but v18's `NO INHERIT` (I76). A
    /// domain's is its type's, [`crate::pgtype::domain_not_null`]'s to say.
    ///
    /// Read against the preamble's final state, as
    /// [`declared_column`](Self::declared_column) is; a `NOT NULL` added after
    /// the data (v18's `NOT VALID`, a post-data primary key) is never in it.
    pub fn column_not_null(&self, table: &str, column: &str) -> bool {
        self.not_null_from(table, column, true, &mut BTreeSet::new())
    }

    /// [`column_not_null`](Self::column_not_null) over `table`, which is the
    /// table asked about where `own` and an ancestor of it elsewhere.
    fn not_null_from<'a>(
        &'a self,
        table: &str,
        column: &str,
        own: bool,
        visited: &mut BTreeSet<&'a str>,
    ) -> bool {
        let Some((name, def)) = self.tables.get_key_value(table) else { return false };
        if !visited.insert(name) {
            return false;
        }
        let binds = |not_null: NotNull| own || not_null == NotNull::Inherited;
        def.columns.iter().filter(|c| c.name == column).filter_map(|c| c.not_null).any(binds)
            || def.not_null.iter().any(|(c, not_null)| c == column && binds(*not_null))
            || def.parents.iter().any(|parent| self.not_null_from(parent, column, false, visited))
    }

    /// The `CHECK` constraints a restore evaluates against each row of table
    /// `table`, in declaration order: its own, then each parent's it takes,
    /// in `INHERITS` order and each parent asked the same way, a parent
    /// passing on every one but a `NO INHERIT` one (I77). Each is paired with
    /// the ancestor declaring it, `None` for the table's own. A name met
    /// twice is one constraint, the server merging an inherited `CHECK` into
    /// a child's own of that name, as `--binary-upgrade`'s child re-declares
    /// it; an unnamed one is never merged.
    ///
    /// Read against the preamble's final state, as
    /// [`declared_column`](Self::declared_column) is; a `CHECK` added after
    /// the data is never in it.
    pub fn table_checks(&self, table: &str) -> Vec<(&CheckConstraint, Option<&str>)> {
        let mut checks = Vec::new();
        self.checks_from(table, true, &mut BTreeSet::new(), &mut checks);
        checks
    }

    /// [`table_checks`](Self::table_checks) over `table`, which is the table
    /// asked about where `own` and an ancestor of it elsewhere.
    fn checks_from<'a>(
        &'a self,
        table: &str,
        own: bool,
        visited: &mut BTreeSet<&'a str>,
        checks: &mut Vec<(&'a CheckConstraint, Option<&'a str>)>,
    ) {
        let Some((name, def)) = self.tables.get_key_value(table) else { return };
        if !visited.insert(name) {
            return;
        }
        let declared_by = (!own).then_some(name.as_str());
        for check in def.checks.iter().filter(|c| own || !c.no_inherit) {
            let merged =
                check.name.is_some() && checks.iter().any(|(held, _)| held.name == check.name);
            if !merged {
                checks.push((check, declared_by));
            }
        }
        for parent in &def.parents {
            self.checks_from(parent, false, visited, checks);
        }
    }

    /// The partition bounds a row loaded into table `table` must fall within:
    /// its own, then its parent's where that is a partition too, and so up,
    /// each paired with the partition it bounds — a partition's constraint
    /// holding every ancestor's (I86). Empty for a table that is no
    /// partition; a cycle, which no server holds, ends where it returns.
    pub fn partition_bounds(&self, table: &str) -> Vec<(&str, &PartitionOf)> {
        let mut bounds: Vec<(&str, &PartitionOf)> = Vec::new();
        let mut next = self.tables.get_key_value(table);
        while let Some((name, def)) = next
            && let Some(partition_of) = &def.partition_of
            && !bounds.iter().any(|(held, _)| held == name)
        {
            bounds.push((name.as_str(), partition_of));
            next = self.tables.get_key_value(&partition_of.parent);
        }
        bounds
    }

    /// Whether some table this preamble declares is a partition of table
    /// `table`, so a row loaded into `table` is routed to one by their bounds.
    pub fn has_partitions(&self, table: &str) -> bool {
        self.tables.values().any(|def| def.partition_of.as_ref().is_some_and(|p| p.parent == table))
    }

    /// The fields of the composite `type_name` names, where it parsed, the
    /// two names compared in their canonical spelling (I29).
    fn composite_fields(&self, type_name: &str) -> Option<&[ColumnDef]> {
        let key = canonical_type_name(type_name)?;
        self.types.iter().find_map(|t| match &t.kind {
            TypeKind::Composite { fields: Some(fields) }
                if canonical_type_name(&t.name).is_some_and(|name| name == key) =>
            {
                Some(fields.as_slice())
            }
            _ => None,
        })
    }

    pub(crate) fn empty(name: Option<String>) -> Self {
        Self {
            name,
            preamble_complete: false,
            server_version: None,
            pg_dump_version: None,
            extensions: Vec::new(),
            types: Vec::new(),
            collations: Vec::new(),
            tables: BTreeMap::new(),
        }
    }
}

/// A `CREATE COLLATION` statement, as the DDL wrote it.
///
/// Two fields, all a plain dump carries that anything here reads (I42): the
/// collation's name, and whether the statement said `deterministic = false`.
/// The provider and locale are in the file and deliberately not kept —
/// nothing resolves a collation's *order* from them, a plain dump omitting
/// the `collversion` that would be needed to (I42).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollationDef {
    /// Schema-qualified name, in the canonical spelling a type's name is kept
    /// in (`public.c_collation`, `public."CI"`) — which
    /// [`crate::pgtype::comparison_for`] reads back as the server reads a
    /// column's `COLLATE` clause, which is how it joins the two.
    pub name: String,
    /// `false` only where the statement carried `deterministic = false`.
    ///
    /// The absence of the clause is the server's own default, not a
    /// conclusion of ours: `CREATE COLLATION` defaults to deterministic, and
    /// `pg_dump` writes `, deterministic = false` wherever the catalog says
    /// otherwise, gated on no dump option (I42). So `true` here is what the
    /// file states, not a guess.
    pub deterministic: bool,
}

/// A `CREATE EXTENSION` line. Extension *versions* are never in a regular
/// dump — `dumpExtension()` deliberately omits them
/// (`docs/design/postgres-invariants.md`) — so there is no version field here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extension {
    pub name: String,
    pub schema: Option<String>,
}

/// One column of a `CREATE TABLE`, as the DDL wrote it.
///
/// Every text field is the dump's own text, never a conclusion (see the
/// module docs): `declared_type` is the type's words as written (`character
/// varying(16)`), comments and spacing dropped, and `collation` the `COLLATE` clause's reference exactly as
/// written — `pg_catalog."C"`, schema-qualified and quoted the way `pg_dump`
/// writes it (I37).
///
/// **`None` is "no clause", not "the database default".** `pg_dump` omits the
/// clause whenever a column's collation is its *type's* default, so a bare
/// `name` column is `C` and a bare `text` column is the database's — two
/// facts behind one absence, decided by
/// [`crate::pgtype::comparison_for`].
///
/// `not_null` is the `NOT NULL` among its constraints, or what implies one —
/// an inline `PRIMARY KEY`, `GENERATED … AS IDENTITY` or a `serial` type —
/// read at their top level (I76). It is this declaration's alone: a column the table inherits it
/// for is found through [`DatabaseMetadata::column_not_null`]. A composite's
/// field never carries one, the server refusing a constraint there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnDef {
    pub name: String,
    pub declared_type: String,
    pub collation: Option<String>,
    pub not_null: Option<NotNull>,
}

impl ColumnDef {
    /// A column carrying no `COLLATE` clause and no `NOT NULL` — the ordinary
    /// case, and the one a test or an embedder building metadata by hand
    /// wants.
    pub fn new(name: impl Into<String>, declared_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            declared_type: declared_type.into(),
            collation: None,
            not_null: None,
        }
    }
}

/// A `NOT NULL` a table declares on one of its columns, by whether the
/// table's inheritance children take it (I76).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotNull {
    /// Taken by every child: every `NOT NULL` before v18, and from v18 every
    /// one but `NO INHERIT`'s, a primary key's and an identity's included.
    Inherited,
    /// v18's `NOT NULL … NO INHERIT`: this table's alone.
    NoInherit,
}

/// One table's declaration, as the DDL wrote it: its own columns, and the
/// objects it takes the rest of its columns from.
///
/// `pg_dump` writes a column only where the table declares it, so an
/// inheritance child's `CREATE TABLE` omits every column it inherits
/// (`shouldPrintColumn`), and a typed table's writes none with its type, only
/// the options of one carrying a default or `NOT NULL` — which hold no type
/// and so are no [`ColumnDef`] here, a `NOT NULL` among them going to
/// [`TableDef::not_null`]. Both are found through
/// [`DatabaseMetadata::declared_column`]. Under `--binary-upgrade` every
/// column is written, and the references arrive after the `CREATE TABLE`, as
/// `ALTER TABLE ONLY … INHERIT …` and `… OF …` ([`TableReference`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableDef {
    /// The columns the table declares itself, in DDL order.
    pub columns: Vec<ColumnDef>,
    /// The `INHERITS (…)` parents, in the order written, each qualified as a
    /// key of [`DatabaseMetadata::tables`] is.
    pub parents: Vec<String>,
    /// The `OF <type>` composite, in [`TypeDef::name`]'s canonical spelling.
    pub of_type: Option<String>,
    /// The columns a `NOT NULL` outside their own definition names, in the
    /// order written: v18's table-level `NOT NULL <column>`, a typed table's
    /// column options, a `PRIMARY KEY (…)` list, and the `ALTER TABLE … ALTER
    /// COLUMN … SET NOT NULL` a dump before v18 writes for an inherited
    /// column (I76).
    pub not_null: Vec<(String, NotNull)>,
    /// The `CHECK` constraints the table declares before its data, in the
    /// order written: in its list, on a column, or by an `ALTER TABLE … ADD
    /// CONSTRAINT … CHECK`, which `--binary-upgrade` writes for one the
    /// table inherits. One it takes from a parent is found through
    /// [`DatabaseMetadata::table_checks`].
    pub checks: Vec<CheckConstraint>,
    /// The partitioned table this one is a partition of, and its bound: from
    /// the `ALTER TABLE ONLY <parent> ATTACH PARTITION <this> <bound>;` every
    /// supported `pg_dump` writes before the data, or a hand-written `CREATE
    /// TABLE <this> PARTITION OF <parent> … <bound>` (I86).
    pub partition_of: Option<PartitionOf>,
}

/// A partition's place in its partitioned table ([`TableDef::partition_of`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionOf {
    /// The partitioned table, qualified as a key of
    /// [`DatabaseMetadata::tables`] is.
    pub parent: String,
    /// gram.y's `PartitionBoundSpec` as its text — `FOR VALUES IN ('a')`,
    /// `FOR VALUES FROM (1) TO (10)`, `FOR VALUES WITH (modulus 2, remainder
    /// 0)` or `DEFAULT` — block comments dropped and each run of spacing
    /// outside a literal one space; `None` where the statement names the
    /// parent and its bound is not one of those.
    pub bound: Option<String>,
}

/// A `CHECK` constraint a table declares: an expression a restore evaluates
/// against each row, which no reading of the field can (I77).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckConstraint {
    /// `CONSTRAINT <name>`'s name, folded as the server folds it; `None`
    /// for one written without, which `pg_dump` never writes.
    pub name: Option<String>,
    /// `NO INHERIT`: this table's alone, no inheritance child taking it.
    pub no_inherit: bool,
}

impl TableDef {
    /// A table declaring `columns` and referring to nothing — the ordinary
    /// shape, and the one a test or an embedder building metadata by hand
    /// wants.
    pub fn with_columns(columns: Vec<ColumnDef>) -> Self {
        Self { columns, ..Self::default() }
    }

    pub(crate) fn add(&mut self, reference: &TableReference) {
        match reference {
            TableReference::Parent(parent) if !self.parents.contains(parent) => {
                self.parents.push(parent.clone());
            }
            TableReference::Parent(_) => {}
            TableReference::OfType(of_type) => self.of_type = Some(of_type.clone()),
            TableReference::NotNull(column) => {
                self.not_null.push((column.clone(), NotNull::Inherited));
            }
            TableReference::Check(check) => self.checks.push(check.clone()),
            TableReference::PartitionOf(partition_of) => {
                self.partition_of = Some(partition_of.clone());
            }
        }
    }
}

/// What an `ALTER TABLE` adds to a table already declared: the references
/// `--binary-upgrade` writes after a full column list in place of the
/// `CREATE TABLE`'s own clauses, the `NOT NULL` a dump before v18 writes
/// for a column the table does not print, a `CHECK` it adds, and the
/// partitioned table it is attached to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TableReference {
    /// `INHERIT <parent>`, qualified as [`TableDef::parents`] is.
    Parent(String),
    /// `OF <type>`, spelled as [`TableDef::of_type`] is.
    OfType(String),
    /// `ALTER [COLUMN] <column> SET NOT NULL`, into [`TableDef::not_null`].
    NotNull(String),
    /// `ADD [CONSTRAINT <name>] CHECK (…)`, into [`TableDef::checks`].
    Check(CheckConstraint),
    /// `ALTER TABLE [ONLY] <parent> ATTACH PARTITION <table> <bound>`, which
    /// alters the parent and is folded into the partition it names, into
    /// [`TableDef::partition_of`].
    PartitionOf(PartitionOf),
}

/// A `CREATE TYPE` or `CREATE DOMAIN` definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeDef {
    /// Schema-qualified name, as SQL spells it. The preamble keeps the
    /// canonical spelling — each part bare where it is a plain lower-case
    /// identifier and quoted elsewhere (I29) — and `crate::pgtype` compares
    /// the canonical spellings of both sides of a lookup, so `PUBLIC."Mood"`
    /// finds a definition of `"public"."Mood"`, and `public.Mood` — which is
    /// `public.mood` — does not.
    pub name: String,
    pub kind: TypeKind,
}

/// Which of `pg_dump`'s six type-emission shapes produced a [`TypeDef`] — see
/// the variants below.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeKind {
    /// Fully determined by the DDL — labels in declaration order. A
    /// `--binary-upgrade` dump emits these via a separate `ALTER TYPE ADD
    /// VALUE` per label (I6); `crate::preamble::fold_alter_type_add_value` folds them back
    /// in here so both forms produce the same shape.
    ///
    /// `exact` says the labels are every label the type holds when its
    /// database's preamble ends, which is what makes a field naming another
    /// one a refusal (I70). It is cleared by a label no plain `'…'` literal spells
    /// and by an `ALTER TYPE` that could change the labels and is not read —
    /// `RENAME VALUE`, `ADD VALUE IF NOT EXISTS`, and any naming a type no
    /// definition here carries, which clears every enum's
    /// (`crate::preamble::fold_enum_labels_unread`). Nothing `pg_dump` writes clears it.
    Enum { labels: Vec<String>, exact: bool },
    /// Reduces to a base type, resolved transitively by [`crate::pgtype`] (a
    /// domain over a domain is legal).
    ///
    /// `collation` is the domain's own `COLLATE` clause, verbatim, which
    /// `pg_dump` writes only where the domain's collation differs from its
    /// base type's (I37). It is the domain's *type default*, so a column
    /// declared with this domain and carrying no clause of its own inherits
    /// it.
    ///
    /// `not_null` is whether the domain declares `NOT NULL`, which `domain_in`
    /// checks of a NULL it is handed, and every domain over this one with it
    /// ([`crate::pgtype::domain_not_null`], I76).
    ///
    /// `check` is whether its `CREATE DOMAIN` declares a `CHECK`, an
    /// expression `domain_in` evaluates against each value of it (I77).
    Domain { base_type: String, collation: Option<String>, not_null: bool, check: bool },
    /// Field name -> declared type, in declaration order — or `None` when the
    /// body held a fragment this grammar could not parse.
    ///
    /// All-or-nothing, unlike `CREATE TABLE`'s column list: `record_out` is
    /// positional and carries no field names (I23), so there is no join to
    /// recover a dropped field the way `crate::resolve::resolve_columns`
    /// recovers a dropped column by name, and a three-field type parsed as
    /// two would fail the field-count check on every valid row of it.
    /// `Some(vec![])` is a real zero-field composite (`CREATE TYPE x AS ();`,
    /// I23) and maps to a zero-field `Struct`; `None` resolves the column to
    /// `Utf8View`, like anything else the grammar does not recognize.
    Composite { fields: Option<Vec<ColumnDef>> },
    /// The subtype named in the `CREATE TYPE ... AS RANGE (...)` parameter
    /// list, if the grammar found one, plus the name of its auto-created
    /// companion multirange type (PG14+) if the DDL named one explicitly via
    /// `multirange_type_name` (I10). `pg_dump` emits no `CREATE TYPE` for that
    /// companion, so this parameter is its only trace in the file and what
    /// `crate::pgtype` resolves a column declared with that name from.
    ///
    /// `canonical` is the `canonical = <function>` parameter, verbatim, which
    /// `pg_dump` writes whenever `pg_range.rngcanonical` is set (I10). Its
    /// *presence* is the fact `crate::pgtype` needs: a user's canonical
    /// function is arbitrary server-side code, so knowing it exists licenses
    /// declining the column rather than reproducing its rewriting.
    Range {
        subtype: Option<String>,
        multirange_type_name: Option<String>,
        canonical: Option<String>,
    },
    /// A C-level base type (`CREATE TYPE x (INPUT = ..., OUTPUT = ...)`) —
    /// information-free but for `delimiter`; the dump says how the *server*
    /// parses it.
    ///
    /// `delimiter` is its `typdelim`, the byte `array_in` splits an array of
    /// it at: the first byte of its `DELIMITER` literal, as `DefineType` takes
    /// it, and `,` where it states none, `dumpBaseType` writing the clause
    /// wherever it is not (I22). `None` where the clause is no plain string
    /// literal, the list holding it is not closed, or the body is no shape
    /// this grammar knows, none of which `pg_dump` writes.
    Base { delimiter: Option<u8> },
    /// `CREATE TYPE x;` with no body at all, ahead of the real definition
    /// (forward-declaration shell type) or genuinely never completed.
    Shell,
}

impl TypeKind {
    /// A domain carrying no `COLLATE` clause, `NOT NULL` or `CHECK` of its
    /// own — the ordinary shape, and the one a test or an embedder building a
    /// type list by hand wants.
    pub fn domain(base_type: impl Into<String>) -> Self {
        Self::Domain { base_type: base_type.into(), collation: None, not_null: false, check: false }
    }

    /// A base type declaring no `DELIMITER`, so `,` — the ordinary shape, and
    /// the one a test or an embedder building a type list by hand wants.
    pub fn base() -> Self {
        Self::Base { delimiter: Some(b',') }
    }

    /// An enum whose labels are exactly these, in declaration order — what
    /// `CREATE TYPE … AS ENUM (…)` declares, and the shape a test or an
    /// embedder building a type list by hand wants.
    pub fn exact_enum<S: Into<String>>(labels: impl IntoIterator<Item = S>) -> Self {
        Self::Enum { labels: labels.into_iter().map(Into::into).collect(), exact: true }
    }
}
