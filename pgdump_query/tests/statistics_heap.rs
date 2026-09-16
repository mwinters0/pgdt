//! What a mapping pass's statistics account says a block holds, against what
//! freeing the block gives back to the allocator.
//!
//! [`BlockStatistics::heap_bytes`] is the whole of the account's retained and
//! loaded terms, so it is checked exactly here: each block's statistics are
//! dropped with a counter in front of the allocator, and the bytes freed must
//! be the bytes it reports — for statistics a pass gathered and for statistics
//! decoded from a cache, whose vectors come back at other capacities. What the
//! account charges an observer still gathering is reconciled end to end by
//! `pgdump_query-cli/tests/statistics_account.rs`, on the instrument build.
//!
//! **This binary installs a counting global allocator**, per thread, which is
//! why it is a test file of its own: a drop on the test thread frees what it
//! frees on that thread, whatever the blocking pool is doing.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::num::NonZeroU64;
use std::path::Path;
use std::sync::Arc;

use pgdump_query::cache::CacheMode;
use pgdump_query::map::{DataBlock, SpanBody};
use pgdump_query::{
    DEFAULT_MEMORY_BUDGET, DumpIndex, LocalFileSource, MapRun, Parallelism, ScanOptions,
    StatisticsRequest, StatisticsSelection, StatisticsTerms, map_file,
};

mod common;
use common::{VERSIONS, sandboxed, statistics_fixture, types_fixture};

struct Counting;

