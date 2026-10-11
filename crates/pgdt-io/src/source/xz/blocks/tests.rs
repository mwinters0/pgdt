use std::sync::Arc;

use crate::partition::{BOUNDARIED_PARTITION_UNITS, cut};
use crate::test_support::*;

use super::*;

/// **Whole-block decode is declined unless the budget affords the one
/// block a reader holds *and* the retention list beside it**
/// (`docs/design/decisions.md`, "D15"). A reader's decode buffer and the
/// block it then retains are the same buffer, and a single reader leaves
/// `(POOL_DEPTH - 1)` units of list nothing else is there to fill, so
/// [`POOL_DEPTH`] units is the line.
#[test]
fn the_block_path_wants_room_for_one_block_and_the_retention_list() {
    let table = four_block_table();
    let cache = BlockCache::for_table(&table).expect("4 KiB blocks have a unit");
    // The two terms a decline does not save, held at zero so that this
    // test is about the block slots alone; its last assertion and
    // `a_block_too_large_to_hold_refuses_the_block_path` are where they
    // bind.
    let unit = cache.unit as u64;
    let retained = POOL_DEPTH as u64 - 1;

    assert!(
        cache.affordable(0, 0, unit * (1 + retained)),
        "one unit for the reader, and the list the pool retains beside it"
    );
    cache.pool.set_limits(cache.unit * 2, POOL_DEPTH);
    assert_eq!(cache.pool.slots(), 2);

    assert!(
        !cache.affordable(0, 0, unit * (1 + retained) - 1),
        "a byte short of the list is a pool the budget never granted, so it is declined"
    );
    // One unit alone is the per-reader term, and the retained list is what
    // it does not carry.
    assert!(!cache.affordable(0, 0, unit));
    // And the reader's other two terms are inside the same number: the
    // same budget that afforded the blocks declines them once a chunk
    // buffer and a decoder are charged beside it.
    assert!(!cache.affordable(1, 1, unit * (1 + retained)));
}

/// **One slot count covers the retained blocks and the free ones
/// together**, so a block pool's ceiling is its stated budget rather than
/// twice it. Driven through the two calls the decode path makes — a slot,
/// then a retention — with the most recent block held live.
#[test]
fn a_block_pool_holds_one_slot_count_across_both_lists() {
    let table = four_block_table();
    let cache = BlockCache::for_table(&table).expect("4 KiB blocks have a unit");
    cache.pool.set_limits(cache.unit * 3, POOL_DEPTH);
    assert_eq!(cache.pool.slots(), 3);

    let mut live = None;
    for index in 0..12 {
        let slot = cache.slot();
        let block = Arc::new(DecodedBlock { slot, len: cache.unit });
        cache.retain(index, Arc::clone(&block));
        live = Some(block);
        let retained = cache.retained.lock().unwrap().len();
        let free = cache.pool.free_len();
        assert!(
            retained + free <= cache.pool.slots(),
            "read {index}: {retained} retained + {free} free exceeds {} slots",
            cache.pool.slots(),
        );
    }
    drop(live);
}

/// **A block the stated budget cannot hold is not decoded whole**, and the
/// line is the budget rather than a constant beside it
/// (`docs/design/decisions.md`, "D15"). Asserted against a synthetic
/// table, that being the only way to have a block this size without
/// writing one.
#[test]
fn a_block_too_large_to_hold_refuses_the_block_path() {
    let block = |uncompressed_size| xz_seek::BlockEntry {
        compressed_offset: 12,
        uncompressed_offset: 0,
        unpadded_size: 64,
        uncompressed_size,
    };
    let table = |uncompressed_size| xz_seek::SeekTable {
        compressed_file_size: 1 << 20,
        streams: vec![xz_seek::StreamEntry {
            compressed_offset: 0,
            uncompressed_offset: 0,
            compressed_size: 1 << 20,
            uncompressed_size,
            check: xz_seek::Check::Crc64,
            first_block_dict_size: Some(8 << 20),
            padding: 0,
            first_block: 0,
            block_count: 1,
        }],
        blocks: vec![block(uncompressed_size)],
    };
    // A cache exists for any file with blocks; whether the block path is
    // *taken* is the budget's answer, asked again on every read. The
    // decoder's charge is stated here rather than read off a reader — a
    // synthetic table has none — and stands for what
    // `xz_seek::Layout::decode_footprint` answers on an 8 MiB-dictionary
    // file.
    const DECODE: u64 = 9_471_776;
    let affordable = |unit: u64, budget: u64| {
        let cache = BlockCache::for_table(&table(unit)).expect("a block to build a unit from");
        cache.affordable(crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES as u64, DECODE, budget)
    };
    // **`POOL_DEPTH` units, not one.** A single reader is charged the
    // block it holds *and* the retention list beside it
    // ([`BlockCache::worker_memory`]), so the default budget declines an
    // ordinary compressed dump (`docs/design/decisions.md`, "D15").
    assert!(!affordable(24 << 20, DEFAULT_MEMORY_BUDGET));
    assert!(affordable(24 << 20, 107 << 20));
    // Larger blocks — `xz --block-size=128MiB`, and `xz -9 -T0` — are
    // declined at the default and taken once the caller allows the room.
    assert!(!affordable(128 << 20, DEFAULT_MEMORY_BUDGET));
    assert!(!affordable(192 << 20, DEFAULT_MEMORY_BUDGET));
    assert!(!affordable(192 << 20, 768 << 20), "four units of 192 MiB is 768");
    assert!(affordable(192 << 20, 779 << 20));
    // **The unavoidable terms come off the top**: the blocks alone fit
    // this budget and the file is declined all the same, a reader of it
    // also holding the chunk buffer and the decoder — both of which the
    // piecewise arm holds too.
    assert!(!affordable(24 << 20, 96 << 20));
    // A file with no blocks at all — `xz -c /dev/null` writes one — has no
    // unit to size a slot with, so there is no cache to ask.
    let empty =
        xz_seek::SeekTable { compressed_file_size: 32, streams: Vec::new(), blocks: Vec::new() };
    assert!(BlockCache::for_table(&empty).is_none());
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
