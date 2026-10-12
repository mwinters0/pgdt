use std::ops::Range;

use crate::test_support::*;
use crate::*;

use super::*;

/// **A plain file's window is unchanged by the split above**: with no
/// boundaries to respect the charge *is* the cut size, so a window is
/// `workers` charges wide exactly as it always was.
#[test]
fn a_plain_window_is_still_the_charge_times_the_worker_count() {
    let (_file, source) = source_of(&[0u8; 64]);
    source.hint_read_size(8);
    let advice = source.partitions(0..64);
    assert_eq!(
        advice.partition_read(),
        PartitionRead::Chunked,
        "a plain partition is read a chunk at a time, so every buffer is a pooled one"
    );
    assert_eq!(
        BareSource.partitions(0..1 << 30).partition_read(),
        PartitionRead::Chunked,
        "and a source that has said nothing is read the conservative way"
    );
    let charge = advice.partition_bytes();
    assert_eq!(advice.window_end(0, 3, u64::MAX), 3 * charge);
    assert_eq!(advice.window_end(charge, 3, u64::MAX), 4 * charge);
    assert_eq!(advice.window_end(0, 3, 17), 17, "the limit binds");
}

/// The two properties every cut has to have, whatever the source advised:
/// the pieces **tile** the range exactly, in ascending order, and there
/// are never more of them than the caller asked for.
fn assert_tiles(range: Range<u64>, pieces: &[Range<u64>], want: usize) {
    assert!(!pieces.is_empty(), "a cut always yields at least the range itself");
    assert!(pieces.len() <= want.max(1), "{} pieces for want {want}", pieces.len());
    assert_eq!(pieces[0].start, range.start);
    assert_eq!(pieces[pieces.len() - 1].end, range.end);
    for pair in pieces.windows(2) {
        assert_eq!(pair[0].end, pair[1].start, "{pieces:?}");
    }
}

/// `Anywhere` is cut evenly, and never into pieces of zero bytes: a range
/// shorter than the worker count is cut into one piece per byte and no
/// finer, a piece with no bytes being able to own no rows.
#[test]
fn an_anywhere_source_is_cut_evenly_and_never_below_a_byte() {
    let advice = Partitioning::anywhere(1 << 20);
    for want in [1usize, 2, 3, 4, 7, 8, 64] {
        let pieces = cut(100..900, &advice, want);
        assert_tiles(100..900, &pieces, want);
        assert_eq!(pieces.len(), want.max(1));
    }
    let pieces = cut(10..13, &advice, 8);
    assert_eq!(pieces, vec![10..11, 11..12, 12..13]);
}

/// `At` is cut only where the source offered, thinned evenly when it
/// offers more boundaries than the caller can use, and taken whole when it
/// offers exactly as many as are wanted.
#[test]
fn an_at_source_is_cut_only_where_it_offered() {
    let advice = Partitioning::at(vec![20, 40, 60, 80], 1 << 20);
    assert_eq!(cut(0..100, &advice, 5), vec![0..20, 20..40, 40..60, 60..80, 80..100]);
    assert_eq!(cut(0..100, &advice, 9), vec![0..20, 20..40, 40..60, 60..80, 80..100]);
    assert_eq!(cut(0..100, &advice, 3), vec![0..40, 40..80, 80..100]);
    assert_eq!(cut(0..100, &advice, 2), vec![0..60, 60..100]);
    // Boundaries outside the range are not cuts, nor are its own ends.
    assert_eq!(cut(40..80, &advice, 4), vec![40..60, 60..80]);
    for want in [1usize, 2, 3, 4, 5, 9] {
        assert_tiles(0..100, &cut(0..100, &advice, want), want);
    }
}

/// A source that declines to be split is not split, however many workers
/// the caller has — the empty `At` is a policy, not an absence
/// (`docs/design/decisions.md`, "D7").
#[test]
fn a_source_that_declines_to_be_split_is_not() {
    assert_eq!(cut(0..1000, &Partitioning::single(0), 16), vec![0..1000]);
}

/// **Every partition the leader hands a worker covers at most
/// [`BOUNDARIED_PARTITION_UNITS`] of a boundaried source's units**, whatever
/// the charge says and whatever the worker count is — which is what makes
/// the cut width a number somebody chose rather than a consequence of the
/// memory charge.
///
/// The failure mode it guards is `partition_bytes` serving as both the
/// memory charge and `run_region`'s cut size
/// (`docs/design/decisions.md`, "D8").
///
/// It walks the window loop rather than one window, a *mid-unit* start
/// being the case a single aligned window would never show. The advice is a
/// block-decoding `.xz` source's: a boundary at every 4 KiB block start
/// inside the range, and a charge of a block, a chunk and the decoder;
/// that `xz_partition_advice` offers those starts is the block cache's to
/// assert.
#[test]
fn a_boundaried_partition_spans_at_most_the_cut_width() {
    const UNIT: u64 = 4096;
    // Enough units that a window of `workers × BOUNDARIED_PARTITION_UNITS`
    // is satisfied out of the boundary list rather than running off the end
    // of the range, which is what makes the bound below bind.
    let units = 8 * BOUNDARIED_PARTITION_UNITS as u64 + 4;
    let size = units * UNIT;
    let advice =
        Partitioning::at((1..units).map(|i| i * UNIT).collect(), UNIT + (1 << 20) + (9 << 20));
    // The charge is orders of magnitude above the 4 KiB unit here, so a
    // window sized by it would swallow the whole range.
    assert!(advice.partition_bytes() > size, "the charge is not the cut size");
    let covering = |piece: &Range<u64>| piece.start / UNIT..piece.end.div_ceil(UNIT);

    for workers in [2usize, 3, 4, 8] {
        for start in [0u64, 1, 4095, 4096, 5000, 8192, 12287] {
            assert!(start < size, "a frontier inside the range");
            let end = advice.window_end(start, workers, size);
            assert!(end > start, "a window must advance: {workers} workers from {start}");
            let ranges = cut(start..end, &advice, workers);
            assert!(
                ranges.len() <= workers,
                "{workers} workers were handed {} pieces from {start}",
                ranges.len()
            );
            for piece in ranges {
                let covering = covering(&piece);
                assert!(
                    covering.end - covering.start <= BOUNDARIED_PARTITION_UNITS as u64,
                    "{piece:?} spans {} units at {workers} workers from {start}, \
                         against a cut width of {BOUNDARIED_PARTITION_UNITS}",
                    covering.end - covering.start
                );
            }
        }
    }
}
