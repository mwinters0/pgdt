//! A query's held lines on its run's ledger (`docs/design/decisions.md`,
//! "D157", "D158", "D159", "D160", "D163"): the ledger carved around its
//! longest line, a line held past its chunk term draws its bytes and its
//! query's decode beside them, returns the draw before any batch is yielded,
//! parks the later sub-streams where it is alone past the room, and answers
//! what a ledger without limit answers however its partitions are driven —
//! its cold mapping pass's census drawing on its own.
//!
//! **These compare answers and count no wait or give-up**: at a four-byte
//! chunk the `types` fixture both waits and gives up, at sixteen nothing gives
//! up, so a changed chunk or fixture can stop exercising give-up unseen. A test
//! wanting a ledger with no room states its floor (`pgdt_fixtures::floored`),
//! never a small allowance, under which the floor is zero. Some test sources
//! forward `hint_parallelism` and not `hint_parked` — pgdt-query's
//! `map_file.rs`, `identity.rs`, `wait_policy.rs` and `partitioned_replay.rs`,
//! pgdt's `determinism.rs`, pgdt-mapping's `backfill/tests.rs` — so a parking
//! test through one gives nothing back.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, SystemTime};

use arrow::array::{ArrayRef, AsArray, RecordBatch};
use arrow::compute::concat_batches;
use bytes::Bytes;
use futures::StreamExt;
use pgdt_codec::SchemaMode;
use pgdt_dump::ReadOptions;
use pgdt_dump::cache::CacheMode;
use pgdt_io::{
    Admission, Allowance, Arrangement, ArrangementRequest, ByteRangeSource, Extent, Jobs,
    LocalFileSource, Parallelism, Partitioning, WaitPolicy, WorkerMemory,
};
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_query::{Expr, Predicate, PredicateOp, QueryOptions, ScanExtent, Table, TablePartitions};
use tokio_util::sync::CancellationToken;

mod common;
use common::{VERSIONS, arranged, selector, types_fixture};

/// `batches` as the one batch they concatenate to: where a run cuts its
/// batches is no part of its answer.
fn joined(batches: &[RecordBatch]) -> RecordBatch {
    concat_batches(&batches[0].schema(), batches).unwrap()
}

