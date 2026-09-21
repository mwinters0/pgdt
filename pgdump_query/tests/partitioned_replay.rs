//! Partitioned replay: one map, N sub-streams
//! (`docs/design/decisions.md`, "D51").
//!
//! **The whole file is one differential test in several shapes.** A
//! partitioned replay has no output of its own to assert against — its
//! contract is that it returns exactly what the serial [`table_stream`]
//! returns, in exactly that order, however the file was cut. So every test
//! here takes the serial rows as the oracle and varies one thing: the worker
//! count, the source's own partitioning advice (a plain file says anywhere, a
//! seekable `.xz` says block boundaries, a single-block `.xz` declines), the
//! filter, the projection, and the block count.
//!
//! What that buys over asserting fixed row lists is that the cut points move
//! with the worker count and land wherever they land — mid-row, mid-header,
//! two inside one row — and every one of those has to come back with the same
//! rows. The `edge_cases` dump is small enough that a high worker count cuts
//! it into pieces of a few bytes, which is where the tiling rules earn their
//! keep.

use std::path::Path;
use std::process::Command;

use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    ByteRangeSource, DEFAULT_MEMORY_BUDGET, Expr, LocalFileSource, Parallelism, PlanNoteKind,
    Predicate, PredicateOp, QueryOptions, ScanOptions, XzSource, table_stream,
    table_stream_partitions,
};

mod common;
use common::{edge_cases, partitions_fixture, rows_of, types_fixture};

type Rows = Vec<Vec<Option<String>>>;

/// Every row a serial `table_stream` yields, in file order — the oracle.
async fn serial_rows(source: &dyn ByteRangeSource, table: &str, options: QueryOptions) -> Rows {
    let mut stream =
        table_stream(source, table, ScanOptions::default(), options, None, CacheMode::DISABLED);
    let mut rows = Rows::new();
    while let Some(batch) = stream.next().await {
        rows.extend(rows_of(&batch.unwrap()));
    }
    rows
}

/// What one concurrent reader costs `source` resident at the chunk size the
/// query announces — the first term of the sub-stream divisor
/// (`docs/design/decisions.md`, "D4").
///
/// **Asked of the source rather than restated as a chunk count.** A plain
/// file's partition is several read chunks and the multiple is the library's
/// to choose, so a test that spelled the product out would be asserting the
/// constant rather than the arithmetic that divides by it. The read size is
/// announced first because the answer scales with it, exactly as the mapping
/// pass announces it before the plan is made — and it is the caller's own
/// `ScanOptions::chunk_size_bytes`, for the reason the span's floor is
/// (`docs/design/decisions.md`, "D84").
fn partition_unit(source: &LocalFileSource, scan: &ScanOptions) -> u64 {
    source.hint_read_size(scan.chunk_size_bytes);
    source.partitions(0..1).partition_bytes()
}

/// Every row `jobs` sub-streams yield, drained one after another in the order
/// the split handed them back — which is file order, so this is directly
/// comparable to [`serial_rows`].
///
/// Draining them sequentially is deliberate: it is the arrangement the design
/// says must be indistinguishable from the serial path
/// (`docs/design/decisions.md`, "D51"), and running them concurrently would test
/// the executor rather than the split.
async fn partitioned_rows(
    source: &dyn ByteRangeSource,
    table: &str,
    options: QueryOptions,
    jobs: usize,
) -> (Rows, usize) {
    let options = QueryOptions { parallelism: Parallelism::workers(jobs, 1 << 30), ..options };
    let streams = table_stream_partitions(
        source,
        table,
        ScanOptions::default(),
        options,
        CacheMode::DISABLED,
    )
    .await
    .unwrap();
    let count = streams.len();
    let mut rows = Rows::new();
    for mut stream in streams {
        while let Some(batch) = stream.next().await {
            rows.extend(rows_of(&batch.unwrap()));
        }
    }
    (rows, count)
}

/// The central property, on a plain source, which advises `Anywhere` — so the
/// cuts are arbitrary byte offsets and land mid-row on almost every worker
/// count. No row is dropped, none is emitted twice, and the order is the
/// file's.
#[tokio::test]
async fn a_plain_source_splits_into_the_same_rows_in_the_same_order() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let expected = serial_rows(&source, "public.widgets", QueryOptions::default()).await;
    assert!(!expected.is_empty(), "the oracle must have rows for this to mean anything");

    for jobs in [1usize, 2, 3, 4, 7, 8, 64] {
        let (rows, count) =
            partitioned_rows(&source, "public.widgets", QueryOptions::default(), jobs).await;
        assert_eq!(rows, expected, "jobs {jobs}");
        assert!(count <= jobs, "jobs {jobs} gave {count} sub-streams");
        if jobs > 1 {
            assert!(count > 1, "jobs {jobs} must actually split a 371-byte data region");
        }
    }
}

/// The sub-streams are polled **interleaved** rather than one after another,
/// which is the arrangement a caller running them concurrently produces — and
/// the rows are still each partition's own, so nothing about a sub-stream's
/// state travels through the shared source.
///
/// It is `join_all` rather than spawned threads because a `TableStream`
/// borrows the source: what this exercises is concurrent *reads* against one
/// source, which is where the compressed source's own concurrency lives.
#[tokio::test]
async fn sub_streams_polled_interleaved_keep_their_own_rows() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let serial = serial_rows(&source, "public.t_int", QueryOptions::default()).await;

    let options = QueryOptions {
        max_rows: 1,
        max_bytes: None,
        parallelism: Parallelism::workers(4, 1 << 30),
        ..Default::default()
    };
    let streams = table_stream_partitions(
        &source,
        "public.t_int",
        ScanOptions::default(),
        options,
        CacheMode::DISABLED,
    )
    .await
    .unwrap();

    let drained = futures::future::join_all(streams.into_iter().map(|mut stream| async move {
        let mut rows = Rows::new();
        while let Some(batch) = stream.next().await {
            rows.extend(rows_of(&batch.unwrap()));
        }
        rows
    }))
    .await;
    assert_eq!(drained.concat(), serial);
}

