//! On-disk structure cache: round-tripping, the "unusable cache is treated
//! as absent" contract `docs/design/architecture.md` requires, and the
//! source-identity check `docs/design/architecture.md`
//! ("Cache: the dump file's identity is checked, not assumed") adds on top.

use std::path::{Path, PathBuf};
use std::process::Command;

use pgdump_query::cache::{CacheMode, CacheStatus};
use pgdump_query::map::SpanBody;
use pgdump_query::{
    ByteRangeSource, Error, LocalFileSource, ScanOptions, XzSource, build_index, cache,
    check_tiling, preamble_only,
};

mod common;
use common::edge_cases;

#[test]
fn colocated_path_appends_the_cache_suffix() {
    assert_eq!(
        cache::colocated_path(Path::new("/a/b/dump.sql")),
        PathBuf::from("/a/b/dump.sql.dqcache")
    );
}

/// The four unusable outcomes are told apart, not collapsed: each is a
/// different sentence `pgdq info` has to print, even though every one of them
/// ends in `pgdq parse` (`docs/design/architecture.md`, "The cache").
/// `SourceChanged` has its own test below, since producing it needs a second
/// file.
#[tokio::test]
async fn an_unusable_cache_says_which_kind_of_unusable_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let source = LocalFileSource::open(edge_cases()).unwrap();

    let missing = dir.path().join("nonexistent.dqcache");
    assert_eq!(cache::load(&missing, &source).await.unwrap(), CacheStatus::Missing);

    let foreign = dir.path().join("garbage.dqcache");
    std::fs::write(&foreign, b"not a cache file").unwrap();
    assert_eq!(cache::load(&foreign, &source).await.unwrap(), CacheStatus::Unreadable);
}

/// A cache whose envelope decodes but whose `format_version` is not this
/// build's is `UnsupportedVersion`, not `Unreadable` — different advice: the
/// path is right, the build that wrote it was not. Produced by corrupting the
/// version field in place, which is the first thing the envelope encodes.
#[tokio::test]
async fn a_cache_from_another_build_is_told_apart_from_foreign_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    let path = dir.path().join("edge_cases.sql.dqcache");
    cache::save(&path, &source, &index).await.unwrap();

    let mut bytes = std::fs::read(&path).unwrap();
    // bincode's standard config writes a `u32` as a varint; a small value is
    // one byte, so bumping it keeps the rest of the envelope decodable.
    bytes[0] = bytes[0].wrapping_add(1);
    std::fs::write(&path, &bytes).unwrap();

    assert_eq!(cache::load(&path, &source).await.unwrap(), CacheStatus::UnsupportedVersion);
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
        CacheStatus::Valid { index, mtime_changed, total_size } => {
            assert!(!mtime_changed, "just-saved cache must match the source's current mtime");
            assert_eq!(total_size, source.size().await.unwrap());
            index
        }
        CacheStatus::Incomplete { .. } => panic!("build_index always scans the whole file"),
        unusable => panic!("a freshly saved cache must load, got {unusable:?}"),
    };

    // Diagnostics are the one field that does not travel through the file —
    // they are recomputed on load from the spans it just read, and
    // `build_index` derives its own from the same spans by the same pure
    // function, so the two agree without anything being persisted
    // (`diagnostics_do_not_round_trip_through_the_cache` pins that directly).
    assert_eq!(loaded, index);
}

