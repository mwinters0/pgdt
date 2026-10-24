//! [`DumpMetadata`] derived from a finished map's spans, never a second scan
//! (`docs/design/decisions.md`, "D34"): one segment per database, the
//! version headers staged across a `\connect`, and the `ALTER` statements
//! folded into the definitions they amend.

use pgdt_model::internal::{DatabaseMetadataExt, TableDefExt};
use pgdt_model::{DatabaseMetadata, DumpMetadata, Extension, Span, SpanBody, TypeDef, TypeKind};

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
/// block — `pgdt_mapping::build_map` and `crate::preamble::scan_preamble`'s own
/// span builder both only ever produce spans this way. A span list with an
/// `Unscanned` tail cut off anywhere else (e.g. a cold query's pass,
/// `pgdt_query::Table::settle`, at its stop) would make the trailing database's `preamble_complete` a lie.
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
            // stay two, as `target_settled` expects — though
            // `database_for_name` answers the first for both (`KD133`).
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

/// Mark a database's segment as read to completion — see
/// [`DatabaseMetadata::preamble_complete`]'s docs for what that means and who
/// relies on it. Shared by every place [`dump_metadata_from_spans`] closes
/// out a segment: a `\connect` boundary, and end of input.
fn finalize(mut db: DatabaseMetadata) -> DatabaseMetadata {
    db.preamble_complete = true;
    db
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
/// [`parse_alter_type_add_value_body`](crate::ddl::parse_alter_type_add_value_body))
/// into the matching `TypeDef` in `types`, if any — a no-op if the type it
/// names isn't an `Enum`. Used by [`dump_metadata_from_spans`], which
/// encounters the label as its own [`pgdt_model::SpanBody::AlterTypeAddValue`]
/// span, separate from the [`pgdt_model::SpanBody::TypeDef`] span it targets.
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

/// Fold an `ALTER TYPE` that could change an enum's labels and is not read (see
/// [`alter_type_labels_unread`](crate::ddl::alter_type_labels_unread)) into
/// `types`: the enum it names keeps its labels, which are no longer known to be
/// all of them. Where it names no definition here — or none this grammar reads
/// — the type it changes could be any enum declared so far, so every one is
/// left inexact.
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
/// (see
/// [`parse_alter_type_drop_attribute_body`](crate::ddl::parse_alter_type_drop_attribute_body))
/// into the matching composite in `types`, removing the field so named — a
/// no-op where the type, or the field, is not there. The field is the
/// placeholder `--binary-upgrade` writes for an attribute the type has dropped,
/// which `record_out` skips, so a composite that kept it would expect a value
/// more than every row holds.
fn fold_alter_type_drop_attribute(types: &mut [TypeDef], type_name: &str, attribute: &str) {
    if let Some(TypeDef { kind: TypeKind::Composite { fields: Some(fields) }, .. }) =
        types.iter_mut().find(|t| t.name == type_name)
    {
        fields.retain(|field| field.name != attribute);
    }
}

#[cfg(test)]
mod tests;