/// One reader stated under a gibibyte, `statistics` bytes of it kept beside
/// the line ([`ArrangementRequest::statistics`]) and other scans having drawn
/// enough of the rest: **a ledger refusing a line past `holds` bytes**, its
/// floor naming the statistics as what came off it
/// (`docs/design/decisions.md`, "D161").
fn beside_the_line(holds: u64, statistics: u64) -> Arrangement {
    let allowance = Allowance::Stated(1 << 30);
    let stated = ArrangementRequest {
        jobs: Jobs::Stated(1),
        statistics,
        ..ArrangementRequest::recommended(1, None)
    };
    let alone = Arrangement::resolve(&allowance, stated).ledger().floor().unwrap().holds;
    let drawn = alone - holds;
    let arrangement = Arrangement::resolve(&allowance, ArrangementRequest { drawn, ..stated });
    let floor = arrangement.ledger().floor().unwrap();
    assert_eq!((floor.holds, floor.beside), (holds, statistics));
    arrangement
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

/// `chunk`-byte reads.
fn reads(chunk: usize) -> ReadOptions {
    ReadOptions::default().with_chunk_size(chunk)
}

/// `table` settled by a typed cold mapping pass over `source`, writing no
/// cache, read in `chunk`-byte reads under `arrangement`, its held lines
/// drawing on its ledger.
async fn settled(
    source: Arc<dyn ByteRangeSource>,
    table: &str,
    chunk: usize,
    arrangement: &Arrangement,
) -> Table {
    let opened = CacheMode::DISABLED.open_source(source).await.unwrap();
    let (extent, typed, token) =
        (ScanExtent::UntilTargetSettled, SchemaMode::Typed, CancellationToken::new());
    let read = reads(chunk);
    let settled =
        Table::settle(opened, &selector(table), read, arrangement, extent, typed, &token).await;
    settled.unwrap().expect("the dump holds it")
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
    let settled = settled(source, table, chunk, arrangement).await;
    settled.plan(reads(chunk), QueryOptions::default(), arrangement).unwrap()
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
        let (source, line) = long_row(declared, short, long);
        let charge = line + (line * held).div_ceil(per);
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

/// `public.t`, an `integer` beside a `long` value of `declared`, between two
/// rows holding `short`: a dump held in memory, and the long row's line.
fn long_row(declared: &str, short: &str, long: &str) -> (Arc<dyn ByteRangeSource>, u64) {
    let file = format!(
        "CREATE TABLE public.t (\n    a integer,\n    b {declared}\n);\n\n\
         COPY public.t (a, b) FROM stdin;\n1\t{short}\n2\t{long}\n3\t{short}\n\\.\n\n"
    );
    let line = format!("2\t{long}\n").len() as u64;
    (Arc::new(Memory(Bytes::from(file))), line)
}

/// **A read over mapped blocks carves its longest line**
/// (`docs/design/decisions.md`, "D157", "D160"): the charge a table answers
/// is what that line draws once it outgrows its chunk — its bytes and its
/// query's one factor of them, a projection reading no long column charging
/// its bytes alone — and nothing where it fits its chunk; and an arrangement
/// carved around it, whose readers would otherwise leave its ledger less than
/// the line, admits that line without waiting, whatever else holds the rest.
#[tokio::test]
async fn a_read_over_mapped_blocks_carves_its_longest_line() {
    let digits = "1,".repeat(8 << 10);
    let array = format!("{{{digits}2}}");
    let text = "x".repeat(16 << 10);
    let cases =
        [("text", "short", text.as_str(), (1, 1)), ("bigint[]", "{1}", array.as_str(), (65, 16))];
    for (declared, short, long, (held, per)) in cases {
        let (source, line) = long_row(declared, short, long);
        let table = settled(Arc::clone(&source), "public.t", CHUNK, &arranged(1)).await;
        let every = QueryOptions::default();
        let charge = line + (line * held).div_ceil(per);
        assert_eq!(table.line_charge(reads(CHUNK), &every).unwrap(), charge, "{declared}");
        let narrow = QueryOptions { projection: Some(vec!["a".into()]), ..every.clone() };
        assert_eq!(table.line_charge(reads(CHUNK), &narrow).unwrap(), line, "{declared}");
        let whole = reads(line as usize);
        assert_eq!(table.line_charge(whole, &every).unwrap(), 0, "{declared}: it fits its chunk");

        // Readers a byte larger than the line, as many as the allowance
        // affords: they leave the room less than the line, unless it is
        // carved.
        let reader = Some(WorkerMemory::per_worker(charge + 1));
        let request = |line_charge| ArrangementRequest {
            line_charge,
            ..ArrangementRequest::recommended(1 << 20, reader)
        };
        let allowance = Allowance::Stated(1 << 30);
        let bare = Arrangement::resolve(&allowance, request(0));
        assert!(bare.ledger().capacity().unwrap() < charge, "{declared}: the readers fill it");
        let arrangement = Arrangement::resolve(&allowance, request(charge));
        let ledger = arrangement.ledger();
        let rest = ledger.capacity().unwrap() - ledger.drawn() - charge;
        let mut earlier = ledger.holder();
        assert_eq!(earlier.draw(0, rest).await, Admission::Held);
        let plan = table.plan(reads(CHUNK), every, &arrangement).unwrap();
        let mut stream = plan.stream(0, 8192);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let mut rows = 0;
        loop {
            match stream.poll_next_unpin(&mut cx) {
                std::task::Poll::Ready(Some(batch)) => rows += batch.unwrap().num_rows(),
                std::task::Poll::Ready(None) => break,
                std::task::Poll::Pending => panic!("{declared}: the carved line waited"),
            }
        }
        assert_eq!(rows, 3, "{declared}");
        assert_eq!(earlier.held(), rest, "{declared}: nothing was asked of the rest");
    }
}

/// **A read whose longest line one reader cannot hold is refused before it
/// reads a byte** (`docs/design/decisions.md`, "D161"): at a floor of its
/// charge the plan reads every row, the line admitted alone though the
/// ledger has no room; a byte under it, the plan is refused, naming that
/// line's start, the bytes it needs and the allowance, and what statistics
/// kept beside the line took off the floor — whatever the worker count, and
/// never for a read holding no field of it.
#[tokio::test]
async fn a_read_whose_longest_line_passes_the_floor_is_refused_before_it_reads() {
    let text = "x".repeat(16 << 10);
    let (source, line) = long_row("text", "short", &text);
    let table = settled(Arc::clone(&source), "public.t", CHUNK, &arranged(1)).await;
    let every = QueryOptions::default();
    let charge = table.line_charge(reads(CHUNK), &every).unwrap();
    assert_eq!(charge, 2 * line);

    let at = table.plan(reads(CHUNK), every.clone(), &pgdt_fixtures::floored(charge)).unwrap();
    let batches: Vec<_> = at.stream(0, 8192).map(Result::unwrap).collect().await;
    assert_eq!(joined(&batches).num_rows(), 3, "at the floor, admitted alone");

    let under = pgdt_fixtures::floored(charge - 1);
    let refused = table.plan(reads(CHUNK), every.clone(), &under).map(|_| ()).unwrap_err();
    let pgdt_core::Error::LineOutgrowsMemory { offset, needs, holds, beside, allowance } = refused
    else {
        panic!("not refused for memory: {refused}");
    };
    let before = "CREATE TABLE public.t (\n    a integer,\n    b text\n);\n\n\
                  COPY public.t (a, b) FROM stdin;\n1\tshort\n";
    assert_eq!(offset, before.len() as u64, "the long line's start");
    assert_eq!((needs, holds, beside, allowance), (charge, charge - 1, 0, 1 << 30));

    // What the run keeps beside the line comes off the floor, and the
    // refusal names it: the same floor, reached with statistics held.
    let statistics = 4 << 20;
    let kept = beside_the_line(charge - 1, statistics);
    let refused = table.plan(reads(CHUNK), every.clone(), &kept).map(|_| ()).unwrap_err();
    let message = refused.to_string();
    let pgdt_core::Error::LineOutgrowsMemory { holds, beside, .. } = refused else {
        panic!("not refused for memory: {refused}");
    };
    assert_eq!((holds, beside), (charge - 1, statistics));
    assert!(message.contains(&format!("the {statistics} byte(s) of statistics")), "{message}");

    // A streamed row draws nothing, not even as its carry starts streaming
    // it (`ChunkCarry::charge_absorbing`), so a floor below one chunk reads it.
    let counted = QueryOptions { projection: Some(Vec::new()), ..every };
    let floor = pgdt_fixtures::floored(1);
    let streamed = table.plan(reads(CHUNK), counted, &floor).unwrap();
    let batches: Vec<_> = streamed.stream(0, 8192).map(Result::unwrap).collect().await;
    assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 3, "no field held");
}

/// `column <op> value`, `op` the comparing variant's constructor.
fn term(column: &str, op: fn(String) -> PredicateOp, value: &str) -> Expr {
    Expr::Term(Predicate { column: column.into(), op: op(value.into()) })
}

/// **The charge counts only lines the read can hold**
/// (`docs/design/decisions.md`, "D160"): a block whose every row group its
/// static filter prunes is charged nothing, the read reaching none of its
/// rows, and so is one whose batch and static filter read no field, the
/// replay streaming its long rows; a filter keeping the long row's group, or
/// one reading a field of a projection reading none, charges it.
#[tokio::test]
async fn a_charge_counts_only_lines_the_read_can_hold() {
    let text = "x".repeat(16 << 10);
    let (source, line) = long_row("text", "short", &text);
    let dump = common::mapped(source, &StatisticsRequest::DATA).await;
    let table = dump.table(&selector("public.t")).unwrap().unwrap();
    let every = QueryOptions::default();
    let charge = 2 * line;
    assert_eq!(table.line_charge(reads(CHUNK), &every).unwrap(), charge);
    let filtered = |filter| QueryOptions { filter, ..every.clone() };
    let pruned = filtered(term("a", PredicateOp::Gt, "5"));
    assert_eq!(table.line_charge(reads(CHUNK), &pruned).unwrap(), 0, "its one group is pruned");
    let kept = filtered(term("a", PredicateOp::Ge, "2"));
    assert_eq!(table.line_charge(reads(CHUNK), &kept).unwrap(), charge, "its group is kept");
    let unread = QueryOptions { use_statistics: false, ..pruned };
    assert_eq!(table.line_charge(reads(CHUNK), &unread).unwrap(), charge, "nothing is pruned");

    let counted = QueryOptions { projection: Some(Vec::new()), ..every.clone() };
    assert_eq!(table.line_charge(reads(CHUNK), &counted).unwrap(), 0, "no field is read");
    let tested = QueryOptions { filter: term("a", PredicateOp::Eq, "2"), ..counted };
    assert_eq!(table.line_charge(reads(CHUNK), &tested).unwrap(), line, "its filter reads `a`");
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

/// What each of a parking source's workers is stated to hold, so a reader
/// parked gives that back to the ledger.
const PARKED_TERM: u64 = 64 << 20;

/// `jobs` workers of `source`'s stated cost under a gibibyte whose room
/// other scans drew whole: every line a reader holds past its chunk passes
/// the room, so the earliest, alone, parks the readers after it.
fn parking(source: &dyn ByteRangeSource, jobs: usize) -> Arrangement {
    let allowance = Allowance::Stated(1 << 30);
    let stated = ArrangementRequest::of(source, Some(jobs));
    let room = Arrangement::resolve(&allowance, stated).ledger().capacity().unwrap();
    let arrangement =
        Arrangement::resolve(&allowance, ArrangementRequest { drawn: room, ..stated });
    assert_eq!(arrangement.ledger().capacity(), Some(0));
    assert_eq!(arrangement.parallelism().jobs(), jobs);
    arrangement
}

/// The readers parked on a source, now and at most at once.
#[derive(Default)]
struct ParkCount {
    now: AtomicIsize,
    most: AtomicIsize,
}

impl ParkCount {
    fn hint(&self, parked: bool) {
        let now = self.now.fetch_add(if parked { 1 } else { -1 }, Ordering::SeqCst);
        self.most.fetch_max(now + 1, Ordering::SeqCst);
    }
}

/// A plain file stating that each of its workers holds [`PARKED_TERM`],
/// counting the readers parked on it.
struct Parks {
    inner: LocalFileSource,
    parked: ParkCount,
}

impl ByteRangeSource for Parks {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = pgdt_core::Result<Bytes>> + Send + '_>> {
        self.inner.read_range(offset, len)
    }
    fn extent(&self) -> Pin<Box<dyn Future<Output = pgdt_core::Result<Extent>> + Send + '_>> {
        self.inner.extent()
    }
    fn modified(
        &self,
    ) -> Pin<Box<dyn Future<Output = pgdt_core::Result<Option<SystemTime>>> + Send + '_>> {
        self.inner.modified()
    }
    fn hint_read_size(&self, len: usize) {
        self.inner.hint_read_size(len);
    }
    /// Forwarded, or the plan is never cut and no reader is later than
    /// another.
    fn partitions(&self, range: std::ops::Range<u64>) -> Partitioning {
        self.inner.partitions(range)
    }
    fn default_worker_memory(&self) -> Option<WorkerMemory> {
        Some(WorkerMemory::per_worker(PARKED_TERM))
    }
    fn hint_parallelism(&self, parallelism: Parallelism) {
        self.inner.hint_parallelism(parallelism);
    }
    fn hint_wait_policy(&self, policy: WaitPolicy) {
        self.inner.hint_wait_policy(policy);
    }
    fn hint_parked(&self, parked: bool) {
        self.parked.hint(parked);
        self.inner.hint_parked(parked);
    }
}

