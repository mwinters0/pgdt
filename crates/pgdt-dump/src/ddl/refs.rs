//! The roles and tablespaces a statement references where no TOC entry of its
//! own names them: `OWNER TO`, a grant's grantee, `ALTER DEFAULT PRIVILEGES
//! FOR ROLE`, `SET default_tablespace` — and the two built-in names never
//! recorded, which a TOC comment's fields are filtered by too.

use std::collections::BTreeSet;

use super::lex::{find_ci, strip_kw};
use pgdt_core::internal::Cursor;

/// Extract every role/tablespace a complete statement (see
/// `crate::map::statement`'s `statement_complete`) references —
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
/// `map::toc`'s `parse_toc_header_line` documents: this only feeds enrichment, never a
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

/// Case-insensitively find `marker` in `haystack` and parse the identifier
/// (bare, or double-quoted with `""` doubled — [`Cursor::parse_ident`]'s two
/// shapes, matching `fmtId()`'s two output forms) immediately following it.
fn ident_after(haystack: &str, marker: &str) -> Option<String> {
    let idx = find_ci(haystack, marker)?;
    let rest = haystack[idx + marker.len()..].trim_start();
    Cursor::new(rest.as_bytes()).parse_ident()
}

#[cfg(test)]
mod tests;
