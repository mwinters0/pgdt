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

use futures::StreamExt;
use pgdump_query::cache::{CacheMode, CacheStatus};
use pgdump_query::{
    BatchOptions, ByteRangeSource, DumpIndex, LocalFileSource, ScanOptions, build_index, cache,
    map_file, preamble_only, table_stream,
};

/// A private copy of `tests/data/edge_cases.sql` in a fresh tempdir, so each
/// test may freely write the colocated `.dqcache` beside it — same fixture and
/// same convention as `tests/query_cache.rs`, which is what makes the "a query
/// stops at its target" setup below behave identically here.
fn sandboxed() -> (tempfile::TempDir, PathBuf) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/edge_cases.sql");
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("edge_cases.sql");
    std::fs::copy(&fixture, &dump).unwrap();
    (dir, dump)
}

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

            let (index, resumed_from) =
                map_file(&source, &ScanOptions::default(), &mode).await.unwrap();
            assert_eq!(resumed_from, 0, "{schema_dir}/{flag_set}: nothing to resume from");
            let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
            assert_matches_eager(&index, &eager, &format!("{schema_dir}/{flag_set}"));
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
        BatchOptions::default(),
        None,
        None,
        mode.clone(),
    );
    while stream.next().await.is_some() {}
    let partial = mode.load(&source).await.unwrap().expect("the query wrote a cache");
    assert!(partial.scanned_through < size, "sanity: the query really did stop short");

    let (finished, resumed_from) = map_file(&source, &ScanOptions::default(), &mode).await.unwrap();
    assert_eq!(resumed_from, partial.scanned_through, "resumed at the cache's own frontier");
    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
    assert!(
        partial.blocks().count() < eager.blocks().count(),
        "sanity: the query left blocks unmapped for this to find"
    );
    assert_matches_eager(&finished, &eager, "resumed from a query's partial cache");

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

    let (finished, resumed_from) = map_file(&source, &ScanOptions::default(), &mode).await.unwrap();
    assert_eq!(resumed_from, preamble_cache.scanned_through);
    let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
    assert_matches_eager(&finished, &eager, "resumed from a preamble-only cache");
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

    let (first, _) = map_file(&source, &ScanOptions::default(), &mode).await.unwrap();
    let (second, resumed_from) = map_file(&source, &ScanOptions::default(), &mode).await.unwrap();

    assert_eq!(resumed_from, size, "the cache already covered the file");
    assert_eq!(first, second);
    assert!(
        !second.diagnostics.is_empty(),
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
    let (finished, resumed_from) = map_file(&source, &ScanOptions::default(), &mode).await.unwrap();
    assert_eq!(resumed_from, first_block_end);
    assert_matches_eager(&finished, &eager, "resumed from an interrupted scan");
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

        let (index, _) = map_file(&source, &ScanOptions::default(), &mode).await.unwrap();
        let metadata = index.metadata.as_ref().expect("a full scan always has metadata");
        assert_eq!(metadata.databases.len(), 2, "pg_dump {version}");
        for db in &metadata.databases {
            assert!(db.preamble_complete, "pg_dump {version}: {:?}", db.name);
            assert!(!db.tables.is_empty(), "pg_dump {version}: {:?} has DDL", db.name);
        }
    }
}
