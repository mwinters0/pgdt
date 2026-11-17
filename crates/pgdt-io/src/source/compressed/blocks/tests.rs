use std::sync::Arc;

use super::*;

/// **Whole-block decode is declined unless the budget affords the one
/// block a reader holds *and* the retention list beside it**
/// (`docs/design/decisions.md`, "D15"). A reader's decode buffer and the
/// block it then retains are the same buffer, and a single reader leaves
/// `(POOL_DEPTH - 1)` units of list nothing else is there to fill, so
/// [`POOL_DEPTH`] units is the line.
#[test]
fn the_block_path_wants_room_for_one_block_and_the_retention_list() {
    let cache = BlockCache::for_unit(4096).expect("4 KiB blocks have a unit");
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
    let cache = BlockCache::for_unit(4096).expect("4 KiB blocks have a unit");
    cache.pool.set_limits(cache.unit * 3, POOL_DEPTH);
    assert_eq!(cache.pool.slots(), 3);

    let mut live = None;
    for index in 0..12 {
        let block = decode(&cache, index);
        live = Some(block);
        let retained = cache.retention.lock().unwrap().blocks.len();
        let free = cache.pool.free_len();
        assert!(
            retained + free <= cache.pool.slots(),
            "read {index}: {retained} retained + {free} free exceeds {} slots",
            cache.pool.slots(),
        );
    }
    drop(live);
}

/// Block `index` decoded through the calls the decode path makes — a claim,
/// a slot, a landing — with nothing written into it.
fn decode(cache: &BlockCache, index: usize) -> Arc<DecodedBlock> {
    let Claim::Decode(decoding) = cache.claim(index) else {
        panic!("block {index} is neither retained nor in flight");
    };
    decoding.land(DecodedBlock { slot: cache.slot(), len: cache.unit })
}

/// **A miss on a block another reader is decoding waits for that decode
/// rather than repeating it**, and is handed the very block the decoder
/// lands; once landed, the block is a retained hit.
#[test]
fn a_second_miss_on_a_block_in_flight_joins_its_decode() {
    let cache = BlockCache::for_unit(4096).expect("4 KiB blocks have a unit");
    let Claim::Decode(decoding) = cache.claim(1) else { panic!("the first miss decodes") };
    let Claim::Wait(flight) = cache.claim(1) else { panic!("the second miss waits") };
    assert!(
        matches!(cache.claim(2), Claim::Decode(_)),
        "a different block is not held up by this one"
    );

    let landed = decoding.land(DecodedBlock { slot: cache.slot(), len: cache.unit });
    let joined = flight.wait().expect("the decode landed");
    assert!(Arc::ptr_eq(&landed, &joined), "one decode, handed to both readers");
    let Claim::Hit(hit) = cache.claim(1) else { panic!("a landed block is retained") };
    assert!(Arc::ptr_eq(&landed, &hit));
}

/// **A decode that never lands hands its block to the next claim**: its
/// waiters wake to an empty flight rather than waiting on a decode nobody is
/// running, and the block is claimable to decode again. A failed decode and
/// a cancelled fetched read both drop their claim this way.
#[test]
fn a_dropped_decode_hands_its_block_to_the_next_claim() {
    let cache = BlockCache::for_unit(4096).expect("4 KiB blocks have a unit");
    let Claim::Decode(decoding) = cache.claim(0) else { panic!("the first miss decodes") };
    let Claim::Wait(flight) = cache.claim(0) else { panic!("the second miss waits") };
    drop(decoding);
    assert!(flight.wait().is_none(), "the flight comes down empty");
    assert!(matches!(cache.claim(0), Claim::Decode(_)), "and the block is anyone's to decode");
    assert!(cache.retention.lock().unwrap().blocks.is_empty());
}

