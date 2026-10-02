//! `stream::map_file` — `pgdt parse`'s scan
//! (`docs/design/decisions.md`, "The CLI").
//!
//! **The claim these tests exist for**: a scan that stopped partway leaves a
//! cache that a later `map_file` finishes, and the finished index is
//! indistinguishable from one `build_index` produced in a single pass. That is
//! the property that would fail *silently* — a resumed scan that dropped a
//! span, mis-seamed a boundary, or lost a census would still print a plausible
//! listing.
//!
//! **Every scan here is at the data level** (`StatisticsRequest::DATA`), so
//! it censuses as the eager producer does, and its statistics are dropped
//! before the two are compared, the eager producer gathering none
//! ([`assert_matches_eager`]). A gathering scan's statistics are compared
//! against a serial one's instead, by `tests/statistics.rs` and
//! `pgdt/tests/determinism.rs`. The back-fill tests start from a map at the
//! metadata level (`StatisticsRequest::METADATA`) over a dropping or a
//! cancelling source, to see what an interrupted back-fill banks and reports.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::datatypes::{DataType, TimeUnit};
use futures::StreamExt;
use pgdump_query::cache::{CacheLoad, CacheMode, CacheStatus};
use pgdump_query::resolve::{ColumnResolution, SchemaMode, resolve_columns};
use pgdump_query::{
    ByteRangeSource, Cancellation, DEFAULT_MEMORY_BUDGET, DataBlock, DumpIndex, LocalFileSource,
    Parallelism, QueryOptions, ScanOptions, SpanBody, StatisticsRequest, build_index, cache,
    map_file, preamble_only, table_stream,
};

/// A cancellation already asked for, so the scan under it stops at its first
/// poll rather than at a moment a test would have to arrange.
fn already_cancelled() -> Arc<Cancellation> {
    let cancel = Arc::new(Cancellation::new());
    cancel.cancel();
    cancel
}

/// A private copy of `tests/data/edge_cases.sql` in a fresh tempdir, so each
/// test may freely write the colocated `.dtcache` beside it — same fixture and
/// same convention as `tests/query_cache.rs`, which is what makes the "a query
/// stops at its target" setup below behave identically here.
mod common;
use common::sandboxed_edge_cases as sandboxed;

/// `index` with every block's statistics dropped, as the eager pass leaves it.
fn without_statistics(index: &DumpIndex) -> DumpIndex {
    let mut index = index.clone();
    for span in &mut index.spans {
        if let SpanBody::Data(DataBlock::Copy(block)) = &mut span.body {
            block.statistics = None;
        }
    }
    index
}

/// Assert that a resumed/finished index is what a single eager pass produces,
/// once the statistics the eager pass never gathers are dropped from it.
/// Field by field before the whole-struct comparison, so a failure names which
/// half drifted rather than dumping two entire indexes.
fn assert_matches_eager(actual: &DumpIndex, eager: &DumpIndex, label: &str) {
    let actual = &without_statistics(actual);
    assert_eq!(actual.scanned_through, eager.scanned_through, "{label}: scanned_through");
    assert_eq!(actual.spans.len(), eager.spans.len(), "{label}: span count");
    for (i, (a, e)) in actual.spans.iter().zip(&eager.spans).enumerate() {
        assert_eq!(a, e, "{label}: span {i}");
    }
    assert_eq!(actual.metadata, eager.metadata, "{label}: metadata");
    assert_eq!(actual.roles, eager.roles, "{label}: roles");
    assert_eq!(actual.tablespaces, eager.tablespaces, "{label}: tablespaces");
    assert_eq!(actual, eager, "{label}");
}

/// A cold `map_file` against a file with no cache at all is `build_index` by
/// another route — the baseline the resumed cases below are measured against.
#[tokio::test]
async fn a_cold_map_file_matches_build_index() {
    for schema_dir in ["edge_cases", "emitters", "objects", "partitions", "statistics", "types"] {
        for flag_set in ["default", "data-only", "schema-only"] {
            let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../fixtures/16")
                .join(schema_dir)
                .join(format!("{flag_set}.sql"));
            if !fixture.exists() {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            let dump = dir.path().join("dump.sql");
            std::fs::copy(&fixture, &dump).unwrap();
            let source = LocalFileSource::open(&dump).unwrap();
            let mode = CacheMode::enabled(cache::colocated_path(&dump));

            let run = map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA)
                .await
                .unwrap();
            assert_eq!(run.resumed_from, 0, "{schema_dir}/{flag_set}: nothing to resume from");
            assert!(!run.interrupted);
            let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
            assert_matches_eager(&run.index, &eager, &format!("{schema_dir}/{flag_set}"));
        }
    }
}

