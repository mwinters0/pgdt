//! **Every path that reads a line holds it to the bound exactly**
//! (`docs/design/decisions.md`, "D164"): a row whose line, its LF included,
//! is the bound is read, and one a byte longer refused at its start naming
//! its table — the serial mapping pass, the leader's split, an `.xz` and a
//! gzip source, a pass resumed from an interrupted cache, and a query's
//! replay, serial and cut into segments.
//!
//! Each is read at a small bound (`pgdt_dump::internal::with_line_bound`) so a
//! dump of a few kilobytes reaches it; the real bound's wiring is `pgdt`'s
//! `tests/line_bound.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use futures::StreamExt;
use pgdt_core::Error;
use pgdt_dump::ReadOptions;
use pgdt_dump::cache::{CacheMode, colocated_path};
use pgdt_dump::internal::with_line_bound;
use pgdt_io::{ByteRangeSource, LocalFileSource, Parallelism};
use pgdt_mapping::map_file;
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::PostgresInvalidValues;
use pgdt_query::{Dump, QueryOptions};
use tokio_util::sync::CancellationToken;

mod common;
use common::{arranged, arranged_as, mapped, rows_of};

/// The bound every path here reads at.
const BOUND: usize = 300;

/// The line ending `public.u`'s block.
const U_END: &str = "\\.\n";

/// A source whose every read at or past `fail_at` fails, as a pass dying
/// there finds it.
struct FailsPast {
    inner: Arc<dyn ByteRangeSource>,
    fail_at: u64,
}

impl ByteRangeSource for FailsPast {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = pgdt_core::Result<bytes::Bytes>> + Send + '_>,
    > {
        Box::pin(async move {
            if offset >= self.fail_at {
                return Err(Error::Io(std::io::Error::other("simulated interruption")));
            }
            self.inner.read_range(offset, len).await
        })
    }

    fn extent(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = pgdt_core::Result<pgdt_io::Extent>> + Send + '_>,
    > {
        self.inner.extent()
    }

    fn modified(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = pgdt_core::Result<Option<std::time::SystemTime>>>
                + Send
                + '_,
        >,
    > {
        self.inner.modified()
    }
}

/// A dump whose `public.t` holds a row whose line is `line` bytes, its LF
/// included, and where that row starts: where `ahead`, `COPY public.u`'s
/// block of short rows before `public.t`'s header, and `public.t`'s rows
/// before the long one.
///
/// A split pass's pieces past a block's terminator scan what follows it
/// (`KD134`), so a long row right after a block fails that block's region,
/// naming no table: only the resumed pass, serial, reads one ahead.
fn dump(dir: &Path, line: usize, ahead: bool) -> (PathBuf, u64) {
    let mut text = String::new();
    if ahead {
        text.push_str("COPY public.u (id) FROM stdin;\n");
        for id in 0..64 {
            text.push_str(&format!("{id}\n"));
        }
        text.push_str(U_END);
        text.push('\n');
    }
    text.push_str("COPY public.t (id, v) FROM stdin;\n");
    for id in 0..32 {
        text.push_str(&format!("{id}\tshort\n"));
    }
    let start = text.len() as u64;
    // `99\t`, the value and its LF.
    text.push_str(&format!("99\t{}\n", "x".repeat(line - 4)));
    for id in 100..132 {
        text.push_str(&format!("{id}\tshort\n"));
    }
    text.push_str("\\.\n");
    let path = dir.join(format!("line-{line}.sql"));
    std::fs::write(&path, text).unwrap();
    (path, start)
}

/// `bound`'s reads, `chunk` bytes apiece.
fn read(chunk: usize, bound: usize) -> ReadOptions {
    with_line_bound(ReadOptions::default().with_chunk_size(chunk), bound)
}

/// `result` is what a path answers of the dump whose long line is `line`:
/// read where that is the bound, and refused at its start naming
/// `public.t` where it is a byte longer.
fn held_to_the_bound<T: std::fmt::Debug>(
    result: pgdt_core::Result<T>,
    line: usize,
    start: u64,
    path: &str,
) {
    match (result, line > BOUND) {
        (Ok(_), false) => {}
        (Err(Error::LineTooLong { offset, table, bound: BOUND }), true) => {
            assert_eq!(offset, start, "{path} at a {line}-byte line");
            assert_eq!(table.as_deref(), Some("public.t"), "{path} at a {line}-byte line");
        }
        (other, _) => panic!("{path} at a {line}-byte line: {other:?}"),
    }
}

/// `source` mapped whole at `read` by `jobs` workers, writing no cache.
async fn map(
    source: Arc<dyn ByteRangeSource>,
    read: ReadOptions,
    jobs: usize,
) -> pgdt_core::Result<()> {
    let opened = CacheMode::DISABLED.open_source(source).await?;
    let token = CancellationToken::new();
    let invalid = PostgresInvalidValues::Default;
    map_file(opened, read, &arranged(jobs), invalid, &StatisticsRequest::DATA, &token).await?;
    Ok(())
}

/// `path` compressed by `xz` in blocks of 512 bytes, beside it.
fn xz(path: &Path) -> PathBuf {
    let status = Command::new("xz")
        .args(["--block-size=512", "-k", "-f"])
        .arg(path)
        .status()
        .expect("`xz` is not runnable, so this test cannot build its fixture; install it.");
    assert!(status.success());
    path.with_extension("sql.xz")
}

/// `path` gzipped beside it, a deflate block ended every 256 bytes.
fn gzip(path: &Path) -> PathBuf {
    use std::io::Write as _;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    for piece in std::fs::read(path).unwrap().chunks(256) {
        encoder.write_all(piece).unwrap();
        encoder.flush().unwrap();
    }
    let gz = path.with_extension("sql.gz");
    std::fs::write(&gz, encoder.finish().unwrap()).unwrap();
    gz
}

/// `path` opened as `pgdt` opens it: an `.xz` file as itself, and a gzip
/// one over its first pass.
async fn opened(path: &Path) -> Arc<dyn ByteRangeSource> {
    let origin = pgdt_io::Origin::local(path);
    let spill = path.parent().unwrap();
    match pgdt_io::open(&origin, pgdt_io::KnownCompression::Unknown, spill).await.unwrap() {
        pgdt_io::Recognized::Source(source) => source,
        pgdt_io::Recognized::Mismatch => unreachable!("claiming nothing is never a mismatch"),
    }
}

/// **A mapping pass holds a line to the bound**: serially, split across
/// workers in reads a fraction of it, and over an `.xz` and a gzip source.
#[tokio::test(flavor = "multi_thread")]
async fn a_mapping_pass_holds_a_line_to_the_bound() {
    let dir = tempfile::tempdir().unwrap();
    for line in [BOUND, BOUND + 1] {
        let (plain, start) = dump(dir.path(), line, false);
        let local = || Arc::new(LocalFileSource::open(&plain).unwrap()) as Arc<dyn ByteRangeSource>;
        for (chunk, jobs) in [(64, 1), (4096, 1), (16, 4), (64, 8)] {
            let path = format!("a plain pass at {jobs} job(s), {chunk}-byte reads");
            held_to_the_bound(map(local(), read(chunk, BOUND), jobs).await, line, start, &path);
        }
        for (name, compressed) in [("xz", xz(&plain)), ("gzip", gzip(&plain))] {
            for jobs in [1, 4] {
                let source = opened(&compressed).await;
                let path = format!("an {name} pass at {jobs} job(s)");
                held_to_the_bound(map(source, read(64, BOUND), jobs).await, line, start, &path);
            }
        }
    }
}

/// **A pass resumed from an interrupted cache holds a line to the bound**:
/// one whose first pass banked `public.u`'s block and died, resumed at its
/// end, reads `public.t`'s long row from there.
#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_pass_holds_a_line_to_the_bound() {
    for line in [BOUND, BOUND + 1] {
        let dir = tempfile::tempdir().unwrap();
        let (plain, start) = dump(dir.path(), line, true);
        let mode = CacheMode::enabled(colocated_path(&plain));
        let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&plain).unwrap());
        let text = std::fs::read_to_string(&plain).unwrap();
        let first_end = (text.find(U_END).unwrap() + U_END.len()) as u64;
        let dying = FailsPast { inner: Arc::clone(&source), fail_at: first_end };
        let token = CancellationToken::new();
        let invalid = PostgresInvalidValues::Default;
        let request = &StatisticsRequest::DATA;
        let slow = read(1, BOUND);
        let opened = mode.open_source(Arc::new(dying)).await.unwrap();
        let died = map_file(opened, slow, &arranged(1), invalid, request, &token).await;
        assert!(matches!(died, Err(Error::Io(_))), "{died:?}");

