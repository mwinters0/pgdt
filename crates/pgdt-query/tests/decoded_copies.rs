//! What decoding one long value holds beside its line: one decoded copy, in
//! a query over escaped text and over `bytea` (`docs/design/decisions.md`,
//! "D141"), and none in a parse, its statistics and a strict parse's check
//! reading an escaped field off its bytes ("D142").
//!
//! **This binary installs a global allocator tracking the bytes live**, and
//! their peak, which is why it is a test file of its own. A line past its
//! chunk is held in a mapping of its own, outside the allocator (D139), so
//! what a window's peak rises by is the decode's copies and the little a pass
//! allocates beside them. Linux only, where that mapping is.

#![cfg(target_os = "linux")]

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use arrow::array::{AsArray, RecordBatch};
use common::Cold;
use futures::StreamExt;
use pgdt_dump::cache::CacheMode;
use pgdt_dump::{ReadOptions, ScanOptions};
use pgdt_io::{ByteRangeSource, LocalFileSource};
use pgdt_mapping::map_file;
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::PostgresInvalidValues;
use pgdt_query::QueryOptions;
use tokio_util::sync::CancellationToken;

struct Live;

/// The bytes allocated and not yet freed.
static LIVE: AtomicUsize = AtomicUsize::new(0);
/// The most [`LIVE`] has been since the last [`window`] opened.
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(by: usize) {
    let live = LIVE.fetch_add(by, Ordering::Relaxed) + by;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Live {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        grew(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    // A grow is counted at both sizes at once, as the copy it may make holds
    // both.
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        grew(new_size);
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Live = Live;

/// The chunk every pass here reads in, far below each long field.
const CHUNK: usize = 64 << 10;

/// Open a window: the peak is reset to what is live, which is returned.
fn window() -> usize {
    let live = LIVE.load(Ordering::Relaxed);
    PEAK.store(live, Ordering::Relaxed);
    live
}

/// A dump of `public.t (a integer, b <declared>)` whose second row's `b` is
/// `field`, as written to disk, between two short rows holding `short`.
fn dump(dir: &Path, declared: &str, short: &str, field: &[u8]) -> std::path::PathBuf {
    let path = dir.join("long.sql");
    let mut file = format!(
        "CREATE TABLE public.t (\n    a integer,\n    b {declared}\n);\n\n\
         COPY public.t (a, b) FROM stdin;\n1\t{short}\n2\t"
    )
    .into_bytes();
    file.extend_from_slice(field);
    file.extend_from_slice(format!("\n3\t{short}\n\\.\n\n").as_bytes());
    std::fs::write(&path, file).unwrap();
    path
}

fn source(path: &Path) -> Arc<dyn ByteRangeSource> {
    Arc::new(LocalFileSource::open(path).unwrap())
}

/// The batches a cold query of `public.t` over `path` reads, and how far
/// the allocator's peak rose past what was live as it began. Its span cap is
/// below the long line, so the line ends its batch, as one past the default
/// cap does: what a `bytea` batch's one buffer copies as it grows past a
/// value is the span's, which the replay is charged (`decisions.md`, "D84").
async fn query(path: &Path) -> (Vec<RecordBatch>, usize) {
    let scan = ScanOptions {
        read: ReadOptions { chunk_size_bytes: CHUNK, ..ReadOptions::default() },
        ..ScanOptions::default()
    };
    let cold = Cold { scan, ..Cold::default() };
    let options = QueryOptions { max_source_span: Some(1 << 20), ..QueryOptions::default() };
    let before = window();
    let mut stream = cold.stream(source(path), "public.t", options).await.unwrap().unwrap();
    let mut batches = Vec::new();
    while let Some(batch) = stream.next().await {
        batches.push(batch.unwrap());
    }
    drop(stream);
    (batches, PEAK.load(Ordering::Relaxed) - before)
}

/// **An escaped text is held once beside its line**: a value of sixteen MiB
/// carrying an escape every few bytes is its column's block as its decode
/// wrote it, never copied into a block of the builder's.
#[tokio::test]
async fn an_escaped_text_is_one_copy() {
    let dir = tempfile::tempdir().unwrap();
    let long = 16 << 20;
    let value = "abcde\t".repeat(long / 6);
    let field = value.replace('\t', "\\t");
    let path = dump(dir.path(), "text", "short", field.as_bytes());

    let (batches, peak) = query(&path).await;
    let column = batches[0].column(1).as_string_view();
    assert_eq!(column.value(1), value);
    assert!(peak < field.len() + long / 4, "{peak} bytes allocated at the peak");
}

/// **A `bytea` is decoded into its column, nothing beside it**: a hex value
/// as `pg_dump` writes it, and one whose every digit `COPY` spells as an
/// escape of its own, each hold the value and little more, never its
/// unescaped text.
#[tokio::test]
async fn a_bytea_is_one_copy() {
    let dir = tempfile::tempdir().unwrap();
    for (bytes, spell) in [(8usize << 20, false), (2 << 20, true)] {
        let value: Vec<u8> = (0..bytes).map(|i| (i % 251) as u8).collect();
        let mut field = b"\\\\x".to_vec();
        for byte in &value {
            let pair = format!("{byte:02x}");
            if spell {
                field.extend(pair.bytes().flat_map(|digit| format!("\\x{digit:02x}").into_bytes()));
            } else {
                field.extend_from_slice(pair.as_bytes());
            }
        }
        let path = dump(dir.path(), "bytea", "\\\\x00", &field);

        let (batches, peak) = query(&path).await;
        let column = batches[0].column(1).as_binary::<i32>();
        assert_eq!(column.value(1), value, "spelled: {spell}");
        assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 3);
        assert!(peak < bytes + bytes / 4, "spelled: {spell}: {peak} bytes allocated at the peak");
    }
}

/// How far the allocator's peak rose past what was live as a whole-file
/// parse of `path` began, gathering statistics at the data level and
/// reading values as `invalid` says, or the parse's error.
async fn parse(path: &Path, invalid: PostgresInvalidValues) -> pgdt_core::Result<usize> {
    let opened = CacheMode::DISABLED.open_source(source(path)).await.unwrap();
    let read = ReadOptions { chunk_size_bytes: CHUNK, ..ReadOptions::default() };
    let before = window();
    let run = map_file(
        opened,
        read,
        &common::arranged(1),
        invalid,
        &StatisticsRequest::DATA,
        &CancellationToken::new(),
    )
    .await?;
    let peak = PEAK.load(Ordering::Relaxed) - before;
    assert!(run.index.is_complete(std::fs::metadata(path).unwrap().len()));
    Ok(peak)
}

/// **A strict parse checks a `bytea` holding none of it**: a field of
/// sixteen MiB is checked, and its statistics gathered, with no unescaped
/// text or decoded value of it allocated, the line past its chunk outside
/// the allocator.
#[tokio::test]
async fn a_strict_bytea_check_holds_no_copy() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = 8usize << 20;
    let mut field = b"\\\\x".to_vec();
    for i in 0..bytes {
        field.extend_from_slice(format!("{:02x}", i % 251).as_bytes());
    }
    let path = dump(dir.path(), "bytea", "\\\\x00", &field);

    let peak = parse(&path, PostgresInvalidValues::Strict).await.unwrap();
    assert!(peak < bytes / 4, "{peak} bytes allocated at the peak");
}

/// **A parse holds no copy of an escaped text**: a value of eight MiB
/// carrying an escape every few bytes is gathered, and checked against its
/// typmod by a strict parse, with none of it allocated; one past its typmod
/// is refused all the same.
#[tokio::test]
async fn a_parse_holds_no_copy_of_an_escaped_text() {
    let dir = tempfile::tempdir().unwrap();
    // `character varying`'s longest typmod, in characters.
    let typmod = 10_485_760usize;
    let escaped = |chars: usize| "abcde\\t".repeat(chars / 6);
    for declared in ["text".to_string(), format!("character varying({typmod})")] {
        let long = 8usize << 20;
        let path = dump(dir.path(), &declared, "short", escaped(long).as_bytes());
        for invalid in [PostgresInvalidValues::Default, PostgresInvalidValues::Strict] {
            let peak = parse(&path, invalid).await.unwrap();
            assert!(peak < long / 4, "{declared}, {invalid:?}: {peak} bytes allocated at the peak");
        }
    }
    let past = escaped(typmod + (1 << 20));
    let path = dump(dir.path(), &format!("character varying({typmod})"), "short", past.as_bytes());
    let refused = parse(&path, PostgresInvalidValues::Strict).await.unwrap_err();
    assert!(matches!(refused, pgdt_core::Error::FieldRefused { .. }), "{refused:?}");
}
