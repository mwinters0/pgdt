//! The gzip adapter, holding only what touches `gz_seek`: so far the
//! plaintext spacing this crate hands it, its codec not yet built.

use std::num::NonZeroU64;

/// The plaintext spacing a forward pass places a checkpoint at the first
/// deflate block boundary at or after each multiple of, and BGZF members are
/// coalesced into units of at least: **the block `xz -T0` writes at its
/// default preset**, so a gzip unit is an ordinary `.xz` block's size for the
/// block cache, the partition grain and the resident bounds read off `.xz`
/// (`docs/design/decisions.md`, "D113").
///
/// A shipped constant, never derived from the file: a first pass chooses it
/// before it knows how long the file is.
#[cfg_attr(not(test), expect(dead_code, reason = "no codec passes it to `gz_seek` yet"))]
pub(super) const CHECKPOINT_SPACING: NonZeroU64 = NonZeroU64::new(24 << 20).unwrap();

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::DEFAULT_MEMORY_BUDGET;
    use crate::source::compressed::blocks::BlockCache;

    /// **A unit of the spacing is declined whole-block decode under the
    /// default budget, as an ordinary `.xz` block is** (`docs/design/
    /// decisions.md`, "D3", "D113"): a library caller stating no budget
    /// reads a single member's units piecewise. Asserted with no decoder
    /// charged, so it holds whatever the inflater's footprint; every unit
    /// but a file's last is at least the spacing.
    #[test]
    fn the_default_budget_declines_a_unit_of_the_spacing() {
        let cache = BlockCache::for_unit(CHECKPOINT_SPACING.get()).expect("a nonzero unit");
        let chunk = crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES as u64;
        assert!(!cache.affordable(chunk, 0, DEFAULT_MEMORY_BUDGET));
    }
}
