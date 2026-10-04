//! Dump-level preamble parsing: `CREATE TABLE`/`TYPE`/`DOMAIN`/`EXTENSION`
//! DDL and the two version header lines, recovered from the pre-data region
//! of a `pg_dump` plain-format file (`docs/design/decisions.md`,
//! "D36").
//!
//! **This module owns the DDL grammar, not a second pass over the file.**
//! [`classify_statement`] and friends (`parse_create_table`, `parse_create_type`,
//! …) are the shared parser [`crate::map::classify`] calls to turn a complete
//! statement into a [`crate::map::SpanBody`] while it builds the full file
//! map in its one pass over [`crate::scan::scan`]'s events. [`DumpMetadata`]
//! is then [`dump_metadata_from_spans`] — a derived view over the resulting
//! spans, computed once, never a second line-by-line scan
//! (`docs/design/decisions.md`, "D34").
//!
//! It stores what the dump said, never what we concluded: declared types are
//! kept as the words written (`character varying(16)`, not a parsed
//! `(base, typmod)` pair), L1 being unable to hold an L2 conclusion
//! (`docs/design/decisions.md`, "D74"). Resolving those strings into Arrow
//! types is [`crate::pgtype`]'s job.
//!
//! [`extract_statement_cross_refs`] is a second, independent kind
//! of statement scan this module owns: unlike [`classify_statement`], it
//! doesn't try to fully parse a statement into a `SpanBody` — it just pulls
//! out whatever role/tablespace references `crate::map::Builder::push_statement_span`
//! feeds it, from statement shapes [`classify_statement`] does not recognize
//! (`OWNER TO`, `GRANT`/`REVOKE`/`ALTER DEFAULT PRIVILEGES FOR ROLE`, `SET
//! default_tablespace`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::copy::Cursor;
use crate::lex::{Lexer, Region, ident_cont};
use crate::map::{Span, SpanBody};

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
    /// version headers carried over ([`dump_metadata_from_spans`]'s `Connect`
    /// arm).
    pub name: Option<String>,
    /// Whether this database's preamble was read to completion. Always
    /// `true` for every entry a [`crate::index::build_index`] full scan or a
    /// [`crate::index::scan_preamble`] prepass produces, neither ever leaving
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
    /// staying flat in dump bytes, over a third of it live structure the
    /// preamble alone pays (`measurements.md`, `peak-rss` and `rss-attribution`).
    /// **(c) unowned**; promoted by a dump with tens of thousands of tables,
    /// nothing in hand being one. It is also why every "resident set" claim
    /// about this system is the *one-block* reading and says so.
    pub tables: BTreeMap<String, TableDef>,
}

impl DatabaseMetadata {
    /// The declaration of column `column` of table `table`: the table's own,
    /// else its `OF` type's field, else the first parent's, in `INHERITS`
    /// order, that declares it — each parent asked the same way, so a
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

    fn empty(name: Option<String>) -> Self {
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
/// Every field is the dump's own text, never a conclusion (see the module
/// docs): `declared_type` is the type's words as written (`character
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
}

impl TableDef {
    /// A table declaring `columns` and referring to nothing — the ordinary
    /// shape, and the one a test or an embedder building metadata by hand
    /// wants.
    pub fn with_columns(columns: Vec<ColumnDef>) -> Self {
        Self { columns, ..Self::default() }
    }

    fn add(&mut self, reference: &TableReference) {
        match reference {
            TableReference::Parent(parent) if !self.parents.contains(parent) => {
                self.parents.push(parent.clone());
            }
            TableReference::Parent(_) => {}
            TableReference::OfType(of_type) => self.of_type = Some(of_type.clone()),
            TableReference::NotNull(column) => {
                self.not_null.push((column.clone(), NotNull::Inherited));
            }
        }
    }
}

/// What an `ALTER TABLE` adds to a table already declared: the references
/// `--binary-upgrade` writes after a full column list in place of the
/// `CREATE TABLE`'s own clauses, and the `NOT NULL` a dump before v18 writes
/// for a column the table does not print.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TableReference {
    /// `INHERIT <parent>`, qualified as [`TableDef::parents`] is.
    Parent(String),
    /// `OF <type>`, spelled as [`TableDef::of_type`] is.
    OfType(String),
    /// `ALTER [COLUMN] <column> SET NOT NULL`, into [`TableDef::not_null`].
    NotNull(String),
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
    /// VALUE` per label (I6); [`fold_alter_type_add_value`] folds them back
    /// in here so both forms produce the same shape.
    ///
    /// `exact` says the labels are every label the type holds when its
    /// database's preamble ends, which is what makes a field naming another
    /// one a refusal (I70). It is cleared by a label this build cannot lex
    /// and by an `ALTER TYPE` that could change the labels and is not read —
    /// `RENAME VALUE`, `ADD VALUE IF NOT EXISTS`, and any naming a type no
    /// definition here carries, which clears every enum's
    /// ([`fold_enum_labels_unread`]). Nothing `pg_dump` writes clears it.
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
    Domain { base_type: String, collation: Option<String>, not_null: bool },
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
    /// information-free; the dump says how the *server* parses it.
    Base,
    /// `CREATE TYPE x;` with no body at all, ahead of the real definition
    /// (forward-declaration shell type) or genuinely never completed.
    Shell,
}

impl TypeKind {
    /// A domain carrying no `COLLATE` clause and no `NOT NULL` of its own —
    /// the ordinary shape, and the one a test or an embedder building a type
    /// list by hand wants.
    pub fn domain(base_type: impl Into<String>) -> Self {
        Self::Domain { base_type: base_type.into(), collation: None, not_null: false }
    }

    /// An enum whose labels are exactly these, in declaration order — what
    /// `CREATE TYPE … AS ENUM (…)` declares, and the shape a test or an
    /// embedder building a type list by hand wants.
    pub fn exact_enum<S: Into<String>>(labels: impl IntoIterator<Item = S>) -> Self {
        Self::Enum { labels: labels.into_iter().map(Into::into).collect(), exact: true }
    }
}

/// Case-insensitive substring search, since none of `str`'s own methods do
/// one and the DDL grammar below only ever cares about a handful of fixed
/// ASCII keywords.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let hay = haystack.as_bytes();
    let pat = needle.as_bytes();
    if pat.is_empty() || hay.len() < pat.len() {
        return None;
    }
    (0..=hay.len() - pat.len()).find(|&i| hay[i..i + pat.len()].eq_ignore_ascii_case(pat))
}

/// Case-insensitively strip a leading keyword ending at a word boundary,
/// returning the (whitespace stripped) remainder: the byte after it may not
/// continue an identifier, so `CREATE TABLE` is no prefix of
/// `CREATE TABLESPACE`. `pub(crate)` for [`crate::map::classify`]'s own
/// `ALTER TYPE` check.
pub(crate) fn strip_kw<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    let bytes = s.as_bytes();
    if bytes.len() < kw.len()
        || !bytes[..kw.len()].eq_ignore_ascii_case(kw.as_bytes())
        || bytes.get(kw.len()).is_some_and(|&b| ident_cont(b))
    {
        return None;
    }
    Some(s[kw.len()..].trim_start())
}

/// Parse a (possibly schema-qualified) identifier from the start of `s`,
/// same folding rules as [`crate::copy::parse_copy_header`]'s table names —
/// so the result matches [`crate::copy::CopyHeader::qualified_name`] exactly
/// for the same object. Returns the qualified name and how many bytes of `s`
/// it consumed. `pub(crate)` for [`crate::map`]'s `INSERT INTO` target
/// parsing, which needs the same identifier grammar this module already
/// hardened.
pub(crate) fn parse_qualified_name(s: &str) -> Option<(String, usize)> {
    let mut cur = Cursor::new(s.as_bytes());
    cur.skip_spaces();
    let first = cur.parse_ident()?;
    let name = if cur.eat_byte(b'.') {
        let second = cur.parse_ident()?;
        format!("{first}.{second}")
    } else {
        first
    };
    Some((name, cur.pos()))
}