/// **What `--jobs` buys a `parse`, and the property it must not cost.** The
/// mapping pass offers every open `COPY` region to the leader's scheduler, so a
/// block large enough to cut is scanned by workers that split its interior —
/// and the index that comes out is still, span for span and census for census,
/// what one serial eager pass gives (`docs/design/decisions.md`, "D52").
///
/// **A fixture is small, so the chunk size is announced small.** The local
/// source's partition is a fixed multiple of the read chunk, so at the shipped
/// 1 MiB every block here is inside one partition and the scheduler correctly
/// declines all of them — which would make this the serial path compared to
/// itself. 64 bytes splits nearly every block, 512 splits the larger ones and
/// leaves the rest to the serial scanner, so the mixed case — a scan that
/// alternates between the two paths — is in here too. `tests/wait_policy.rs` is
/// where "the leader was actually reached under these options" is asserted, off
/// the one announcement only its scheduler makes.
#[tokio::test]
async fn a_parallel_mapping_pass_builds_the_index_a_serial_one_does() {
    for schema_dir in ["edge_cases", "emitters", "objects", "partitions", "statistics", "types"] {
        for flag_set in ["default", "data-only"] {
            let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../fixtures/16")
                .join(schema_dir)
                .join(format!("{flag_set}.sql"));
            if !fixture.exists() {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            let dump = dir.path().join("dump.sql");
            std::fs::copy(&fixture, &dump).unwrap();
            let source = LocalFileSource::open(&dump).unwrap();
            let eager = build_index(&source, &ScanOptions::default()).await.unwrap();

            for chunk_size in [64usize, 512] {
                for jobs in [2usize, 3, 8] {
                    let label =
                        format!("{schema_dir}/{flag_set}, {jobs} jobs, {chunk_size}B chunk");
                    let mode =
                        CacheMode::enabled(dir.path().join(format!("{chunk_size}-{jobs}.dtcache")));
                    let options = ScanOptions {
                        chunk_size_bytes: chunk_size,
                        parallelism: Parallelism::workers(jobs, DEFAULT_MEMORY_BUDGET),
                        ..ScanOptions::default()
                    };
                    let run =
                        map_file(&source, &options, &mode, &StatisticsRequest::DATA).await.unwrap();
                    assert!(!run.interrupted, "{label}");
                    assert_matches_eager(&run.index, &eager, &label);
                }
            }
        }
    }
}

/// **The central claim of resuming.** A query that stopped at its target leaves a
/// genuinely partial cache; `map_file` finishes it, and the result is span for
/// span — census included — what one eager pass gives.
#[tokio::test]
async fn a_partial_cache_is_finished_into_the_same_index_an_eager_scan_builds() {
    let (_dir, dump) = sandboxed();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();
    let size = source.size().await.unwrap();
    let mode = CacheMode::enabled(cache_path.clone());

    // A cold query stops once `widgets` is settled, well short of EOF.
    let mut stream = table_stream(
        &source,
        "public.widgets",
        ScanOptions::default(),
        QueryOptions::default(),
        None,
        mode.clone(),
    );
    while stream.next().await.is_some() {}
    let CacheLoad::Index(partial) = mode.load(&source).await.unwrap() else {
        panic!("the query wrote a cache")
    };
    assert!(partial.scanned_through < size, "sanity: the query really did stop short");

    let run =
        map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA).await.unwrap();
    assert_eq!(run.resumed_from, partial.scanned_through, "resumed at the cache's own frontier");
    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
    assert!(
        partial.blocks().count() < eager.blocks().count(),
        "sanity: the query left blocks unmapped for this to find"
    );
    assert_matches_eager(&run.index, &eager, "resumed from a query's partial cache");

    // And what landed on disk is the finished index, not the partial one.
    let CacheLoad::Index(reloaded) = mode.load(&source).await.unwrap() else {
        panic!("the map wrote a cache")
    };
    assert_eq!(reloaded.scanned_through, size);
    assert_eq!(without_statistics(&reloaded).spans, eager.spans);
}

/// A block-rich dump with one three-row `COPY` block per table, the schema
/// section first the way `pg_dump` orders a plain dump. The shape koji cannot
/// show — block-rich and byte-poor — at a size a test can afford, and the one
/// the save throttle's gate actually closes over: every block after the first
/// completes far inside 20× the first save's own cost.
fn block_rich(dir: &Path, tables: usize) -> PathBuf {
    let mut sql = String::from("SET client_encoding = 'UTF8';\n\n");
    for i in 0..tables {
        sql.push_str(&format!("CREATE TABLE public.t{i} (id integer, v text);\n\n"));
    }
    for i in 0..tables {
        sql.push_str(&format!("COPY public.t{i} (id, v) FROM stdin;\n"));
        for row in 0..3 {
            sql.push_str(&format!("{row}\tvalue {row} of t{i}\n"));
        }
        sql.push_str("\\.\n\n\n");
    }
    let dump = dir.join("block_rich.sql");
    std::fs::write(&dump, sql).unwrap();
    dump
}

/// **The gate the splice rides.** `stream::splice` fires at the save
/// throttle's openings rather than at every `CopyEnd`, so on a block-rich file
/// most blocks never rebuild `index.spans` on their own. Two things must
/// survive that, and neither is visible on a fixture with three blocks:
///
/// - **The early stop.** `target_settled` is the one reader of `index.spans`
///   inside the mapping loop, so a block whose header could settle the query
///   opens the gate itself. Query the *last* table and the stop still lands on
///   its block, short of EOF — if the gate had eaten it, the scan would run to
///   EOF and `scanned_through` would be the file's size.
/// - **The map is whole anyway.** A skipped splice is not a lost block: the
///   `Builder` accumulates every one, and the next splice snapshots all of
///   them. So the cache the query banks holds all 40, and finishing it
///   reproduces the eager index span for span.
#[tokio::test]
async fn a_query_settles_on_a_late_block_and_banks_every_block_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let dump = block_rich(dir.path(), 40);
    let source = LocalFileSource::open(&dump).unwrap();
    let size = source.size().await.unwrap();
    let mode = CacheMode::enabled(cache::colocated_path(&dump));

    let mut stream = table_stream(
        &source,
        "public.t39",
        ScanOptions::default(),
        QueryOptions::default(),
        None,
        mode.clone(),
    );
    let mut rows = 0;
    while let Some(batch) = stream.next().await {
        rows += batch.unwrap().num_rows();
    }
    assert_eq!(rows, 3, "the last table's rows");

    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
    let last_end = eager.blocks().last().expect("40 blocks").end_offset;
    assert!(last_end < size, "sanity: the file does not end at the last block");

    let CacheLoad::Index(banked) = mode.load(&source).await.unwrap() else {
        panic!("the query wrote a cache")
    };
    assert_eq!(banked.scanned_through, last_end, "stopped on the settling block, not at EOF");
    assert_eq!(banked.blocks().count(), 40, "every block before it is in the map too");

    let run =
        map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA).await.unwrap();
    assert_matches_eager(&run.index, &eager, "finished from a gated query's cache");
}

/// The other shape of partial cache: `--preamble-only`, which stops at the
/// first `COPY` header and leaves an `Unscanned` tail covering everything
/// after it. Resuming from it must splice onto the prepass's spans rather than
/// starting a second, parallel map.
#[tokio::test]
async fn a_preamble_only_cache_is_finished_into_the_same_index() {
    let (_dir, dump) = sandboxed();
    let source = LocalFileSource::open(&dump).unwrap();
    let mode = CacheMode::enabled(cache::colocated_path(&dump));

    preamble_only(&source, &ScanOptions::default(), &mode).await.unwrap();
    let CacheLoad::Index(preamble_cache) = mode.load(&source).await.unwrap() else {
        panic!("preamble_only wrote a cache")
    };
    assert!(preamble_cache.scanned_through > 0);
    assert!(preamble_cache.blocks().next().is_none(), "the prepass stops before the first block");

    let run =
        map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA).await.unwrap();
    assert_eq!(run.resumed_from, preamble_cache.scanned_through);
    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
    assert_matches_eager(&run.index, &eager, "resumed from a preamble-only cache");
}

