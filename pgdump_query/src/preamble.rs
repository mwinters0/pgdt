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
//! feeds it, from statement shapes this module otherwise leaves `Unparsed`
//! (`OWNER TO`, `GRANT`/`REVOKE`/`ALTER DEFAULT PRIVILEGES FOR ROLE`, `SET
//! default_tablespace`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::copy::Cursor;
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
    /// caller walking `DumpIndex::metadata` checks this per database rather
    /// than assuming the whole list is complete.
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
    /// [`crate::copy::CopyHeader::qualified_name`] is) -> its columns in DDL
    /// order.
    ///
    /// Deficiency register: `deficiency: KD14` — this is the structure a scan
    /// holds per table, and peak resident set grows with the table count while
    /// staying flat in dump bytes, over a third of it live structure the
    /// preamble alone pays (`measurements.md`, `peak-rss` and `rss-attribution`).
    /// **(c) unowned**; promoted by a dump with tens of thousands of tables,
    /// nothing in hand being one. It is also why every "resident set" claim
    /// about this system is the *one-block* reading and says so.
    pub tables: BTreeMap<String, Vec<ColumnDef>>,
}

impl DatabaseMetadata {
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnDef {
    pub name: String,
    pub declared_type: String,
    pub collation: Option<String>,
}

impl ColumnDef {
    /// A column carrying no `COLLATE` clause — the ordinary case, and the one
    /// a test or an embedder building metadata by hand wants.
    pub fn new(name: impl Into<String>, declared_type: impl Into<String>) -> Self {
        Self { name: name.into(), declared_type: declared_type.into(), collation: None }
    }
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
    Enum { labels: Vec<String> },
    /// Reduces to a base type, resolved transitively by [`crate::pgtype`] (a
    /// domain over a domain is legal). `NOT NULL` is discarded — every Arrow
    /// field is nullable regardless.
    ///
    /// `collation` is the domain's own `COLLATE` clause, verbatim, which
    /// `pg_dump` writes only where the domain's collation differs from its
    /// base type's (I37). It is the domain's *type default*, so a column
    /// declared with this domain and carrying no clause of its own inherits
    /// it — which is why it is kept rather than discarded like `NOT NULL`.
    Domain { base_type: String, collation: Option<String> },
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
    /// A domain carrying no `COLLATE` clause of its own — the ordinary shape,
    /// and the one a test or an embedder building a type list by hand wants.
    pub fn domain(base_type: impl Into<String>) -> Self {
        Self::Domain { base_type: base_type.into(), collation: None }
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

/// Case-insensitively strip a leading keyword, returning the (whitespace
/// stripped) remainder. `pub(crate)` for [`crate::map::classify`]'s own
/// `ALTER TYPE` check.
pub(crate) fn strip_kw<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    if s.len() < kw.len() || !s.as_bytes()[..kw.len()].eq_ignore_ascii_case(kw.as_bytes()) {
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

/// Split `s` on top-level commas — not ones nested inside parens, a
/// single-quoted string or a double-quoted identifier — trimming and dropping
/// empty fragments.
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
            b'(' => depth += 1,
            b')' => depth -= 1,
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
/// starting at `s`'s first byte. `s` must have nothing but leading/trailing
/// whitespace around the literal.
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
    Some(ColumnDef { name, declared_type, collation: extract_collation(rest) })
}

/// `CREATE TABLE <name> (<col> <type>, ...);` (or, for a typed/partition
/// table with no column list at all — I5 — just `<name>`).
fn parse_create_table(rest: &str) -> Option<(String, Vec<ColumnDef>)> {
    let (name, consumed) = parse_qualified_name(rest)?;
    let after = rest[consumed..].trim_start();
    if !after.starts_with('(') {
        return Some((name, Vec::new()));
    }
    let close = matching_paren(after.as_bytes(), 0)?;
    let inner = &after[1..close];
    let columns =
        split_top_level_commas(inner).into_iter().filter_map(parse_column_fragment).collect();
    Some((name, columns))
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
    Some(TypeDef { name, kind: TypeKind::Domain { base_type, collation } })
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
        let labels = split_top_level_commas(&body[open + 1..close])
            .into_iter()
            .filter_map(parse_string_literal)
            .collect();
        return Some(TypeDef { name, kind: TypeKind::Enum { labels } });
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
pub(crate) fn parse_alter_type_add_value_body(rest: &str) -> Option<(String, String)> {
    let (name, consumed) = parse_type_name(rest)?;
    let after = &rest[consumed..];
    let idx = find_ci(after, "ADD VALUE")?;
    let label = parse_string_literal(after[idx + "ADD VALUE".len()..].trim_start())?;
    Some((name, label))
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
/// `types`, if any — a no-op if the name isn't found or isn't an `Enum`.
/// Used by [`dump_metadata_from_spans`], which encounters the label as its
/// own [`crate::map::SpanBody::AlterTypeAddValue`] span, separate from the
/// [`crate::map::SpanBody::TypeDef`] span it targets.
fn fold_alter_type_add_value(types: &mut [TypeDef], type_name: &str, label: &str) {
    if let Some(TypeDef { kind: TypeKind::Enum { labels }, .. }) =
        types.iter_mut().find(|t| t.name == type_name)
    {
        labels.push(label.to_string());
    }
}

/// Case-insensitively find `marker` in `haystack` and parse the identifier
/// (bare or double-quoted, `''`/`""`-doubled — [`Cursor::parse_ident`]'s two
/// shapes, matching `fmtId()`'s two output forms) immediately following it.
fn ident_after(haystack: &str, marker: &str) -> Option<String> {
    let idx = find_ci(haystack, marker)?;
    let rest = haystack[idx + marker.len()..].trim_start();
    Cursor::new(rest.as_bytes()).parse_ident()
}

/// Filters out the pseudo-role `_printTocEntry`/`buildACLCommands` write
/// literally as `PUBLIC` whenever a grant/revoke's grantee list is empty:
/// `PUBLIC` is never reported as a role. Case-insensitive because
/// [`ident_after`]'s [`Cursor::parse_ident`] lowercases every *unquoted*
/// identifier, so the keyword arrives here as `public` — the same fold an
/// unquoted role genuinely named `public` goes through, which is an
/// irreducible ambiguity in the dump text: `GRANT ... TO public;` unquoted
/// means the pseudo-role to PostgreSQL's own parser too.
pub(crate) fn insert_role(roles: &mut BTreeSet<String>, role: String) {
    if !role.eq_ignore_ascii_case("PUBLIC") {
        roles.insert(role);
    }
}

/// Filters out `pg_default`, the reserved, uncreatable name for a database's
/// implicit default tablespace — never reported as one, the same way
/// `PUBLIC` is filtered from roles. Case-insensitive for the same
/// reason [`insert_role`]'s `PUBLIC` check is.
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
/// `ALTER TYPE ADD VALUE` is not among them: it mutates an already-declared
/// object rather than introducing one, so it has its own two entry points —
/// [`parse_alter_type_add_value_body`] for [`crate::map::classify`] and
/// [`fold_alter_type_add_value`] for [`dump_metadata_from_spans`].
#[derive(Debug)]
pub(crate) enum StatementShape {
    Table { name: String, columns: Vec<ColumnDef> },
    Type(TypeDef),
    Extension(Extension),
    Collation(CollationDef),
}

/// Classify a complete statement (see [`statement_complete`]) as one of
/// this module's recognized `CREATE` shapes, or `None` for anything else —
/// including `ALTER TYPE ADD VALUE`, which needs an already-open `TypeDef`
/// to fold into (see [`StatementShape`]'s docs) rather than being
/// classifiable from its own text alone.
pub(crate) fn classify_statement(stmt: &str) -> Option<StatementShape> {
    let trimmed = stmt.trim_start();
    if let Some(rest) = strip_kw(trimmed, "CREATE TABLE") {
        return parse_create_table(rest)
            .map(|(name, columns)| StatementShape::Table { name, columns });
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
pub(crate) fn parse_connect(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("\\connect ")?;
    Cursor::new(rest.trim().as_bytes()).parse_ident()
}

/// An incremental, byte-level scan of SQL statement text: how deep the
/// parens are, whether the bytes so far end inside a single-quoted string, a
/// double-quoted identifier or a `--` line comment, and what the last
/// non-whitespace byte was.
///
/// Byte-level rather than `char`-level: every byte it acts on — `'`, `"`,
/// `-`, `(`, `)`, `\n` — is ASCII, and an ASCII byte never occurs inside a
/// multi-byte UTF-8 sequence, so the scan gives the same answer over raw bytes
/// as over a validated `str` and needs no validation pass in front of it.
/// That is what lets [`crate::map`]'s `INSERT` runs be classified without a
/// `String` per line (`docs/design/decisions.md`, "D33").
///
/// **A caller may split the statement's bytes anywhere.** A `''`, `""` or
/// `--` pair straddling two [`feed`](Self::feed) calls is carried across in
/// `pending`, so a reader walking a file in chunks gets the same answer as one
/// holding the whole statement in a buffer.
///
/// Double quotes and comments are tracked because without it an apostrophe
/// inside either (`public."it's"`, `-- it's here`) would open a string that
/// never closes and swallow every following line. `E'…'` escapes are not a
/// concern — `pg_dump` sets `standard_conforming_strings = on`, so `''` is the
/// only in-string escape.
#[derive(Debug, Clone)]
pub(crate) struct StatementScan {
    depth: i32,
    in_string: bool,
    in_dquote: bool,
    in_comment: bool,
    /// A quote or `-` that ended the previous [`feed`](Self::feed) and might
    /// yet pair with the first byte of the next one.
    pending: Pending,
    /// The last byte fed that is not [`is_sql_space`], or `0` if none is —
    /// the incremental form of `buf.trim_end()`'s last character.
    last_significant: u8,
    /// Bytes fed since the last [`reset`](Self::reset), so that
    /// [`is_empty`](Self::is_empty) answers what `buf.is_empty()` answered.
    len: usize,
}

/// A two-byte token whose first byte ended a [`StatementScan::feed`] run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    None,
    /// A `'` seen while inside a string: it either closes the string or is
    /// the first half of a `''`.
    StringQuote,
    /// The same, for a `"` inside a quoted identifier.
    IdentQuote,
    /// A `-` seen outside everything: it might begin a `--` comment.
    Dash,
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
    pub(crate) const fn new() -> Self {
        Self {
            depth: 0,
            in_string: false,
            in_dquote: false,
            in_comment: false,
            pending: Pending::None,
            last_significant: 0,
            len: 0,
        }
    }

    /// Start a fresh statement.
    pub(crate) fn reset(&mut self) {
        *self = Self::new();
    }

    /// Nothing has been fed since the last [`reset`](Self::reset) — the
    /// incremental equivalent of `buf.is_empty()`, and [`crate::map`]'s
    /// signal that the next line either opens a statement or ends the run.
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Feed one more physical line, joined to what came before with `\n`
    /// exactly as [`push_stmt_line`] joins them — only between lines, so a
    /// trailing `-- comment` stays open at the end of a buffer, no `\n`
    /// having followed it.
    pub(crate) fn feed_line(&mut self, line: &[u8]) {
        if self.len != 0 {
            self.feed(b"\n");
        }
        self.feed(line);
    }

    /// Feed the next contiguous run of statement bytes.
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.len += bytes.len();
        if let Some(&b) = bytes.iter().rev().find(|&&b| !is_sql_space(b)) {
            self.last_significant = b;
        }

        let mut i = 0;
        match self.pending {
            Pending::None => {}
            Pending::StringQuote => {
                self.pending = Pending::None;
                if bytes[0] == b'\'' {
                    i = 1;
                } else {
                    self.in_string = false;
                }
            }
            Pending::IdentQuote => {
                self.pending = Pending::None;
                if bytes[0] == b'"' {
                    i = 1;
                } else {
                    self.in_dquote = false;
                }
            }
            Pending::Dash => {
                self.pending = Pending::None;
                if bytes[0] == b'-' {
                    self.in_comment = true;
                    i = 1;
                }
            }
        }

        while i < bytes.len() {
            // Inside a comment, a string or a quoted identifier there is
            // exactly one byte that matters, so the bulk of a statement's
            // bytes — which is its string values — is crossed by `memchr`
            // rather than one byte at a time.
            if self.in_comment {
                match memchr::memchr(b'\n', &bytes[i..]) {
                    Some(k) => {
                        self.in_comment = false;
                        i += k + 1;
                    }
                    None => break,
                }
                continue;
            }
            if self.in_string {
                match memchr::memchr(b'\'', &bytes[i..]) {
                    Some(k) => {
                        let p = i + k;
                        match bytes.get(p + 1) {
                            Some(b'\'') => i = p + 2,
                            Some(_) => {
                                self.in_string = false;
                                i = p + 1;
                            }
                            None => {
                                self.pending = Pending::StringQuote;
                                i = p + 1;
                            }
                        }
                    }
                    None => break,
                }
                continue;
            }
            if self.in_dquote {
                match memchr::memchr(b'"', &bytes[i..]) {
                    Some(k) => {
                        let p = i + k;
                        match bytes.get(p + 1) {
                            Some(b'"') => i = p + 2,
                            Some(_) => {
                                self.in_dquote = false;
                                i = p + 1;
                            }
                            None => {
                                self.pending = Pending::IdentQuote;
                                i = p + 1;
                            }
                        }
                    }
                    None => break,
                }
                continue;
            }
            // Outside all three, five bytes matter and `memchr` takes at
            // most three needles — so the run up to the next byte that could
            // change *mode* is found with one SIMD pass, and the parens
            // inside it, which only move a counter, are counted with a
            // second. Both beat walking the run a byte at a time, and
            // outside a string is where the bytes of an `INSERT` statement
            // that are not values live.
            //
            // Deficiency register: `deficiency: KD9` — an `INSERT` run costs
            // several times a `COPY` scan's per-byte CPU warm, and the device
            // is what decides whether a reader meets it
            // (`measurements.md`, `scan-throughput-warm` and
            // `scan-throughput-nvme`). Two cuts against the remainder are
            // known: no `INSERT` statement's end depends on `depth`, so a
            // run-only scan would skip this pass; and `insert_run_line`
            // (`map.rs`) feeds the `INSERT INTO <table>` prefix it has just
            // matched, whose scan state is provably unchanged, so those bytes
            // are crossed twice. **(b) owned by P8**, whose row reader is the
            // caller that can say the count is dead weight; it is not a
            // licence to drop `depth`, which `statement_complete` reads.
            let rest = &bytes[i..];
            let stop = memchr::memchr3(b'\'', b'"', b'-', rest).unwrap_or(rest.len());
            let plain = &rest[..stop];
            for k in memchr::memchr2_iter(b'(', b')', plain) {
                if plain[k] == b'(' {
                    self.depth += 1;
                } else {
                    self.depth -= 1;
                }
            }
            i += stop;
            if stop == rest.len() {
                break;
            }
            match bytes[i] {
                b'\'' => {
                    self.in_string = true;
                    i += 1;
                }
                b'"' => {
                    self.in_dquote = true;
                    i += 1;
                }
                // `-`, by elimination.
                _ => match bytes.get(i + 1) {
                    Some(b'-') => {
                        self.in_comment = true;
                        i += 2;
                    }
                    Some(_) => i += 1,
                    None => {
                        self.pending = Pending::Dash;
                        i += 1;
                    }
                },
            }
        }
    }

    /// Whether what has been fed so far is a complete SQL statement: parens
    /// balanced, not mid string literal or double-quoted identifier or `--`
    /// line comment, and ending in `;`.
    ///
    /// A [`Pending`] quote is resolved as *closing*, which is what the
    /// end of the buffer means for it — the same answer a scan that could
    /// peek past the end would give.
    pub(crate) fn complete(&self) -> bool {
        !self.in_string_settled()
            && !self.in_dquote_settled()
            && !self.in_comment
            && self.depth == 0
            && self.last_significant == b';'
    }

    /// Whether the bytes so far end inside an open single-quoted string or
    /// double-quoted identifier — the one case where a line that *looks* like
    /// a fresh boundary (starts with `--`, in [`crate::map`]'s case) is really
    /// string content spanning physical lines. `pg_dump` never emits a `--`
    /// comment inside a non-dollar-quoted statement's own parens, so paren
    /// depth does not gate this the same way.
    pub(crate) fn in_quote(&self) -> bool {
        self.in_string_settled() || self.in_dquote_settled()
    }

    fn in_string_settled(&self) -> bool {
        self.in_string && self.pending != Pending::StringQuote
    }

    fn in_dquote_settled(&self) -> bool {
        self.in_dquote && self.pending != Pending::IdentQuote
    }
}

/// Whether `buf` (everything accumulated for a statement so far) is a
/// complete SQL statement — [`StatementScan::complete`] over a buffer a
/// caller holds whole rather than feeding incrementally. One implementation
/// for both, [`crate::map`] deciding the same question from a `String` for a
/// DDL statement and from raw bytes for an `INSERT` run.
pub(crate) fn statement_complete(buf: &str) -> bool {
    let mut scan = StatementScan::new();
    scan.feed(buf.as_bytes());
    scan.complete()
}

/// [`StatementScan::in_quote`] over a whole buffer — see that method.
pub(crate) fn in_open_quote(buf: &str) -> bool {
    let mut scan = StatementScan::new();
    scan.feed(buf.as_bytes());
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
/// enum-label folding on [`crate::map::SpanBody::AlterTypeAddValue`].
///
/// `spans` must come from a scan that stops at one of two safe boundaries:
/// end of file, or (per I1) the start of the current database's first `COPY`
/// block — [`crate::map::build_map`] and `crate::index::scan_preamble`'s own
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
            SpanBody::Connect { database } => {
                if seen_connect {
                    let mut next = DatabaseMetadata::empty(Some(database.clone()));
                    next.server_version = pending_headers.0.take();
                    next.pg_dump_version = pending_headers.1.take();
                    let finished = std::mem::replace(&mut current, next);
                    databases.push(finalize(finished));
                } else {
                    let mut next = DatabaseMetadata::empty(Some(database.clone()));
                    next.server_version = current.server_version.take();
                    next.pg_dump_version = current.pg_dump_version.take();
                    current = next;
                }
                seen_connect = true;
            }
            SpanBody::VersionHeader { server_version, pg_dump_version } => {
                if current.preamble_complete {
                    if server_version.is_some() {
                        pending_headers.0 = server_version.clone();
                    }
                    if pg_dump_version.is_some() {
                        pending_headers.1 = pg_dump_version.clone();
                    }
                } else {
                    if server_version.is_some() {
                        current.server_version = server_version.clone();
                    }
                    if pg_dump_version.is_some() {
                        current.pg_dump_version = pg_dump_version.clone();
                    }
                }
            }
            SpanBody::Data(_) => {
                current.preamble_complete = true;
            }
            // I1 guarantees none of these five can genuinely follow a `Data`
            // span for the current database before its next `Connect` — the
            // guard is defensive, matching what a line-triggered scan would
            // have done, rather than assuming the invariant holds.
            SpanBody::Table { name, columns } if !current.preamble_complete => {
                current.tables.insert(name.clone(), columns.clone());
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
            SpanBody::Collation { collation } if !current.preamble_complete => {
                current.collations.push(collation.clone());
            }
            SpanBody::Table { .. }
            | SpanBody::TypeDef { .. }
            | SpanBody::Extension { .. }
            | SpanBody::AlterTypeAddValue { .. }
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
        match parse(lines) {
            StatementShape::Table { name, columns } => (name, columns),
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
                ColumnDef::new("id", "integer"),
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
        assert_eq!(cols[0], ColumnDef::new("a", "character varying(16)"));
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
    /// thing a column-level clause exists to override — so it is kept rather
    /// than discarded the way `NOT NULL` is.
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
            TypeKind::Enum {
                labels: vec![
                    "sad".to_string(),
                    "has space".to_string(),
                    "has,comma".to_string(),
                    "has'quote".to_string(),
                ]
            }
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
                kind: TypeKind::Enum { labels: Vec::new() },
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
            TypeKind::Enum { labels: vec!["sad".to_string(), "has'quote".to_string()] }
        );
    }

    #[test]
    fn parses_a_domain_over_a_domain() {
        let def = parse_type(&["CREATE DOMAIN public.derived AS public.base_domain NOT NULL;"]);
        assert_eq!(
            def,
            TypeDef {
                name: "public.derived".to_string(),
                kind: TypeKind::domain("public.base_domain"),
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
            array_shapes: Some(Vec::new()),
        })))
    }

    #[test]
    fn plain_dump_has_no_connect_and_names_no_database() {
        let meta = dump_metadata_from_spans(&[span(SpanBody::Table {
            name: "public.t".to_string(),
            columns: vec![ColumnDef::new("id", "integer")],
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
                columns: vec![ColumnDef::new("id", "integer")],
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
            span(SpanBody::Table { name: "public.a".to_string(), columns: Vec::new() }),
            dummy_data_span(),
            // Per I1, nothing more should be captured for this database now.
            span(SpanBody::Table { name: "public.ignored".to_string(), columns: Vec::new() }),
            span(SpanBody::Connect { database: "two".to_string() }),
            span(SpanBody::Table { name: "public.b".to_string(), columns: Vec::new() }),
        ]);

        assert_eq!(meta.databases.len(), 2);
        assert_eq!(meta.databases[0].name.as_deref(), Some("one"));
        assert!(meta.databases[0].tables.contains_key("public.a"));
        assert!(!meta.databases[0].tables.contains_key("public.ignored"));
        assert_eq!(meta.databases[1].name.as_deref(), Some("two"));
        assert!(meta.databases[1].tables.contains_key("public.b"));
    }

    /// I9: every `\connect`-segment in a real `pg_dumpall`/concatenated dump
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
            span(SpanBody::Table { name: "public.a".to_string(), columns: Vec::new() }),
            dummy_data_span(),
            span(SpanBody::VersionHeader {
                server_version: Some("16.15".to_string()),
                pg_dump_version: Some("16.15".to_string()),
            }),
            span(SpanBody::Connect { database: "two".to_string() }),
            span(SpanBody::Table { name: "public.b".to_string(), columns: Vec::new() }),
        ]);

        assert_eq!(meta.databases.len(), 2);
        assert_eq!(meta.databases[0].server_version.as_deref(), Some("16.14"));
        assert_eq!(meta.databases[0].pg_dump_version.as_deref(), Some("16.14"));
        assert_eq!(meta.databases[1].server_version.as_deref(), Some("16.15"));
        assert_eq!(meta.databases[1].pg_dump_version.as_deref(), Some("16.15"));
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

    /// Guards against an apostrophe in comment prose opening a string that
    /// never closes.
    #[test]
    fn statement_complete_is_not_confused_by_an_apostrophe_in_a_line_comment() {
        assert!(statement_complete("CREATE TABLE t (id integer);\n-- it's here\nSELECT 1;"));
        // Not complete until a real `;` follows the comment.
        assert!(!statement_complete("SELECT 1\n-- it's here"));
    }

    /// Guards against an apostrophe inside a double-quoted identifier
    /// opening a string that never closes.
    #[test]
    fn statement_complete_is_not_confused_by_an_apostrophe_in_a_double_quoted_identifier() {
        assert!(statement_complete(r#"CREATE TABLE public."it's" (id integer);"#));
    }

    #[test]
    fn statement_complete_handles_doubled_double_quotes() {
        assert!(statement_complete(r#"CREATE TABLE public."a""b" (id integer);"#));
        assert!(!statement_complete(r#"CREATE TABLE public."a""b (id integer);"#));
    }

    #[test]
    fn statement_complete_a_comment_run_to_end_of_buffer_is_never_complete() {
        // No trailing `\n` to close the comment — matches a statement scan
        // whose last fed line is itself a bare comment.
        assert!(!statement_complete("SELECT 1;\n-- trailing comment, no newline after it"));
    }

    /// A [`StatementScan`] fed in pieces answers what one fed the whole
    /// buffer answers, **at every split point** — which is the property that
    /// makes it usable by a reader walking a file in chunks rather than a
    /// line at a time, and the one a naive byte loop gets wrong: `''`, `""`
    /// and `--` are two-byte tokens, so a split between their halves is
    /// exactly where a scan that cannot look back mis-reads.
    #[test]
    fn a_statement_scan_gives_the_same_answer_at_every_chunk_boundary() {
        let cases = [
            "INSERT INTO t VALUES ('it''s', 1);",
            "INSERT INTO t VALUES ('a''''b');",
            "CREATE TABLE public.\"it''s\" (a integer);",
            "CREATE TABLE public.\"q\"\"q\" (a integer);",
            "SELECT 1; -- trailing",
            "SELECT 1; -- trailing\nSELECT 2;",
            "SELECT 1 - -2;",
            "INSERT INTO t VALUES ('-- not a comment');",
            "INSERT INTO t VALUES ('a\nb');",
            "SELECT ((1));",
            "SELECT (1;",
        ];
        for case in cases {
            let mut whole = StatementScan::new();
            whole.feed(case.as_bytes());
            for split in 0..=case.len() {
                let mut piecewise = StatementScan::new();
                piecewise.feed(&case.as_bytes()[..split]);
                piecewise.feed(&case.as_bytes()[split..]);
                assert_eq!(
                    piecewise.complete(),
                    whole.complete(),
                    "complete() disagrees for {case:?} split at {split}"
                );
                assert_eq!(
                    piecewise.in_quote(),
                    whole.in_quote(),
                    "in_quote() disagrees for {case:?} split at {split}"
                );
            }
        }
    }

    /// [`StatementScan::feed_line`] joins with `\n` the way
    /// [`push_stmt_line`] does — *between* lines, never before the first —
    /// so a buffer whose last line is a bare comment stays open, which is
    /// what [`crate::map`]'s dangling-close arm relies on.
    #[test]
    fn feeding_lines_matches_joining_them_with_newlines() {
        let cases: [&[&str]; 4] = [
            &["INSERT INTO t VALUES (1);"],
            &["INSERT INTO t VALUES (", "  'a''b'", ");"],
            &["SELECT 1;", "-- trailing comment"],
            &["INSERT INTO t VALUES ('a", "-- still string content", "');"],
        ];
        for lines in cases {
            let mut scan = StatementScan::new();
            for line in lines {
                scan.feed_line(line.as_bytes());
            }
            let joined = lines.join("\n");
            assert_eq!(scan.complete(), statement_complete(&joined), "for {joined:?}");
            assert_eq!(scan.in_quote(), in_open_quote(&joined), "for {joined:?}");
        }
    }

    /// `is_empty` is what [`crate::map`]'s `INSERT` run reads as "a fresh
    /// statement starts here", and it must survive a whitespace-only line
    /// exactly as `buf.is_empty()` did: `push_stmt_line` appends such a line,
    /// so the buffer stops being empty.
    #[test]
    fn a_statement_scan_is_empty_only_before_anything_is_fed() {
        let mut scan = StatementScan::new();
        assert!(scan.is_empty());
        scan.feed_line(b"");
        assert!(scan.is_empty(), "an empty line appends nothing, as `push_stmt_line` does not");
        scan.feed_line(b"   ");
        assert!(!scan.is_empty());
        scan.reset();
        assert!(scan.is_empty());
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
