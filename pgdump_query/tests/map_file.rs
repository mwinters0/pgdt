//! `stream::map_file` — `pgdq parse`'s scan
//! (`docs/design/architecture.md`, "CLI surface").
//!
//! **The claim these tests exist for**: a scan that stopped partway leaves a
//! cache that a later `map_file` finishes, and the finished index is
//! indistinguishable from one `build_index` produced in a single pass. That is
//! the property that would fail *silently* — a resumed scan that dropped a
//! span, mis-seamed a boundary, or lost a census would still print a plausible
//! listing.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use arrow::datatypes::{DataType, TimeUnit};
use futures::StreamExt;
use pgdump_query::cache::{CacheMode, CacheStatus};
use pgdump_query::resolve::{ColumnResolution, SchemaMode, resolve_columns};
use pgdump_query::{
    ByteRangeSource, DumpIndex, LocalFileSource, QueryOptions, ScanOptions, build_index, cache,
    map_file, preamble_only, table_stream,
};

/// A private copy of `tests/data/edge_cases.sql` in a fresh tempdir, so each
/// test may freely write the colocated `.dqcache` beside it — same fixture and
/// same convention as `tests/query_cache.rs`, which is what makes the "a query
/// stops at its target" setup below behave identically here.
mod common;
use common::sandboxed_edge_cases as sandboxed;

/// Assert that a resumed/finished index is what a single eager pass produces.
/// Field by field before the whole-struct comparison, so a failure names which
/// half drifted rather than dumping two entire indexes.
fn assert_matches_eager(actual: &DumpIndex, eager: &DumpIndex, label: &str) {
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
    for schema_dir in ["edge_cases", "objects", "partitions", "types"] {
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
            let mode = CacheMode::Enabled(cache::colocated_path(&dump));

            let run = map_file(&source, &ScanOptions::default(), &mode).await.unwrap();
            assert_eq!(run.resumed_from, 0, "{schema_dir}/{flag_set}: nothing to resume from");
            assert!(!run.interrupted);
            let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
            assert_matches_eager(&run.index, &eager, &format!("{schema_dir}/{flag_set}"));
        }
    }
}

/// **The phase's central claim.** A query that stopped at its target leaves a
/// genuinely partial cache; `map_file` finishes it, and the result is span for
/// span — census included — what one eager pass gives.
#[tokio::test]
async fn a_partial_cache_is_finished_into_the_same_index_an_eager_scan_builds() {
    let (_dir, dump) = sandboxed();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();
    let size = source.size().await.unwrap();
    let mode = CacheMode::Enabled(cache_path.clone());

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
    let partial = mode.load(&source).await.unwrap().expect("the query wrote a cache");
    assert!(partial.scanned_through < size, "sanity: the query really did stop short");

    let run = map_file(&source, &ScanOptions::default(), &mode).await.unwrap();
    assert_eq!(run.resumed_from, partial.scanned_through, "resumed at the cache's own frontier");
    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
    assert!(
        partial.blocks().count() < eager.blocks().count(),
        "sanity: the query left blocks unmapped for this to find"
    );
    assert_matches_eager(&run.index, &eager, "resumed from a query's partial cache");

    // And what landed on disk is the finished index, not the partial one.
    let reloaded = mode.load(&source).await.unwrap().unwrap();
    assert_eq!(reloaded.scanned_through, size);
    assert_eq!(reloaded.spans, eager.spans);
}

