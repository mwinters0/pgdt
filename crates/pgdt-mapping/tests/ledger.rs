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