thread_local! {
    /// Bytes this thread has freed.
    static FREED: Cell<u64> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        FREED.set(FREED.get() + layout.size() as u64);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size < layout.size() {
            FREED.set(FREED.get() + (layout.size() - new_size) as u64);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn request(group_size: u64) -> StatisticsRequest {
    StatisticsRequest {
        selection: StatisticsSelection::All,
        group_size: Some(NonZeroU64::new(group_size).unwrap()),
        ..StatisticsRequest::ALL
    }
}

async fn parse(dump: &Path, options: &ScanOptions, statistics: &StatisticsRequest) -> MapRun {
    let source = LocalFileSource::open(dump).unwrap();
    let mode = CacheMode::Enabled(pgdump_query::cache::colocated_path(dump));
    let run = map_file(&source, options, &mode, statistics).await.unwrap();
    assert!(!run.interrupted);
    run
}

/// Every block's statistics, dropped one by one: the bytes each reported, and
/// the bytes its drop freed. Consumes the index so that each `Arc` is the last
/// reference to its statistics.
fn reported_and_freed(mut index: DumpIndex) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    for span in &mut index.spans {
        let SpanBody::Data(DataBlock::Copy(block)) = &mut span.body else { continue };
        let Some(statistics) = block.statistics.take() else { continue };
        assert_eq!(Arc::strong_count(&statistics), 1, "the map is the only holder");
        let reported = statistics.heap_bytes();
        let before = FREED.get();
        drop(statistics);
        out.push((reported, FREED.get() - before));
    }
    out
}

/// Nothing is left gathering once a pass returns: an observer's charge is
/// released into the block it became, or with the observer.
fn assert_only_blocks_are_held(run: &MapRun, label: &str) {
    let now = run.statistics.now;
    let blocks = StatisticsTerms {
        retained: now.retained,
        loaded: now.loaded,
        ..StatisticsTerms::default()
    };
    assert_eq!(now, blocks, "{label}: an observer's charge outlived the pass");
}

/// **A gathered block reports exactly the heap it frees, and so does a loaded
/// one**, over every major's statistics and types fixtures: the first `parse`
/// gathers, the second loads the cache the first wrote and finds nothing
/// lacking, and the third re-reads every block at another group size.
#[tokio::test]
async fn a_block_reports_exactly_the_heap_it_frees() {
    let mut blocks = 0;
    for version in VERSIONS {
        for fixture in [statistics_fixture(version, "default"), types_fixture(version, "default")] {
            let (_dir, dump) = sandboxed(&fixture, "heap.sql");
            let options = ScanOptions::default();

            let gathered = parse(&dump, &options, &request(4096)).await;
            let label = format!("{} gathered", fixture.display());
            assert_only_blocks_are_held(&gathered, &label);
            assert_eq!(gathered.statistics.now.loaded, 0, "{label}");
            let pairs = reported_and_freed(gathered.index);
            let total: u64 = pairs.iter().map(|(reported, _)| reported).sum();
            assert_eq!(gathered.statistics.now.retained, total, "{label}");
            for (reported, freed) in pairs {
                assert_eq!(reported, freed, "{label}");
                blocks += 1;
            }

            let loaded = parse(&dump, &options, &request(4096)).await;
            let label = format!("{} loaded", fixture.display());
            assert_only_blocks_are_held(&loaded, &label);
            assert_eq!(loaded.statistics.now.retained, 0, "{label}: nothing was lacking");
            let pairs = reported_and_freed(loaded.index);
            let total: u64 = pairs.iter().map(|(reported, _)| reported).sum();
            assert_eq!(loaded.statistics.now.loaded, total, "{label}");
            for (reported, freed) in pairs {
                assert_eq!(reported, freed, "{label}");
            }

            let refilled = parse(&dump, &options, &request(8192)).await;
            let label = format!("{} back-filled", fixture.display());
            assert_only_blocks_are_held(&refilled, &label);
            assert_eq!(refilled.statistics.now.loaded, 0, "{label}: every block was replaced");
            assert_eq!(refilled.statistics.term_peaks.loaded, total, "{label}");
            let pairs = reported_and_freed(refilled.index);
            let retained: u64 = pairs.iter().map(|(reported, _)| reported).sum();
            assert_eq!(refilled.statistics.now.retained, retained, "{label}");
        }
    }
    assert!(blocks > 30, "only {blocks} blocks gathered statistics");
}

/// **A split block's pieces are charged while they gather and released when
/// they fold**, the peak seeing them and nothing outliving the pass — the
/// block and chunk `tests/statistics.rs` splits in
/// `a_gathering_scan_is_the_serial_scan_whatever_the_worker_count`.
#[tokio::test]
async fn a_split_blocks_pieces_are_charged_and_released() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("long_block.sql");
    let mut text = String::from(
        "CREATE TABLE public.t (\n    a integer,\n    b text COLLATE pg_catalog.\"C\",\n    c text\n);\n\n",
    );
    text.push_str("COPY public.t (a, b, c) FROM stdin;\n");
    for i in 0..2000 {
        text.push_str(&format!("{i}\t{:05}\tv{}\n", 2000 - i, i % 3));
    }
    text.push_str("\\.\n\nSELECT 1;\n");
    std::fs::write(&dump, &text).unwrap();
    for jobs in [2, 4, 8] {
        let _ = std::fs::remove_file(pgdump_query::cache::colocated_path(&dump));
        let options = ScanOptions {
            chunk_size: 64,
            parallelism: Parallelism::workers(jobs, DEFAULT_MEMORY_BUDGET),
            ..ScanOptions::default()
        };
        let run = parse(&dump, &options, &request(256)).await;
        let label = format!("{jobs} jobs");
        assert_only_blocks_are_held(&run, &label);
        assert!(run.statistics.term_peaks.pieces > 0, "{label}: no piece was charged");
        assert!(run.statistics.term_peaks.interned > 0, "{label}: no dictionary was charged");
        assert!(run.statistics.peak >= run.statistics.now.retained, "{label}");
        let pairs = reported_and_freed(run.index);
        for (reported, freed) in &pairs {
            assert_eq!(reported, freed, "{label}");
        }
        let total: u64 = pairs.iter().map(|(reported, _)| reported).sum();
        assert_eq!(run.statistics.now.retained, total, "{label}");
    }
}
