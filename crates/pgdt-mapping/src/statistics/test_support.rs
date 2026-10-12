//! What more than one statistics module's tests build: a seeded generator,
//! and the density rule's reference model, the ladder of sizes
//! `scripts/row_density.py` walks.

use super::density::{max_rows_group, min_rows_group};

/// SplitMix64, seeded, so a failing case reproduces.
pub(super) struct Rng(pub(super) u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub(super) fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// The size [`density_merges`](super::density::density_merges) must choose, written the way
/// `scripts/row_density.py` chooses it: every size from the gathered one
/// to a single group, and the first whose upper middle group holds the
/// minimum, else the single group.
pub(super) fn chosen_merges(rows: &[u64], min_rows: u64) -> u32 {
    if rows.is_empty() {
        return 0;
    }
    let mut ladder = vec![rows.to_vec()];
    while ladder.last().unwrap().len() > 1 {
        let last = ladder.last().unwrap();
        let coarser =
            (0..last.len()).step_by(2).map(|i| last[i..last.len().min(i + 2)].iter().sum());
        ladder.push(coarser.collect());
    }
    let median = |level: &Vec<u64>| {
        let mut sorted = level.clone();
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    };
    let reaches = ladder.iter().position(|level| median(level) >= min_rows);
    reaches.unwrap_or(ladder.len() - 1) as u32
}

/// The size [`density_merges`](super::density::density_merges) must choose under a maximum as well as a
/// minimum: the ladder walked as [`chosen_merges`] walks it, stopping at
/// the last size whose 90th-percentile group is within the maximum.
pub(super) fn chosen_merges_between(rows: &[u64], min_rows: u64, max_rows: u64) -> u32 {
    let mut level = rows.to_vec();
    let mut merges = 0;
    while level.len() > 1 && min_rows_group(&level) < min_rows {
        let coarser: Vec<u64> = level.chunks(2).map(|pair| pair.iter().sum()).collect();
        if max_rows_group(&coarser) > max_rows {
            break;
        }
        (level, merges) = (coarser, merges + 1);
    }
    merges
}