/// A second `map_file` over an already-complete cache scans nothing — it
/// reports the frontier it found rather than a resume point — and still hands
/// back the same index, diagnostics included. Diagnostics are the interesting
/// half: they are `#[serde(skip)]`, so an index that came wholly off disk
/// carries none until this recomputes them.
#[tokio::test]
async fn a_complete_cache_is_reported_without_rescanning() {
    let (_dir, dump) = sandboxed();
    let source = LocalFileSource::open(&dump).unwrap();
    let size = source.size().await.unwrap();
    let mode = CacheMode::enabled(cache::colocated_path(&dump));

    let first = map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA)
        .await
        .unwrap()
        .index;
    let second =
        map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA).await.unwrap();

    assert_eq!(second.resumed_from, size, "the cache already covered the file");
    assert_eq!(first, second.index);
    assert!(
        !second.index.diagnostics.is_empty(),
        "the TOC-coverage figure is recomputed, not read back from a cache that never stored it"
    );
}

/// A [`ByteRangeSource`] that refuses to read past `fail_at` — the killed
/// `pgdt parse` these tests cannot produce with a signal. Everything below
/// `fail_at` reads normally, so the scan makes real progress and banks real
/// saves before it dies.
struct FailsPast<'a> {
    inner: &'a LocalFileSource,
    fail_at: u64,
}

impl ByteRangeSource for FailsPast<'_> {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = pgdump_query::Result<bytes::Bytes>> + Send + '_>,
    > {
        Box::pin(async move {
            if offset >= self.fail_at {
                return Err(pgdump_query::Error::Io(std::io::Error::other(
                    "simulated interruption",
                )));
            }
            self.inner.read_range(offset, len).await
        })
    }

    fn size(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = pgdump_query::Result<u64>> + Send + '_>>
    {
        self.inner.size()
    }

    fn modified(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = pgdump_query::Result<Option<std::time::SystemTime>>>
                + Send
                + '_,
        >,
    > {
        self.inner.modified()
    }
}

/// **The write-per-block claim, and the reason resuming is worth building.**
/// Every completed block is banked at a `CopyEnd` watermark, which is a
/// resumable point by construction — an interrupted scan leaves something to
/// resume from and something to report, rather than nothing at all.
///
/// The interruption is cut exactly one byte past the first block's end, with
/// one-byte reads so that boundary is a read boundary: the block's own save has
/// happened, nothing after it has.
#[tokio::test]
async fn an_interrupted_map_file_leaves_a_resumable_cache() {
    let (_dir, dump) = sandboxed();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();
    let size = source.size().await.unwrap();
    let mode = CacheMode::enabled(cache_path.clone());

    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
    let first_block_end = eager.blocks().next().expect("the fixture has blocks").end_offset;
    assert!(first_block_end < size, "sanity: there is a file left after the first block");

    let dying = FailsPast { inner: &source, fail_at: first_block_end };
    let slow = ScanOptions { chunk_size_bytes: 1, ..ScanOptions::default() };
    let err = map_file(&dying, &slow, &mode, &StatisticsRequest::DATA)
        .await
        .expect_err("the source dies mid-scan");
    assert!(matches!(err, pgdump_query::Error::Io(_)), "{err:?}");

    let status = cache::load(&cache_path, &source).await.unwrap();
    let CacheStatus::Incomplete { index, total_size, .. } = status else {
        panic!("an interrupted scan must leave an incomplete cache, got {status:?}");
    };
    assert_eq!(total_size, size);
    assert_eq!(index.scanned_through, first_block_end, "banked exactly the block that completed");
    assert_eq!(index.blocks().count(), 1);

    // Finishing it from there agrees with one eager pass, like any other
    // partial cache.
    let run =
        map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA).await.unwrap();
    assert_eq!(run.resumed_from, first_block_end);
    assert_matches_eager(&run.index, &eager, "resumed from an interrupted scan");
}

/// A full scan recovers **every** database's DDL, not just the first's. A
/// query only ever captures the first (`scan_preamble`'s bounded prepass), and
/// that is all it may honestly claim while the map is short of EOF; `map_file`
/// reaches the other boundary `dump_metadata_from_spans` may be called at, so
/// `pgdt parse` leaves every database `preamble_complete`.
#[tokio::test]
async fn a_full_scan_recovers_every_databases_ddl() {
    for version in [13, 16, 18] {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures")
            .join(version.to_string())
            .join("edge_cases/create.sql");
        let content = std::fs::read_to_string(&fixture).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let dump = dir.path().join("multidb.sql");
        std::fs::write(&dump, format!("{content}{}", content.replace("pgdt_fixture", "pgdt_2")))
            .unwrap();
        let source = LocalFileSource::open(&dump).unwrap();
        let mode = CacheMode::enabled(cache::colocated_path(&dump));

        let index = map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA)
            .await
            .unwrap()
            .index;
        let metadata = index.metadata.as_ref().expect("a full scan always has metadata");
        assert_eq!(metadata.databases.len(), 2, "pg_dump {version}");
        for db in &metadata.databases {
            assert!(db.preamble_complete, "pg_dump {version}: {:?}", db.name);
            assert!(!db.tables.is_empty(), "pg_dump {version}: {:?} has DDL", db.name);
        }
    }
}

/// A [`ByteRangeSource`] that trips a cancellation flag once the scan asks for
/// a byte at or past `trip` — the `SIGINT` these tests cannot deliver,
/// arriving at a file offset instead of at a wall-clock moment. The read
/// itself succeeds, so the scan is stopped by the flag alone and never by an
/// I/O failure.
struct CancelsPast<'a> {
    inner: &'a LocalFileSource,
    trip: u64,
    cancel: Arc<Cancellation>,
}

