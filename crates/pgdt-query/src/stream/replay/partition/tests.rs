use std::collections::BTreeMap;
use std::pin::Pin;

use bytes::Bytes;
use pgdt_core::Result;
use pgdt_io::SCAN_CHUNK_DEFAULT_SIZE_BYTES;
use pgdt_io::internal::worker_count;
use pgdt_io::{ByteRangeSource, Parallelism, WorkerMemory};
use pgdt_model::{CopyBlock, Finding};

use super::{derived_source_span, distribute, plan_partitions};
use crate::stream::replay::segment::{Segment, SegmentEntry};
use crate::stream::tests::two_blocks;
use crate::stream::{PlanLever, PlanNoteKind};

/// **The span is spent before the count is cut, and it stops at the
/// floor** (`docs/design/decisions.md`, "D84"). Every arm of
/// [`derived_source_span`], against a plain source's shape — an 8 MiB
/// per-worker charge with no pool term — the shipped 64 MiB ceiling, and
/// the shipped 1 MiB read chunk as the floor.
///
/// The exact-inversion arm is the load-bearing one: what comes back must
/// be the largest span at which the count asked for is still afforded, so
/// each case asserts the returned span *and* that `worker_count` then
/// hands out every worker (or, at the floor, how many it could).
#[test]
fn a_stated_span_is_spent_down_to_seat_the_readers_and_no_further() {
    let footprint = 8 << 20;
    let charge = WorkerMemory::per_worker(footprint);
    let stated = 64 << 20;
    let chunk = SCAN_CHUNK_DEFAULT_SIZE_BYTES;
    let derived = |parallelism: Parallelism, stated: usize| {
        derived_source_span(charge, parallelism, stated, chunk)
    };
    let seated = |parallelism: Parallelism, span: usize| {
        worker_count(parallelism, charge.plus_per_worker(span as u64))
    };

    // Nothing stated: nothing to solve against.
    assert_eq!(derived(Parallelism::default(), stated), stated);

    // One worker is what the floors deliver whatever the charge, so the
    // stated span is already affordable and is left alone — which is what
    // keeps a serial replay's batches the size the caller asked for.
    let serial = Parallelism::workers(1, 64 << 20);
    assert_eq!(derived(serial, stated), stated);

    // A budget with room to spare leaves the ceiling in place.
    let roomy = Parallelism::workers(2, 1 << 30);
    assert_eq!(derived(roomy, stated), stated);

    // Two readers of 8 MiB inside 64 MiB leave 24 MiB apiece, and 24 MiB
    // is exactly what comes back: one byte more would seat only one.
    let two = Parallelism::workers(2, 64 << 20);
    assert_eq!(derived(two, stated), 24 << 20);
    assert_eq!(seated(two, 24 << 20), 2);
    assert_eq!(seated(two, (24 << 20) + 1), 1);

    // Four readers leave 8 MiB apiece, on the same arithmetic.
    let four = Parallelism::workers(4, 64 << 20);
    assert_eq!(derived(four, stated), 8 << 20);
    assert_eq!(seated(four, 8 << 20), 4);

    // Eight readers leave nothing, so the span stops at the floor and the
    // shortfall is the count's: 9 MiB apiece affords seven of the eight.
    let eight = Parallelism::workers(8, 64 << 20);
    assert_eq!(derived(eight, stated), chunk);
    assert_eq!(seated(eight, chunk), 7);

    // A budget under one reader's own charge cannot go below the floor
    // either — `AllocationBelowFloor` is what names that arrangement.
    let starved = Parallelism::workers(8, 1 << 20);
    assert_eq!(derived(starved, stated), chunk);

    // **It only ever moves down.** A caller stating less than the floor is
    // stating a batch size, and gets it.
    assert_eq!(derived(eight, 4096), 4096);
    assert_eq!(derived(two, chunk), chunk);
}

