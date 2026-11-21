//! What a parse's readers draw on its ledger: a line held past its chunk —
//! a field its census, its statistics or its check reads whole — waits
//! behind an earlier line, or is given up to one and read again, or is held
//! alone past the room while later readers park, and a parse answers what it
//! answers drawing on no ledger (`docs/design/decisions.md`, "D157",
//! "D163"). What these cannot see is pgdt-query's `tests/ledger.rs`'s to say.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::SystemTime;

use bytes::Bytes;

use pgdt_dump::ReadOptions;
use pgdt_dump::cache::CacheMode;
use pgdt_fixtures::{VERSIONS, statistics_fixture, types_fixture};
use pgdt_io::{
    Allowance, Arrangement, ArrangementRequest, ByteRangeSource, Extent, Jobs, LocalFileSource,
    Parallelism, Partitioning, WaitPolicy, WorkerMemory,
};
use pgdt_mapping::map_file;
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::{CopyBlock, PostgresInvalidValues};
use tokio_util::sync::CancellationToken;

/// `jobs` workers stated under no allowance: a ledger without limit.
fn arranged(jobs: usize) -> Arrangement {
    let request = ArrangementRequest {
        jobs: Jobs::Stated(jobs),
        ..ArrangementRequest::recommended(jobs, None)
    };
    Arrangement::resolve(&Allowance::NoneFound, request)
}

/// [`arranged`]'s workers and budget under a stated gibibyte whose room
/// other scans drew whole ([`ArrangementRequest::drawn`]): **a ledger with
/// no room**, which admits the earliest line alone, so every line a reader
/// holds past its chunk while another holds one waits or gives its own up.
fn no_room(jobs: usize) -> Arrangement {
    let allowance = Allowance::Stated(1 << 30);
    let stated = ArrangementRequest {
        jobs: Jobs::Stated(jobs),
        ..ArrangementRequest::recommended(jobs, None)
    };
    let room = Arrangement::resolve(&allowance, stated).ledger().capacity().unwrap();
    let arrangement =
        Arrangement::resolve(&allowance, ArrangementRequest { drawn: room, ..stated });
    assert_eq!(arrangement.ledger().capacity(), Some(0));
    let (parallelism, unbounded) = (arrangement.parallelism(), arranged(jobs).parallelism());
    assert_eq!(
        (parallelism.jobs(), parallelism.budget_bytes()),
        (unbounded.jobs(), unbounded.budget_bytes())
    );
    arrangement
}

/// One reader stated under a gibibyte other scans drew enough of: **a ledger
/// refusing a line past `holds` bytes** with no room to lend, as
/// `pgdt_fixtures::floored` is, which this crate's tests cannot reach.
fn floored(holds: u64) -> Arrangement {
    let allowance = Allowance::Stated(1 << 30);
    let stated =
        ArrangementRequest { jobs: Jobs::Stated(1), ..ArrangementRequest::recommended(1, None) };
    let alone = Arrangement::resolve(&allowance, stated).ledger().floor().unwrap().holds;
    let drawn = alone - holds;
    let arrangement = Arrangement::resolve(&allowance, ArrangementRequest { drawn, ..stated });
    assert_eq!(arrangement.ledger().floor().map(|floor| floor.holds), Some(holds));
    arrangement
}

/// The blocks a cacheless data-level `map_file` of `dump` maps to under
/// `arrangement`, in `chunk`-byte reads, or the error it fails with.
async fn outcome(
    dump: &std::path::Path,
    chunk: usize,
    arrangement: &Arrangement,
    invalid: PostgresInvalidValues,
) -> Result<Vec<CopyBlock>, String> {
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(dump).unwrap());
    outcome_over(source, chunk, arrangement, invalid).await
}

/// [`outcome`], read from `source`.
async fn outcome_over(
    source: Arc<dyn ByteRangeSource>,
    chunk: usize,
    arrangement: &Arrangement,
    invalid: PostgresInvalidValues,
) -> Result<Vec<CopyBlock>, String> {
    let read = ReadOptions::default().with_chunk_size(chunk);
    let opened = CacheMode::DISABLED.open_source(source).await.unwrap();
    let (statistics, cancel) = (&StatisticsRequest::DATA, CancellationToken::new());
    let run = map_file(opened, read, arrangement, invalid, statistics, &cancel).await;
    let index = run.map_err(|e| e.to_string())?.index;
    Ok(index.blocks().cloned().collect())
}