/// **However many threads miss on one block together, one decodes it** and
/// every other is handed the block it landed — the shape a fused worker's
/// tail read into its successor's block takes, raced rather than staged.
#[test]
fn concurrent_misses_on_one_block_decode_it_once() {
    const READERS: usize = 8;
    let cache = BlockCache::for_unit(4096).expect("4 KiB blocks have a unit");
    let decodes = std::sync::atomic::AtomicUsize::new(0);
    let start = std::sync::Barrier::new(READERS);
    let blocks: Vec<_> = std::thread::scope(|scope| {
        let readers: Vec<_> = (0..READERS)
            .map(|_| {
                scope.spawn(|| {
                    start.wait();
                    loop {
                        match cache.claim(3) {
                            Claim::Hit(block) => return block,
                            Claim::Wait(flight) => {
                                if let Some(block) = flight.wait() {
                                    return block;
                                }
                            }
                            Claim::Decode(decoding) => {
                                decodes.fetch_add(1, Ordering::Relaxed);
                                // Long enough that the others arrive mid-decode.
                                std::thread::sleep(std::time::Duration::from_millis(20));
                                let slot = cache.slot();
                                return decoding.land(DecodedBlock { slot, len: cache.unit });
                            }
                        }
                    }
                })
            })
            .collect();
        readers.into_iter().map(|reader| reader.join().unwrap()).collect()
    });
    assert_eq!(decodes.load(Ordering::Relaxed), 1);
    assert!(blocks.iter().all(|block| Arc::ptr_eq(block, &blocks[0])));
}

/// [`Flight::landed`] is woken by a landing that happens while it is
/// parked, which is the fetched source's wait.
#[tokio::test]
async fn an_awaiting_miss_is_woken_by_the_landing() {
    let cache = Arc::new(BlockCache::for_unit(4096).expect("a unit"));
    let Claim::Decode(decoding) = cache.claim(2) else { panic!("the first miss decodes") };
    let Claim::Wait(flight) = cache.claim(2) else { panic!("the second miss waits") };
    let waiter = tokio::spawn(async move { flight.landed().await });
    tokio::task::yield_now().await;
    let landed = decoding.land(DecodedBlock { slot: cache.slot(), len: cache.unit });
    let joined = waiter.await.unwrap().expect("the decode landed");
    assert!(Arc::ptr_eq(&landed, &joined));
}