/// `Parallelism::Serial` is one sub-stream and one only — the serial replay,
/// reached as a property of the value rather than by a branch
/// (`docs/design/decisions.md`, "I/O, memory and parallelism").
#[tokio::test]
async fn serial_parallelism_is_exactly_one_sub_stream() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let streams = table_stream_partitions(
        &source,
        "public.widgets",
        ScanOptions::default(),
        QueryOptions::default(),
        CacheMode::DISABLED,
    )
    .await
    .unwrap();
    assert_eq!(streams.len(), 1);

    // `--jobs 1` is the same value, so it is the same one sub-stream.
    let (_, count) = partitioned_rows(&source, "public.widgets", QueryOptions::default(), 1).await;
    assert_eq!(count, 1);
}

/// A stated `--jobs` the memory budget cannot afford in full says so,
/// naming the numbers that would raise it — **and it says it only after the
/// batch span has been spent** (`docs/design/decisions.md`, "D84"). The
/// shipped CLI defaults are exactly this case: `QueryOptions::max_source_span`'s
/// 64 MiB alone meets `DEFAULT_MEMORY_BUDGET`'s 64 MiB, so the eight workers
/// asked for are unaffordable at the stated span, the span narrows to the
/// announced read chunk, and what that buys — seven of the eight — is what
/// the shortfall is reported against.
///
/// Two notes, in that order, and the arithmetic of both is pinned: the span
/// at its floor beside an 8 MiB footprint is 9 MiB a sub-stream, which
/// `DEFAULT_MEMORY_BUDGET` affords seven of.
#[tokio::test]
async fn a_budget_bound_worker_count_announces_why() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let scan = ScanOptions::default();
    let floor = scan.chunk_size_bytes as u64;
    let options = QueryOptions {
        parallelism: Parallelism::workers(8, DEFAULT_MEMORY_BUDGET),
        ..Default::default()
    };
    let streams = table_stream_partitions(
        &source,
        "public.widgets",
        scan.clone(),
        options.clone(),
        CacheMode::DISABLED,
    )
    .await
    .unwrap();
    let footprint = partition_unit(&source, &scan);
    let planned = (DEFAULT_MEMORY_BUDGET / (footprint + floor)) as usize;
    assert_eq!(planned, 7, "the floored span leaves room for seven of the eight");
    assert_eq!(streams.len(), planned, "the budget affords exactly this many sub-streams");
    let diagnostics = streams[0].plan_notes();
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    let PlanNoteKind::BatchSpanNarrowed { stated_bytes, planned_bytes, workers, memory_bytes } =
        &diagnostics[0].kind
    else {
        panic!("{diagnostics:?}")
    };
    assert_eq!(*stated_bytes, options.max_source_span.unwrap() as u64);
    assert_eq!(*planned_bytes, floor, "nothing narrows past the floor");
    assert_eq!(*workers, planned);
    assert_eq!(*memory_bytes, DEFAULT_MEMORY_BUDGET);
    let PlanNoteKind::ParallelismBudgetLimited {
        requested,
        planned: planned_workers,
        footprint: charged_footprint,
        max_source_span,
        memory_bytes,
    } = &diagnostics[1].kind
    else {
        panic!("{diagnostics:?}")
    };
    assert_eq!(*requested, 8);
    assert_eq!(*planned_workers, planned);
    assert_eq!(*charged_footprint, footprint);
    assert_eq!(
        *max_source_span,
        Some(floor),
        "the shortfall is named against the span actually charged, not the one stated"
    );
    assert_eq!(*memory_bytes, DEFAULT_MEMORY_BUDGET);
    // Every sub-stream carries the same facts: the property being tested is
    // that the caller need not pick which sub-stream to ask.
    for stream in &streams {
        assert_eq!(stream.plan_notes(), diagnostics);
    }
}

/// **The below-reserve arrangement is pinned here, and it is what a 256 MiB
/// cgroup resolves to.** `limit − MEMORY_RESERVE` is zero at or under the
/// reserve, and a serial count means `ParallelismBudgetLimited` cannot fire —
/// `requested` is one and one is what runs — so without
/// [`PlanNoteKind::AllocationBelowFloor`] a user in a tight allocation is told
/// nothing at all (`docs/design/decisions.md`, "D3").
///
/// **The rows are the assertion beside it.** Three floors turn a budget of
/// zero into one reader on the streaming path, and a later change to any of
/// them must not silently make this something else: the note names the same
/// unit the source charges, and the answer is still the serial oracle's.
#[tokio::test]
async fn a_budget_below_one_readers_worth_says_the_allocation_bound_it() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let unit = partition_unit(&source, &ScanOptions::default());
    let options = QueryOptions { parallelism: Parallelism::workers(1, 0), ..Default::default() };
    let mut streams = table_stream_partitions(
        &source,
        "public.widgets",
        ScanOptions::default(),
        options,
        CacheMode::DISABLED,
    )
    .await
    .unwrap();
    assert_eq!(streams.len(), 1, "a budget of zero is one reader, not none");
    let notes = streams[0].plan_notes();
    assert_eq!(notes.len(), 1, "the count note cannot fire at a serial request: {notes:?}");
    let PlanNoteKind::AllocationBelowFloor { unit_bytes, memory_bytes } = &notes[0].kind else {
        panic!("{notes:?}")
    };
    assert_eq!(*memory_bytes, 0);
    assert_eq!(*unit_bytes, unit, "the note charges what the source charges");
    assert!(
        notes[0].message().contains("one-slot floor"),
        "the message names the arrangement: {}",
        notes[0].message()
    );

    let mut rows = Rows::new();
    while let Some(batch) = streams[0].next().await {
        rows.extend(rows_of(&batch.unwrap()));
    }
    assert_eq!(rows, serial_rows(&source, "public.widgets", QueryOptions::default()).await);
}

