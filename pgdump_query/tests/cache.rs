//! On-disk structure cache: round-tripping, the "unusable cache is treated
//! as absent" contract (`docs/design/decisions.md`, "D20") requires, and the
//! source-identity check (`docs/design/decisions.md`, "D21") adds on top.

use std::path::{Path, PathBuf};
use std::process::Command;

use futures::StreamExt;
use pgdump_query::cache::{CacheClaim, CacheLoad, CacheMode, CacheStatus};
use pgdump_query::map::SpanBody;
use pgdump_query::{
    ByteRangeSource, DiagnosticKind, Error, KnownCompression, LocalFileSource, QueryOptions,
    Recognized, ScanOptions, XzSource, build_index, cache, check_tiling, map_file, open_local,
    preamble_only, table_stream,
};

mod common;
use common::{edge_cases, sandboxed_edge_cases as sandboxed};

#[test]
fn colocated_path_appends_the_cache_suffix() {
    assert_eq!(
        cache::colocated_path(Path::new("/a/b/dump.sql")),
        PathBuf::from("/a/b/dump.sql.dqcache")
    );
}

/// The four unusable outcomes are told apart, not collapsed: each is a
/// different sentence `pgdq info` has to print, even though every one of them
/// ends in `pgdq parse` (`docs/design/decisions.md`, "D22").
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
/// is populated by every `build_index` scan, so this also pins that a
/// `Some(DumpMetadata { .. })` round-trips.
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
        CacheStatus::Valid { index, mtime_changed, total_size, compression } => {
            assert!(!mtime_changed, "just-saved cache must match the source's current mtime");
            assert_eq!(total_size, source.size().await.unwrap());
            assert_eq!(compression, None, "a plain source sits under no container");
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
/// always reaches EOF, it stops at the first `COPY` header — so it's the
/// place that persists a real `SpanBody::Unscanned` tail, rather than
/// the variant only ever appearing in `crate::map`'s own unit tests
/// (`docs/design/decisions.md`, "D30").
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
    // it's a real, usable partial scan (`docs/design/decisions.md`,
    // "D22").
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
        CacheStatus::Valid { index, mtime_changed: false, total_size, compression: None }
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

/// `CacheMode::load` carries all four unusable statuses across as their own
/// [`CacheLoad`] variants rather than answering "no index" for each
/// (`docs/design/decisions.md`, "D22"). It is the same four
/// `an_unusable_cache_says_which_kind_of_unusable_it_is` and
/// `size_mismatch_invalidates_the_cache` put to `cache::load`; what this pins
/// is that the reason survives the trip to a caller that holds a live source
/// and could scan.
#[tokio::test]
async fn cache_mode_load_names_each_unusable_status() {
    let dir = tempfile::tempdir().unwrap();
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();

    let missing = dir.path().join("nonexistent.dqcache");
    assert_eq!(CacheMode::Enabled(missing).load(&source).await.unwrap(), CacheLoad::Missing);

    let foreign = dir.path().join("garbage.dqcache");
    std::fs::write(&foreign, b"not a cache file").unwrap();
    assert_eq!(CacheMode::Enabled(foreign).load(&source).await.unwrap(), CacheLoad::Unreadable);

    let stale = dir.path().join("stale.dqcache");
    cache::save(&stale, &source, &index).await.unwrap();
    let mut bytes = std::fs::read(&stale).unwrap();
    bytes[0] = bytes[0].wrapping_add(1);
    std::fs::write(&stale, &bytes).unwrap();
    assert_eq!(
        CacheMode::Enabled(stale).load(&source).await.unwrap(),
        CacheLoad::UnsupportedVersion
    );

    let path = dir.path().join("edge_cases.sql.dqcache");
    cache::save(&path, &source, &index).await.unwrap();
    let grown = dir.path().join("grown.sql");
    let mut grown_bytes = std::fs::read(edge_cases()).unwrap();
    grown_bytes.push(b'\n');
    std::fs::write(&grown, &grown_bytes).unwrap();
    let grown_source = LocalFileSource::open(&grown).unwrap();
    assert_eq!(
        CacheMode::Enabled(path).load(&grown_source).await.unwrap(),
        CacheLoad::SourceChanged {
            cached_stored_size: source.stored_size().await.unwrap(),
            live_stored_size: grown_source.stored_size().await.unwrap(),
        },
        "the two sizes are the evidence a caller states the mismatch with"
    );
}

/// **The library never replaces cache data automatically.** All three scan
/// entry points refuse a cache that records another file's stored size —
/// naming the path, what the cache expected and what the source is — rather
/// than starting cold and overwriting it at their first save
/// (`docs/design/decisions.md`, "D20"). The other three unusable
/// statuses still start cold; this is the one that is an error.
///
/// The cache is read back byte for byte afterwards, which is the half that
/// would fail silently: a refusal that still wrote is indistinguishable from
/// one that did not until the file is compared.
#[tokio::test]
async fn a_scan_refuses_a_cache_that_records_another_source_and_leaves_it_alone() {
    let (_dir, dump) = sandboxed();
    let source = LocalFileSource::open(&dump).unwrap();
    let cached_stored_size = source.stored_size().await.unwrap();
    let path = cache::colocated_path(&dump);
    let mode = CacheMode::Enabled(path.clone());
    map_file(&source, &ScanOptions::default(), &mode).await.unwrap();
    let before = std::fs::read(&path).unwrap();

    // Grow the dump under its own cache: the file at `path` is now a valid
    // cache for a file that no longer exists, which is what pointing
    // `--dqcache` at the wrong path produces too.
    let mut grown = std::fs::read(&dump).unwrap();
    grown.push(b'\n');
    std::fs::write(&dump, &grown).unwrap();
    let source = LocalFileSource::open(&dump).unwrap();
    let live_stored_size = source.stored_size().await.unwrap();
    assert_ne!(live_stored_size, cached_stored_size);

    let expected = |what: &str, e: Error| match e {
        Error::CacheSourceMismatch {
            path: named,
            cached_stored_size: cached,
            live_stored_size: live,
        } => {
            assert_eq!(named, path, "{what}: the refusal names the cache it refused");
            assert_eq!(cached, cached_stored_size, "{what}: what the cache was written for");
            assert_eq!(live, live_stored_size, "{what}: what this source is");
        }
        other => panic!("{what}: expected a source mismatch, got {other:?}"),
    };

    expected("map_file", map_file(&source, &ScanOptions::default(), &mode).await.unwrap_err());
    expected(
        "preamble_only",
        preamble_only(&source, &ScanOptions::default(), &mode).await.unwrap_err(),
    );
    let mut stream = table_stream(
        &source,
        "public.widgets",
        ScanOptions::default(),
        QueryOptions::default(),
        None,
        mode.clone(),
    );
    let first = stream.next().await.expect("the stream yields the refusal, not nothing");
    expected("table_stream", first.unwrap_err());

    assert_eq!(std::fs::read(&path).unwrap(), before, "the refused cache is untouched");
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

    // The existing valid cache at the colocated path is ignored, not read —
    // and the reason says so: `Disabled` is about the caller, not about
    // anything found at a path (`docs/design/decisions.md`, "D22").
    assert_eq!(mode.load(&source).await.unwrap(), CacheLoad::Disabled);

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
/// (`docs/design/decisions.md`, "D21"): mtime granularity and preservation
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

    let CacheLoad::Index(loaded) = CacheMode::Enabled(cache_path).load(&source).await.unwrap()
    else {
        panic!("an mtime change does not invalidate")
    };
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

    let CacheLoad::Index(loaded) = CacheMode::Enabled(cache_path).load(&source).await.unwrap()
    else {
        panic!("the cache this test just saved is usable")
    };
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
/// hands it back as `CacheLoad::Index`, the same as a `Valid` cache, since its
/// callers (`table_stream`, `preamble_only`) want a partial map to build
/// forward from rather than a signal to start over
/// (`docs/design/decisions.md`, "D22").
#[tokio::test]
async fn an_incomplete_cache_still_loads_as_an_index_through_cache_mode() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge_cases.sql.dqcache");
    let mode = CacheMode::Enabled(path.clone());

    preamble_only(&source, &ScanOptions::default(), &mode).await.unwrap();

    let CacheLoad::Index(index) = mode.load(&source).await.unwrap() else {
        panic!("Incomplete still yields an index")
    };
    let size = source.size().await.unwrap();
    assert!(index.scanned_through < size, "a preamble-only cache never reaches EOF");
}

/// `docs/design/decisions.md`, "The compressed source and the cache": `CacheMode::Offline` is rejected by every method that
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
/// (`docs/design/decisions.md`, "The compressed source and the cache").
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
/// (`docs/design/decisions.md`, "The compressed source and the cache").
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
/// `mise`-pinned, so a missing binary fails loudly rather than skipping
/// (`docs/design/roadmap.md`, "A test may assume the tools `mise` pins").
///
/// **512, not a round number picked for looks**: `edge_cases.sql` is 2,352
/// bytes, so a `--block-size` at or above that never actually splits it —
/// confirmed with `xz --list -v`. 512 yields 5 blocks on this fixture;
/// callers that need the seekable property to hold assert `is_seekable()`
/// themselves rather than trusting the picked size to keep working as the
/// fixture changes.
fn xz_compress(path: &Path) -> tempfile::NamedTempFile {
    let out = Command::new("xz")
        .arg("--block-size=512")
        .arg("-c")
        .arg(path)
        .output()
        .expect("`xz` is not runnable, so this test cannot build its fixture; install it.");
    assert!(out.status.success(), "xz failed: {}", String::from_utf8_lossy(&out.stderr));
    let mut compressed = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut compressed, &out.stdout).unwrap();
    compressed
}

