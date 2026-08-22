//! `table_stream`/`read_table`'s cache-consulting behavior
//! (`docs/design/mvp.md`, "Index / structure cache"): replaying already-
//! cached blocks, skipping non-matching ones at zero I/O cost, and
//! persisting newly-discovered blocks as a live scan finds them. See
//! `tests/stream.rs`/`tests/batch.rs` for cache-free behavior, and
//! `tests/cache.rs` for the on-disk cache format itself.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    BatchOptions, ByteRangeSource, LocalFileSource, ScanOptions, build_index, cache, table_stream,
};

fn edge_cases() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/edge_cases.sql")
}

/// A private copy of `edge_cases.sql` in a fresh tempdir, so every test can
/// freely read/write a colocated `.dqcache` next to it without touching the
/// checked-in fixture.
fn sandboxed_edge_cases() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("edge_cases.sql");
    std::fs::copy(edge_cases(), &dump).unwrap();
    (dir, dump)
}

fn rows_of(batch: &arrow::array::RecordBatch) -> Vec<Vec<Option<String>>> {
    use arrow::array::{Array, StringViewArray};
    let columns: Vec<&StringViewArray> = batch
        .columns()
        .iter()
        .map(|c| c.as_any().downcast_ref::<StringViewArray>().unwrap())
        .collect();
    (0..batch.num_rows())
        .map(|row| {
            columns.iter().map(|c| c.is_valid(row).then(|| c.value(row).to_string())).collect()
        })
        .collect()
}

fn widgets_expected() -> Vec<Vec<Option<String>>> {
    vec![
        vec![
            Some("1".into()),
            Some("alpha".into()),
            Some("a simple widget".into()),
            Some("2024-01-01 00:00:00+00".into()),
        ],
        vec![Some("2".into()), Some("beta".into()), None, Some("2024-01-02 00:00:00+00".into())],
        vec![
            Some("3".into()),
            Some("gamma".into()),
            Some("multi\nline\twith a backslash \\ inside".into()),
            None,
        ],
        vec![
            Some("4".into()),
            Some("delta".into()),
            Some(
                "contains a COPY-like phrase: COPY public.widgets (id) FROM stdin; -- not a directive"
                    .into(),
            ),
            Some("2024-01-04 00:00:00+00".into()),
        ],
        vec![
            Some("5".into()),
            Some("".into()),
            Some("empty name to the left".into()),
            Some("2024-01-05 00:00:00+00".into()),
        ],
        vec![
            Some("6".into()),
            Some("epsilon".into()),
            Some("carriage\rreturn, octal A, hex B".into()),
            Some("2024-01-06 00:00:00+00".into()),
        ],
    ]
}

async fn drain(
    source: &impl ByteRangeSource,
    table: &str,
    batch_options: BatchOptions,
    cache: CacheMode,
) -> Vec<Vec<Option<String>>> {
    let mut stream =
        table_stream(source, table, ScanOptions::default(), batch_options, None, cache);
    let mut rows = Vec::new();
    while let Some(batch) = stream.next().await {
        rows.extend(rows_of(&batch.unwrap()));
    }
    rows
}

/// A `ByteRangeSource` wrapper that counts every byte actually requested,
/// to prove non-matching cached blocks cost zero I/O rather than just
/// trusting the row output.
struct CountingSource {
    inner: LocalFileSource,
    bytes_read: AtomicU64,
}

impl CountingSource {
    fn wrap(inner: LocalFileSource) -> Self {
        Self { inner, bytes_read: AtomicU64::new(0) }
    }

    fn bytes_read(&self) -> u64 {
        self.bytes_read.load(Ordering::SeqCst)
    }
}

impl ByteRangeSource for CountingSource {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> impl Future<Output = pgdump_query::Result<Bytes>> + Send {
        self.bytes_read.fetch_add(len as u64, Ordering::SeqCst);
        self.inner.read_range(offset, len)
    }

    fn size(&self) -> impl Future<Output = pgdump_query::Result<u64>> + Send {
        self.inner.size()
    }
}

/// Once the whole file is cached, querying a table other than the last one
/// in file order costs exactly that table's own block bytes — nothing for
/// the other cached (non-matching) blocks, and nothing for a live rescan.
#[tokio::test]
async fn non_matching_cached_blocks_cost_zero_bytes() {
    let (_dir, dump) = sandboxed_edge_cases();
    let cache_path = cache::colocated_path(&dump);

    let seed_source = LocalFileSource::open(&dump).unwrap();
    let index = build_index(&seed_source, &ScanOptions::default()).await.unwrap();
    cache::save(&cache_path, &index).unwrap();
    let empty_table_block =
        index.blocks_for("public.empty_table").next().expect("empty_table was indexed");
    let expected_bytes = empty_table_block.end_offset - empty_table_block.header_offset;

    let counting = CountingSource::wrap(LocalFileSource::open(&dump).unwrap());
    let rows = drain(
        &counting,
        "public.empty_table",
        BatchOptions::default(),
        CacheMode::Enabled(cache_path),
    )
    .await;

    assert!(rows.is_empty(), "empty_table has no rows");
    assert_eq!(counting.bytes_read(), expected_bytes);
}

