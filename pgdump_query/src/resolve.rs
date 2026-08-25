//! `ResolvedSchema`: the join of a `COPY` header's column list against
//! `DumpMetadata` (`docs/design/architecture.md`, "Joining a header against
//! the metadata").
//!
//! This is a *preview*, not what decodes a row — `decode.rs` does that. The real `RecordBatch` schema built in `batch.rs`
//! stays every-column-`Utf8View` regardless of what a [`ResolvedSchema`]
//! reports here (its `RowBatcher` only ever builds `StringViewArray`s, and
//! `RecordBatch::try_new` would reject a schema/array type mismatch if that
//! changed without the decoder to back it) — so a caller sees the *target*
//! typing this build already understands, ahead of the code that would
//! apply it.

use std::sync::Arc;

use arrow::datatypes::{Field, Schema, SchemaRef};

use crate::diagnostic::Severity;
use crate::pgtype::{DeferredKind, TypeOutcome, resolve_declared_type};
use crate::preamble::DumpMetadata;

/// Whether a query resolves column types at all. See "Output model" in the
/// phase doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SchemaMode {
    /// Each column gets the narrowest Arrow type this build's mapping table
    /// covers for its declared PostgreSQL type; anything it doesn't cover
    /// stays `Utf8View` with a diagnostic naming why.
    #[default]
    Typed,
    /// Every column is `Utf8View`, byte-for-byte the untyped path, and
    /// no DDL lookup is attempted — the escape hatch for a caller who
    /// distrusts the mapping or wants a schema stable across releases.
    Strings,
}

/// Per-column outcome of resolving one declared type against this build's
/// mapping table — see "Failure and diagnostics" in the phase doc. Distinct
/// from [`crate::pgtype::TypeOutcome`]: this adds the "no DDL explained this
/// column at all" case, which is a join-against-`DumpMetadata` question, not
/// a type-mapping one.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnResolution {
    Mapped,
    /// Declared, but this build's mapping table has nothing for it.
    UnknownType,
    /// No DDL explained this column — `--data-only`, a typed table (`CREATE
    /// TABLE x OF t`), or `SchemaMode::Strings` (which never looks).
    NotDeclared,
    /// Decodable in principle; deferred until the nested-quoting decoder exists.
    Deferred {
        kind: DeferredKind,
    },
    /// A C-level base type or a shell/undefined type.
    OpaqueBaseType,
    EmptyEnum,
}

/// One column's full resolution, named and carrying the raw declared type
/// string (if any DDL named one) alongside the outcome — what a human-facing
/// display (`pgdq info`) needs in one place, for every column, not just the
/// unmapped ones.
///
/// **A note, not a diagnostic.** There is exactly one of these per column,
/// always, and the ordinary case is a column that resolved cleanly — so this
/// is a per-column record, not an exception report. The file-level exception
/// channel is [`crate::diagnostic::Diagnostic`], and the two deliberately
/// stay separate types: `DumpIndex` is L1 while [`ColumnResolution`] is an L2
/// conclusion about PostgreSQL type semantics, so one enum spanning both
/// would have L1 name an L2 type
/// (`docs/design/architecture.md`, "Diagnostics: one severity scale, two types"). What they share is the
/// [`Severity`] scale, so a caller reading both filters uniformly.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnNote {
    pub column: String,
    pub declared: Option<String>,
    pub resolution: ColumnResolution,
}

impl ColumnNote {
    /// Where this column sits on the shared [`Severity`] scale: a column that
    /// resolved to a real Arrow type is `Info`; anything that fell back to
    /// `Utf8View` is a `Warning`, since its values come back unparsed and a
    /// caller may want to know which columns those were.
    ///
    /// Derived rather than stored, for the reason
    /// [`crate::preamble::DumpMetadata`] is a view over spans and
    /// [`crate::index::DumpIndex::blocks`] is a filter rather than a field:
    /// it is a pure function of `resolution`, and a stored copy could
    /// disagree with it. A note whose severity is *not* derivable earns a
    /// field when one exists.
    pub fn severity(&self) -> Severity {
        match self.resolution {
            ColumnResolution::Mapped => Severity::Info,
            _ => Severity::Warning,
        }
    }
}

