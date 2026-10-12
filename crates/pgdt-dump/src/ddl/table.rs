//! `CREATE TABLE` in each of its forms, and the `ALTER TABLE` statements that
//! add a reference to a table already declared. A column's own grammar is
//! [`column`](super::column)'s.

use super::column::{check_constraints, not_null_clause, parse_column_fragment};
use super::syntax::{
    collapse_spacing, matching_paren, parse_qualified_name, split_top_level_commas,
    strip_block_comments,
};
use crate::lex::strip_kw;
use pgdt_core::internal::{Cursor, parse_type_name};
use pgdt_model::{CheckConstraint, ColumnDef, NotNull, PartitionOf, TableDef, TableReference};

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
pub(super) fn parse_create_table(rest: &str) -> Option<(String, TableDef)> {
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
/// never writes for these. Read by [`classify`](super::classify), as
/// [`parse_alter_type_add_value_body`](super::types::parse_alter_type_add_value_body) is.
pub(super) fn parse_alter_table_reference(stmt: &str) -> Option<(String, TableReference)> {
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

#[cfg(test)]
mod tests;