/// Rows/schema replayed from a cached block are identical to a fresh
/// (uncached) scan — both for a headered table and a headerless one (which
/// must still recover the right field count from its first row on replay).
#[tokio::test]
async fn replay_matches_a_fresh_scan() {
    let (_dir, dump) = sandboxed_edge_cases();
    let cache_path = cache::colocated_path(&dump);

    let seed_source = LocalFileSource::open(&dump).unwrap();
    let index = build_index(&seed_source, &ScanOptions::default()).await.unwrap();
    cache::save(&cache_path, &index).unwrap();

    let source = LocalFileSource::open(&dump).unwrap();
    let widgets = drain(
        &source,
        "public.widgets",
        BatchOptions::default(),
        CacheMode::Enabled(cache_path.clone()),
    )
    .await;
    assert_eq!(widgets, widgets_expected());

    let headerless = drain(
        &source,
        "public.no_column_list",
        BatchOptions::default(),
        CacheMode::Enabled(cache_path),
    )
    .await;
    assert_eq!(
        headerless,
        vec![vec![Some("\\.".to_string())], vec![Some("just a value".to_string())]]
    );
}

/// `CacheMode::Disabled` is a byte-for-byte regression no-op: same output as
/// plain streaming, and it never leaves a cache file behind.
#[tokio::test]
async fn disabled_cache_is_a_noop() {
    let (_dir, dump) = sandboxed_edge_cases();
    let source = LocalFileSource::open(&dump).unwrap();

    let rows = drain(&source, "public.widgets", BatchOptions::default(), CacheMode::Disabled).await;
    assert_eq!(rows, widgets_expected());
    assert!(!cache::colocated_path(&dump).exists());
}

/// Querying one table against a cold (nonexistent) cache leaves behind a
/// cache covering every table in the file, not just the one queried.
#[tokio::test]
async fn cold_cache_gets_fully_populated_by_one_query() {
    let (_dir, dump) = sandboxed_edge_cases();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();

    let rows = drain(
        &source,
        "public.widgets",
        BatchOptions::default(),
        CacheMode::Enabled(cache_path.clone()),
    )
    .await;
    assert_eq!(rows, widgets_expected());

    let index = cache::load(&cache_path).unwrap().expect("a cache was written");
    let mut tables: Vec<&str> = index.blocks.iter().map(|b| b.header.table.as_str()).collect();
    tables.sort_unstable();
    assert_eq!(tables, vec!["Odd Table", "empty_table", "no_column_list", "widgets"]);
    assert_eq!(index.scanned_through, source.size().await.unwrap());
}

/// Dropping a stream partway through a block still leaves a valid, usable
/// cache behind: only fully-completed blocks are recorded, and a later
/// query against the same cache still finds everything — no bytes silently
/// skipped because an earlier query stopped early.
#[tokio::test]
async fn interrupted_scan_leaves_correct_partial_progress() {
    let (_dir, dump) = sandboxed_edge_cases();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();

    {
        let mut stream = table_stream(
            &source,
            "public.widgets",
            ScanOptions::default(),
            BatchOptions { max_rows: 1, max_bytes: None },
            None,
            CacheMode::Enabled(cache_path.clone()),
        );
        // `widgets` is the second block in the file; pulling two of its six
        // one-row batches stops well before its own `CopyEnd`.
        for _ in 0..2 {
            stream.next().await.unwrap().unwrap();
        }
    }

    let index = cache::load(&cache_path).unwrap().expect("partial progress was persisted");
    assert_eq!(index.blocks.len(), 1, "only the fully-completed empty_table block is recorded");
    let empty_table = &index.blocks[0];
    assert_eq!(empty_table.header.table, "empty_table");
    assert_eq!(index.scanned_through, empty_table.end_offset);

    // A second query against the same (partial) cache still finds
    // everything past where the first one stopped.
    let headerless = drain(
        &source,
        "public.no_column_list",
        BatchOptions::default(),
        CacheMode::Enabled(cache_path),
    )
    .await;
    assert_eq!(
        headerless,
        vec![vec![Some("\\.".to_string())], vec![Some("just a value".to_string())]]
    );
}

/// Repeat queries against an already-fully-cached file don't grow or
/// duplicate the persisted index.
#[tokio::test]
async fn no_duplication_on_repeat_queries() {
    let (_dir, dump) = sandboxed_edge_cases();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();

    drain(
        &source,
        "public.widgets",
        BatchOptions::default(),
        CacheMode::Enabled(cache_path.clone()),
    )
    .await;
    let before = cache::load(&cache_path).unwrap().unwrap();

    drain(
        &source,
        "public.no_column_list",
        BatchOptions::default(),
        CacheMode::Enabled(cache_path.clone()),
    )
    .await;
    let after = cache::load(&cache_path).unwrap().unwrap();

    assert_eq!(before.blocks.len(), after.blocks.len());
    let mut offsets: Vec<u64> = after.blocks.iter().map(|b| b.header_offset).collect();
    offsets.sort_unstable();
    offsets.dedup();
    assert_eq!(offsets.len(), after.blocks.len(), "no duplicate header_offsets");
}
