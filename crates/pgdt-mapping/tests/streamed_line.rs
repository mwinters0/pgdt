//! What a mapping pass allocates for a row many chunks long: none of it
//! where the row's block is at the metadata level, the pass streaming the
//! row (`pgdt_dump::ChunkEvents::stream_rows`), serially and in an interior
//! worker alike, and the row's bytes where the block is censused.
//!
//! **This binary installs a global allocator recording the largest single
//! allocation**, which is why it is a test file of its own: a held line is
//! one allocation at least its length, wherever it grows, and nothing else a
//! pass over this file allocates comes near it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pgdt_dump::ReadOptions;
use pgdt_dump::cache::CacheMode;
use pgdt_io::{ByteRangeSource, LocalFileSource};
use pgdt_mapping::map_file;
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::{DataBlock, PostgresInvalidValues, SpanBody};
use tokio_util::sync::CancellationToken;

struct Largest;

/// The largest allocation made since the last [`reset`].
static LARGEST: AtomicUsize = AtomicUsize::new(0);

#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Largest {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LARGEST.fetch_max(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LARGEST.fetch_max(new_size, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Largest = Largest;

/// The row's length: past every buffer a pass sizes from its chunk.
const LONG: usize = 16 << 20;
/// The chunk every pass here reads in.
const CHUNK: usize = 64 << 10;

/// `jobs` workers stated under no allowance (`pgdt_fixtures::arranged`,
/// written again as `statistics_heap.rs` writes it).
fn arranged(jobs: usize) -> pgdt_io::Arrangement {
    let request = pgdt_io::ArrangementRequest {
        jobs: pgdt_io::Jobs::Stated(jobs),
        ..pgdt_io::ArrangementRequest::recommended(jobs, None)
    };
    pgdt_io::Arrangement::resolve(&pgdt_io::Allowance::NoneFound, request)
}

/// The largest allocation a cacheless `map_file` of `dump` makes at `jobs`
/// under `statistics`, and the rows it counted in its one block.
async fn largest(
    dump: &std::path::Path,
    jobs: usize,
    statistics: &StatisticsRequest,
) -> (usize, u64) {
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(dump).unwrap());
    let opened = CacheMode::DISABLED.open_source(source).await.unwrap();
    let read = ReadOptions { chunk_size_bytes: CHUNK, ..ReadOptions::default() };
    LARGEST.store(0, Ordering::Relaxed);
    let run = map_file(
        opened,
        read,
        &arranged(jobs),
        PostgresInvalidValues::Default,
        statistics,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let largest = LARGEST.load(Ordering::Relaxed);
    let rows = run
        .index
        .spans
        .iter()
        .find_map(|span| match &span.body {
            SpanBody::Data(DataBlock::Copy(block)) => Some(block.row_count),
            _ => None,
        })
        .unwrap();
    (largest, rows)
}

/// **A metadata-level parse holds no row it counts**: one of a row
/// sixteen MiB long among short ones allocates nothing near the row,
/// serially and at four workers, where a data-level one holds it whole.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_metadata_level_parse_streams_a_long_row() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("long.sql");
    let mut file = b"COPY public.t (a, b) FROM stdin;\n".to_vec();
    for i in 0..1000 {
        file.extend_from_slice(format!("{i}\tshort\n").as_bytes());
    }
    file.extend_from_slice(b"1000\t");
    file.resize(file.len() + LONG, b'x');
    file.extend_from_slice(b"\n1001\tshort\n\\.\n");
    std::fs::write(&dump, &file).unwrap();

    for jobs in [1usize, 4] {
        let (held, rows) = largest(&dump, jobs, &StatisticsRequest::METADATA).await;
        assert_eq!(rows, 1002, "{jobs} jobs");
        assert!(held < LONG / 8, "{jobs} jobs: a {held}-byte allocation for a {LONG}-byte row");
    }
    let (held, rows) = largest(&dump, 1, &StatisticsRequest::DATA).await;
    assert_eq!(rows, 1002);
    assert!(held > LONG, "a data-level parse holds the row: its largest allocation {held}");
}