/// A table query's resolved schema: the Arrow schema a fully-typed decoder
/// would eventually produce (see the module docs for why that's not yet what
/// `RecordBatch`es actually carry), plus one [`ColumnNote`] per column.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedSchema {
    pub schema: SchemaRef,
    /// Positional, parallel to `schema.fields()`.
    pub columns: Vec<ColumnResolution>,
    /// Named, one entry per column, in the same order.
    pub notes: Vec<ColumnNote>,
}

impl Default for ResolvedSchema {
    /// The empty schema — what a caller sees before a matching block has
    /// been found (or if the table never appears at all).
    fn default() -> Self {
        Self { schema: Arc::new(Schema::empty()), columns: Vec::new(), notes: Vec::new() }
    }
}

impl ResolvedSchema {
    /// How many columns have no `Mapped` resolution — the count `pgdq info`'s
    /// default summary line reports (`"N of M columns unmapped"`).
    pub fn unmapped_count(&self) -> usize {
        self.columns.iter().filter(|c| **c != ColumnResolution::Mapped).count()
    }
}

/// Find the database named `database` in `metadata.databases` — an exact
/// match on [`crate::preamble::DatabaseMetadata::name`], never a guess.
/// `database: None` matches the single unnamed database a plain (non-`\connect`)
/// dump produces.
///
/// This used to be "the first database whose DDL mentions `qualified_table`"
/// — a guess, since nothing tracked which database a `CopyBlock` actually
/// belonged to. Per-block attribution (`docs/design/architecture.md`,
/// "One target per query") turns it into a fact: the caller already knows,
/// from the block it matched, which database's DDL applies.
fn database_for_name<'a>(
    metadata: &'a DumpMetadata,
    database: Option<&str>,
) -> Option<&'a crate::preamble::DatabaseMetadata> {
    metadata.databases.iter().find(|db| db.name.as_deref() == database)
}

