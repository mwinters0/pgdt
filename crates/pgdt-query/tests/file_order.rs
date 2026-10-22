//! A plan's whole table as one stream in file order, and its serial resume
//! (`docs/design/decisions.md`, "D49", "D51" and "D52").
//!
//! The oracle is a cold serial query, as for the partitioned replay:
//! a stream in file order has no rows of its own to assert, its contract
//! being that it returns exactly the serial rows in exactly their order,
//! however many partitions the plan was cut into and wherever a consumption
//! paused.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use futures::StreamExt;
use pgdt_core::Error;
use pgdt_dump::ReadOptions;
use pgdt_io::{ByteRangeSource, LocalFileSource, Parallelism};
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_query::{Dump, QueryOptions, Table, TablePartitions, TableStream};

mod common;
use common::{Cold, dump_over, edge_cases, mapped, rows_of, types_fixture};

type Rows = Vec<Vec<Option<String>>>;

/// Every row a serial cold query yields, in file order — the oracle.
async fn serial_rows(source: Arc<dyn ByteRangeSource>, table: &str) -> Rows {
    let stream = Cold::default().stream(source, table, QueryOptions::default()).await;
    let mut stream = stream.unwrap().unwrap();
    let mut rows = Rows::new();
    while let Some(batch) = stream.next().await {
        rows.extend(rows_of(&batch.unwrap()));
    }
    rows
}

/// `path` mapped whole, and its table `name`.
async fn table_of(path: &Path, name: &str) -> (Arc<dyn ByteRangeSource>, Dump, Table) {
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(path).unwrap());
    let dump = mapped(Arc::clone(&source), &StatisticsRequest::DATA).await;
    let selected = dump.tables().iter().find(|t| t.qualified_name() == name).unwrap().clone();
    let table = dump.table(&selected.into()).unwrap().unwrap();
    (source, dump, table)
}

/// `table` planned over `jobs` partitions.
fn plan(table: &Table, jobs: usize) -> TablePartitions {
    let options =
        QueryOptions { parallelism: Parallelism::workers(jobs, 1 << 30), ..Default::default() };
    table.plan(ReadOptions::default(), options).unwrap()
}

/// What is left of `stream`, as rows.
async fn drain(stream: &mut TableStream<'static>) -> Rows {
    let mut rows = Rows::new();
    while let Some(batch) = stream.next().await {
        rows.extend(rows_of(&batch.unwrap()));
    }
    rows
}

/// **The serial rows, at every partition count**, batched at one row so the
/// merge picks between partitions at every batch, the source offset each
/// batch reports rising, and the schema the plan's, held before the first
/// poll.
#[tokio::test]
async fn the_whole_table_in_file_order_is_the_serial_replay() {
    for (path, name) in
        [(edge_cases(), "public.widgets"), (types_fixture(16, "default"), "public.t_int")]
    {
        let (source, _dump, table) = table_of(&path, name).await;
        let expected = serial_rows(Arc::clone(&source), name).await;
        assert!(!expected.is_empty(), "{name}: the oracle must have rows");
        let mut split = false;
        for jobs in [1usize, 2, 3, 4, 7, 64] {
            let plan = plan(&table, jobs);
            split |= plan.len() > 1;
            let mut stream = plan.in_file_order(1);
            assert_eq!(stream.resolved_schema().schema(), plan.resolved_schema().schema());
            let (mut rows, mut previous) = (Rows::new(), None);
            while let Some(batch) = stream.next().await {
                rows.extend(rows_of(&batch.unwrap()));
                let offset = stream.batch_source_offset();
                assert!(previous.is_none_or(|previous| previous < offset), "{name}, jobs {jobs}");
                previous = Some(offset);
            }
            assert_eq!(rows, expected, "{name}, jobs {jobs}");
        }
        assert!(split, "{name}: no plan split, so the merge chose between nothing");
    }
}

/// **A stream in file order resumes where it paused**, serially, whatever
/// cut its plan was made under and whichever plan of the table resumes it:
/// the rows before the pause and the rows after it are the serial rows,
/// none twice. Paused at every batch.
#[tokio::test]
async fn a_file_ordered_stream_resumes_at_its_token() {
    let name = "public.t_int";
    let (source, _dump, table) = table_of(&types_fixture(16, "default"), name).await;
    let expected = serial_rows(Arc::clone(&source), name).await;
    let (cut, serial) = (plan(&table, 4), plan(&table, 1));
    assert!(cut.len() > 1, "the plan must split for the token to cross a partition");
    for paused_after in 0..=expected.len() {
        let mut stream = cut.in_file_order(1);
        let mut rows = Rows::new();
        for _ in 0..paused_after {
            rows.extend(rows_of(&stream.next().await.unwrap().unwrap()));
        }
        let token = stream.resume_token();
        for resuming in [&cut, &serial] {
            let mut resumed = resuming.resume(token.clone(), 2).unwrap();
            let mut all = rows.clone();
            all.extend(drain(&mut resumed).await);
            assert_eq!(all, expected, "paused after {paused_after}");
        }
    }

    // A resumed stream's own token resumes again.
    let mut first = cut.in_file_order(1);
    let mut rows = rows_of(&first.next().await.unwrap().unwrap());
    let mut second = cut.resume(first.resume_token(), 1).unwrap();
    rows.extend(rows_of(&second.next().await.unwrap().unwrap()));
    let mut third = cut.resume(second.resume_token(), 1).unwrap();
    rows.extend(drain(&mut third).await);
    assert_eq!(rows, expected);
}

