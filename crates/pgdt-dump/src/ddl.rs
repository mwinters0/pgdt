//! The statement grammar: `CREATE TABLE`/`TYPE`/`DOMAIN`/`EXTENSION`/
//! `COLLATION`, the `ALTER` statements amending them, `\connect`, and the
//! roles and tablespaces a statement references, read from any complete
//! statement the map builder closes, before the data or after it
//! (`docs/design/decisions.md`, "D36"). Tables and their columns are
//! [`table`], the other declarations [`types`], the references [`refs`], the
//! derived view [`metadata`], and the keyword and quoting helpers they share
//! [`lex`].
//!
//! **This module owns the DDL grammar, not a second pass over the file.**
//! [`classify`] turns a complete statement into a [`pgdt_model::SpanBody`],
//! which the map builder calls while it builds the full file map in its one
//! pass over [`crate::scan::scan`]'s events.
//! [`pgdt_model::DumpMetadata`] is then [`dump_metadata_from_spans`] — a
//! derived view over the resulting spans, computed once, never a second
//! line-by-line scan (`docs/design/decisions.md`, "D34").
//!
//! It stores what the dump said, never what we concluded: declared types are
//! kept as the words written (`character varying(16)`, not a parsed
//! `(base, typmod)` pair), the model being unable to hold a codec conclusion
//! (`docs/design/decisions.md`, "D74"). Resolving those strings into Arrow
//! types is `pgdt_codec::pgtype`'s job.
//!
//! [`extract_statement_cross_refs`] is a second, independent kind
//! of statement scan this module owns: unlike [`classify`], it
//! doesn't try to fully parse a statement into a `SpanBody` — it just pulls
//! out whatever role/tablespace references `crate::map::Builder::push_statement_span`
//! feeds it, from statement shapes [`classify`] does not recognize
//! (`OWNER TO`, `GRANT`/`REVOKE`/`ALTER DEFAULT PRIVILEGES FOR ROLE`, `SET
//! default_tablespace`).

mod lex;
mod metadata;
mod refs;
mod table;
mod types;

pub(crate) use self::lex::parse_qualified_name;
use self::lex::strip_kw;
pub use self::metadata::dump_metadata_from_spans;
pub(crate) use self::refs::{extract_statement_cross_refs, insert_role, insert_tablespace};
use self::table::{parse_alter_table_reference, parse_create_table};
use self::types::{
    alter_type_labels_unread, parse_alter_type_add_value_body,
    parse_alter_type_drop_attribute_body, parse_create_collation, parse_create_domain,
    parse_create_extension, parse_create_type,
};
use pgdt_core::internal::Cursor;
use pgdt_model::{Extension, SpanBody, TypeDef};

/// Classify a complete statement (see `statement_complete`) as the span it
/// is, [`SpanBody::Unparsed`] for anything this grammar does not read. The
/// map builder has already taken the framing statements out
/// (`looks_like_framing_statement`), which are the map's, not the grammar's.
///
/// Three `ALTER` statements are spans of their own rather than declarations:
/// each amends an already-declared object rather than introducing one, so it
/// is folded into it later — `ALTER TYPE ADD VALUE` and `DROP ATTRIBUTE` by
/// `fold_alter_type_add_value` and `fold_alter_type_drop_attribute` in
/// [`dump_metadata_from_spans`], an `ALTER TABLE` adding a
/// [`pgdt_model::TableReference`] by [`pgdt_model::TableDef`]. The rest are
/// the `CREATE` shapes introducing one ([`classify_create`]).
pub(crate) fn classify(stmt: &str) -> SpanBody {
    if let Some(rest) = strip_kw(stmt.trim_start(), "ALTER TYPE") {
        if let Some((type_name, label)) = parse_alter_type_add_value_body(rest) {
            return SpanBody::AlterTypeAddValue { type_name, label };
        }
        if let Some((type_name, attribute)) = parse_alter_type_drop_attribute_body(rest) {
            return SpanBody::AlterTypeDropAttribute { type_name, attribute };
        }
        if let Some(type_name) = alter_type_labels_unread(rest) {
            return SpanBody::EnumLabelsUnread { type_name };
        }
    }
    if let Some((table, reference)) = parse_alter_table_reference(stmt) {
        return SpanBody::AlterTableReference { table, reference };
    }
    classify_create(stmt).unwrap_or(SpanBody::Unparsed)
}

/// Classify a complete statement as one of the four `CREATE` shapes this
/// grammar reads — a table, a type or domain, an extension, a collation —
/// or `None` for anything else, the `ALTER` statements [`classify`] reads
/// before it included.
///
/// A table is any of the three `dumpTableSchema` writes as `CREATE %s%s %s`
/// (I54): a plain one, an `UNLOGGED` one and a `FOREIGN TABLE`, whose list
/// and `INHERITS` are a plain table's, its `SERVER` clause after them.
fn classify_create(stmt: &str) -> Option<SpanBody> {
    let trimmed = stmt.trim_start();
    if let Some(rest) = ["CREATE TABLE", "CREATE UNLOGGED TABLE", "CREATE FOREIGN TABLE"]
        .into_iter()
        .find_map(|kw| strip_kw(trimmed, kw))
    {
        return parse_create_table(rest)
            .map(|(name, definition)| SpanBody::Table { name, definition });
    }
    if let Some(rest) = strip_kw(trimmed, "CREATE TYPE") {
        return parse_create_type(rest)
            .map(|TypeDef { name, kind }| SpanBody::TypeDef { name, kind });
    }
    if let Some(rest) = strip_kw(trimmed, "CREATE DOMAIN") {
        return parse_create_domain(rest)
            .map(|TypeDef { name, kind }| SpanBody::TypeDef { name, kind });
    }
    if let Some(rest) = strip_kw(trimmed, "CREATE EXTENSION") {
        return parse_create_extension(rest)
            .map(|Extension { name, schema }| SpanBody::Extension { name, schema });
    }
    if let Some(rest) = strip_kw(trimmed, "CREATE COLLATION") {
        return parse_create_collation(rest).map(|collation| SpanBody::Collation { collation });
    }
    None
}

/// `\connect <name>` — a bare prefix check on a line the scanner already
/// holds, read by the map builder: tracking which database a `CopyBlock`
/// belongs to needs only the name a `\connect` yields, never a column type.
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

#[cfg(test)]
mod tests;
