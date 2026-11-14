//! A query's held lines on its run's ledger (`docs/design/decisions.md`,
//! "D157", "D158", "D159"): a line held past its chunk term draws its bytes
//! and its query's decode beside them, returns the draw before any batch is
//! yielded, and answers what a ledger without limit answers however its
//! partitions are driven — its cold mapping pass's census drawing on its
//! own.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arrow::array::RecordBatch;
use arrow::compute::concat_batches;
use bytes::Bytes;
use futures::StreamExt;
use pgdt_codec::SchemaMode;
use pgdt_dump::ReadOptions;
use pgdt_dump::cache::CacheMode;
use pgdt_io::{
    Admission, Allowance, Arrangement, ArrangementRequest, ByteRangeSource, Extent, Jobs,
    LocalFileSource,
};
use pgdt_query::{QueryOptions, ScanExtent, Table, TablePartitions};
use tokio_util::sync::CancellationToken;

mod common;
use common::{VERSIONS, arranged, selector, types_fixture};

/// `batches` as the one batch they concatenate to: where a run cuts its
/// batches is no part of its answer.
fn joined(batches: &[RecordBatch]) -> RecordBatch {
    concat_batches(&batches[0].schema(), batches).unwrap()
}

/// How long a drive may take before it is called stalled.
const STALLED: Duration = Duration::from_secs(60);

/// `jobs` workers stated under a gibibyte other scans drew all but
/// `capacity` bytes of: **a ledger lending exactly `capacity`**, beside the
/// workers and budget [`arranged`] states.
fn lending(jobs: usize, capacity: u64) -> Arrangement {
    let allowance = Allowance::Stated(1 << 30);
    let stated = ArrangementRequest {
        jobs: Jobs::Stated(jobs),
        ..ArrangementRequest::recommended(jobs, None)
    };
    let room = Arrangement::resolve(&allowance, stated).ledger().capacity().unwrap();
    let drawn = room - capacity;
    let arrangement = Arrangement::resolve(&allowance, ArrangementRequest { drawn, ..stated });
    assert_eq!(arrangement.ledger().capacity(), Some(capacity));
    let (parallelism, unbounded) = (arrangement.parallelism(), arranged(jobs).parallelism());
    assert_eq!(
        (parallelism.jobs(), parallelism.budget_bytes()),
        (unbounded.jobs(), unbounded.budget_bytes())
    );
    arrangement
}

/// `table`'s typed cold plan in `source`, writing no cache, both passes read
/// in `chunk`-byte reads under `arrangement`, their held lines drawing on its
/// ledger one pass after the other, as `pgdt query`'s draw on their own.
async fn plan(
    source: Arc<dyn ByteRangeSource>,
    table: &str,
    chunk: usize,
    arrangement: &Arrangement,
) -> TablePartitions {
    let read = ReadOptions { chunk_size_bytes: chunk, ..ReadOptions::default() };
    let opened = CacheMode::DISABLED.open_source(source).await.unwrap();
    let (extent, typed, token) =
        (ScanExtent::UntilTargetSettled, SchemaMode::Typed, CancellationToken::new());
    let settled =
        Table::settle(opened, &selector(table), read, arrangement, extent, typed, &token).await;
    let settled = settled.unwrap().expect("the dump holds it");
    let options =
        QueryOptions { parallelism: arrangement.parallelism(), ..QueryOptions::default() };
    settled.plan(read, options).unwrap().drawing_on(arrangement.ledger())
}

/// A dump held in memory, every read answered at once: a replay polled by
/// hand waits on nothing but its ledger.
struct Memory(Bytes);

impl ByteRangeSource for Memory {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = pgdt_core::Result<Bytes>> + Send + '_>> {
        let at = offset as usize;
        let bytes = self.0.slice(at..at + len);
        Box::pin(async move { Ok(bytes) })
    }

    fn extent(&self) -> Pin<Box<dyn Future<Output = pgdt_core::Result<Extent>> + Send + '_>> {
        let len = self.0.len() as u64;
        Box::pin(async move { Ok(Extent::Exact(len)) })
    }

    fn modified(
        &self,
    ) -> Pin<Box<dyn Future<Output = pgdt_core::Result<Option<SystemTime>>> + Send + '_>> {
        Box::pin(async { Ok(None) })
    }
}