/// **A query whose sub-streams park answers what one without limit
/// answers** (`docs/design/decisions.md`, "D163"): each table of every
/// major's `types` fixture, settled with no limit and read in four-byte
/// reads at four workers whose parking gives their terms back to a ledger
/// with no room — each line held alone past it, the sub-streams after it
/// parked — answers in file order and driven as `CoalescePartitionsExec`
/// drives it what it answers without limit, neither stalling though the
/// merge holds back each partition at a batch it yielded, sub-streams
/// parking along the way and none left parked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_query_parking_its_sub_streams_answers_as_on_an_unlimited_ledger() {
    const JOBS: usize = 4;
    const CHUNK: usize = 4;
    for version in VERSIONS {
        let dump = types_fixture(version, "default");
        let (mut cut, mut parked) = (false, 0);
        for table in TABLES.iter().filter(|&&table| version >= 14 || table != "public.t_multirange")
        {
            let at = format!("{table} on {version}");
            let source = Arc::new(Parks {
                inner: LocalFileSource::open(&dump).unwrap(),
                parked: ParkCount::default(),
            });
            let shared = || -> Arc<dyn ByteRangeSource> { Arc::clone(&source) as _ };
            let settled = settled(shared(), table, CHUNK, &arranged(JOBS)).await;
            let unlimited = settled.plan(reads(CHUNK), QueryOptions::default(), &arranged(JOBS));
            let want = in_file_order(&unlimited.unwrap(), 2).await;
            assert!(want.num_rows() > 0, "{at} holds rows");
            assert_eq!(
                source.parked.most.load(Ordering::SeqCst),
                0,
                "{at}: nothing parks unlimited"
            );

            let arrangement = parking(source.as_ref(), JOBS);
            let ordered =
                settled.plan(reads(CHUNK), QueryOptions::default(), &arrangement).unwrap();
            cut |= ordered.len() > 1;
            let merged = tokio::time::timeout(STALLED, in_file_order(&ordered, 2));
            let merged = merged.await.unwrap_or_else(|_| panic!("{at}: the merge stalled"));
            assert_eq!(merged, want, "{at}, in file order");
            let tasks = settled.plan(reads(CHUNK), QueryOptions::default(), &arrangement).unwrap();
            let tasks = tokio::time::timeout(STALLED, coalesced(tasks, 2));
            let tasks = tasks.await.unwrap_or_else(|_| panic!("{at}: the tasks stalled"));
            assert_eq!(tasks, want, "{at}, a task a partition");
            assert_eq!(source.parked.now.load(Ordering::SeqCst), 0, "{at}");
            assert_eq!(arrangement.ledger().parked(), 0, "{at}");
            parked = parked.max(source.parked.most.load(Ordering::SeqCst));
        }
        assert!(cut, "some table on {version} is read in partitions");
        assert!(parked >= 1, "{version}: no sub-stream parked");
    }
}