/// **Only the whole table's token resumes**: a partition's knows nothing of
/// the range it was confined to, and one from another query of the table
/// would resume a different set of rows (`docs/design/decisions.md`, "D51").
#[tokio::test]
async fn resume_refuses_a_partitions_token_and_another_querys() {
    let (_source, _dump, table) = table_of(&types_fixture(16, "default"), "public.t_int").await;
    let plan = plan(&table, 4);
    let mut partition = plan.stream(0, 1);
    partition.next().await.unwrap().unwrap();
    let refused = plan.resume(partition.resume_token(), 1).err().unwrap();
    assert!(matches!(refused, Error::ResumeQueryMismatch), "{refused}");

    let projected = QueryOptions { projection: Some(Vec::new()), ..Default::default() };
    let other = table.plan(ReadOptions::default(), projected).unwrap();
    let mut whole = other.in_file_order(1);
    whole.next().await.unwrap().unwrap();
    let refused = plan.resume(whole.resume_token(), 1).err().unwrap();
    assert!(matches!(refused, Error::ResumeQueryMismatch), "{refused}");
}

/// A source failing every read that touches `ranges`, each with its own
/// message, and otherwise the plain file it wraps, its advice on where to cut
/// included.
struct FailsWithin {
    inner: LocalFileSource,
    ranges: Vec<(Range<u64>, &'static str)>,
}

type Boxed<'a, T> = Pin<Box<dyn std::future::Future<Output = pgdt_core::Result<T>> + Send + 'a>>;

impl ByteRangeSource for FailsWithin {
    fn read_range(&self, offset: u64, len: usize) -> Boxed<'_, bytes::Bytes> {
        Box::pin(async move {
            let end = offset + len as u64;
            for (range, label) in &self.ranges {
                if offset < range.end && range.start < end {
                    return Err(Error::Io(std::io::Error::other(*label)));
                }
            }
            self.inner.read_range(offset, len).await
        })
    }

    fn extent(&self) -> Boxed<'_, pgdt_io::Extent> {
        self.inner.extent()
    }

    fn modified(&self) -> Boxed<'_, Option<std::time::SystemTime>> {
        self.inner.modified()
    }

    fn hint_read_size(&self, len: usize) {
        self.inner.hint_read_size(len);
    }

    fn partitions(&self, range: Range<u64>) -> pgdt_io::Partitioning {
        self.inner.partitions(range)
    }
}

/// A one-table dump of `rows` integers, `public.t`.
fn integers(dir: &Path, rows: u32) -> PathBuf {
    let mut file = String::from("SET client_encoding = 'UTF8';\n\n");
    file.push_str("CREATE TABLE public.t (\n    id integer\n);\n\n");
    file.push_str("COPY public.t (id) FROM stdin;\n");
    for i in 1..=rows {
        file.push_str(&format!("{i}\n"));
    }
    file.push_str("\\.\n\n");
    let path = dir.join("integers.sql");
    std::fs::write(&path, file).unwrap();
    path
}

/// **The error is the earliest failing piece's, after every row before it**
/// (`docs/design/decisions.md`, "D52"). Two reads fail: one late in the
/// file, which the second partition reaches in its first chunks, and one
/// early, which the first partition reaches only after several batches. The
/// early one is raised, at every partition count, and no row past it is
/// yielded — though the partitions after the failing one were each holding a
/// batch when it failed.
#[tokio::test]
async fn the_earliest_failing_piece_is_the_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = integers(dir.path(), 40_000);
    let bytes = std::fs::read(&path).unwrap();
    let at = |row: u32| {
        let needle = format!("\n{row}\n");
        let found = bytes.windows(needle.len()).position(|w| w == needle.as_bytes()).unwrap();
        (found + 1) as u64
    };
    let (early, late) = (at(18_000), at(22_000));

    let plain: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&path).unwrap());
    let index = mapped(plain, &StatisticsRequest::DATA).await.index().clone();
    let failing = FailsWithin {
        inner: LocalFileSource::open(&path).unwrap(),
        ranges: vec![(early..early + 1, "early"), (late..late + 1, "late")],
    };
    let dump = dump_over(Arc::new(failing), &index, dir.path()).await.unwrap();
    let table = dump.table(&pgdt_query::TableSelector::named("t")).unwrap().unwrap();
    let read = ReadOptions { chunk_size_bytes: 4096, ..ReadOptions::default() };
    let mut split = false;
    for jobs in [1usize, 2, 3, 4, 8] {
        let options =
            QueryOptions { parallelism: Parallelism::workers(jobs, 1 << 30), ..Default::default() };
        let plan = table.plan(read, options).unwrap();
        split |= plan.len() > 1;
        let mut stream = plan.in_file_order(8192);
        let mut last = 0u32;
        let err = loop {
            match stream.next().await.expect("the stream fails before it ends") {
                Ok(batch) => {
                    for row in rows_of(&batch) {
                        let id: u32 = row[0].as_deref().unwrap().parse().unwrap();
                        assert!(id < 18_000, "jobs {jobs}: row {id} is past the error");
                        last = id;
                    }
                }
                Err(err) => break err,
            }
        };
        assert!(err.to_string().contains("early"), "jobs {jobs} raised {err}");
        assert!(last > 0, "jobs {jobs}: the rows before the error come first");
        assert!(stream.next().await.is_none(), "jobs {jobs}: nothing follows the error");
    }
    assert!(split, "no plan split, so no later partition failed first");
}
