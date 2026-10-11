//! A block's state while a query reads it, the query's projection resolved
//! against one block's own column order — its filter is resolved by
//! [`crate::predicate::resolve_expr`] — and the stamp a resume token carries
//! of the query that produced it.

use std::sync::Arc;

use arrow::datatypes::Schema;

use pgdt_codec::compare::ComparisonSemantics;
use pgdt_codec::{ResolvedSchema, SchemaMode};
use pgdt_core::CopyHeader;
use pgdt_core::{Error, Result};
use pgdt_dump::Row;

use crate::batch::{QueryOptions, RowBatcher};
use crate::predicate::{ComparisonNote, Expr, PredicateOp, ResolvedExpr};

/// State for a `COPY` block whose table matches the query: the batcher
/// accumulating its rows, `QueryOptions::filter` resolved against this block's
/// own schema (a table's blocks can order its columns differently), and the database this block
/// is attributed to. Each
/// [`ResolvedExpr`] leaf carries the field index it reads — into the block's
/// **unprojected** column list — plus the typed comparison it makes. Shared
/// with the block's [`PlannedBlock`](super::plan::PlannedBlock), so every piece of a block evaluates one
/// tree.
pub(super) type Active = (u64, CopyHeader, RowBatcher, Arc<ResolvedExpr>, Option<String>);

/// What [`activate`](super::segment::activate) hands back: the block's state, and the schema and notes
/// a stream publishes for it.
pub(super) type Opened = (Active, ResolvedSchema, Vec<ComparisonNote>);

/// Where `row` sits inside `prefix`, the UTF-8-validated leading part of the
/// span it was scanned out of — `None` when the row runs past it, which is
/// every row of a span whose validation failed and none of one whose
/// validation held. `str::get` rather than an index, so a mis-derived offset
/// costs the row its fast path instead of panicking.
pub(super) fn row_text<'a>(prefix: &'a str, span_base: u64, row: &Row<'_>) -> Option<&'a str> {
    let start = usize::try_from(row.offset.checked_sub(span_base)?).ok()?;
    prefix.get(start..start.checked_add(row.raw.len())?)
}

/// `database`, rendered as `database.schema.table`, or just `schema.table`
/// when the file had no `\connect` at all — the form `Error::AmbiguousTable`
/// names its candidates in.
pub(super) fn render_candidate((database, qualified_name): &(Option<String>, String)) -> String {
    match database {
        Some(db) => format!("{db}.{qualified_name}"),
        None => qualified_name.clone(),
    }
}

/// Cut `resolved` down to `projection`, and say which of the block's fields
/// each projected column is fed by. All five of [`ResolvedSchema`]'s vectors
/// are cut together, in the requested order
/// (`docs/design/decisions.md`, "D28").
///
/// Returns the projected schema and `field_targets` — one entry per field of
/// the block, `Some(i)` when that field feeds projected column `i`. `None`
/// projection is every column, in file order. Duplicate names are the caller's
/// to reject: this resolves each requested name independently and would
/// silently accept one twice.
pub(super) fn project(
    resolved: &ResolvedSchema,
    projection: Option<&[String]>,
    header_offset: u64,
) -> Result<(ResolvedSchema, Vec<Option<usize>>)> {
    let width = resolved.schema.fields().len();
    let Some(names) = projection else {
        return Ok((resolved.clone(), (0..width).map(Some).collect()));
    };
    let mut field_targets = vec![None; width];
    let mut sources = Vec::with_capacity(names.len());
    for name in names {
        let source =
            resolved.schema.fields().iter().position(|f| f.name() == name).ok_or_else(|| {
                Error::UnknownProjectionColumn { header_offset, column: name.clone() }
            })?;
        field_targets[source] = Some(sources.len());
        sources.push(source);
    }
    let projected = ResolvedSchema {
        schema: Arc::new(Schema::new(
            sources.iter().map(|&i| resolved.schema.field(i).clone()).collect::<Vec<_>>(),
        )),
        columns: sources.iter().map(|&i| resolved.columns[i].clone()).collect(),
        notes: sources.iter().map(|&i| resolved.notes[i].clone()).collect(),
        plans: sources.iter().map(|&i| resolved.plans[i].clone()).collect(),
        comparisons: sources.iter().map(|&i| resolved.comparisons[i].clone()).collect(),
        not_null: sources.iter().map(|&i| resolved.not_null[i]).collect(),
    };
    Ok((projected, field_targets))
}