/// The chunk [`a_held_line_draws_its_bytes_and_its_query_s_decode`] reads
/// in, far below its long field.
const CHUNK: usize = 1 << 10;

/// **A line held past its chunk term draws its bytes and what its query's
/// one factor says its batch decodes beside them**, and returns the draw
/// before each batch is yielded (`docs/design/decisions.md`, "D159"): a
/// `text` value's factor is one, so its line draws twice its bytes, and a
/// `bigint[]`'s is `65/16` (`stream/replay/plan/tests.rs`). An earlier line
/// holding a byte, its ledger lends exactly what both need: one byte less
/// and the long line waits, until the earlier one is returned.
#[tokio::test]
async fn a_held_line_draws_its_bytes_and_its_query_s_decode() {
    let digits = "1,".repeat(8 << 10);
    let array = format!("{{{digits}2}}");
    let text = "x".repeat(16 << 10);
    let cases =
        [("text", "short", text.as_str(), (1, 1)), ("bigint[]", "{1}", array.as_str(), (65, 16))];
    for (declared, short, long, (held, per)) in cases {
        let file = format!(
            "CREATE TABLE public.t (\n    a integer,\n    b {declared}\n);\n\n\
             COPY public.t (a, b) FROM stdin;\n1\t{short}\n2\t{long}\n3\t{short}\n\\.\n\n"
        );
        let line = format!("2\t{long}\n").len() as u64;
        let charge = line + (line * held).div_ceil(per);
        let source: Arc<dyn ByteRangeSource> = Arc::new(Memory(Bytes::from(file)));
        let unlimited = plan(Arc::clone(&source), "public.t", CHUNK, &arranged(1)).await;
        let want: Vec<_> = unlimited.stream(0, 8192).map(Result::unwrap).collect().await;
        let want = joined(&want);
        assert_eq!(want.num_rows(), 3);
        for short_by in [0, 1] {
            let arrangement = lending(1, 1 + charge - short_by);
            let ledger = arrangement.ledger();
            let mut earlier = ledger.holder();
            assert_eq!(earlier.draw(0, 1).await, Admission::Held);
            let plan = plan(Arc::clone(&source), "public.t", CHUNK, &arrangement).await;
            let mut stream = plan.stream(0, 8192);
            let mut batches = Vec::new();
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            loop {
                let batch = match stream.poll_next_unpin(&mut cx) {
                    std::task::Poll::Ready(Some(batch)) => batch.unwrap(),
                    std::task::Poll::Ready(None) => break,
                    std::task::Poll::Pending => {
                        assert_eq!(
                            short_by, 1,
                            "{declared}: the line waits only where it cannot fit"
                        );
                        assert!(batches.len() < 2, "{declared}: it waits before its row");
                        earlier.release();
                        continue;
                    }
                };
                assert_eq!(ledger.drawn(), earlier.held(), "{declared}: returned as it is yielded");
                batches.push(batch);
            }
            assert_eq!(joined(&batches), want, "{declared}, short by {short_by}");
            assert_eq!(earlier.held(), 1 - short_by, "{declared}: the earlier line waited on");
        }
    }
}

/// The tables of the `types` fixture whose lines a four-byte chunk holds
/// past its term, among them each kind of factor: text and `bytea` one, an
/// array's, a `jsonb`'s, a nested value's and a multirange's, which no dump
/// before 14 holds; and containers of datetimes and `interval`s, which a cold
/// pass's census holds whole.
const TABLES: [&str; 7] = [
    "public.t_extremes_nested",
    "public.t_text",
    "public.t_bytea",
    "public.t_array",
    "public.t_json",
    "public.t_nested_array",
    "public.t_multirange",
];

