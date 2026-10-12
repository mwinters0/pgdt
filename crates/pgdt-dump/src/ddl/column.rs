//! A column's grammar, which a table's list, a composite's fields and a
//! domain's tail share: its declared type, its `COLLATE`, its `NOT NULL` and
//! its `CHECK`s.

use super::syntax::{
    is_word_start, parse_qualified_name, skip_double_quoted, skip_quoted, strip_block_comments,
    top_level_words, type_tokens,
};
use pgdt_core::internal::Cursor;
use pgdt_model::{CheckConstraint, ColumnDef, NotNull};

/// Parse one `<name> <type> [constraints...]` fragment from a column or
/// composite-field list.
pub(super) fn parse_column_fragment(frag: &str) -> Option<ColumnDef> {
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

pub(super) fn extract_type_words(rest: &str) -> String {
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

/// The `COLLATE <collation>` clause of a column or domain definition, with
/// the reference returned **verbatim** (`pg_catalog."C"`) — the model stores what the
/// dump said, and `pgdt_codec::pgtype` decides what it means.
///
/// `rest` is the tail after a column's name, or a domain's base-type tail, so
/// the clause is not adjacent to the type: `pg_dump` writes it *after*
/// `DEFAULT` and `NOT NULL` (I37), which is why this is a scan rather than a
/// look at the token following the type words. The scan is top-level only —
/// paren depth zero, outside `'…'` and `"…"` — so a `CHECK (v COLLATE "C" >
/// 'a')` constraint and a `DEFAULT 'collate me'` literal are both stepped
/// over rather than mistaken for the column's own clause.
pub(super) fn extract_collation(rest: &str) -> Option<String> {
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

/// The `NOT NULL` among a column's or a domain's constraints — `rest` being
/// the tail after its name or `AS` — or what makes the column one: an inline
/// `PRIMARY KEY`, or `GENERATED … AS IDENTITY` (I76). Read off the top-level
/// words, so a `CHECK (v IS NOT NULL)`, a `GENERATED ALWAYS AS (…)` expression
/// and a `'NOT NULL'` literal are stepped over; a default cannot hold `IS
/// NOT NULL` outside parens, gram.y's `b_expr` having no `IS NULL`.
pub(super) fn not_null_clause(rest: &str) -> Option<NotNull> {
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
pub(super) fn check_constraints(rest: &str) -> Vec<CheckConstraint> {
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

#[cfg(test)]
mod tests;
