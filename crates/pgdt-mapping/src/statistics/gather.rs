//! The statistics gatherer: `pgdt_model`'s `BlockObserver` implemented
//! above the codec, where a column's comparison is known
//! (`docs/design/decisions.md`, "D74").
//!
//! [`observer_for`] builds one per `COPY` block from the block's resolved
//! schema. Every tracked column counts its NULLs per group; a scalar column
//! also keeps per-group bounds and the block's row order in each order some
//! semantics compares it by exactly, under the key a filter orders by, a
//! float's zeros told apart ([`compare_keys_stored`](pgdt_codec::internal::compare_keys_stored)) — one set where the
//! orders coincide, two where PostgreSQL's is exact and Arrow's is another
//! (`pgdt_codec::ResolvedColumn::bounds_kinds`);
//! a column its comparison equates exactly keeps a dictionary per group; and a
//! column the typed read emits as an integer or a `Decimal128` keeps a sum per
//! group; and every column keeps its values' text bytes per group. **A column
//! whose type cannot hold every value counts those per group**, and its
//! bounds and row order are over the values it holds, each other view kept
//! apart only where a group holds one it reads otherwise
//! (`docs/design/decisions.md`, "D97"; [`KeyedGroup`](self::bounds::KeyedGroup), [`ViewOrders`](self::order::ViewOrders)).
//!
//! **A leader piece gathers into an observer of its own, and the pieces join
//! in file order into exactly what one observer handed every row gathers**
//! ([`Gatherer::join`](self::gatherer::Gatherer::join)), unless the block declines, which a stated allowance
//! decides over every piece charging at once (`docs/design/decisions.md`,
//! "D85"). Two things cross a join: the group a cut falls
//! inside, which a piece holds open rather than closing ([`Gatherer::head`](self::gatherer::Gatherer::head)),
//! and each ordered column's first value, which the rows before the piece
//! place their last value against ([`RowOrder`](self::order::RowOrder)).
//!
//! **Every observer charges what it holds to the pass's
//! [`StatisticsAccount`](super::account::StatisticsAccount)** (`docs/design/decisions.md`, "D81"): a block's
//! observer as [`Term::Gathering`](super::account::Term::Gathering), a piece's as [`Term::Pieces`](super::account::Term::Pieces), each
//! dictionary's interning map as [`Term::Interned`](super::account::Term::Interned). What it holds is
//! recomputed, O(1) a column, whenever its rows' growth passes
//! [`STATISTICS_ACCOUNT_CHARGE_STEP`](super::account::STATISTICS_ACCOUNT_CHARGE_STEP), as each column closes a group and through a piece's fold;
//! a vector or a map is charged ahead of the allocation growing it.
//!
//! **A block past its cap merges its closed groups pairwise into exactly what
//! gathering at twice the size gathers** (`docs/design/decisions.md`, "D82"),
//! never mid-scan while a piece it made is alive; at the block's end it merges
//! whatever is outstanding, a piece alive there joining nothing more
//! ([`Gatherer::fit_cap`](self::gatherer::Gatherer::fit_cap)). **A finished
//! block short of its density minimum merges the same way**
//! (`docs/design/decisions.md`, "D82"; [`Gatherer::fit_density`](self::gatherer::Gatherer::fit_density)).
//!
//! **A block whose gathering passes the pass's statistics allowance declines**
//! ([`Gatherer::decline`](self::gatherer::Gatherer::decline)): it frees what it holds, reads its remaining rows
//! for the census alone — and a strict parse's check ([`checker`]) — and
//! answers [`BlockGathered::Declined`](pgdt_model::BlockGathered::Declined) with the
//! allowance, which the map records (`docs/design/decisions.md`, "D85"). The
//! close `Gatherer::finish` makes is kept even where it passes the
//! allowance; the next block declines at its first charge.

use std::sync::Arc;

use pgdt_codec::internal::column_tiers;
use pgdt_core::CopyHeader;
use pgdt_core::internal::StatisticsScope;
use pgdt_model::{DumpMetadata, GroupSizing, PostgresInvalidValues, internal::BlockObserver};

use self::column::ColumnGatherer;
use self::gatherer::{Gatherer, Sizing};
use self::strict::field_checks;
use super::account::{Charge, StatisticsAccount, Term};
use super::request::{ROW_GROUP_DEFAULT_SIZE_BYTES, StatisticsBackfill, StatisticsRequest};
use super::stored::{declared_column, stored_resolution};

mod bounds;
mod charge;
mod clip;
mod column;
mod gatherer;
mod order;
pub(crate) mod strict;

#[cfg(feature = "test-support")]
pub use self::bounds::one_group_bounds;

