//! How `info` names what the cache holds — a column's resolution and Arrow
//! type, a type's kind and an enum's labels, a span — and the per-block passes
//! the listing and `--json` share: the one resolution pass, and what a strict
//! parse checks nothing of.

use arrow::datatypes::DataType;
use pgdt_codec::compare::{CompareKind, ComparisonPlan};
use pgdt_codec::pgtype::{NestedPlan, RANGE_STRUCT_FIELDS};
use pgdt_codec::{ColumnResolution, ResolvedSchema, SchemaMode, resolve_columns};
use pgdt_mapping::{StrictUnchecked, strict_unchecked};
use pgdt_model::{ArrayShape, DataBlock, DumpIndex, Span, SpanBody, TableReference, TypeKind};

/// One column's resolution outcome as the stable token `--json` prints
/// (`docs/design/decisions.md`, "The CLI"); the sentence `info --detail`
/// prints is the library's, [`ColumnResolution::describe`]. **Both matches are
/// exhaustive**, so a new [`ColumnResolution`] variant is a compile error that
/// has to answer both.
pub(super) fn resolution_token(r: &ColumnResolution) -> &'static str {
    match r {
        ColumnResolution::Mapped => "mapped",
        ColumnResolution::UnknownType => "unknown_type",
        ColumnResolution::NotDeclared => "not_declared",
        ColumnResolution::MetadataNotScanned => "metadata_not_scanned",
        ColumnResolution::OpaqueElementType => "opaque_element_type",
        ColumnResolution::NestedArrayElement => "nested_array_element",
        ColumnResolution::VaryingArrayShape => "varying_array_shape",
        ColumnResolution::OpaqueBaseType => "opaque_base_type",
        ColumnResolution::EmptyEnum => "empty_enum",
        ColumnResolution::UnrepresentableValues => "unrepresentable_values",
    }
}

/// What one column became in Arrow — the other half of `info --detail`'s
/// per-column line (`docs/design/decisions.md`, "The CLI").
///
/// Arrow's own `Display` is what prints, with one substitution: the five-field
/// range struct, identical for every range column in every dump, collapses to
/// `Range<T>`, `T` being the bound type. The manual states the struct's real
/// layout once, which makes the elision lossless.
///
/// **The substitution is detected from the [`NestedPlan`], never from the
/// field names** — a user composite may declare five fields with exactly
/// those names, and `pgtype::RANGE_STRUCT_FIELDS` reserves dispatch to the
/// plan. A built-in multirange and an array of the matching range render
/// *identically* (`List(Range<Int32>)`): the same Arrow type, different plans,
/// told apart by the declared PostgreSQL type, which `info --detail` carries
/// on its `columns:` summary line rather than on this one, and `--json`
/// beside this string.
///
/// The type and the plan are built together and cannot disagree; this
/// being display code, a disagreeing pair falls back to plain `Display`
/// rather than panicking.
pub(super) fn arrow_type_label(data_type: &DataType, plan: &NestedPlan) -> String {
    match (plan, data_type) {
        (NestedPlan::Array(element), DataType::List(field)) => {
            format!("List({})", arrow_type_label(field.data_type(), &element.plan))
        }
        (NestedPlan::Record(field_plans), DataType::Struct(fields))
            if field_plans.len() == fields.len() =>
        {
            let rendered: Vec<String> = fields
                .iter()
                .zip(field_plans)
                .map(|(f, p)| {
                    format!("{:?}: {}", f.name(), arrow_type_label(f.data_type(), &p.plan))
                })
                .collect();
            format!("Struct({})", rendered.join(", "))
        }
        (NestedPlan::Range(bound), _) => range_label(data_type, bound),
        (NestedPlan::Multirange(bound), DataType::List(field)) => {
            format!("List({})", range_label(field.data_type(), bound))
        }
        _ => data_type.to_string(),
    }
}

/// The `Range<T>` substitution itself, shared by the `Range` and `Multirange`
/// plans — the latter is a `List` of exactly this struct.
fn range_label(data_type: &DataType, bound: &NestedPlan) -> String {
    match data_type {
        DataType::Struct(fields) if fields.len() == RANGE_STRUCT_FIELDS.len() => {
            format!("Range<{}>", arrow_type_label(fields[0].data_type(), bound))
        }
        _ => data_type.to_string(),
    }
}

/// An enum column's declared labels, in declaration order, or `None` for
/// every other column — read off the column's own [`ComparisonPlan`], where
/// resolution already put them (`docs/design/decisions.md`, "The CLI").
///
/// A domain over an enum answers here too, `compare::walk::comparison_user_type`
/// recursing through the domain chain. An *empty* enum is
/// `ComparisonPlan::Refused` and has nothing to list, matching the
/// `empty enum` sentence the line above prints.
pub(super) fn enum_labels(plan: &ComparisonPlan) -> Option<&[String]> {
    match plan {
        ComparisonPlan::Compared { kind: CompareKind::Enum { labels, .. }, .. } => Some(labels),
        _ => None,
    }
}

