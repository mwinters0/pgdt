//! Dump-level preamble parsing: `CREATE TABLE`/`TYPE`/`DOMAIN`/`EXTENSION`
//! DDL and the two version header lines, recovered from the pre-data region
//! of a `pg_dump` plain-format file (`docs/design/roadmap-phase2-typed-columns.md`,
//! "The preamble pass").
//!
//! **This module owns the DDL grammar, not a second pass over the file.**
//! [`classify_statement`] and friends (`parse_create_table`, `parse_create_type`,
//! …) are the shared parser [`crate::map::classify`] calls to turn a complete
//! statement into a [`crate::map::SpanBody`] while it builds the full file
//! map in its one pass over [`crate::scan::scan`]'s events. [`DumpMetadata`]
//! is then [`dump_metadata_from_spans`] — a derived view over the resulting
//! spans, computed once, never a second line-by-line scan
//! (`docs/design/roadmap-phase3-object-inventory.md`, "The span is the
//! container"). Before Phase 3.2.1.1 this module drove its own line-by-line
//! state machine (`PreambleBuilder`) in parallel with the span builder; see
//! `docs/design/roadmap-phase3.2.1-span-wiring-notes.md` for why the two
//! were unified.
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
    /// names no database. Never guessed — see [`dump_metadata_from_spans`]'s
    /// docs for how a `--create` dump's pre-`\connect` segment (which names
    /// no real database either, but for a different reason) is told apart
    /// from this case.
    pub name: Option<String>,
    /// Whether this database's preamble was read to completion. Always
    /// `true` for every entry a [`crate::index::build_index`] full scan or a
    /// [`crate::index::scan_preamble`] prepass produces — both only ever
    /// finish a database's segment, never leave one half-read. What it
    /// composes with is `DumpIndex::scanned_through`: the *first* database's
    /// metadata is guaranteed present after any scan that persists a cache
    /// (`crate::stream::table_stream`'s Phase 2.2.1 prepass — see
    /// `docs/design/roadmap-phase2-typed-columns-notes.md`, "Preamble
    /// parsing"), but a
    /// later `\connect`-ed database's is only ever populated by a full scan
    /// — so a caller walking `DumpIndex::metadata` still needs to check this
    /// per-database rather than assume the whole list is complete just
    /// because a cache file exists.
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
    /// VALUE` per label (I6); [`fold_alter_type_add_value`] folds them back
    /// in here so both forms produce the same shape.
    Enum { labels: Vec<String> },
    /// Reduces to a base type, resolved transitively by Phase 2.3 (a domain
    /// over a domain is legal). `NOT NULL` is discarded — Phase 2 makes
    /// every field nullable regardless (see "Nullability" in the phase doc).
    Domain { base_type: String },
    /// Field name -> declared type, in declaration order. Decoding COPY
    /// TEXT's record literal is Phase 4's job; this is what that decoder
    /// will need.
    Composite { fields: Vec<(String, String)> },
    /// The subtype named in the `CREATE TYPE ... AS RANGE (...)` parameter
    /// list, if the grammar found one, plus the name of its auto-created
    /// companion multirange type (PG14+), if the DDL named one explicitly
    /// via `multirange_type_name` (I10, `docs/design/postgres-invariants.md`).
    /// `pg_dump` never emits a `CREATE TYPE` for that companion at all — this
    /// parameter is its only trace in the file, which is why `crate::pgtype`
    /// needs it to resolve a column declared with that name instead of
    /// falling through to `Unknown`.
    Range { subtype: Option<String>, multirange_type_name: Option<String> },
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
        let mut subtype = None;
        let mut multirange_type_name = None;
        for kv in split_top_level_commas(&body[open + 1..close]) {
            let Some((k, v)) = kv.split_once('=') else { continue };
            let k = k.trim();
            let v = v.trim().to_string();
            if k.eq_ignore_ascii_case("subtype") {
                subtype = Some(v);
            } else if k.eq_ignore_ascii_case("multirange_type_name") {
                multirange_type_name = Some(v);
            }
        }
        return Some(TypeDef { name, kind: TypeKind::Range { subtype, multirange_type_name } });
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

