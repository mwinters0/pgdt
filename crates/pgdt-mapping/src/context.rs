//! What a mapping pass runs under, stated once: the source and how it is
//! read, the ledger and the token its readers share, where the file ends,
//! where the pass saves, and what a parse gathers under.

use std::sync::Arc;

use pgdt_core::Result;
use pgdt_dump::ScanOptions;
use pgdt_dump::cache::{CacheMode, SourceWatch};
use pgdt_io::{ByteRangeSource, Extent, Ledger};
use pgdt_model::{DumpIndex, PostgresInvalidValues};
use tokio_util::sync::CancellationToken;

use crate::statistics::account::StatisticsAccount;
use crate::statistics::request::StatisticsRequest;

/// What every reader of a pass reads under: the source, the read options
/// and parallelism, the run's ledger, which each reader draws a line held
/// past its chunk from (`docs/design/decisions.md`, "D157"), the caller's ask
/// that it stop (`docs/design/decisions.md`, "D26"), and where the file ends,
/// where the source has said so.
///
/// **The half of a [`PassContext`] a read needs**, apart because a block read
/// again outside a pass ([`backfill_one`](crate::internal::backfill_one)) reads
/// under one and saves nowhere.
#[derive(Clone, Copy)]
pub struct ReadContext<'a> {
    /// The source every reader reads.
    pub source: &'a dyn ByteRangeSource,
    /// The chunk every read loop reads with, and the concurrency the leader
    /// may use.
    pub scan: ScanOptions,
    /// The run's ledger.
    pub ledger: &'a Ledger,
    /// The caller's ask that the pass stop, polled once per chunk, leader
    /// window and closed block.
    pub cancel: &'a CancellationToken,
    /// Where the file ends, or that it is not known yet.
    pub extent: Extent,
}

impl<'a> ReadContext<'a> {
    /// This context with the file ending at `extent`: a back-fill run once the
    /// map has reached the end reads to the end the map found.
    pub(crate) fn ending_at(self, extent: Extent) -> Self {
        Self { extent, ..self }
    }
}

/// A mapping pass's context: what its readers read under, and the mode and
/// watch it saves through, the watch being the open's, which every save an
/// enabled mode writes checks the source against ([`CacheMode::save`]).
#[derive(Clone, Copy)]
pub struct PassContext<'a> {
    /// What every reader of the pass reads under.
    pub read: ReadContext<'a>,
    /// The mode every save goes to, whose path a moved block's refusal
    /// names.
    pub cache: &'a CacheMode,
    /// The open's watch on the source.
    pub watch: &'a SourceWatch,
}

impl<'a> PassContext<'a> {
    /// The source the pass reads.
    pub(crate) fn source(&self) -> &'a dyn ByteRangeSource {
        self.read.source
    }

    /// The caller's ask that the pass stop.
    pub(crate) fn cancel(&self) -> &'a CancellationToken {
        self.read.cancel
    }

    /// This context with the file ending at `extent` ([`ReadContext::ending_at`]).
    pub(crate) fn ending_at(self, extent: Extent) -> Self {
        Self { read: self.read.ending_at(extent), ..self }
    }

    /// Persist `index` per the pass's mode, checking the source first where
    /// the mode writes ([`CacheMode::save`]).
    pub(crate) async fn save(&self, index: &DumpIndex) -> Result<()> {
        self.cache.save(self.watch, self.read.source, index).await
    }
}

/// What a parse's observers gather under: the levels its request gives each
/// table, the account every observer charges, bounded by the run's statistics
/// allowance, and what a field its type's `*_in` refuses does
/// ([`PostgresInvalidValues`]). A query's pass gathers nothing, so holds none
/// ([`Pass`](crate::open::Pass)).
#[derive(Clone, Copy)]
pub struct Gathering<'a> {
    pub(crate) request: &'a StatisticsRequest,
    pub(crate) account: &'a Arc<StatisticsAccount>,
    pub(crate) invalid: PostgresInvalidValues,
}

impl Gathering<'_> {
    /// The bytes every statistic alive may hold between them, where the run
    /// states an allowance: the account's bound.
    pub(crate) fn allowance(&self) -> Option<u64> {
        self.account.allowance()
    }

    /// Whether every field is checked ([`PostgresInvalidValues::Strict`]).
    pub(crate) fn strict(&self) -> bool {
        self.invalid == PostgresInvalidValues::Strict
    }
}
