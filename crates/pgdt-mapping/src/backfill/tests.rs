use super::*;
use std::num::NonZeroU64;

use pgdt_dump::cache::{CacheMode, colocated_path};
use pgdt_fixtures::{all_fixtures, refused_field, statistics_fixture};
use pgdt_io::{
    Allowance, ArrangementRequest, DEFAULT_MEMORY_BUDGET, Extent, Jobs, Ledger, LocalFileSource,
    Parallelism,
};
use pgdt_model::internal::{ColumnStatisticsExt, ColumnStatisticsParts};
use pgdt_model::{ColumnStatistics, GroupSizing};

use crate::map_file;
use crate::statistics::request::{
    BLOCK_MAX_ROW_GROUPS, ROW_GROUP_DEFAULT_MIN_ROWS, StatisticsSelection, StatisticsSizing,
};
use crate::statistics::stored::bounded_columns;

/// Every table and column at the data level, gathered at `group_size`.
fn stated(group_size: u64) -> StatisticsRequest {
    let sizing = StatisticsSizing::Stated(NonZeroU64::new(group_size).unwrap());
    StatisticsRequest { selection: StatisticsSelection::DATA, sizing }
}

/// `source` mapped whole under `request`, with no cache.
async fn mapped(source: Arc<dyn ByteRangeSource>, request: &StatisticsRequest) -> DumpIndex {
    let opened = CacheMode::DISABLED.open_source(source).await.unwrap();
    let cancel = CancellationToken::new();
    let serial =
        Arrangement::resolve(&Allowance::NoneFound, ArrangementRequest::recommended(1, None));
    let invalid = PostgresInvalidValues::Default;
    map_file(opened, ReadOptions::default(), &serial, invalid, request, &cancel)
        .await
        .unwrap()
        .index
}

/// One read of `block` gathering what `plan` names, under `options` and no
/// allowance: what a back-fill's read of it gathers.
async fn gathered_at(
    source: &dyn ByteRangeSource,
    options: ScanOptions,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    block: &CopyBlock,
    plan: &StatisticsBackfill,
) -> BlockStatistics {
    let (cancel, account, ledger) = (CancellationToken::new(), Arc::default(), Ledger::unlimited());
    let extent = source.extent().await.unwrap();
    let mut reader = Rereads {
        read: ReadContext { source, scan: options, ledger: &ledger, cancel: &cancel, extent },
        cache_path: None,
        gathering: Gathering {
            request: &StatisticsRequest::DATA,
            account: &account,
            invalid: PostgresInvalidValues::Default,
        },
        loaded: HashSet::new(),
        shortfall_reported: false,
    };
    let reread = reader.reread(metadata, database, block, Some(plan)).await.unwrap();
    reread
        .expect("nothing fires the token")
        .observed
        .gathered
        .gathered()
        .expect("an unbounded allowance declines nothing")
}

/// A scan reading 64 bytes at a time, serially.
fn serial() -> ScanOptions {
    ScanOptions { read: ReadOptions::default().with_chunk_size(64), ..ScanOptions::default() }
}

