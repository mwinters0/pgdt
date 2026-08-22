//! Dump-level preamble parsing: `CREATE TABLE`/`TYPE`/`DOMAIN`/`EXTENSION`
//! DDL and the two version header lines, recovered from the pre-data region
//! of a `pg_dump` plain-format file (`docs/design/roadmap-phase2-typed-columns.md`,
//! "The preamble pass").
//!
//! [`PreambleBuilder`] is fed the [`crate::scan::Line`] events
//! [`crate::index::build_index`] already receives from [`crate::scan::scan`]
//! while it walks the file for `COPY` block structure — no second pass over
//! the file is needed, since I1 (`docs/design/postgres-invariants.md`) means
//! nothing this module cares about can appear after a database's first
//! `COPY` block.
//!
//! **Store what the dump said, never what we concluded.** Declared types are
//! kept as strings exactly as written (`character varying(16)`, not a parsed
//! `(base, typmod)` pair) — see "What the cache stores" in the phase doc.
//! Resolving those strings into Arrow types is Phase 2.3's job
//! (`crate::pgtype`, not yet built).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::copy::Cursor;

/// Everything the preamble pass recovered, per database. See "Multi-database
/// dumps" in `docs/design/roadmap-phase2-typed-columns.md`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpMetadata {
    pub databases: Vec<DatabaseMetadata>,
}

/// One database's preamble, in DDL order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseMetadata {
    /// `None` for a plain `pg_dump` output, which has no `\connect` and so
    /// names no database. Never guessed — see [`PreambleBuilder`]'s module
    /// docs for how a `--create` dump's pre-`\connect` segment (which names
    /// no real database either, but for a different reason) is told apart
    /// from this case.
    pub name: Option<String>,
    /// Whether this database's preamble was read to completion. Always
    /// `true` for an entry produced by a full scan (`build_index`) — it
    /// exists to compose with `DumpIndex::scanned_through` once an
    /// *incremental* query can also produce metadata (Phase 2.3).
    pub preamble_complete: bool,
    pub server_version: Option<String>,
    pub pg_dump_version: Option<String>,
    pub extensions: Vec<Extension>,
    pub types: Vec<TypeDef>,
    /// Qualified table name (`schema.table`, folded the same way
    /// [`crate::copy::CopyHeader::qualified_name`] is) -> `(column, declared
    /// type)` in DDL order.
    pub tables: BTreeMap<String, Vec<(String, String)>>,
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
            tables: BTreeMap::new(),
        }
    }
}

/// A `CREATE EXTENSION` line. Extension *versions* are never in a regular
/// dump — `dumpExtension()` deliberately omits them
/// (`docs/design/postgres-invariants.md`, evidenced in
/// `docs/status/history/2026-08-22.md`) — so there is no version field here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extension {
    pub name: String,
    pub schema: Option<String>,
}

/// A `CREATE TYPE` or `CREATE DOMAIN` definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeDef {
    /// Schema-qualified name, as it appears in a column's declared type.
    pub name: String,
    pub kind: TypeKind,
}

/// Which of `pg_dump`'s six type-emission shapes produced a [`TypeDef`] —
/// see "`CREATE TYPE`: six emitted forms" in
/// `docs/status/history/2026-08-22.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeKind {
    /// Fully determined by the DDL — labels in declaration order. A
    /// `--binary-upgrade` dump emits these via a separate `ALTER TYPE ADD
    /// VALUE` per label (I6); [`PreambleBuilder`] folds them back in here so
    /// both forms produce the same shape.
    Enum { labels: Vec<String> },
    /// Reduces to a base type, resolved transitively by Phase 2.3 (a domain
    /// over a domain is legal). `NOT NULL` is discarded — Phase 2 makes
    /// every field nullable regardless (see "Nullability" in the phase doc).
    Domain { base_type: String },
    /// Field name -> declared type, in declaration order. Decoding COPY
    /// TEXT's record literal is Phase 3's job; this is what that decoder
    /// will need.
    Composite { fields: Vec<(String, String)> },
    /// The subtype named in the `CREATE TYPE ... AS RANGE (...)` parameter
    /// list, if the grammar found one. No fixture or koji evidence exercises
    /// this shape (both use built-in range types), so this is best-effort
    /// from `pg_dump` source reading alone.
    Range { subtype: Option<String> },
    /// A C-level base type (`CREATE TYPE x (INPUT = ..., OUTPUT = ...)`) —
    /// information-free; the dump says how the *server* parses it.
    Base,
    /// `CREATE TYPE x;` with no body at all, ahead of the real definition
    /// (forward-declaration shell type) or genuinely never completed.
    Shell,
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
/// stripped) remainder.
fn strip_kw<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    if s.len() < kw.len() || !s.as_bytes()[..kw.len()].eq_ignore_ascii_case(kw.as_bytes()) {
        return None;
    }
    Some(s[kw.len()..].trim_start())
}