/// **A block the stated budget cannot hold is not decoded whole**, and the
/// line is the budget rather than a constant beside it
/// (`docs/design/decisions.md`, "D15"). Asserted against a stated unit,
/// that being the only way to have a block this size without writing one.
#[test]
fn a_block_too_large_to_hold_refuses_the_block_path() {
    // A cache exists for any file with blocks; whether the block path is
    // *taken* is the budget's answer, asked again on every read. The
    // decoder's charge is stated here rather than read off a reader — a
    // stated unit has none — and stands for what
    // `xz_seek::Layout::decode_footprint` answers on an 8 MiB-dictionary
    // file.
    const DECODE: u64 = 9_471_776;
    let affordable = |unit: u64, budget: u64| {
        let cache = BlockCache::for_unit(unit).expect("a block to build a unit from");
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
    assert!(BlockCache::for_unit(0).is_none());
}

/// **A streaming-fallback source advises one partition even though its
/// file has boundaries** (`docs/design/decisions.md`, "D15"). Asserted
/// against four 4 KiB units' boundaries, `partition_advice` being a free
/// function precisely so that the fallback arm can be handed a file that
/// *does* have boundaries.
#[test]
fn a_streaming_fallback_source_advises_one_partition() {
    let cuts = || vec![4096, 2 * 4096, 3 * 4096];
    let cache = BlockCache::for_unit(4096).expect("4 KiB blocks decode whole");
    let decoding = partition_advice(cuts, Some(&cache), 1 << 20, 9 << 20);
    assert_eq!(decoding.max_partitions(), Some(4));
    assert_eq!(decoding.retained_unit(), RetainedUnit::Partition);
    assert_eq!(
        decoding.partition_bytes(),
        4096 + (1 << 20) + (9 << 20),
        "one block slot, the chunk buffer, and the decoder's own retention"
    );

    let streaming = partition_advice(cuts, None, 1 << 20, 9 << 20);
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
    // asked for. The unit the block arm states is the file's own largest
    // unit rather than the piece.
    assert_eq!(decoding.partition_read(), PartitionRead::Whole { unit: 4096 });
    assert_eq!(streaming.partition_read(), PartitionRead::Chunked);
}

/// What `budget`'s block pool holds now: its retained blocks, and its free
/// buffers.
fn held(budget: &DecodeBudget) -> (usize, usize) {
    let cache = budget.blocks.as_ref().expect("a file with blocks");
    (cache.retention.lock().unwrap().blocks.len(), cache.pool.free_len())
}

/// `count` blocks from `first` decoded through `budget`'s block cache, one
/// after another, none still viewed.
fn decode_through(budget: &DecodeBudget, first: usize, count: usize) {
    let cache = budget.blocks.as_ref().expect("a file with blocks");
    (first..first + count).for_each(|index| drop(decode(cache, index)));
}

/// **A reader that parks gives back its share of the retained blocks at
/// once, and takes it again as it resumes** (`docs/design/decisions.md`,
/// "D162"): above [`POOL_DEPTH`] readers the block pool's depth is the
/// readers still reading, and the list is drained to one below it as the
/// reader parks rather than at the next decode. Nothing else moves: the arm
/// a read takes and the cost of a reader stand.
#[test]
fn a_parked_reader_gives_back_its_retained_blocks_and_takes_them_again() {
    let unit = 4096;
    let budget = DecodeBudget::over(unit as u64, 0);
    budget.hint_read_size(1024);
    budget.hint_parallelism(Parallelism::workers(8, 1 << 20));
    let cache = budget.blocks.as_ref().expect("a file with blocks");
    assert_eq!(cache.pool.slots(), 8, "one slot a reader, the budget affording them");
    let block_path = budget.block_path().is_some();
    let block_decode_bytes = budget.block_decode_bytes();
    assert!(block_path);

    decode_through(&budget, 0, 12);
    assert_eq!(held(&budget), (8, 0), "a full list, the last decode landed in its last slot");

    (0..3).for_each(|_| budget.hint_parked(true));
    assert_eq!(cache.pool.slots(), 5, "the five readers still reading");
    let (retained, free) = held(&budget);
    assert_eq!(retained, 4, "drained to one below the slots as the readers park");
    assert!(retained + free <= 5, "{retained} retained and {free} free past five slots");
    assert_eq!(budget.block_path().is_some(), block_path, "the arm a read takes stands");
    assert_eq!(budget.block_decode_bytes(), block_decode_bytes, "and what a reader costs");

    decode_through(&budget, 12, 6);
    let (retained, free) = held(&budget);
    assert!(retained + free <= 5, "decoding while parked keeps to five slots");

    (0..3).for_each(|_| budget.hint_parked(false));
    assert_eq!(cache.pool.slots(), 8, "resumed, the depth is back");
    decode_through(&budget, 18, 12);
    assert_eq!(held(&budget), (8, 0), "and the list regrows as decodes land");
}

/// **At or below [`POOL_DEPTH`] readers a park keeps the depth and still
/// drains the list to one below it**, so a block is given back at the park
/// rather than at the next decode; a resume past every park is no resume.
#[test]
fn a_parked_reader_under_the_pool_depth_gives_back_its_last_block() {
    let budget = DecodeBudget::over(4096, 0);
    budget.hint_read_size(1024);
    budget.hint_parallelism(Parallelism::workers(2, 1 << 20));
    let cache = budget.blocks.as_ref().expect("a file with blocks");
    assert_eq!(cache.pool.slots(), POOL_DEPTH);

    decode_through(&budget, 0, 8);
    assert_eq!(held(&budget), (POOL_DEPTH, 0));
    budget.hint_parked(true);
    assert_eq!(cache.pool.slots(), POOL_DEPTH, "the depth is the pool's own, not a reader's");
    assert_eq!(held(&budget).0, POOL_DEPTH - 1, "the parked reader's block given back");

    budget.hint_parked(false);
    budget.hint_parked(false);
    budget.hint_parked(true);
    assert_eq!(budget.parked.load(Ordering::Relaxed), 1, "an unmatched resume counts nothing");
    assert_eq!(cache.pool.slots(), POOL_DEPTH);
}
