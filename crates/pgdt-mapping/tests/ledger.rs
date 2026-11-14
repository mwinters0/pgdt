//! What a parse's readers draw on its ledger: a line held past its chunk —
//! a field its census, its statistics or its check reads whole — waits
//! behind an earlier line, or is given up to one and read again, and a parse
//! answers what it answers drawing on no ledger (`docs/design/decisions.md`,
//! "D157").

use std::sync::Arc;

use pgdt_dump::ReadOptions;
use pgdt_dump::cache::CacheMode;
use pgdt_fixtures::{VERSIONS, statistics_fixture, types_fixture};
use pgdt_io::{Allowance, Arrangement, ArrangementRequest, ByteRangeSource, Jobs, LocalFileSource};
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
    let read = ReadOptions { chunk_size_bytes: chunk, ..ReadOptions::default() };
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(dump).unwrap());
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
        let checked = b.checked_in_full;
        format!("{:?} {} {:?} {checked}", b.census, b.row_count, b.longest_line)
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
/// row, reads it under that floor.
#[tokio::test]
async fn a_parse_holding_a_field_past_its_floor_is_refused_for_memory() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("long.sql");
    let long = format!("{}1", "0".repeat(4 << 10));
    let before = "CREATE TABLE public.t (\n    a bigint\n);\n\nCOPY public.t (a) FROM stdin;\n1\n";
    std::fs::write(&dump, format!("{before}{long}\n2\n\\.\n\n")).unwrap();
    let (chunk, default) = (64, PostgresInvalidValues::Default);

    let read = outcome(&dump, chunk, &floored(8 << 10), default).await.unwrap();
    assert_eq!(read.iter().map(|block| block.row_count).sum::<u64>(), 3);

    let floor = 1 << 10;
    let read = ReadOptions { chunk_size_bytes: chunk, ..ReadOptions::default() };
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&dump).unwrap());
    let opened = CacheMode::DISABLED.open_source(source).await.unwrap();
    let (data, cancel) = (&StatisticsRequest::DATA, CancellationToken::new());
    let arrangement = floored(floor);
    let refused = map_file(opened, read, &arrangement, default, data, &cancel).await.unwrap_err();
    let pgdt_core::Error::LineOutgrowsMemory { offset, needs, holds, allowance } = refused else {
        panic!("not refused for memory: {refused}");
    };
    assert_eq!(offset, before.len() as u64, "where the field starts");
    assert!(needs > floor && needs <= long.len() as u64 + 1, "{needs}");
    assert_eq!((holds, allowance), (floor, 1 << 30));

    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&dump).unwrap());
    let opened = CacheMode::DISABLED.open_source(source).await.unwrap();
    let metadata = &StatisticsRequest::METADATA;
    let run = map_file(opened, read, &arrangement, default, metadata, &cancel).await.unwrap();
    assert_eq!(run.index.blocks().map(|block| block.row_count).sum::<u64>(), 3);
}