/// A [`ResumeToken`](super::table::ResumeToken)'s stamp of the query that produced it: the table, the
/// projection, the filter terms and their semantics, the schema mode, how a
/// value its column cannot hold is read and — for a sub-stream of a
/// partitioned replay — which partition of how many it came out of
/// (`docs/design/decisions.md`, "D49"). The hasher's output is not stable
/// across Rust releases: a token is valid only within its own process.
///
/// **`partition` is what keeps a sub-stream's token from resuming as a whole
/// one.** A token carries an offset and nothing about the range its stream was
/// confined to, so stamping the partition turns what would be a silent
/// superset into `Error::ResumeQueryMismatch`
/// (`docs/design/decisions.md`, "D51").
pub(super) fn query_fingerprint(
    table: &str,
    options: &QueryOptions,
    partition: Option<(usize, usize)>,
) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    table.hash(&mut hasher);
    match partition {
        None => 0u8.hash(&mut hasher),
        Some((index, of)) => {
            1u8.hash(&mut hasher);
            index.hash(&mut hasher);
            of.hash(&mut hasher);
        }
    }
    match options.schema_mode {
        SchemaMode::Typed => 0u8,
        SchemaMode::Strings => 1u8,
    }
    .hash(&mut hasher);
    match &options.projection {
        None => 0u8.hash(&mut hasher),
        Some(columns) => {
            1u8.hash(&mut hasher);
            columns.hash(&mut hasher);
        }
    }
    hash_expr(&options.filter, &mut hasher);
    match options.semantics {
        ComparisonSemantics::Postgres => 0u8,
        ComparisonSemantics::DataFusion => 1u8,
    }
    .hash(&mut hasher);
    options.unrepresentable.hash(&mut hasher);
    hasher.finish()
}

/// Fold one filter expression into `hasher`, node kind first, then arity,
/// then each child in order — so two trees of different shape cannot collide
/// by carrying the same terms. Two conjunctions differing only in term order
/// fingerprint differently; canonicalizing instead would make the stamp depend
/// on an ordering rule of its own.
fn hash_expr<H: std::hash::Hasher>(expr: &Expr, hasher: &mut H) {
    use std::hash::Hash;

    match expr {
        Expr::Term(term) => {
            0u8.hash(hasher);
            term.column.hash(hasher);
            match term.op {
                PredicateOp::Eq => 0u8,
                PredicateOp::Ne => 1u8,
                PredicateOp::IsNull => 2u8,
                PredicateOp::IsNotNull => 3u8,
                PredicateOp::Lt => 4u8,
                PredicateOp::Le => 5u8,
                PredicateOp::Gt => 6u8,
                PredicateOp::Ge => 7u8,
                PredicateOp::IsDistinctFrom => 8u8,
                PredicateOp::IsNotDistinctFrom => 9u8,
                PredicateOp::IsUnrepresentable => 10u8,
                PredicateOp::IsNotUnrepresentable => 11u8,
            }
            .hash(hasher);
            term.value.hash(hasher);
        }
        Expr::In(membership) => {
            4u8.hash(hasher);
            membership.column.hash(hasher);
            membership.values.hash(hasher);
        }
        Expr::And(children) => {
            1u8.hash(hasher);
            hash_children(children, hasher);
        }
        Expr::Or(children) => {
            2u8.hash(hasher);
            hash_children(children, hasher);
        }
        Expr::Not(inner) => {
            3u8.hash(hasher);
            hash_expr(inner, hasher);
        }
    }
}

/// Arity, then each child in order.
fn hash_children<H: std::hash::Hasher>(children: &[Expr], hasher: &mut H) {
    use std::hash::Hash;

    children.len().hash(hasher);
    for child in children {
        hash_expr(child, hasher);
    }
}