/// A dump held in memory whose reads from `gate` on wait until it is
/// opened, stating that each of its workers holds [`PARKED_TERM`], counting
/// the readers parked on it and the reads it handed out still held.
struct Gated {
    bytes: Bytes,
    gate: u64,
    open: AtomicBool,
    parked: ParkCount,
    live: Arc<AtomicIsize>,
}

/// One read [`Gated`] handed out, its own copy, counted until its last
/// reference drops.
struct Live(Vec<u8>, Arc<AtomicIsize>);

impl AsRef<[u8]> for Live {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.1.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ByteRangeSource for Gated {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = pgdt_core::Result<Bytes>> + Send + '_>> {
        let at = offset as usize;
        let gated = offset >= self.gate;
        Box::pin(async move {
            std::future::poll_fn(|_| match !gated || self.open.load(Ordering::SeqCst) {
                true => Poll::Ready(()),
                false => Poll::Pending,
            })
            .await;
            self.live.fetch_add(1, Ordering::SeqCst);
            let read = Live(self.bytes[at..at + len].to_vec(), Arc::clone(&self.live));
            Ok(Bytes::from_owner(read))
        })
    }
    fn extent(&self) -> Pin<Box<dyn Future<Output = pgdt_core::Result<Extent>> + Send + '_>> {
        let len = self.bytes.len() as u64;
        Box::pin(async move { Ok(Extent::Exact(len)) })
    }
    fn modified(
        &self,
    ) -> Pin<Box<dyn Future<Output = pgdt_core::Result<Option<SystemTime>>> + Send + '_>> {
        Box::pin(async { Ok(None) })
    }
    fn default_worker_memory(&self) -> Option<WorkerMemory> {
        Some(WorkerMemory::per_worker(PARKED_TERM))
    }
    fn hint_parked(&self, parked: bool) {
        self.parked.hint(parked);
    }
}