/// The other shape of partial cache: `--preamble-only`, which stops at the
/// first `COPY` header and leaves an `Unscanned` tail covering everything
/// after it. Resuming from it must splice onto the prepass's spans rather than
/// starting a second, parallel map.
#[tokio::test]
async fn a_preamble_only_cache_is_finished_into_the_same_index() {
    let (_dir, dump) = sandboxed();
    let source = LocalFileSource::open(&dump).unwrap();
    let mode = CacheMode::Enabled(cache::colocated_path(&dump));

    preamble_only(&source, &ScanOptions::default(), &mode).await.unwrap();
    let preamble_cache = mode.load(&source).await.unwrap().expect("preamble_only wrote a cache");
    assert!(preamble_cache.scanned_through > 0);
    assert!(preamble_cache.blocks().next().is_none(), "the prepass stops before the first block");

    let run = map_file(&source, &ScanOptions::default(), &mode).await.unwrap();
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
    let mode = CacheMode::Enabled(cache::colocated_path(&dump));

    let first = map_file(&source, &ScanOptions::default(), &mode).await.unwrap().index;
    let second = map_file(&source, &ScanOptions::default(), &mode).await.unwrap();

    assert_eq!(second.resumed_from, size, "the cache already covered the file");
    assert_eq!(first, second.index);
    assert!(
        !second.index.diagnostics.is_empty(),
        "the TOC-coverage figure is recomputed, not read back from a cache that never stored it"
    );
}

/// A [`ByteRangeSource`] that refuses to read past `fail_at` — the killed
/// `pgdq parse` these tests cannot produce with a signal. Everything below
/// `fail_at` reads normally, so the scan makes real progress and banks real
/// saves before it dies.
struct FailsPast<'a> {
    inner: &'a LocalFileSource,
    fail_at: u64,
}

impl ByteRangeSource for FailsPast<'_> {
    async fn read_range(&self, offset: u64, len: usize) -> pgdump_query::Result<bytes::Bytes> {
        if offset >= self.fail_at {
            return Err(pgdump_query::Error::Io(std::io::Error::other("simulated interruption")));
        }
        self.inner.read_range(offset, len).await
    }

    async fn size(&self) -> pgdump_query::Result<u64> {
        self.inner.size().await
    }

    async fn modified(&self) -> pgdump_query::Result<Option<std::time::SystemTime>> {
        self.inner.modified().await
    }
}

/// **The write-per-block claim, and the reason resuming is worth building.**
/// Under the previous `parse` the cache was written only once the whole scan
/// returned, so an interrupted scan left nothing at all — neither something to
/// resume from nor something to report. Now every completed block is banked at
/// a `CopyEnd` watermark, which is a resumable point by construction.
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
    let mode = CacheMode::Enabled(cache_path.clone());

    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
    let first_block_end = eager.blocks().next().expect("the fixture has blocks").end_offset;
    assert!(first_block_end < size, "sanity: there is a file left after the first block");

    let dying = FailsPast { inner: &source, fail_at: first_block_end };
    let slow = ScanOptions { chunk_size: 1, ..ScanOptions::default() };
    let err = map_file(&dying, &slow, &mode).await.expect_err("the source dies mid-scan");
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
    let run = map_file(&source, &ScanOptions::default(), &mode).await.unwrap();
    assert_eq!(run.resumed_from, first_block_end);
    assert_matches_eager(&run.index, &eager, "resumed from an interrupted scan");
}

/// A full scan recovers **every** database's DDL, not just the first's. A
/// query only ever captures the first (`scan_preamble`'s bounded prepass), and
/// that is all it may honestly claim while the map is short of EOF; `map_file`
/// reaches the other boundary `dump_metadata_from_spans` may be called at, so
/// `pgdq parse` leaves every database `preamble_complete`.
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
        std::fs::write(&dump, format!("{content}{}", content.replace("pgdq_fixture", "pgdq_2")))
            .unwrap();
        let source = LocalFileSource::open(&dump).unwrap();
        let mode = CacheMode::Enabled(cache::colocated_path(&dump));

        let index = map_file(&source, &ScanOptions::default(), &mode).await.unwrap().index;
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
    cancel: Arc<AtomicBool>,
}

impl ByteRangeSource for CancelsPast<'_> {
    async fn read_range(&self, offset: u64, len: usize) -> pgdump_query::Result<bytes::Bytes> {
        if offset >= self.trip {
            self.cancel.store(true, Ordering::SeqCst);
        }
        self.inner.read_range(offset, len).await
    }

    async fn size(&self) -> pgdump_query::Result<u64> {
        self.inner.size().await
    }

    async fn modified(&self) -> pgdump_query::Result<Option<std::time::SystemTime>> {
        self.inner.modified().await
    }
}

