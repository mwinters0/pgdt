//! What a mapping pass holds for a row many chunks long: none of it where
//! the pass reads no field of it whole — a metadata-level parse, and a
//! data-level or strict one over a `text`, `bytea` or `jsonb` field, its
//! statistics and its check folding the field's runs as they go by — serially
//! and in an interior worker alike, and the field where it is a scalar a
//! parse reads whole, in a mapping of the line's own, outside the allocator
//! (`docs/design/decisions.md`, "D138", "D139"). A census alone holds only
//! the fields it decodes (`pgdt-query`'s `streamed_census.rs`). And what a
//! streamed row gathers and refuses is what the row held whole does.
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
use pgdt_fixtures::{VERSIONS, statistics_fixture, types_fixture};
use pgdt_io::{ByteRangeSource, LocalFileSource};
use pgdt_mapping::map_file;
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::{CopyBlock, DataBlock, DumpIndex, PostgresInvalidValues, SpanBody};
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
    largest_under(dump, jobs, statistics, PostgresInvalidValues::Default).await
}

/// [`largest`], a field PostgreSQL refuses met as `invalid` says.
async fn largest_under(
    dump: &std::path::Path,
    jobs: usize,
    statistics: &StatisticsRequest,
    invalid: PostgresInvalidValues,
) -> (Held, u64) {
    let read = ReadOptions { chunk_size_bytes: CHUNK, ..ReadOptions::default() };
    LARGEST.store(0, Ordering::Relaxed);
    // `5` resets the peak resident set to the current one (`proc(5)`).
    std::fs::write("/proc/self/clear_refs", "5").unwrap();
    let before = status_bytes("VmRSS:");
    let index = mapped(dump, read, jobs, statistics, invalid).await.unwrap();
    let held = Held {
        allocation: LARGEST.load(Ordering::Relaxed),
        resident: status_bytes("VmHWM:").saturating_sub(before),
    };
    let rows = index
        .spans
        .iter()
        .find_map(|span| match &span.body {
            SpanBody::Data(DataBlock::Copy(block)) => Some(block.row_count),
            _ => None,
        })
        .unwrap();
    (held, rows)
}

/// A cacheless `map_file` of `dump` in `read`'s chunks at `jobs`.
async fn mapped(
    dump: &std::path::Path,
    read: ReadOptions,
    jobs: usize,
    statistics: &StatisticsRequest,
    invalid: PostgresInvalidValues,
) -> pgdt_core::Result<DumpIndex> {
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(dump).unwrap());
    let opened = CacheMode::DISABLED.open_source(source).await.unwrap();
    let cancel = CancellationToken::new();
    let run = map_file(opened, read, &arranged(jobs), invalid, statistics, &cancel).await?;
    Ok(run.index)
}

/// A dump of `public.t (a integer, b <declared>)` whose 1001st row's `b` is
/// `field`, as written to disk, among a thousand and one short rows whose `b`
/// is `short`.
fn dump_holding(
    dir: &std::path::Path,
    declared: &str,
    short: &str,
    field: &[u8],
) -> std::path::PathBuf {
    let path = dir.join("long.sql");
    let mut file = format!(
        "CREATE TABLE public.t (\n    a integer,\n    b {declared}\n);\n\n\
         COPY public.t (a, b) FROM stdin;\n"
    )
    .into_bytes();
    for i in 0..1000 {
        file.extend_from_slice(format!("{i}\t{short}\n").as_bytes());
    }
    file.extend_from_slice(b"1000\t");
    file.extend_from_slice(field);
    file.extend_from_slice(format!("\n1001\t{short}\n\\.\n\n").as_bytes());
    std::fs::write(&path, &file).unwrap();
    path
}

/// **A metadata-level parse holds no row it counts**: one of a row
/// sixteen MiB long among short ones holds nothing near the row, serially
/// and at four workers, where a data-level parse summing its `bigint` and a
/// strict one keying it hold it whole — and allocate nothing near it either,
/// the line past its chunk held in a mapping of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_metadata_level_parse_streams_a_long_row() {
    let dir = tempfile::tempdir().unwrap();
    let mut field = vec![b'0'; LONG];
    *field.last_mut().unwrap() = b'1';
    let dump = dump_holding(dir.path(), "bigint", "7", &field);
    drop(field);
    let strict = PostgresInvalidValues::Strict;
    let (held, rows) = largest(&dump, 1, &StatisticsRequest::DATA).await;
    assert_eq!(rows, 1002);
    for (leg, read) in [
        ("summed", &held),
        ("keyed", &largest_under(&dump, 1, &StatisticsRequest::METADATA, strict).await.0),
    ] {
        // A resident set read against the one before, which what the pass
        // frees lowers: three quarters of the row, past anything else the
        // pass holds.
        let resident = read.resident;
        assert!(resident > LONG / 4 * 3, "a {leg} field is held: {resident} bytes");
        let allocation = read.allocation;
        assert!(
            allocation < LONG / 8,
            "the {leg} row held in the allocator: a {allocation}-byte allocation"
        );
    }
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