/// **A block past its cap gathers, split by the leader, what the serial pass
/// gathers, and that is what gathering exactly at the size it reaches
/// gathers** — every block of every fixture, re-read at a group size of a few
/// bytes under a cap of two groups, so most blocks merge, at chunks the
/// leader cuts inside a group. What reaches the cap
/// under a request is [`StatisticsRequest::backfill`]'s answer, asserted
/// first.
#[tokio::test]
async fn every_fixture_block_past_its_cap_gathers_what_its_final_size_gathers() {
    const BASE: u64 = 8;
    const CAP: usize = 2;
    let unstated = StatisticsRequest::DATA;
    let stated = stated(BASE);
    let (mut blocks, mut merged) = (0, 0);
    for fixture in all_fixtures() {
        let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&fixture).unwrap());
        let index = mapped(source.clone(), &StatisticsRequest::METADATA).await;
        let metadata = index.metadata.as_deref();
        let refused = refused_field(&fixture).map(|r| r.table);
        // A block holding a field PostgreSQL refuses is refused by every
        // re-read (`pgdt-query`'s `tests/decode.rs`,
        // `every_refused_field_fails_a_data_level_parse_reading_it`).
        for (database, block) in index
            .blocks_with_database()
            .filter(|(_, b)| refused != Some(b.header.qualified_name().as_str()))
        {
            let backfill = unstated
                .backfill(block, metadata, database, None)
                .expect("the block holds no statistics");
            assert_eq!(backfill.group_cap, Some(BLOCK_MAX_ROW_GROUPS));
            assert_eq!(stated.backfill(block, metadata, database, None).unwrap().group_cap, None);
            let capped = StatisticsBackfill {
                group_size: BASE,
                group_cap: Some(CAP),
                min_rows: None,
                ..backfill
            };
            let gather = |options: ScanOptions, plan: StatisticsBackfill| {
                let source = source.as_ref();
                async move { gathered_at(source, options, metadata, database, block, &plan).await }
            };
            let reference = gather(serial(), capped.clone()).await;
            let label = format!("{}: {}", fixture.display(), block.header.table);
            assert!(reference.groups.len() <= CAP, "{label}");
            let exact = StatisticsBackfill {
                group_size: reference.group_size,
                group_cap: None,
                ..capped.clone()
            };
            assert_eq!(gather(serial(), exact).await, reference, "{label}");
            for jobs in [3, 8] {
                let parallelism = Parallelism::workers(jobs, DEFAULT_MEMORY_BUDGET);
                let parallel = ScanOptions { parallelism, ..serial() };
                let split = gather(parallel, capped.clone()).await;
                assert_eq!(split, reference, "{label}: {jobs} jobs");
            }
            blocks += 1;
            merged += usize::from(reference.group_size > BASE);
        }
    }
    assert!(merged * 2 > blocks, "only {merged} of {blocks} blocks merged");
}

/// The merges the density minimum must choose over a block gathered exactly
/// at its base size, written as `scripts/row_density.py` chooses: the first
/// size, from the base to a single group, whose upper middle group holds
/// `min_rows`, else the single group.
fn chosen_merges(rows: &[u64], min_rows: u64) -> u32 {
    let mut level = rows.to_vec();
    let mut merges = 0;
    loop {
        let mut sorted = level.clone();
        sorted.sort_unstable();
        if level.len() <= 1 || sorted[sorted.len() / 2] >= min_rows {
            return merges;
        }
        level = level.chunks(2).map(|pair| pair.iter().sum()).collect();
        merges += 1;
    }
}