/// A budget that affords a whole reader is silent about the floor: the note is
/// for the starved allocation, not for every plan that could have had more.
#[tokio::test]
async fn a_budget_that_affords_one_reader_says_nothing_about_a_floor() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let unit = partition_unit(&source, &ScanOptions::default());
    let options = QueryOptions { parallelism: Parallelism::workers(1, unit), ..Default::default() };
    let streams = table_stream_partitions(
        &source,
        "public.widgets",
        ScanOptions::default(),
        options,
        CacheMode::DISABLED,
    )
    .await
    .unwrap();
    assert!(streams[0].plan_notes().is_empty(), "{:?}", streams[0].plan_notes());
}

/// A budget that affords every requested worker says nothing: the diagnostic
/// is for the case the budget actually declined a worker, not a running
/// commentary on every plan.
#[tokio::test]
async fn a_budget_that_affords_every_worker_is_silent() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let options =
        QueryOptions { parallelism: Parallelism::workers(4, 1 << 30), ..Default::default() };
    let streams = table_stream_partitions(
        &source,
        "public.t_int",
        ScanOptions::default(),
        options,
        CacheMode::DISABLED,
    )
    .await
    .unwrap();
    assert!(streams.iter().all(|s| s.plan_notes().is_empty()));
}

/// A worker count above one really does hand out more than one sub-stream on
/// a file that can be split — otherwise every equality above would be
/// comparing the serial path to itself.
#[tokio::test]
async fn a_split_actually_happens() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let expected = serial_rows(&source, "public.t_int", QueryOptions::default()).await;
    let (rows, count) = partitioned_rows(&source, "public.t_int", QueryOptions::default(), 4).await;
    assert!(count > 1, "four workers over a splittable block must yield more than one sub-stream");
    assert_eq!(rows, expected);
}

/// A table whose data is several `COPY` blocks (I2) partitions across the
/// blocks as well as inside them, and still comes back in file order.
#[tokio::test]
async fn a_multi_block_table_splits_across_and_inside_its_blocks() {
    for version in [13, 16, 18] {
        let source = LocalFileSource::open(partitions_fixture(version, "default")).unwrap();
        let options =
            QueryOptions { scan_extent: pgdump_query::ScanExtent::Full, ..Default::default() };
        let expected = serial_rows(&source, "public.feel", options.clone()).await;
        assert!(!expected.is_empty(), "v{version}: the oracle must have rows");
        for jobs in [2usize, 3, 8] {
            let (rows, count) =
                partitioned_rows(&source, "public.feel", options.clone(), jobs).await;
            assert_eq!(rows, expected, "v{version}, jobs {jobs}");
            assert!(count > 1, "v{version}, jobs {jobs}: two blocks must reach two sub-streams");
        }
    }
}

/// The filter and the projection are resolved per block, and a sub-stream
/// that starts *inside* a block resolves them off the map's own copy of the
/// header rather than off a header line it never sees. Both have to give the
/// serial answer.
#[tokio::test]
async fn a_filtered_and_projected_query_splits_to_the_same_rows() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let options = QueryOptions {
        projection: Some(vec!["v_integer".to_string()]),
        filter: Expr::Term(Predicate {
            column: "v_integer".to_string(),
            op: PredicateOp::IsNotNull,
            value: None,
        }),
        ..Default::default()
    };
    let expected = serial_rows(&source, "public.t_int", options.clone()).await;
    for jobs in [2usize, 5, 16] {
        let (rows, _) = partitioned_rows(&source, "public.t_int", options.clone(), jobs).await;
        assert_eq!(rows, expected, "jobs {jobs}");
    }
}

/// A batch size small enough to flush several times inside one sub-stream
/// still tiles: a piece's last row is flushed at the piece's limit rather
/// than at a `CopyEnd` it never reaches.
#[tokio::test]
async fn a_small_batch_size_flushes_at_a_piece_boundary_without_losing_a_row() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let options = QueryOptions { max_rows: 1, max_bytes: None, ..Default::default() };
    let expected = serial_rows(&source, "public.t_int", options.clone()).await;
    for jobs in [2usize, 3, 9] {
        let (rows, _) = partitioned_rows(&source, "public.t_int", options.clone(), jobs).await;
        assert_eq!(rows, expected, "jobs {jobs}");
    }
}

