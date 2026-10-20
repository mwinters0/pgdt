//! `--filter` and `--where`: the term grammar, the expression over it, and
//! `--filter`'s entry, which refuses what `--where` would read as structure.

use anyhow::Result;
use pgdt_query::Expr;

pub(crate) use expr::parse_where;
use expr::refuse_where_structure;
use term::parse_filter;

mod expr;
mod term;

/// One `--filter` argument: the term grammar below, and before it
/// [`expr::refuse_where_structure`]. That runs first, so a term both
/// structural and malformed earns the structural message —
/// `--filter 'and is null'` is told that `AND` is a reserved spelling.
pub(super) fn parse_filter_flag(spec: &str) -> Result<Expr> {
    // deficiency: KD127 — `spec` reaches `parse_filter` untrimmed, where the
    // tokenizer asked `filter_op_at` from its first non-blank byte, so a term
    // opening with whitespace before `IS [NOT] DISTINCT FROM` passes the
    // refusal as one leaf and then splits at that operator, at a boundary the
    // tokenizer did not see: the empty column, which no table holds, where
    // `--where` refuses the same term as malformed.
    refuse_where_structure(spec)?;
    parse_filter(spec)
}

#[cfg(test)]
mod tests;