/// **The floor is the chunk this caller announced, not the shipped one**
/// (`docs/design/decisions.md`, "D84"): the pin is the span rounded out to
/// what the source retains, and what it retains is what the replay loop
/// asked it to read in (`pgdt_dump::ReadOptions::chunk_size_bytes`).
///
/// The shape is `--chunk-size 64k` on a plain source, whose per-worker
/// footprint is eight of whatever chunk it was told to read
/// (`pgdt_io::PLAIN_PARTITION_CHUNKS`), against a budget of exactly what
/// the readers asked for cost — so the room left for a span is nothing and
/// the floor is the whole answer. The two floors differ by a factor of
/// sixteen there, and so does what the budget then seats.
#[test]
fn the_floor_is_one_announced_read_chunk_and_a_small_one_buys_readers() {
    let chunk = 64 << 10;
    let unit = 8 * chunk as u64;
    let charge = WorkerMemory::per_worker(unit);
    let stated = 64 << 20;
    // Eight readers of `unit` inside eight units: the count is afforded at
    // no span at all, which is where the floor decides.
    let eight = Parallelism::workers(8, 8 * unit);
    let seated = |span: usize| worker_count(eight, charge.plus_per_worker(span as u64));

    let span = derived_source_span(charge, eight, stated, chunk);
    assert_eq!(span, chunk, "the floor is the announced chunk");
    assert_eq!(seated(span), 7, "which is what the budget then seats");

    // What the shipped constant would have floored at, and what it cost:
    // a span sixteen times the unit the source actually retains, and five
    // of the seven readers declined for bytes no batch pins.
    let shipped = derived_source_span(charge, eight, stated, SCAN_CHUNK_DEFAULT_SIZE_BYTES);
    assert_eq!(shipped, SCAN_CHUNK_DEFAULT_SIZE_BYTES);
    assert_eq!(seated(shipped), 2);

    // **A chunk larger than the shipped one floors above it**, the other
    // direction of the same defect: the span is rounded out to the chunk
    // whatever the chunk says, so narrowing below it buys nothing.
    let big = 4 << 20;
    let charge = WorkerMemory::per_worker(8 * big as u64);
    let eight = Parallelism::workers(8, 8 * 8 * big as u64);
    assert_eq!(derived_source_span(charge, eight, stated, big), big);
}