/// `preamble_only` is a genuinely partial scan — unlike `build_index`, which
/// always reaches EOF, it stops at the first `COPY` header — so it's the one
/// place today that persists a real `SpanBody::Unscanned` tail, rather than
/// the variant only ever appearing in `crate::map`'s own unit tests
/// (`docs/design/architecture.md`, "The file map").
#[tokio::test]
async fn preamble_only_persists_a_real_unscanned_tail() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge_cases.sql.dqcache");
    let mode = CacheMode::Enabled(path.clone());

    preamble_only(&source, &ScanOptions::default(), &mode).await.unwrap();

    // A preamble-only scan never reaches EOF (that's the point of it), so
    // the cache it persists is `Incomplete` by the same completeness check
    // `pgdq info`'s default listing uses to decide whether to trust a cache
    // as the whole file's map — not `Valid`, and not `Absent` either, since
    // it's a real, usable partial scan (`docs/design/architecture.md`,
    // "The cache").
    let index = match cache::load(&path, &source).await.unwrap() {
        CacheStatus::Incomplete { index, .. } => index,
        CacheStatus::Valid { .. } => panic!("a preamble-only scan cannot reach EOF"),
        unusable => panic!("preamble_only must persist a cache, got {unusable:?}"),
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

    let total_size = source.size().await.unwrap();
    assert_eq!(
        cache::load(&path, &source).await.unwrap(),
        CacheStatus::Valid { index, mtime_changed: false, total_size }
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

/// A cache saved against a differently-sized file is invalidated outright —
/// every offset it holds could be wrong. It reports **which** sizes
/// disagreed, because "your file changed since you parsed it" is the sentence
/// a reporting caller has to print and those two numbers are its evidence.
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

    assert_eq!(
        cache::load(&path, &grown_source).await.unwrap(),
        CacheStatus::SourceChanged {
            cached_stored_size: source.stored_size().await.unwrap(),
            live_stored_size: grown_source.stored_size().await.unwrap(),
        }
    );
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
        CacheStatus::Valid { index: loaded, mtime_changed, .. } => {
            assert!(mtime_changed);
            // Diagnostics are recomputed on load rather than restored, and
            // `build_index`'s own are the same pure function of the same
            // spans, so the two agree without either being persisted.
            assert_eq!(loaded, index);
        }
        CacheStatus::Incomplete { .. } => panic!("build_index always scans the whole file"),
        unusable => {
            panic!("an mtime-only mismatch must not invalidate the cache, got {unusable:?}")
        }
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
/// (`docs/design/architecture.md`, "The cache"): mtime granularity and preservation
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
    assert!(
        loaded.diagnostics.contains(&pgdump_query::Diagnostic {
            severity: Severity::Warning,
            kind: DiagnosticKind::CacheMtimeChanged,
        }),
        "diagnostics: {:?}",
        loaded.diagnostics
    );
}

/// Diagnostics are never persisted: a stored one would replay a warning
/// about a check *this* run performed successfully.
#[tokio::test]
async fn diagnostics_do_not_round_trip_through_the_cache() {
    use pgdump_query::{Diagnostic, DiagnosticKind, Severity};

    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("edge_cases.sql");
    std::fs::copy(edge_cases(), &dump).unwrap();
    let cache_path = pgdump_query::cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();

    let mut index = build_index(&source, &ScanOptions::default()).await.unwrap();
    // `build_index` always reports a `TocCoverage` figure — cleared here so
    // this test's own pushed diagnostic is the only one in play.
    index.diagnostics.clear();
    index
        .diagnostics
        .push(Diagnostic { severity: Severity::Warning, kind: DiagnosticKind::CacheMtimeChanged });
    pgdump_query::cache::save(&cache_path, &source, &index).await.unwrap();

    let loaded = CacheMode::Enabled(cache_path).load(&source).await.unwrap().unwrap();
    // The saved `CacheMtimeChanged` is gone: the mismatch was between the
    // cache and *that* run's observation, and this run's mtime check passed.
    // What is present is recomputed, not restored — the TOC-coverage figure
    // the load derives from the spans it just read.
    assert!(
        !loaded.diagnostics.iter().any(|d| d.kind == DiagnosticKind::CacheMtimeChanged),
        "a persisted diagnostic must not be restored: {:?}",
        loaded.diagnostics
    );
    assert!(
        loaded.diagnostics.iter().any(|d| matches!(d.kind, DiagnosticKind::TocCoverage { .. })),
        "the file-level figures are recomputed on load: {:?}",
        loaded.diagnostics
    );
}

/// A cache whose `scanned_through` falls short of its recorded size loads as
/// `Incomplete`, not `Valid` — the case `preamble_only_persists_a_real_unscanned_tail`
/// already exercises through `preamble_only` — but `CacheMode::load` still
/// hands it back as `Some`, the same as a `Valid` cache, since its callers
/// (`table_stream`, `preamble_only`) want a partial map to build forward
/// from rather than a signal to start over
/// (`docs/design/architecture.md`, "The cache").
#[tokio::test]
async fn an_incomplete_cache_still_loads_as_some_through_cache_mode() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge_cases.sql.dqcache");
    let mode = CacheMode::Enabled(path.clone());

    preamble_only(&source, &ScanOptions::default(), &mode).await.unwrap();

    let index = mode.load(&source).await.unwrap().expect("Incomplete still yields Some");
    let size = source.size().await.unwrap();
    assert!(index.scanned_through < size, "a preamble-only cache never reaches EOF");
}

/// `docs/design/architecture.md`, "The cache": `CacheMode::Offline` is rejected by every method that
/// requires a live source, and `CacheMode::load_offline` rejects the other
/// two variants the opposite way.
#[tokio::test]
async fn offline_mode_is_rejected_by_live_methods_and_vice_versa() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge_cases.sql.dqcache");

    let offline = CacheMode::Offline(path.clone());
    assert!(matches!(offline.load(&source).await, Err(Error::CacheModeMismatch(_))));
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    assert!(matches!(offline.save(&source, &index).await, Err(Error::CacheModeMismatch(_))));
    assert!(matches!(offline.require_enabled("parse"), Err(Error::CacheModeMismatch(_))));

    assert!(matches!(
        CacheMode::Enabled(path).load_offline().await,
        Err(Error::CacheModeMismatch(_))
    ));
    assert!(matches!(CacheMode::Disabled.load_offline().await, Err(Error::CacheModeMismatch(_))));
}

