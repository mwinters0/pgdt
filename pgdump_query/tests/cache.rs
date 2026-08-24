//! On-disk structure cache: round-tripping, the "unusable cache is treated
//! as absent" contract `docs/design/roadmap-phase1-mvp.md` requires, and the
//! source-identity check `docs/design/roadmap-phase3-object-inventory.md`
//! ("Cache: the dump file's identity is checked, not assumed") adds on top.

use std::path::{Path, PathBuf};

use pgdump_query::cache::{CacheMode, CacheStatus};
use pgdump_query::map::SpanBody;
use pgdump_query::{
    ByteRangeSource, Error, LocalFileSource, ScanOptions, build_index, cache, check_tiling,
    preamble_only,
};

fn edge_cases() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/edge_cases.sql")
}

#[test]
fn colocated_path_appends_the_cache_suffix() {
    assert_eq!(
        cache::colocated_path(Path::new("/a/b/dump.sql")),
        PathBuf::from("/a/b/dump.sql.dqcache")
    );
}

#[tokio::test]
async fn missing_cache_file_is_treated_as_absent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nonexistent.dqcache");
    let source = LocalFileSource::open(edge_cases()).unwrap();
    assert_eq!(cache::load(&path, &source).await.unwrap(), CacheStatus::Absent);
}

#[tokio::test]
async fn foreign_bytes_at_the_cache_path_are_treated_as_absent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("garbage.dqcache");
    std::fs::write(&path, b"not a cache file").unwrap();
    let source = LocalFileSource::open(edge_cases()).unwrap();
    assert_eq!(cache::load(&path, &source).await.unwrap(), CacheStatus::Absent);
}

/// A saved index round-trips exactly, including the still-reserved-and-
/// unpopulated fields (`CopyBlock::sparse_index`/`column_stats`) — they must
/// serialize as `None` rather than being silently dropped, which is the
/// whole point of reserving them ahead of population. `DumpIndex::metadata`
/// is no longer one of these: every `build_index` scan populates it (Phase
/// 2.2), so this also pins that a `Some(DumpMetadata { .. })` round-trips.
#[tokio::test]
async fn saved_index_round_trips_exactly() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    assert!(index.blocks().next().is_some());
    assert!(index.blocks().all(|b| b.sparse_index.is_none() && b.column_stats.is_none()));
    assert!(index.metadata.is_some());

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge_cases.sql.dqcache");
    cache::save(&path, &source, &index).await.unwrap();
    let loaded = match cache::load(&path, &source).await.unwrap() {
        CacheStatus::Valid { index, mtime_changed } => {
            assert!(!mtime_changed, "just-saved cache must match the source's current mtime");
            index
        }
        CacheStatus::Absent => panic!("a freshly saved cache must load"),
    };

    assert_eq!(loaded, index);
}

/// `preamble_only` is a genuinely partial scan — unlike `build_index`, which
/// always reaches EOF, it stops at the first `COPY` header — so it's the one
/// place today that persists a real `SpanBody::Unscanned` tail, rather than
/// the variant only ever appearing in `crate::map`'s own unit tests
/// (`docs/design/roadmap-phase3-object-inventory.md`, "Scan coverage is a
/// prefix, expressed as a span").
#[tokio::test]
async fn preamble_only_persists_a_real_unscanned_tail() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge_cases.sql.dqcache");
    let mode = CacheMode::Enabled(path.clone());

    preamble_only(&source, &ScanOptions::default(), &mode).await.unwrap();

    let index = match cache::load(&path, &source).await.unwrap() {
        CacheStatus::Valid { index, .. } => index,
        CacheStatus::Absent => panic!("preamble_only must persist a cache"),
    };
    let size = source.size().await.unwrap();
    assert!(index.scanned_through < size, "edge_cases.sql has COPY blocks past its preamble");

    let last = index.spans.last().expect("a partial scan still produces spans");
    assert_eq!(last.body, SpanBody::Unscanned);
    assert_eq!(last.start, index.scanned_through);
    assert_eq!(last.end, size);
    assert!(check_tiling(&index.spans, size).is_empty(), "spans: {:?}", index.spans);
}

#[tokio::test]
async fn save_overwrites_an_existing_cache() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge_cases.sql.dqcache");
    std::fs::write(&path, b"stale placeholder").unwrap();
    cache::save(&path, &source, &index).await.unwrap();

    assert_eq!(
        cache::load(&path, &source).await.unwrap(),
        CacheStatus::Valid { index, mtime_changed: false }
    );
}

/// A write failure (here: parent directory doesn't exist) must propagate as
/// a hard error, never a silent fallback to running without a cache.
#[tokio::test]
async fn save_propagates_write_failures() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("no-such-subdir").join("edge_cases.sql.dqcache");
    assert!(matches!(cache::save(&path, &source, &index).await, Err(Error::Io(_))));
}

/// A cache saved against a differently-sized file is invalidated outright
/// (`Absent`) — every offset it holds could be wrong.
#[tokio::test]
async fn size_mismatch_invalidates_the_cache() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge_cases.sql.dqcache");
    cache::save(&path, &source, &index).await.unwrap();

    let dump_bytes = std::fs::read(edge_cases()).unwrap();
    let grown = dir.path().join("grown.sql");
    let mut grown_bytes = dump_bytes.clone();
    grown_bytes.push(b'\n');
    std::fs::write(&grown, &grown_bytes).unwrap();
    let grown_source = LocalFileSource::open(&grown).unwrap();
    assert_ne!(grown_source.size().await.unwrap(), source.size().await.unwrap());

    assert_eq!(cache::load(&path, &grown_source).await.unwrap(), CacheStatus::Absent);
}