/// The labels as `info --detail` prints them: each one single-quoted with any
/// interior quote doubled, comma-separated.
///
/// **Quoting is forced by the data**: a label is arbitrary text (`has space`,
/// `has,comma`, `has'quote` are all legal and all in the fixtures), so a bare
/// comma-joined list cannot be read back apart. Single quotes with `''`
/// doubling is both what the dump's own `CREATE TYPE … AS ENUM (…)` writes
/// and what a `--filter` value accepts (the term grammar's `dequote`).
///
/// *Rejected:* Rust's `{:?}`, which `arrow_type_label` uses for a composite's
/// field names; it spells a PostgreSQL literal in Rust's escape vocabulary,
/// and the double quote it produces is the *other* quote here.
pub(super) fn label_list(labels: &[String]) -> String {
    labels.iter().map(|l| format!("'{}'", l.replace('\'', "''"))).collect::<Vec<_>>().join(", ")
}

/// One user-defined type's kind, rendered with whatever payload that kind
/// carries — the enum's labels, the domain's base type and `COLLATE` clause,
/// the composite's fields, the range's subtype
/// (`docs/design/decisions.md`, "The CLI").
///
/// **Every arm renders**, not the enum alone. `Composite { fields: None }`
/// says `(fields not parsed)` explicitly, being the one arm whose absence
/// changes how a column of the type resolves; a `Range` naming no subtype
/// says so for symmetry. Neither shape is one `pg_dump` writes, so both are
/// pinned by this module's unit test. [`type_kind_label`] is `--map`'s
/// one-word name for the same vocabulary.
pub(super) fn type_kind_summary(kind: &TypeKind) -> String {
    match kind {
        TypeKind::Enum { labels, exact: true } if labels.is_empty() => {
            "enum: (no labels)".to_string()
        }
        TypeKind::Enum { labels, exact: false } if labels.is_empty() => {
            "enum: (labels not read)".to_string()
        }
        TypeKind::Enum { labels, exact: true } => format!("enum: {}", label_list(labels)),
        TypeKind::Enum { labels, exact: false } => {
            format!("enum: {} (labels not read exactly)", label_list(labels))
        }
        TypeKind::Domain { base_type, collation, not_null, check } => {
            let collation = collation.as_ref().map(|c| format!(" COLLATE {c}")).unwrap_or_default();
            let not_null = if *not_null { " NOT NULL" } else { "" };
            let check = if *check { " CHECK" } else { "" };
            format!("domain over {base_type}{collation}{not_null}{check}")
        }
        TypeKind::Composite { fields: None } => "composite: (fields not parsed)".to_string(),
        TypeKind::Composite { fields: Some(fields) } if fields.is_empty() => {
            "composite: (no fields)".to_string()
        }
        TypeKind::Composite { fields: Some(fields) } => {
            let rendered: Vec<String> = fields
                .iter()
                .map(|f| match &f.collation {
                    Some(c) => format!("{} {} COLLATE {c}", f.name, f.declared_type),
                    None => format!("{} {}", f.name, f.declared_type),
                })
                .collect();
            format!("composite: {}", rendered.join(", "))
        }
        // The `canonical` function is named where the DDL declares one: it
        // is why a column of this type refuses every filter operator.
        TypeKind::Range { subtype, canonical, .. } => {
            let over = match subtype {
                Some(subtype) => format!("range over {subtype}"),
                None => "range (subtype not parsed)".to_string(),
            };
            match canonical {
                Some(function) => format!("{over}, canonical {function}"),
                None => over,
            }
        }
        TypeKind::Base { .. } => "base type".to_string(),
        TypeKind::Shell => "shell type".to_string(),
    }
}

/// One `COPY` block's resolved schema, paired back with the block it came
/// from — the single resolution pass `--detail`'s text and `--json`'s export
/// both render (`docs/design/decisions.md`, "The CLI").
///
/// `complete` says whether `index` covers the file
/// ([`DumpIndex::is_complete`]), which decides whether a block's array-shape
/// census may be believed: a *mapped* block's census is total for that block,
/// but one table's data can occupy several blocks (I2), so a map that stopped
/// short cannot speak for a block past its frontier and every column resolves
/// optimistically until it can (`docs/design/decisions.md`, "D35"). A block
/// mapped at the metadata level holds no census, and resolves optimistically
/// too.
///
/// A header-less block copies no columns (I5) and resolves to an empty
/// schema. It is still listed, so the export's shape does not vary per
/// block.
pub(super) fn block_resolutions(
    index: &DumpIndex,
    complete: bool,
) -> Vec<(Option<&str>, &pgdt_model::CopyBlock, ResolvedSchema)> {
    index
        .blocks_with_database()
        .map(|(database, block)| {
            let census: &[ArrayShape] = match block.array_shapes() {
                Some(census) if complete => census,
                _ => &[],
            };
            let resolved = resolve_columns(
                &block.header.qualified_name(),
                &block.header.columns,
                index.metadata.as_deref(),
                database,
                SchemaMode::Typed,
                census,
            );
            (database, block, resolved)
        })
        .collect()
}

