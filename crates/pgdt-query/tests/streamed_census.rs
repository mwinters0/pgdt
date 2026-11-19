//! What a cold query's mapping pass holds of a row many chunks long in a
//! block it censuses: none of it where no field it counts is a container,
//! the row streamed and its fields folded into the census as they go by —
//! a counted scalar from a head — and the container's field alone where one
//! is counted, in a mapping of its own (`docs/design/decisions.md`, "D138",
//! "D139"); and that what a streamed census records is what a census holding
//! every row records.
//!
//! **This binary reads the process's peak resident set**, reset before each
//! query (`/proc/self/clear_refs`), which is why it is a test file of its
//! own: a held line is resident at its length, and nothing else a query over
//! these files holds comes near it. Linux only, where both are.

#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use pgdt_dump::cache::{CacheMode, CacheStatus};
use pgdt_dump::internal::load;
use pgdt_dump::{ReadOptions, ScanOptions};
use pgdt_io::{ByteRangeSource, LocalFileSource, Parallelism};
use pgdt_mapping::build_index;
use pgdt_model::{BlockCensus, DumpIndex};
use pgdt_query::{QueryOptions, ScanExtent};

use common::{Cold, VERSIONS, types_fixture};

/// The long row's length: past every buffer a pass sizes from its chunk.
const LONG: usize = 16 << 20;
/// The chunk the long-row queries read in.
const CHUNK: usize = 64 << 10;

/// A field of `/proc/self/status`, in bytes.
fn status_bytes(field: &str) -> usize {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find(|line| line.starts_with(field)).unwrap();
    let kib: usize = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    kib << 10
}

/// A dump of `public.t (a text, b <b_type>)` — a thousand short rows, then
/// `long`, a row written as it stands — and after it `public.s (a integer)`,
/// the table queried, so the mapping pass censuses `t` on its way.
fn dump_holding(dir: &Path, b_type: &str, long: &[u8]) -> PathBuf {
    let path = dir.join(format!("long-{}.sql", b_type.replace(['[', ']'], "_")));
    let mut file = format!(
        "CREATE TABLE public.t (\n    a text,\n    b {b_type}\n);\n\n\
         CREATE TABLE public.s (\n    a integer\n);\n\n\
         COPY public.t (a, b) FROM stdin;\n"
    )
    .into_bytes();
    for i in 0..1000 {
        file.extend_from_slice(format!("{i}\t\\N\n").as_bytes());
    }
    file.extend_from_slice(long);
    file.extend_from_slice(b"\n\\.\n\nCOPY public.s (a) FROM stdin;\n1\n2\n\\.\n\n");
    std::fs::write(&path, file).unwrap();
    path
}

/// What a cold query of `public.s` in `path`, its mapping pass at `jobs`
/// in `CHUNK`-byte reads, saving its map to `cache`, answers: the rows it
/// emitted; how far its peak resident set rose past the resident set it
/// began with; and the census its pass recorded of `public.t`.
async fn cold_query(path: &Path, jobs: usize, cache: &Path) -> (usize, usize, BlockCensus) {
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(path).unwrap());
    let read = ReadOptions::default().with_chunk_size(CHUNK);
    let scan = ScanOptions { read, parallelism: Parallelism::workers(jobs, 1 << 30) };
    let cold = Cold { scan, cache: CacheMode::enabled(cache), ..Cold::default() };
    // `5` resets the peak resident set to the current one (`proc(5)`).
    std::fs::write("/proc/self/clear_refs", "5").unwrap();
    let before = status_bytes("VmRSS:");
    let stream = cold.stream(Arc::clone(&source), "public.s", QueryOptions::default());
    let mut stream = stream.await.unwrap().unwrap();
    let mut rows = 0;
    while let Some(batch) = stream.next().await {
        rows += batch.unwrap().num_rows();
    }
    drop(stream);
    let resident = status_bytes("VmHWM:").saturating_sub(before);
    let index = loaded(cache, source.as_ref()).await;
    (rows, resident, census(&index, "public.t"))
}

/// The map a query saved to `cache`.
async fn loaded(cache: &Path, source: &dyn ByteRangeSource) -> DumpIndex {
    let CacheStatus::Loaded(loaded) = load(cache, source).await.unwrap() else {
        panic!("the query wrote a cache")
    };
    loaded.index
}