/// What a block records of its rows whatever its statistics hold: a ledger
/// with no room leaves statistics no allowance, so every block declines them.
fn rows_of(blocks: &[CopyBlock]) -> Vec<String> {
    let rows = |b: &CopyBlock| {
        let checked = b.record.checked_in_full;
        format!(
            "{:?} {} {:?} {checked}",
            b.record.census, b.record.row_count, b.record.longest_line
        )
    };
    blocks.iter().map(rows).collect()
}

/// **A parse whose readers wait and give their lines up maps what one
/// reader maps**: every block of every major's `types` and `statistics`
/// fixture, read in reads so short that nearly every row outgrows its chunk,
/// by a default and a strict parse at four workers on a ledger with no room
/// — their held fields waiting behind an earlier piece's and given up to it
/// — maps to the blocks one reader maps under it, and to the census, count,
/// longest line and check one drawing on no ledger maps.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parse_waiting_on_its_ledger_maps_what_one_reader_does() {
    for version in VERSIONS {
        for (fixture, chunks) in [
            (types_fixture(version, "default"), &[16usize, 61][..]),
            (statistics_fixture(version, "default"), &[61][..]),
        ] {
            for invalid in [PostgresInvalidValues::Default, PostgresInvalidValues::Strict] {
                for &chunk in chunks {
                    let at = format!("{version}: {fixture:?}, {invalid:?}, chunk {chunk}");
                    let unlimited = outcome(&fixture, chunk, &arranged(1), invalid).await;
                    let serial = outcome(&fixture, chunk, &no_room(1), invalid).await;
                    let waiting = outcome(&fixture, chunk, &no_room(4), invalid).await;
                    assert!(waiting == serial, "{at}: {waiting:?}");
                    match (&serial, &unlimited) {
                        (Ok(serial), Ok(unlimited)) => {
                            assert!(!serial.is_empty(), "{at}");
                            assert_eq!(rows_of(serial), rows_of(unlimited), "{at}");
                        }
                        (serial, unlimited) => assert_eq!(serial, unlimited, "{at}"),
                    }
                }
            }
        }
    }
}

/// **A parse holding a field one reader cannot hold is refused for memory**
/// (`docs/design/decisions.md`, "D161"): a zero-padded `bigint` a data-level
/// parse collects whole, its reads far shorter, is read under a floor of its
/// length and refused under one far below it, naming where it starts, what
/// it needs and the allowance — while a metadata-level parse, streaming the
/// row, reads it under that floor and under one below a read.
#[tokio::test]
async fn a_parse_holding_a_field_past_its_floor_is_refused_for_memory() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("long.sql");
    let long = format!("{}1", "0".repeat(4 << 10));
    let before = "CREATE TABLE public.t (\n    a bigint\n);\n\nCOPY public.t (a) FROM stdin;\n1\n";
    std::fs::write(&dump, format!("{before}{long}\n2\n\\.\n\n")).unwrap();
    let (chunk, default) = (64, PostgresInvalidValues::Default);

    let read = outcome(&dump, chunk, &floored(8 << 10), default).await.unwrap();
    assert_eq!(read.iter().map(|block| block.record.row_count).sum::<u64>(), 3);

    let floor = 1 << 10;
    let read = ReadOptions::default().with_chunk_size(chunk);
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&dump).unwrap());
    let opened = CacheMode::DISABLED.open_source(source).await.unwrap();
    let (data, cancel) = (&StatisticsRequest::DATA, CancellationToken::new());
    let arrangement = floored(floor);
    let refused = map_file(opened, read, &arrangement, default, data, &cancel).await.unwrap_err();
    let pgdt_core::Error::LineOutgrowsMemory { offset, needs, holds, beside, allowance } = refused
    else {
        panic!("not refused for memory: {refused}");
    };
    assert_eq!(offset, before.len() as u64, "where the field starts");
    assert!(needs > floor && needs <= long.len() as u64 + 1, "{needs}");
    assert_eq!((holds, beside, allowance), (floor, 0, 1 << 30));

    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&dump).unwrap());
    let opened = CacheMode::DISABLED.open_source(source).await.unwrap();
    let metadata = &StatisticsRequest::METADATA;
    let run = map_file(opened, read, &arrangement, default, metadata, &cancel).await.unwrap();
    assert_eq!(run.index.blocks().map(|block| block.record.row_count).sum::<u64>(), 3);

    // Its streamed row is drawn for nothing, not even as its carry starts
    // streaming it, so a floor below one read holds it too.
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&dump).unwrap());
    let opened = CacheMode::DISABLED.open_source(source).await.unwrap();
    let under = floored(1);
    let run = map_file(opened, read, &under, default, metadata, &cancel).await.unwrap();
    assert_eq!(run.index.blocks().map(|block| block.record.row_count).sum::<u64>(), 3);
}

