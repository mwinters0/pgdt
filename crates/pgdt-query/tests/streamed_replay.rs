//! What a replay holds for a row many chunks long that nothing it asks of
//! the row reads a field of — no column projected, no filter term: none of
//! it, the row streamed and its fields counted as they go by
//! (`pgdt_dump::ChunkEvents::stream_rows`), where a replay reading a field
//! holds the row whole, in a mapping of the line's own
//! (`docs/design/decisions.md`, "D138" and "D139"). A streamed row is held to
//! its block's width as a held one is (D28).
//!
//! **This binary reads the process's peak resident set**, reset before each
//! replay (`/proc/self/clear_refs`), which is why it is a test file of its
//! own: a held line is resident at its length, and nothing else a replay
//! over this file holds comes near it. Linux only, where both are.

#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use pgdt_dump::ReadOptions;
use pgdt_io::{ByteRangeSource, LocalFileSource};
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_query::{Dump, Expr, Predicate, PredicateOp, QueryOptions};

/// The long row's length: past every buffer a replay sizes from its chunk.
const LONG: usize = 16 << 20;
/// The chunk every replay here reads in.
const CHUNK: usize = 64 << 10;

/// A field of `/proc/self/status`, in bytes.
fn status_bytes(field: &str) -> usize {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find(|line| line.starts_with(field)).unwrap();
    let kib: usize = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    kib << 10
}

/// A dump of `public.t (a integer, b text)`, a thousand short rows and then
/// `long`, a row written as it stands, and a short one after it.
fn dump_holding(dir: &Path, long: &[u8]) -> PathBuf {
    let path = dir.join("long.sql");
    let mut file = b"CREATE TABLE public.t (\n    a integer,\n    b text\n);\n\n\
                     COPY public.t (a, b) FROM stdin;\n"
        .to_vec();
    for i in 0..1000 {
        file.extend_from_slice(format!("{i}\tshort\n").as_bytes());
    }
    file.extend_from_slice(long);
    file.extend_from_slice(b"\n1001\tshort\n\\.\n\n");
    std::fs::write(&path, file).unwrap();
    path
}

/// `path` mapped at the data level, holding no cache.
async fn mapped(path: &Path) -> Dump {
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(path).unwrap());
    common::mapped(source, &StatisticsRequest::DATA).await
}

/// What a serial replay of `public.t` in `dump`, projecting `projection`
/// under `filter`, in `chunk`-byte reads, answers: the rows it emitted, or
/// its error; and how far its peak resident set rose past the resident set
/// it began with.
async fn replay(
    dump: &Dump,
    chunk: usize,
    projection: &[&str],
    filter: Expr,
) -> (Result<usize, String>, usize) {
    let table = dump.table(&common::selector("public.t")).unwrap().unwrap();
    let options = QueryOptions {
        projection: Some(projection.iter().map(|column| column.to_string()).collect()),
        filter,
        ..QueryOptions::default()
    };
    let read = ReadOptions { chunk_size_bytes: chunk, ..ReadOptions::default() };
    let plan = table.plan(read, options, &pgdt_fixtures::arranged(1)).unwrap();
    // `5` resets the peak resident set to the current one (`proc(5)`).
    std::fs::write("/proc/self/clear_refs", "5").unwrap();
    let before = status_bytes("VmRSS:");
    let mut stream = plan.in_file_order(8192);
    let mut rows = Ok(0);
    while let Some(batch) = stream.next().await {
        match batch {
            Ok(batch) => rows = rows.map(|rows| rows + batch.num_rows()),
            Err(error) => {
                rows = Err(format!("{error:?}"));
                break;
            }
        }
    }
    drop(stream);
    (rows, status_bytes("VmHWM:").saturating_sub(before))
}

/// `column = value`.
fn equals(column: &str, value: &str) -> Expr {
    Expr::Term(Predicate { column: column.into(), op: PredicateOp::Eq(value.into()) })
}

/// **A replay reading no field holds no row it counts**: a row sixteen MiB
/// long among short ones is counted with nothing near it resident, where a
/// replay projecting its long column, or filtering on its short one, holds
/// it whole.
#[tokio::test]
async fn a_replay_reading_no_field_streams_a_long_row() {
    let dir = tempfile::tempdir().unwrap();
    let mut long = b"1000\t".to_vec();
    long.resize(long.len() + LONG, b'x');
    let dump = mapped(&dump_holding(dir.path(), &long)).await;
    drop(long);

    let (rows, streamed) = replay(&dump, CHUNK, &[], Expr::default()).await;
    assert_eq!(rows, Ok(1002));
    for (projection, filter, want) in
        [(&["b"][..], Expr::default(), 1002), (&[][..], equals("a", "1000"), 1)]
    {
        let (rows, held) = replay(&dump, CHUNK, projection, filter.clone()).await;
        assert_eq!(rows, Ok(want), "{projection:?} under {filter:?}");
        // A resident set read against the one before, which what the replay
        // frees lowers: three quarters of the row, past anything else it holds.
        assert!(held > LONG / 4 * 3, "{projection:?} under {filter:?}: {held} bytes resident");
        assert!(
            streamed + LONG / 2 < held,
            "{projection:?} under {filter:?}: {streamed} bytes resident beside a held row's {held}"
        );
    }
}

/// **A streamed row is held to its block's width as a held one is**: a row
/// one field wider or narrower than its block, many chunks long, refuses a
/// replay reading no field of it with the error a replay holding it whole —
/// the whole file one chunk — raises.
#[tokio::test]
async fn a_streamed_row_is_refused_for_its_width_as_a_held_one_is() {
    let dir = tempfile::tempdir().unwrap();
    let value = "x".repeat(4096);
    for (long, found) in [(format!("1000\t{value}\t{value}"), 3), (format!("1000{value}"), 1)] {
        let dump = mapped(&dump_holding(dir.path(), long.as_bytes())).await;
        let (streamed, _) = replay(&dump, 64, &[], Expr::default()).await;
        let (held, _) = replay(&dump, 1 << 20, &[], Expr::default()).await;
        let refused = streamed.expect_err("a row of the wrong width is read");
        assert_eq!(Err(refused.clone()), held, "{found} fields");
        assert!(refused.contains("ColumnCountMismatch"), "{refused}");
        assert!(refused.contains(&format!("found: {found}")), "{refused}");
    }
}
