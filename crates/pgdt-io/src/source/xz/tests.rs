use crate::source::pool::POOL_DEPTH;
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