impl ByteRangeSource for CancelsPast<'_> {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = pgdump_query::Result<bytes::Bytes>> + Send + '_>,
    > {
        if offset >= self.trip {
            self.cancel.cancel();
        }
        self.inner.read_range(offset, len)
    }

    fn size(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = pgdump_query::Result<u64>> + Send + '_>>
    {
        self.inner.size()
    }

    fn modified(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = pgdump_query::Result<Option<std::time::SystemTime>>>
                + Send
                + '_,
        >,
    > {
        self.inner.modified()
    }

    /// The three the leader reads, forwarded so that a scan stating a
    /// `Parallelism` through this wrapper is cut exactly as it would be through
    /// the file itself — the default `partitions` declines to advise, and a
    /// source that declines is never split.
    fn hint_read_size(&self, len: usize) {
        self.inner.hint_read_size(len);
    }

    fn partitions(&self, range: std::ops::Range<u64>) -> pgdump_query::Partitioning {
        self.inner.partitions(range)
    }

    fn hint_parallelism(&self, parallelism: Parallelism) {
        self.inner.hint_parallelism(parallelism);
    }
}

/// A [`ByteRangeSource`] that answers a read at or past `trip` the way a
/// source whose wait is a request answers one: it trips the cancellation and
/// **drops the read**, handing back `Error::ScanCancelled` instead of bytes
/// (`docs/design/decisions.md`, "D26"). [`CancelsPast`] is the other provider
/// at the same offset — flag set, bytes delivered — so a pair of tests over
/// the two says whether the run's shape follows the run or the provider.
struct DropsPast<'a> {
    inner: &'a LocalFileSource,
    trip: u64,
    cancel: Arc<Cancellation>,
}

impl ByteRangeSource for DropsPast<'_> {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = pgdump_query::Result<bytes::Bytes>> + Send + '_>,
    > {
        if offset >= self.trip {
            self.cancel.cancel();
            return Box::pin(std::future::ready(Err(pgdump_query::Error::ScanCancelled {
                scanned_through: offset,
            })));
        }
        self.inner.read_range(offset, len)
    }

    fn size(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = pgdump_query::Result<u64>> + Send + '_>>
    {
        self.inner.size()
    }

    fn modified(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = pgdump_query::Result<Option<std::time::SystemTime>>>
                + Send
                + '_,
        >,
    > {
        self.inner.modified()
    }

    /// Forwarded for the reason [`CancelsPast`] forwards them.
    fn hint_read_size(&self, len: usize) {
        self.inner.hint_read_size(len);
    }

    fn partitions(&self, range: std::ops::Range<u64>) -> pgdump_query::Partitioning {
        self.inner.partitions(range)
    }

    fn hint_parallelism(&self, parallelism: Parallelism) {
        self.inner.hint_parallelism(parallelism);
    }
}

/// **The interrupt guard, reached inside a region the workers were splitting.**
/// A `COPY` block long enough to need several windows is cancelled between two
/// of them, which is a fourth place the flag is read and the only one that can
/// abandon a block already partly scanned: nothing about it is banked, the last
/// spliced watermark stands, and the scan is a resume point like any other.
///
/// The flag trips on the first read at or past the block's `data_offset`, which
/// is the leader's own first read — so window one dispatches, window two sees
/// the flag, and the interior is left unfinished by construction rather than by
/// timing.
#[tokio::test]
async fn a_cancelled_parallel_region_banks_nothing_and_stays_resumable() {
    let mut file = b"COPY public.t (a) FROM stdin;\n".to_vec();
    let data_offset = file.len() as u64;
    for i in 0..2000 {
        file.extend_from_slice(format!("{i}\n").as_bytes());
    }
    file.extend_from_slice(b"\\.\n\nSELECT 1;\n");
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("dump.sql");
    std::fs::write(&dump, &file).unwrap();
    let source = LocalFileSource::open(&dump).unwrap();
    let mode = CacheMode::enabled(cache::colocated_path(&dump));

    let cancel = Arc::new(Cancellation::new());
    let tripping = CancelsPast { inner: &source, trip: data_offset, cancel: Arc::clone(&cancel) };
    let options = ScanOptions {
        chunk_size_bytes: 64,
        cancel: Some(Arc::clone(&cancel)),
        parallelism: Parallelism::workers(4, DEFAULT_MEMORY_BUDGET),
        ..ScanOptions::default()
    };

    let run = map_file(&tripping, &options, &mode, &StatisticsRequest::DATA).await.unwrap();
    assert!(run.interrupted, "a cancelled region interrupts the scan");
    assert_eq!(run.index.scanned_through, 0, "the abandoned block banks nothing");
    assert_eq!(run.index.blocks().count(), 0);

    // And it resumes into the index one eager pass builds.
    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
    let resumed =
        map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA).await.unwrap();
    assert!(!resumed.interrupted);
    assert_matches_eager(&resumed.index, &eager, "resumed from a cancelled parallel region");
}

/// A [`ByteRangeSource`] whose reads fail **out of file order**: everything at
/// or past `fail_at` fails, and the read that starts exactly at `fail_at`
/// yields to the runtime eight times before it does, where every later one
/// fails on its first poll.
///
/// That is the arrangement the ordering rule exists for and the one a real
/// source produces only by luck — four workers reading four pieces of one
/// region, the earliest of them the slowest to come back. The failure names its
/// own offset, so the test can say *which* read the scan reported.
struct FailsOutOfOrder<'a> {
    inner: &'a LocalFileSource,
    fail_at: u64,
}

