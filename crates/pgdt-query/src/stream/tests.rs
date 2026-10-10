use super::*;
use pgdt_io::SCAN_CHUNK_DEFAULT_SIZE_BYTES;

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
/// asked it to read in (`pgdt_dump::ScanOptions::chunk_size_bytes`).
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

/// **The count note names a lever a plain source actually has** (`KD32`):
/// a source recommending no per-reader cost keeps
/// `pgdt_io::DEFAULT_MEMORY_BUDGET` however large an allowance is
/// stated (`docs/design/decisions.md`, "D83"), so a remedy offering the
/// budget alone is inert exactly where this note fires on one. Both arms
/// are checked, the span-carrying one and the bare, and the second half
/// is the premise itself — without it the first half is a string
/// asserting its own wording.
#[test]
fn the_count_note_names_a_lever_a_plain_source_has() {
    for span in [Some(1 << 20), None] {
        let note = PlanNote::parallelism_budget_limited(8, 2, 8 << 20, span, 64 << 20, false, true);
        let message = note.message();
        assert!(message.contains("a smaller read chunk"), "{message}");
    }

    // What makes the budget clause alone inert: twice the allowance is
    // the same budget, both of them `DEFAULT_MEMORY_BUDGET`.
    let budget = |allowance| Parallelism::within(8, None, allowance).memory_bytes();
    assert_eq!(budget(2 << 30), Some(pgdt_io::DEFAULT_MEMORY_BUDGET));
    assert_eq!(budget(4 << 30), budget(2 << 30));
}