/// **The key a caller merges the sub-streams back on.** A `RecordBatch`
/// carries no position, so `TableStream::batch_source_offset` is what
/// `pgdt query` sorts one-batch-per-partition on
/// (`docs/design/decisions.md`, "D51").
///
/// The sub-streams are drained **round-robin**, which is the arrival order a
/// caller polling them concurrently sees — and it is asserted here to be
/// something other than file order, so that the sort below is doing work
/// rather than re-confirming an order the draining already had. That is why
/// the fixture is the eight-row `edge_cases` widgets against two sub-streams
/// rather than a table with one row per partition, where round-robin *is*
/// file order and the assertion would hold for the wrong reason.
#[tokio::test]
async fn batches_sorted_on_their_source_offset_are_the_serial_order() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let options = QueryOptions { max_rows: 1, max_bytes: None, ..Default::default() };
    let expected = serial_rows(&source, "public.widgets", options.clone()).await;
    assert!(expected.len() > 3, "the oracle needs several rows per partition");

    let options = QueryOptions { parallelism: Parallelism::workers(2, 1 << 30), ..options };
    let mut streams = table_stream_partitions(
        &source,
        "public.widgets",
        ScanOptions::default(),
        options,
        CacheMode::DISABLED,
    )
    .await
    .unwrap();
    assert!(streams.len() > 1, "one sub-stream would make this the serial path");

    // One batch per live sub-stream per pass, which is the merge's own fill
    // round: nothing is ever held beyond a batch apiece.
    let mut arrived: Vec<(u64, Rows)> = Vec::new();
    let mut live = vec![true; streams.len()];
    while live.iter().any(|l| *l) {
        for (index, stream) in streams.iter_mut().enumerate() {
            if !live[index] {
                continue;
            }
            match stream.next().await {
                Some(batch) => {
                    arrived.push((stream.batch_source_offset(), rows_of(&batch.unwrap())));
                }
                None => live[index] = false,
            }
        }
    }

    let unsorted: Rows = arrived.iter().flat_map(|(_, rows)| rows.clone()).collect();
    assert_ne!(unsorted, expected, "round-robin arrival was already file order");

    arrived.sort_by_key(|(offset, _)| *offset);
    let offsets: Vec<u64> = arrived.iter().map(|(offset, _)| *offset).collect();
    assert!(
        offsets.windows(2).all(|pair| pair[0] < pair[1]),
        "two batches claim one start, so the key does not order them: {offsets:?}"
    );
    let merged: Rows = arrived.into_iter().flat_map(|(_, rows)| rows).collect();
    assert_eq!(merged, expected);
}

/// The offset a batch reports is **where its first row starts**, not where
/// its sub-stream stopped: the byte before it is the LF that ended the
/// previous line. Checked against the file's own bytes, since nothing else
/// in the library would notice the two being swapped — they order identically
/// within one stream.
#[tokio::test]
async fn a_reported_offset_is_the_start_of_a_row() {
    let path = types_fixture(16, "default");
    let bytes = std::fs::read(&path).unwrap();
    let source = LocalFileSource::open(&path).unwrap();
    let options = QueryOptions { max_rows: 1, max_bytes: None, ..Default::default() };
    let mut stream = table_stream(
        &source,
        "public.t_int",
        ScanOptions::default(),
        options,
        None,
        CacheMode::DISABLED,
    );

    let mut seen = 0usize;
    let mut previous: Option<u64> = None;
    while let Some(batch) = stream.next().await {
        batch.unwrap();
        let offset = stream.batch_source_offset();
        assert!(offset > 0 && (offset as usize) < bytes.len(), "offset {offset} is off the file");
        assert_eq!(bytes[offset as usize - 1], b'\n', "offset {offset} does not follow an LF");
        if let Some(previous) = previous {
            assert!(previous < offset, "{previous} then {offset}");
        }
        previous = Some(offset);
        seen += 1;
    }
    assert!(seen > 1, "one batch would say nothing about the ordering");
}

/// A table the dump does not carry is one sub-stream that yields nothing, so
/// a caller never has to tell "no partitions" from "no rows".
#[tokio::test]
async fn a_table_that_is_not_in_the_dump_is_one_empty_sub_stream() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let (rows, count) =
        partitioned_rows(&source, "public.not_a_table", QueryOptions::default(), 8).await;
    assert_eq!(count, 1);
    assert!(rows.is_empty());
}

/// A sub-stream's resume token is refused by [`table_stream`] rather than
/// silently resuming as a whole stream would — which would hand back every
/// row from that offset on, not the ones that partition had left
/// (`docs/design/decisions.md`, "D51").
#[tokio::test]
async fn a_sub_streams_resume_token_is_refused_by_the_whole_stream() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let options = QueryOptions {
        max_rows: 1,
        max_bytes: None,
        parallelism: Parallelism::workers(4, 1 << 30),
        ..Default::default()
    };
    let mut streams = table_stream_partitions(
        &source,
        "public.t_int",
        ScanOptions::default(),
        options.clone(),
        CacheMode::DISABLED,
    )
    .await
    .unwrap();
    let first = &mut streams[0];
    first.next().await.expect("the first sub-stream has rows").unwrap();
    let token = first.resume_token();

    let mut resumed = table_stream(
        &source,
        "public.t_int",
        ScanOptions::default(),
        options,
        Some(token),
        CacheMode::DISABLED,
    );
    assert!(
        matches!(resumed.next().await, Some(Err(pgdump_query::Error::ResumeQueryMismatch))),
        "a partition's token must not resume as a whole stream"
    );
}

/// Compress `path` into `dir` with `args`, the way `tests/cache.rs` does —
/// `xz` is `mise`-pinned nowhere, so a missing binary fails loudly rather
/// than the test skipping (`docs/design/roadmap.md`, "A test may assume the
/// tools `mise` pins").
fn xz_compress(path: &Path, args: &[&str]) -> tempfile::NamedTempFile {
    let out = Command::new("xz")
        .args(args)
        .arg("-c")
        .arg(path)
        .output()
        .expect("`xz` is not runnable, so this test cannot build its fixture; install it.");
    assert!(out.status.success(), "xz failed: {}", String::from_utf8_lossy(&out.stderr));
    let mut compressed = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut compressed, &out.stdout).unwrap();
    compressed
}