/// What a strict parse checks nothing of in `block`, in `database`, read off the cache's
/// preamble and the block's own framing (`pgdt_mapping::strict_unchecked`):
/// a property of the dump, so the same whichever mode built the cache.
pub(super) fn block_unchecked(
    index: &DumpIndex,
    database: Option<&str>,
    block: &pgdt_model::CopyBlock,
) -> StrictUnchecked {
    strict_unchecked(block, index.metadata.as_deref(), database)
}

/// One-line label for a span in `--map` output.
pub(super) fn span_summary(span: &Span) -> String {
    match &span.body {
        SpanBody::Data(DataBlock::Copy(block)) => {
            format!("COPY {} ({} rows)", block.header.qualified_name(), block.row_count)
        }
        SpanBody::Data(DataBlock::InsertRun(run)) => {
            format!("INSERT run: {} ({} statements)", run.table, run.row_count)
        }
        SpanBody::Data(DataBlock::LargeObjects(_)) => "large objects".to_string(),
        SpanBody::Table { name, .. } => format!("TABLE {name}"),
        SpanBody::TypeDef { name, kind } => format!("TYPE {name} ({})", type_kind_label(kind)),
        SpanBody::Extension { name, .. } => format!("EXTENSION {name}"),
        SpanBody::Collation { collation } => {
            let determinism = if collation.deterministic { "" } else { " (deterministic = false)" };
            format!("COLLATION {}{determinism}", collation.name)
        }
        SpanBody::Connect { database } => format!("\\connect {database}"),
        SpanBody::VersionHeader { .. } => "version header".to_string(),
        SpanBody::AlterTypeAddValue { type_name, label } => {
            format!("ALTER TYPE {type_name} ADD VALUE {label:?}")
        }
        SpanBody::AlterTypeDropAttribute { type_name, attribute } => {
            format!("ALTER TYPE {type_name} DROP ATTRIBUTE {attribute:?}")
        }
        SpanBody::EnumLabelsUnread { type_name: Some(type_name) } => {
            format!("ALTER TYPE {type_name} (enum labels not read)")
        }
        SpanBody::EnumLabelsUnread { type_name: None } => {
            "ALTER TYPE (enum labels not read)".to_string()
        }
        SpanBody::AlterTableReference { table, reference } => match reference {
            TableReference::Parent(parent) => format!("ALTER TABLE {table} INHERIT {parent}"),
            TableReference::OfType(of_type) => format!("ALTER TABLE {table} OF {of_type}"),
            TableReference::NotNull(column) => {
                format!("ALTER TABLE {table} ALTER COLUMN {column} SET NOT NULL")
            }
            TableReference::Check(check) => match &check.name {
                Some(name) => format!("ALTER TABLE {table} ADD CONSTRAINT {name} CHECK"),
                None => format!("ALTER TABLE {table} ADD CHECK"),
            },
            TableReference::PartitionOf(of) => format!(
                "ALTER TABLE {} ATTACH PARTITION {table} {}",
                of.parent,
                of.bound.as_deref().unwrap_or("(bound not read)")
            ),
        },
        SpanBody::Framing => "framing".to_string(),
        SpanBody::Unparsed => match &span.toc {
            Some(toc) => format!("{} {}", toc.kind, toc.name),
            None => "unparsed".to_string(),
        },
        SpanBody::Unscanned => "unscanned".to_string(),
    }
}

/// Short label for a [`TypeKind`] — `--map`'s compact form of the vocabulary
/// `docs/manual/type-handling.md` explains. [`type_kind_summary`] is the
/// `--detail` listing's fuller rendering, payload included.
fn type_kind_label(kind: &TypeKind) -> &'static str {
    match kind {
        TypeKind::Enum { .. } => "enum",
        TypeKind::Domain { .. } => "domain",
        TypeKind::Composite { .. } => "composite",
        TypeKind::Range { .. } => "range",
        TypeKind::Base { .. } => "base",
        TypeKind::Shell => "shell",
    }
}

#[cfg(test)]
mod tests;
