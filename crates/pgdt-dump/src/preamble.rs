//! Dump-level preamble parsing: `CREATE TABLE`/`TYPE`/`DOMAIN`/`EXTENSION`
//! DDL and the two version header lines, recovered from the pre-data region
//! of a `pg_dump` plain-format file (`docs/design/decisions.md`,
//! "D36").
//!
//! **This module owns the DDL grammar, not a second pass over the file.**
//! [`classify_statement`] and friends (`parse_create_table`, `parse_create_type`,
//! …) are the shared parser [`crate::map::classify`] calls to turn a complete
//! statement into a [`pgdt_model::SpanBody`] while it builds the full file
//! map in its one pass over [`crate::scan::scan`]'s events. [`DumpMetadata`]
//! is then [`dump_metadata_from_spans`] — a derived view over the resulting
//! spans, computed once, never a second line-by-line scan
//! (`docs/design/decisions.md`, "D34").
//!
//! It stores what the dump said, never what we concluded: declared types are
//! kept as the words written (`character varying(16)`, not a parsed
//! `(base, typmod)` pair), the model being unable to hold a codec conclusion
//! (`docs/design/decisions.md`, "D74"). Resolving those strings into Arrow
//! types is [`pgdt_codec::pgtype`]'s job.
//!
//! [`extract_statement_cross_refs`] is a second, independent kind
//! of statement scan this module owns: unlike [`classify_statement`], it
//! doesn't try to fully parse a statement into a `SpanBody` — it just pulls
//! out whatever role/tablespace references `crate::map::Builder::push_statement_span`
//! feeds it, from statement shapes [`classify_statement`] does not recognize
//! (`OWNER TO`, `GRANT`/`REVOKE`/`ALTER DEFAULT PRIVILEGES FOR ROLE`, `SET
//! default_tablespace`).

use std::collections::BTreeSet;

use pgdt_core::internal::Cursor;
use pgdt_core::internal::{Lexer, Region, canonical_type_name, ident_cont, parse_type_name};
use pgdt_model::internal::{DatabaseMetadataExt, TableDefExt};
use pgdt_model::{
    CheckConstraint, CollationDef, ColumnDef, DatabaseMetadata, DumpMetadata, Extension, NotNull,
    PartitionOf, Span, SpanBody, TableDef, TableReference, TypeDef, TypeKind,
};

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
/// same folding rules as [`pgdt_core::parse_copy_header`]'s table names —
/// so the result matches [`pgdt_core::CopyHeader::qualified_name`] exactly
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
/// the reference returned **verbatim** (`pg_catalog."C"`) — the model stores what the
/// dump said, and [`pgdt_codec::pgtype`] decides what it means.
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