/// The observer for one block, charging what it holds to `account`, or
/// `None` when `request` tracks nothing in it — but under
/// [`PostgresInvalidValues::Strict`], when it is the block's [`checker`].
///
/// Resolved against an empty census: the census moves only an array column,
/// and a nested column gets neither bounds nor a dictionary.
pub(crate) fn observer_for(
    request: &StatisticsRequest,
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    account: &Arc<StatisticsAccount>,
    invalid: PostgresInvalidValues,
) -> Option<Box<dyn BlockObserver>> {
    match request.tracked_columns(header) {
        Some(tracked) => {
            let gathering = request.gathering(tracked.clone(), tracked);
            Some(observer_tracking(&gathering, header, metadata, database, account, invalid))
        }
        None if invalid == PostgresInvalidValues::Strict => {
            Some(checker(header, metadata, database, account))
        }
        None => None,
    }
}

/// The observer for one block gathering `plan`'s columns — positional to
/// `header` — at its group size, merged pairwise past its cap and short of its
/// minimum: what [`observer_for`] builds from a request, and what a back-fill
/// builds from [`StatisticsRequest::backfill`]'s answer. `invalid` says
/// whether a field its type's `*_in` refuses stops the block, in the columns
/// the plan's request tracks; in the rest it is gone past and recorded
/// ([`StatisticsBackfill::requested`]) — but under
/// [`PostgresInvalidValues::Strict`], which checks every field of every
/// column first, as [`checker`] does, whether its statistics decline or not.
pub(crate) fn observer_tracking(
    plan: &StatisticsBackfill,
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    account: &Arc<StatisticsAccount>,
    invalid: PostgresInvalidValues,
) -> Box<dyn BlockObserver> {
    let _attributed = StatisticsScope::enter();
    // Opened before anything the observer allocates, and charged once the
    // resolution's scratch is freed, so neither update reads either as held.
    let charge = Charge::new(Arc::clone(account), Term::Gathering);
    let qualified = header.qualified_name();
    let resolved = stored_resolution(header, metadata, database);
    let checks = (invalid == PostgresInvalidValues::Strict)
        .then(|| field_checks(header, metadata, database, &resolved));
    let tiers = column_tiers(&resolved);
    let columns: Vec<Option<ColumnGatherer>> = header
        .columns
        .iter()
        .enumerate()
        .map(|(i, name)| {
            plan.columns.get(i).is_some_and(|&t| t).then(|| {
                let def = declared_column(metadata, database, &qualified, name);
                let record = &resolved.columns()[i];
                let mut column = ColumnGatherer::new(
                    def.map(|d| d.declared_type.clone()),
                    def.and_then(|d| d.collation.clone()),
                    record.bounds_kinds(),
                    record.comparison(),
                    record.plan(),
                    record.data_type(),
                    tiers[i].clone(),
                    if plan.requested.get(i).copied().unwrap_or(false) {
                        invalid
                    } else {
                        PostgresInvalidValues::Ignore
                    },
                );
                column.not_null = record.not_null();
                column
            })
        })
        .collect();
    drop((resolved, tiers, qualified));
    let mut gatherer = Gatherer::block(Sizing::of(plan), columns, charge);
    gatherer.checks = checks;
    gatherer.charge_held();
    // An account already full declines this block before its first row, so a
    // pass past its allowance gathers no row of any block after it
    // (`docs/design/decisions.md`, "D85").
    gatherer.decline_if_over();
    Box::new(gatherer)
}

/// **An observer gathering nothing that checks every field of the block** as
/// a strict parse does ([`PostgresInvalidValues::Strict`](pgdt_model::PostgresInvalidValues::Strict);
/// `docs/design/decisions.md`, "D103"): what a block whose
/// columns a request tracks none of is observed by, and what re-reads a block
/// the cache holds that no strict parse checked. It answers
/// [`BlockGathered::Checked`](pgdt_model::BlockGathered::Checked), or [`BlockGathered::Refused`](pgdt_model::BlockGathered::Refused) at the first
/// field its type's `*_in` refuses or NULL its column's `NOT NULL` refuses,
/// and never declines, holding nothing. What it holds to check — a
/// comparison plan per column — is charged to no account (D81): fixed per
/// block, shared with the block's pieces and freed with the observer.
pub(crate) fn checker(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    account: &Arc<StatisticsAccount>,
) -> Box<dyn BlockObserver> {
    let _attributed = StatisticsScope::enter();
    let charge = Charge::new(Arc::clone(account), Term::Gathering);
    let resolved = stored_resolution(header, metadata, database);
    let checks = field_checks(header, metadata, database, &resolved);
    drop(resolved);
    // No group is ever opened, so the size is never read.
    let sizing = Sizing {
        group_size: ROW_GROUP_DEFAULT_SIZE_BYTES,
        cap: None,
        min_rows: None,
        max_rows: None,
        record: GroupSizing::Stated,
    };
    let mut gatherer = Gatherer::block(sizing, Vec::new(), charge);
    gatherer.gathers = false;
    gatherer.checks = Some(checks);
    gatherer.charge_held();
    Box::new(gatherer)
}

#[cfg(test)]
mod tests;
