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
//! [`classify_statement`] and friends (`parse_create_table`,
//! `parse_create_type`, …) are the shared parser [`crate::map::classify`] calls
//! to turn a complete statement into a [`pgdt_model::SpanBody`] while it builds
//! the full file map in its one pass over [`crate::scan::scan`]'s events.
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
//! of statement scan this module owns: unlike [`classify_statement`], it
//! doesn't try to fully parse a statement into a `SpanBody` — it just pulls
//! out whatever role/tablespace references `crate::map::Builder::push_statement_span`
//! feeds it, from statement shapes [`classify_statement`] does not recognize
//! (`OWNER TO`, `GRANT`/`REVOKE`/`ALTER DEFAULT PRIVILEGES FOR ROLE`, `SET
//! default_tablespace`).

mod lex;
mod metadata;
mod refs;
mod table;
mod types;

pub(crate) use self::lex::{parse_qualified_name, strip_kw};
pub use self::metadata::dump_metadata_from_spans;
pub(crate) use self::refs::{extract_statement_cross_refs, insert_role, insert_tablespace};
pub(crate) use self::table::parse_alter_table_reference;
use self::table::parse_create_table;
pub(crate) use self::types::{
    alter_type_labels_unread, parse_alter_type_add_value_body, parse_alter_type_drop_attribute_body,
};
use self::types::{
    parse_create_collation, parse_create_domain, parse_create_extension, parse_create_type,
};
use pgdt_core::internal::Cursor;
use pgdt_model::{CollationDef, Extension, TableDef, TypeDef};

/// The four statement shapes [`classify_statement`] recognizes directly.
/// `ALTER TYPE ADD VALUE` and `DROP ATTRIBUTE` are not among them: each
/// mutates an already-declared object rather than introducing one, so it has
/// its own two entry points — [`parse_alter_type_add_value_body`] and
/// [`parse_alter_type_drop_attribute_body`] for [`crate::map::classify`],
/// `fold_alter_type_add_value` and `fold_alter_type_drop_attribute` for
/// [`dump_metadata_from_spans`] — and so, for the same reason, has an `ALTER
/// TABLE` adding a [`pgdt_model::TableReference`]
/// ([`parse_alter_table_reference`], folded by [`TableDef`]).
#[derive(Debug)]
pub(crate) enum StatementShape {
    Table { name: String, definition: TableDef },
    Type(TypeDef),
    Extension(Extension),
    Collation(CollationDef),
}

/// Classify a complete statement (see `statement_complete`) as one of
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