/// **The same code for a plain source and a compressed one.** A seekable
/// `.xz` advises cuts at its own block boundaries, so the pieces are
/// block-aligned rather than evenly sized — and the rows are still the plain
/// file's, in the plain file's order.
///
/// **64, not `tests/cache.rs`'s 512**: what has to fall inside the range is a
/// block boundary *within the queried block's data*, and `public.widgets`
/// occupies bytes 1,607–1,978 of this 2,352-byte fixture — which the 512-byte
/// blocks straddle without ever landing in. At 64 the fixture has 37 blocks
/// and several boundaries inside that range. The seekability and the split are
/// both asserted rather than assumed, since either failing would make this
/// test the serial path compared to itself.
#[tokio::test]
async fn a_seekable_xz_splits_at_its_own_block_boundaries() {
    let plain = LocalFileSource::open(edge_cases()).unwrap();
    let expected = serial_rows(&plain, "public.widgets", QueryOptions::default()).await;

    let compressed = xz_compress(&edge_cases(), &["--block-size=64"]);
    let xz = XzSource::open(compressed.path()).unwrap();
    assert!(
        xz.seek_table().unwrap().is_seekable(),
        "a single-block fixture would advise one partition and pass for the wrong reason"
    );

    let mut split_at_least_once = false;
    for jobs in [2usize, 3, 8] {
        let (rows, count) =
            partitioned_rows(&xz, "public.widgets", QueryOptions::default(), jobs).await;
        assert_eq!(rows, expected, "jobs {jobs}");
        split_at_least_once |= count > 1;
    }
    assert!(
        split_at_least_once,
        "the block boundaries inside this block's data must have produced a real split, or the \
         equality above is the serial path compared to itself"
    );
}

/// **The span term is charged per source, not universally.** A block-decoding
/// `.xz` retains by the *partition*: a batch holding views into a decoded
/// block pins that block, which `partition_bytes` has already charged for, so
/// adding `max_source_span` on top would count the same bytes twice
/// (`docs/design/decisions.md`, "D47").
///
/// One budget, one span and one job count, put to both source shapes:
///
/// - the compressed source is charged what one reader holds and nothing more,
///   so a budget of exactly four such readers affords the four workers asked
///   for, has no span to spend and **says nothing at all**;
/// - the plain source is charged that budget against its own partition **plus**
///   the shipped 64 MiB span, which swamps it, so the plan spends the span
///   down to what four readers leave and says so
///   ([`PlanNoteKind::BatchSpanNarrowed`]; `docs/design/decisions.md`, "D84").
///
/// **The narrowing note is what the per-source charge is observed through**,
/// now that spending the span is what a chunk-shaped source does before its
/// count is cut: the same budget, the same stated span and the same job count
/// produce a note on one shape and silence on the other, which is the
/// accounting difference and nothing else.
///
/// The rows are the plain file's either way, which is what keeps this a
/// statement about the *accounting* rather than about what gets read.
#[tokio::test]
async fn a_block_shaped_source_is_not_charged_the_batch_span() {
    let plain = LocalFileSource::open(edge_cases()).unwrap();
    let expected = serial_rows(&plain, "public.widgets", QueryOptions::default()).await;

    let compressed = xz_compress(&edge_cases(), &["--block-size=64"]);
    let xz = XzSource::open(compressed.path()).unwrap();
    assert!(
        xz.seek_table().unwrap().is_seekable(),
        "a single-block fixture would advise one partition and pass for the wrong reason"
    );
    // **The budget is four of the compressed source's own reader charges**,
    // read off the source rather than restated here, so this stays a test of
    // the accounting when the terms of that charge move. It is announced the
    // chunk size the run will announce, that being one of the terms.
    xz.hint_read_size(ScanOptions::default().chunk_size_bytes);
    let reader = xz.block_decode_bytes().expect("a compressed source states its block-path cost");
    let budget = 4 * reader;
    let options =
        QueryOptions { parallelism: Parallelism::workers(4, budget), ..Default::default() };

    let streams = table_stream_partitions(
        &plain,
        "public.widgets",
        ScanOptions::default(),
        options.clone(),
        CacheMode::DISABLED,
    )
    .await
    .unwrap();
    match streams[0].plan_notes().as_slice() {
        [note] => {
            let PlanNoteKind::BatchSpanNarrowed {
                stated_bytes, planned_bytes, memory_bytes, ..
            } = &note.kind
            else {
                panic!("{note:?}")
            };
            assert_eq!(
                *stated_bytes,
                options.max_source_span.unwrap() as u64,
                "a chunk-shaped source is charged the span, and the note names what was stated"
            );
            assert!(
                *planned_bytes < *stated_bytes,
                "the span was spent to seat the workers: {planned_bytes} of {stated_bytes}"
            );
            assert_eq!(*memory_bytes, budget);
        }
        other => panic!("expected exactly one plan note, got {other:?}"),
    }

    let streams = table_stream_partitions(
        &xz,
        "public.widgets",
        ScanOptions::default(),
        options,
        CacheMode::DISABLED,
    )
    .await
    .unwrap();
    assert!(
        streams.iter().all(|s| s.plan_notes().is_empty()),
        "the span is not charged here, so the budget affords every worker: {:?}",
        streams[0].plan_notes()
    );
    assert!(streams.len() > 1, "and the plan really did hand out more than one sub-stream");

    let mut rows = Rows::new();
    for mut stream in streams {
        while let Some(batch) = stream.next().await {
            rows.extend(rows_of(&batch.unwrap()));
        }
    }
    assert_eq!(rows, expected);
}