/// An mtime mismatch alone is a loud warning, not grounds for invalidation
/// — the cache still loads as `Valid`. Simulated by touching the dump file's
/// mtime forward after the cache was saved, without changing its size.
#[tokio::test]
async fn mtime_mismatch_alone_does_not_invalidate_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("edge_cases.sql");
    std::fs::copy(edge_cases(), &dump).unwrap();
    let source = LocalFileSource::open(&dump).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();

    let path = cache::colocated_path(&dump);
    cache::save(&path, &source, &index).await.unwrap();

    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    std::fs::File::options().write(true).open(&dump).unwrap().set_modified(future).unwrap();

    match cache::load(&path, &source).await.unwrap() {
        CacheStatus::Valid { index: loaded, mtime_changed } => {
            assert!(mtime_changed);
            assert_eq!(loaded, index);
        }
        CacheStatus::Absent => panic!("an mtime-only mismatch must not invalidate the cache"),
    }
}

#[test]
fn cache_mode_resolves_default_explicit_and_disabled() {
    let dump = Path::new("/a/b/dump.sql");

    assert_eq!(
        CacheMode::resolve(dump, None),
        CacheMode::Enabled(PathBuf::from("/a/b/dump.sql.dqcache"))
    );
    assert_eq!(
        CacheMode::resolve(dump, Some(Path::new("/other/path.dqcache"))),
        CacheMode::Enabled(PathBuf::from("/other/path.dqcache"))
    );
    assert_eq!(CacheMode::resolve(dump, Some(Path::new("none"))), CacheMode::Disabled);
}

#[tokio::test]
async fn disabled_cache_ignores_an_existing_file_and_persists_nothing() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("edge_cases.sql");
    let path = cache::colocated_path(&dump);
    cache::save(&path, &source, &index).await.unwrap();
    assert!(path.exists());

    let mode = CacheMode::resolve(&dump, Some(Path::new("none")));
    assert_eq!(mode, CacheMode::Disabled);

    // The existing valid cache at the colocated path is ignored, not read.
    assert!(mode.load(&source).await.unwrap().is_none());

    // Saving under a disabled mode is a no-op: it must not touch whatever is
    // (or isn't) at the would-be colocated path.
    std::fs::write(&path, b"clobbered after the disabled mode was resolved").unwrap();
    mode.save(&source, &index).await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"clobbered after the disabled mode was resolved");
}

#[test]
fn require_enabled_errors_when_disabled() {
    assert!(CacheMode::Enabled(PathBuf::from("/x")).require_enabled("parse").is_ok());
    assert!(matches!(
        CacheMode::Disabled.require_enabled("parse"),
        Err(Error::CacheDisabled { operation: "parse" })
    ));
}

/// An mtime that changed since the cache was saved is a **warning on the
/// loaded index**, not an invalidation and not an error
/// (`docs/design/roadmap-phase3-object-inventory.md`, "Cache: the dump file's
/// identity is checked, not assumed"): mtime granularity and preservation
/// vary too much across filesystems, copies and restores to be conclusive.
/// The cache's contents come back intact.
#[tokio::test]
async fn a_changed_mtime_is_a_diagnostic_not_an_invalidation() {
    use pgdump_query::{DiagnosticKind, Severity};

    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("edge_cases.sql");
    std::fs::copy(edge_cases(), &dump).unwrap();
    let cache_path = pgdump_query::cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();

    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    pgdump_query::cache::save(&cache_path, &source, &index).await.unwrap();

    // Same bytes, new mtime — exactly the ambiguous case the design refuses
    // to treat as conclusive.
    let bytes = std::fs::read(&dump).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&dump, &bytes).unwrap();

    let loaded = CacheMode::Enabled(cache_path)
        .load(&source)
        .await
        .unwrap()
        .expect("an mtime change does not invalidate");
    assert_eq!(loaded.blocks().count(), index.blocks().count(), "contents survive intact");
    assert_eq!(
        loaded.diagnostics,
        vec![pgdump_query::FileDiagnostic {
            severity: Severity::Warning,
            kind: DiagnosticKind::CacheMtimeChanged,
        }]
    );
}

/// Diagnostics are never persisted: a stored one would replay a warning
/// about a check *this* run performed successfully.
#[tokio::test]
async fn diagnostics_do_not_round_trip_through_the_cache() {
    use pgdump_query::{DiagnosticKind, FileDiagnostic, Severity};

    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("edge_cases.sql");
    std::fs::copy(edge_cases(), &dump).unwrap();
    let cache_path = pgdump_query::cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();

    let mut index = build_index(&source, &ScanOptions::default()).await.unwrap();
    assert!(index.diagnostics.is_empty(), "a real fixture tiles, so nothing is reported");
    index.diagnostics.push(FileDiagnostic {
        severity: Severity::Warning,
        kind: DiagnosticKind::CacheMtimeChanged,
    });
    pgdump_query::cache::save(&cache_path, &source, &index).await.unwrap();

    let loaded = CacheMode::Enabled(cache_path).load(&source).await.unwrap().unwrap();
    assert!(loaded.diagnostics.is_empty(), "recomputed on load, never restored");
}
