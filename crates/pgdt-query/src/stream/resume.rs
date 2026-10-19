//! A consumption's cursor: the token that resumes a stream, and the stamp it
//! carries of the query that produced it (`docs/design/decisions.md`, "D49").

use pgdt_codec::SchemaMode;
use pgdt_codec::compare::ComparisonSemantics;
use pgdt_core::CopyHeader;

use crate::options::QueryOptions;
use crate::predicate::{Expr, PredicateOp};

/// Opaque cursor into a
/// [`TablePartitions::in_file_order`](crate::stream::TablePartitions::in_file_order)
/// or [`TablePartitions::resume`](crate::stream::TablePartitions::resume)
/// consumption, sufficient to resume from just past the last batch a caller
/// accepted. Valid only within the process that produced it; persisting one
/// across a restart is out of scope (`docs/design/roadmap.md`).
#[derive(Debug, Clone)]
pub struct ResumeToken {
    pub(super) offset: u64,
    pub(super) rows_emitted: u64,
    /// Stamp of the query this token came out of — see [`query_fingerprint`].
    /// Resuming a stream whose options hash differently is
    /// `Error::ResumeQueryMismatch`, which is what defends "one schema per
    /// stream, resolved up front".
    pub(super) query_fingerprint: u64,
    /// Reserved for the structural cache's generation stamp; the cache does
    /// not stamp generations, so this is always 0.
    #[allow(dead_code)]
    pub(super) generation: u64,
    pub(super) in_copy: Option<InCopyResume>,
}

#[derive(Debug, Clone)]
pub(super) struct InCopyResume {
    pub(super) header: CopyHeader,
    pub(super) header_offset: u64,
    pub(super) rows_in_block: u64,
    pub(super) database: Option<String>,
}

impl ResumeToken {
    pub(super) fn start(query_fingerprint: u64) -> Self {
        Self { offset: 0, rows_emitted: 0, query_fingerprint, generation: 0, in_copy: None }
    }
}

/// A [`ResumeToken`]'s stamp of the query that produced it: the table, the
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
                PredicateOp::Eq(_) => 0u8,
                PredicateOp::Ne(_) => 1u8,
                PredicateOp::IsNull => 2u8,
                PredicateOp::IsNotNull => 3u8,
                PredicateOp::Lt(_) => 4u8,
                PredicateOp::Le(_) => 5u8,
                PredicateOp::Gt(_) => 6u8,
                PredicateOp::Ge(_) => 7u8,
                PredicateOp::IsDistinctFrom(_) => 8u8,
                PredicateOp::IsNotDistinctFrom(_) => 9u8,
                PredicateOp::IsUnrepresentable => 10u8,
                PredicateOp::IsNotUnrepresentable => 11u8,
                PredicateOp::In(_) => 12u8,
                PredicateOp::NotIn(_) => 13u8,
            }
            .hash(hasher);
            term.op.operand().hash(hasher);
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