impl ByteRangeSource for FailsOutOfOrder<'_> {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = pgdump_query::Result<bytes::Bytes>> + Send + '_>,
    > {
        Box::pin(async move {
            if offset >= self.fail_at {
                if offset == self.fail_at {
                    for _ in 0..8 {
                        tokio::task::yield_now().await;
                    }
                }
                return Err(pgdump_query::Error::Io(std::io::Error::other(format!(
                    "read failed at {offset}"
                ))));
            }
            self.inner.read_range(offset, len).await
        })
    }

    fn size(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = pgdump_query::Result<u64>> + Send + '_>>
    {
        self.inner.size()
    }

    fn modified(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = pgdump_query::Result<Option<std::time::SystemTime>>>
                + Send
                + '_,
        >,
    > {
        self.inner.modified()
    }

    /// Forwarded for the reason [`CancelsPast`] forwards them: a scan stating a
    /// `Parallelism` through this wrapper must be cut exactly as it would be
    /// through the file itself, and the default `partitions` declines to
    /// advise.
    fn hint_read_size(&self, len: usize) {
        self.inner.hint_read_size(len);
    }

    fn partitions(&self, range: std::ops::Range<u64>) -> pgdump_query::Partitioning {
        self.inner.partitions(range)
    }

    fn hint_parallelism(&self, parallelism: Parallelism) {
        self.inner.hint_parallelism(parallelism);
    }
}

/// **The lowest-offset error is the one a split region raises**
/// (`docs/design/decisions.md`, "D52"). Four workers each fail on their own piece of one `COPY` block's
/// interior, and the *earliest* piece is deliberately the last to answer — so a
/// scheduler that raised whichever failure arrived first would report the
/// second worker's offset, and a user re-running to confirm the failure would
/// get a different message each time.
///
/// The window is four 64-byte pieces starting at the block's `data_offset`, so
/// the four failing offsets are known exactly and the assertion names one
/// rather than a set.
#[tokio::test]
async fn the_lowest_offset_error_is_the_one_a_split_region_raises() {
    let mut file = b"COPY public.t (a) FROM stdin;\n".to_vec();
    let data_offset = file.len() as u64;
    for i in 0..2000 {
        file.extend_from_slice(format!("{i}\n").as_bytes());
    }
    file.extend_from_slice(b"\\.\n\nSELECT 1;\n");
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("dump.sql");
    std::fs::write(&dump, &file).unwrap();
    let source = LocalFileSource::open(&dump).unwrap();
    let mode = CacheMode::DISABLED;

    let failing = FailsOutOfOrder { inner: &source, fail_at: data_offset };
    let options = ScanOptions {
        chunk_size_bytes: 64,
        parallelism: Parallelism::workers(4, DEFAULT_MEMORY_BUDGET),
        ..ScanOptions::default()
    };

    let err = map_file(&failing, &options, &mode, &StatisticsRequest::DATA)
        .await
        .expect_err("every worker's read fails");
    assert_eq!(
        err.to_string(),
        format!("io error: read failed at {data_offset}"),
        "the earliest piece's failure is the one raised, however late it arrived"
    );
}

/// **The interrupt guard.** A cancelled scan is not an error and not a lie: it
/// reports `interrupted`, the index it returns stops at the last **spliced**
/// watermark, and the cache on disk holds exactly that — every exit holding a
/// watermark saves, so what is on disk is never behind what is in hand. (The
/// one exit that holds none writes nothing at all:
/// `a_dropped_read_inside_the_prepass_banks_and_writes_nothing`.)
///
/// The flag trips one byte past the first block's end, with one-byte reads so
/// that offset is a read boundary: the block's `CopyEnd` has been processed,
/// nothing after it has. **The first block of a segment is always a spliced
/// watermark** — `SaveThrottle::new` starts due — so "last spliced" and "last
/// completed" coincide here, which is what keeps this assertion exact rather
/// than timing-dependent.
#[tokio::test]
async fn a_cancelled_map_file_reports_it_and_banks_what_it_scanned() {
    let (_dir, dump) = sandboxed();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();
    let size = source.size().await.unwrap();
    let mode = CacheMode::enabled(cache_path.clone());

    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
    let first_block_end = eager.blocks().next().expect("the fixture has blocks").end_offset;
    assert!(first_block_end < size, "sanity: there is a file left after the first block");

    let cancel = Arc::new(Cancellation::new());
    let tripping =
        CancelsPast { inner: &source, trip: first_block_end, cancel: Arc::clone(&cancel) };
    let options = ScanOptions {
        chunk_size_bytes: 1,
        cancel: Some(Arc::clone(&cancel)),
        ..ScanOptions::default()
    };

    let run = map_file(&tripping, &options, &mode, &StatisticsRequest::DATA).await.unwrap();
    assert!(run.interrupted, "a cancelled scan says so");
    assert_eq!(run.index.scanned_through, first_block_end, "banked the first spliced watermark");
    assert!(!run.index.is_complete(size), "and does not claim the whole file");

    let status = cache::load(&cache_path, &source).await.unwrap();
    let CacheStatus::Incomplete { index, .. } = status else {
        panic!("a cancelled scan must leave an incomplete cache, got {status:?}");
    };
    assert_eq!(index.scanned_through, first_block_end);
    assert_eq!(index.blocks().count(), 1);

    // And it is a resume point like any other.
    let resumed =
        map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA).await.unwrap();
    assert!(!resumed.interrupted);
    assert_eq!(resumed.resumed_from, first_block_end);
    assert_matches_eager(&resumed.index, &eager, "resumed from a cancelled scan");
}

/// **The same guard, reached through a dropped read.** A source that answers
/// a cancellation by abandoning the request in flight raises it where the
/// polled provider would have delivered the chunk, so the scan hears the
/// interrupt one read earlier and by another route. The run is the same run:
/// it reports `interrupted`, banks the same watermark, leaves the same
/// incomplete cache, and resumes into the eager index — which is what says the
/// shape follows the run and not the provider
/// (`docs/design/decisions.md`, "D26").
#[tokio::test]
async fn a_dropped_read_is_the_same_interrupt_as_the_flag() {
    let (_dir, dump) = sandboxed();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();
    let size = source.size().await.unwrap();
    let mode = CacheMode::enabled(cache_path.clone());

    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
    let first_block_end = eager.blocks().next().expect("the fixture has blocks").end_offset;

    let cancel = Arc::new(Cancellation::new());
    let dropping = DropsPast { inner: &source, trip: first_block_end, cancel: Arc::clone(&cancel) };
    let options = ScanOptions {
        chunk_size_bytes: 1,
        cancel: Some(Arc::clone(&cancel)),
        ..ScanOptions::default()
    };

    let run = map_file(&dropping, &options, &mode, &StatisticsRequest::DATA)
        .await
        .expect("a dropped read is an interrupted run, not an error");
    assert!(run.interrupted, "a dropped read says so");
    assert_eq!(run.index.scanned_through, first_block_end, "banked the first spliced watermark");
    assert!(!run.index.is_complete(size), "and does not claim the whole file");

    let status = cache::load(&cache_path, &source).await.unwrap();
    let CacheStatus::Incomplete { index, .. } = status else {
        panic!("a dropped read must leave an incomplete cache, got {status:?}");
    };
    assert_eq!(index.scanned_through, first_block_end);

    let resumed =
        map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA).await.unwrap();
    assert!(!resumed.interrupted);
    assert_eq!(resumed.resumed_from, first_block_end);
    assert_matches_eager(&resumed.index, &eager, "resumed from a dropped read");
}