/// **[`PlanNote::budget_bytes`] answers for exactly the notes whose
/// sentence quotes a budget**, which is what a caller appends its
/// provenance clause to (`docs/design/decisions.md`, "D64"). Asserted
/// against the message rather than against a second list of kinds: a note
/// added later that names a budget and forgets the accessor fails here,
/// where a list would agree with itself.
#[test]
fn a_note_quoting_a_budget_is_the_one_that_reports_it() {
    let memory_bytes = 64 << 20;
    let notes = [
        PlanNote::compressed_block_path_declined(9, 1 << 20, 2 << 20, memory_bytes, true),
        PlanNote::allocation_below_floor(8 << 20, memory_bytes, false, true),
        PlanNote::batch_span_narrowed(64 << 20, 1 << 20, 7, memory_bytes, false, true),
        PlanNote::parallelism_budget_limited(
            8,
            7,
            8 << 20,
            Some(1 << 20),
            memory_bytes,
            false,
            true,
        ),
        PlanNote {
            kind: PlanNoteKind::StatisticsPruned {
                skipped_groups: 1,
                groups: 2,
                skipped_bytes: 3,
                bytes: 4,
            },
            levers: Vec::new(),
        },
    ];
    for note in notes {
        let quoted = note.message().contains(&format!("memory budget of {memory_bytes} byte(s)"));
        assert_eq!(note.budget_bytes().is_some(), quoted, "{}", note.message());
        assert!(note.budget_bytes().is_none_or(|bytes| bytes == memory_bytes));
    }
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
        database: None,
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
        array_shapes: Some(Vec::new()),
        unrepresentable: Some(Vec::new()),
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
    assert_eq!(Parallelism::within(1, None, 64 << 30).memory_bytes(), Some(cap));

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
        fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
            Box::pin(async { Ok(45) })
        }
        fn modified(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
            Box::pin(async { Ok(None) })
        }
    }
    let block = CopyBlock {
        header: pgdt_core::parse_copy_header(b"COPY public.t (a, b) FROM stdin;").unwrap(),
        database: None,
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
        array_shapes: Some(Vec::new()),
        unrepresentable: Some(Vec::new()),
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

/// **A note is a `Warning` exactly where the budget declined something
/// asked of it**: a skip and a narrowed span are `Info`, as `pgdt` has
/// always printed them `note:`. A sink recovers the note itself.
#[test]
fn a_plan_note_warns_where_the_budget_declined_what_was_asked() {
    let memory_bytes = 64 << 20;
    let cases = [
        (PlanNote::compressed_block_path_declined(9, 1, 2, memory_bytes, true), Severity::Warning),
        (PlanNote::allocation_below_floor(8 << 20, memory_bytes, false, true), Severity::Warning),
        (
            PlanNote::parallelism_budget_limited(8, 7, 1, None, memory_bytes, false, true),
            Severity::Warning,
        ),
        (
            PlanNote::batch_span_narrowed(64 << 20, 1 << 20, 7, memory_bytes, false, true),
            Severity::Info,
        ),
        (
            PlanNote {
                kind: PlanNoteKind::StatisticsPruned {
                    skipped_groups: 1,
                    groups: 2,
                    skipped_bytes: 3,
                    bytes: 4,
                },
                levers: Vec::new(),
            },
            Severity::Info,
        ),
    ];
    for (note, severity) in cases {
        let finding: &dyn Finding = &note;
        assert_eq!(finding.severity(), severity, "{}", finding.message());
        assert_eq!(finding.as_any().downcast_ref::<PlanNote>(), Some(&note));
    }
}

/// The throttle's whole rule, over measured quantities rather than a
/// clock: a save that cost nothing never blocks another, and a save that
/// cost something blocks the next one until the scan has done `K` times
/// that much work.
#[test]
fn a_save_is_due_once_the_scan_has_outrun_the_last_ones_cost() {
    let ms = Duration::from_millis;

    assert!(SaveThrottle::due_after(Duration::ZERO, Duration::ZERO), "a free save never waits");
    assert!(SaveThrottle::due_after(ms(1), Duration::ZERO));

    // K = 20: 10ms of saving buys 200ms of silence.
    assert!(!SaveThrottle::due_after(ms(199), ms(10)));
    assert!(SaveThrottle::due_after(ms(200), ms(10)));
    assert!(SaveThrottle::due_after(ms(1000), ms(10)));

    // Blocks far apart and saves far cheaper than they are: untouched.
    assert!(SaveThrottle::due_after(Duration::from_secs(45), ms(300)));

    // And the pathological one: a cache expensive enough that saving it
    // every block is the scan.
    assert!(!SaveThrottle::due_after(ms(11), ms(10)));
}

/// A `last_cost` big enough to overflow `Duration * u32` saturates instead
/// of panicking. Unreachable in practice — it takes a save costing a
/// twentieth of `Duration::MAX` — but the multiplication is on the hot
/// path of every block.
#[test]
fn an_absurd_save_cost_saturates_rather_than_panicking() {
    assert!(!SaveThrottle::due_after(Duration::MAX / 2, Duration::MAX));
}

/// **The guard no caller can trip, pinned so it stays that way.**
/// [`resolve_block`] refuses `Typed` resolution against metadata with no
/// complete entry for the block's database, which every call site
/// satisfies by construction — so `Error::MetadataNotScanned` is
/// unreachable but through a hand-built `DumpIndex`, and this is what stands
/// between a future reordering and a silently wrongly-typed row.
#[test]
fn resolving_a_block_against_a_database_the_metadata_lacks_refuses() {
    use pgdt_model::DatabaseMetadata;

    let header = pgdt_core::parse_copy_header(b"COPY public.t (id) FROM stdin;")
        .expect("a well-formed header");
    let first = DatabaseMetadata {
        name: Some("first".to_string()),
        preamble_complete: true,
        server_version: None,
        pg_dump_version: None,
        extensions: Vec::new(),
        types: Vec::new(),
        collations: Vec::new(),
        tables: Default::default(),
    };
    let metadata = DumpMetadata { databases: vec![first] };

    let err = resolve_block(
        &header,
        Some(&metadata),
        Some("second"),
        SchemaMode::Typed,
        &[],
        &[],
        ComparisonSemantics::Postgres,
    )
    .expect_err("the metadata has no entry for `second`");
    assert!(
        matches!(&err, Error::MetadataNotScanned { database } if database.as_deref() == Some("second")),
        "{err:?}"
    );

    // The same block in a database the metadata covers resolves, so the
    // refusal is about coverage and not about the lookup failing.
    assert!(
        resolve_block(
            &header,
            Some(&metadata),
            Some("first"),
            SchemaMode::Typed,
            &[],
            &[],
            ComparisonSemantics::Postgres
        )
        .is_ok()
    );

    // `Strings` never looks, so it is never refused.
    assert!(
        resolve_block(
            &header,
            Some(&metadata),
            Some("second"),
            SchemaMode::Strings,
            &[],
            &[],
            ComparisonSemantics::Postgres
        )
        .is_ok()
    );
}

/// Grouping keeps the pieces in file order and contiguous, which is what
/// makes concatenating the sub-streams equal the serial replay, and it
/// balances by bytes.
#[test]
fn sub_streams_get_contiguous_runs_balanced_by_bytes() {
    let block = CopyBlock {
        header: pgdt_core::parse_copy_header(b"COPY public.t (id) FROM stdin;").unwrap(),
        database: None,
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
        array_shapes: Some(Vec::new()),
        unrepresentable: Some(Vec::new()),
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

/// A resumed stream keeps a segment while it owns a row at or past the
/// pause — the row starting exactly at its limit included — and enters
/// one begun before the pause on the LF ending the paused row.
#[test]
fn a_segment_resumes_from_the_row_its_pause_left() {
    let block = CopyBlock {
        header: pgdt_core::parse_copy_header(b"COPY public.t (id) FROM stdin;").unwrap(),
        database: None,
        header_offset: 10,
        data_offset: 40,
        terminator_offset: 400,
        end_offset: 403,
        row_count: 0,
        partition_root: None,
        statistics: None,
        statistics_declined: None,
        ignored_refusals: None,
        checked_in_full: false,
        array_shapes: Some(Vec::new()),
        unrepresentable: Some(Vec::new()),
    };
    let whole = Segment::over(&block, 39..403);
    assert_eq!((whole.start, whole.entry), (10, SegmentEntry::Header));
    let run = Segment::over(&block, 99..199);
    assert_eq!((run.start, run.entry), (99, SegmentEntry::Interior));
    let at = |segment: &Segment, offset| {
        segment.clone().resumed_at(offset).map(|s| (s.start, s.limit, s.entry))
    };

    assert_eq!(at(&run, 0), Some((99, 199, SegmentEntry::Interior)));
    assert_eq!(at(&run, 60), Some((99, 199, SegmentEntry::Interior)));
    assert_eq!(at(&run, 100), Some((99, 199, SegmentEntry::Interior)));
    assert_eq!(at(&run, 150), Some((149, 199, SegmentEntry::Interior)));
    assert_eq!(at(&run, 199), Some((198, 199, SegmentEntry::Interior)));
    assert_eq!(at(&run, 200), None);
    assert_eq!(at(&whole, 10), Some((10, 403, SegmentEntry::Header)));
    assert_eq!(at(&whole, 120), Some((119, 403, SegmentEntry::Interior)));
}

/// No blocks at all is one sub-stream that yields nothing, so a caller
/// never has to distinguish "no partitions" from "no rows".
#[test]
fn a_table_with_no_blocks_is_still_one_sub_stream() {
    let groups = distribute(Vec::new(), 8);
    assert_eq!(groups.len(), 1);
    assert!(groups[0].is_empty());
}

/// A sub-stream's resume token is stamped with the partition it came out
/// of, so "resuming a partition is unsupported" is an error rather than a
/// silent superset of the rows that partition had left.
#[test]
fn a_partitions_fingerprint_differs_from_the_whole_streams() {
    let options = QueryOptions::default();
    let whole = query_fingerprint("public.t", &options, None);
    let first = query_fingerprint("public.t", &options, Some((0, 4)));
    let second = query_fingerprint("public.t", &options, Some((1, 4)));
    let of_two = query_fingerprint("public.t", &options, Some((0, 2)));
    assert_ne!(whole, first);
    assert_ne!(first, second);
    assert_ne!(first, of_two);
}

/// One table in two blocks naming its columns in different orders.
fn two_blocks(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("two_blocks.sql");
    std::fs::write(
        &path,
        "COPY public.t (a, b) FROM stdin;\n1\tx\n2\t\\N\n\\.\n\n\
             COPY public.t (b, a) FROM stdin;\ny\t3\n\\.\n",
    )
    .unwrap();
    path
}

/// **A replay planned for `table_stream` or `table_stream_partitions`
/// keeps no block's statistics**: once its plan and segments
/// exist, the matched blocks the map handed over are the only holders of
/// each block's `Arc`, so dropping them frees the statistics before a row
/// is read, on the partitioned path and the serial one alike.
#[tokio::test]
async fn a_replay_holds_no_block_s_statistics() {
    use pgdt_io::LocalFileSource;
    use pgdt_model::{BlockStatistics, GroupSizing};

    let dir = tempfile::tempdir().unwrap();
    let source = LocalFileSource::open(two_blocks(dir.path())).unwrap();
    let query_options = QueryOptions {
        scan_extent: ScanExtent::Full,
        parallelism: Parallelism::workers(4, pgdt_io::DEFAULT_MEMORY_BUDGET),
        ..QueryOptions::default()
    };
    let mapped = map_for_query(
        &source,
        "public.t",
        &ScanOptions::default(),
        &query_options,
        &CacheMode::DISABLED,
        &SourceWatch::open(&source, StrictIdentity::ADVISORY).await.unwrap(),
    )
    .await
    .unwrap();
    let mut matches = mapped.matches;
    assert_eq!(matches.len(), 2);
    for block in &mut matches {
        block.statistics = Some(Arc::new(BlockStatistics {
            group_size: 1 << 20,
            sizing: GroupSizing::Stated,
            groups: Vec::new(),
            columns: vec![None; block.header.columns.len()],
        }));
    }
    let held: Vec<_> =
        matches.iter().map(|block| Arc::clone(block.statistics.as_ref().unwrap())).collect();

    let PlannedReplay { plan, groups, .. } =
        plan_replay(&source, &matches, mapped.metadata, ScanOptions::default(), query_options)
            .unwrap();
    let serial: Vec<Segment> = matches.iter().flat_map(|block| plan.segments(block)).collect();
    assert!(groups.iter().flatten().chain(&serial).all(|s| s.block.statistics.is_none()));
    drop(matches);
    for statistics in &held {
        assert_eq!(Arc::strong_count(statistics), 1, "only the test's own copy remains");
    }
}

/// **A dynamic filter's stop is armed only in a group a row of which can
/// pass it**: under `id < 150` over ascending ids, of the groups the
/// state keeps only the one whose maximum reaches the bound is asked
/// row by row, so the groups before it pay nothing for the stop.
#[tokio::test]
async fn a_dynamic_stop_is_armed_only_where_a_row_can_pass_it() {
    use crate::predicate::Predicate;
    use crate::statistics::{StatisticsRequest, StatisticsSelection};
    use pgdt_io::LocalFileSource;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ascending.sql");
    let rows: String = (1..=200).map(|id| format!("{id}\n")).collect();
    std::fs::write(
        &path,
        format!(
            "CREATE TABLE public.t (\n    id integer\n);\n\n\
                 COPY public.t (id) FROM stdin;\n{rows}\\.\n"
        ),
    )
    .unwrap();
    let source = LocalFileSource::open(&path).unwrap();
    let request = StatisticsRequest {
        selection: StatisticsSelection::DATA,
        group_size: Some(std::num::NonZeroU64::new(64).unwrap()),
        ..StatisticsRequest::DATA
    };
    let run =
        map_file(&source, &ScanOptions::default(), &CacheMode::DISABLED, &request).await.unwrap();
    let block = run.index.blocks_for("public.t").next().unwrap();
    let statistics = Arc::clone(block.statistics.as_ref().unwrap());
    let metadata = run.index.metadata.as_ref();
    let census = vec![ArrayShape::default(); block.header.columns.len()];
    let full = resolve_block(
        &block.header,
        metadata,
        None,
        SchemaMode::Typed,
        &census,
        &[],
        ComparisonSemantics::Postgres,
    )
    .unwrap();
    let below = Expr::all([Predicate {
        column: "id".into(),
        op: PredicateOp::Lt,
        value: Some("150".into()),
    }]);
    let filter =
        resolve_expr(&below, &full, block.header_offset, ComparisonSemantics::Postgres).unwrap();
    let reading = pgdt_model::StatisticsView::Every;
    let mut pruning =
        DynamicPruning::new(block, Arc::clone(&statistics), metadata, reading).unwrap();
    pruning.read(Arc::new(filter));

    let bounds = statistics.columns[0].as_ref().unwrap().bounds.as_ref().unwrap();
    let (mut armed, mut reaching) = (Vec::new(), Vec::new());
    for group in 0..pruning.groups() {
        if !pruning.keeps(group) {
            continue;
        }
        pruning.arm(group);
        armed.push(pruning.stop().is_some());
        let max: i64 = bounds.groups[group].as_ref().unwrap().max.parse().unwrap();
        reaching.push(max >= 150);
    }
    assert!(armed.len() > 3, "{armed:?}");
    assert_eq!(armed, reaching);
    assert_eq!(armed.iter().filter(|&&armed| armed).count(), 1, "{armed:?}");
}

/// **A block prunes in the view its query reads its statistics in**: over
/// a `date` column, one value a group, a value past the format spec is
/// its rank in every value's view and a NULL in the representable one, and
/// one past the calendar is a NULL in the displayable one too — so a
/// bound rules each out, or `IS NULL` keeps it, by the view — and a column
/// sorted but for an infinity is sorted, and stops a read, only where the
/// infinity is a NULL.
#[tokio::test]
async fn a_block_prunes_in_the_view_its_query_reads() {
    use crate::predicate::Predicate;
    use crate::statistics::StatisticsRequest;
    use pgdt_io::LocalFileSource;
    use pgdt_model::StatisticsView;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dates.sql");
    std::fs::write(
        &path,
        "CREATE TABLE public.t (\n    d date\n);\n\n\
             CREATE TABLE public.u (\n    d date\n);\n\n\
             COPY public.t (d) FROM stdin;\n\
             2024-01-01\ninfinity\n-infinity\n262143-01-01\n\\N\n\\.\n\n\
             COPY public.u (d) FROM stdin;\n\
             2020-01-01\n2021-01-01\n2022-01-01\n-infinity\n\\.\n",
    )
    .unwrap();
    let source = LocalFileSource::open(&path).unwrap();
    // One row a group: a row's first byte is all a group holds.
    let request = StatisticsRequest {
        group_size: Some(std::num::NonZeroU64::new(1).unwrap()),
        ..StatisticsRequest::DATA
    };
    let run =
        map_file(&source, &ScanOptions::default(), &CacheMode::DISABLED, &request).await.unwrap();
    let metadata = run.index.metadata.as_ref();
    let filter = |table: &str, op: PredicateOp, value: Option<&str>| {
        let block = run.index.blocks_for(table).next().unwrap();
        let census = vec![ArrayShape::default(); block.header.columns.len()];
        let full = resolve_block(
            &block.header,
            metadata,
            None,
            SchemaMode::Typed,
            &census,
            &[],
            ComparisonSemantics::Postgres,
        )
        .unwrap();
        let term = Predicate { column: "d".into(), op, value: value.map(str::to_string) };
        let expr = Expr::all([term]);
        (
            block,
            resolve_expr(&expr, &full, block.header_offset, ComparisonSemantics::Postgres).unwrap(),
        )
    };
    // The values each view's pruning keeps a group of, in file order.
    let kept = |table: &str, op: PredicateOp, value: Option<&str>, view: StatisticsView| {
        let (block, filter) = filter(table, op, value);
        let pruning = prune_block(block, &filter, metadata, view).unwrap();
        let statistics = block.statistics.as_ref().unwrap();
        let values = ["2024-01-01", "infinity", "-infinity", "262143-01-01", "NULL"];
        let holding: Vec<usize> =
            (0..statistics.groups.len()).filter(|&g| statistics.groups[g].rows > 0).collect();
        assert_eq!(holding.len(), values.len(), "one row a group");
        holding
            .iter()
            .zip(values)
            .filter(|&(&g, _)| pruning.kept_groups[g])
            .map(|(_, value)| value)
            .collect::<Vec<_>>()
    };
    use StatisticsView::{Displayable, Every, Representable};
    let later = |view| kept("public.t", PredicateOp::Gt, Some("2030-01-01"), view);
    assert_eq!(later(Every), ["infinity", "262143-01-01"]);
    assert_eq!(later(Representable), ["262143-01-01"]);
    assert_eq!(later(Displayable), Vec::<&str>::new());
    let null = |view| kept("public.t", PredicateOp::IsNull, None, view);
    assert_eq!(null(Every), ["NULL"]);
    assert_eq!(null(Representable), ["infinity", "-infinity", "NULL"]);
    assert_eq!(null(Displayable), ["infinity", "-infinity", "262143-01-01", "NULL"]);

    let stops = |view| {
        let (block, filter) = filter("public.u", PredicateOp::Lt, Some("2021-06-01"));
        prune_block(block, &filter, metadata, view).unwrap().stop.is_some()
    };
    assert!(!stops(Every), "-infinity closes the column out of order");
    assert!(stops(Representable) && stops(Displayable));
}

/// **The plan resolves every block before any is read, and activation
/// takes that resolution rather than making its own.** A block's filter
/// or projection refusal refuses the plan.
#[tokio::test]
async fn the_filter_is_resolved_once_per_block_at_plan_time() {
    use crate::predicate::Predicate;
    use pgdt_io::LocalFileSource;

    let dir = tempfile::tempdir().unwrap();
    let source = LocalFileSource::open(two_blocks(dir.path())).unwrap();
    let not_null = |column: &str| {
        Expr::all([Predicate { column: column.into(), op: PredicateOp::IsNotNull, value: None }])
    };
    let query_options = QueryOptions {
        filter: not_null("a"),
        projection: Some(vec!["a".into()]),
        // Past the first block, which would otherwise settle the table.
        scan_extent: ScanExtent::Full,
        ..QueryOptions::default()
    };
    let mapped = map_for_query(
        &source,
        "public.t",
        &ScanOptions::default(),
        &query_options,
        &CacheMode::DISABLED,
        &SourceWatch::open(&source, StrictIdentity::ADVISORY).await.unwrap(),
    )
    .await
    .unwrap();
    let [first, second] = &mapped.matches[..] else { panic!("two blocks: {:?}", mapped.matches) };
    let plan_for = |query_options: &QueryOptions| {
        ReplayPlan::new(
            ScanOptions::default(),
            query_options.clone(),
            &mapped.matches,
            mapped.metadata.clone(),
        )
    };

    // A filter no block can answer refuses the plan, naming the first.
    let refusing = QueryOptions { filter: not_null("c"), ..query_options.clone() };
    assert!(matches!(
        plan_for(&refusing).map(|_| ()),
        Err(Error::UnknownPredicateColumn { header_offset, .. })
            if header_offset == first.header_offset
    ));

    // So does a projection naming a column no block carries.
    let refusing = QueryOptions { projection: Some(vec!["c".into()]), ..query_options.clone() };
    assert!(matches!(
        plan_for(&refusing).map(|_| ()),
        Err(Error::UnknownProjectionColumn { header_offset, .. })
            if header_offset == first.header_offset
    ));

    let plan = plan_for(&query_options).unwrap();
    assert_eq!(
        plan.blocks.keys().copied().collect::<Vec<_>>(),
        [first.header_offset, second.header_offset]
    );

    // The planned entry is what resolving the block where it is reached
    // would have produced.
    let live = |block: &CopyBlock| {
        resolve_for_query(
            &block.header,
            block.header_offset,
            block.database.as_deref(),
            &query_options,
            mapped.metadata.as_ref(),
            &plan.table,
            block.unrepresentable.as_deref(),
        )
    };
    let planned = &plan.blocks[&first.header_offset];
    assert_eq!(format!("{planned:?}"), format!("{:?}", live(first).unwrap()));

    // The second block's `a` is its second field, and it is `a` that
    // feeds the projection.
    assert_eq!(plan.blocks[&second.header_offset].field_targets, [None, Some(0)]);

    // Activating the planned block hands out the plan's own tree.
    let open = |header: &CopyHeader, block: &CopyBlock| {
        activate(header.clone(), block.header_offset, block.database.clone(), &plan)
    };
    let ((_, _, _, filter, _), resolved, notes) = open(&first.header, first).unwrap();
    assert!(Arc::ptr_eq(&filter, &planned.filter));
    assert_eq!((resolved, notes), (planned.resolved.clone(), planned.notes.clone()));

    // An activation the entry does not describe resolves for itself.
    let ((_, _, _, other, _), _, _) = open(&second.header, first).unwrap_or_else(|e| panic!("{e}"));
    assert!(!Arc::ptr_eq(&other, &planned.filter));
}
