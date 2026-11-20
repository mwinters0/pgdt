//! What one pass over a `COPY` block's rows records beside them, as the cache
//! persists it: the array-shape census and the unrepresentable count
//! (`docs/design/decisions.md`, "D35", "D96"). How a row is folded into them
//! is `crate::block`'s.

use serde::{Deserialize, Serialize};

/// What one column's array values look like within one `COPY` block — the
/// shape census (`docs/design/decisions.md`, "D35").
///
/// Per column because an array's dimensionality belongs to the *value*
/// (I21), not to the DDL; per block because that is what the map extends
/// incrementally. Combining blocks is min-of-mins, max-of-maxes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArrayShape {
    /// Fewest and most dimensions any value in this column carried, as
    /// `(min, max)`. `None` until a value contributes one — a SQL NULL never
    /// does, and neither does `{}`, which is what `array_out` writes for an
    /// empty array of *any* dimensionality (I25) and so fits every depth.
    /// Both bounds, never the maximum alone (`docs/design/decisions.md`,
    /// "D35").
    pub dims: Option<(u8, u8)>,
    /// Whether any value carried an explicit `[lb:ub]=` lower-bound prefix.
    /// Kept apart from `dims` because it disqualifies a column on its own:
    /// Arrow lists are 0-based, with nowhere to record an index origin.
    pub lower_bound_prefix: bool,
}

/// One column's count of values its Arrow type cannot hold in one `COPY`
/// block, each value in the one tier it is past — the format spec's
/// dominating (`docs/design/decisions.md`, "D96"). Beside the array-shape
/// census, and at the same level: a block mapped at the metadata level holds
/// neither.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unrepresentable {
    pub(crate) format: u64,
    pub(crate) engine: u64,
}

/// What one pass over a block's rows records beside its rows' extent: the
/// array-shape census and the unrepresentable count, one entry each per
/// column in `header.columns` order — the data level's two query affordances
/// (`docs/design/decisions.md`, "D35", "D96"). The census grows for a row
/// wider than its header holding a `{` or `[`, which a query refuses; the count never does, a
/// column past the header having no declared type to be tested against. A
/// column that saw no array-shaped literal holds the default shape, and both
/// are empty only for a header naming no columns.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockCensus {
    pub shapes: Vec<ArrayShape>,
    pub unrepresentable: Vec<Unrepresentable>,
}