/// **A read dropped inside the preamble prepass banks nothing and writes
/// nothing.** The prepass holds every span aside until it is whole
/// (`docs/design/decisions.md`, "D26"), so the arm that catches its dropped
/// read has an index describing zero bytes in hand; saving that would leave a
/// cache claiming a size the file had at that instant, which a resume against
/// a file still being written then refuses outright. The counterpart is
/// `a_scan_cancelled_before_it_starts_maps_only_the_preamble`, where the
/// prepass *completed* and the cache it wrote is a real resume point — the
/// difference being whether there was anything to bank, not which provider
/// raised the interrupt.
#[tokio::test]
async fn a_dropped_read_inside_the_prepass_banks_and_writes_nothing() {
    let (_dir, dump) = sandboxed();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();
    let mode = CacheMode::enabled(cache_path.clone());
    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();

    // Every read drops, so the first one the prepass makes is the interrupt.
    let cancel = Arc::new(Cancellation::new());
    let dropping = DropsPast { inner: &source, trip: 0, cancel: Arc::clone(&cancel) };
    let options = ScanOptions { cancel: Some(Arc::clone(&cancel)), ..ScanOptions::default() };

    let run = map_file(&dropping, &options, &mode, &StatisticsRequest::DATA)
        .await
        .expect("a dropped read is an interrupted run, not an error");
    assert!(run.interrupted, "a dropped prepass read says so");
    assert_eq!(run.index.scanned_through, 0, "nothing was banked");
    assert!(run.index.metadata.is_none(), "and no half-read preamble came back");
    assert!(!cache_path.exists(), "with nothing to bank, no cache is written at all");

    // And the next run is a cold scan, not a resume onto an empty prefix.
    let resumed =
        map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA).await.unwrap();
    assert!(!resumed.interrupted);
    assert_eq!(resumed.resumed_from, 0);
    assert_matches_eager(&resumed.index, &eager, "scanned cold after a dropped prepass read");
}

/// **A read dropped inside the back-fill is the back-fill's own interrupt.**
/// The counts a polled stop reports are reported here too, which is what keeps
/// `pgdt parse`'s two interrupt sentences apart: the map is whole and the
/// re-read is partial, not the other way round.
#[tokio::test]
async fn a_dropped_read_inside_the_backfill_reports_the_backfills_counts() {
    let (_dir, dump) = sandboxed();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();
    let mode = CacheMode::enabled(cache_path.clone());
    let bare = map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::METADATA)
        .await
        .unwrap();
    let blocks = bare.index.blocks().count();
    let second = bare.index.blocks().nth(1).unwrap().data_offset;

    let cancel = Arc::new(Cancellation::new());
    let dropping = DropsPast { inner: &source, trip: second, cancel: Arc::clone(&cancel) };
    let options = ScanOptions {
        chunk_size_bytes: 1,
        cancel: Some(Arc::clone(&cancel)),
        ..ScanOptions::default()
    };
    let run = map_file(&dropping, &options, &mode, &StatisticsRequest::DATA)
        .await
        .expect("a dropped read is an interrupted run, not an error");
    assert!(run.interrupted, "a cancelled back-fill says so");
    assert_eq!((run.lacking_statistics, run.backfilled), (blocks, 1));
}

/// **An interrupted back-fill banks the blocks it re-read and resumes into
/// the rest.** A map holding no statistics is asked for them, and the flag
/// trips on the first read of the second block's data — one-byte reads again,
/// so the first block has closed and been banked, the throttle starting due.
/// The cache is complete and holds the first block's statistics alone; a
/// second run re-reads only the blocks still lacking them, into the map one
/// gathering pass builds.
#[tokio::test]
async fn an_interrupted_backfill_banks_the_blocks_it_reread() {
    let (_dir, dump) = sandboxed();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();
    let mode = CacheMode::enabled(cache_path.clone());
    let bare = map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::METADATA)
        .await
        .unwrap();
    let blocks = bare.index.blocks().count();
    assert!(blocks > 2, "sanity: blocks are left after the second");
    let second = bare.index.blocks().nth(1).unwrap().data_offset;

    let cancel = Arc::new(Cancellation::new());
    let tripping = CancelsPast { inner: &source, trip: second, cancel: Arc::clone(&cancel) };
    let options = ScanOptions {
        chunk_size_bytes: 1,
        cancel: Some(Arc::clone(&cancel)),
        ..ScanOptions::default()
    };
    let run = map_file(&tripping, &options, &mode, &StatisticsRequest::DATA).await.unwrap();
    assert!(run.interrupted, "a cancelled back-fill says so");
    assert_eq!((run.lacking_statistics, run.backfilled), (blocks, 1));

    let status = cache::load(&cache_path, &source).await.unwrap();
    let CacheStatus::Valid { index, .. } = status else {
        panic!("a back-fill leaves the map complete, got {status:?}");
    };
    let gathered: Vec<bool> = index.blocks().map(|b| b.statistics.is_some()).collect();
    assert_eq!(gathered.iter().filter(|g| **g).count(), 1, "{gathered:?}");
    assert!(gathered[0], "the first block is the one banked: {gathered:?}");

    let resumed =
        map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA).await.unwrap();
    assert!(!resumed.interrupted);
    assert_eq!((resumed.lacking_statistics, resumed.backfilled), (blocks - 1, blocks - 1));
    let straight =
        map_file(&source, &ScanOptions::default(), &CacheMode::DISABLED, &StatisticsRequest::DATA)
            .await
            .unwrap();
    assert_eq!(resumed.index.spans, straight.index.spans);
}

