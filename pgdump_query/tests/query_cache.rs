//! `table_stream`/`read_table`'s cache-consulting behavior
//! (`docs/design/decisions.md`, "The compressed source and the cache"): replaying
//! already- cached blocks, skipping non-matching ones at zero I/O cost, and
//! persisting newly-discovered blocks as a live scan finds them. See
//! `tests/stream.rs`/`tests/batch.rs` for cache-free behavior, and
//! `tests/cache.rs` for the on-disk cache format itself.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use futures::StreamExt;
use pgdump_query::cache::{CacheLoad, CacheMode};
use pgdump_query::{
    ByteRangeSource, LocalFileSource, QueryOptions, ScanExtent, ScanOptions, build_index, cache,
    check_tiling, table_stream,
};

mod common;
use common::{objects_fixture, rows_of, sandboxed, sandboxed_edge_cases};

/// A private copy of `fixtures/16/objects/default.sql`, the same convention
/// as [`sandboxed_edge_cases`].
fn sandboxed_objects_fixture() -> (tempfile::TempDir, PathBuf) {
    sandboxed(&objects_fixture(16, "default"), "objects.sql")
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
    source: &dyn ByteRangeSource,
    table: &str,
    batch_options: QueryOptions,
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
    ) -> Pin<Box<dyn Future<Output = pgdump_query::Result<Bytes>> + Send + '_>> {
        self.bytes_read.fetch_add(len as u64, Ordering::SeqCst);
        self.inner.read_range(offset, len)
    }

    fn size(&self) -> Pin<Box<dyn Future<Output = pgdump_query::Result<u64>> + Send + '_>> {
        self.inner.size()
    }

    fn modified(
        &self,
    ) -> Pin<
        Box<dyn Future<Output = pgdump_query::Result<Option<std::time::SystemTime>>> + Send + '_>,
    > {
        self.inner.modified()
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
    cache::save(&cache_path, &seed_source, &index).await.unwrap();
    let empty_table_block =
        index.blocks_for("public.empty_table").next().expect("empty_table was indexed");
    let expected_bytes = empty_table_block.end_offset - empty_table_block.header_offset;

    let counting = CountingSource::wrap(LocalFileSource::open(&dump).unwrap());
    let rows = drain(
        &counting,
        "public.empty_table",
        QueryOptions::default(),
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
    cache::save(&cache_path, &seed_source, &index).await.unwrap();

    let source = LocalFileSource::open(&dump).unwrap();
    let widgets = drain(
        &source,
        "public.widgets",
        QueryOptions::default(),
        CacheMode::Enabled(cache_path.clone()),
    )
    .await;
    assert_eq!(widgets, widgets_expected());

    let headerless = drain(
        &source,
        "public.no_column_list",
        QueryOptions::default(),
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

    let rows = drain(&source, "public.widgets", QueryOptions::default(), CacheMode::Disabled).await;
    assert_eq!(rows, widgets_expected());
    assert!(!cache::colocated_path(&dump).exists());
}

/// A cold query maps only as far as it must: every block up to and including
/// the queried table's, and nothing past it
/// (`docs/design/decisions.md`, "D48"). `widgets` is the second of four blocks, so the
/// cache it leaves knows two tables, not four — that bound is the whole
/// point, since it is what keeps a query against an early table in a huge
/// dump from costing a full scan.
#[tokio::test]
async fn a_cold_query_maps_up_to_its_target_and_stops() {
    let (_dir, dump) = sandboxed_edge_cases();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();

    let rows = drain(
        &source,
        "public.widgets",
        QueryOptions::default(),
        CacheMode::Enabled(cache_path.clone()),
    )
    .await;
    assert_eq!(rows, widgets_expected());

    let CacheLoad::Index(index) =
        CacheMode::Enabled(cache_path.clone()).load(&source).await.unwrap()
    else {
        panic!("a cache was written")
    };
    let mut tables: Vec<&str> = index.blocks().map(|b| b.header.table.as_str()).collect();
    tables.sort_unstable();
    assert_eq!(tables, vec!["empty_table", "widgets"]);
    let widgets = index.blocks_for("public.widgets").next().expect("the queried block is mapped");
    assert_eq!(index.scanned_through, widgets.end_offset, "stopped at the target, not at EOF");
    assert!(index.scanned_through < source.size().await.unwrap());
}

/// `ScanExtent::Full` is the way back to a whole-file map from a query — the
/// same coverage `pgdq parse` produces, at the cost of a full scan.
#[tokio::test]
async fn scan_extent_full_maps_the_whole_file_from_a_query() {
    let (_dir, dump) = sandboxed_edge_cases();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();

    let rows = drain(
        &source,
        "public.widgets",
        QueryOptions { scan_extent: ScanExtent::Full, ..Default::default() },
        CacheMode::Enabled(cache_path.clone()),
    )
    .await;
    assert_eq!(rows, widgets_expected(), "the row set is the same either way");

    let CacheLoad::Index(index) = CacheMode::Enabled(cache_path).load(&source).await.unwrap()
    else {
        panic!("a cache was written")
    };
    let mut tables: Vec<&str> = index.blocks().map(|b| b.header.table.as_str()).collect();
    tables.sort_unstable();
    assert_eq!(tables, vec!["Odd Table", "empty_table", "no_column_list", "widgets"]);
    assert_eq!(index.scanned_through, source.size().await.unwrap());
}

/// The concrete case `docs/design/decisions.md`'s
/// "D31" names for why the cross-reference set can't stop
/// where the map does: `objects.widgets`' own `COPY` block closes long before
/// the file's post-data `GRANT`/`ALTER DEFAULT PRIVILEGES` section
/// (`fixtures/16/objects/default.sql`) grants `fixture_reader` access to it.
/// A cold query that stops at its target never reaches that section, so its
/// `roles` set is real but incomplete — `postgres` (every preceding object's
/// owner) without `fixture_reader`. `ScanExtent::Full` is the way to the
/// complete set, the same way it already is for `blocks()`.
#[tokio::test]
async fn a_cold_query_misses_post_data_grants_but_scan_extent_full_finds_them() {
    let (_dir, dump) = sandboxed_objects_fixture();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();

    drain(
        &source,
        "objects.widgets",
        QueryOptions::default(),
        CacheMode::Enabled(cache_path.clone()),
    )
    .await;
    let CacheLoad::Index(index) =
        CacheMode::Enabled(cache_path.clone()).load(&source).await.unwrap()
    else {
        panic!("a cache was written")
    };
    assert!(index.scanned_through < source.size().await.unwrap(), "stopped before EOF");
    assert!(index.roles.contains("postgres"));
    assert!(
        !index.roles.contains("fixture_reader"),
        "the GRANT that names it is past the stopping point: {:?}",
        index.roles
    );

    drain(
        &source,
        "objects.widgets",
        QueryOptions { scan_extent: ScanExtent::Full, ..Default::default() },
        CacheMode::Enabled(cache_path.clone()),
    )
    .await;
    let CacheLoad::Index(index) = CacheMode::Enabled(cache_path).load(&source).await.unwrap()
    else {
        panic!("a cache was written")
    };
    assert_eq!(index.scanned_through, source.size().await.unwrap());
    assert!(
        index.roles.contains("fixture_reader"),
        "a full scan reaches the GRANT: {:?}",
        index.roles
    );
}

/// Dropping a stream partway through a block still leaves a valid, usable
/// cache behind, and a later query against it still finds everything — no
/// bytes silently skipped because an earlier query stopped early.
///
/// The queried block is **fully mapped before its first row is emitted**
/// (`docs/design/decisions.md`, "D48"), so stopping mid-`widgets` still leaves `widgets`
/// itself in the cache — the map can never be behind the rows a caller has
/// already seen. That is the property the split exists for, and it is what
/// lets a resume point always land inside mapped territory.
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
            QueryOptions { max_rows: 1, max_bytes: None, ..Default::default() },
            None,
            CacheMode::Enabled(cache_path.clone()),
        );
        // `widgets` is the second block in the file; pulling two of its six
        // one-row batches stops well before its own `CopyEnd`.
        for _ in 0..2 {
            stream.next().await.unwrap().unwrap();
        }
    }

    let CacheLoad::Index(index) =
        CacheMode::Enabled(cache_path.clone()).load(&source).await.unwrap()
    else {
        panic!("partial progress was persisted")
    };
    let tables: Vec<&str> = index.blocks().map(|b| b.header.table.as_str()).collect();
    assert_eq!(
        tables,
        vec!["empty_table", "widgets"],
        "the target block is mapped before any of its rows go out"
    );
    let widgets = index.blocks_for("public.widgets").next().unwrap();
    assert_eq!(index.scanned_through, widgets.end_offset);

    // A second query against the same (partial) cache still finds
    // everything past where the first one stopped.
    let headerless = drain(
        &source,
        "public.no_column_list",
        QueryOptions::default(),
        CacheMode::Enabled(cache_path),
    )
    .await;
    assert_eq!(
        headerless,
        vec![vec![Some("\\.".to_string())], vec![Some("just a value".to_string())]]
    );
}

