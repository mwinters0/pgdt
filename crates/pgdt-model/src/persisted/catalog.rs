//! What the preamble declares, per database, as the cache persists it: the
//! words its DDL wrote (`docs/design/decisions.md`, "D74"). The walks over
//! them are `crate::catalog`'s.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Everything the preamble pass recovered, per database — see
/// `docs/design/decisions.md`, "The file map and the preamble".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpMetadata {
    pub databases: Vec<DatabaseMetadata>,
}

/// One database's preamble: its extensions, types and collations in DDL
/// order, its tables by qualified name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseMetadata {
    /// `None` for a plain `pg_dump` output, which has no `\connect` and so
    /// names no database. Never guessed: a `--create` dump's pre-`\connect`
    /// segment names no real database either, and is told apart by being
    /// *replaced* rather than pushed when the first `\connect` arrives, its
    /// version headers carried over ([`pgdt_dump::dump_metadata_from_spans`]'s `Connect`
    /// arm).
    pub name: Option<String>,
    /// Whether this database's preamble was read to completion. Always
    /// `true` for every entry a `pgdt_fixtures::build_index` full scan or a
    /// [`pgdt_dump::internal::scan_preamble`] prepass produces, neither ever leaving
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
    /// [`pgdt_core::CopyHeader::qualified_name`] is) -> what its `CREATE
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
    /// [`pgdt_codec::compare::comparison_for`] reads back as the server reads a
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
/// [`pgdt_codec::compare::comparison_for`].
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
    /// identifier and quoted elsewhere (I29) — and `pgdt_codec::pgtype` compares
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
    /// Fully determined by the DDL — labels in sort order (I33), which is
    /// declaration order but for a label an `ADD VALUE` placed `BEFORE` or
    /// `AFTER` another. A `--binary-upgrade` dump emits these via a separate
    /// `ALTER TYPE ADD VALUE` per label (I6); `pgdt_dump`'s
    /// `ddl::metadata::fold_alter_type_add_value` folds them back in here so
    /// both forms produce the same shape.
    ///
    /// `exact` says the labels are every label the type holds when its
    /// database's preamble ends, which is what makes a field naming another
    /// one a refusal (I70). It is cleared by a label no plain `'…'` literal spells
    /// and by an `ALTER TYPE` that could change the labels and is not read —
    /// `RENAME VALUE`, `ADD VALUE IF NOT EXISTS`, an `ADD VALUE` placed beside
    /// a label not read, and any naming a type no definition here carries,
    /// which clears every enum's (`pgdt_dump`'s
    /// `ddl::metadata::fold_enum_labels_unread`). Nothing `pg_dump` writes clears it.
    Enum { labels: Vec<String>, exact: bool },
    /// Reduces to a base type, resolved transitively by [`pgdt_codec::pgtype`] (a
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
    /// (`pgdt_codec`'s `domain_not_null`, I76).
    ///
    /// `check` is whether its `CREATE DOMAIN` declares a `CHECK`, an
    /// expression `domain_in` evaluates against each value of it (I77).
    Domain { base_type: String, collation: Option<String>, not_null: bool, check: bool },
    /// Field name -> declared type, in declaration order — or `None` when the
    /// body held a fragment this grammar could not parse.
    ///
    /// All-or-nothing, unlike `CREATE TABLE`'s column list: `record_out` is
    /// positional and carries no field names (I23), so there is no join to
    /// recover a dropped field the way `pgdt_codec::resolve_columns`
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
    /// `pgdt_codec::pgtype` resolves a column declared with that name from.
    ///
    /// `canonical` is the `canonical = <function>` parameter, verbatim, which
    /// `pg_dump` writes whenever `pg_range.rngcanonical` is set (I10). Its
    /// *presence* is the fact `pgdt_codec::pgtype` needs: a user's canonical
    /// function is arbitrary server-side code, so knowing it exists licenses
    /// declining the column rather than reproducing its rewriting.
    ///
    /// `collation` is the `collation = <collation>` parameter, verbatim, which
    /// `pg_dump` writes only where the range's collation differs from its
    /// subtype's, and which the server compares the bounds under (I95).
    Range {
        subtype: Option<String>,
        multirange_type_name: Option<String>,
        canonical: Option<String>,
        collation: Option<String>,
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