/// A flag set before the scan starts stops it at the first thing it can stop
/// at, and still writes the cache — the degenerate case of the rule above, and
/// the one that would otherwise return an index claiming to describe a file it
/// never opened.
///
/// What it *does* claim is the preamble. `scan_preamble` ignores the flag
/// deliberately (`ScanOptions::cancel`): a stop inside it could not be told
/// from reaching the first `COPY` header, so a truncated preamble would be
/// cached as a complete one. It polls no flag and runs before `map_forward`'s
/// first check, and it banks its result — so even the most immediate *polled*
/// interrupt leaves a cache whose columns resolve rather than one that reports
/// `not declared` for all of them. It is not uncancellable: the cancellation
/// is announced to the source, so a read in flight can be dropped inside it
/// (`a_dropped_read_inside_the_prepass_banks_and_writes_nothing`).
#[tokio::test]
async fn a_scan_cancelled_before_it_starts_maps_only_the_preamble() {
    let (_dir, dump) = sandboxed();
    let source = LocalFileSource::open(&dump).unwrap();
    let mode = CacheMode::enabled(cache::colocated_path(&dump));

    let options = ScanOptions { cancel: Some(already_cancelled()), ..ScanOptions::default() };
    let run = map_file(&source, &options, &mode, &StatisticsRequest::DATA).await.unwrap();

    assert!(run.interrupted);
    assert_eq!(run.resumed_from, 0, "the prepass is this run's work, not a resume point");
    assert!(run.index.blocks().next().is_none(), "no block was mapped");
    assert!(run.index.scanned_through > 0, "the preamble was read");
    let metadata = run.index.metadata.as_ref().expect("the preamble prepass banked its DDL");
    assert!(metadata.databases.first().unwrap().preamble_complete);

    // On disk, and resumable from where the prepass stopped.
    let CacheLoad::Index(reloaded) = mode.load(&source).await.unwrap() else {
        panic!("the prepass wrote a cache")
    };
    assert_eq!(reloaded.scanned_through, run.index.scanned_through);
    assert_eq!(reloaded.metadata, run.index.metadata);
}

/// A cancelled **query** is an error, not a short stream. `map_forward` is one
/// loop with two callers, and the second one cannot report partiality: rows
/// from the blocks a cancelled mapping pass happened to reach are a prefix of
/// the answer with nothing saying so.
#[tokio::test]
async fn a_cancelled_query_errors_rather_than_returning_a_prefix() {
    let (_dir, dump) = sandboxed();
    let source = LocalFileSource::open(&dump).unwrap();
    let mode = CacheMode::enabled(cache::colocated_path(&dump));

    let options = ScanOptions { cancel: Some(already_cancelled()), ..ScanOptions::default() };
    let mut stream =
        table_stream(&source, "public.widgets", options, QueryOptions::default(), None, mode);
    let err = stream.next().await.expect("the stream yields once").expect_err("cancelled");
    assert!(matches!(err, pgdump_query::Error::ScanCancelled { .. }), "{err:?}");
}

/// Two copies of `edge_cases/create.sql` concatenated, the second's database
/// renamed. A `pg_dumpall` and a bare `cat a.sql b.sql` are different shapes
/// (I9) and this is the only fixture for the second one, so it stays beside
/// the real dump rather than being replaced by it. The name is deliberately
/// not `pgdt_fixture_2`, which the synthetic multi-database helpers elsewhere
/// in the suite use: with both constructions in one file, a failure naming
/// `pgdt_fixture_2` would not say which it came from.
fn multidb(version: u32) -> (tempfile::TempDir, PathBuf) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(version.to_string())
        .join("edge_cases/create.sql");
    let content = std::fs::read_to_string(&fixture).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("multidb.sql");
    std::fs::write(&dump, format!("{content}{}", content.replace("pgdt_fixture", "pgdt_2")))
        .unwrap();
    (dir, dump)
}

/// Every banked block's columns, resolved the way `pgdt info` resolves them:
/// against the index's own metadata, with no census (a partial index may not
/// believe one — `DumpIndex::is_complete`).
fn outcomes(index: &DumpIndex) -> Vec<(String, Vec<ColumnResolution>)> {
    let metadata = index.metadata.as_ref();
    index
        .blocks()
        .map(|b| {
            let schema = resolve_columns(
                &b.header.qualified_name(),
                &b.header.columns,
                metadata,
                b.database.as_deref(),
                SchemaMode::Typed,
                &[],
            );
            (b.header.qualified_name(), schema.columns)
        })
        .collect()
}

/// **What the preamble prepass buys an interrupted `parse`.** Without it, a
/// cache with no `DumpMetadata` would leave `resolve_columns` answering
/// `NotDeclared` — "the dump never explained this column", final — for every
/// column of every block it had just banked, with the `CREATE TABLE` sitting
/// in the same cache's spans. The preamble prepass is what makes those blocks
/// genuinely *typed* rather than merely labelled with better advice.
#[tokio::test]
async fn an_interrupted_scans_banked_blocks_resolve_against_real_ddl() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("dump.sql");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/16/edge_cases/default.sql"),
        &dump,
    )
    .unwrap();
    let source = LocalFileSource::open(&dump).unwrap();
    let mode = CacheMode::enabled(cache::colocated_path(&dump));

    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
    let trip = eager.blocks().nth(1).expect("the fixture has two blocks").end_offset;

    let cancel = Arc::new(Cancellation::new());
    let tripping = CancelsPast { inner: &source, trip, cancel: Arc::clone(&cancel) };
    let options = ScanOptions {
        chunk_size_bytes: 1,
        cancel: Some(Arc::clone(&cancel)),
        ..ScanOptions::default()
    };
    let run = map_file(&tripping, &options, &mode, &StatisticsRequest::DATA).await.unwrap();

    assert!(run.interrupted);
    assert!(!run.index.is_complete(source.size().await.unwrap()));
    let checked = outcomes(&run.index);
    assert!(!checked.is_empty(), "the scan banked blocks with column lists to check");
    for (table, columns) in &checked {
        assert!(
            !columns.contains(&ColumnResolution::NotDeclared)
                && !columns.contains(&ColumnResolution::MetadataNotScanned),
            "{table}: {columns:?}"
        );
    }
}