/// **A plan note lists only the levers that move it, for this source and
/// this budget**, and `AllocationBelowFloor`'s sentence offers the chunk
/// exactly where its levers list one. Over a plain file, whose budget a
/// larger allowance raises only below `DEFAULT_MEMORY_BUDGET` and whose
/// reader the chunk sizes, the floor note has three arrangements: a zero
/// budget moved by memory alone, one between zero and a reader by memory
/// or the chunk, and one at that cap under a chunk past it by the chunk
/// alone. At the cap the count note keeps only the chunk, and the span
/// note the chunk and fewer sub-streams — a smaller chunk being a cheaper
/// reader, which leaves each seated sub-stream a wider batch.
#[test]
fn a_plan_note_lists_only_the_levers_that_move_it() {
    use PlanLever::{FewerSubStreams, LargerAllowance, SmallerReadChunk};
    use pgdt_io::LocalFileSource;

    let dir = tempfile::tempdir().unwrap();
    let source = LocalFileSource::open(two_blocks(dir.path())).unwrap();
    let block = CopyBlock {
        header: pgdt_core::parse_copy_header(b"COPY public.t (a, b) FROM stdin;").unwrap(),
        header_offset: 0,
        data_offset: 33,
        terminator_offset: 42,
        end_offset: 45,
        row_count: 2,
        partition_root: None,
        statistics: None,
        statistics_declined: None,
        ignored_refusals: None,
        checked_in_full: false,
        census: Some(pgdt_model::BlockCensus::default()),
    };
    // Nothing is read: the plan prices the chunk the source was last told.
    let planned = |chunk: usize, parallelism, span| {
        source.hint_read_size(chunk);
        let (_, notes, span, _) = plan_partitions(
            &source,
            std::slice::from_ref(&block),
            &BTreeMap::new(),
            parallelism,
            span,
            chunk,
        );
        (notes, span)
    };
    let plan = |chunk, parallelism, span| planned(chunk, parallelism, span).0;
    let floor = |chunk, budget| {
        let notes = plan(chunk, Parallelism::workers(1, budget), None);
        let [note] = notes.as_slice() else { panic!("{notes:?}") };
        assert!(matches!(note.kind, PlanNoteKind::AllocationBelowFloor { .. }), "{note:?}");
        let offers_chunk = note.message().contains("a smaller read chunk");
        assert_eq!(offers_chunk, note.levers.contains(&SmallerReadChunk), "{}", note.message());
        assert!(note.message().contains("a larger memory budget"), "{}", note.message());
        note.levers.clone()
    };
    assert_eq!(floor(1 << 20, 0), vec![LargerAllowance]);
    assert_eq!(floor(1 << 20, 4 << 20), vec![LargerAllowance, SmallerReadChunk]);
    let cap = pgdt_io::DEFAULT_MEMORY_BUDGET;
    assert_eq!(floor(2 * cap as usize, cap), vec![SmallerReadChunk]);
    // The cap is what an allowance of any size carves for this source.
    let request = pgdt_io::ArrangementRequest::recommended(1, None);
    let carved = pgdt_io::Arrangement::resolve(&pgdt_io::Allowance::Stated(64 << 30), request);
    assert_eq!(carved.parallelism().memory_bytes(), Some(cap));

    let notes = plan(1 << 20, Parallelism::workers(8, cap), Some(64 << 20));
    let levers: Vec<_> = notes.iter().map(|note| (&note.kind, note.levers.clone())).collect();
    assert!(
        matches!(
            levers.as_slice(),
            [
                (PlanNoteKind::BatchSpanNarrowed { .. }, narrowed),
                (PlanNoteKind::ParallelismBudgetLimited { .. }, limited),
            ] if narrowed == &[SmallerReadChunk, FewerSubStreams]
                && limited == &[SmallerReadChunk]
        ),
        "{notes:?}"
    );

    // Four readers at the cap seat every one on a narrowed span, so the
    // span note is alone, and the chunk it offers is what widens it: each
    // reader's charge is a multiple of the chunk, and the span is what
    // the budget leaves over the readers.
    let (notes, narrowed) = planned(1 << 20, Parallelism::workers(4, cap), Some(64 << 20));
    let [note] = notes.as_slice() else { panic!("{notes:?}") };
    assert!(matches!(note.kind, PlanNoteKind::BatchSpanNarrowed { .. }), "{note:?}");
    assert_eq!(note.levers, [SmallerReadChunk, FewerSubStreams]);
    assert!(note.message().contains("a smaller read chunk"), "{}", note.message());
    let (_, widened) = planned(256 << 10, Parallelism::workers(4, cap), Some(64 << 20));
    assert!(widened > narrowed, "{widened:?} against {narrowed:?}");
}