/// Parse a (possibly schema-qualified) type name from the start of `s` into
/// its **canonical spelling**, returning it and how many bytes of `s` it
/// consumed — what [`TypeDef::name`] holds, and what every lookup of a type by
/// name compares ([`canonical_type_name`]).
///
/// Each part is read as the server reads it — an unquoted identifier folded to
/// lower case, a quoted one taken verbatim (I29) — and written back bare where
/// it is a plain lower-case identifier, quoted with `""` doubled everywhere
/// else. So `public."Mood"`, `PUBLIC."Mood"` and `"public"."Mood"` agree while
/// `public.Mood` is `public.mood`, and no two distinct names share a spelling:
/// a bare part holds no `.` or `"`. It is `fmtId()`'s spelling but for a
/// keyword, which `pg_dump` quotes and this leaves bare.
pub(crate) fn parse_type_name(s: &str) -> Option<(String, usize)> {
    fn push_part(out: &mut String, part: &str) {
        let bytes = part.as_bytes();
        let plain = bytes.first().is_some_and(|b| b.is_ascii_lowercase() || *b == b'_')
            && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_');
        if plain {
            out.push_str(part);
        } else {
            out.push('"');
            out.push_str(&part.replace('"', "\"\""));
            out.push('"');
        }
    }
    let mut cur = Cursor::new(s.as_bytes());
    cur.skip_spaces();
    let mut name = String::new();
    push_part(&mut name, &cur.parse_ident()?);
    let before_dot = cur.pos();
    cur.skip_spaces();
    if cur.eat_byte(b'.') {
        cur.skip_spaces();
        name.push('.');
        push_part(&mut name, &cur.parse_ident()?);
        return Some((name, cur.pos()));
    }
    Some((name, before_dot))
}

/// `reference` — a declared type, a domain's base, a multirange companion's
/// name — in [`parse_type_name`]'s canonical spelling, or `None` where it is
/// not one type name alone (`integer[]`, `double precision`, text the
/// identifier grammar does not accept). Applied to both sides of a lookup, so
/// two spellings of one name find each other and nothing else does.
pub(crate) fn canonical_type_name(reference: &str) -> Option<String> {
    let (name, consumed) = parse_type_name(reference)?;
    reference[consumed..].trim().is_empty().then_some(name)
}

/// Skip over the single-quoted string literal starting at `open_idx`,
/// returning the index just past its closing quote — or `bytes.len()` for an
/// unterminated one, which lets a caller's scan terminate rather than loop.
///
/// One implementation of the quoting rule, `''` being an escaped quote rather
/// than the end of the string, for every scanner below that steps over a
/// literal.
fn skip_quoted(bytes: &[u8], open_idx: usize) -> usize {
    debug_assert_eq!(bytes.get(open_idx), Some(&b'\''));
    let mut i = open_idx + 1;
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            if bytes.get(i + 1) == Some(&b'\'') {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    i
}

/// Skip over the double-quoted identifier starting at `open_idx`, returning
/// the index just past its closing quote — `""` being the escape, exactly as
/// `''` is inside a string literal. Unterminated behaves like
/// [`skip_quoted`]'s: the scan ends rather than looping.
fn skip_double_quoted(bytes: &[u8], open_idx: usize) -> usize {
    debug_assert_eq!(bytes.get(open_idx), Some(&b'"'));
    let mut i = open_idx + 1;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            if bytes.get(i + 1) == Some(&b'"') {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    i
}

/// Whether byte `i` starts a word — i.e. the byte before it cannot be part of
/// an identifier. Keeps a keyword search from matching inside a longer name.
fn is_word_start(bytes: &[u8], i: usize) -> bool {
    let Some(prev) = i.checked_sub(1).and_then(|p| bytes.get(p)) else { return true };
    !(prev.is_ascii_alphanumeric() || *prev == b'_' || *prev == b'$' || *prev >= 0x80)
}

/// The `COLLATE <collation>` clause of a column or domain definition, with
/// the reference returned **verbatim** (`pg_catalog."C"`) — L1 stores what the
/// dump said, and [`crate::pgtype`] decides what it means.
///
/// `rest` is the tail after a column's name, or a domain's base-type tail, so
/// the clause is not adjacent to the type: `pg_dump` writes it *after*
/// `DEFAULT` and `NOT NULL` (I37), which is why this is a scan rather than a
/// look at the token following the type words. The scan is top-level only —
/// paren depth zero, outside `'…'` and `"…"` — so a `CHECK (v COLLATE "C" >
/// 'a')` constraint and a `DEFAULT 'collate me'` literal are both stepped
/// over rather than mistaken for the column's own clause.
fn extract_collation(rest: &str) -> Option<String> {
    const KW: &[u8] = b"COLLATE";
    let bytes = rest.as_bytes();
    let mut depth: i32 = 0;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                i = skip_quoted(bytes, i);
                continue;
            }
            b'"' => {
                i = skip_double_quoted(bytes, i);
                continue;
            }
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        let matched = depth == 0
            && is_word_start(bytes, i)
            && bytes.len() >= i + KW.len()
            && bytes[i..i + KW.len()].eq_ignore_ascii_case(KW)
            && bytes.get(i + KW.len()).is_some_and(u8::is_ascii_whitespace);
        if matched {
            let after = &rest[i + KW.len()..];
            let (_, consumed) = parse_qualified_name(after)?;
            return Some(after[..consumed].trim().to_string());
        }
        i += 1;
    }
    None
}

/// Find the index of `bytes[open_idx..]`'s matching `)`, respecting
/// single-quoted strings and double-quoted identifiers, so a literal like
/// `')'` or a name like `s."a)"` (I29) never confuses the depth count.
fn matching_paren(bytes: &[u8], open_idx: usize) -> Option<usize> {
    debug_assert_eq!(bytes.get(open_idx), Some(&b'('));
    let mut depth: i32 = 0;
    let mut i = open_idx;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                i = skip_quoted(bytes, i);
                continue;
            }
            b'"' => {
                i = skip_double_quoted(bytes, i);
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Split `s` on top-level commas — not ones nested inside parens, an
/// `ARRAY[…]` constructor's or a subscript's brackets, a single-quoted string
/// or a double-quoted identifier — trimming and dropping empty fragments.
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut parts = Vec::new();
    let mut depth: i32 = 0;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                i = skip_quoted(bytes, i);
                continue;
            }
            b'"' => {
                i = skip_double_quoted(bytes, i);
                continue;
            }
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&s[start..]);
    parts.into_iter().map(str::trim).filter(|p| !p.is_empty()).collect()
}

/// Parse a single SQL string literal (`'...'`, `''` as an escaped quote)
/// starting at `s`'s first non-whitespace byte, ending at its closing quote;
/// whatever follows that is not read.
fn parse_string_literal(s: &str) -> Option<String> {
    let mut chars = s.trim().chars().peekable();
    if chars.next() != Some('\'') {
        return None;
    }
    let mut out = String::new();
    loop {
        let c = chars.next()?;
        if c == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                out.push('\'');
                continue;
            }
            return Some(out);
        }
        out.push(c);
    }
}

/// Column/constraint-keyword boundary: a column or composite-field
/// definition is `<type words...> [constraint...]`, and `pg_dump` never puts
/// a space inside a type's own parenthesized modifier (`numeric(38,10)`,
/// `character varying(16)`), so whitespace-splitting the tail after the name
/// — outside a quoted identifier, whose spaces are its own (I29) — and
/// stopping at the first of these is enough to isolate it — no need to parse
/// constraint syntax at all.
const STOP_WORDS: &[&str] = &[
    "NOT",
    "DEFAULT",
    "COLLATE",
    "GENERATED",
    "PRIMARY",
    "REFERENCES",
    "CHECK",
    "UNIQUE",
    "CONSTRAINT",
];

/// Strip `/* ... */` block comments — the literal shape `pg_dump` writes a
/// `--binary-upgrade`-recreated dropped column's placeholder type in
/// (`INTEGER /* dummy */`, I5; `fixtures/*/edge_cases/binary-upgrade.sql`) —
/// stepping over string literals and quoted identifiers, inside which `/*`
/// is text (I29). Assumes no nesting, which matches every comment `pg_dump`
/// emits.
fn strip_block_comments(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut copied, mut i) = (0usize, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => i = skip_quoted(bytes, i),
            b'"' => i = skip_double_quoted(bytes, i),
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                out.push_str(&s[copied..i]);
                i = s[i + 2..].find("*/").map_or(bytes.len(), |end| i + 2 + end + 2);
                copied = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&s[copied..]);
    out
}

/// The whitespace-separated words of `s`, a quoted identifier being one word
/// with whatever it holds — spaces and all (I29).
fn type_tokens(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i = if bytes[i] == b'"' { skip_double_quoted(bytes, i) } else { i + 1 };
        }
        tokens.push(&s[start..i]);
    }
    tokens
}