/// Resolve `columns` (in `COPY`-header order — placeholder names like
/// `column1` when the header carried none, same as `crate::batch::schema_for`
/// derives) against `metadata` for `qualified_table`, scoped to the
/// database this block was attributed to (`database` — `None` for a plain
/// dump's single unnamed database).
///
/// `SchemaMode::Strings` never looks anything up: every column comes back
/// `NotDeclared`/`Utf8View`, matching the untyped path exactly and at zero cost.
pub fn resolve_columns(
    qualified_table: &str,
    columns: &[String],
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    mode: SchemaMode,
) -> ResolvedSchema {
    let db = match mode {
        SchemaMode::Strings => None,
        SchemaMode::Typed => metadata.and_then(|m| database_for_name(m, database)),
    };
    let declared_cols = db.and_then(|d| d.tables.get(qualified_table));

    let mut fields = Vec::with_capacity(columns.len());
    let mut resolutions = Vec::with_capacity(columns.len());
    let mut notes = Vec::with_capacity(columns.len());

    for name in columns {
        let declared = declared_cols.and_then(|cols| cols.iter().find(|(n, _)| n == name));
        let (resolution, arrow_type) = match declared {
            None => (ColumnResolution::NotDeclared, arrow::datatypes::DataType::Utf8View),
            Some((_, ty)) => {
                // `db` is always `Some` here: `declared_cols` only came from
                // `db.tables`, so `db.types` is the right list to resolve
                // this same database's `CREATE TYPE`/`DOMAIN` references
                // against.
                match resolve_declared_type(ty, &db.unwrap().types) {
                    TypeOutcome::Mapped(dt) => (ColumnResolution::Mapped, dt),
                    TypeOutcome::Unknown => {
                        (ColumnResolution::UnknownType, arrow::datatypes::DataType::Utf8View)
                    }
                    TypeOutcome::Deferred(kind) => {
                        (ColumnResolution::Deferred { kind }, arrow::datatypes::DataType::Utf8View)
                    }
                    TypeOutcome::OpaqueBaseType => {
                        (ColumnResolution::OpaqueBaseType, arrow::datatypes::DataType::Utf8View)
                    }
                    TypeOutcome::EmptyEnum => {
                        (ColumnResolution::EmptyEnum, arrow::datatypes::DataType::Utf8View)
                    }
                }
            }
        };
        // Every Arrow field is nullable, regardless of a `NOT
        // NULL` in the DDL -- see "Nullability" in the phase doc.
        fields.push(Field::new(name, arrow_type, true));
        notes.push(ColumnNote {
            column: name.clone(),
            declared: declared.map(|(_, ty)| ty.clone()),
            resolution: resolution.clone(),
        });
        resolutions.push(resolution);
    }

    ResolvedSchema { schema: Arc::new(Schema::new(fields)), columns: resolutions, notes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preamble::{DatabaseMetadata, TypeDef, TypeKind};

    fn one_db(tables: &[(&str, &[(&str, &str)])], types: Vec<TypeDef>) -> DumpMetadata {
        let mut db = DatabaseMetadata {
            name: None,
            preamble_complete: true,
            server_version: None,
            pg_dump_version: None,
            extensions: Vec::new(),
            types,
            tables: Default::default(),
        };
        for (name, cols) in tables {
            db.tables.insert(
                name.to_string(),
                cols.iter().map(|(c, t)| (c.to_string(), t.to_string())).collect(),
            );
        }
        DumpMetadata { databases: vec![db] }
    }

    #[test]
    fn mapped_and_not_declared_columns() {
        let meta = one_db(&[("public.t", &[("id", "integer"), ("name", "text")])], vec![]);
        let cols = vec!["id".to_string(), "name".to_string(), "extra".to_string()];
        let resolved = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed);
        assert_eq!(
            resolved.columns,
            [ColumnResolution::Mapped, ColumnResolution::Mapped, ColumnResolution::NotDeclared,]
        );
        assert_eq!(resolved.schema.field(0).data_type(), &arrow::datatypes::DataType::Int32);
        assert_eq!(resolved.schema.field(1).data_type(), &arrow::datatypes::DataType::Utf8View);
        assert_eq!(resolved.schema.field(2).data_type(), &arrow::datatypes::DataType::Utf8View);
        assert!(resolved.schema.fields().iter().all(|f| f.is_nullable()));
        assert_eq!(resolved.unmapped_count(), 1);
    }

    #[test]
    fn strings_mode_never_looks_up_ddl() {
        let meta = one_db(&[("public.t", &[("id", "integer")])], vec![]);
        let cols = vec!["id".to_string()];
        let resolved = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Strings);
        assert_eq!(resolved.columns, [ColumnResolution::NotDeclared]);
        assert_eq!(resolved.schema.field(0).data_type(), &arrow::datatypes::DataType::Utf8View);
    }

    #[test]
    fn no_metadata_at_all_is_every_column_not_declared() {
        let cols = vec!["id".to_string()];
        let resolved = resolve_columns("public.t", &cols, None, None, SchemaMode::Typed);
        assert_eq!(resolved.columns, [ColumnResolution::NotDeclared]);
    }

    #[test]
    fn unknown_deferred_opaque_and_empty_enum_outcomes() {
        let types = vec![
            TypeDef { name: "public.gtype".to_string(), kind: TypeKind::Base },
            TypeDef { name: "public.mood".to_string(), kind: TypeKind::Enum { labels: vec![] } },
        ];
        let meta = one_db(
            &[(
                "public.t",
                &[("a", "money"), ("b", "integer[]"), ("c", "public.gtype"), ("d", "public.mood")],
            )],
            types,
        );
        let cols = vec!["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string()];
        let resolved = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed);
        assert_eq!(
            resolved.columns,
            [
                ColumnResolution::UnknownType,
                ColumnResolution::Deferred { kind: DeferredKind::Array },
                ColumnResolution::OpaqueBaseType,
                ColumnResolution::EmptyEnum,
            ]
        );
    }

    #[test]
    fn diagnostic_carries_the_declared_string_when_present() {
        let meta = one_db(&[("public.t", &[("id", "integer")])], vec![]);
        let cols = vec!["id".to_string(), "extra".to_string()];
        let resolved = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed);
        assert_eq!(resolved.notes[0].declared.as_deref(), Some("integer"));
        assert_eq!(resolved.notes[1].declared, None);
    }

    /// `database_for_name` selects by the attributed database's *name*, not
    /// by which database's DDL happens to mention the table first — proven
    /// here by giving the two databases genuinely different declared types
    /// for the same qualified table name, so the outcome differs observably
    /// depending on which name is passed. This is what the one-target-per-query
    /// rule
    /// (`docs/design/architecture.md`, "One target per query") turned the old first-match guess into: the caller already
    /// knows, from the matched `CopyBlock`'s own attribution, which database
    /// applies — see `docs/design/architecture.md`,
    /// "One target per query", for the guess this test used to pin down.
    #[test]
    fn database_selects_by_attributed_name_not_by_first_match() {
        let mut a = one_db(&[("public.t", &[("id", "text")])], vec![]).databases.remove(0);
        a.name = Some("a".to_string());
        let mut b = one_db(&[("public.t", &[("id", "integer")])], vec![]).databases.remove(0);
        b.name = Some("b".to_string());
        let meta = DumpMetadata { databases: vec![a, b] };
        let cols = vec!["id".to_string()];

        let resolved_a =
            resolve_columns("public.t", &cols, Some(&meta), Some("a"), SchemaMode::Typed);
        assert_eq!(resolved_a.schema.field(0).data_type(), &arrow::datatypes::DataType::Utf8View);

        let resolved_b =
            resolve_columns("public.t", &cols, Some(&meta), Some("b"), SchemaMode::Typed);
        assert_eq!(resolved_b.schema.field(0).data_type(), &arrow::datatypes::DataType::Int32);
    }

    /// A [`ColumnNote`]'s severity is derived from its resolution, never
    /// stored — so it cannot drift out of agreement with the outcome it
    /// describes, the same reason `DumpMetadata` is a view over spans.
    #[test]
    fn column_note_severity_follows_its_resolution() {
        let note = |resolution| ColumnNote { column: "c".to_string(), declared: None, resolution };
        assert_eq!(note(ColumnResolution::Mapped).severity(), Severity::Info);
        for fell_back in [
            ColumnResolution::UnknownType,
            ColumnResolution::NotDeclared,
            ColumnResolution::OpaqueBaseType,
            ColumnResolution::EmptyEnum,
        ] {
            assert_eq!(note(fell_back.clone()).severity(), Severity::Warning, "{fell_back:?}");
        }
    }

    /// The two channels share one scale, which is the whole point of keeping
    /// them separate types: a caller can filter both by the same threshold
    /// without knowing which produced what.
    #[test]
    fn both_channels_order_on_one_severity_scale() {
        use crate::diagnostic::{Diagnostic, DiagnosticKind};

        let file = Diagnostic::tiling_broken(vec![crate::map::TilingIssue::Empty { index: 0 }]);
        let column = ColumnNote {
            column: "c".to_string(),
            declared: None,
            resolution: ColumnResolution::UnknownType,
        };
        assert!(file.severity > column.severity());
        assert!(matches!(file.kind, DiagnosticKind::TilingBroken { .. }));
    }
}
