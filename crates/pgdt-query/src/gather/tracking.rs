//! Which columns a block's observer tracks and how each is read: the
//! observer a request builds for a block, and what a column's statistics are
//! stored against.

use std::sync::Arc;

use arrow::datatypes::DataType;

use pgdt_codec::compare::{CompareKind, ComparisonPlan, ComparisonSemantics};
use pgdt_codec::internal::column_tiers;
use pgdt_codec::pgtype::NestedPlan;
use pgdt_codec::{ResolvedSchema, SchemaMode, resolve_columns};
use pgdt_core::CopyHeader;
use pgdt_core::internal::StatisticsScope;
use pgdt_model::{ColumnDef, DumpMetadata, PostgresInvalidValues, internal::BlockObserver};

use super::column::{ColumnGatherer, Summand};
use super::gatherer::{Gatherer, Sizing};
use super::strict::{checker, field_checks};
use crate::statistics::{Charge, StatisticsAccount, StatisticsBackfill, StatisticsRequest, Term};

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
                let mut column = ColumnGatherer::new(
                    def.map(|d| d.declared_type.clone()),
                    def.and_then(|d| d.collation.clone()),
                    resolved.bounds_kinds(i),
                    &resolved.comparisons[i],
                    &resolved.plans[i],
                    resolved.schema.field(i).data_type(),
                    tiers[i].clone(),
                    if plan.requested.get(i).copied().unwrap_or(false) {
                        invalid
                    } else {
                        PostgresInvalidValues::Ignore
                    },
                );
                column.not_null = resolved.not_null[i];
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

/// Which of `header`'s columns gathering keeps bounds for, positionally, under
/// the DDL `metadata` states for it — what a block's held statistics are read
/// against to find a column gathered by a build that kept none
/// ([`crate::StatisticsRequest::backfill`]).
pub(crate) fn bounded_columns(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
) -> Vec<bool> {
    stored_bounds_kinds(header, metadata, database).iter().map(|kinds| kinds[0].is_some()).collect()
}

/// The kinds each of `header`'s columns' stored sets of bounds are ordered
/// by, positionally, resolved as [`observer_tracking`] resolves them — so for
/// a column whose statistics record the declared type and collation
/// `metadata` states now, the kinds its sets were gathered under
/// (`crate::prune`; `docs/design/decisions.md`, "D79").
pub(crate) fn stored_bounds_kinds(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
) -> Vec<[Option<CompareKind>; 2]> {
    let resolved = stored_resolution(header, metadata, database);
    (0..header.columns.len()).map(|i| resolved.bounds_kinds(i)).collect()
}

/// `header`'s columns resolved as [`observer_tracking`] resolves them, under
/// the DDL `metadata` states for it now: for a column whose statistics record
/// that declared type and collation, the resolution they were gathered under.
pub(crate) fn stored_resolution(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
) -> ResolvedSchema {
    resolve_columns(
        &header.qualified_name(),
        &header.columns,
        metadata,
        database,
        SchemaMode::Typed,
        &[],
    )
}

/// Whether gathering keeps column `i` of `stored` a dictionary whose entries
/// are each field's own text, as [`ColumnGatherer::new`] and
/// [`DictionaryGatherer::observe`](super::dictionary::DictionaryGatherer::observe) decide it: every dictionary kept but a
/// `character` column's, whose entries give up their trailing blanks.
pub(crate) fn dictionary_holds_field_text(stored: &ResolvedSchema, i: usize) -> bool {
    let comparison = &stored.comparisons[i];
    stored.plans[i].is_scalar()
        && comparison.dictionary_answers_in(ComparisonSemantics::Postgres)
        && !matches!(
            comparison,
            ComparisonPlan::Compared { kind: CompareKind::PaddedText { .. }, .. }
        )
}

/// Whether gathering keeps sums for a column the typed read emits as
/// `data_type` ([`pgdt_model::ColumnStatistics::sums`]) — and so
/// whether a column a query emits so is the one they were summed as.
pub(crate) fn keeps_sums(data_type: &DataType) -> bool {
    Summand::of(data_type, &NestedPlan::Scalar).is_some()
}

/// The declaration `metadata` holds for column `name` of the table
/// `qualified` in `database`, wherever the table takes it from
/// ([`pgdt_model::DatabaseMetadata::declared_column`]) — what a column's
/// statistics record their declared type and collation from, and what a
/// query compares those against (`crate::prune`).
pub(crate) fn declared_column<'m>(
    metadata: Option<&'m DumpMetadata>,
    database: Option<&str>,
    qualified: &str,
    name: &str,
) -> Option<&'m ColumnDef> {
    metadata
        .and_then(|m| m.databases.iter().find(|db| db.name.as_deref() == database))
        .and_then(|db| db.declared_column(qualified, name))
}
