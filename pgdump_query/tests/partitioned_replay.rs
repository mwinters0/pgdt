//! Partitioned replay: one map, N sub-streams
//! (`docs/design/architecture.md`, "Partitioned replay").
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
    ByteRangeSource, Expr, LocalFileSource, Parallelism, Predicate, PredicateOp, QueryOptions,
    ScanOptions, XzSource, table_stream, table_stream_partitions,
};

mod common;
use common::{edge_cases, partitions_fixture, rows_of, types_fixture};

type Rows = Vec<Vec<Option<String>>>;

/// Every row a serial `table_stream` yields, in file order — the oracle.
async fn serial_rows(source: &dyn ByteRangeSource, table: &str, options: QueryOptions) -> Rows {
    let mut stream =
        table_stream(source, table, ScanOptions::default(), options, None, CacheMode::Disabled);
    let mut rows = Rows::new();
    while let Some(batch) = stream.next().await {
        rows.extend(rows_of(&batch.unwrap()));
    }
    rows
}

/// Every row `jobs` sub-streams yield, drained one after another in the order
/// the split handed them back — which is file order, so this is directly
/// comparable to [`serial_rows`].
///
/// Draining them sequentially is deliberate: it is the arrangement the design
/// says must be indistinguishable from the serial path
/// (`docs/design/roadmap-P16-parallel-scan.md`, "The library hands out
/// partitions; the CLI merges them"), and running them concurrently would test
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
        CacheMode::Disabled,
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
        CacheMode::Disabled,
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
/// (`docs/design/architecture.md`, "Execution model and API surface").
#[tokio::test]
async fn serial_parallelism_is_exactly_one_sub_stream() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let streams = table_stream_partitions(
        &source,
        "public.widgets",
        ScanOptions::default(),
        QueryOptions::default(),
        CacheMode::Disabled,
    )
    .await
    .unwrap();
    assert_eq!(streams.len(), 1);

    // `--jobs 1` is the same value, so it is the same one sub-stream.
    let (_, count) = partitioned_rows(&source, "public.widgets", QueryOptions::default(), 1).await;
    assert_eq!(count, 1);
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
/// `pgdq query` sorts one-batch-per-partition on
/// (`docs/design/architecture.md`, "Partitioned replay").
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
        CacheMode::Disabled,
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
        CacheMode::Disabled,
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
/// (`docs/design/architecture.md`, "Partitioned replay").
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
        CacheMode::Disabled,
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
        CacheMode::Disabled,
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

/// A source that declines to be split is not split, whatever `--jobs` says —
/// the single-block `.xz`, whose streaming fallback would make two readers
/// each force the other's restart (`docs/design/architecture.md`, "The
/// compressed source"). One sub-stream, and the same rows.
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

/// A stated byte budget caps the sub-stream count below the stated `--jobs`,
/// which is the arithmetic a memory-bounded caller states both numbers for
/// (`docs/design/architecture.md`, "Execution model and API surface"). The
/// rows are unaffected.
#[tokio::test]
async fn a_tight_budget_hands_out_fewer_sub_streams_than_jobs() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let expected = serial_rows(&source, "public.t_int", QueryOptions::default()).await;

    // One read chunk per partition on a local file, so a budget of two chunks
    // affords two workers however many jobs are asked for.
    let chunk = ScanOptions::default().chunk_size as u64;
    let options =
        QueryOptions { parallelism: Parallelism::workers(8, 2 * chunk), ..Default::default() };
    let streams = table_stream_partitions(
        &source,
        "public.t_int",
        ScanOptions::default(),
        options,
        CacheMode::Disabled,
    )
    .await
    .unwrap();
    assert_eq!(streams.len(), 2, "the budget binds before the job count does");

    let mut rows = Rows::new();
    for mut stream in streams {
        while let Some(batch) = stream.next().await {
            rows.extend(rows_of(&batch.unwrap()));
        }
    }
    assert_eq!(rows, expected);
}