/// Parse a (possibly schema-qualified) identifier from the start of `s`,
/// same folding rules as [`crate::copy::parse_copy_header`]'s table names —
/// so the result matches [`crate::copy::CopyHeader::qualified_name`] exactly
/// for the same object. Returns the qualified name and how many bytes of `s`
/// it consumed.
fn parse_qualified_name(s: &str) -> Option<(String, usize)> {
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

/// Find the index of `bytes[open_idx..]`'s matching `)`, respecting
/// single-quoted strings (with `''` as an escaped quote) so a literal like
/// `'has,comma'` or, hypothetically, `')'` inside a string never confuses
/// the depth count.
fn matching_paren(bytes: &[u8], open_idx: usize) -> Option<usize> {
    debug_assert_eq!(bytes.get(open_idx), Some(&b'('));
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut i = open_idx;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if b == b'\'' {
                if bytes.get(i + 1) == Some(&b'\'') {
                    i += 2;
                    continue;
                }
                in_string = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' => in_string = true,
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

/// Split `s` on top-level commas — not ones nested inside parens or a
/// single-quoted string — trimming and dropping empty fragments.
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut parts = Vec::new();
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if b == b'\'' {
                if bytes.get(i + 1) == Some(&b'\'') {
                    i += 2;
                    continue;
                }
                in_string = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' => in_string = true,
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
/// and stopping at the first of these is enough to isolate it — no need to
/// parse constraint syntax at all.
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

/// Strip `/* ... */` block comments — not real SQL syntax `pg_dump` itself
/// would need to escape around, but the literal shape it writes a
/// `--binary-upgrade`-recreated dropped column's placeholder type in
/// (`INTEGER /* dummy */`, I5) — confirmed:
/// `fixtures/*/edge_cases/binary-upgrade.sql`. Assumes no nesting, which
/// matches every comment `pg_dump` itself emits.
fn strip_block_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start..].find("*/") {
            Some(end) => rest = &rest[start + end + 2..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn extract_type_words(rest: &str) -> String {
    let rest = strip_block_comments(rest);
    let mut words = Vec::new();
    for tok in rest.split_whitespace() {
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
fn parse_column_fragment(frag: &str) -> Option<(String, String)> {
    let frag = frag.trim();
    if frag.is_empty() {
        return None;
    }
    let mut cur = Cursor::new(frag.as_bytes());
    let name = cur.parse_ident()?;
    let rest = &frag[cur.pos()..];
    let type_str = extract_type_words(rest);
    if type_str.is_empty() {
        return None;
    }
    Some((name, type_str))
}

/// `CREATE TABLE <name> (<col> <type>, ...);` (or, for a typed/partition
/// table with no column list at all — I5 — just `<name>`).
fn parse_create_table(rest: &str) -> Option<(String, Vec<(String, String)>)> {
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

/// `CREATE DOMAIN <name> AS <basetype> [constraints...];`
fn parse_create_domain(rest: &str) -> Option<TypeDef> {
    let (name, consumed) = parse_qualified_name(rest)?;
    let after_as = strip_kw(rest[consumed..].trim_start(), "AS")?;
    let base_type = extract_type_words(after_as);
    if base_type.is_empty() {
        return None;
    }
    Some(TypeDef { name, kind: TypeKind::Domain { base_type } })
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

/// `CREATE TYPE <name>` in any of its six shapes (see [`TypeKind`]).
fn parse_create_type(rest: &str) -> Option<TypeDef> {
    let (name, consumed) = parse_qualified_name(rest)?;
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
        let subtype = split_top_level_commas(&body[open + 1..close]).into_iter().find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            k.trim().eq_ignore_ascii_case("subtype").then(|| v.trim().to_string())
        });
        return Some(TypeDef { name, kind: TypeKind::Range { subtype } });
    }
    if let Some(body) = strip_kw(after, "AS") {
        let body = body.trim_start();
        if body.starts_with('(') {
            let close = matching_paren(body.as_bytes(), 0)?;
            let fields = split_top_level_commas(&body[1..close])
                .into_iter()
                .filter_map(parse_column_fragment)
                .collect();
            return Some(TypeDef { name, kind: TypeKind::Composite { fields } });
        }
        return Some(TypeDef { name, kind: TypeKind::Base });
    }
    // `CREATE TYPE x (INPUT = ..., OUTPUT = ...);` — the C-level base type
    // shape, with no `AS` at all.
    Some(TypeDef { name, kind: TypeKind::Base })
}

/// `ALTER TYPE <name> ADD VALUE '<label>' [BEFORE|AFTER '<other>'];` — the
/// `--binary-upgrade` shape for enum labels (I6). `BEFORE`/`AFTER` is
/// ignored: a `--binary-upgrade` dump only ever emits these in declaration
/// order to recreate the type from scratch (confirmed:
/// `fixtures/*/types/binary-upgrade.sql`), so appending is equivalent to
/// respecting them.
fn apply_alter_type_add_value(types: &mut [TypeDef], rest: &str) {
    let Some((name, consumed)) = parse_qualified_name(rest) else { return };
    let after = &rest[consumed..];
    let Some(idx) = find_ci(after, "ADD VALUE") else { return };
    let Some(label) = parse_string_literal(after[idx + "ADD VALUE".len()..].trim_start()) else {
        return;
    };
    if let Some(TypeDef { kind: TypeKind::Enum { labels }, .. }) =
        types.iter_mut().find(|t| t.name == name)
    {
        labels.push(label);
    }
}

fn parse_connect(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("\\connect ")?;
    Cursor::new(rest.trim().as_bytes()).parse_ident()
}

/// Whether `buf` (everything accumulated for a statement so far) is a
/// complete SQL statement: parens balanced, not mid string literal, and
/// ending in `;`. `pg_dump`'s own DDL is simple enough (no dollar-quoted or
/// otherwise `;`-containing content inside these five statement shapes)
/// that quote/paren tracking alone is sufficient — it never needs to parse
/// an arbitrary expression.
fn statement_complete(buf: &str) -> bool {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut chars = buf.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                } else {
                    in_string = false;
                }
            }
            continue;
        }
        match c {
            '\'' => in_string = true,
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
    }
    !in_string && depth == 0 && buf.trim_end().ends_with(';')
}

struct PendingStmt {
    buf: String,
}

/// Incrementally builds a [`DumpMetadata`] from the [`crate::scan::Line`]
/// events a scan yields for every non-header, non-dollar-quoted line outside
/// a COPY block.
///
/// **A `--create` dump's pre-`\connect` segment is dropped, not kept as a
/// `name: None` database — but its version headers survive onto the
/// database that follows.** Before the first `\connect`, `pg_dump --create`
/// output is just `CREATE DATABASE ...;` — connected to whichever database
/// initiated the dump, not the one being described — so it never carries a
/// real table or type. `name: None` is reserved for the genuine case: a
/// plain (non-`--create`) dump has no `\connect` at all, and its single
/// implicit database *is* real. The two are told apart by whether a
/// `\connect` is ever seen: only the segment open when the **first** one
/// arrives is discarded; every later `\connect` pushes the segment it
/// closes. The version headers are carried over specially because real
/// `pg_dump --create` output (confirmed: the koji sample) prints them
/// exactly once, ahead of the first `\connect`, never repeating them per
/// database — so without this they would vanish for every `--create` dump.
pub(crate) struct PreambleBuilder {
    seen_connect: bool,
    current: DatabaseMetadata,
    databases: Vec<DatabaseMetadata>,
    pending: Option<PendingStmt>,
}

impl PreambleBuilder {
    pub(crate) fn new() -> Self {
        Self {
            seen_connect: false,
            current: DatabaseMetadata::empty(None),
            databases: Vec::new(),
            pending: None,
        }
    }

    /// A `COPY` block just started: per I1, nothing this module cares about
    /// can follow for the current database until (if ever) the next
    /// `\connect`.
    pub(crate) fn on_copy_start(&mut self) {
        self.pending = None;
        self.current.preamble_complete = true;
    }

    fn on_connect(&mut self, name: String) {
        self.pending = None;
        if self.seen_connect {
            let finished =
                std::mem::replace(&mut self.current, DatabaseMetadata::empty(Some(name)));
            self.databases.push(Self::finalize(finished));
        } else {
            // Discard the pre-connect segment itself (see the struct docs)
            // but carry its version headers forward: confirmed against the
            // real koji sample (`pg_dump --create`, 16.14) that `RestoreArchive()`
            // prints them exactly once, ahead of the first `\connect`, not
            // per database — so they'd otherwise vanish entirely for every
            // `--create` dump instead of merely being attributed to the
            // dump's first real database, which is where a caller actually
            // looks for them.
            let mut next = DatabaseMetadata::empty(Some(name));
            next.server_version = self.current.server_version.take();
            next.pg_dump_version = self.current.pg_dump_version.take();
            self.current = next;
        }
        self.seen_connect = true;
    }

    fn finalize(mut db: DatabaseMetadata) -> DatabaseMetadata {
        db.preamble_complete = true;
        db
    }

    /// Feed one outside-block line (already known not to be a COPY header
    /// and not touched by dollar-quote tracking).
    pub(crate) fn feed_line(&mut self, raw: &[u8]) {
        let line = String::from_utf8_lossy(raw);

        if let Some(name) = parse_connect(&line) {
            self.on_connect(name);
            return;
        }

        // A statement in progress absorbs this line regardless of
        // `preamble_complete` — that flag can only be set between
        // statements (`on_copy_start`/`on_connect` both clear `pending`
        // first), so a line reaching here while `pending.is_some()` is
        // always mid-statement.
        if let Some(pending) = &mut self.pending {
            pending.buf.push('\n');
            pending.buf.push_str(&line);
            if statement_complete(&pending.buf) {
                let stmt = self.pending.take().unwrap().buf;
                self.dispatch(&stmt);
            }
            return;
        }

        if self.current.preamble_complete {
            return;
        }

        if let Some(rest) = line.strip_prefix("-- Dumped from database version ") {
            self.current.server_version = Some(rest.trim().to_string());
            return;
        }
        if let Some(rest) = line.strip_prefix("-- Dumped by pg_dump version ") {
            self.current.pg_dump_version = Some(rest.trim().to_string());
            return;
        }

        let trimmed = line.trim_start();
        let triggers =
            ["CREATE TABLE ", "CREATE TYPE ", "CREATE DOMAIN ", "CREATE EXTENSION ", "ALTER TYPE "];
        if triggers.iter().any(|kw| {
            trimmed.len() >= kw.len()
                && trimmed.as_bytes()[..kw.len()].eq_ignore_ascii_case(kw.as_bytes())
        }) {
            let buf = trimmed.to_string();
            if statement_complete(&buf) {
                self.dispatch(&buf);
            } else {
                self.pending = Some(PendingStmt { buf });
            }
        }
    }

    fn dispatch(&mut self, stmt: &str) {
        let trimmed = stmt.trim_start();
        if let Some(rest) = strip_kw(trimmed, "CREATE TABLE") {
            if let Some((name, cols)) = parse_create_table(rest) {
                self.current.tables.insert(name, cols);
            }
        } else if let Some(rest) = strip_kw(trimmed, "CREATE TYPE") {
            if let Some(def) = parse_create_type(rest) {
                self.current.types.push(def);
            }
        } else if let Some(rest) = strip_kw(trimmed, "CREATE DOMAIN") {
            if let Some(def) = parse_create_domain(rest) {
                self.current.types.push(def);
            }
        } else if let Some(rest) = strip_kw(trimmed, "CREATE EXTENSION") {
            if let Some(ext) = parse_create_extension(rest) {
                self.current.extensions.push(ext);
            }
        } else if let Some(rest) = strip_kw(trimmed, "ALTER TYPE") {
            apply_alter_type_add_value(&mut self.current.types, rest);
        }
    }

    /// Finish the scan: whatever database is still open (the sole implicit
    /// one for a non-`--create` dump, or the last `\connect`ed one) is
    /// always real, so it is always pushed.
    pub(crate) fn finish(mut self) -> DumpMetadata {
        self.databases.push(Self::finalize(self.current));
        DumpMetadata { databases: self.databases }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_stmt(lines: &[&str]) -> DatabaseMetadata {
        let mut b = PreambleBuilder::new();
        for line in lines {
            b.feed_line(line.as_bytes());
        }
        let mut meta = b.finish();
        assert_eq!(meta.databases.len(), 1);
        meta.databases.pop().unwrap()
    }

    #[test]
    fn parses_a_simple_table() {
        let db = one_stmt(&[
            "CREATE TABLE public.t_int (",
            "    id integer NOT NULL,",
            "    v_smallint smallint,",
            "    v_bigint bigint",
            ");",
        ]);
        assert_eq!(
            db.tables.get("public.t_int").unwrap(),
            &vec![
                ("id".to_string(), "integer".to_string()),
                ("v_smallint".to_string(), "smallint".to_string()),
                ("v_bigint".to_string(), "bigint".to_string()),
            ]
        );
    }

    #[test]
    fn captures_multi_word_and_parameterized_types() {
        let db = one_stmt(&[
            "CREATE TABLE public.t (",
            "    a character varying(16) NOT NULL,",
            "    b numeric(38,10),",
            "    c timestamp with time zone DEFAULT now(),",
            "    d public.mood",
            ");",
        ]);
        let cols = db.tables.get("public.t").unwrap();
        assert_eq!(cols[0], ("a".to_string(), "character varying(16)".to_string()));
        assert_eq!(cols[1], ("b".to_string(), "numeric(38,10)".to_string()));
        assert_eq!(cols[2], ("c".to_string(), "timestamp with time zone".to_string()));
        assert_eq!(cols[3], ("d".to_string(), "public.mood".to_string()));
    }

    #[test]
    fn binary_upgrade_dummy_column_comment_and_quoted_identifier() {
        // I5, confirmed against `fixtures/*/edge_cases/binary-upgrade.sql`:
        // a `--binary-upgrade` dump recreates a dropped column as a dummy
        // typed column, quoting its mangled name and annotating the type
        // with a C-style comment neither of which is ordinary SQL syntax
        // elsewhere in this grammar.
        let db = one_stmt(&[
            "CREATE TABLE public.dropped_column (",
            "    id integer NOT NULL,",
            "    keep_me text,",
            "    \"........pg.dropped.3........\" INTEGER /* dummy */,",
            "    also_keep boolean",
            ");",
        ]);
        let cols = db.tables.get("public.dropped_column").unwrap();
        assert_eq!(cols[2], ("........pg.dropped.3........".to_string(), "INTEGER".to_string()));
    }

    #[test]
    fn table_with_no_column_list_registers_with_zero_columns() {
        // I5: a typed table (`CREATE TABLE x OF t`) has no column list.
        let db = one_stmt(&["CREATE TABLE public.typed OF public.point2d;"]);
        assert_eq!(db.tables.get("public.typed").unwrap(), &Vec::new());
    }

    #[test]
    fn parses_an_enum() {
        let db = one_stmt(&[
            "CREATE TYPE public.mood AS ENUM (",
            "    'sad',",
            "    'has space',",
            "    'has,comma',",
            "    'has''quote'",
            ");",
        ]);
        assert_eq!(db.types.len(), 1);
        assert_eq!(db.types[0].name, "public.mood");
        assert_eq!(
            db.types[0].kind,
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

    #[test]
    fn binary_upgrade_enum_labels_arrive_via_alter_type() {
        let db = one_stmt(&[
            "CREATE TYPE public.mood AS ENUM (",
            ");",
            "SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16438'::pg_catalog.oid);",
            "ALTER TYPE public.mood ADD VALUE 'sad';",
            "SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16440'::pg_catalog.oid);",
            "ALTER TYPE public.mood ADD VALUE 'has''quote';",
        ]);
        assert_eq!(
            db.types[0].kind,
            TypeKind::Enum { labels: vec!["sad".to_string(), "has'quote".to_string()] }
        );
    }

    #[test]
    fn binary_upgrade_oid_noise_between_comment_and_statement_is_skipped() {
        // I6: every object's real statement is preceded by
        // `binary_upgrade_set_next_*_oid` noise, not just enums'.
        let db = one_stmt(&[
            "-- For binary upgrade, must preserve pg_type oid",
            "SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16450'::pg_catalog.oid);",
            "",
            "CREATE DOMAIN public.base_domain AS integer;",
        ]);
        assert_eq!(db.types[0].kind, TypeKind::Domain { base_type: "integer".to_string() });
    }

    #[test]
    fn parses_a_domain_over_a_domain() {
        let db = one_stmt(&["CREATE DOMAIN public.derived AS public.base_domain NOT NULL;"]);
        assert_eq!(
            db.types[0],
            TypeDef {
                name: "public.derived".to_string(),
                kind: TypeKind::Domain { base_type: "public.base_domain".to_string() },
            }
        );
    }

    #[test]
    fn parses_a_composite_type() {
        let db = one_stmt(&["CREATE TYPE public.point2d AS (", "\tx integer,", "\ty text", ");"]);
        assert_eq!(
            db.types[0].kind,
            TypeKind::Composite {
                fields: vec![
                    ("x".to_string(), "integer".to_string()),
                    ("y".to_string(), "text".to_string())
                ]
            }
        );
    }

    #[test]
    fn parses_a_shell_type() {
        let db = one_stmt(&["CREATE TYPE public.shelly;"]);
        assert_eq!(db.types[0].kind, TypeKind::Shell);
    }

    #[test]
    fn parses_a_base_type() {
        let db = one_stmt(&[
            "CREATE TYPE public.mytype (",
            "    INPUT = mytype_in,",
            "    OUTPUT = mytype_out",
            ");",
        ]);
        assert_eq!(db.types[0].kind, TypeKind::Base);
    }

    #[test]
    fn parses_a_range_type() {
        let db = one_stmt(&[
            "CREATE TYPE public.myrange AS RANGE (subtype = int4, subtype_diff = int4mi);",
        ]);
        assert_eq!(db.types[0].kind, TypeKind::Range { subtype: Some("int4".to_string()) });
    }

    #[test]
    fn parses_an_extension() {
        let db = one_stmt(&["CREATE EXTENSION IF NOT EXISTS pgcrypto WITH SCHEMA public;"]);
        assert_eq!(
            db.extensions[0],
            Extension { name: "pgcrypto".to_string(), schema: Some("public".to_string()) }
        );
    }

    #[test]
    fn extension_without_a_schema_clause() {
        let db = one_stmt(&["CREATE EXTENSION IF NOT EXISTS pgcrypto;"]);
        assert_eq!(db.extensions[0], Extension { name: "pgcrypto".to_string(), schema: None });
    }

    #[test]
    fn version_headers_are_captured() {
        let db = one_stmt(&[
            "-- Dumped from database version 16.15",
            "-- Dumped by pg_dump version 16.15",
        ]);
        assert_eq!(db.server_version.as_deref(), Some("16.15"));
        assert_eq!(db.pg_dump_version.as_deref(), Some("16.15"));
    }

    #[test]
    fn plain_dump_has_no_connect_and_names_no_database() {
        let db = one_stmt(&["CREATE TABLE public.t (id integer);"]);
        assert_eq!(db.name, None);
        assert!(db.preamble_complete);
    }

    #[test]
    fn connect_starts_a_new_named_database_and_drops_the_preconnect_segment() {
        let mut b = PreambleBuilder::new();
        for line in [
            "-- Dumped from database version 16.14",
            "CREATE DATABASE koji WITH TEMPLATE = template0;",
        ] {
            b.feed_line(line.as_bytes());
        }
        b.feed_line(b"\\connect koji");
        b.feed_line(b"CREATE TABLE public.t (id integer);");
        let meta = b.finish();

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
        // first database here starts via `\connect`, same as the second.
        let mut b = PreambleBuilder::new();
        b.feed_line(b"\\connect one");
        b.feed_line(b"CREATE TABLE public.a (id integer);");
        b.on_copy_start();
        // Per I1, nothing more should be captured for this database now.
        b.feed_line(b"CREATE TABLE public.ignored (id integer);");
        b.feed_line(b"\\connect two");
        b.feed_line(b"CREATE TABLE public.b (id integer);");
        let meta = b.finish();

        assert_eq!(meta.databases.len(), 2);
        assert_eq!(meta.databases[0].name.as_deref(), Some("one"));
        assert!(meta.databases[0].tables.contains_key("public.a"));
        assert!(!meta.databases[0].tables.contains_key("public.ignored"));
        assert_eq!(meta.databases[1].name.as_deref(), Some("two"));
        assert!(meta.databases[1].tables.contains_key("public.b"));
    }

    #[test]
    fn data_only_dump_has_no_tables_but_still_reports_versions() {
        let db = one_stmt(&[
            "-- Dumped from database version 16.15",
            "-- Dumped by pg_dump version 16.15",
        ]);
        assert!(db.tables.is_empty());
        assert!(db.types.is_empty());
        assert_eq!(db.server_version.as_deref(), Some("16.15"));
    }
}
