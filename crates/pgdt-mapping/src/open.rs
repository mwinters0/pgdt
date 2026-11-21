//! **A `COPY` block opened in one place**: what each pass records of a block
//! and how it reads one, stated as that pass's policy, and turned into the
//! block's [`Recorder`] here (`docs/design/decisions.md`, "D170").

use pgdt_codec::internal::counter_for;
use pgdt_core::CopyHeader;
use pgdt_model::internal::Recorder;
use pgdt_model::{DumpMetadata, timescaledb};

use crate::context::Gathering;
use crate::statistics::gather;
use crate::statistics::request::StatisticsBackfill;

/// What a mapping pass records of each block it maps
/// (`docs/design/decisions.md`, "D77").
#[derive(Clone, Copy)]
pub enum Pass<'a> {
    /// [`map_file`](crate::map_file)'s: each block at the level the request
    /// gives its table, censused where that is the data level, and its
    /// data-level columns gathered, every observer charging the account and
    /// treating a field its type's `*_in` refuses as the gathering says
    /// ([`Gathering`]).
    Parse(Gathering<'a>),
    /// A query's: every block censused, nothing gathered, whatever level a
    /// parse would have given it (`docs/design/decisions.md`, "D35").
    Query,
}

/// What a read of a block the map already holds observes: the read its
/// caller decided to make, which the open turns into the block's observer.
#[derive(Clone, Copy)]
pub enum Reread<'a> {
    /// A back-fill's read gathering the plan's columns at its size
    /// (`StatisticsRequest::backfill`), charging the gathering's account,
    /// a field its type's `*_in` refuses met as the gathering says.
    Gathers(&'a StatisticsBackfill, Gathering<'a>),
    /// A back-fill's read gathering nothing and checking every field, as a
    /// strict parse does, for a block no strict parse checked
    /// (`docs/design/decisions.md`, "D103").
    Checks(Gathering<'a>),
    /// A query's census of a block mapped at the metadata level, observing
    /// nothing (`docs/design/decisions.md`, "D35").
    Censuses,
}

/// **What a pass records of each block it opens**, and so how it reads one:
/// the passes' differences as data, each the pass's own business.
pub(crate) enum Policy<'a> {
    /// A forward pass mapping the block, as `pass` says: [`Pass::Parse`]
    /// censuses it by its level and attaches the observer its request and
    /// mode give it, [`Pass::Query`] censuses it and gathers nothing. Both
    /// record TimescaleDB's catalog's rows, reading such a block on the
    /// pass's own loop, never offering it to the interior
    /// (`docs/design/decisions.md`, "D133").
    Maps(Pass<'a>),
    /// A block the map holds, read again: censused, observed as the read
    /// says — a back-fill's gatherer or checker, or nothing — its rows
    /// recorded by the passes that map it and never here (D133), and offered
    /// to the interior first.
    Rereads(Reread<'a>),
}

/// A block as [`open`] answers it: its recorder, and how the pass reads it.
pub(crate) struct OpenBlock {
    /// What records the block's rows, from its `CopyStart` to its `CopyEnd`.
    pub(crate) recorder: Recorder,
    /// Whether a row outgrowing its chunk is streamed
    /// (`pgdt_dump::ChunkEvents::stream_rows`): not where the recorder holds
    /// every row whole ([`Recorder::holds_rows`]).
    pub(crate) streams: bool,
    /// Whether the block's rows are offered to the interior split
    /// (`crate::interior::scan_region`).
    pub(crate) offers_interior: bool,
}

/// **Open the block whose `COPY` header is `header` and whose first data
/// byte is at `data_offset`** under `policy`: its census counted against the
/// DDL `metadata` states for it in `database`, its observer attached, its
/// rows recorded where the policy says, and whether it streams and is
/// offered to the interior derived from what it records.
pub(crate) fn open(
    header: &CopyHeader,
    data_offset: u64,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    policy: Policy<'_>,
) -> OpenBlock {
    let (census, observer, records) = match policy {
        Policy::Maps(Pass::Query) => (true, None, timescaledb::records_rows(header)),
        // A block no request tracks is observed too under
        // `PostgresInvalidValues::Strict`, to check its fields.
        Policy::Maps(Pass::Parse(gathering)) => (
            gathering.request.tracked_columns(header).is_some(),
            gather::observer_for(
                gathering.request,
                header,
                metadata,
                database,
                gathering.account,
                gathering.invalid,
            ),
            timescaledb::records_rows(header),
        ),
        Policy::Rereads(Reread::Gathers(plan, gathering)) => {
            let (account, invalid) = (gathering.account, gathering.invalid);
            let observer =
                gather::observer_tracking(plan, header, metadata, database, account, invalid);
            (true, Some(observer), false)
        }
        Policy::Rereads(Reread::Checks(gathering)) => {
            (true, Some(gather::checker(header, metadata, database, gathering.account)), false)
        }
        Policy::Rereads(Reread::Censuses) => (true, None, false),
    };
    let mut recorder = Recorder::new(data_offset);
    if census {
        let counter = counter_for(header, metadata, database);
        recorder = recorder.with_census(header.columns.len(), counter);
    }
    if let Some(observer) = observer {
        recorder = recorder.with_observer(observer);
    }
    if records {
        recorder = recorder.with_rows();
    }
    // A block whose rows are counted, censused and observed holds none of
    // them: a row outgrowing its chunk is streamed, its census and its
    // observer holding only the fields they read whole.
    let streams = !recorder.holds_rows();
    OpenBlock { recorder, streams, offers_interior: !records }
}