/// **The count note offers the chunk for the span only where the span
/// sits on the chunk floor.** Over a source taking the default advice —
/// one partition at no reader cost, `RemoteSource`'s — the chunk sizes no
/// reader, so what it can move is the span alone: a span floored on the
/// chunk lists it and the sentence names it, a span stated below the
/// chunk is charged as stated and does neither. The premise is checked
/// too: a smaller chunk lowers the first charged span and leaves the
/// second where it was.
#[test]
fn the_count_note_offers_the_chunk_only_where_the_span_sits_on_it() {
    use PlanLever::{LargerAllowance, SmallerReadChunk};
    use std::future::Future;
    use std::time::SystemTime;
    struct Unadvised;
    impl ByteRangeSource for Unadvised {
        fn read_range(
            &self,
            _offset: u64,
            _len: usize,
        ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
            Box::pin(async { Ok(Bytes::new()) })
        }
        fn extent(&self) -> Pin<Box<dyn Future<Output = Result<pgdt_io::Extent>> + Send + '_>> {
            Box::pin(async { Ok(pgdt_io::Extent::Exact(45)) })
        }
        fn modified(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
            Box::pin(async { Ok(None) })
        }
    }
    let block = CopyBlock {
        header: pgdt_core::parse_copy_header(b"COPY public.t (a, b) FROM stdin;").unwrap(),
        header_offset: 0,
        data_offset: 33,
        terminator_offset: 42,
        end_offset: 45,
        row_count: 2,
        partition_root: None,
        statistics: None,
        statistics_declined: None,
        ignored_refusals: None,
        checked_in_full: false,
        census: Some(pgdt_model::BlockCensus::default()),
    };
    let chunk = 1 << 20;
    let plan = |chunk: usize, budget: u64, span: usize| {
        plan_partitions(
            &Unadvised,
            std::slice::from_ref(&block),
            &BTreeMap::new(),
            Parallelism::workers(8, budget),
            Some(span),
            chunk,
        )
    };
    let limited = |chunk: usize, budget: u64, span: usize| {
        let (_, notes, charged, _) = plan(chunk, budget, span);
        let note = notes
            .into_iter()
            .find(|note| matches!(note.kind, PlanNoteKind::ParallelismBudgetLimited { .. }))
            .unwrap_or_else(|| panic!("no count note at {budget} byte(s), span {span}"));
        (note, charged)
    };

    // Eight readers of no cost inside 4 MiB leave half a MiB apiece, below
    // the chunk, so a 64 MiB span is floored on it and four are seated.
    let (floored, charged) = limited(chunk, 4 << 20, 64 << 20);
    assert_eq!(charged, Some(chunk));
    assert_eq!(floored.levers, [LargerAllowance, SmallerReadChunk]);
    assert!(floored.message().contains("a smaller read chunk"), "{}", floored.message());
    assert!(plan(chunk / 2, 4 << 20, 64 << 20).2 < charged, "a smaller chunk lowers it");

    // A 256 KiB span stated below the chunk inside 1 MiB seats four too,
    // charged as stated whatever the chunk.
    let (stated, charged) = limited(chunk, 1 << 20, 256 << 10);
    assert_eq!(charged, Some(256 << 10));
    assert_eq!(stated.levers, [LargerAllowance]);
    assert!(!stated.message().contains("read chunk"), "{}", stated.message());
    assert!(stated.message().contains("a larger memory budget"), "{}", stated.message());
    assert_eq!(plan(chunk / 2, 1 << 20, 256 << 10).2, charged, "nothing the chunk moves");
}

/// Grouping keeps the pieces in file order and contiguous, which is what
/// makes concatenating the sub-streams equal the serial replay, and it
/// balances by bytes.
#[test]
fn sub_streams_get_contiguous_runs_balanced_by_bytes() {
    let block = CopyBlock {
        header: pgdt_core::parse_copy_header(b"COPY public.t (id) FROM stdin;").unwrap(),
        header_offset: 0,
        data_offset: 0,
        terminator_offset: 0,
        end_offset: 0,
        row_count: 0,
        partition_root: None,
        statistics: None,
        statistics_declined: None,
        ignored_refusals: None,
        checked_in_full: false,
        census: Some(pgdt_model::BlockCensus::default()),
    };
    let piece = |start: u64, limit: u64| Segment {
        block: block.clone(),
        start,
        limit,
        entry: SegmentEntry::Interior,
    };
    let segments = vec![piece(0, 10), piece(10, 20), piece(20, 30), piece(30, 40), piece(40, 50)];

    let groups = distribute(segments.clone(), 5);
    assert_eq!(groups.len(), 5);
    assert!(groups.iter().all(|g| g.len() == 1));

    // Contiguity and order: flattening the groups gives the input list.
    for streams in [1usize, 2, 3, 4, 5, 9] {
        let groups = distribute(segments.clone(), streams);
        assert!(groups.len() <= streams.max(1));
        assert!(groups.iter().all(|g| !g.is_empty()));
        let flat: Vec<u64> = groups.iter().flatten().map(|s| s.start).collect();
        assert_eq!(flat, vec![0, 10, 20, 30, 40], "streams {streams}");
    }

    // One piece carrying most of the bytes: two groups, two pieces each.
    let lopsided = vec![piece(0, 1), piece(1, 2), piece(2, 1002), piece(1002, 1003)];
    let groups = distribute(lopsided, 2);
    assert_eq!(groups.iter().map(Vec::len).collect::<Vec<_>>(), vec![2, 2]);
}

/// No blocks at all is one sub-stream that yields nothing, so a caller
/// never has to distinguish "no partitions" from "no rows".
#[test]
fn a_table_with_no_blocks_is_still_one_sub_stream() {
    let groups = distribute(Vec::new(), 8);
    assert_eq!(groups.len(), 1);
    assert!(groups[0].is_empty());
}