/// **The interrupt guard.** A cancelled scan is not an error and not a lie: it
/// reports `interrupted`, the index it returns stops at the last completed
/// block, and the cache on disk holds exactly that — whether or not the
/// throttle had skipped that block's own save, since every exit saves
/// unconditionally.
///
/// The flag trips one byte past the first block's end, with one-byte reads so
/// that offset is a read boundary: the block's `CopyEnd` has been processed,
/// nothing after it has.
#[tokio::test]
async fn a_cancelled_map_file_reports_it_and_banks_what_it_scanned() {
    let (_dir, dump) = sandboxed();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();
    let size = source.size().await.unwrap();
    let mode = CacheMode::Enabled(cache_path.clone());

    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
    let first_block_end = eager.blocks().next().expect("the fixture has blocks").end_offset;
    assert!(first_block_end < size, "sanity: there is a file left after the first block");

    let cancel = Arc::new(AtomicBool::new(false));
    let tripping =
        CancelsPast { inner: &source, trip: first_block_end, cancel: Arc::clone(&cancel) };
    let options =
        ScanOptions { chunk_size: 1, cancel: Some(Arc::clone(&cancel)), ..ScanOptions::default() };

    let run = map_file(&tripping, &options, &mode).await.unwrap();
    assert!(run.interrupted, "a cancelled scan says so");
    assert_eq!(run.index.scanned_through, first_block_end, "banked the block that completed");
    assert!(!run.index.is_complete(size), "and does not claim the whole file");

    let status = cache::load(&cache_path, &source).await.unwrap();
    let CacheStatus::Incomplete { index, .. } = status else {
        panic!("a cancelled scan must leave an incomplete cache, got {status:?}");
    };
    assert_eq!(index.scanned_through, first_block_end);
    assert_eq!(index.blocks().count(), 1);

    // And it is a resume point like any other.
    let resumed = map_file(&source, &ScanOptions::default(), &mode).await.unwrap();
    assert!(!resumed.interrupted);
    assert_eq!(resumed.resumed_from, first_block_end);
    assert_matches_eager(&resumed.index, &eager, "resumed from a cancelled scan");
}

/// A flag set before the scan starts stops it at the first thing it can stop
/// at, and still writes the cache — the degenerate case of the rule above, and
/// the one that would otherwise return an index claiming to describe a file it
/// never opened.
///
/// What it *does* claim is the preamble. `scan_preamble` ignores the flag
/// deliberately (`ScanOptions::cancel`): a stop inside it could not be told
/// from reaching the first `COPY` header, so a truncated preamble would be
/// cached as a complete one. It is an uncancellable region bounded by its own
/// length, it runs before `map_forward`'s first flag check, and it banks its
/// result — so even the most immediate interrupt leaves a cache whose columns
/// resolve rather than one that reports `not declared` for all of them.
#[tokio::test]
async fn a_scan_cancelled_before_it_starts_maps_only_the_preamble() {
    let (_dir, dump) = sandboxed();
    let source = LocalFileSource::open(&dump).unwrap();
    let mode = CacheMode::Enabled(cache::colocated_path(&dump));

    let options =
        ScanOptions { cancel: Some(Arc::new(AtomicBool::new(true))), ..ScanOptions::default() };
    let run = map_file(&source, &options, &mode).await.unwrap();

    assert!(run.interrupted);
    assert_eq!(run.resumed_from, 0, "the prepass is this run's work, not a resume point");
    assert!(run.index.blocks().next().is_none(), "no block was mapped");
    assert!(run.index.scanned_through > 0, "the preamble was read");
    let metadata = run.index.metadata.as_ref().expect("the preamble prepass banked its DDL");
    assert!(metadata.databases.first().unwrap().preamble_complete);

    // On disk, and resumable from where the prepass stopped.
    let reloaded = mode.load(&source).await.unwrap().expect("the prepass wrote a cache");
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
    let mode = CacheMode::Enabled(cache::colocated_path(&dump));

    let options =
        ScanOptions { cancel: Some(Arc::new(AtomicBool::new(true))), ..ScanOptions::default() };
    let mut stream =
        table_stream(&source, "public.widgets", options, QueryOptions::default(), None, mode);
    let err = stream.next().await.expect("the stream yields once").expect_err("cancelled");
    assert!(matches!(err, pgdump_query::Error::ScanCancelled { .. }), "{err:?}");
}