/// `load_offline` against a cache that was never fully scanned reports
/// `Incomplete` with the total size it fell short of, the same way `load`
/// does for a live source — the completeness check reads the cache's own
/// recorded size, since there is no live file to stat
/// (`docs/design/architecture.md`, "The cache").
#[tokio::test]
async fn load_offline_reports_incomplete_for_a_partial_scan() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge_cases.sql.dqcache");
    let mode = CacheMode::Enabled(path.clone());
    preamble_only(&source, &ScanOptions::default(), &mode).await.unwrap();

    let size = source.size().await.unwrap();
    match CacheMode::Offline(path).load_offline().await.unwrap() {
        CacheStatus::Incomplete { index, total_size, .. } => {
            assert_eq!(total_size, size);
            assert!(index.scanned_through < total_size);
        }
        other => panic!("expected Incomplete, got {other:?}"),
    }
}

/// A cache-only load — `Valid` or `Incomplete` — always carries a
/// `CacheOffline` diagnostic: there is no live file to check it against, so
/// the result is unverified and historical regardless of completeness
/// (`docs/design/architecture.md`, "The cache").
#[tokio::test]
async fn load_offline_always_pushes_the_cache_offline_diagnostic() {
    use pgdump_query::DiagnosticKind;

    let source = LocalFileSource::open(edge_cases()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let full_path = dir.path().join("full.dqcache");
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    pgdump_query::cache::save(&full_path, &source, &index).await.unwrap();

    let status = CacheMode::Offline(full_path).load_offline().await.unwrap();
    let CacheStatus::Valid { index: loaded, .. } = status else {
        panic!("expected Valid for a fully scanned cache");
    };
    assert!(
        loaded.diagnostics.iter().any(|d| d.kind == DiagnosticKind::CacheOffline),
        "diagnostics: {:?}",
        loaded.diagnostics
    );

    let partial_path = dir.path().join("partial.dqcache");
    let mode = CacheMode::Enabled(partial_path.clone());
    preamble_only(&source, &ScanOptions::default(), &mode).await.unwrap();
    let status = CacheMode::Offline(partial_path).load_offline().await.unwrap();
    let CacheStatus::Incomplete { index: loaded, .. } = status else {
        panic!("expected Incomplete for a preamble-only cache");
    };
    assert!(
        loaded.diagnostics.iter().any(|d| d.kind == DiagnosticKind::CacheOffline),
        "diagnostics: {:?}",
        loaded.diagnostics
    );
}

/// A cache path that doesn't exist is `Missing` offline too, and gets no
/// diagnostic pushed onto it — there is no index to push one onto.
#[tokio::test]
async fn load_offline_missing_file_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nonexistent.dqcache");
    assert_eq!(CacheMode::Offline(path).load_offline().await.unwrap(), CacheStatus::Missing);
}

/// Compress `path` with `xz`, forcing several blocks so this exercises the
/// seekable shape, into a temp file this test owns. `xz` is not
/// `mise`-pinned (`docs/design/roadmap-P13-compressed-input.md`,
/// "Fixtures"), so a missing binary fails loudly rather than skipping
/// (`docs/design/roadmap.md`, "A test may assume the tools `mise` pins").
fn xz_compress(path: &Path) -> tempfile::NamedTempFile {
    let out = Command::new("xz")
        .arg("--block-size=65536")
        .arg("-c")
        .arg(path)
        .output()
        .expect("`xz` is not runnable, so this test cannot build its fixture; install it.");
    assert!(out.status.success(), "xz failed: {}", String::from_utf8_lossy(&out.stderr));
    let mut compressed = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut compressed, &out.stdout).unwrap();
    compressed
}

/// `XzSource` is just another `ByteRangeSource` to everything above `io.rs`:
/// `build_index`'s scan and `cache::save`/`load`'s round trip produce the
/// same `DumpIndex` whether the bytes came straight off disk or through the
/// decoder (`docs/design/roadmap-P13-compressed-input.md`, "D5"/"D6") — the
/// differential parity this slice owes at the library level. The CLI-level
/// parity against the phase's own two committed fixtures is 13.5's.
#[tokio::test]
async fn xz_source_produces_the_same_index_and_cache_as_the_plain_file() {
    let plain = LocalFileSource::open(edge_cases()).unwrap();
    let plain_index = build_index(&plain, &ScanOptions::default()).await.unwrap();

    let compressed = xz_compress(&edge_cases());
    let xz = XzSource::open(compressed.path()).unwrap();
    assert_eq!(xz.size().await.unwrap(), plain.size().await.unwrap());
    assert_ne!(
        xz.stored_size().await.unwrap(),
        plain.stored_size().await.unwrap(),
        "the compressed file's on-disk size must not be reported as the plain one's"
    );

    let xz_index = build_index(&xz, &ScanOptions::default()).await.unwrap();
    assert_eq!(xz_index, plain_index);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge_cases.sql.xz.dqcache");
    cache::save(&path, &xz, &xz_index).await.unwrap();
    match cache::load(&path, &xz).await.unwrap() {
        CacheStatus::Valid { index, mtime_changed, total_size } => {
            assert!(!mtime_changed, "just-saved cache must match the source's current mtime");
            assert_eq!(total_size, plain.size().await.unwrap());
            assert_eq!(index, plain_index);
        }
        other => panic!("a freshly saved cache must load, got {other:?}"),
    }
}