/// The first database's preamble is captured up front, before any segment
/// scanning begins — so even a stream dropped after its very first row
/// still leaves a cache with `DumpMetadata` behind, not just whatever
/// blocks happened to complete before the drop.
#[tokio::test]
async fn interrupted_scan_still_captures_the_first_database_preamble() {
    let (_dir, dump) = sandboxed_edge_cases();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();

    {
        let mut stream = table_stream(
            &source,
            "public.widgets",
            ScanOptions::default(),
            QueryOptions { max_rows: 1, max_bytes: None, ..Default::default() },
            None,
            CacheMode::Enabled(cache_path.clone()),
        );
        // One row of `widgets` (the second real block) is enough to prove
        // the point without waiting on any block, let alone the file, to
        // finish.
        stream.next().await.unwrap().unwrap();
    }

    let CacheLoad::Index(index) =
        CacheMode::Enabled(cache_path.clone()).load(&source).await.unwrap()
    else {
        panic!("progress was persisted")
    };
    assert!(
        index.scanned_through < source.size().await.unwrap(),
        "sanity check: this scan really did stop well short of EOF"
    );
    let metadata = index.metadata.expect("preamble captured despite the early stop");
    let first_db = metadata.databases.first().expect("at least one database entry");
    assert!(first_db.preamble_complete);
}