/// Parse the body of `ALTER TYPE <name> ADD VALUE '<label>' [BEFORE|AFTER
/// '<other>'];` — the `--binary-upgrade` shape for enum labels (I6) — after
/// `ALTER TYPE` has already been stripped. `BEFORE`/`AFTER` is ignored: a
/// `--binary-upgrade` dump only ever emits these in declaration order to
/// recreate the type from scratch (confirmed:
/// `fixtures/*/types/binary-upgrade.sql`), so appending is equivalent to
/// respecting them. Used by [`crate::map::classify`] to recognize the
/// statement as its own [`crate::map::SpanBody::AlterTypeAddValue`] span,
/// since that module has no already-open `TypeDef` to fold into the way
/// [`fold_alter_type_add_value`] does for [`dump_metadata_from_spans`].
pub(crate) fn parse_alter_type_add_value_body(rest: &str) -> Option<(String, String)> {
    let (name, consumed) = parse_qualified_name(rest)?;
    let after = &rest[consumed..];
    let idx = find_ci(after, "ADD VALUE")?;
    let label = parse_string_literal(after[idx + "ADD VALUE".len()..].trim_start())?;
    Some((name, label))
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

/// The three statement shapes [`classify_statement`] recognizes directly.
/// `ALTER TYPE ADD VALUE` isn't among them: it doesn't introduce a new
/// object, it mutates an already-declared one, which needs a different
/// signature — [`parse_alter_type_add_value_body`] for
/// [`crate::map::classify`] (no open `TypeDef` to fold into; gets its own
/// [`crate::map::SpanBody::AlterTypeAddValue`] span instead) and
/// [`fold_alter_type_add_value`] for [`dump_metadata_from_spans`] (which
/// folds that span's label into the `TypeDef` it targets).
#[derive(Debug)]
pub(crate) enum StatementShape {
    Table { name: String, columns: Vec<(String, String)> },
    Type(TypeDef),
    Extension(Extension),
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
    None
}

/// `\connect <name>` — a bare prefix check on a line the scanner already
/// holds. Exposed to `crate::stream`'s live scan too (Phase 2.3.3,
/// `docs/design/roadmap-phase2-typed-columns.md`, "One target per query"):
/// tracking which database a `CopyBlock` belongs to needs only the name a
/// `\connect` yields, never a column type, so it isn't "reading preamble as
/// it goes" in the sense that section rules out.
pub(crate) fn parse_connect(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("\\connect ")?;
    Cursor::new(rest.trim().as_bytes()).parse_ident()
}

/// Whether `buf` (everything accumulated for a statement so far) is a
/// complete SQL statement: parens balanced, not mid string literal or
/// double-quoted identifier or `--` line comment, and ending in `;`.
///
/// Tracks single-quoted strings (`''` doubling), double-quoted identifiers
/// (`""` doubling), and `--` line comments (closed by the next `\n` in
/// `buf`, since `buf` accumulates multiple physical lines joined by `\n` —
/// see [`crate::map`]'s module docs for why a comment can't just be
/// stripped up front). Without double-quote and comment awareness, an
/// apostrophe inside either (`public."it's"`, `-- it's here`) would open a
/// string that never closes and swallow every following line into the same
/// pending statement forever — unreachable through this module's five
/// `CREATE`/`ALTER TYPE` triggers (`pg_dump` emits neither shape for them),
/// but reachable by [`crate::map`]'s general statement scan, which is what
/// this hardening is for (`docs/status/history/2026-08-23.md`, "The Phase 2
/// statement accumulator is not yet safe for arbitrary statements").
/// `E'…'` escapes are not a concern: `pg_dump` sets
/// `standard_conforming_strings = on`, so `''` is the only in-string escape.
/// Where [`statement_complete`]'s scan over `buf` ends up: whether it's
/// mid string/double-quoted-identifier/line-comment, and the paren depth.
/// Exposed as [`in_open_quote`] for [`crate::map`]'s boundary-reassertion
/// check — see that function's docs.
struct ScanState {
    depth: i32,
    in_string: bool,
    in_dquote: bool,
    in_comment: bool,
}

fn scan_buf(buf: &str) -> ScanState {
    let mut st = ScanState { depth: 0, in_string: false, in_dquote: false, in_comment: false };
    let mut chars = buf.chars().peekable();
    while let Some(c) = chars.next() {
        if st.in_comment {
            if c == '\n' {
                st.in_comment = false;
            }
            continue;
        }
        if st.in_string {
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                } else {
                    st.in_string = false;
                }
            }
            continue;
        }
        if st.in_dquote {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                } else {
                    st.in_dquote = false;
                }
            }
            continue;
        }
        match c {
            '\'' => st.in_string = true,
            '"' => st.in_dquote = true,
            '-' if chars.peek() == Some(&'-') => {
                chars.next();
                st.in_comment = true;
            }
            '(' => st.depth += 1,
            ')' => st.depth -= 1,
            _ => {}
        }
    }
    st
}