/// `XzSource` is just another `ByteRangeSource` to everything above `io.rs`
/// (`docs/design/decisions.md`, "D6"): `build_index`'s scan and
/// `cache::save`/`load`'s round trip produce the same `DumpIndex` whether the
/// bytes came straight off disk or through the decoder — the differential
/// parity at the library level. The CLI-level parity against generated
/// fixtures is `pgdump_query-cli/tests/xz_source.rs`.
#[tokio::test]
async fn xz_source_produces_the_same_index_and_cache_as_the_plain_file() {
    let plain = LocalFileSource::open(edge_cases()).unwrap();
    let plain_index = build_index(&plain, &ScanOptions::default()).await.unwrap();

    let compressed = xz_compress(&edge_cases());
    let xz = XzSource::open(compressed.path()).unwrap();
    assert!(
        xz.seek_table().unwrap().is_seekable(),
        "this test's whole point is the seekable shape — a non-seekable fixture would earn a \
         D2 diagnostic the plain file's index does not have, and pass for the wrong reason"
    );
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
        CacheStatus::Valid { index, mtime_changed, total_size, compression } => {
            assert!(!mtime_changed, "just-saved cache must match the source's current mtime");
            assert_eq!(total_size, plain.size().await.unwrap());
            assert_eq!(index, plain_index);
            // The shape comes back off the persisted seek table, so a
            // reader learns what to raise a budget to without `xz --list`.
            let shape = compression.expect("an .xz source records its container's shape");
            assert_eq!(shape.container, "xz");
            assert_eq!(shape.blocks, xz.seek_table().unwrap().block_count());
            assert_eq!(shape.streams, xz.seek_table().unwrap().stream_count());
            assert_eq!(
                shape.max_block_uncompressed,
                xz.seek_table().unwrap().max_block_uncompressed()
            );
            assert!(shape.blocks > 1, "the --block-size=512 fixture is multi-block");
        }
        other => panic!("a freshly saved cache must load, got {other:?}"),
    }
}