/// Rows a parking test's batches are cut at.
const PARKING_BATCH: usize = 16;

/// **A sub-stream asked to park keeps its batch, emitting nothing for the
/// park, and pins no read chunk while parked** (`docs/design/decisions.md`,
/// "D162", "D163"): forty rows of `public.t (a <declared>)`, the `n`th
/// holding `value(n)`, read in `chunk`-byte reads at
/// [`PARKING_BATCH`] rows a batch. The replay holds rows of its second batch
/// when a line alone past a ledger with no room asks it to park; it parks at
/// its next read without yielding, holding no read but the one before its
/// park (`KD172`), the line is admitted once it has, and the replay resumes as
/// the line's draw is returned, its batches those it yields without limit.
async fn parks_keeping_its_batch(declared: &str, value: fn(usize) -> String, chunk: usize) {
    let rows: String = (1..=40).map(|row| format!("{}\n", value(row))).collect();
    let before =
        format!("CREATE TABLE public.t (\n    a {declared}\n);\n\nCOPY public.t (a) FROM stdin;\n");
    let file = format!("{before}{rows}\\.\n\n");
    let gate =
        (before.len() + file[before.len()..].find(&format!("{}\n", value(24))).unwrap()) as u64;
    let source = Arc::new(Gated {
        bytes: Bytes::from(file),
        gate,
        open: AtomicBool::new(true),
        parked: ParkCount::default(),
        live: Arc::default(),
    });
    let shared = || -> Arc<dyn ByteRangeSource> { Arc::clone(&source) as _ };
    let table = settled(shared(), "public.t", chunk, &arranged(1)).await;
    let every = QueryOptions::default();
    let unlimited = table.plan(reads(chunk), every.clone(), &arranged(1)).unwrap();
    let want: Vec<_> = unlimited.stream(0, PARKING_BATCH).map(Result::unwrap).collect().await;
    assert_eq!(want.iter().map(RecordBatch::num_rows).collect::<Vec<_>>(), [16, 16, 8]);
    let reads_held = || source.live.load(Ordering::SeqCst);
    let before_the_stream = reads_held();

    let arrangement = parking(source.as_ref(), 2);
    let ledger = arrangement.ledger();
    let plan = table.plan(reads(chunk), every, &arrangement).unwrap();
    assert_eq!(plan.len(), 1);
    source.open.store(false, Ordering::SeqCst);
    let mut stream = plan.stream(0, PARKING_BATCH);
    let mut cx = Context::from_waker(Waker::noop());
    let mut batches = Vec::new();
    while let Poll::Ready(batch) = stream.poll_next_unpin(&mut cx) {
        batches.push(owned(batch.expect("it waits at the gate").unwrap()));
    }
    assert_eq!(batches.len(), 1, "it waits at the gate holding rows of its second batch");

    let mut line = ledger.holder();
    {
        let mut draw = std::pin::pin!(line.draw(0, 1));
        assert!(draw.as_mut().poll(&mut cx).is_pending(), "the line waits on the park");
        source.open.store(true, Ordering::SeqCst);
        assert!(stream.poll_next_unpin(&mut cx).is_pending(), "it parks, yielding nothing");
        assert_eq!((ledger.parked(), source.parked.now.load(Ordering::SeqCst)), (1, 1));
        assert!(
            reads_held() - before_the_stream <= 1,
            "parked, it holds no read but the one before its park: {}",
            reads_held() - before_the_stream,
        );
        assert_eq!(draw.poll(&mut cx), Poll::Ready(Admission::Held));
    }
    line.release();
    loop {
        match stream.poll_next_unpin(&mut cx) {
            Poll::Ready(Some(batch)) => batches.push(owned(batch.unwrap())),
            Poll::Ready(None) => break,
            Poll::Pending => panic!("the released line let it go"),
        }
    }
    assert_eq!(batches, want, "cut where an unlimited ledger cuts");
    assert_eq!((ledger.parked(), source.parked.now.load(Ordering::SeqCst)), (0, 0));
}

/// `batch` holding none of the reads its views point into, as a test keeping
/// it must, or the reads it holds are counted against the stream (`KD171`).
fn owned(batch: RecordBatch) -> RecordBatch {
    let columns = batch
        .columns()
        .iter()
        .map(|column| match column.as_string_view_opt() {
            Some(text) => Arc::new(text.gc()) as ArrayRef,
            None => Arc::clone(column),
        })
        .collect();
    RecordBatch::try_new(batch.schema(), columns).unwrap()
}

/// [`parks_keeping_its_batch`] over integers, which take no view.
#[tokio::test]
async fn a_sub_stream_asked_to_park_keeps_its_batch() {
    parks_keeping_its_batch("integer", |row| row.to_string(), 4).await;
}

/// [`parks_keeping_its_batch`] over text past a view's inline twelve bytes,
/// read in chunks holding several rows whole, so its batch views the reads
/// it parks between and re-owns them.
#[tokio::test]
async fn a_sub_stream_asked_to_park_re_owns_its_views() {
    parks_keeping_its_batch("text", |row| format!("the value of row {row:02}"), 64).await;
}