pub(crate) fn statement_complete(buf: &str) -> bool {
    let st = scan_buf(buf);
    !st.in_string
        && !st.in_dquote
        && !st.in_comment
        && st.depth == 0
        && buf.trim_end().ends_with(';')
}

/// Whether `buf` ends inside an open single-quoted string or double-quoted
/// identifier — the one case where a line that syntactically *looks* like a
/// fresh boundary (starts with `--`, in [`crate::map`]'s case) is actually
/// just string content spanning multiple physical lines, and must not be
/// treated as one. `pg_dump` never emits a `--` comment inside a
/// non-dollar-quoted statement's own parens, so paren depth doesn't gate
/// this the same way — only being mid-string does.
pub(crate) fn in_open_quote(buf: &str) -> bool {
    let st = scan_buf(buf);
    st.in_string || st.in_dquote
}

/// Append `line` to a statement buffer being accumulated line by line,
/// joining with `\n` — but never a leading one before the buffer's first
/// line. Used by [`crate::map`]'s general statement scan.
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
/// instead of raw lines — the derived view
/// `docs/design/roadmap-phase3-object-inventory.md`'s "The span is the
/// container" calls for: multi-database segmenting on
/// [`crate::map::SpanBody::Connect`], version-header staging across that
/// boundary on [`crate::map::SpanBody::VersionHeader`], and `--binary-upgrade`
/// enum-label folding on [`crate::map::SpanBody::AlterTypeAddValue`] — see
/// `docs/design/roadmap-phase3.2.1-span-wiring-notes.md` for why those three
/// span kinds needed to exist before this could be written.
///
/// `spans` must come from a scan that stops at one of two safe boundaries:
/// end of file, or (per I1) the start of the current database's first `COPY`
/// block — [`crate::map::build_map`] and `crate::index::scan_preamble`'s own
/// span builder both only ever produce spans this way. A span list with an
/// `Unscanned` tail cut off anywhere else (e.g. `crate::stream::table_stream`'s
/// live segment) would make the trailing database's `preamble_complete` a lie.
pub fn dump_metadata_from_spans(spans: &[crate::map::Span]) -> DumpMetadata {
    use crate::map::SpanBody;

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
            // I1 guarantees none of these four can genuinely follow a `Data`
            // span for the current database before its next `Connect` — the
            // guard is defensive, matching what a line-triggered scan would
            // have done, rather than assuming the invariant holds.
            SpanBody::Table { name, columns } if !current.preamble_complete => {
                current.tables.insert(name.clone(), columns.clone());
            }
            SpanBody::TypeDef { name, kind } if !current.preamble_complete => {
                current.types.push(TypeDef { name: name.clone(), kind: kind.clone() });
            }
            SpanBody::Extension { name, schema } if !current.preamble_complete => {
                current.extensions.push(Extension { name: name.clone(), schema: schema.clone() });
            }
            SpanBody::AlterTypeAddValue { type_name, label } if !current.preamble_complete => {
                fold_alter_type_add_value(&mut current.types, type_name, label);
            }
            SpanBody::Table { .. }
            | SpanBody::TypeDef { .. }
            | SpanBody::Extension { .. }
            | SpanBody::AlterTypeAddValue { .. }
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
    /// result — these tests exercise the DDL grammar directly rather than
    /// through a database-segmenting builder, since nothing about that
    /// grammar depends on one.
    fn parse(lines: &[&str]) -> StatementShape {
        classify_statement(&lines.join("\n")).unwrap()
    }

    fn parse_table(lines: &[&str]) -> (String, Vec<(String, String)>) {
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
                ("id".to_string(), "integer".to_string()),
                ("v_smallint".to_string(), "smallint".to_string()),
                ("v_bigint".to_string(), "bigint".to_string()),
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
        let (_, cols) = parse_table(&[
            "CREATE TABLE public.dropped_column (",
            "    id integer NOT NULL,",
            "    keep_me text,",
            "    \"........pg.dropped.3........\" INTEGER /* dummy */,",
            "    also_keep boolean",
            ");",
        ]);
        assert_eq!(cols[2], ("........pg.dropped.3........".to_string(), "INTEGER".to_string()));
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

    /// The `--binary-upgrade` fold (I6) is [`dump_metadata_from_spans`]'s job
    /// now, not `classify_statement`'s — it needs an already-open `TypeDef`
    /// to fold the label into, which a span-level view of one statement at a
    /// time doesn't have. Exercised here at the level it now lives at: a
    /// `TypeDef` span for the empty enum, an unrelated `Unparsed` span for
    /// the `binary_upgrade_set_next_pg_enum_oid` noise every real dump
    /// interleaves (I6), and two `AlterTypeAddValue` spans.
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
                kind: TypeKind::Domain { base_type: "public.base_domain".to_string() },
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
                fields: vec![
                    ("x".to_string(), "integer".to_string()),
                    ("y".to_string(), "text".to_string())
                ]
            }
        );
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
            TypeKind::Range { subtype: Some("int4".to_string()), multirange_type_name: None }
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
        crate::map::Span { start: 0, end: 0, database: None, text: None, toc: None, body }
    }

    fn dummy_data_span() -> crate::map::Span {
        span(SpanBody::Data(crate::index::CopyBlock {
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
            sparse_index: None,
            column_stats: None,
        }))
    }

    #[test]
    fn plain_dump_has_no_connect_and_names_no_database() {
        let meta = dump_metadata_from_spans(&[span(SpanBody::Table {
            name: "public.t".to_string(),
            columns: vec![("id".to_string(), "integer".to_string())],
        })]);
        assert_eq!(meta.databases.len(), 1);
        assert_eq!(meta.databases[0].name, None);
        assert!(meta.databases[0].preamble_complete);
    }

    #[test]
    fn connect_starts_a_new_named_database_and_drops_the_preconnect_segment() {
        // `CREATE DATABASE koji ...;` isn't one of `classify`'s three shapes
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
                columns: vec![("id".to_string(), "integer".to_string())],
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
    /// just the first one — see `postgres-invariants.md`. Regression test
    /// for the gap phase 2.3.1's fixture (`fixtures/*/edge_cases/create.sql`
    /// x2, concatenated) surfaced: a second database's headers used to be
    /// silently dropped because they land while `current` is still the
    /// first database's already-`preamble_complete` segment.
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

    /// The gap `docs/status/history/2026-08-23.md` flagged: an apostrophe in
    /// comment prose used to open a string that never closed.
    #[test]
    fn statement_complete_is_not_confused_by_an_apostrophe_in_a_line_comment() {
        assert!(statement_complete("CREATE TABLE t (id integer);\n-- it's here\nSELECT 1;"));
        // Not complete until a real `;` follows the comment.
        assert!(!statement_complete("SELECT 1\n-- it's here"));
    }

    /// The gap `docs/status/history/2026-08-23.md` flagged: an apostrophe
    /// inside a double-quoted identifier used to open a string that never
    /// closed.
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

    #[test]
    fn classify_statement_recognizes_the_three_span_shapes() {
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
        assert!(classify_statement("ALTER TABLE t OWNER TO postgres;").is_none());
        assert!(classify_statement("ALTER TYPE t ADD VALUE 'x';").is_none());
    }
}