/// **A block sized by its density minimum gathers, split by the leader, what
/// the serial pass gathers, at the size the rows its base size gathered
/// choose, and that is what gathering exactly at that size gathers** — every
/// block of every fixture, re-read at a group size of a few bytes under a
/// minimum of a few rows, so most blocks coarsen. What sizes a block under a
/// request is [`StatisticsRequest::backfill`]'s answer, asserted first.
#[tokio::test]
async fn every_fixture_block_sized_by_its_minimum_gathers_what_its_final_size_gathers() {
    const BASE: u64 = 8;
    const MIN_ROWS: u64 = 3;
    let unstated = StatisticsRequest::DATA;
    let stated = stated(BASE);
    let (mut blocks, mut coarsened) = (0, 0);
    for fixture in all_fixtures() {
        let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&fixture).unwrap());
        let index = mapped(source.clone(), &StatisticsRequest::METADATA).await;
        let metadata = index.metadata.as_deref();
        let refused = refused_field(&fixture).map(|r| r.table);
        // A block holding a field PostgreSQL refuses is refused by every
        // re-read (`pgdt-query`'s `tests/decode.rs`,
        // `every_refused_field_fails_a_data_level_parse_reading_it`).
        for (database, block) in index
            .blocks_with_database()
            .filter(|(_, b)| refused != Some(b.header.qualified_name().as_str()))
        {
            let backfill = unstated
                .backfill(block, metadata, database, None)
                .expect("the block holds no statistics");
            let default_sizing =
                GroupSizing::Density { min_rows: ROW_GROUP_DEFAULT_MIN_ROWS, max_rows: None };
            assert_eq!(backfill.min_rows, Some(ROW_GROUP_DEFAULT_MIN_ROWS));
            assert_eq!(backfill.sizing, default_sizing);
            let exact_request = stated.backfill(block, metadata, database, None).unwrap();
            assert_eq!((exact_request.min_rows, exact_request.sizing), (None, GroupSizing::Stated));
            let sized = StatisticsBackfill {
                group_size: BASE,
                group_cap: None,
                min_rows: Some(MIN_ROWS),
                ..backfill
            };
            let gather = |options: ScanOptions, plan: StatisticsBackfill| {
                let source = source.as_ref();
                async move { gathered_at(source, options, metadata, database, block, &plan).await }
            };
            let label = format!("{}: {}", fixture.display(), block.header.table);
            let reference = gather(serial(), sized.clone()).await;
            assert_eq!(reference.sizing, default_sizing, "{label}: the record is the plan's");
            let exact =
                |group_size| StatisticsBackfill { group_size, min_rows: None, ..sized.clone() };
            let base = gather(serial(), exact(BASE)).await;
            let rows: Vec<u64> = base.groups.iter().map(|g| g.rows).collect();
            let merges = chosen_merges(&rows, MIN_ROWS);
            assert_eq!(reference.group_size, BASE << merges, "{label}: {rows:?}");
            assert_eq!(gather(serial(), exact(reference.group_size)).await, reference, "{label}");
            for jobs in [3, 8] {
                let parallelism = Parallelism::workers(jobs, DEFAULT_MEMORY_BUDGET);
                let parallel = ScanOptions { parallelism, ..serial() };
                let split = gather(parallel, sized.clone()).await;
                assert_eq!(split, reference, "{label}: {jobs} jobs");
            }
            blocks += 1;
            coarsened += usize::from(merges > 0);
        }
    }
    assert!(coarsened * 2 > blocks, "only {coarsened} of {blocks} blocks coarsened");
}

/// **A held column lacks bounds only where gathering keeps them**
/// (`docs/design/decisions.md`, "D79"): every column of `ordered` is bounded,
/// as its text where no DDL declares it; with one column's bounds stripped,
/// the block lacks them and is re-read at the size and under the record it
/// holds, and where no column is bounded it lacks nothing.
#[tokio::test]
async fn a_held_column_lacks_bounds_only_where_gathering_keeps_them() {
    let dump = statistics_fixture(16, "default");
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&dump).unwrap());
    let index = mapped(source, &StatisticsRequest::DATA).await;
    let metadata = index.metadata.as_deref();
    let ordered = index.blocks().find(|b| b.header.matches("public.ordered")).unwrap();
    let bounded = bounded_columns(ordered, metadata, None);
    assert!(bounded.iter().all(|&b| b), "every column of `ordered` is bounded: {bounded:?}");
    assert_eq!(bounded_columns(ordered, None, None), bounded, "no DDL, bounded as its text");
    let request = StatisticsRequest::DATA;
    assert_eq!(
        request.backfill(ordered, metadata, None, None),
        None,
        "as gathered, it lacks nothing"
    );

    let mut unbounded = ordered.clone();
    let mut held = BlockStatistics::clone(unbounded.record.statistics.as_deref().unwrap());
    let column = held.columns[11].as_mut().unwrap();
    *column = ColumnStatistics::from_parts(ColumnStatisticsParts {
        bounds: None,
        ..column.clone().into_parts()
    });
    Arc::make_mut(&mut unbounded.record).statistics = Some(Arc::new(held.clone()));
    let backfill = request
        .backfill(&unbounded, metadata, None, None)
        .expect("a bounded column held without bounds lacks them");
    assert_eq!((backfill.group_size, backfill.sizing), (held.group_size, held.sizing));
    assert_eq!(
        request.backfill_bounded(&unbounded, None, || vec![false; bounded.len()]),
        None,
        "a column bounded nowhere lacks nothing"
    );
}

/// A [`ByteRangeSource`] answering a read at or past `trip` as a source whose
/// wait is a request answers a cancellation: it fires `cancel` and **drops the
/// read**, failing it with [`Error::ScanCancelled`] instead of handing back
/// bytes (`docs/design/decisions.md`, "D26").
struct DropsPast {
    inner: Arc<dyn ByteRangeSource>,
    trip: u64,
    cancel: CancellationToken,
}