/// **A gathering parse holds no line of the types a dump's long fields
/// hold**: a row whose one long field is a `text` with an escape every few
/// bytes, a `bytea` or a `jsonb`, sixteen and sixty-four MiB long, read by a
/// data-level and a strict parse, serially and at four workers, holds under
/// a quarter of the shorter row resident and allocates nothing near it — flat
/// in the line's length — its statistics and its check folding the field as
/// it goes by.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gathering_parse_streams_the_types_long_fields_hold() {
    let dir = tempfile::tempdir().unwrap();
    // Each declared type, its short rows' field, and its long field of a length.
    type Long = fn(usize) -> Vec<u8>;
    let fields: [(&str, &str, Long); 3] = [
        ("text", "short", |len| b"abc\\tdef".iter().copied().cycle().take(len).collect()),
        ("bytea", "\\\\x00", |len| {
            let mut field = b"\\\\x".to_vec();
            field.extend(b"0a".iter().copied().cycle().take(len / 2 * 2));
            field
        }),
        ("jsonb", "[1]", |len| {
            let mut field = b"[1".to_vec();
            field.extend(b", 2".iter().copied().cycle().take(len / 3 * 3));
            field.push(b']');
            field
        }),
    ];
    for (declared, short, field) in fields {
        for len in [LONG, LONG * 4] {
            let dump = dump_holding(dir.path(), declared, short, &field(len));
            for invalid in [PostgresInvalidValues::Default, PostgresInvalidValues::Strict] {
                for jobs in [1usize, 4] {
                    let at = format!("{declared}, {len} bytes, {invalid:?}, {jobs} jobs");
                    let (read, rows) =
                        largest_under(&dump, jobs, &StatisticsRequest::DATA, invalid).await;
                    assert_eq!(rows, 1002, "{at}");
                    let (resident, allocation) = (read.resident, read.allocation);
                    assert!(resident < LONG / 4, "{at}: {resident} bytes resident");
                    assert!(allocation < LONG / 8, "{at}: a {allocation}-byte allocation");
                }
            }
        }
    }
}

/// The blocks `dump` maps to, or the error mapping it fails with, read in
/// `chunk`-byte reads at `jobs` under `invalid`, at the data level.
async fn outcome(
    dump: &std::path::Path,
    chunk: usize,
    jobs: usize,
    invalid: PostgresInvalidValues,
) -> Result<Vec<CopyBlock>, String> {
    let read = ReadOptions { chunk_size_bytes: chunk, ..ReadOptions::default() };
    let index = mapped(dump, read, jobs, &StatisticsRequest::DATA, invalid).await;
    index.map(|index| index.blocks().cloned().collect()).map_err(|e| e.to_string())
}

/// **A streamed row gathers and checks what it does held whole**: every
/// block of every major's `types` and `statistics` fixture, read in reads so
/// short that nearly every row outgrows its chunk, serially and at four
/// workers, by a default, an ignoring and a strict parse, maps to the blocks
/// a parse holding every row maps it to — statistics, census, what it went
/// past and whether it was checked in full.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streamed_row_gathers_and_checks_what_a_held_one_does() {
    let invalids = [
        PostgresInvalidValues::Default,
        PostgresInvalidValues::Ignore,
        PostgresInvalidValues::Strict,
    ];
    for version in VERSIONS {
        for (fixture, chunks) in [
            (types_fixture(version, "default"), &[16usize, 61][..]),
            (statistics_fixture(version, "default"), &[61][..]),
        ] {
            for invalid in invalids {
                let held = outcome(&fixture, 1 << 20, 1, invalid).await;
                let blocks = held.as_ref().map_or(0, Vec::len);
                assert!(blocks > 0, "{version}: {invalid:?} maps {fixture:?}: {held:?}");
                for &chunk in chunks {
                    for jobs in [1usize, 4] {
                        let streamed = outcome(&fixture, chunk, jobs, invalid).await;
                        assert!(
                            streamed == held,
                            "{version}: {fixture:?}, {invalid:?}, chunk {chunk}, {jobs} jobs",
                        );
                    }
                }
            }
        }
    }
}

/// **A long field PostgreSQL refuses is refused, or gone past, as it is
/// held whole**: a `text` past its typmod, a `bytea` and a `jsonb` that do
/// not read, and a `smallint[]` element past its type, each the long field of
/// a row a 64 KiB read streams, refused by a default and a strict parse as
/// read in one read, and counted alike by an ignoring one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streamed_field_is_refused_as_a_held_one_is() {
    let dir = tempfile::tempdir().unwrap();
    let long = 1usize << 20;
    let fields: [(&str, Vec<u8>); 4] = [
        ("character varying(1000)", b"ab\\tc".iter().copied().cycle().take(long).collect()),
        ("bytea", {
            let mut field = b"\\\\x".to_vec();
            field.extend(b"0a".iter().copied().cycle().take(long));
            field.extend(b"zz");
            field
        }),
        ("jsonb", {
            let mut field = b"[1".to_vec();
            field.extend(b", 2".iter().copied().cycle().take(long));
            field.extend(b",]");
            field
        }),
        ("smallint[]", {
            let mut field = b"{1".to_vec();
            field.extend(b",2".iter().copied().cycle().take(long));
            field.extend(b",99999}");
            field
        }),
    ];
    for (declared, field) in fields {
        let dump = dump_holding(dir.path(), declared, "\\N", &field);
        for invalid in [
            PostgresInvalidValues::Default,
            PostgresInvalidValues::Ignore,
            PostgresInvalidValues::Strict,
        ] {
            let held = outcome(&dump, 4 << 20, 1, invalid).await;
            if invalid == PostgresInvalidValues::Strict {
                assert!(held.is_err(), "{declared}: a strict parse refuses it");
            }
            for jobs in [1usize, 4] {
                let streamed = outcome(&dump, CHUNK, jobs, invalid).await;
                assert!(streamed == held, "{declared}, {invalid:?}, {jobs} jobs: {streamed:?}");
            }
        }
    }
}
