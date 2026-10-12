use super::{density_merges, max_rows_group};
use crate::statistics::test_support::{Rng, chosen_merges, chosen_merges_between};

/// **The density rule chooses what the ladder of sizes chooses**: the
/// finest size whose upper middle group holds the minimum, and the single
/// group where no size reaches it.
#[test]
fn density_merges_choose_the_finest_size_whose_median_holds_the_minimum() {
    assert_eq!(density_merges(vec![], 5, None), 0);
    assert_eq!(density_merges(vec![3], 5, None), 0, "one group is as coarse as a block gets");
    assert_eq!(density_merges(vec![5, 0, 5], 5, None), 0, "the second smallest of three");
    assert_eq!(density_merges(vec![0, 5, 0, 5], 5, None), 0, "the third smallest of four");
    assert_eq!(density_merges(vec![0, 0, 0, 5], 5, None), 1, "three of four fall short");
    assert_eq!(density_merges(vec![1, 1, 1, 1, 1], 5, None), 3, "never reached: one group");
    assert_eq!(density_merges(vec![9, 9, 9, 9], 0, None), 0, "a minimum of zero merges nothing");
    let mut rng = Rng(0x0020_0004);
    for round in 0..2000 {
        let groups = 1 + rng.below(40) as usize;
        let skew = 1 + rng.below(4);
        let rows: Vec<u64> =
            (0..groups).map(|_| if rng.below(skew) == 0 { 0 } else { rng.below(12) }).collect();
        let min_rows = rng.below(30);
        assert_eq!(
            density_merges(rows.clone(), min_rows, None),
            chosen_merges(&rows, min_rows),
            "round {round}: {rows:?} at {min_rows}"
        );
    }
}

/// **The predicate is monotone in size**, which is what lets a block the
/// cap has already coarsened read its minimum from the size the cap left:
/// once the upper middle group holds the minimum, no coarser size falls
/// back below it. The odd tail is the shape that breaks it for the
/// nearest-rank median — `[m, m, m, m, 0, 0, 0]` meets the minimum, the
/// pairs the cap would leave it do not, and the block cascades to one
/// group — so it is asserted at both sizes, and the sweep holds every
/// level of the ladder.
#[test]
fn the_density_predicate_is_monotone_in_size() {
    assert_eq!(density_merges(vec![5, 5, 5, 5, 0, 0, 0], 5, None), 0, "the odd tail, gathered");
    assert_eq!(density_merges(vec![10, 10, 0, 0], 5, None), 0, "the same block, once capped");
    let mut rng = Rng(0x0020_0011);
    let mut reached_early = 0;
    for round in 0..4000 {
        let groups = 1 + rng.below(40) as usize;
        let skew = 1 + rng.below(4);
        let rows: Vec<u64> =
            (0..groups).map(|_| if rng.below(skew) == 0 { 0 } else { rng.below(12) }).collect();
        let min_rows = rng.below(30);
        let first = chosen_merges(&rows, min_rows) as usize;
        let mut level = rows.clone();
        for merges in 0..groups {
            // Every level at or past the first one that holds is asked
            // again from there, as a capped block's `fit_density` does.
            let from_here = density_merges(level.clone(), min_rows, None) as usize;
            assert_eq!(
                merges + from_here,
                first.max(merges),
                "round {round}: {rows:?} at {min_rows}, from {merges} merges in"
            );
            level = level.chunks(2).map(|pair| pair.iter().sum()).collect();
        }
        reached_early += usize::from(first > 0 && first < groups);
    }
    assert!(reached_early > 500, "only {reached_early} rounds coarsened short of one group");
}

/// **A stated maximum stops the merging, whatever the minimum asks.** A
/// block already past the maximum at the size it holds, its groups pairing
/// evenly, merges nothing — nothing here makes a block finer. Past the
/// maximum, a size whose groups all pair stays past it at the next: of `G`
/// groups, `⌊G/10⌋+1` past it leave at least `⌊G/20⌋+1` pairs past it. A
/// size whose last group stands unpaired is exempt, and can let one merge
/// through (`an_unpaired_last_group_lets_a_block_past_the_maximum_merge`).
#[test]
fn a_stated_maximum_stops_the_merging_whatever_the_minimum_asks() {
    assert_eq!(density_merges(vec![1, 1, 1, 1], 5, Some(2)), 1, "one merge fits, two do not");
    assert_eq!(density_merges(vec![1, 1, 1, 1], 5, Some(9)), 2, "the minimum is reached");
    assert_eq!(density_merges(vec![9, 9, 9, 9], 5, Some(2)), 0, "past the maximum already");
    assert_eq!(density_merges(vec![1, 1, 1, 1], 5, None), 2, "no maximum stops nothing");
    let mut rng = Rng(0x0020_0050);
    let (mut stopped, mut agreed_with_minimum) = (0, 0);
    for round in 0..4000 {
        let groups = 1 + rng.below(40) as usize;
        let skew = 1 + rng.below(4);
        let rows: Vec<u64> =
            (0..groups).map(|_| if rng.below(skew) == 0 { 0 } else { rng.below(12) }).collect();
        let (min_rows, max_rows) = (rng.below(30), rng.below(60));
        let merges = density_merges(rows.clone(), min_rows, Some(max_rows));
        assert_eq!(
            merges,
            chosen_merges_between(&rows, min_rows, max_rows),
            "round {round}: {rows:?} between {min_rows} and {max_rows}"
        );
        let alone = chosen_merges(&rows, min_rows);
        assert!(merges <= alone, "round {round}: a maximum never merges further");
        stopped += usize::from(merges < alone);
        agreed_with_minimum += usize::from(merges == alone);
        // Monotone from every level past the maximum whose groups pair.
        let mut level = rows.clone();
        while level.len() > 1 {
            let coarser: Vec<u64> = level.chunks(2).map(|pair| pair.iter().sum()).collect();
            if level.len().is_multiple_of(2) && max_rows_group(&level) > max_rows {
                assert!(
                    max_rows_group(&coarser) > max_rows,
                    "round {round}: {rows:?} came back within {max_rows} from {level:?}"
                );
            }
            level = coarser;
        }
    }
    assert!(stopped > 300, "only {stopped} rounds were stopped by their maximum");
    assert!(agreed_with_minimum > 300, "only {agreed_with_minimum} rounds reached it");
}

/// **`KD43`, pinned**: nineteen groups, two of them past a maximum of 5,
/// put the block past it, and the merge that pairs them leaves ten whose
/// 90th-percentile group is empty, so the merge goes through and the block
/// holds a group of 20. Closing the deficiency makes this block merge
/// nothing, and flips the assertion.
#[test]
fn an_unpaired_last_group_lets_a_block_past_the_maximum_merge() {
    let rows: Vec<u64> = [10, 10].into_iter().chain([0; 17]).collect();
    assert!(max_rows_group(&rows) > 5, "the block is past the maximum as gathered");
    assert_eq!(density_merges(rows, 1, Some(5)), 1);
}