/// Every row of `plan` in file order, as `pgdt query` reads it: one batch a
/// partition held at a time, only the one just emitted from polled again
/// past the first round (`KD57`).
async fn in_file_order(plan: &TablePartitions, max_rows: usize) -> RecordBatch {
    let batches: Vec<_> = plan.in_file_order(max_rows).map(Result::unwrap).collect().await;
    joined(&batches)
}

/// Every row of `plan`, its partitions driven as DataFusion's
/// `CoalescePartitionsExec` drives them — a task each, sending into one
/// channel holding a batch a partition — and put back in partition order,
/// which is file order.
async fn coalesced(plan: TablePartitions, max_rows: usize) -> RecordBatch {
    let plan = Arc::new(plan);
    let (tx, mut rx) = tokio::sync::mpsc::channel(plan.len());
    for partition in 0..plan.len() {
        let (tx, mut stream) = (tx.clone(), plan.stream(partition, max_rows));
        tokio::spawn(async move {
            while let Some(batch) = stream.next().await {
                if tx.send((partition, batch.unwrap())).await.is_err() {
                    return;
                }
            }
        });
    }
    drop(tx);
    let mut each = vec![Vec::new(); plan.len()];
    while let Some((partition, batch)) = rx.recv().await {
        each[partition].push(batch);
    }
    joined(&each.concat())
}

/// `table` of `dump`, read in `chunk`-byte reads at `arrangement`'s
/// parallelism, its lines drawing on its ledger: how many partitions it is
/// cut into, and its rows in file order and driven as
/// `CoalescePartitionsExec` drives it, each within [`STALLED`].
async fn drives(
    dump: &Path,
    table: &str,
    chunk: usize,
    arrangement: &Arrangement,
) -> (usize, RecordBatch, RecordBatch) {
    let source = || -> Arc<dyn ByteRangeSource> { Arc::new(LocalFileSource::open(dump).unwrap()) };
    let ordered = plan(source(), table, chunk, arrangement).await;
    let partitions = ordered.len();
    let merged = tokio::time::timeout(STALLED, in_file_order(&ordered, 2));
    let merged = merged.await.unwrap_or_else(|_| panic!("{table}: the merge stalled"));
    let tasks = coalesced(plan(source(), table, chunk, arrangement).await, 2);
    let tasks = tokio::time::timeout(STALLED, tasks);
    let tasks = tasks.await.unwrap_or_else(|_| panic!("{table}: the tasks stalled"));
    (partitions, merged, tasks)
}

/// **A query's partitions on a ledger with no room answer what they answer
/// on one without limit**, in file order and driven as
/// `CoalescePartitionsExec` drives them, and neither stalls
/// (`docs/design/decisions.md`, "D159"): every line one holds past a
/// four-byte chunk while another holds one waits, or gives its own up
/// and reads it again, the mapping pass's workers' and census's as the
/// replay's. The merge holds back each partition at a batch it yielded,
/// which holds no draw there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_query_s_partitions_on_a_full_ledger_answer_as_on_an_unlimited_one() {
    const JOBS: usize = 4;
    const CHUNK: usize = 4;
    for version in VERSIONS {
        let dump = types_fixture(version, "default");
        let mut cut = false;
        for table in TABLES.iter().filter(|&&table| version >= 14 || table != "public.t_multirange")
        {
            let (_, want, _) = drives(&dump, table, CHUNK, &arranged(JOBS)).await;
            assert!(want.num_rows() > 0, "{table} on {version} holds rows");
            let (partitions, merged, tasks) = drives(&dump, table, CHUNK, &lending(JOBS, 0)).await;
            assert_eq!(merged, want, "{table} on {version}, in file order");
            assert_eq!(tasks, want, "{table} on {version}, a task a partition");
            cut |= partitions > 1;
        }
        assert!(cut, "some table on {version} is read in partitions");
    }
}