/// A source that declines to be split is not split, whatever `--jobs` says —
/// the single-block `.xz`, whose piecewise arm would make two readers
/// each force the other's restart (`docs/design/decisions.md`, "D15"). One
/// sub-stream, and the same rows.
#[tokio::test]
async fn a_single_block_xz_declines_to_be_split() {
    let plain = LocalFileSource::open(edge_cases()).unwrap();
    let expected = serial_rows(&plain, "public.widgets", QueryOptions::default()).await;

    let compressed = xz_compress(&edge_cases(), &[]);
    let xz = XzSource::open(compressed.path()).unwrap();
    assert!(
        !xz.seek_table().unwrap().is_seekable(),
        "fixture must actually be single-block for this test to mean anything"
    );

    let (rows, count) = partitioned_rows(&xz, "public.widgets", QueryOptions::default(), 8).await;
    assert_eq!(count, 1, "a declining source yields one sub-stream");
    assert_eq!(rows, expected);
}

/// A multi-block `.xz` whose blocks the stated budget cannot hold is
/// read through the streaming decoder, and the plan says so — naming the
/// file's largest block beside the budget that declined it, which is the
/// number the read-buffer budget has to clear.
///
/// **Multi-block on purpose.** The decline is about a file that *does* have
/// seekability to lose; a single-block file has none at any budget and earns
/// `DiagnosticKind::NonSeekableCompressedSource` instead, which is a property
/// of the file and reaches a different channel.
#[tokio::test]
async fn a_budget_declined_block_path_announces_the_block_to_budget_for() {
    let compressed = xz_compress(&edge_cases(), &["--block-size=512"]);
    let xz = XzSource::open(compressed.path()).unwrap();
    let table = xz.seek_table().unwrap();
    assert!(table.is_seekable(), "the decline is only interesting on a file with blocks to lose");

    // A budget below one block, so no whole block can be held.
    let budget = table.max_block_uncompressed() - 1;
    let options =
        QueryOptions { parallelism: Parallelism::workers(4, budget), ..Default::default() };
    let streams = table_stream_partitions(
        &xz,
        "public.widgets",
        ScanOptions::default(),
        options,
        CacheMode::DISABLED,
    )
    .await
    .unwrap();
    let notes = streams[0].plan_notes();
    let declined = notes
        .iter()
        .find_map(|n| match &n.kind {
            PlanNoteKind::CompressedBlockPathDeclined {
                block_count,
                max_block_uncompressed,
                reader_bytes,
                memory_bytes,
            } => Some((*block_count, *max_block_uncompressed, *reader_bytes, *memory_bytes)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{notes:?}"));
    // The recourse the note names is the source's own number, not a multiple
    // this test re-derives: one reader's block with the retention list the
    // pool keeps beside it, the chunk buffer a straddling read is assembled
    // into, and the decoder's own retention. The assertion is a lower bound.
    let reader = xz.block_decode_bytes().expect("a compressed source states its block-path cost");
    assert!(
        reader > 2 * table.max_block_uncompressed(),
        "the chunk and the decoder are inside it too, not just the two blocks: {reader}"
    );
    assert_eq!(declined, (table.block_count(), table.max_block_uncompressed(), reader, budget));

    // The rows are unaffected — the fallback is slower on a backward read,
    // never a different answer.
    let plain = LocalFileSource::open(edge_cases()).unwrap();
    let expected = serial_rows(&plain, "public.widgets", QueryOptions::default()).await;
    let mut rows = Rows::new();
    for mut stream in streams {
        while let Some(batch) = stream.next().await {
            rows.extend(rows_of(&batch.unwrap()));
        }
    }
    assert_eq!(rows, expected);
}

/// The three shapes that say nothing: a budget that affords a whole block, a
/// plain source (no container to decline), and a single-block `.xz` (nothing
/// to seek by at any budget, which is the file-level warning's case and not
/// this one).
#[tokio::test]
async fn a_block_path_that_was_taken_is_silent() {
    let declines = |notes: &[pgdump_query::PlanNote]| {
        notes.iter().any(|n| matches!(n.kind, PlanNoteKind::CompressedBlockPathDeclined { .. }))
    };

    let compressed = xz_compress(&edge_cases(), &["--block-size=512"]);
    let xz = XzSource::open(compressed.path()).unwrap();
    let affordable =
        QueryOptions { parallelism: Parallelism::workers(4, 1 << 30), ..Default::default() };
    let streams = table_stream_partitions(
        &xz,
        "public.widgets",
        ScanOptions::default(),
        affordable.clone(),
        CacheMode::DISABLED,
    )
    .await
    .unwrap();
    assert!(!declines(&streams[0].plan_notes()), "{:?}", streams[0].plan_notes());

    let plain = LocalFileSource::open(edge_cases()).unwrap();
    let streams = table_stream_partitions(
        &plain,
        "public.widgets",
        ScanOptions::default(),
        affordable.clone(),
        CacheMode::DISABLED,
    )
    .await
    .unwrap();
    assert!(!declines(&streams[0].plan_notes()), "{:?}", streams[0].plan_notes());

    // A single-block file, under a budget far too small to hold its one
    // block: still silent here, because it has nothing to seek by whatever
    // the budget says.
    let single = xz_compress(&edge_cases(), &[]);
    let xz = XzSource::open(single.path()).unwrap();
    let options = QueryOptions { parallelism: Parallelism::workers(4, 64), ..Default::default() };
    let streams = table_stream_partitions(
        &xz,
        "public.widgets",
        ScanOptions::default(),
        options,
        CacheMode::DISABLED,
    )
    .await
    .unwrap();
    assert!(!declines(&streams[0].plan_notes()), "{:?}", streams[0].plan_notes());
}

/// A stated byte budget caps the sub-stream count below the stated `--jobs`,
/// which is the arithmetic a memory-bounded caller states both numbers for
/// (`docs/design/decisions.md`, "D4"). The
/// rows are unaffected.
///
/// **The divisor is two terms, not one**: what a concurrent reader costs the
/// source (`partition_bytes`) plus what a sub-stream's held batch pins
/// (`max_source_span`, charged here because a plain file retains by the read
/// chunk). **The second term is a ceiling the plan spends before it cuts the
/// count** (`docs/design/decisions.md`, "D84"), so a budget of four
/// partition-units against eight jobs narrows a one-unit span to the announced
/// read chunk and then affords three workers — the count binds only once the
/// span has nothing left to give.
///
/// **The unit is read off the source rather than restated**
/// ([`partition_unit`]), so the arithmetic below stays the arithmetic under
/// test when the plain source's own partition size moves.
///
/// **The plan says why, too**: [`PlanNoteKind::ParallelismBudgetLimited`]
/// names the same two divisor terms and the budget that declined them, so this
/// exact arithmetic is checked against the diagnostic as well as against the
/// sub-stream count.
#[tokio::test]
async fn a_tight_budget_hands_out_fewer_sub_streams_than_jobs() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let expected = serial_rows(&source, "public.t_int", QueryOptions::default()).await;

    // One partition-unit read plus a floored span pinned makes a budget of
    // four such units afford three workers however many jobs are asked for.
    let scan = ScanOptions::default();
    let floor = scan.chunk_size_bytes as u64;
    let chunk = partition_unit(&source, &scan);
    let planned = (4 * chunk / (chunk + floor)) as usize;
    assert_eq!(planned, 3, "eight jobs at this budget plan down to three");
    let options = QueryOptions {
        parallelism: Parallelism::workers(8, 4 * chunk),
        max_source_span: Some(chunk as usize),
        ..Default::default()
    };
    let streams =
        table_stream_partitions(&source, "public.t_int", scan, options, CacheMode::DISABLED)
            .await
            .unwrap();
    assert_eq!(streams.len(), planned, "the budget binds before the job count does");
    match streams[0].plan_notes().as_slice() {
        [narrowed, d] => {
            let PlanNoteKind::BatchSpanNarrowed { stated_bytes, planned_bytes, .. } =
                &narrowed.kind
            else {
                panic!("{narrowed:?}")
            };
            assert_eq!(*stated_bytes, chunk);
            assert_eq!(*planned_bytes, floor);
            let PlanNoteKind::ParallelismBudgetLimited {
                requested,
                planned: planned_workers,
                footprint,
                max_source_span,
                memory_bytes,
            } = &d.kind
            else {
                panic!("{d:?}")
            };
            assert_eq!(*requested, 8);
            assert_eq!(*planned_workers, planned);
            assert_eq!(*footprint, chunk);
            assert_eq!(*max_source_span, Some(floor));
            assert_eq!(*memory_bytes, 4 * chunk);
        }
        other => panic!("expected the narrowing note and the count note, got {other:?}"),
    }

    let mut rows = Rows::new();
    for mut stream in streams {
        while let Some(batch) = stream.next().await {
            rows.extend(rows_of(&batch.unwrap()));
        }
    }
    assert_eq!(rows, expected);
}

/// **The span's floor is the chunk this query announced, not the shipped
/// default** (`docs/design/decisions.md`, "D84"): what a held batch pins is its
/// span rounded out to the unit the source retains, and that unit is the read
/// size the replay loop announces (`ScanOptions::chunk_size_bytes`).
///
/// The shape is the test above at a sixteenth of the shipped chunk and at a
/// budget of exactly what the readers asked for cost, so the room left for a
/// span is nothing and the floor is the whole answer. The plain source's
/// partition follows the announced chunk down, the floor does too, and the
/// budget seats seven of the eight; at the shipped 1 MiB the span would have
/// floored sixteen times above the unit a batch actually pins and five of those
/// seven would have been declined for bytes nothing holds — which is the defect
/// this pins.
#[tokio::test]
async fn the_span_floors_at_the_announced_chunk_not_the_shipped_one() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let expected = serial_rows(&source, "public.t_int", QueryOptions::default()).await;

    let scan = ScanOptions { chunk_size_bytes: 64 << 10, ..ScanOptions::default() };
    let floor = scan.chunk_size_bytes as u64;
    let shipped_floor = ScanOptions::default().chunk_size_bytes as u64;
    let unit = partition_unit(&source, &scan);
    assert!(unit < shipped_floor, "the partition follows the announced chunk down: {unit}");
    let planned = (8 * unit / (unit + floor)) as usize;
    assert_eq!(planned, 7, "seven of the eight, at the announced floor");
    assert_eq!(
        (8 * unit / (unit + shipped_floor)) as usize,
        2,
        "two at the shipped floor, which is what makes this test discriminating"
    );

    // Eight readers of one unit inside eight units: the count is afforded at no
    // span at all, so what the span narrows to is the floor and nothing else.
    let options =
        QueryOptions { parallelism: Parallelism::workers(8, 8 * unit), ..Default::default() };
    let streams =
        table_stream_partitions(&source, "public.t_int", scan, options, CacheMode::DISABLED)
            .await
            .unwrap();
    assert_eq!(streams.len(), planned);
    match streams[0].plan_notes().as_slice() {
        [narrowed, limited] => {
            let PlanNoteKind::BatchSpanNarrowed { planned_bytes, workers, .. } = &narrowed.kind
            else {
                panic!("{narrowed:?}")
            };
            assert_eq!(*planned_bytes, floor, "narrowed to the announced chunk");
            assert_eq!(*workers, planned);
            let PlanNoteKind::ParallelismBudgetLimited { requested, max_source_span, .. } =
                &limited.kind
            else {
                panic!("{limited:?}")
            };
            assert_eq!(*requested, 8);
            assert_eq!(*max_source_span, Some(floor));
        }
        other => panic!("expected the narrowing note and the count note, got {other:?}"),
    }

    let mut rows = Rows::new();
    for mut stream in streams {
        while let Some(batch) = stream.next().await {
            rows.extend(rows_of(&batch.unwrap()));
        }
    }
    assert_eq!(rows, expected, "the rows are the serial oracle's whatever the chunk");
}

/// **`max_source_span: None` opts a query out of the span term entirely**,
/// falling back to the decode-footprint-only divisor a discovery worker's own
/// call uses — an unbounded batch has no number to add, and pretending it
/// were zero would silently under-count what a sub-stream can grow to hold.
/// So the same budget that a stated span would have clamped to one worker
/// affords two once the span is left unbounded.
///
/// The diagnostic agrees: `max_source_span` reads back `None` rather than a
/// number that was never charged.
#[tokio::test]
async fn an_unbounded_span_falls_back_to_the_decode_footprint_alone() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let expected = serial_rows(&source, "public.t_int", QueryOptions::default()).await;

    let chunk = partition_unit(&source, &ScanOptions::default());
    let options = QueryOptions {
        parallelism: Parallelism::workers(8, 2 * chunk),
        max_source_span: None,
        ..Default::default()
    };
    let streams = table_stream_partitions(
        &source,
        "public.t_int",
        ScanOptions::default(),
        options,
        CacheMode::DISABLED,
    )
    .await
    .unwrap();
    assert_eq!(streams.len(), 2, "an unbounded span divides by the decode footprint alone");
    match streams[0].plan_notes().as_slice() {
        [d] => {
            let PlanNoteKind::ParallelismBudgetLimited {
                requested,
                planned,
                footprint,
                max_source_span,
                memory_bytes,
            } = &d.kind
            else {
                panic!("{d:?}")
            };
            assert_eq!(*requested, 8);
            assert_eq!(*planned, 2);
            assert_eq!(*footprint, chunk);
            assert_eq!(*max_source_span, None);
            assert_eq!(*memory_bytes, 2 * chunk);
        }
        other => panic!("expected exactly one plan note, got {other:?}"),
    }

    let mut rows = Rows::new();
    for mut stream in streams {
        while let Some(batch) = stream.next().await {
            rows.extend(rows_of(&batch.unwrap()));
        }
    }
    assert_eq!(rows, expected);
}