/// Repeat queries against an already-fully-cached file don't grow or
/// duplicate the persisted index.
#[tokio::test]
async fn no_duplication_on_repeat_queries() {
    let (_dir, dump) = sandboxed_edge_cases();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();

    // `ScanExtent::Full` is what makes the file *fully* cached, which is the
    // precondition this test is about; the default stops at its target.
    drain(
        &source,
        "public.widgets",
        QueryOptions { scan_extent: ScanExtent::Full, ..Default::default() },
        CacheMode::Enabled(cache_path.clone()),
    )
    .await;
    let CacheLoad::Index(before) =
        CacheMode::Enabled(cache_path.clone()).load(&source).await.unwrap()
    else {
        panic!("the first query wrote a cache")
    };

    drain(
        &source,
        "public.no_column_list",
        QueryOptions::default(),
        CacheMode::Enabled(cache_path.clone()),
    )
    .await;
    let CacheLoad::Index(after) =
        CacheMode::Enabled(cache_path.clone()).load(&source).await.unwrap()
    else {
        panic!("the second query left a cache")
    };

    let after_count = after.blocks().count();
    assert_eq!(before.blocks().count(), after_count);
    assert_eq!(before.spans, after.spans, "a fully-mapped file is not remapped at all");
    let mut offsets: Vec<u64> = after.blocks().map(|b| b.header_offset).collect();
    offsets.sort_unstable();
    offsets.dedup();
    assert_eq!(offsets.len(), after_count, "no duplicate header_offsets");
}