/// Compress `path` with a bare `xz` invocation — no `-T`/`--block-size` — so
/// it comes out one stream, one block: the non-seekable shape
/// (`docs/design/decisions.md`, "D19").
fn xz_compress_single_block(path: &Path) -> tempfile::NamedTempFile {
    let out = Command::new("xz")
        .arg("-c")
        .arg(path)
        .output()
        .expect("`xz` is not runnable, so this test cannot build its fixture; install it.");
    assert!(out.status.success(), "xz failed: {}", String::from_utf8_lossy(&out.stderr));
    let mut compressed = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut compressed, &out.stdout).unwrap();
    compressed
}

fn has_non_seekable_warning(diagnostics: &[pgdump_query::Diagnostic]) -> bool {
    diagnostics.iter().any(|d| matches!(d.kind, DiagnosticKind::NonSeekableCompressedSource { .. }))
}

/// `build_index` warns about a source with no seek structure, and does
/// not warn about the same content compressed seekably
/// (`docs/design/decisions.md`, "D19").
#[tokio::test]
async fn build_index_warns_about_a_non_seekable_xz_source() {
    let non_seekable = xz_compress_single_block(&edge_cases());
    let xz = XzSource::open(non_seekable.path()).unwrap();
    assert!(
        !xz.seek_table().unwrap().is_seekable(),
        "fixture must actually be single-block for this test to mean anything"
    );
    let index = build_index(&xz, &ScanOptions::default()).await.unwrap();
    assert!(has_non_seekable_warning(&index.diagnostics), "diagnostics: {:?}", index.diagnostics);

    let seekable = xz_compress(&edge_cases());
    let xz_seekable = XzSource::open(seekable.path()).unwrap();
    assert!(xz_seekable.seek_table().unwrap().is_seekable(), "fixture must actually be seekable");
    let seekable_index = build_index(&xz_seekable, &ScanOptions::default()).await.unwrap();
    assert!(!has_non_seekable_warning(&seekable_index.diagnostics));
}