/// **A plan over a map the caller holds replays what the query's own mapping
/// pass would**: the same partitions, the same rows, a schema known before
/// any is read — and each partition started afresh as often as it is asked
/// for, at whatever batch size. A map short of the file's end is refused
/// before a byte is read.
#[tokio::test]
async fn a_plan_over_a_held_map_replays_as_the_mapping_pass_would() {
    use std::sync::Arc;

    use pgdump_query::cache::{SourceWatch, StrictIdentity};
    use pgdump_query::{
        Error, StatisticsRequest, TableName, TablePartitions, build_index, map_file, table_schema,
    };

    let path = partitions_fixture(16, "load-via-partition-root");
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&path).unwrap());
    let run = map_file(
        source.as_ref(),
        &ScanOptions::default(),
        &CacheMode::DISABLED,
        &StatisticsRequest::NONE,
    )
    .await
    .unwrap();
    let index = run.index;
    let watch =
        Arc::new(SourceWatch::open(source.as_ref(), StrictIdentity::ADVISORY).await.unwrap());
    let options =
        QueryOptions { parallelism: Parallelism::workers(4, 1 << 30), ..QueryOptions::default() };

    let mut compared = 0;
    for table in index.tables() {
        let (expected, count) =
            partitioned_rows(source.as_ref(), &table.qualified(), QueryOptions::default(), 4).await;
        let plan = TablePartitions::plan(
            Arc::clone(&source),
            &index,
            Arc::clone(&watch),
            &table,
            ScanOptions::default(),
            options.clone(),
        )
        .await
        .unwrap();
        assert_eq!(plan.len(), count, "{}", table.qualified());
        let schema = table_schema(&index, &table, &options).unwrap();
        assert_eq!(plan.resolved_schema().schema, schema.schema);
        for _ in 0..2 {
            let mut rows = Rows::new();
            for partition in 0..plan.len() {
                let mut stream = plan.stream(partition, 1);
                while let Some(batch) = stream.next().await {
                    let batch = batch.unwrap();
                    assert!(batch.num_rows() <= 1);
                    assert_eq!(batch.schema(), schema.schema);
                    rows.extend(rows_of(&batch));
                }
            }
            assert_eq!(rows, expected, "{}", table.qualified());
        }
        compared += 1;
    }
    assert!(compared > 1);

    // A map claiming less than the whole file: nothing is scanned to finish it.
    let mut partial = build_index(source.as_ref(), &ScanOptions::default()).await.unwrap();
    partial.scanned_through = 1;
    let table = TableName { database: None, schema: Some("public".into()), table: "spread".into() };
    let refused = TablePartitions::plan(
        Arc::clone(&source),
        &partial,
        watch,
        &table,
        ScanOptions::default(),
        options,
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(refused, Error::MapIncomplete { scanned_through: 1, .. }), "{refused}");
}
