//! What a position diverges in from PostgreSQL's comparison when DataFusion
//! compares it: not the register's API but what a crate above calls, so
//! `internal` re-exports it.

use super::{CompareKind, ComparisonDivergence};

/// What a position of `kind` diverges in under
/// [`ComparisonSemantics::DataFusion`](super::ComparisonSemantics::DataFusion),
/// given the register's own `divergence` for it:
/// [`CompareKind::datafusion_divergence`], then the register's, but for
/// `jsonb`'s string collation, the whole value being compared as text there.
pub fn datafusion_position_divergences(
    kind: &CompareKind,
    divergence: Option<ComparisonDivergence>,
) -> Vec<ComparisonDivergence> {
    kind.datafusion_divergence()
        .into_iter()
        .chain(divergence.filter(|d| *d != ComparisonDivergence::JsonbStringCollation))
        .collect()
}
