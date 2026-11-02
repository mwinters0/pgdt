//! What a mapping pass holds for a row many chunks long: none of it where
//! the row's block is at the metadata level, the pass streaming the row
//! (`pgdt_dump::ChunkEvents::stream_rows`), serially and in an interior
//! worker alike, and the row's bytes where the block's statistics are
//! gathered — in a mapping of the line's own, outside the allocator
//! (`docs/design/decisions.md`, "D139"). A census alone holds only the
//! fields it decodes (`pgdt-query`'s `streamed_census.rs`).
//!
//! **This binary installs a global allocator recording the largest single
//! allocation**, which is why it is a test file of its own, and reads the
//! process's peak resident set, reset before each pass
//! (`/proc/self/clear_refs`): a held line is resident at its length, and
//! nothing else a pass over this file allocates comes near it.

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

/// A field of `/proc/self/status`, in bytes.
fn status_bytes(field: &str) -> usize {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find(|line| line.starts_with(field)).unwrap();
    let kib: usize = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    kib << 10
}

/// What a pass held: its largest allocation, and how far its peak resident
/// set rose past the resident set it began with.
struct Held {
    allocation: usize,
    resident: usize,
}

/// What a cacheless `map_file` of `dump` holds at `jobs` under `statistics`,
/// and the rows it counted in its one block.
async fn largest(
    dump: &std::path::Path,
    jobs: usize,
    statistics: &StatisticsRequest,
) -> (Held, u64) {
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(dump).unwrap());
    let opened = CacheMode::DISABLED.open_source(source).await.unwrap();
    let read = ReadOptions { chunk_size_bytes: CHUNK, ..ReadOptions::default() };
    LARGEST.store(0, Ordering::Relaxed);
    // `5` resets the peak resident set to the current one (`proc(5)`).
    std::fs::write("/proc/self/clear_refs", "5").unwrap();
    let before = status_bytes("VmRSS:");
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
    let held = Held {
        allocation: LARGEST.load(Ordering::Relaxed),
        resident: status_bytes("VmHWM:").saturating_sub(before),
    };
    let rows = run
        .index
        .spans
        .iter()
        .find_map(|span| match &span.body {
            SpanBody::Data(DataBlock::Copy(block)) => Some(block.row_count),
            _ => None,
        })
        .unwrap();
    (held, rows)
}

/// **A metadata-level parse holds no row it counts**: one of a row
/// sixteen MiB long among short ones holds nothing near the row, serially
/// and at four workers, where a data-level one holds it whole — and
/// allocates nothing near it either, the line past its chunk held in a
/// mapping of its own.
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

    drop(file);
    let (held, rows) = largest(&dump, 1, &StatisticsRequest::DATA).await;
    assert_eq!(rows, 1002);
    // A resident set read against the one before, which what the pass frees
    // lowers: three quarters of the row, past anything else the pass holds.
    let resident = held.resident;
    assert!(resident > LONG / 4 * 3, "a data-level parse holds the row: {resident} bytes");
    let allocation = held.allocation;
    assert!(allocation < LONG / 8, "the row held in the allocator: a {allocation}-byte allocation");
    for jobs in [1usize, 4] {
        let (streamed, rows) = largest(&dump, jobs, &StatisticsRequest::METADATA).await;
        assert_eq!(rows, 1002, "{jobs} jobs");
        let allocation = streamed.allocation;
        assert!(allocation < LONG / 8, "{jobs} jobs: a {allocation}-byte allocation");
        assert!(
            streamed.resident + LONG / 2 < held.resident,
            "{jobs} jobs: {} bytes resident beside a held row's {}",
            streamed.resident,
            held.resident,
        );
    }
}
