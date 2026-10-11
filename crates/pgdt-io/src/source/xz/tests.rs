use crate::partition::{BOUNDARIED_PARTITION_UNITS, cut};
use crate::test_support::*;
use crate::*;

use super::*;

/// A block-decoding `.xz` advises its block boundaries, so a partition is
/// a whole number of blocks and no cut falls inside one.
/// The boundaries are ascending and strictly inside the range: the block
/// containing `range.start` begins at or before it, and is not a split.
#[tokio::test]
async fn an_xz_source_advises_its_block_boundaries() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let source = XzSource::open(compressed.path()).unwrap();
    let table = source.seek_table().unwrap();
    let starts: Vec<u64> = table.blocks.iter().map(|b| b.uncompressed_offset).collect();
    assert!(starts.len() > 2, "this test needs several blocks, got {}", starts.len());

    let whole = source.partitions(0..payload.len() as u64);
    assert_eq!(whole.boundaries(), &PartitionBoundaries::At(starts[1..].to_vec()));
    assert_eq!(whole.max_partitions(), Some(starts.len()));

    // A range opening inside a block: that block's own start is behind
    // `range.start` and is not offered as a split.
    let from = starts[1] + 10;
    let inner = source.partitions(from..payload.len() as u64);
    assert_eq!(inner.boundaries(), &PartitionBoundaries::At(starts[2..].to_vec()));

    // A range inside one block has nothing to split at.
    let one = source.partitions(from..from + 10);
    assert_eq!(one.max_partitions(), Some(1));

    // A partition costs what one concurrent reader holds: one block slot
    // — the one being decoded, which is the one it then retains — the
    // chunk buffer a straddling read is assembled into, and the decoder's
    // own retention. The chunk term is the one a scan settles at, no read
    // loop having announced one here
    // ([`XzBudget::charged_chunk_bytes`]).
    let unit = source.budget.blocks.as_ref().unwrap().unit as u64;
    assert_eq!(
        whole.partition_bytes(),
        unit + crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES as u64 + source.budget.decode_bytes
    );
    assert_eq!(whole.worker_memory().bytes_per_worker(), whole.partition_bytes());

    // **And the gate is that same charge at one reader**: the per-reader
    // term plus the `(POOL_DEPTH - 1)` units of retention list nobody else
    // is there to take ([`WorkerMemory`]).
    assert_eq!(
        source.block_decode_bytes(),
        Some(whole.partition_bytes() + (POOL_DEPTH as u64 - 1) * unit)
    );
    assert_eq!(source.block_decode_bytes(), Some(whole.worker_memory().at(1)));

    // And what a retained batch pins is that same block, which the line
    // above has already charged for — so a caller must not add its span
    // allowance on top ([`RetainedUnit`]).
    assert_eq!(whole.retained_unit(), RetainedUnit::Partition);
}

/// **A streaming-fallback source advises one partition even though its
/// table has boundaries** (`docs/design/decisions.md`, "D15"). Asserted
/// against a synthetic multi-block table, `xz_partition_advice` being a free
/// function precisely so that the fallback arm can be handed a table that
/// *does* have boundaries.
#[test]
fn a_streaming_fallback_source_advises_one_partition() {
    let table = four_block_table();
    assert!(table.is_seekable(), "the table has boundaries to advise");

    let cache = BlockCache::for_table(&table).expect("4 KiB blocks decode whole");
    let decoding = xz_partition_advice(&table, Some(&cache), 1 << 20, 9 << 20, 0..4 * 4096);
    assert_eq!(decoding.max_partitions(), Some(4));
    assert_eq!(decoding.retained_unit(), RetainedUnit::Partition);
    assert_eq!(
        decoding.partition_bytes(),
        4096 + (1 << 20) + (9 << 20),
        "one block slot, the chunk buffer, and the decoder's own retention"
    );

    let streaming = xz_partition_advice(&table, None, 1 << 20, 9 << 20, 0..4 * 4096);
    assert_eq!(streaming.max_partitions(), Some(1));
    assert_eq!(
        streaming.partition_bytes(),
        1 << 20,
        "one chunk buffer a reader; the fallback's single decoder is a cost of the source"
    );
    assert_eq!(
        streaming.retained_unit(),
        RetainedUnit::ReadChunk,
        "a piecewise read is assembled into a chunk buffer, so chunks are what a batch pins"
    );
    // **The read shape is stated apart from both of those**: a
    // block-decoding piece is read whole and copies nothing, while a
    // piecewise read is assembled into a chunk buffer whatever length is
    // asked for. The unit the block arm states is the table's own largest
    // block rather than the piece.
    assert_eq!(decoding.partition_read(), PartitionRead::Whole { unit: 4096 });
    assert_eq!(
        table.max_block_uncompressed(),
        4096,
        "the stated unit is the table's largest block, not the piece"
    );
    assert_eq!(streaming.partition_read(), PartitionRead::Chunked);
}

/// **Every partition the leader hands a worker covers at most
/// [`BOUNDARIED_PARTITION_UNITS`] of this source's units**, whatever the
/// charge says and whatever the worker count is — which is what makes the
/// cut width a number somebody chose rather than a consequence of the
/// memory charge.
///
/// The failure mode it guards is `partition_bytes` serving as both the
/// memory charge and `run_region`'s cut size
/// (`docs/design/decisions.md`, "D8").
///
/// It walks the window loop rather than one window, a *mid-block* start
/// being the case a single aligned window would never show.
#[test]
fn a_block_decoding_partition_spans_at_most_the_cut_width() {
    // Enough blocks that a window of `workers × BOUNDARIED_PARTITION_UNITS`
    // is satisfied out of the boundary list rather than running off the end
    // of the file, which is what makes the bound below bind.
    let blocks = 8 * BOUNDARIED_PARTITION_UNITS as u64 + 4;
    let table = block_table(blocks);
    let size = blocks * 4096;
    let cache = BlockCache::for_table(&table).expect("4 KiB blocks decode whole");
    let advice = xz_partition_advice(&table, Some(&cache), 1 << 20, 9 << 20, 0..size);
    // The charge is a block, a chunk and the decoder — orders of
    // magnitude above the 4 KiB unit here, so a window sized by it would
    // swallow the whole file.
    assert!(advice.partition_bytes() > size, "the charge is not the cut size");

    for workers in [2usize, 3, 4, 8] {
        for start in [0u64, 1, 4095, 4096, 5000, 8192, 12287] {
            assert!(start < size, "a frontier inside the file");
            let end = advice.window_end(start, workers, size);
            assert!(end > start, "a window must advance: {workers} workers from {start}");
            let ranges = cut(start..end, &advice, workers);
            assert!(
                ranges.len() <= workers,
                "{workers} workers were handed {} pieces from {start}",
                ranges.len()
            );
            for piece in ranges {
                let covering = table.blocks_in(piece.clone());
                assert!(
                    covering.len() <= BOUNDARIED_PARTITION_UNITS,
                    "{piece:?} spans {} blocks at {workers} workers from {start}, \
                         against a cut width of {BOUNDARIED_PARTITION_UNITS}",
                    covering.len()
                );
            }
        }
    }
}