/// Two copies of `edge_cases/create.sql` concatenated, the second's database
/// renamed. A `pg_dumpall` and a bare `cat a.sql b.sql` are different shapes
/// (I9) and this is the only fixture for the second one, so it stays beside
/// the real dump rather than being replaced by it. The name is deliberately
/// not `pgdq_fixture_2`, which the synthetic multi-database helpers elsewhere
/// in the suite use: with both constructions in one file, a failure naming
/// `pgdq_fixture_2` would not say which it came from.
fn multidb(version: u32) -> (tempfile::TempDir, PathBuf) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(version.to_string())
        .join("edge_cases/create.sql");
    let content = std::fs::read_to_string(&fixture).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("multidb.sql");
    std::fs::write(&dump, format!("{content}{}", content.replace("pgdq_fixture", "pgdq_2")))
        .unwrap();
    (dir, dump)
}

/// Every banked block's columns, resolved the way `pgdq info` resolves them:
/// against the index's own metadata, with no census (a partial index may not
/// believe one — `DumpIndex::is_complete`).
fn outcomes(index: &DumpIndex) -> Vec<(String, Vec<ColumnResolution>)> {
    let metadata = index.metadata.as_ref();
    index
        .blocks()
        // A header with no column list takes placeholder names from its first
        // row, which no DDL can ever explain; those are `NotDeclared` by
        // construction and say nothing about the metadata.
        .filter(|b| !b.header.columns.is_empty())
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

/// **The defect this slice exists for.** An interrupted `parse` used to leave
/// a cache with no `DumpMetadata` at all, so `resolve_columns` answered
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
    let mode = CacheMode::Enabled(cache::colocated_path(&dump));

    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
    let trip = eager.blocks().nth(1).expect("the fixture has two blocks").end_offset;

    let cancel = Arc::new(AtomicBool::new(false));
    let tripping = CancelsPast { inner: &source, trip, cancel: Arc::clone(&cancel) };
    let options =
        ScanOptions { chunk_size: 1, cancel: Some(Arc::clone(&cancel)), ..ScanOptions::default() };
    let run = map_file(&tripping, &options, &mode).await.unwrap();

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
    let mode = CacheMode::Enabled(cache::colocated_path(dump));

    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
    let trip = eager
        .blocks()
        .find(|b| b.database.as_deref() == Some(second))
        .unwrap_or_else(|| panic!("{label}: {second} has blocks"))
        .end_offset;

    let cancel = Arc::new(AtomicBool::new(false));
    let tripping = CancelsPast { inner: &source, trip, cancel: Arc::clone(&cancel) };
    let options =
        ScanOptions { chunk_size: 1, cancel: Some(Arc::clone(&cancel)), ..ScanOptions::default() };
    let run = map_file(&tripping, &options, &mode).await.unwrap();
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
    let resumed = map_file(&source, &ScanOptions::default(), &mode).await.unwrap();
    assert_matches_eager(&resumed.index, &eager, &format!("{label}: resumed"));
}

/// The boundary against a file `pg_dump` actually wrote. `edge_cases/dumpall`
/// carries `COPY` blocks in two consecutive segments — `pgdq_fixture` and
/// `pgdq_tenant`, in that order by I30 — and `pgdq_tenant.public.widgets`
/// repeats the earlier database's table name with a different type on every
/// column, so an index that resolved these blocks against `pgdq_fixture`'s DDL
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
        assert_an_interrupt_inside(&dump, "pgdq_tenant", &format!("pg_dump {version} dumpall"))
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
        let index =
            map_file(&source, &ScanOptions::default(), &CacheMode::Disabled).await.unwrap().index;

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
            types_of("pgdq_fixture"),
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
            types_of("pgdq_tenant"),
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
        assert_an_interrupt_inside(&dump, "pgdq_2", &format!("pg_dump {version} concatenated"))
            .await;
    }
}