/// The warning survives a save/load round trip through the persisted
/// `compression` field — `status_from_file` recomputes it from the cache
/// alone, with no live source to re-walk
/// (`docs/design/decisions.md`, "D22").
#[tokio::test]
async fn a_non_seekable_warning_survives_the_cache_round_trip() {
    let non_seekable = xz_compress_single_block(&edge_cases());
    let xz = XzSource::open(non_seekable.path()).unwrap();
    let index = build_index(&xz, &ScanOptions::default()).await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("single_block.xz.dqcache");
    cache::save(&path, &xz, &index).await.unwrap();
    match cache::load(&path, &xz).await.unwrap() {
        CacheStatus::Valid { index, .. } => {
            assert!(
                has_non_seekable_warning(&index.diagnostics),
                "diagnostics: {:?}",
                index.diagnostics
            );
        }
        other => panic!("expected Valid, got {other:?}"),
    }
}

/// `preamble_only` pushes the same warning on a fresh scan, and does not
/// duplicate it on a second call that finds the preamble already complete in
/// the cache the first call just wrote.
#[tokio::test]
async fn preamble_only_warns_without_duplicating_across_calls() {
    let non_seekable = xz_compress_single_block(&edge_cases());
    let xz = XzSource::open(non_seekable.path()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mode = CacheMode::Enabled(dir.path().join("preamble.xz.dqcache"));

    let (_metadata, diagnostics) =
        preamble_only(&xz, &ScanOptions::default(), &mode).await.unwrap();
    assert_eq!(
        diagnostics
            .iter()
            .filter(|d| matches!(d.kind, DiagnosticKind::NonSeekableCompressedSource { .. }))
            .count(),
        1,
        "diagnostics: {diagnostics:?}"
    );

    // The preamble is now complete in the cache this just wrote, so this
    // second call takes the `known` branch — the warning it inherits from
    // the cache load must not be pushed a second time.
    let (_metadata, diagnostics) =
        preamble_only(&xz, &ScanOptions::default(), &mode).await.unwrap();
    assert_eq!(
        diagnostics
            .iter()
            .filter(|d| matches!(d.kind, DiagnosticKind::NonSeekableCompressedSource { .. }))
            .count(),
        1,
        "diagnostics: {diagnostics:?}"
    );
}

/// The saving, end to end at the library level: a cache saved from an `.xz`
/// source hands its seek table back to recognition, which builds a source
/// from it instead of re-walking the file's stream footers
/// (`docs/design/decisions.md`, "D18"). The table the
/// new source reports is the one that was persisted, and it reads the same
/// bytes.
#[tokio::test]
async fn a_saved_cache_hands_its_seek_table_back_to_recognition() {
    let compressed = xz_compress(&edge_cases());
    let xz = XzSource::open(compressed.path()).unwrap();
    assert!(xz.seek_table().unwrap().is_seekable(), "fixture must actually be seekable");
    let index = build_index(&xz, &ScanOptions::default()).await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge_cases.sql.xz.dqcache");
    cache::save(&path, &xz, &index).await.unwrap();

    let known = match cache::claim(&path, compressed.path()).unwrap() {
        CacheClaim::Compression(known) => known,
        other => panic!("the file's own cache describes it: {other:?}"),
    };
    assert_eq!(known, KnownCompression::Xz(xz.seek_table().unwrap()));

    let source = match open_local(compressed.path(), known).unwrap() {
        Recognized::Source(source) => source,
        Recognized::Mismatch => panic!("the file's own cache must describe it"),
    };
    assert_eq!(source.seek_table(), xz.seek_table());
    assert_eq!(source.size().await.unwrap(), xz.size().await.unwrap());
    let whole = source.size().await.unwrap() as usize;
    assert_eq!(source.read_range(0, whole).await.unwrap(), xz.read_range(0, whole).await.unwrap());
}

/// A cache saved from a source with no compression layer says so, which is a
/// claim recognition can check rather than an absence it has to guess at.
#[tokio::test]
async fn a_cache_saved_from_a_plain_source_claims_plain() {
    let plain = LocalFileSource::open(edge_cases()).unwrap();
    let index = build_index(&plain, &ScanOptions::default()).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge_cases.sql.dqcache");
    cache::save(&path, &plain, &index).await.unwrap();

    assert_eq!(
        cache::claim(&path, &edge_cases()).unwrap(),
        CacheClaim::Compression(KnownCompression::Plain)
    );
}

/// Every unusable outcome but one collapses to "nothing is known": there is no
/// cache, it is foreign bytes, or there is no file to compare it against.
/// `load` is what reports *why* a moment later, unchanged. The exception —
/// a cache recorded against a file of another stored size — is the test below.
#[tokio::test]
async fn a_claim_is_unknown_wherever_the_cache_is_unusable() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nothing.dqcache");
    assert_eq!(
        cache::claim(&missing, &edge_cases()).unwrap(),
        CacheClaim::Compression(KnownCompression::Unknown)
    );

    let foreign = dir.path().join("foreign.dqcache");
    std::fs::write(&foreign, b"not a cache at all").unwrap();
    assert_eq!(
        cache::claim(&foreign, &edge_cases()).unwrap(),
        CacheClaim::Compression(KnownCompression::Unknown)
    );

    let plain = LocalFileSource::open(edge_cases()).unwrap();
    let index = build_index(&plain, &ScanOptions::default()).await.unwrap();
    let path = dir.path().join("edge_cases.sql.dqcache");
    cache::save(&path, &plain, &index).await.unwrap();
    // A real cache, and a dump path with nothing at it at all: there is no
    // size to compare against, and the open that follows is where that has a
    // sentence to say.
    assert_eq!(
        cache::claim(&path, &dir.path().join("gone.sql")).unwrap(),
        CacheClaim::Compression(KnownCompression::Unknown)
    );
}