/// **The recurring boundary, which a prepass alone does not reach.** I1 lets
/// `dump_metadata_from_spans` be called at each database's first `COPY` block
/// as well as at EOF, and the second kind of point recurs once per `\connect`.
/// Without it a `parse` interrupted inside database 2 holds DDL for database 1
/// alone — having read database 2's entire preamble on the way past — and
/// every one of database 2's banked blocks reports `MetadataNotScanned`.
///
/// `dump` must hold at least two `COPY`-carrying databases, `second` naming
/// the later one; the scan is cut at the end of that database's first block,
/// so the boundary has been crossed exactly twice when the cache is banked.
async fn assert_an_interrupt_inside(dump: &Path, second: &str, label: &str) {
    let source = LocalFileSource::open(dump).unwrap();
    let mode = CacheMode::enabled(cache::colocated_path(dump));

    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
    let trip = eager
        .blocks()
        .find(|b| b.database.as_deref() == Some(second))
        .unwrap_or_else(|| panic!("{label}: {second} has blocks"))
        .end_offset;

    let cancel = Arc::new(Cancellation::new());
    let tripping = CancelsPast { inner: &source, trip, cancel: Arc::clone(&cancel) };
    let options = ScanOptions {
        chunk_size_bytes: 1,
        cancel: Some(Arc::clone(&cancel)),
        ..ScanOptions::default()
    };
    let run = map_file(&tripping, &options, &mode, &StatisticsRequest::DATA).await.unwrap();
    assert!(run.interrupted, "{label}");

    // Every database that banked a block has that database's own DDL — not
    // merely *some* DDL, which is what an index carrying only database 1's
    // would also satisfy. A segment with no blocks (`template1`, and
    // `postgres` past the stopping point) is not this test's subject: it has
    // no rows to type.
    let banked: std::collections::BTreeSet<String> =
        run.index.blocks().filter_map(|b| b.database.clone()).collect();
    assert!(banked.contains(second), "{label}: the scan banked a {second} block");
    let metadata = run.index.metadata.as_ref().unwrap_or_else(|| panic!("{label}: metadata"));
    for name in &banked {
        let db = metadata
            .databases
            .iter()
            .find(|d| d.name.as_deref() == Some(name.as_str()))
            .unwrap_or_else(|| panic!("{label}: {name} has an entry in {metadata:?}"));
        assert!(db.preamble_complete, "{label}: {name}");
        assert!(!db.tables.is_empty(), "{label}: {name} has DDL");
    }

    for (table, columns) in outcomes(&run.index) {
        assert!(
            !columns.contains(&ColumnResolution::MetadataNotScanned),
            "{label}: {table}: {columns:?}"
        );
    }

    // And finishing it is still span for span what one eager pass gives.
    let resumed =
        map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA).await.unwrap();
    assert_matches_eager(&resumed.index, &eager, &format!("{label}: resumed"));
}

/// The boundary against a file `pg_dump` actually wrote. `edge_cases/dumpall`
/// carries `COPY` blocks in two consecutive segments — `pgdt_fixture` and
/// `pgdt_tenant`, in that order by I30 — and `pgdt_tenant.public.widgets`
/// repeats the earlier database's table name with a different type on every
/// column, so an index that resolved these blocks against `pgdt_fixture`'s DDL
/// would answer wrongly rather than merely fall silent.
#[tokio::test]
async fn an_interrupt_inside_a_later_database_types_the_segments_it_finished() {
    for version in [13, 16, 18] {
        // Copied out of `fixtures/`, since the run writes a colocated cache.
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures")
            .join(version.to_string())
            .join("edge_cases/dumpall.sql");
        let dir = tempfile::tempdir().unwrap();
        let dump = dir.path().join("dumpall.sql");
        std::fs::copy(&fixture, &dump).unwrap();
        assert_an_interrupt_inside(&dump, "pgdt_tenant", &format!("pg_dump {version} dumpall"))
            .await;
    }
}

/// **Resolution keys by database, and the fixture can now show it.**
/// `edge_cases/dumpall` holds `public.widgets` in *both* data-carrying
/// databases with a different type on every column, so the same qualified name
/// must answer differently depending on which segment the block came from. A
/// resolver that reached for the first database's DDL — or for whichever entry
/// happened to be last — fails here rather than passing quietly; on a file
/// whose segments share a schema it could not be caught at all.
#[tokio::test]
async fn one_table_name_in_two_databases_resolves_to_each_databases_own_types() {
    for version in [13, 16, 18] {
        let dump = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures")
            .join(version.to_string())
            .join("edge_cases/dumpall.sql");
        let source = LocalFileSource::open(&dump).unwrap();
        let index = map_file(
            &source,
            &ScanOptions::default(),
            &CacheMode::DISABLED,
            &StatisticsRequest::DATA,
        )
        .await
        .unwrap()
        .index;

        let types_of = |database: &str| {
            let block = index
                .blocks()
                .find(|b| {
                    b.database.as_deref() == Some(database)
                        && b.header.qualified_name() == "public.widgets"
                })
                .unwrap_or_else(|| panic!("pg_dump {version}: {database} has a widgets block"));
            let schema = resolve_columns(
                &block.header.qualified_name(),
                &block.header.columns,
                index.metadata.as_ref(),
                block.database.as_deref(),
                SchemaMode::Typed,
                &[],
            );
            assert!(
                schema.columns.iter().all(|c| *c == ColumnResolution::Mapped),
                "pg_dump {version}: {database}: {:?}",
                schema.columns
            );
            schema.schema.fields().iter().map(|f| f.data_type().clone()).collect::<Vec<_>>()
        };

        assert_eq!(
            types_of("pgdt_fixture"),
            vec![
                DataType::Int32,
                DataType::Utf8View,
                DataType::Utf8View,
                DataType::Boolean,
                DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            ],
            "pg_dump {version}"
        );
        assert_eq!(
            types_of("pgdt_tenant"),
            vec![
                DataType::Int64,
                DataType::FixedSizeBinary(16),
                DataType::Binary,
                DataType::Int16,
                DataType::Date32,
            ],
            "pg_dump {version}"
        );
    }
}

/// The same boundary in a *concatenated* file, which I9 makes a different
/// shape from a `pg_dumpall` — two whole dumps, each with its own header and
/// trailer, rather than one cluster's segments. Nothing else covers it.
#[tokio::test]
async fn an_interrupt_inside_a_later_database_of_a_concatenated_file_types_what_it_finished() {
    for version in [13, 16, 18] {
        let (_dir, dump) = multidb(version);
        assert_an_interrupt_inside(&dump, "pgdt_2", &format!("pg_dump {version} concatenated"))
            .await;
    }
}