/// The words of `rest` at its top level — outside parens and `'…'` — as
/// keywords: a quoted identifier is its text, quotes and all, and a word a `.`
/// qualifies an empty word, neither matching a keyword, so no sequence matches
/// across them.
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
                let start = i;
                i = skip_double_quoted(bytes, i);
                if depth == 0 {
                    words.push(rest.get(start..i).unwrap_or(""));
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

/// The `CHECK` constraints among a column's constraints, a table constraint,
/// or a domain's constraints — `rest` being the fragment, or the tail after
/// `AS` — read off the top-level words, as [`not_null_clause`] reads, so a
/// `CHECK` word stands only where gram.y's `ConstraintElem` or
/// `ColConstraintElem` opens one (`check` being reserved, a name spelled so is
/// quoted). Each is named by a `CONSTRAINT <name>` before it, and is `NO
/// INHERIT` where those words follow it before the next `CONSTRAINT` or
/// `CHECK` word — a column's `NOT NULL NO INHERIT` after it being taken for
/// its own, which `pg_dump`, writing a `CHECK` as a table constraint, never
/// writes.
fn check_constraints(rest: &str) -> Vec<CheckConstraint> {
    let rest = strip_block_comments(rest);
    let words = top_level_words(&rest);
    let is = |at: usize, word: &str| words.get(at).is_some_and(|w| w.eq_ignore_ascii_case(word));
    (0..words.len())
        .filter(|&at| is(at, "CHECK"))
        .map(|at| {
            let name = at
                .checked_sub(2)
                .filter(|&named| is(named, "CONSTRAINT"))
                .and_then(|named| Cursor::new(words[named + 1].as_bytes()).parse_ident());
            let attributes = words[at + 1..]
                .iter()
                .take_while(|w| {
                    !w.eq_ignore_ascii_case("CONSTRAINT") && !w.eq_ignore_ascii_case("CHECK")
                })
                .count();
            let no_inherit =
                (at + 1..at + 1 + attributes).any(|i| is(i, "NO") && is(i + 1, "INHERIT"));
            CheckConstraint { name, no_inherit }
        })
        .collect()
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
/// its own: each is its type's field ([`TableDef`]), and only a `NOT NULL` and
/// a `CHECK` among its options are kept.
///
/// A partition's `CREATE TABLE <name> PARTITION OF <parent> [(<options>,
/// ...)] <bound>`, which no supported `pg_dump` writes (I86), takes the same
/// list a typed table does, and records its parent and bound.
fn parse_create_table(rest: &str) -> Option<(String, TableDef)> {
    let (name, consumed) = parse_qualified_name(rest)?;
    let mut after = rest[consumed..].trim_start();
    let mut table = TableDef::default();
    if let Some(of) = strip_kw(after, "OF") {
        let (of_type, consumed) = parse_type_name(of)?;
        table.of_type = Some(of_type);
        after = of[consumed..].trim_start();
    }
    // deficiency: KD102 — a partition declared `PARTITION OF` takes every
    // column, in its parent's order, and every `NOT NULL` and `CHECK` from
    // its parent, and only its parent and bound are recorded here: no lookup
    // walks to the parent, so each of its columns is declared nowhere, read
    // as text and checked for nothing, and a parent's `CHECK` is not named
    // for it. No `pg_dump` writes one (I86). The fix walks `partition_of`'s
    // parent as `OF`'s type is walked for a table declaring no column of its
    // own — never for an attached one, whose own list is whole and in its own
    // order — and in `column_not_null` and `table_checks` as a parent is.
    let mut partition_parent = None;
    if let Some(of) = strip_kw(after, "PARTITION").and_then(|p| strip_kw(p, "OF")) {
        let (parent, consumed) = parse_qualified_name(of)?;
        partition_parent = Some(parent);
        after = of[consumed..].trim_start();
    }
    let partition_of = |tail: &str| {
        partition_parent
            .clone()
            .map(|parent| PartitionOf { parent, bound: partition_bound(tail).map(|(b, _)| b) })
    };
    if !after.starts_with('(') {
        table.partition_of = partition_of(after);
        return Some((name, table));
    }
    let close = matching_paren(after.as_bytes(), 0)?;
    table.partition_of = partition_of(&after[close + 1..]);
    let typed = table.of_type.is_some() || table.partition_of.is_some();
    for fragment in split_top_level_commas(&after[1..close]) {
        let element = parse_table_element(fragment, typed);
        // A column's, a typed table's column option's and a table
        // constraint's `CHECK` alike; a `LIKE`'s options hold none.
        if !matches!(element, Some(TableElement::Like)) {
            table.checks.extend(check_constraints(fragment));
        }
        match element {
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

/// gram.y's `PartitionBoundSpec` at the start of `s` — `DEFAULT`, or `FOR
/// VALUES` then `IN (…)`, `FROM (…) TO (…)` or `WITH (…)` — as
/// [`PartitionOf::bound`] holds it, and what follows it.
fn partition_bound(s: &str) -> Option<(String, &str)> {
    fn parenthesized<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
        let open = strip_kw(s, kw)?;
        if !open.starts_with('(') {
            return None;
        }
        Some(&open[matching_paren(open.as_bytes(), 0)? + 1..])
    }
    let start = s.trim_start();
    let after = match strip_kw(start, "DEFAULT") {
        Some(after) => after,
        None => {
            let values = strip_kw(strip_kw(start, "FOR")?, "VALUES")?;
            parenthesized(values, "IN")
                .or_else(|| parenthesized(values, "WITH"))
                .or_else(|| parenthesized(parenthesized(values, "FROM")?.trim_start(), "TO"))?
        }
    };
    let text = &start[..start.len() - after.len()];
    Some((collapse_spacing(&strip_block_comments(text)), after))
}

/// `s` with each run of whitespace outside a `'…'` literal or a `"…"`
/// identifier one space, and none at either end.
fn collapse_spacing(s: &str) -> String {
    let bytes = s.trim().as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let end = match bytes[i] {
            b'\'' => skip_quoted(bytes, i),
            b'"' => skip_double_quoted(bytes, i),
            b if b.is_ascii_whitespace() => {
                out.push(b' ');
                i += bytes[i..].iter().take_while(|b| b.is_ascii_whitespace()).count();
                continue;
            }
            _ => i + 1,
        };
        out.extend_from_slice(&bytes[i..end]);
        i = end;
    }
    // Cut only beside ASCII bytes, so as whole as `s`.
    String::from_utf8_lossy(&out).into_owned()
}

/// `ALTER [FOREIGN] TABLE [ONLY] <name> INHERIT <parent>;` or `… OF
/// <type>;` — the two references `--binary-upgrade` writes
/// after a table's full column list (`dumpTableSchema`) — `… ALTER
/// [COLUMN] <column> SET NOT NULL;`, which a dump before v18 writes after a
/// `CREATE TABLE` not printing the column (I76), or `… ADD [CONSTRAINT
/// <name>] CHECK (…) …;`, which `--binary-upgrade` writes for a `CHECK` the
/// table inherits and a dump writes after the data for one it holds apart
/// (I77), as the table they alter and what they add; or `ALTER TABLE [ONLY]
/// <parent> ATTACH PARTITION <partition> <bound>;`, which every supported
/// `pg_dump` writes before the data (I86), as the partition it attaches and
/// its parent and bound. `None` for every other
/// `ALTER TABLE`, one listing several subcommands included, which the dump
/// never writes for these. `pub(crate)` for [`crate::map::classify`], as
/// [`parse_alter_type_add_value_body`] is.
pub(crate) fn parse_alter_table_reference(stmt: &str) -> Option<(String, TableReference)> {
    let stmt = stmt.trim_start();
    let rest = strip_kw(stmt, "ALTER TABLE").or_else(|| strip_kw(stmt, "ALTER FOREIGN TABLE"))?;
    let rest = strip_kw(rest, "ONLY").unwrap_or(rest);
    let (table, consumed) = parse_qualified_name(rest)?;
    let rest = rest[consumed..].trim_start();
    // gram.y's `partition_cmd` stands alone in its statement, so the bound
    // ends it; one that is none of `PartitionBoundSpec`'s forms is recorded
    // as unread rather than dropping the parent.
    if let Some(attach) = strip_kw(rest, "ATTACH").and_then(|a| strip_kw(a, "PARTITION")) {
        let (partition, consumed) = parse_qualified_name(attach)?;
        let tail = &attach[consumed..];
        let bound = partition_bound(tail).filter(|(_, after)| after.trim() == ";");
        if bound.is_none() && !tail.trim_end().ends_with(';') {
            return None;
        }
        let bound = bound.map(|(bound, _)| bound);
        return Some((
            partition,
            TableReference::PartitionOf(PartitionOf { parent: table, bound }),
        ));
    }
    if let Some(add) = strip_kw(rest, "ADD") {
        let body = add.trim_end().strip_suffix(';')?;
        let opens = strip_kw(body, "CONSTRAINT").is_some() || strip_kw(body, "CHECK").is_some();
        let [check] = <[CheckConstraint; 1]>::try_from(check_constraints(body)).ok()?;
        return (opens && split_top_level_commas(body).len() == 1)
            .then_some((table, TableReference::Check(check)));
    }
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
/// is told apart and dropped, a `CHECK` being read off the fragment beside it
/// ([`check_constraints`]), and a `LIKE`'s columns are not followed.
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
    let check = !check_constraints(after_as).is_empty();
    Some(TypeDef { name, kind: TypeKind::Domain { base_type, collation, not_null, check } })
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
        return Some(TypeDef { name, kind: TypeKind::Base { delimiter: None } });
    }
    // `CREATE TYPE x (INPUT = ..., OUTPUT = ...);` — the C-level base type
    // shape, with no `AS` at all.
    Some(TypeDef { name, kind: TypeKind::Base { delimiter: base_type_delimiter(after) } })
}

/// The `typdelim` a base type's parameter list `body` declares: the first
/// byte of its `DELIMITER` literal, as `DefineType` takes `p[0]` of it, and
/// `,` where it names none (I22). `None` where the list is not closed or the
/// value is no plain string literal — `pg_dump` writing one through
/// `appendStringLiteralAH`, whose doubled backslash under
/// `standard_conforming_strings = off` leaves the first byte as it was. A v15+
/// server writes a non-ASCII `typdelim` as `charout`'s octal escape,
/// `DELIMITER = '\377'`, which a restore takes as `\`; this reads that byte,
/// and the split declines it.
fn base_type_delimiter(body: &str) -> Option<u8> {
    if !body.starts_with('(') {
        return None;
    }
    let close = matching_paren(body.as_bytes(), 0)?;
    let Some(value) = split_top_level_commas(&body[1..close]).into_iter().find_map(|kv| {
        let (key, value) = kv.split_once('=')?;
        key.trim().eq_ignore_ascii_case("delimiter").then_some(value.trim())
    }) else {
        return Some(b',');
    };
    let literal = parse_string_literal(value)?;
    // The literal alone, nothing after its closing quote.
    (format!("'{}'", literal.replace('\'', "''")) == value)
        .then(|| literal.bytes().next().unwrap_or(0))
}

/// Parse the body of `ALTER TYPE <name> ADD VALUE '<label>' [BEFORE|AFTER
/// '<other>'];` — the `--binary-upgrade` shape for enum labels (I6) — after
/// `ALTER TYPE` has already been stripped. `BEFORE`/`AFTER` is ignored: a
/// `--binary-upgrade` dump only ever emits these in declaration order
/// (`fixtures/*/types/binary-upgrade.sql`), so appending is equivalent to
/// respecting them. Used by [`crate::map::classify`] to recognize the
/// statement as its own [`pgdt_model::SpanBody::AlterTypeAddValue`] span,
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
/// own [`pgdt_model::SpanBody::AlterTypeAddValue`] span, separate from the
/// [`pgdt_model::SpanBody::TypeDef`] span it targets.
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

/// Filters out the pseudo-role `buildACLCommands` writes literally as
/// `PUBLIC` whenever a grant/revoke's grantee list is empty, and nothing
/// else. [`ident_after`]'s [`Cursor::parse_ident`] lowercases every
/// *unquoted* identifier, so the keyword arrives here as `public`, the one
/// name no role can hold (I85); a role quoted `"PUBLIC"` arrives as itself
/// and is kept, as is a TOC `Owner:` field's raw `PUBLIC`, which names it.
pub(crate) fn insert_role(roles: &mut BTreeSet<String>, role: String) {
    if role != "public" {
        roles.insert(role);
    }
}

/// Filters out `pg_default`, the database's built-in default tablespace,
/// compared exactly as [`insert_role`] compares `public`: a tablespace
/// quoted `"PG_DEFAULT"` is one a server can create (I85), and is kept.
pub(crate) fn insert_tablespace(tablespaces: &mut BTreeSet<String>, tablespace: String) {
    if tablespace != "pg_default" {
        tablespaces.insert(tablespace);
    }
}

/// Extract every role/tablespace a complete statement (see
/// [`statement_complete`]) references —
/// the two sources [`pgdt_model::Span::toc`] alone can't cover, since none of
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
/// holds. Exposed to `pgdt_query::stream`'s live scan too:
/// tracking which database a `CopyBlock` belongs to needs only the name a
/// `\connect` yields, never a column type.
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
/// where [`pgdt_core::lex`] leaves it — the rules [`crate::scan::CopyScanner`]
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
        // Deficiency register: `deficiency: KD9` — an `INSERT` run costs an
        // order of magnitude more than a `COPY` scan warm, and the device is
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

/// Build a [`DumpMetadata`] by walking already-classified [`pgdt_model::Span`]s
/// instead of raw lines — the derived view `docs/design/decisions.md`'s "D34"
/// calls for: multi-database segmenting on
/// [`pgdt_model::SpanBody::Connect`], version-header staging across that
/// boundary on [`pgdt_model::SpanBody::VersionHeader`], and `--binary-upgrade`
/// enum-label folding on [`pgdt_model::SpanBody::AlterTypeAddValue`] and
/// [`pgdt_model::SpanBody::EnumLabelsUnread`], dropped-attribute folding on
/// [`pgdt_model::SpanBody::AlterTypeDropAttribute`]
/// and table-reference folding on [`pgdt_model::SpanBody::AlterTableReference`].
///
/// `spans` must come from a scan that stops at one of two safe boundaries:
/// end of file, or (per I1) the start of the current database's first `COPY`
/// block — [`pgdt_query::build_map`] and `crate::load::scan_preamble`'s own
/// span builder both only ever produce spans this way. A span list with an
/// `Unscanned` tail cut off anywhere else (e.g. `pgdt_query::table_stream`'s
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
mod tests;