/// **The claim the mapping/streaming split exists for.** A `DumpIndex` built
/// by a query
/// tiles its file exactly, the same way `build_index`'s does — every byte in
/// exactly one span, no gaps, no overlaps — with no exemption for a partial
/// scan, a warm cache, or a resumed stream
/// (`docs/design/decisions.md`, "D30").
///
/// Each state below stresses a different seam in the splice: the cold case
/// joins the preamble prepass's spans to a live segment's; the warm case
/// joins a *previously persisted* prefix to a new segment starting at the old
/// frontier; and the `Full` case runs the segment all the way to EOF, where
/// the trailing `Unscanned` span disappears entirely.
#[tokio::test]
async fn a_query_built_index_tiles_in_every_cache_state() {
    for schema_dir in ["edge_cases", "objects", "partitions", "statistics", "types"] {
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
            let cache_path = cache::colocated_path(&dump);
            let source = LocalFileSource::open(&dump).unwrap();
            let size = source.size().await.unwrap();
            let label = format!("{schema_dir}/{flag_set}");

            // Cold: nothing cached, so the prepass's spans are the only
            // prefix the live segment has to splice onto.
            drain(
                &source,
                "public.widgets",
                QueryOptions::default(),
                CacheMode::Enabled(cache_path.clone()),
            )
            .await;
            let CacheLoad::Index(cold) =
                CacheMode::Enabled(cache_path.clone()).load(&source).await.unwrap()
            else {
                panic!("{label}: the cold query wrote a cache")
            };
            assert_eq!(check_tiling(&cold.spans, size), vec![], "{label}: cold");

            // Warm: a second query resumes mapping from the persisted
            // frontier, splicing onto spans it did not build itself.
            drain(
                &source,
                "public.no_column_list",
                QueryOptions::default(),
                CacheMode::Enabled(cache_path.clone()),
            )
            .await;
            let CacheLoad::Index(warm) =
                CacheMode::Enabled(cache_path.clone()).load(&source).await.unwrap()
            else {
                panic!("{label}: the warm query left a cache")
            };
            assert_eq!(check_tiling(&warm.spans, size), vec![], "{label}: warm");
            assert!(warm.scanned_through >= cold.scanned_through, "{label}: coverage only grows");

            // Full: the segment reaches EOF, so there is no `Unscanned` tail
            // and the map should match what a full `build_index` scan gives.
            drain(
                &source,
                "public.widgets",
                QueryOptions { scan_extent: ScanExtent::Full, ..Default::default() },
                CacheMode::Enabled(cache_path.clone()),
            )
            .await;
            let CacheLoad::Index(full) =
                CacheMode::Enabled(cache_path.clone()).load(&source).await.unwrap()
            else {
                panic!("{label}: the full query left a cache")
            };
            assert_eq!(check_tiling(&full.spans, size), vec![], "{label}: full");
            assert_eq!(full.scanned_through, size, "{label}");
            let eager = build_index(&source, &ScanOptions::default()).await.unwrap();
            // Span for span, array-shape census included: every mapping pass
            // censuses, so the three queries that built this index agree
            // with a single eager scan in every field
            // (`docs/design/decisions.md`, "D35").
            assert_eq!(
                full.spans, eager.spans,
                "{label}: a fully-mapped query agrees with build_index span for span"
            );
        }
    }
}

/// A resumed stream replays out of the map rather than falling back to a
/// live scan, so the index it leaves tiles like any other, not only the one
/// `build_index` produces directly.
#[tokio::test]
async fn a_resumed_query_leaves_a_tiling_index() {
    let (_dir, dump) = sandboxed_edge_cases();
    let cache_path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();
    let size = source.size().await.unwrap();

    let token = {
        let mut stream = table_stream(
            &source,
            "public.widgets",
            ScanOptions::default(),
            QueryOptions { max_rows: 1, max_bytes: None, ..Default::default() },
            None,
            CacheMode::Enabled(cache_path.clone()),
        );
        stream.next().await.unwrap().unwrap();
        stream.resume_token()
    };

    let mut resumed = table_stream(
        &source,
        "public.widgets",
        ScanOptions::default(),
        QueryOptions::default(),
        Some(token),
        CacheMode::Enabled(cache_path.clone()),
    );
    let mut rows = Vec::new();
    while let Some(batch) = resumed.next().await {
        rows.extend(rows_of(&batch.unwrap()));
    }
    assert_eq!(rows, widgets_expected()[1..], "the rows the first stream hadn't delivered");

    let CacheLoad::Index(index) = CacheMode::Enabled(cache_path).load(&source).await.unwrap()
    else {
        panic!("the resumed query left a cache")
    };
    assert_eq!(check_tiling(&index.spans, size), vec![]);
}