impl ByteRangeSource for DropsPast {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bytes::Bytes>> + Send + '_>>
    {
        if offset >= self.trip {
            self.cancel.cancel();
            let dropped = Err(Error::ScanCancelled { scanned_through: offset });
            return Box::pin(std::future::ready(dropped));
        }
        self.inner.read_range(offset, len)
    }

    fn extent(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Extent>> + Send + '_>> {
        self.inner.extent()
    }

    fn modified(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<std::time::SystemTime>>> + Send + '_>,
    > {
        self.inner.modified()
    }

    /// The three the leader reads, forwarded so the re-read is cut as it would
    /// be through the file itself: the default `partitions` declines to advise,
    /// and a source that declines is never split.
    fn hint_read_size(&self, len: usize) {
        self.inner.hint_read_size(len);
    }

    fn partitions(&self, range: std::ops::Range<u64>) -> pgdt_io::Partitioning {
        self.inner.partitions(range)
    }

    fn hint_parallelism(&self, parallelism: Parallelism) {
        self.inner.hint_parallelism(parallelism);
    }
}

/// **A read the leader dispatched, dropped by a cancellation, interrupts the
/// back-fill as the mapping pass's does** (`docs/design/decisions.md`, "D26"):
/// a block long enough to split is mapped at the metadata level, then asked
/// for its statistics at four jobs over a source dropping every read of its
/// rows, the first of them a worker's. The run is interrupted, not an
/// [`Error::ScanCancelled`]; its cache stays the whole map it was, and the
/// next run back-fills the block.
#[tokio::test]
async fn a_dropped_read_the_leader_dispatched_interrupts_the_backfill() {
    let mut file = b"COPY public.t (a) FROM stdin;\n".to_vec();
    let data_offset = file.len() as u64;
    for i in 0..2000 {
        file.extend_from_slice(format!("{i}\n").as_bytes());
    }
    file.extend_from_slice(b"\\.\n\nSELECT 1;\n");
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("dump.sql");
    std::fs::write(&dump, &file).unwrap();
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&dump).unwrap());
    let mode = CacheMode::enabled(colocated_path(&dump));
    let invalid = PostgresInvalidValues::Default;
    let serial =
        Arrangement::resolve(&Allowance::NoneFound, ArrangementRequest::recommended(1, None));
    let parallel = Arrangement::resolve(
        &Allowance::NoneFound,
        ArrangementRequest { jobs: Jobs::Stated(4), ..ArrangementRequest::recommended(4, None) },
    );
    let (read, never) = (ReadOptions::default(), CancellationToken::new());
    let opened = mode.open_source(Arc::clone(&source)).await.unwrap();
    let bare = map_file(opened, read, &serial, invalid, &StatisticsRequest::METADATA, &never)
        .await
        .unwrap();
    assert!(!bare.interrupted && bare.index.is_complete(file.len() as u64));

    let cancel = CancellationToken::new();
    let dropping =
        DropsPast { inner: Arc::clone(&source), trip: data_offset, cancel: cancel.clone() };
    let opened = mode.open_source(Arc::new(dropping)).await.unwrap();
    let chunked = ReadOptions::default().with_chunk_size(64);
    let run = map_file(opened, chunked, &parallel, invalid, &StatisticsRequest::DATA, &cancel)
        .await
        .expect("a dropped read is an interrupted run, not an error");
    assert!(run.interrupted, "a cancelled back-fill says so");
    assert_eq!((run.lacking_statistics, run.backfilled), (1, 0));
    assert!(run.index.is_complete(file.len() as u64), "the map is whole; the re-read is not");

    let opened = mode.open_source(Arc::clone(&source)).await.unwrap();
    let resumed =
        map_file(opened, read, &serial, invalid, &StatisticsRequest::DATA, &never).await.unwrap();
    assert!(!resumed.interrupted);
    assert_eq!((resumed.resumed_from, resumed.backfilled), (file.len() as u64, 1));
    assert!(resumed.index.blocks().all(|block| block.record.statistics.is_some()));
}
