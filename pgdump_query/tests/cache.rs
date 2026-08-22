//! On-disk structure cache: round-tripping, and the "unusable cache is
//! treated as absent" contract `docs/design/mvp.md` requires.

use std::path::{Path, PathBuf};

use pgdump_query::{LocalFileSource, ScanOptions, build_index, cache};

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

#[test]
fn missing_cache_file_is_treated_as_absent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nonexistent.dqcache");
    assert!(cache::load(&path).unwrap().is_none());
}

#[test]
fn foreign_bytes_at_the_cache_path_are_treated_as_absent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("garbage.dqcache");
    std::fs::write(&path, b"not a cache file").unwrap();
    assert!(cache::load(&path).unwrap().is_none());
}

/// A saved index round-trips exactly, including the reserved-but-unpopulated
/// fields (`DumpIndex::metadata`, `CopyBlock::sparse_index`/`column_stats`) —
/// they must serialize as `None` rather than being silently dropped, which is
/// the whole point of reserving them ahead of population.
#[tokio::test]
async fn saved_index_round_trips_exactly() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    assert!(!index.blocks.is_empty());
    assert!(index.blocks.iter().all(|b| b.sparse_index.is_none() && b.column_stats.is_none()));
    assert!(index.metadata.is_none());

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge_cases.sql.dqcache");
    cache::save(&path, &index).unwrap();
    let loaded = cache::load(&path).unwrap().expect("a freshly saved cache must load");

    assert_eq!(loaded, index);
}

#[tokio::test]
async fn save_overwrites_an_existing_cache() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge_cases.sql.dqcache");
    std::fs::write(&path, b"stale placeholder").unwrap();
    cache::save(&path, &index).unwrap();

    assert_eq!(cache::load(&path).unwrap().as_ref(), Some(&index));
}