/// A plain file stating that each of its workers holds [`PARKED_TERM`]
/// bytes, so a reader parked gives that back to the ledger, and counting
/// the readers parked on it, now and at most at once.
struct Parks {
    inner: LocalFileSource,
    parked: AtomicIsize,
    most: AtomicIsize,
}

/// What each of [`Parks`]'s workers is stated to hold.
const PARKED_TERM: u64 = 64 << 20;

impl Parks {
    fn open(dump: &std::path::Path) -> Arc<Self> {
        let inner = LocalFileSource::open(dump).unwrap();
        Arc::new(Self { inner, parked: AtomicIsize::new(0), most: AtomicIsize::new(0) })
    }
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
    /// Forwarded, or the leader declines every region and no reader is
    /// ever later than another.
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
        let now = self.parked.fetch_add(if parked { 1 } else { -1 }, Ordering::SeqCst);
        self.most.fetch_max(now + 1, Ordering::SeqCst);
        self.inner.hint_parked(parked);
    }
}

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

/// **A parse whose readers park maps what one reader maps** (D163): every
/// block of every major's `types` fixture, read in reads so short that nearly
/// every row outgrows its chunk, by a default and a strict parse at four
/// workers whose parking gives their terms back to a ledger with no room —
/// each line held alone past it, the readers after it parked — maps to the
/// blocks one reader maps, readers parking along the way and none left
/// parked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parse_parking_its_readers_maps_what_one_reader_does() {
    for version in VERSIONS {
        let fixture = types_fixture(version, "default");
        for invalid in [PostgresInvalidValues::Default, PostgresInvalidValues::Strict] {
            let at = format!("{version}: {fixture:?}, {invalid:?}");
            let serial = outcome(&fixture, 61, &no_room(1), invalid).await;
            let source = Parks::open(&fixture);
            let arrangement = parking(source.as_ref(), 4);
            let shared: Arc<dyn ByteRangeSource> = Arc::clone(&source) as _;
            let parked = outcome_over(shared, 61, &arrangement, invalid).await;
            assert!(parked == serial, "{at}: {parked:?}");
            assert!(source.most.load(Ordering::SeqCst) >= 1, "{at}: no reader parked");
            assert_eq!(source.parked.load(Ordering::SeqCst), 0, "{at}");
            assert_eq!(arrangement.ledger().parked(), 0, "{at}");
        }
    }
}

/// **A back-fill whose readers park checks what one reader checks** (D163):
/// each block of every major's `types` fixture, mapped at the metadata
/// level and so never checked, back-filled by a strict check at four workers
/// whose parking gives their terms back to a ledger with no room, in reads
/// so short that nearly every row outgrows its chunk, is checked in full and
/// still ends where its map says, readers parking along the way and none
/// left parked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_back_fill_parking_its_readers_checks_every_block() {
    let read = ReadOptions::default().with_chunk_size(61);
    let strict = PostgresInvalidValues::Strict;
    for version in VERSIONS {
        let fixture = types_fixture(version, "default");
        let source = Parks::open(&fixture);
        let shared: Arc<dyn ByteRangeSource> = Arc::clone(&source) as _;
        let opened = CacheMode::DISABLED.open_source(shared).await.unwrap();
        let (metadata, cancel) = (&StatisticsRequest::METADATA, CancellationToken::new());
        let default = PostgresInvalidValues::Default;
        let run = map_file(opened, read, &arranged(1), default, metadata, &cancel).await.unwrap();
        let arrangement = parking(source.as_ref(), 4);
        let dump_metadata = run.index.metadata.as_deref();
        for mut block in run.index.blocks().cloned() {
            let table = format!("{version}: {}", block.header.table);
            assert!(!block.record.checked_in_full, "{table}");
            let request = &StatisticsRequest::DATA;
            let filled = pgdt_fixtures::backfill_block(
                source.as_ref(),
                read,
                &arrangement,
                strict,
                request,
                dump_metadata,
                None,
                &mut block,
            )
            .await;
            assert!(filled.is_ok_and(|read| read), "{table}");
            assert!(block.record.checked_in_full, "{table}");
        }
        assert!(source.most.load(Ordering::SeqCst) >= 1, "{version}: no reader parked");
        assert_eq!(source.parked.load(Ordering::SeqCst), 0, "{version}");
        assert_eq!(arrangement.ledger().parked(), 0, "{version}");
    }
}