fn extract_type_words(rest: &str) -> String {
    let rest = strip_block_comments(rest);
    let mut words = Vec::new();
    for tok in type_tokens(&rest) {
        if STOP_WORDS.iter().any(|kw| tok.eq_ignore_ascii_case(kw)) {
            break;
        }
        words.push(tok);
    }
    let mut joined = words.join(" ");
    // A domain's base type with no trailing constraint runs straight into
    // the statement's own closing `;` (`CREATE DOMAIN x AS integer;`), since
    // nothing upstream of here strips it — unlike a column fragment, which
    // is always pre-split from its enclosing parens and so never has one.
    if joined.ends_with(';') {
        joined.pop();
    }
    joined
}

/// The words of `rest` at its top level — outside parens, `'…'` and `"…"` —
/// as keywords: a quoted identifier, and a word a `.` qualifies, being no
/// keyword, are each an empty word, so no sequence matches across them.
fn top_level_words(rest: &str) -> Vec<&str> {
    let bytes = rest.as_bytes();
    let mut words = Vec::new();
    let mut depth: i32 = 0;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                i = skip_quoted(bytes, i);
                continue;
            }
            b'"' => {
                i = skip_double_quoted(bytes, i);
                if depth == 0 {
                    words.push("");
                }
                continue;
            }
            b'(' => depth += 1,
            b')' => depth -= 1,
            // A number's run, `1e5` say, is read whole and is no word.
            b if ident_cont(b) => {
                let start = i;
                while i < bytes.len() && ident_cont(bytes[i]) {
                    i += 1;
                }
                let keyword = depth == 0
                    && !bytes[start].is_ascii_digit()
                    && bytes[start] != b'$'
                    && (start == 0 || bytes[start - 1] != b'.');
                if depth == 0 {
                    words.push(if keyword { &rest[start..i] } else { "" });
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    words
}

/// The `NOT NULL` among a column's or a domain's constraints — `rest` being
/// the tail after its name or `AS` — or what makes the column one: an inline
/// `PRIMARY KEY`, or `GENERATED … AS IDENTITY` (I76). Read off the top-level
/// words, so a `CHECK (v IS NOT NULL)`, a `GENERATED ALWAYS AS (…)` expression
/// and a `'NOT NULL'` literal are stepped over; a default cannot hold `IS
/// NOT NULL` outside parens, gram.y's `b_expr` having no `IS NULL`.
fn not_null_clause(rest: &str) -> Option<NotNull> {
    let rest = strip_block_comments(rest);
    let words = top_level_words(&rest);
    let is = |at: usize, word: &str| words.get(at).is_some_and(|w| w.eq_ignore_ascii_case(word));
    let mut found = None;
    for at in 0..words.len() {
        if is(at, "NOT") && is(at + 1, "NULL") {
            let no_inherit = is(at + 2, "NO") && is(at + 3, "INHERIT");
            found =
                found.or(Some(if no_inherit { NotNull::NoInherit } else { NotNull::Inherited }));
        } else if (is(at, "PRIMARY") && is(at + 1, "KEY"))
            || (is(at, "AS") && is(at + 1, "IDENTITY"))
        {
            found = Some(NotNull::Inherited);
        }
    }
    found
}

/// Parse one `<name> <type> [constraints...]` fragment from a column or
/// composite-field list.
fn parse_column_fragment(frag: &str) -> Option<ColumnDef> {
    let frag = frag.trim();
    if frag.is_empty() {
        return None;
    }
    let mut cur = Cursor::new(frag.as_bytes());
    let name = cur.parse_ident()?;
    let rest = &frag[cur.pos()..];
    let declared_type = extract_type_words(rest);
    if declared_type.is_empty() {
        return None;
    }
    // `serial` and its kin are no type but a column made `NOT NULL`, as
    // gram.y's caller `transformColumnDefinition` makes it (I76).
    let serial = ["smallserial", "serial2", "serial", "serial4", "bigserial", "serial8"]
        .iter()
        .any(|kind| declared_type.eq_ignore_ascii_case(kind));
    let not_null = not_null_clause(rest).or(serial.then_some(NotNull::Inherited));
    Some(ColumnDef { name, declared_type, collation: extract_collation(rest), not_null })
}

/// `CREATE TABLE <name> (<col> <type>, ...) [INHERITS (<parent>, ...)] …;`,
/// or a typed table's `CREATE TABLE <name> OF <type> [(<options>, ...)] …;`
/// (gram.y's `CreateStmt`), or a table with no list at all — I5 — just
/// `<name>`.
///
/// A typed table's list is gram.y's `TypedTableElement`, a column's options
/// or a table constraint, and never holds a type, so it declares no column of
/// its own: each is its type's field ([`TableDef`]), and only a `NOT NULL`
/// among its options is kept.
fn parse_create_table(rest: &str) -> Option<(String, TableDef)> {
    let (name, consumed) = parse_qualified_name(rest)?;
    let mut after = rest[consumed..].trim_start();
    let mut table = TableDef::default();
    if let Some(of) = strip_kw(after, "OF") {
        let (of_type, consumed) = parse_type_name(of)?;
        table.of_type = Some(of_type);
        after = of[consumed..].trim_start();
    }
    if !after.starts_with('(') {
        return Some((name, table));
    }
    let close = matching_paren(after.as_bytes(), 0)?;
    let typed = table.of_type.is_some();
    for fragment in split_top_level_commas(&after[1..close]) {
        match parse_table_element(fragment, typed) {
            Some(TableElement::Column(column)) => table.columns.push(column),
            Some(TableElement::NotNull(columns, not_null)) => {
                table.not_null.extend(columns.into_iter().map(|column| (column, not_null)));
            }
            Some(TableElement::Constraint | TableElement::Like) | None => {}
        }
    }
    // A parent the identifier grammar refuses reaches nothing, as an unparsed
    // column fragment does, rather than costing the table its own columns.
    if let Some(inherits) = strip_kw(after[close + 1..].trim_start(), "INHERITS")
        && inherits.starts_with('(')
        && let Some(close) = matching_paren(inherits.as_bytes(), 0)
    {
        table.parents = split_top_level_commas(&inherits[1..close])
            .into_iter()
            .filter_map(whole_table_name)
            .collect();
    }
    Some((name, table))
}

/// `s`, trimmed, as one table name and nothing more, qualified as
/// [`parse_qualified_name`] qualifies it.
fn whole_table_name(s: &str) -> Option<String> {
    let (name, consumed) = parse_qualified_name(s)?;
    s[consumed..].trim().is_empty().then_some(name)
}

/// `ALTER [FOREIGN] TABLE [ONLY] <name> INHERIT <parent>;` or `… OF
/// <type>;` — the two references `--binary-upgrade` writes
/// after a table's full column list (`dumpTableSchema`) — or `… ALTER
/// [COLUMN] <column> SET NOT NULL;`, which a dump before v18 writes after a
/// `CREATE TABLE` not printing the column (I76), as the table they
/// alter and what they add. `None` for every other `ALTER TABLE`,
/// one listing several subcommands included, which the dump never writes for
/// these. `pub(crate)` for [`crate::map::classify`], as
/// [`parse_alter_type_add_value_body`] is.
pub(crate) fn parse_alter_table_reference(stmt: &str) -> Option<(String, TableReference)> {
    let stmt = stmt.trim_start();
    let rest = strip_kw(stmt, "ALTER TABLE").or_else(|| strip_kw(stmt, "ALTER FOREIGN TABLE"))?;
    let rest = strip_kw(rest, "ONLY").unwrap_or(rest);
    let (table, consumed) = parse_qualified_name(rest)?;
    let rest = rest[consumed..].trim_start();
    let (reference, tail) = if let Some(inherit) = strip_kw(rest, "INHERIT") {
        let (parent, consumed) = parse_qualified_name(inherit)?;
        (TableReference::Parent(parent), &inherit[consumed..])
    } else if let Some(alter) = strip_kw(rest, "ALTER") {
        let column = strip_kw(alter, "COLUMN").unwrap_or(alter);
        let mut cur = Cursor::new(column.as_bytes());
        let name = cur.parse_ident()?;
        let set = strip_kw(column[cur.pos()..].trim_start(), "SET")?;
        let tail = strip_kw(strip_kw(set, "NOT")?, "NULL")?;
        (TableReference::NotNull(name), tail)
    } else {
        let of = strip_kw(rest, "OF")?;
        let (of_type, consumed) = parse_type_name(of)?;
        (TableReference::OfType(of_type), &of[consumed..])
    };
    (tail.trim() == ";").then_some((table, reference))
}

/// One top-level fragment of a `CREATE TABLE` list: gram.y's `TableElement`,
/// a `columnDef`, a `TableConstraint` or a `TableLikeClause` — or, in a typed
/// table's list, a `TypedTableElement`, a column's options or a constraint.
///
/// A column is held, and the columns a constraint makes `NOT NULL`; the rest
/// is told apart and dropped: no reader reads another table constraint, and a
/// `LIKE`'s columns are not followed.
enum TableElement {
    Column(ColumnDef),
    /// v18's `[CONSTRAINT <name>] NOT NULL <column> [NO INHERIT]`, a `PRIMARY
    /// KEY (…)`'s columns, or a typed table's column options holding a `NOT
    /// NULL` (I76).
    NotNull(Vec<String>, NotNull),
    Constraint,
    Like,
}

/// Which [`TableElement`] `frag` is, told by its first word: a constraint or
/// a `LIKE` opens with a keyword no column's name can be written as bare, but
/// for `EXCLUDE`, which opens one only where `USING` or `(` follows (I53).
/// `NOT` is 18's table-level `NOT NULL <column>`. In a `typed` table's list
/// anything else is a column's options, `<name> [WITH OPTIONS] <constraints>`,
/// which declares no type.
fn parse_table_element(frag: &str, typed: bool) -> Option<TableElement> {
    const CONSTRAINT_WORDS: &[&str] =
        &["CONSTRAINT", "CHECK", "UNIQUE", "PRIMARY", "FOREIGN", "NOT"];
    let frag = frag.trim_start();
    if CONSTRAINT_WORDS.iter().any(|kw| strip_kw(frag, kw).is_some()) {
        return Some(table_constraint_not_null(frag).unwrap_or(TableElement::Constraint));
    }
    if strip_kw(frag, "LIKE").is_some() {
        return Some(TableElement::Like);
    }
    if let Some(after) = strip_kw(frag, "EXCLUDE")
        && (after.starts_with('(') || strip_kw(after, "USING").is_some())
    {
        return Some(TableElement::Constraint);
    }
    if typed {
        let mut cur = Cursor::new(frag.as_bytes());
        let name = cur.parse_ident()?;
        let not_null = not_null_clause(&frag[cur.pos()..])?;
        return Some(TableElement::NotNull(vec![name], not_null));
    }
    parse_column_fragment(frag).map(TableElement::Column)
}

/// The columns table constraint `frag` makes `NOT NULL`, where it is one that
/// does: `[CONSTRAINT <name>] NOT NULL <column> [NO INHERIT]`, which v18
/// writes for a column the table does not print, or `[CONSTRAINT <name>]
/// PRIMARY KEY (<column>, …)` (I76).
fn table_constraint_not_null(frag: &str) -> Option<TableElement> {
    let mut rest = frag.trim_start();
    if let Some(named) = strip_kw(rest, "CONSTRAINT") {
        let mut cur = Cursor::new(named.as_bytes());
        cur.parse_ident()?;
        rest = named[cur.pos()..].trim_start();
    }
    if let Some(column) = strip_kw(rest, "NOT").and_then(|not| strip_kw(not, "NULL")) {
        let mut cur = Cursor::new(column.as_bytes());
        let name = cur.parse_ident()?;
        let tail = column[cur.pos()..].trim_start();
        let no_inherit = strip_kw(tail, "NO").is_some_and(|t| strip_kw(t, "INHERIT").is_some());
        let not_null = if no_inherit { NotNull::NoInherit } else { NotNull::Inherited };
        return Some(TableElement::NotNull(vec![name], not_null));
    }
    let key = strip_kw(strip_kw(rest, "PRIMARY")?, "KEY")?;
    if !key.starts_with('(') {
        return None;
    }
    let close = matching_paren(key.as_bytes(), 0)?;
    let columns = split_top_level_commas(&key[1..close])
        .into_iter()
        .map(|column| {
            let mut cur = Cursor::new(column.as_bytes());
            let name = cur.parse_ident()?;
            column[cur.pos()..].trim().is_empty().then_some(name)
        })
        .collect::<Option<Vec<_>>>()?;
    Some(TableElement::NotNull(columns, NotNull::Inherited))
}

/// `CREATE DOMAIN <name> AS <basetype> [COLLATE ...] [constraints...];`
fn parse_create_domain(rest: &str) -> Option<TypeDef> {
    let (name, consumed) = parse_type_name(rest)?;
    let after_as = strip_kw(rest[consumed..].trim_start(), "AS")?;
    let base_type = extract_type_words(after_as);
    if base_type.is_empty() {
        return None;
    }
    let collation = extract_collation(after_as);
    let not_null = not_null_clause(after_as).is_some();
    Some(TypeDef { name, kind: TypeKind::Domain { base_type, collation, not_null } })
}

/// `CREATE EXTENSION [IF NOT EXISTS] <name> [WITH] [SCHEMA <schema>];`
fn parse_create_extension(rest: &str) -> Option<Extension> {
    let rest = strip_kw(rest, "IF NOT EXISTS").unwrap_or(rest);
    let mut cur = Cursor::new(rest.as_bytes());
    cur.skip_spaces();
    let name = cur.parse_ident()?;
    let after = &rest[cur.pos()..];
    let schema = find_ci(after, "SCHEMA").and_then(|idx| {
        let mut c = Cursor::new(&after.as_bytes()[idx + "SCHEMA".len()..]);
        c.skip_spaces();
        c.parse_ident()
    });
    Some(Extension { name, schema })
}

/// `CREATE COLLATION <name> (provider = …[, deterministic = false][, locale =
/// …][, rules = …]);` — or the `CREATE COLLATION <name> FROM <other>;` copy
/// form, which carries no option list at all.
///
/// Only `deterministic` is read, and only at the option list's top level: a
/// `locale = 'x, deterministic = false'` literal is stepped over, since
/// [`split_top_level_commas`] respects quoting and an ICU locale is an
/// arbitrary string the server never re-quotes on the way out.
///
/// The copy form parses as `deterministic = true`, which is *not* the same
/// claim as reading the source collation's own determinism: `CREATE COLLATION
/// x FROM y` copies `collisdeterministic`, so a copy of a non-deterministic
/// collation would be called deterministic here. `pg_dump` never writes the
/// copy form (I42), so the shape is reachable only from a hand-written file,
/// where under-claiming costs an unprinted note rather than a wrong row
/// set.
fn parse_create_collation(rest: &str) -> Option<CollationDef> {
    let (name, consumed) = parse_type_name(rest)?;
    let after = rest[consumed..].trim_start();
    let mut deterministic = true;
    if after.starts_with('(') {
        let close = matching_paren(after.as_bytes(), 0)?;
        for option in split_top_level_commas(&after[1..close]) {
            let Some((key, value)) = option.split_once('=') else { continue };
            if key.trim().eq_ignore_ascii_case("deterministic")
                && value.trim().eq_ignore_ascii_case("false")
            {
                deterministic = false;
            }
        }
    }
    Some(CollationDef { name, deterministic })
}

/// `CREATE TYPE <name>` in any of its six shapes, which reach five of
/// [`TypeKind`]'s variants — [`TypeKind::Domain`] comes from
/// [`parse_create_domain`] instead.
fn parse_create_type(rest: &str) -> Option<TypeDef> {
    let (name, consumed) = parse_type_name(rest)?;
    let after = rest[consumed..].trim_start();

    if after.is_empty() || after.starts_with(';') {
        return Some(TypeDef { name, kind: TypeKind::Shell });
    }
    if let Some(body) = strip_kw(after, "AS ENUM") {
        let open = body.find('(')?;
        let close = matching_paren(body.as_bytes(), open)?;
        // A fragment that is no plain string literal — an `E''` one, say — is
        // a label this build cannot read, so the set it leaves is short.
        let fragments = split_top_level_commas(&body[open + 1..close]);
        let labels: Vec<String> =
            fragments.iter().copied().filter_map(parse_string_literal).collect();
        let exact = labels.len() == fragments.len();
        return Some(TypeDef { name, kind: TypeKind::Enum { labels, exact } });
    }
    if let Some(body) = strip_kw(after, "AS RANGE") {
        let open = body.find('(')?;
        let close = matching_paren(body.as_bytes(), open)?;
        let mut subtype = None;
        let mut multirange_type_name = None;
        let mut canonical = None;
        for kv in split_top_level_commas(&body[open + 1..close]) {
            let Some((k, v)) = kv.split_once('=') else { continue };
            let k = k.trim();
            let v = v.trim().to_string();
            if k.eq_ignore_ascii_case("subtype") {
                subtype = Some(v);
            } else if k.eq_ignore_ascii_case("multirange_type_name") {
                multirange_type_name = Some(canonical_type_name(&v).unwrap_or(v));
            } else if k.eq_ignore_ascii_case("canonical") {
                canonical = Some(v);
            }
        }
        return Some(TypeDef {
            name,
            kind: TypeKind::Range { subtype, multirange_type_name, canonical },
        });
    }
    if let Some(body) = strip_kw(after, "AS") {
        let body = body.trim_start();
        if body.starts_with('(') {
            let close = matching_paren(body.as_bytes(), 0)?;
            let inner = &body[1..close];
            // All-or-nothing (see [`TypeKind::Composite`]): one unparseable
            // fragment discards the whole list rather than shortening it. An
            // empty body is a zero-field composite, not a failure (I23).
            let fields = if inner.trim().is_empty() {
                Some(Vec::new())
            } else {
                split_top_level_commas(inner).into_iter().map(parse_column_fragment).collect()
            };
            return Some(TypeDef { name, kind: TypeKind::Composite { fields } });
        }
        return Some(TypeDef { name, kind: TypeKind::Base });
    }
    // `CREATE TYPE x (INPUT = ..., OUTPUT = ...);` — the C-level base type
    // shape, with no `AS` at all.
    Some(TypeDef { name, kind: TypeKind::Base })
}

/// Parse the body of `ALTER TYPE <name> ADD VALUE '<label>' [BEFORE|AFTER
/// '<other>'];` — the `--binary-upgrade` shape for enum labels (I6) — after
/// `ALTER TYPE` has already been stripped. `BEFORE`/`AFTER` is ignored: a
/// `--binary-upgrade` dump only ever emits these in declaration order
/// (`fixtures/*/types/binary-upgrade.sql`), so appending is equivalent to
/// respecting them. Used by [`crate::map::classify`] to recognize the
/// statement as its own [`crate::map::SpanBody::AlterTypeAddValue`] span,
/// since that module has no already-open `TypeDef` to fold into the way
/// [`fold_alter_type_add_value`] does for [`dump_metadata_from_spans`].
// deficiency: KD89 — a hand-written `ADD VALUE … BEFORE` or `AFTER` placing
// the label anywhere but last is folded as appended, so the label orders after
// every other: an ordering term, a group's bounds and the pruning they drive
// place it where the server does not, and a query can lose its rows.
pub(crate) fn parse_alter_type_add_value_body(rest: &str) -> Option<(String, String)> {
    let (name, consumed) = parse_type_name(rest)?;
    let after = strip_kw(strip_kw(rest[consumed..].trim_start(), "ADD")?, "VALUE")?;
    let label = parse_string_literal(after)?;
    Some((name, label))
}

/// Whether the body of an `ALTER TYPE`, after `ALTER TYPE` has been stripped,
/// is one that could change an enum's labels — `ADD VALUE`, `RENAME VALUE`,
/// in any form, the keyword `VALUE` being in no other subcommand, so the
/// word anywhere after the name counts, an identifier's included — answered
/// where [`parse_alter_type_add_value_body`] does not read it, as the type it
/// names, `None` where it names none this grammar reads. Such a statement
/// leaves the labels inexact ([`fold_enum_labels_unread`]). `pub(crate)` for
/// [`crate::map::classify`], as [`parse_alter_type_add_value_body`] is.
pub(crate) fn alter_type_labels_unread(rest: &str) -> Option<Option<String>> {
    let (name, after) = match parse_type_name(rest) {
        Some((name, consumed)) => (Some(name), &rest[consumed..]),
        None => (None, rest),
    };
    holds_word(after, "VALUE").then_some(name)
}

/// Whether `haystack` holds `word`, in any case, bounded on both sides by a
/// byte no identifier continues with.
fn holds_word(haystack: &str, word: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut from = 0;
    while let Some(at) = find_ci(&haystack[from..], word).map(|i| from + i) {
        let end = at + word.len();
        let before = at.checked_sub(1).map(|i| bytes[i]);
        if !before.is_some_and(ident_cont) && !bytes.get(end).copied().is_some_and(ident_cont) {
            return true;
        }
        from = at + 1;
    }
    false
}

/// Parse the body of `ALTER TYPE <name> DROP ATTRIBUTE <attr>;`, after `ALTER
/// TYPE` has been stripped — the statement a `--binary-upgrade` dump drops a
/// composite's placeholder for a dropped attribute with (I5) — as the type
/// and the attribute. `None` for anything more than that
/// one subcommand, which `dumpCompositeType` never writes. `pub(crate)` for
/// [`crate::map::classify`], as [`parse_alter_type_add_value_body`] is.
pub(crate) fn parse_alter_type_drop_attribute_body(rest: &str) -> Option<(String, String)> {
    let (name, consumed) = parse_type_name(rest)?;
    let after = strip_kw(rest[consumed..].trim_start(), "DROP ATTRIBUTE")?;
    let mut cur = Cursor::new(after.as_bytes());
    let attribute = cur.parse_ident()?;
    (after[cur.pos()..].trim() == ";").then_some((name, attribute))
}

/// Record one `CREATE TYPE`/`CREATE DOMAIN` into `types`, keyed on the type
/// name rather than on the statement — one entry per type, not one per
/// statement (`docs/design/decisions.md`, "D36"): `pg_dump` emits a completed
/// C-level base type *twice* under one name, `CREATE TYPE x;` under
/// `SHELL TYPE` and then the full definition (I11).
///
/// Two rules, the second of which is what makes the shell lose: a definition
/// for a name already present **replaces** it, and a [`TypeKind::Shell`] never
/// replaces anything. Replacement is in place, so the list stays in the order
/// each name was first declared.
fn record_type(types: &mut Vec<TypeDef>, name: &str, kind: &TypeKind) {
    match types.iter_mut().find(|t| t.name == name) {
        Some(_) if matches!(kind, TypeKind::Shell) => {}
        Some(existing) => existing.kind = kind.clone(),
        None => types.push(TypeDef { name: name.to_string(), kind: kind.clone() }),
    }
}

/// Fold an already-parsed `ALTER TYPE <type_name> ADD VALUE '<label>'` (see
/// [`parse_alter_type_add_value_body`]) into the matching `TypeDef` in
/// `types`, if any — a no-op if the type it names isn't an `Enum`.
/// Used by [`dump_metadata_from_spans`], which encounters the label as its
/// own [`crate::map::SpanBody::AlterTypeAddValue`] span, separate from the
/// [`crate::map::SpanBody::TypeDef`] span it targets.
///
/// A name no definition here carries leaves every enum inexact, as
/// [`fold_enum_labels_unread`] does: the server may resolve it to one of
/// them (an unqualified name under a `search_path`) where this does not.
fn fold_alter_type_add_value(types: &mut [TypeDef], type_name: &str, label: &str) {
    match types.iter_mut().find(|t| t.name == type_name) {
        Some(TypeDef { kind: TypeKind::Enum { labels, .. }, .. }) => labels.push(label.to_string()),
        Some(_) => {}
        None => fold_enum_labels_unread(types, None),
    }
}

/// Fold an `ALTER TYPE` that could change an enum's labels and is not read
/// (see [`alter_type_labels_unread`]) into `types`: the enum it names keeps
/// its labels, which are no longer known to be all of them. Where it names
/// no definition here — or none this grammar reads — the type it changes
/// could be any enum declared so far, so every one is left inexact.
// deficiency: KD87 — only an `ALTER TYPE` is read for a change to the labels.
// Code that changes them otherwise — a `DO` block or a function a statement
// calls running one, a write to `pg_enum`, a psql `\i` — leaves them marked
// exact, so a field naming a label it added is refused at parse where a
// restore reads it. No `pg_dump` writes such code; a hand-written dump can.
fn fold_enum_labels_unread(types: &mut [TypeDef], type_name: Option<&str>) {
    let named = type_name.and_then(|name| types.iter().position(|t| t.name == name));
    for (i, def) in types.iter_mut().enumerate() {
        if let TypeKind::Enum { exact, .. } = &mut def.kind
            && named.is_none_or(|n| n == i)
        {
            *exact = false;
        }
    }
}

/// Fold an already-parsed `ALTER TYPE <type_name> DROP ATTRIBUTE <attribute>`
/// (see [`parse_alter_type_drop_attribute_body`]) into the matching composite
/// in `types`, removing the field so named — a no-op where the type, or the
/// field, is not there. The field is the placeholder `--binary-upgrade`
/// writes for an attribute the type has dropped, which `record_out` skips, so
/// a composite that kept it would expect a value more than every row holds.
fn fold_alter_type_drop_attribute(types: &mut [TypeDef], type_name: &str, attribute: &str) {
    if let Some(TypeDef { kind: TypeKind::Composite { fields: Some(fields) }, .. }) =
        types.iter_mut().find(|t| t.name == type_name)
    {
        fields.retain(|field| field.name != attribute);
    }
}

/// Case-insensitively find `marker` in `haystack` and parse the identifier
/// (bare, or double-quoted with `""` doubled — [`Cursor::parse_ident`]'s two
/// shapes, matching `fmtId()`'s two output forms) immediately following it.
fn ident_after(haystack: &str, marker: &str) -> Option<String> {
    let idx = find_ci(haystack, marker)?;
    let rest = haystack[idx + marker.len()..].trim_start();
    Cursor::new(rest.as_bytes()).parse_ident()
}

/// Filters out the pseudo-role `_printTocEntry`/`buildACLCommands` write
/// literally as `PUBLIC` whenever a grant/revoke's grantee list is empty:
/// `PUBLIC` is never reported as a role. [`ident_after`]'s
/// [`Cursor::parse_ident`] lowercases every *unquoted* identifier, so the
/// keyword arrives here as `public`, a name no role can take, quoted or not
/// (gram.y's `RoleSpec`).
// deficiency: KD91 — compared case-insensitively, so a role quoted
// `"PUBLIC"`, which a server can hold, is dropped too, and a tablespace
// quoted `"PG_DEFAULT"` by `insert_tablespace`.
pub(crate) fn insert_role(roles: &mut BTreeSet<String>, role: String) {
    if !role.eq_ignore_ascii_case("PUBLIC") {
        roles.insert(role);
    }
}

/// Filters out `pg_default`, the reserved, uncreatable name for a database's
/// implicit default tablespace — never reported as one, the same way
/// `PUBLIC` is filtered from roles, and compared as [`insert_role`] compares
/// it (`KD91`).
pub(crate) fn insert_tablespace(tablespaces: &mut BTreeSet<String>, tablespace: String) {
    if !tablespace.eq_ignore_ascii_case("pg_default") {
        tablespaces.insert(tablespace);
    }
}

/// Extract every role/tablespace a complete statement (see
/// [`statement_complete`]) references, per
/// `docs/design/decisions.md`'s "D31" —
/// the two sources [`crate::map::Span::toc`] alone can't cover, since none of
/// these three statement shapes is its own TOC entry:
///
/// - `ALTER <object> OWNER TO <role>;` (`pg_backup_archiver.c`'s
///   `_printTocEntry`, generic across every ownable object kind).
/// - `GRANT ... TO <role>;` / `REVOKE ... FROM <role>;`, standalone or
///   prefixed onto one line by `ALTER DEFAULT PRIVILEGES FOR ROLE <role> [IN
///   SCHEMA <schema>] ` (`dumputils.c`'s `buildACLCommands`/
///   `buildDefaultACLCommands`) — both the prefix's own role and the
///   grant/revoke's grantee are recorded.
/// - `SET default_tablespace = <tablespace>;` (`pg_backup_archiver.c`'s
///   `_selectTablespace`) — **not** recorded when the value is the quoted
///   empty string (`SET default_tablespace = '';`), which means "revert to
///   the database's own default," not a reference to a real tablespace.
///
/// Matching is by marker substring, the same tolerance
/// [`parse_toc_header_line`] documents: this only feeds enrichment, never a
/// span boundary, so an identifier containing a marker text (`" TO "`,
/// `" FROM "`) is an accepted source of a missed or mis-attributed
/// reference.
pub(crate) fn extract_statement_cross_refs(
    stmt: &str,
    roles: &mut BTreeSet<String>,
    tablespaces: &mut BTreeSet<String>,
) {
    if let Some(role) = ident_after(stmt, "OWNER TO ") {
        insert_role(roles, role);
    }
    if let Some(rest) = strip_kw(stmt.trim_start(), "ALTER DEFAULT PRIVILEGES FOR ROLE")
        && let Some(role) = Cursor::new(rest.as_bytes()).parse_ident()
    {
        insert_role(roles, role);
    }
    if find_ci(stmt, "GRANT ").is_some()
        && let Some(role) = ident_after(stmt, " TO ")
    {
        insert_role(roles, role);
    }
    if find_ci(stmt, "REVOKE ").is_some()
        && let Some(role) = ident_after(stmt, " FROM ")
    {
        insert_role(roles, role);
    }
    if let Some(idx) = find_ci(stmt, "SET default_tablespace") {
        let rest = &stmt[idx..];
        if let Some(eq) = rest.find('=') {
            let value = rest[eq + 1..].trim_start();
            if !value.starts_with('\'')
                && let Some(tablespace) = Cursor::new(value.as_bytes()).parse_ident()
            {
                insert_tablespace(tablespaces, tablespace);
            }
        }
    }
}

/// The four statement shapes [`classify_statement`] recognizes directly.
/// `ALTER TYPE ADD VALUE` and `DROP ATTRIBUTE` are not among them: each
/// mutates an already-declared object rather than introducing one, so it has
/// its own two entry points — [`parse_alter_type_add_value_body`] and
/// [`parse_alter_type_drop_attribute_body`] for [`crate::map::classify`],
/// [`fold_alter_type_add_value`] and [`fold_alter_type_drop_attribute`] for
/// [`dump_metadata_from_spans`] — and so, for the same reason, has an `ALTER
/// TABLE` adding a [`TableReference`] ([`parse_alter_table_reference`],
/// folded by [`TableDef`]).
#[derive(Debug)]
pub(crate) enum StatementShape {
    Table { name: String, definition: TableDef },
    Type(TypeDef),
    Extension(Extension),
    Collation(CollationDef),
}

/// Classify a complete statement (see [`statement_complete`]) as one of
/// this module's recognized `CREATE` shapes, or `None` for anything else —
/// including `ALTER TYPE ADD VALUE`, which needs an already-open `TypeDef`
/// to fold into (see [`StatementShape`]'s docs) rather than being
/// classifiable from its own text alone.
///
/// A table is any of the three `dumpTableSchema` writes as `CREATE %s%s %s`
/// (I54): a plain one, an `UNLOGGED` one and a `FOREIGN TABLE`, whose list
/// and `INHERITS` are a plain table's, its `SERVER` clause after them.
pub(crate) fn classify_statement(stmt: &str) -> Option<StatementShape> {
    let trimmed = stmt.trim_start();
    if let Some(rest) = ["CREATE TABLE", "CREATE UNLOGGED TABLE", "CREATE FOREIGN TABLE"]
        .into_iter()
        .find_map(|kw| strip_kw(trimmed, kw))
    {
        return parse_create_table(rest)
            .map(|(name, definition)| StatementShape::Table { name, definition });
    }
    if let Some(rest) = strip_kw(trimmed, "CREATE TYPE") {
        return parse_create_type(rest).map(StatementShape::Type);
    }
    if let Some(rest) = strip_kw(trimmed, "CREATE DOMAIN") {
        return parse_create_domain(rest).map(StatementShape::Type);
    }
    if let Some(rest) = strip_kw(trimmed, "CREATE EXTENSION") {
        return parse_create_extension(rest).map(StatementShape::Extension);
    }
    if let Some(rest) = strip_kw(trimmed, "CREATE COLLATION") {
        return parse_create_collation(rest).map(StatementShape::Collation);
    }
    None
}

/// `\connect <name>` — a bare prefix check on a line the scanner already
/// holds. Exposed to `crate::stream`'s live scan too
/// (`docs/design/decisions.md`, "D49"):
/// tracking which database a `CopyBlock` belongs to needs only the name a
/// `\connect` yields, never a column type, so it isn't "reading preamble as
/// it goes" in the sense that section rules out.
///
/// Both of `appendPsqlMetaConnect`'s forms (I55): the name as an identifier,
/// and for a name holding a byte outside `[A-Za-z0-9_.]`, `-reuse-previous=on`
/// and a connection string `dbname='<name>'` written as one.
pub(crate) fn parse_connect(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("\\connect ")?.trim();
    match rest.strip_prefix("-reuse-previous=on ") {
        Some(conninfo) => conninfo_dbname(&Cursor::new(conninfo.trim().as_bytes()).parse_ident()?),
        None => Cursor::new(rest.as_bytes()).parse_ident(),
    }
}

/// The name in a connection string that is `dbname='<name>'` and nothing
/// more, read as `appendConnStrVal` writes it: single-quoted, `\` escaping a
/// `'` or a `\`.
fn conninfo_dbname(conninfo: &str) -> Option<String> {
    let mut chars = conninfo.strip_prefix("dbname='")?.chars();
    let mut name = String::new();
    loop {
        match chars.next()? {
            '\\' => name.push(chars.next()?),
            '\'' => return chars.as_str().is_empty().then_some(name),
            c => name.push(c),
        }
    }
}

/// An incremental scan of a SQL statement's lines: how deep its parens are,
/// where [`crate::lex`] leaves it — the rules [`crate::scan::CopyScanner`]
/// lexes the same lines by, so a statement ends where the scanner's regions
/// say it can — whether its last line ends in a `--` comment, and what its
/// last non-whitespace byte was.
///
/// Line at a time, as the lexer is: [`crate::map`] holds every statement as
/// the lines the scanner surfaced. Byte-level rather than `char`-level: the
/// lexer tells a non-ASCII byte from an ASCII one and no further, and no ASCII
/// byte occurs inside a multi-byte UTF-8 sequence, so it answers alike over
/// raw bytes and over their lossy conversion, and an `INSERT` run is
/// classified off raw bytes with no `String` per line
/// (`docs/design/decisions.md`, "D33").
#[derive(Debug, Clone)]
pub(crate) struct StatementScan {
    lexer: Lexer,
    depth: i32,
    /// The last line fed ends in a `--` comment, which only the newline a
    /// further line brings would close.
    in_comment: bool,
    /// The last byte fed that is not [`is_sql_space`], or `0` if none is —
    /// the incremental form of `buf.trim_end()`'s last character.
    last_significant: u8,
    /// A non-empty line has been fed since the last [`reset`](Self::reset),
    /// so that [`is_empty`](Self::is_empty) answers what `buf.is_empty()`
    /// answered.
    fed: bool,
}

/// The whitespace `str::trim_end` strips, restricted to ASCII. A non-ASCII
/// Unicode space after a statement's `;` is therefore *not* trimmed here and
/// the statement reads as incomplete — `pg_dump` writes none, and the
/// consequence of being wrong is the graceful-degradation path
/// [`crate::map`] already takes for an unterminated statement.
const fn is_sql_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

impl StatementScan {
    /// A scan at a statement's start, under the dump's
    /// `standard_conforming_strings` (I50).
    pub(crate) const fn new(standard_strings: bool) -> Self {
        Self {
            lexer: Lexer::with_standard_strings(standard_strings),
            depth: 0,
            in_comment: false,
            last_significant: 0,
            fed: false,
        }
    }

    /// Start a fresh statement, under the same setting.
    pub(crate) fn reset(&mut self) {
        *self = Self::new(self.lexer.standard_strings());
    }

    /// Nothing has been fed since the last [`reset`](Self::reset) — the
    /// incremental equivalent of `buf.is_empty()`, and [`crate::map`]'s
    /// signal that the next line either opens a statement or ends the run.
    pub(crate) fn is_empty(&self) -> bool {
        !self.fed
    }

    /// Feed one more physical line, its newline stripped, as
    /// [`push_stmt_line`] joins them.
    pub(crate) fn feed_line(&mut self, line: &[u8]) {
        if line.is_empty() && !self.fed {
            return;
        }
        self.fed = true;
        if let Some(&b) = line.iter().rev().find(|&&b| !is_sql_space(b)) {
            self.last_significant = b;
        }
        let depth = &mut self.depth;
        // Deficiency register: `deficiency: KD9` — an `INSERT` run costs
        // several times a `COPY` scan's per-byte CPU warm, and the device is
        // what decides whether a reader meets it (`measurements.md`,
        // `scan-throughput-warm` and `scan-throughput-nvme`). Three cuts
        // against the remainder are known. No `INSERT` statement's end
        // depends on `depth`, so a run-only scan would skip this count;
        // `insert_run_line` (`map.rs`) feeds the `INSERT INTO <table>` prefix
        // it has just matched, whose lexing is provably a run of code, so
        // those bytes are crossed twice; and `CopyScanner` has already lexed
        // every line it surfaces by these rules, so a line's region, comment
        // and paren count could travel with its `Event::Line` rather than be
        // found again here. **(b) owned by P8**, whose row reader is the
        // caller that can say the count is dead weight; it is not a licence
        // to drop `depth`, which `statement_complete` reads.
        let found = self.lexer.line(line, |code| {
            for k in memchr::memchr2_iter(b'(', b')', code) {
                *depth += if code[k] == b'(' { 1 } else { -1 };
            }
        });
        self.in_comment = found.line_comment;
    }

    /// Whether what has been fed so far is a complete SQL statement: parens
    /// balanced, between tokens rather than inside a literal, identifier or
    /// comment, and ending in `;`.
    pub(crate) fn complete(&self) -> bool {
        *self.lexer.region() == Region::Code
            && !self.in_comment
            && self.depth == 0
            && self.last_significant == b';'
    }

    /// Whether the lines so far end inside a literal, a quoted identifier or
    /// a `/* */` comment — the one case where a line that *looks* like a
    /// fresh boundary (starts with `--`, in [`crate::map`]'s case) is really
    /// that region's content spanning physical lines. `pg_dump` never emits
    /// a `--` comment inside a non-dollar-quoted statement's own parens, so
    /// paren depth does not gate this the same way.
    pub(crate) fn in_quote(&self) -> bool {
        *self.lexer.region() != Region::Code
    }

    fn feed_text(&mut self, buf: &str) {
        for line in buf.split('\n') {
            self.feed_line(line.as_bytes());
        }
    }
}

/// Whether `buf` (everything accumulated for a statement so far, its lines
/// joined by [`push_stmt_line`]) is a complete SQL statement —
/// [`StatementScan::complete`] over a buffer a caller holds whole rather than
/// feeding line by line. One implementation for both, [`crate::map`] deciding
/// the same question from a `String` for a DDL statement and from raw bytes
/// for an `INSERT` run.
pub(crate) fn statement_complete(buf: &str, standard_strings: bool) -> bool {
    let mut scan = StatementScan::new(standard_strings);
    scan.feed_text(buf);
    scan.complete()
}

/// [`StatementScan::in_quote`] over a whole buffer — see that method.
pub(crate) fn in_open_quote(buf: &str, standard_strings: bool) -> bool {
    let mut scan = StatementScan::new(standard_strings);
    scan.feed_text(buf);
    scan.in_quote()
}

/// Append `line` to a statement buffer being accumulated line by line,
/// joining with `\n` — but never a leading one before the buffer's first
/// line. Used by [`crate::map`]'s general statement scan, whose spans carry
/// the statement's own text; an `INSERT` run's does not, and feeds
/// [`StatementScan::feed_line`] instead.
pub(crate) fn push_stmt_line(buf: &mut String, line: &str) {
    if !buf.is_empty() {
        buf.push('\n');
    }
    buf.push_str(line);
}

/// Mark a database's segment as read to completion — see
/// [`DatabaseMetadata::preamble_complete`]'s docs for what that means and who
/// relies on it. Shared by every place [`dump_metadata_from_spans`] closes
/// out a segment: a `\connect` boundary, and end of input.
fn finalize(mut db: DatabaseMetadata) -> DatabaseMetadata {
    db.preamble_complete = true;
    db
}

/// Build a [`DumpMetadata`] by walking already-classified [`crate::map::Span`]s
/// instead of raw lines — the derived view `docs/design/decisions.md`'s "D34"
/// calls for: multi-database segmenting on
/// [`crate::map::SpanBody::Connect`], version-header staging across that
/// boundary on [`crate::map::SpanBody::VersionHeader`], and `--binary-upgrade`
/// enum-label folding on [`crate::map::SpanBody::AlterTypeAddValue`] and
/// [`crate::map::SpanBody::EnumLabelsUnread`], dropped-attribute folding on
/// [`crate::map::SpanBody::AlterTypeDropAttribute`]
/// and table-reference folding on [`crate::map::SpanBody::AlterTableReference`].
///
/// `spans` must come from a scan that stops at one of two safe boundaries:
/// end of file, or (per I1) the start of the current database's first `COPY`
/// block — [`crate::stream::build_map`] and `crate::index::scan_preamble`'s own
/// span builder both only ever produce spans this way. A span list with an
/// `Unscanned` tail cut off anywhere else (e.g. `crate::stream::table_stream`'s
/// live segment) would make the trailing database's `preamble_complete` a lie.
pub fn dump_metadata_from_spans(spans: &[Span]) -> DumpMetadata {
    let mut seen_connect = false;
    let mut current = DatabaseMetadata::empty(None);
    let mut databases = Vec::new();
    let mut pending_headers: (Option<String>, Option<String>) = (None, None);

    for span in spans {
        match &span.body {
            // I58: one `pg_dump` invocation `\connect`s only the database it
            // created, again after its `DATABASE PROPERTIES` entry and before
            // any data, so a `\connect` naming the current segment's database,
            // with no new invocation's header pair staged (I9) and no block
            // read, is that reconnect and continues the segment. Every other
            // one opens a segment: two concatenated dumps of one database
            // stay two, as `target_settled` and `database_for_name` expect.
            SpanBody::Connect { database }
                if seen_connect
                    && current.name.as_deref() == Some(database.as_str())
                    && pending_headers == (None, None)
                    && !current.preamble_complete => {}
            SpanBody::Connect { database } => {
                let mut next = DatabaseMetadata::empty(Some(database.clone()));
                if pending_headers.0.is_some() || pending_headers.1.is_some() {
                    next.server_version = pending_headers.0.take();
                    next.pg_dump_version = pending_headers.1.take();
                } else if !seen_connect {
                    next.server_version = current.server_version.take();
                    next.pg_dump_version = current.pg_dump_version.take();
                }
                let finished = std::mem::replace(&mut current, next);
                if seen_connect {
                    databases.push(finalize(finished));
                }
                seen_connect = true;
            }
            // I9: one `pg_dump` invocation writes one header pair, ahead of
            // its own `\connect` under `--create`, and after the one
            // `pg_dumpall` writes for `template1` and `postgres`. So a pair
            // arriving into a segment that already holds one opens the next
            // invocation's output, and waits for the `\connect` naming its
            // database.
            SpanBody::VersionHeader { server_version, pg_dump_version } => {
                let held = current.server_version.is_some() || current.pg_dump_version.is_some();
                let (server, pg_dump) = if held {
                    (&mut pending_headers.0, &mut pending_headers.1)
                } else {
                    (&mut current.server_version, &mut current.pg_dump_version)
                };
                if server_version.is_some() {
                    *server = server_version.clone();
                }
                if pg_dump_version.is_some() {
                    *pg_dump = pg_dump_version.clone();
                }
            }
            SpanBody::Data(_) => {
                current.preamble_complete = true;
            }
            // I1 guarantees none of these eight can genuinely follow a `Data`
            // span for the current database before its next `Connect` — the
            // guard is defensive, matching what a line-triggered scan would
            // have done, rather than assuming the invariant holds.
            SpanBody::Table { name, definition } if !current.preamble_complete => {
                current.tables.insert(name.clone(), definition.clone());
            }
            SpanBody::AlterTableReference { table, reference } if !current.preamble_complete => {
                if let Some(definition) = current.tables.get_mut(table) {
                    definition.add(reference);
                }
            }
            SpanBody::TypeDef { name, kind } if !current.preamble_complete => {
                record_type(&mut current.types, name, kind);
            }
            SpanBody::Extension { name, schema } if !current.preamble_complete => {
                current.extensions.push(Extension { name: name.clone(), schema: schema.clone() });
            }
            SpanBody::AlterTypeAddValue { type_name, label } if !current.preamble_complete => {
                fold_alter_type_add_value(&mut current.types, type_name, label);
            }
            SpanBody::AlterTypeDropAttribute { type_name, attribute }
                if !current.preamble_complete =>
            {
                fold_alter_type_drop_attribute(&mut current.types, type_name, attribute);
            }
            SpanBody::EnumLabelsUnread { type_name } if !current.preamble_complete => {
                fold_enum_labels_unread(&mut current.types, type_name.as_deref());
            }
            SpanBody::Collation { collation } if !current.preamble_complete => {
                current.collations.push(collation.clone());
            }
            SpanBody::Table { .. }
            | SpanBody::TypeDef { .. }
            | SpanBody::Extension { .. }
            | SpanBody::AlterTypeAddValue { .. }
            | SpanBody::AlterTypeDropAttribute { .. }
            | SpanBody::EnumLabelsUnread { .. }
            | SpanBody::AlterTableReference { .. }
            | SpanBody::Collation { .. }
            | SpanBody::Framing
            | SpanBody::Unparsed
            | SpanBody::Unscanned => {}
        }
    }
    databases.push(finalize(current));
    DumpMetadata { databases }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::SpanBody;

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
            }
        );
        let (_, table) = parse_table_def(&["CREATE TABLE public.t OF PUBLIC.\"Person\";"]);
        assert_eq!(table.of_type.as_deref(), Some("public.\"Person\""));
    }

    /// The two references `--binary-upgrade` writes after a full column list,
    /// the `SET NOT NULL` a dump before v18 writes for a column its list does
    /// not print (I76), and nothing else an `ALTER TABLE` says: the removals,
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

    /// A reference reaching nothing — a parent or type the preamble does not
    /// hold, a composite whose list did not parse, a cycle only a
    /// hand-written file can hold — answers `None` rather than guessing or
    /// looping.
    #[test]
    fn a_reference_reaching_nothing_declares_nothing() {
        let unparsed = TypeDef {
            name: "public.opaque".to_string(),
            kind: TypeKind::Composite { fields: None },
        };
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
            TypeKind::Enum {
                labels: vec!["sad".to_string(), "has'quote".to_string()],
                exact: true
            }
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
        assert_eq!(
            parse_alter_type_drop_attribute_body("public.t DROP ATTRIBUTE b CASCADE;"),
            None
        );
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
                },
            }
        );
    }

    #[test]
    fn parses_a_composite_type() {
        let def =
            parse_type(&["CREATE TYPE public.point2d AS (", "\tx integer,", "\ty text", ");"]);
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
        assert_eq!(def.kind, TypeKind::Base);
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
        assert_eq!(
            ext,
            Extension { name: "pgcrypto".to_string(), schema: Some("public".to_string()) }
        );
    }

    #[test]
    fn extension_without_a_schema_clause() {
        let ext = parse_extension(&["CREATE EXTENSION IF NOT EXISTS pgcrypto;"]);
        assert_eq!(ext, Extension { name: "pgcrypto".to_string(), schema: None });
    }

    /// A span with placeholder offsets — `dump_metadata_from_spans` never
    /// reads `start`/`end`/`database`, only `body`.
    fn span(body: SpanBody) -> crate::map::Span {
        crate::map::Span {
            start: 0,
            end: 0,
            database: None,
            text: None,
            toc: None,
            toc_owned: false,
            body,
        }
    }

    fn dummy_data_span() -> crate::map::Span {
        span(SpanBody::Data(crate::map::DataBlock::Copy(crate::index::CopyBlock {
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
            .map(|db| {
                (db.name.as_deref(), db.server_version.as_deref(), db.pg_dump_version.as_deref())
            })
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
            classify_statement("CREATE TABLESPACE fast OWNER postgres LOCATION '/srv/fast';")
                .is_none()
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
            !parse("CREATE COLLATION public.nd (PROVIDER = icu, DETERMINISTIC = FALSE);")
                .deterministic
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

    /// [`extract_statement_cross_refs`]'s five recognized shapes — real
    /// lines from `fixtures/16/objects/default.sql`, including
    /// `objects.no_public_execute()`'s `REVOKE` and
    /// `objects.tablespaced_table`'s `SET default_tablespace = fixture_ts;`.
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

    /// A type name is kept in one spelling however the DDL wrote it: each part
    /// read as the server reads it — an unquoted one folded, a quoted one
    /// verbatim — and quoted back only where it is not a plain lower-case
    /// identifier (I29). Text that is not one name has no spelling.
    #[test]
    fn a_type_name_has_one_canonical_spelling() {
        for (reference, canonical) in [
            ("public.mood", "public.mood"),
            ("PUBLIC.Mood", "public.mood"),
            (r#""public"."mood""#, "public.mood"),
            (r#"public."Mood""#, r#"public."Mood""#),
            (r#"s."x ARRAY""#, r#"s."x ARRAY""#),
            (r#"s."a""b""#, r#"s."a""b""#),
            (r#"s."a.b""#, r#"s."a.b""#),
            ("s.x$1", r#"s."x$1""#),
            ("s . t ", "s.t"),
            ("mood", "mood"),
        ] {
            assert_eq!(canonical_type_name(reference).as_deref(), Some(canonical), "{reference}");
            assert_eq!(canonical_type_name(canonical).as_deref(), Some(canonical), "{canonical}");
        }
        for reference in ["integer[]", "double precision", r#"s."a"[]"#, "numeric(10,2)", "s."] {
            assert_eq!(canonical_type_name(reference), None, "{reference}");
        }
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
}