        let opened = mode.open_source(source).await.unwrap();
        let resumed = map_file(opened, read(64, BOUND), &arranged(1), invalid, request, &token)
            .await
            .map(|run| run.resumed_from);
        if let Ok(resumed_from) = resumed {
            assert_eq!(resumed_from, first_end);
        }
        held_to_the_bound(resumed, line, start, "a resumed pass");
    }
}

/// **A query's replay holds a line to the bound**, over a map taken at the
/// real one: serially, and cut into segments over four and eight
/// partitions, a segment reading through the line its successors search
/// inside.
#[tokio::test(flavor = "multi_thread")]
async fn a_replay_holds_a_line_to_the_bound() {
    let dir = tempfile::tempdir().unwrap();
    for line in [BOUND, BOUND + 1] {
        let (plain, start) = dump(dir.path(), line, false);
        let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&plain).unwrap());
        let dump: Dump = mapped(source, &StatisticsRequest::DATA).await;
        let selected = dump.tables().iter().find(|t| t.qualified_name() == "public.t").unwrap();
        let table = dump.table(&selected.clone().into()).unwrap().unwrap();
        for (chunk, jobs) in [(64, 1), (16, 4), (64, 8)] {
            let replay = arranged_as(Parallelism::workers(jobs, 1 << 30));
            let plan = table.plan(read(chunk, BOUND), QueryOptions::default(), &replay).unwrap();
            let mut stream = plan.in_file_order(8192);
            let mut rows = 0;
            let mut read = Ok(());
            while let Some(batch) = stream.next().await {
                match batch {
                    Ok(batch) => rows += rows_of(&batch).len(),
                    Err(e) => {
                        read = Err(e);
                        break;
                    }
                }
            }
            if read.is_ok() {
                assert_eq!(rows, 65, "a replay at {jobs} partition(s)");
            }
            let path = format!("a replay at {jobs} partition(s), {chunk}-byte reads");
            held_to_the_bound(read, line, start, &path);
        }
    }
}