/// The census `index` holds of `table`'s first block.
fn census(index: &DumpIndex, table: &str) -> BlockCensus {
    let block = index.blocks().find(|block| block.header.matches(table)).unwrap();
    block.census.clone().expect("a query censuses every block it maps")
}

/// **A cold query's census holds only the fields it decodes**: a row
/// sixteen MiB long in a table the query does not read is censused with
/// nothing near it resident, serially and at four workers, where its long
/// field is text, which nothing counts, or where it is a `date` read from
/// its head — counted past the calendar — and held whole where its long
/// field is a `date[]`, a counted container; and each census is the one
/// a pass holding the row records.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cold_querys_census_holds_only_the_fields_it_decodes() {
    let dir = tempfile::tempdir().unwrap();
    let mut text = b"1000\t".to_vec();
    text.resize(text.len() + LONG, b'x');
    let mut date = vec![b'x'; LONG];
    date.extend_from_slice(b"\t262143-01-01");
    let mut dates = b"1000\t{".to_vec();
    while dates.len() < LONG {
        dates.extend_from_slice(b"2020-01-01,");
    }
    dates.extend_from_slice(b"262143-01-01}");
    let cases = [("text", &text, false), ("date", &date, false), ("date[]", &dates, true)];
    let mut streamed_resident = Vec::new();
    for (b_type, long, holds) in cases {
        let path = dump_holding(dir.path(), b_type, long);
        let source = LocalFileSource::open(&path).unwrap();
        let reference = build_index(&source, &ScanOptions::default()).await.unwrap();
        let want = census(&reference, "public.t");
        for jobs in [1usize, 4] {
            let cache = dir.path().join(format!("{jobs}.pgdt"));
            let _ = std::fs::remove_file(&cache);
            let (rows, resident, got) = cold_query(&path, jobs, &cache).await;
            let context = format!("{b_type} at {jobs} jobs");
            assert_eq!(rows, 2, "{context}");
            assert_eq!(got, want, "{context}");
            if holds {
                // Three quarters of the field, past anything else it holds.
                assert!(resident > LONG / 4 * 3, "{context}: {resident} bytes resident");
            } else {
                streamed_resident.push((context, resident));
            }
        }
        if b_type != "text" {
            let counted = want.unrepresentable[1];
            assert_eq!((counted.format(), counted.engine()), (0, 1), "{b_type}: counted");
        }
    }
    for (context, resident) in streamed_resident {
        assert!(resident < LONG / 4, "{context}: {resident} bytes resident");
    }
}

/// **A streamed census is the census held**: every block of every major's
/// `types` fixture, mapped by a cold query in reads so short that nearly
/// every row outgrows its chunk, serially and at four workers, is censused
/// and counted as the whole-file pass holding every row censuses it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streamed_census_is_the_census_held() {
    for version in VERSIONS {
        let path = types_fixture(version, "default");
        let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&path).unwrap());
        let reference = build_index(source.as_ref(), &ScanOptions::default()).await.unwrap();
        for (chunk, jobs) in [(16usize, 1usize), (61, 1), (16, 4), (61, 4)] {
            let dir = tempfile::tempdir().unwrap();
            let cache = dir.path().join("dump.pgdt");
            let read = ReadOptions::default().with_chunk_size(chunk);
            let scan = ScanOptions { read, parallelism: Parallelism::workers(jobs, 1 << 30) };
            let cold = Cold {
                scan,
                extent: ScanExtent::Full,
                cache: CacheMode::enabled(&cache),
                ..Cold::default()
            };
            let options = QueryOptions::default();
            let stream = cold.stream(Arc::clone(&source), "public.t_array", options);
            let mut stream = stream.await.unwrap().unwrap();
            while let Some(batch) = stream.next().await {
                batch.unwrap();
            }
            drop(stream);
            let index = loaded(&cache, source.as_ref()).await;
            let blocks = reference.blocks().zip(index.blocks());
            for (want, got) in blocks {
                let table = want.header.qualified_name();
                let context = format!("{version}: {table}, chunk {chunk}, {jobs} jobs");
                assert_eq!(got.header, want.header, "{context}");
                assert_eq!(got.census, want.census, "{context}");
            }
            assert_eq!(index.blocks().count(), reference.blocks().count(), "{version}");
        }
    }
}