/// The one unusable outcome a caller can act on before opening anything: a
/// cache whose recorded stored size is not this file's describes some *other*
/// file, and every command refuses it. The stored-size check is the same one
/// `load` applies, done here against a plain `stat` because no source exists
/// yet — and answering it here is what spares an `.xz` file the stream-footer
/// walk it would otherwise pay to reach that refusal
/// (`docs/design/decisions.md`, "D20").
#[tokio::test]
async fn a_cache_recorded_against_another_file_is_settled_before_any_source_exists() {
    let dir = tempfile::tempdir().unwrap();
    let compressed = xz_compress(&edge_cases());
    let xz = XzSource::open(compressed.path()).unwrap();
    let index = build_index(&xz, &ScanOptions::default()).await.unwrap();
    let path = dir.path().join("edge_cases.sql.xz.dqcache");
    cache::save(&path, &xz, &index).await.unwrap();

    // The compressed file's own cache, put to the plain file it decompresses
    // to: same content, different stored size.
    assert_eq!(
        cache::claim(&path, &edge_cases()).unwrap(),
        CacheClaim::SourceChanged {
            cached_stored_size: std::fs::metadata(compressed.path()).unwrap().len(),
            live_stored_size: std::fs::metadata(edge_cases()).unwrap().len(),
        },
        "a cache of the compressed file must not be believed about the plain one"
    );

    // The same verdict `load` reaches once a source exists, which is what
    // makes the early refusal a pre-emption rather than a second rule.
    let plain = LocalFileSource::open(edge_cases()).unwrap();
    assert_eq!(
        cache::load(&path, &plain).await.unwrap(),
        CacheStatus::SourceChanged {
            cached_stored_size: std::fs::metadata(compressed.path()).unwrap().len(),
            live_stored_size: std::fs::metadata(edge_cases()).unwrap().len(),
        }
    );
}
