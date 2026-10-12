//! The group-density rule (`docs/design/decisions.md`, "D82"): which group
//! a block's density minimum and maximum are read at, how many pairwise
//! merges a finished block's groups take to reach the minimum without
//! breaking the maximum, and what a block already gathered says of a stated
//! maximum. Nothing here persists: a block records the request that sized it
//! (`pgdt_model::GroupSizing`), never these readings.

use pgdt_model::BlockStatistics;

/// The `rank`-th smallest of `rows`, counting from zero.
fn nth_smallest(rows: &[u64], rank: usize) -> u64 {
    let mut sorted = rows.to_vec();
    *sorted.select_nth_unstable(rank).1
}

/// **The group a density minimum is read at**: the upper middle one, the
/// `⌊G/2⌋+1`-th smallest of `G` groups' rows, so that at most half the groups
/// fall short of the minimum. Reading the upper middle group rather than the
/// nearest-rank `⌈G/2⌉`-th is what makes the predicate monotone in size
/// ([`density_merges`]). Panics on no groups.
pub(super) fn min_rows_group(rows: &[u64]) -> u64 {
    nth_smallest(rows, rows.len() / 2)
}

/// **The group a density maximum is read at**: the 90th-percentile one by
/// nearest rank, the `⌈9G/10⌉`-th smallest of `G` groups' rows, so that at most
/// a tenth of the groups hold more than the maximum. Near enough a bound for a
/// person who asked for one, without one dense stretch multiplying a whole
/// block's groups. Panics on no groups.
pub(super) fn max_rows_group(rows: &[u64]) -> u64 {
    nth_smallest(rows, (9 * rows.len()).div_ceil(10) - 1)
}

/// How many times a block whose groups hold `rows` merges pairwise before its
/// **upper middle** group ([`min_rows_group`]) holds `min_rows`, or it is one
/// group: at each merge the rows per group are the sums of adjacent pairs, a
/// last odd group standing alone.
///
/// Reading the upper middle group rather than the nearest-rank `⌈G/2⌉`-th is
/// what makes the predicate **monotone in size**: a merged group falls short
/// only where both its parts did, so of `S ≤ ⌊G/2⌋` short groups at most
/// `⌊S/2⌋` pairs fall short, plus a short odd tail — within `⌊⌈G/2⌉/2⌋` at
/// every `G` (by cases on `G` mod 4), the same predicate at the coarser size.
/// `the_density_predicate_is_monotone_in_size` holds it.
///
/// **`max_rows` stops the merging first**, wherever the next size would put
/// more than that many rows in its 90th-percentile group ([`max_rows_group`]):
/// so a stated maximum outranks the minimum. Nothing here can make a block
/// *finer*: a block breaking `max_rows` at the size it gathered from is
/// re-read instead ([`StatisticsRequest::backfill`](crate::statistics::StatisticsRequest::backfill)).
///
/// deficiency: KD43 — the maximum's predicate is monotone in size only where
/// every group pairs. A last odd group stands alone, so a pairwise sum can
/// rank below the parts it replaces: at 19, 39, 59… groups a block already
/// past the maximum can merge once (`[10, 10, 0 × 17]` under a maximum of 5
/// becomes `[20, 0 × 9]`, whose 90th-percentile group is 0), and stopping at
/// the first merge that breaks the predicate can miss a coarser size that
/// would hold it. `an_unpaired_last_group_lets_a_block_past_the_maximum_merge`
/// pins the example. **(c) unowned**; promoted by a stated maximum seen to
/// leave a block's groups past it.
pub(super) fn density_merges(mut rows: Vec<u64>, min_rows: u64, max_rows: Option<u64>) -> u32 {
    let mut merges = 0;
    while rows.len() > 1 && min_rows_group(&rows) < min_rows {
        let coarser: Vec<u64> = rows.chunks(2).map(|pair| pair.iter().sum()).collect();
        if max_rows.is_some_and(|max_rows| max_rows_group(&coarser) > max_rows) {
            break;
        }
        rows = coarser;
        merges += 1;
    }
    merges
}

/// What a block already gathered says of a stated maximum: an extension
/// trait, the type being the model's and the reading this crate's.
pub(crate) trait BlockStatisticsExt {
    fn breaks_max_rows(&self, max_rows: u64) -> bool;
    fn predicted_group_size(&self, max_rows: u64) -> u64;
}

impl BlockStatisticsExt for BlockStatistics {
    /// Whether this block's groups break `max_rows`: its 90th-percentile group
    /// holds more rows than that. A block with no group breaks nothing.
    fn breaks_max_rows(&self, max_rows: u64) -> bool {
        let rows: Vec<u64> = self.groups.iter().map(|group| group.rows).collect();
        !rows.is_empty() && max_rows_group(&rows) > max_rows
    }

    /// **The group size a re-read of this block should gather at to meet
    /// `max_rows`**: halved, from the size it holds, until the rows its
    /// 90th-percentile group would then hold are within the maximum — a
    /// prediction, since halving a group does not halve every group's rows,
    /// and one the re-read is free to miss where a block's rows cluster.
    /// Asked only of a block [`Self::breaks_max_rows`] answered for, so it
    /// holds at least one group.
    fn predicted_group_size(&self, max_rows: u64) -> u64 {
        let rows: Vec<u64> = self.groups.iter().map(|group| group.rows).collect();
        let (mut size, mut densest) = (self.group_size, max_rows_group(&rows));
        while densest > max_rows && size > 1 {
            (size, densest) = (size / 2, densest.div_ceil(2));
        }
        size
    }
}

#[cfg(test)]
mod tests;
