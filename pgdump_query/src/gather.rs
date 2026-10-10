//! The statistics gatherer: [`crate::model::BlockObserver`] implemented
//! in query, where a column's comparison is known
//! (`docs/design/decisions.md`, "D74").
//!
//! [`observer_for`] builds one per `COPY` block from the block's resolved
//! schema. Every tracked column counts its NULLs per group; a scalar column
//! also keeps per-group bounds and the block's row order in each order some
//! semantics compares it by exactly, under the key a filter orders by, a
//! float's zeros told apart ([`ValueKey::stored_order`]) — one set where the
//! orders coincide, two where PostgreSQL's is exact and Arrow's is another
//! (`crate::ResolvedSchema::bounds_kinds`);
//! a column its comparison equates exactly keeps a dictionary per group; and a
//! column the typed read emits as an integer or a `Decimal128` keeps a sum per
//! group; and every column keeps its values' text bytes per group. **A column
//! whose type cannot hold every value counts those per group**, and its
//! bounds and row order are over the values it holds, each other view kept
//! apart only where a group holds one it reads otherwise
//! (`docs/design/decisions.md`, "D97"; [`KeyedGroup`], [`ViewOrders`]).
//!
//! **A leader piece gathers into an observer of its own, and the pieces join
//! in file order into exactly what one observer handed every row gathers**
//! ([`Gatherer::join`]), unless the block declines, which a stated allowance
//! decides over every piece charging at once (`docs/design/decisions.md`,
//! "D85"). Two things cross a join: the group a cut falls
//! inside, which a piece holds open rather than closing ([`Gatherer::head`]),
//! and each ordered column's first value, which the rows before the piece
//! place their last value against ([`RowOrder`]).
//!
//! **Every observer charges what it holds to the pass's
//! [`StatisticsAccount`]** (`docs/design/decisions.md`, "D81"): a block's
//! observer as [`Term::Gathering`], a piece's as [`Term::Pieces`], each
//! dictionary's interning map as [`Term::Interned`]. What it holds is
//! recomputed, O(1) a column, whenever its rows' growth passes
//! [`STATISTICS_ACCOUNT_CHARGE_STEP`], as each column closes a group and through a piece's fold;
//! a vector or a map is charged ahead of the allocation growing it.
//!
//! **A block past its cap merges its closed groups pairwise into exactly what
//! gathering at twice the size gathers** (`docs/design/decisions.md`, "D82"),
//! never mid-scan while a piece it made is alive; at the block's end it merges
//! whatever is outstanding, a piece alive there joining nothing more
//! ([`Gatherer::fit_cap`]). **A finished
//! block short of its density minimum merges the same way**
//! (`docs/design/decisions.md`, "D82"; [`Gatherer::fit_density`]).
//!
//! **A block whose gathering passes the pass's statistics allowance declines**
//! ([`Gatherer::decline`]): it frees what it holds, reads its remaining rows
//! for the census alone — and a strict parse's check ([`checker`]) — and
//! answers [`BlockGathered::Declined`] with the
//! allowance, which the map records (`docs/design/decisions.md`, "D85"). The
//! close [`Gatherer::finish`] makes is kept even where it passes the
//! allowance; the next block declines at its first charge.

use std::any::Any;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::mem::{self, align_of, size_of};
use std::sync::{Arc, OnceLock};

use arrow::datatypes::DataType;

use crate::copy::{CopyHeader, decode_field, split_fields};
use crate::decode::{
    Unread, bytea_unread, char_typmod_refuses, decode_bytea, decode_bytea_escape, render_bytea,
    typmod_unscaled_digits,
};
use crate::instrument::StatisticsScope;
use crate::key::{ValueKey, field_refused, grammar_refuses};
use crate::model::{
    BlockGathered, BlockObserver, BlockStatistics, Bounds, BoundsView, ColumnBounds, ColumnDef,
    ColumnDictionary, ColumnStatistics, CopyBlock, DICTIONARY_ENTRY_MAX_BYTES,
    DICTIONARY_MAX_ENTRIES, DumpMetadata, FieldRefusal, GroupSizing, IgnoredRefusals,
    PostgresInvalidValues, RowGroup, Sortedness, Unrepresentable, UnrepresentableTier,
    max_rows_group, min_rows_group, text_heap, vec_heap,
};
use crate::pgtype::{
    CompareKind, ComparisonPlan, ComparisonSemantics, NestedPlan, TextGrammar, Unchecked,
    column_grammar, unchecked_positions,
};
use crate::resolve::{ResolvedSchema, SchemaMode, database_for_name, resolve_columns};
use crate::statistics::{
    Charge, ROW_GROUP_DEFAULT_SIZE_BYTES, STATISTICS_ACCOUNT_CHARGE_STEP, StatisticsAccount,
    StatisticsBackfill, StatisticsRequest, Term,
};
use crate::unrepresentable::{ColumnTier, column_tiers};

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

/// **An observer gathering nothing that checks every field of the block** as
/// a strict parse does ([`PostgresInvalidValues::Strict`];
/// `docs/design/decisions.md`, "D103"): what a block whose
/// columns a request tracks none of is observed by, and what re-reads a block
/// the cache holds that no strict parse checked. It answers
/// [`BlockGathered::Checked`], or [`BlockGathered::Refused`] at the first
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

/// What a strict parse checks each of `header`'s columns by, positionally:
/// its comparison under `resolved`, which is what names the type a field is
/// read as ([`field_refused`]), or for a column ordered by nothing its
/// declared type's input grammar ([`text_grammar`]), and whether its
/// declaration makes it `NOT NULL`; `None` for a column read as no type and
/// declared nullable.
fn field_checks(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    resolved: &ResolvedSchema,
) -> Arc<[Option<FieldCheck>]> {
    let qualified = header.qualified_name();
    let types =
        metadata.and_then(|m| database_for_name(m, database)).map_or(&[][..], |db| &db.types[..]);
    header
        .columns
        .iter()
        .zip(resolved.comparisons.iter().zip(&resolved.not_null))
        .map(|(name, (plan, &not_null))| {
            let declared_type = declared_column(metadata, database, &qualified, name)
                .map(|def| def.declared_type.clone())
                .unwrap_or_default();
            let grammar = column_grammar(plan, &declared_type, types);
            let reads = grammar.is_some()
                || !matches!(plan, ComparisonPlan::Refused | ComparisonPlan::Unanswerable(_));
            (reads || not_null).then(|| FieldCheck {
                plan: plan.clone(),
                grammar,
                declared_type,
                reads,
                not_null,
            })
        })
        .collect()
}

/// **What a strict parse checks nothing of in a `COPY` block** of a table
/// ([`strict_unchecked`]): what PostgreSQL refuses there is decided by
/// something besides the field and its column's declaration, or no reader
/// here reads it — the boundary a clean strict parse's promise stops at,
/// named per dump, since nothing else lets that promise be checked against
/// one (`docs/design/roadmap.md`, "A literal is guaranteed in `*_out`'s form
/// and never read past `*_in`'s").
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct StrictUnchecked {
    /// Whether the dump declares the table. Where it does not, as in a
    /// `--data-only` dump, no column has a type or a `NOT NULL` to check by,
    /// so no field of it is checked and `columns` lists none.
    pub declared: bool,
    /// Each column, or position within one, whose fields a strict parse
    /// checks for nothing, in column order and each column's in walk order.
    pub columns: Vec<UncheckedColumn>,
    /// The `CHECK` constraints a restore evaluates against each row, its own
    /// and those it inherits.
    pub checks: Vec<UncheckedCheck>,
    /// The partition bounds a restore checks each row against where the
    /// block loads into a partition: its own, then each ancestor's that is a
    /// partition too (I86).
    pub bounds: Vec<UncheckedBound>,
    /// The partitioned table a restore routes each row through to the
    /// partition whose bound admits it, refusing one none admits: the
    /// block's table, where it carries `pg_dump`'s `load via partition root`
    /// marker or the preamble declares a partition of it (I86).
    pub routed_through: Option<String>,
}

impl StrictUnchecked {
    /// Whether a strict parse checks everything a restore refuses a row by
    /// in this block but its post-data constraints.
    pub fn is_empty(&self) -> bool {
        self.declared
            && self.columns.is_empty()
            && self.checks.is_empty()
            && self.bounds.is_empty()
            && self.routed_through.is_none()
    }
}

/// One position a strict parse checks nothing of ([`StrictUnchecked`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UncheckedColumn {
    pub column: String,
    /// The position within the column's value, spelled as
    /// [`crate::NestedCompare::uncomparable`] spells one — `[]` an element,
    /// `.name` a field, `.bound` a range's bound — and empty for the value.
    pub path: String,
    /// The type declared at that position, as the dump wrote it; empty for
    /// a column declared nowhere.
    pub declared: String,
    #[serde(flatten)]
    pub why: Unchecked,
}

/// A `CHECK` constraint a restore evaluates against each row of a table
/// ([`StrictUnchecked`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UncheckedCheck {
    /// Its name, `None` for one written without.
    pub name: Option<String>,
    /// The ancestor declaring it, `None` for the table's own.
    pub inherited_from: Option<String>,
}

/// A partition bound a restore checks each row of a block against
/// ([`StrictUnchecked`]): an expression over the partition key, compared
/// under its operator classes and collations.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UncheckedBound {
    /// The partition it bounds: the block's table, or an ancestor of it.
    pub partition: String,
    /// The partitioned table it is a partition of.
    pub parent: String,
    /// Its text ([`crate::model::PartitionOf::bound`]), `None` where the
    /// preamble names the parent and the bound is not read.
    pub bound: Option<String>,
}

/// What a strict parse checks nothing of in `block`, read off the preamble
/// in `metadata` for the block's database as the parse's own check is
/// ([`field_checks`]): the same resolution, the same plan and grammar per
/// column, so the two cannot disagree.
///
/// A property of the dump's declarations and the block's own framing, not of
/// any run: the same for a block a strict parse checked and one it never
/// read.
pub fn strict_unchecked(block: &CopyBlock, metadata: Option<&DumpMetadata>) -> StrictUnchecked {
    let header = &block.header;
    let database = block.database.as_deref();
    let qualified = header.qualified_name();
    let db = metadata.and_then(|m| database_for_name(m, database));
    let routed =
        block.partition_root.is_some() || db.is_some_and(|db| db.has_partitions(&qualified));
    let routed_through = routed.then(|| qualified.clone());
    let Some(db) = db.filter(|db| db.tables.contains_key(&qualified)) else {
        return StrictUnchecked { routed_through, ..StrictUnchecked::default() };
    };
    let resolved = stored_resolution(header, metadata, database);
    let mut columns = Vec::new();
    for (name, plan) in header.columns.iter().zip(&resolved.comparisons) {
        let Some(def) = db.declared_column(&qualified, name) else {
            columns.push(UncheckedColumn {
                column: name.clone(),
                path: String::new(),
                declared: String::new(),
                why: Unchecked::Undeclared,
            });
            continue;
        };
        let grammar = column_grammar(plan, &def.declared_type, &db.types);
        columns.extend(
            unchecked_positions(&def.declared_type, plan, grammar, &db.types).into_iter().map(
                |(path, declared, why)| UncheckedColumn {
                    column: name.clone(),
                    path,
                    declared,
                    why,
                },
            ),
        );
    }
    let checks = db
        .table_checks(&qualified)
        .into_iter()
        .map(|(check, from)| UncheckedCheck {
            name: check.name.clone(),
            inherited_from: from.map(str::to_string),
        })
        .collect();
    let bounds = db
        .partition_bounds(&qualified)
        .into_iter()
        .map(|(partition, of)| UncheckedBound {
            partition: partition.to_string(),
            parent: of.parent.clone(),
            bound: of.bound.clone(),
        })
        .collect();
    StrictUnchecked { declared: true, columns, checks, bounds, routed_through }
}

/// One column's part of a strict parse's check ([`field_checks`]).
struct FieldCheck {
    plan: ComparisonPlan,
    /// Read in place of `plan`, which then orders nothing.
    grammar: Option<TextGrammar>,
    declared_type: String,
    /// Whether a value is checked, by `grammar` or `plan`.
    reads: bool,
    /// Whether a NULL is refused (I76).
    not_null: bool,
}

/// The first field of the row at `offset`, the `line`-th the observer was
/// handed, that its column's check finds refused, in column order.
fn check_row(
    checks: &[Option<FieldCheck>],
    offset: u64,
    line: u64,
    raw: &[u8],
) -> Option<FieldRefusal> {
    for (column, (field, check)) in split_fields(raw).zip(checks).enumerate() {
        let Some(check) = check else { continue };
        // A NULL is refused only by a `NOT NULL`, and a field that is not
        // text is no value at all, which no `*_in` is handed.
        let value = match decode_field(field) {
            Ok(None) if check.not_null => None,
            Ok(Some(text)) if check.reads => {
                let refused = match check.grammar {
                    Some(grammar) => grammar_refuses(grammar, &text),
                    None => field_refused(&check.plan, &text),
                };
                if !refused {
                    continue;
                }
                Some(text.into_owned())
            }
            _ => continue,
        };
        return Some(FieldRefusal {
            offset,
            line,
            column,
            declared_type: check.declared_type.clone(),
            value,
        });
    }
    None
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
/// [`DictionaryGatherer::observe`] decide it: every dictionary kept but a
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
/// `data_type` ([`crate::model::ColumnStatistics::sums`]) — and so
/// whether a column a query emits so is the one they were summed as.
pub(crate) fn keeps_sums(data_type: &DataType) -> bool {
    Summand::of(data_type, &NestedPlan::Scalar).is_some()
}

/// The declaration `metadata` holds for column `name` of the table
/// `qualified` in `database`, wherever the table takes it from
/// ([`crate::model::DatabaseMetadata::declared_column`]) — what a column's
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

/// A field PostgreSQL refuses — one its type's `*_in` refuses, met while
/// keying it ([`BoundsGatherer::observe`]), or a NULL its column's `NOT NULL`
/// refuses — or one a strict parse's check refuses.
#[derive(Debug)]
struct Refused;

/// What [`ColumnGatherer::observe`] answers of a field it read.
struct Observed {
    /// What the column's heap grew by.
    grew: i64,
    /// Whether the field is one its type's `*_in` refuses, kept out of the
    /// group's statistics rather than stopping the block.
    ignored: bool,
}

/// The group a row is being added to.
struct OpenGroup {
    index: u64,
    first_start: u64,
    rows: u64,
}

/// How a block observer sizes its groups: a [`StatisticsBackfill`] less its
/// columns.
#[derive(Clone, Copy)]
struct Sizing {
    group_size: u64,
    cap: Option<usize>,
    min_rows: Option<u64>,
    max_rows: Option<u64>,
    record: GroupSizing,
}

impl Sizing {
    fn of(plan: &StatisticsBackfill) -> Self {
        let (group_size, cap) = (plan.group_size, plan.group_cap);
        let (min_rows, max_rows) = (plan.min_rows, plan.max_rows);
        Self { group_size, cap, min_rows, max_rows, record: plan.sizing }
    }
}

struct Gatherer {
    group_size: u64,
    /// The most groups a block holds, `None` under a stated size or maximum
    /// and for a piece, which never merges.
    cap: Option<usize>,
    /// The fewest rows the finished block's median group is merged *towards*,
    /// `None` for an exact size and for a piece, which is never finished. Not
    /// a floor: the merging stops at one group, or where the next size would
    /// break `max_rows`, whichever comes first ([`density_merges`]).
    min_rows: Option<u64>,
    /// The most rows a merge here may put in the finished block's
    /// 90th-percentile group; `None` for an exact size and for a piece. Not a
    /// ceiling: a block already past it at the size it gathered from merges
    /// nothing, where its groups pair evenly ([`density_merges`]), and
    /// finishes over it, the re-read in
    /// [`crate::statistics::StatisticsRequest::backfill`] being what brings it
    /// back within one.
    max_rows: Option<u64>,
    /// What the finished block records it was sized under.
    record: GroupSizing,
    /// Shared with every piece this block has made, once it has made one:
    /// mid-scan a block merges only while no piece holds a clone, so each
    /// piece joins at the size it gathered at. At the block's end the check
    /// is dropped, a piece still alive being past joining
    /// ([`Gatherer::fit_cap`]).
    pieces: OnceLock<Arc<()>>,
    /// Whether this observes a piece of its block rather than the whole of it:
    /// a piece lists no group ahead of its first row's, holds that group open
    /// once a row moves past it, and is joined rather than finished.
    piece: bool,
    /// Every closed group in order, from the block's first — or, in a piece,
    /// from the one after [`Self::head`].
    groups: Vec<RowGroup>,
    /// A piece's first group once a later row has moved past it, with where
    /// its last row ends. **Held open, not closed**: the rows before the piece
    /// may have begun it, and only [`Gatherer::join`] knows. Each column's
    /// state over it is [`ColumnGatherer::head`].
    head: Option<(OpenGroup, u64)>,
    open: Option<OpenGroup>,
    /// Whether any column is tracked, so a row is worth splitting.
    splits: bool,
    /// Whether this observer gathers statistics at all, which a [`checker`]
    /// does not: it opens no group, never declines, and answers
    /// [`BlockGathered::Checked`].
    gathers: bool,
    columns: Vec<Option<ColumnGatherer>>,
    /// What a strict parse checks every field by, one per column, where this
    /// observer checks them ([`PostgresInvalidValues::Strict`]): each row is
    /// checked before it is gathered, and after a decline too.
    checks: Option<Arc<[Option<FieldCheck>]>>,
    /// What the open groups have grown by since [`Self::charge_held`] last
    /// ran, which it runs again once this passes [`STATISTICS_ACCOUNT_CHARGE_STEP`].
    uncharged: i64,
    /// Whether the account stood over its allowance at the last charge update
    /// — read at a row's end and at a fold's, never mid-merge, so an observer
    /// declines only where its structures are whole.
    over: bool,
    /// What a piece [`Self::join`] is folding in still holds, which this
    /// observer's charge carries until the fold ends.
    carried: (u64, u64),
    /// The allowance this observer **declined** under, once the account passed
    /// it: everything gathered is freed, no further row is gathered, and a block
    /// answers [`BlockGathered::Declined`]
    /// (`docs/design/decisions.md`, "D85"). A piece carries it into the block
    /// it folds into, the block having lost that piece's rows.
    declined: Option<u64>,
    /// The first field this observer met that its type's `*_in` refuses: it
    /// frees what it holds and reads no further row, as a declined observer
    /// does, and a block answers [`BlockGathered::Refused`]. A piece carries
    /// it into the block it folds into.
    refused: Option<FieldRefusal>,
    /// The fields its type's `*_in` refuses that this observer keyed and went
    /// past, told to ignore them. A piece's is folded into its block's, its
    /// first numbered past the block's rows, as a refusal is.
    ignored: Option<IgnoredRefusals>,
    /// The rows this observer has been handed, and, in a block, those of
    /// every piece it has folded in, **counted whether or not it has
    /// stopped**: a piece made before a decline can still refuse, and its
    /// line is numbered past every row before it ([`FieldRefusal::line`]).
    rows: u64,
    /// What this observer holds, in the pass's account. **Last**, so it is
    /// released after everything above is freed ([`Charge`]).
    charge: Charge,
}

impl Gatherer {
    fn block(sizing: Sizing, columns: Vec<Option<ColumnGatherer>>, charge: Charge) -> Self {
        Self {
            group_size: sizing.group_size,
            cap: sizing.cap.map(|cap| cap.max(1)),
            min_rows: sizing.min_rows,
            max_rows: sizing.max_rows,
            record: sizing.record,
            pieces: OnceLock::new(),
            piece: false,
            groups: Vec::new(),
            head: None,
            open: None,
            splits: columns.iter().any(Option::is_some),
            gathers: true,
            columns,
            checks: None,
            uncharged: 0,
            over: false,
            carried: (0, 0),
            declined: None,
            refused: None,
            ignored: None,
            rows: 0,
            charge,
        }
    }

    /// Whether this observer has stopped gathering rows, having declined or
    /// refused; one that declined still checks them where it checks any.
    fn stopped(&self) -> bool {
        self.declined.is_some() || self.refused.is_some()
    }

    /// Free everything this observer holds, its charge going to nothing in
    /// the same update: what [`Self::decline`] and [`Self::refuse`] share.
    fn free(&mut self) {
        debug_assert_eq!(self.carried, (0, 0), "a fold is not a place to stop");
        drop(mem::take(&mut self.columns));
        drop(mem::take(&mut self.groups));
        (self.head, self.open, self.splits) = (None, None, false);
        self.uncharged = 0;
        self.charge.set(0, 0);
    }

    /// **Stop at a field PostgreSQL refuses**: the pass fails on it, so what
    /// was gathered is freed rather than finished, and no later row is read
    /// for statistics (`roadmap.md`, "A literal is guaranteed in `*_out`'s
    /// form and never read past `*_in`'s").
    fn refuse(&mut self, refusal: FieldRefusal) {
        self.refused = Some(refusal);
        self.free();
    }

    /// **Drop everything this observer holds and gather no more**, the account
    /// having passed its allowance: the block's groups and columns are freed,
    /// its charge goes to nothing in the same update, and the allowance it
    /// declined under is what the map records
    /// (`crate::model::CopyBlock::statistics_declined`). Called with the
    /// caller's [`StatisticsScope`] open, so what it frees is attributed as
    /// what allocated it was.
    ///
    /// **Nothing else declines with it**: the scan goes on, and a piece still
    /// gathering is freed as it is folded in or dropped
    /// (`docs/design/decisions.md`, "D85").
    ///
    /// deficiency: KD33 — the account this tests is cumulative, and nothing
    /// releases `Term::Retained` while the pass gathers forward (the back-fill
    /// does, replacing a block's statistics, `crate::stream::backfill_statistics`), so
    /// once a long dump's retained statistics reach the allowance every block
    /// from there on declines on its first charge update. Statistics are then a *prefix* of
    /// the file rather than a sample of it, and a query prunes nothing over
    /// the tail; how much is covered depends on the allowance the box
    /// resolved. Coarsening under that pressure is refused by "D85" — a cache
    /// would depend on its container — so the remedy owned by P23 is a
    /// granularity derived from the dump's length, which is known up front and
    /// is the same on every machine.
    fn decline(&mut self, allowance: u64) {
        self.declined = Some(allowance);
        self.free();
    }

    /// Decline where the last charge update found the account over its
    /// allowance, which [`Self::charge_held`] records.
    fn decline_if_over(&mut self) {
        if self.over && !self.stopped() && self.gathers {
            let allowance = self.charge.allowance().expect("only an allowance can be passed");
            self.decline(allowance);
        }
    }

    /// The heap this observer holds, as its structure and its interning maps.
    fn held(&self) -> (u64, u64) {
        let own = vec_heap(&self.groups) + vec_heap(&self.columns);
        self.columns
            .iter()
            .flatten()
            .map(ColumnGatherer::held)
            .fold((own, 0), |(structure, interned), (s, i)| (structure + s, interned + i))
    }

    /// Charge what this observer holds now to the account, with what a piece
    /// being folded in still holds.
    fn charge_held(&mut self) {
        let (structure, interned) = self.held();
        self.over = self.charge.set(structure + self.carried.0, interned + self.carried.1);
        self.uncharged = 0;
    }

    /// Close the open group, its last row's line ending at `end`, and list an
    /// empty group for every index short of `next`. **The rows' growth is
    /// charged first**, since each column's close moves the charge by its own
    /// difference alone.
    ///
    /// **A block past its cap merges as the groups close**, wherever their
    /// count is past the cap and even and so every pair is whole, `next`
    /// halving with them — so a long row lists at most one group past the
    /// cap, except while a piece this block made is alive, when nothing merges
    /// before [`Gatherer::fit_cap`].
    fn close_through(&mut self, end: u64, next: u64) {
        if self.uncharged != 0 {
            self.charge_held();
        }
        let mut next = next;
        let mut at = match self.open.take() {
            Some(group) => {
                let mut after = group.index + 1;
                if self.piece && self.head.is_none() {
                    for column in self.columns.iter_mut().flatten() {
                        column.hold_head();
                    }
                    self.head = Some((group, end));
                } else {
                    self.push_closed(group, end);
                    self.merge_whole_pairs(&mut after, &mut next);
                }
                after
            }
            // A piece before its first row lists nothing ahead of it.
            None if self.piece => next,
            None => 0,
        };
        while at < next {
            push_charged(&mut self.groups, RowGroup { rows: 0, bytes: 0 }, &mut self.charge);
            for column in self.columns.iter_mut().flatten() {
                column.close_group(&mut self.charge);
            }
            at += 1;
            self.merge_whole_pairs(&mut at, &mut next);
        }
        self.charge_held();
    }

    /// The cap this block merges past now: none for a piece or under a stated
    /// size or maximum,
    /// and none while a piece it made is alive.
    fn merging_cap(&self) -> Option<usize> {
        let alive = self.pieces.get().is_some_and(|pieces| Arc::strong_count(pieces) > 1);
        self.cap.filter(|_| !alive)
    }

    /// Merge the closed groups pairwise if they are past the cap and even in
    /// number, halving `at` and `next` — group indices — with them.
    fn merge_whole_pairs(&mut self, at: &mut u64, next: &mut u64) {
        let Some(cap) = self.merging_cap() else { return };
        let closed = self.groups.len();
        if closed > cap && closed.is_multiple_of(2) {
            self.merge_pairs();
            (*at, *next) = (*at / 2, *next / 2);
        }
    }

    /// **Merge this block's groups pairwise until they are within its cap.**
    /// Mid-block, a last closed group left without a pair is taken back into
    /// the open group that follows it, which is its pair ([`Self::reopen_last`]);
    /// `finishing`, no row follows, so it stands alone, and a piece still
    /// alive — past the block's end — joins nothing more. A block folding a
    /// window of pieces can pass its cap by the window's groups, which those
    /// pieces held already, until the last of them is folded.
    fn fit_cap(&mut self, finishing: bool) {
        let cap = if finishing { self.cap } else { self.merging_cap() };
        let Some(cap) = cap else { return };
        if self.groups.len() <= cap {
            return;
        }
        if self.uncharged != 0 {
            self.charge_held();
        }
        while self.groups.len() > cap {
            if !self.groups.len().is_multiple_of(2) && !finishing {
                self.reopen_last();
            }
            self.merge_pairs();
        }
    }

    /// **Merge a finished block's groups pairwise until its median group holds
    /// its minimum**, or it is one group, or the next merge would put more
    /// than its maximum in its 90th-percentile group — the smallest size, no
    /// finer than the one it holds, at which the upper middle group reaches
    /// the minimum, and never past the coarsest at which the maximum holds
    /// ([`density_merges`]). Run after [`Self::fit_cap`], from the size the
    /// cap left: the finer sizes' rows are summed as the cap merges, and the
    /// predicate is monotone in size, so the size the cap left is never
    /// coarsened past one the minimum would have chosen for itself.
    fn fit_density(&mut self) {
        let Some(min_rows) = self.min_rows else { return };
        let rows: Vec<u64> = self.groups.iter().map(|group| group.rows).collect();
        for _ in 0..density_merges(rows, min_rows, self.max_rows) {
            self.merge_pairs();
        }
    }

    /// Take the last closed group back into the open group following it, as
    /// though both groups' rows had been observed into one.
    fn reopen_last(&mut self) {
        let closed = self.groups.pop().expect("a block past its cap holds closed groups");
        let open = self.open.as_mut().expect("a block mid-scan holds an open group");
        debug_assert_eq!(open.index, self.groups.len() as u64 + 1, "the open group follows");
        open.index -= 1;
        // The closed group's bytes run from its first row to the open group's.
        open.first_start -= closed.bytes;
        open.rows += closed.rows;
        for column in self.columns.iter_mut().flatten() {
            column.reopen_last(&mut self.charge);
        }
    }

    /// Merge every closed group with the one after it, a last one alone, and
    /// double the group size: what gathering at twice the size would hold.
    fn merge_pairs(&mut self) {
        merge_adjacent(&mut self.groups, |a, b| RowGroup {
            rows: a.rows + b.rows,
            bytes: a.bytes + b.bytes,
        });
        for column in self.columns.iter_mut().flatten() {
            column.merge_pairs(&mut self.charge);
        }
        self.group_size *= 2;
        if let Some(open) = &mut self.open {
            debug_assert_eq!(open.index % 2, 0, "an open group's pair is still to come");
            open.index /= 2;
        }
        self.charge_held();
    }

    fn push_closed(&mut self, group: OpenGroup, end: u64) {
        let closed = RowGroup { rows: group.rows, bytes: end - group.first_start };
        push_charged(&mut self.groups, closed, &mut self.charge);
        for column in self.columns.iter_mut().flatten() {
            column.close_group(&mut self.charge);
        }
    }

    /// Fold `later`, a piece whose rows all follow this observer's in file
    /// order, into it — leaving it as it would be had it observed those rows
    /// itself. Its first group continues this one's open group where both hold
    /// the same index, and each ordered column steps from this observer's last
    /// value to the piece's first.
    ///
    /// **The piece's charge moves onto this observer's in one update**, and
    /// from there this observer charges what the piece still holds beside
    /// what it holds itself, as each part moves across or is freed: the
    /// account never reads the moved groups as held by neither or by both.
    /// What is left of the piece goes back to its own charge in one update,
    /// released as the piece is freed.
    /// Pieces are folded with none of them gathering (`crate::leader`), so no
    /// other observer updates the account while one is half moved.
    fn join(&mut self, mut later: Gatherer) {
        debug_assert!(!self.piece && later.piece, "a block observer joins its pieces");
        later.charge_held();
        self.charge.take_over(&mut later.charge);
        self.carried = later.held();
        self.charge_held();
        self.join_rows(&mut later);
        self.charge.hand_back(&mut later.charge, mem::take(&mut self.carried));
        drop(later);
        self.charge_held();
    }

    /// [`Self::join`] but for moving the charge, draining `later` in place so
    /// that what it has left is what [`Self::held`] reads of it.
    fn join_rows(&mut self, later: &mut Gatherer) {
        let (first, first_end) = match (later.head.take(), later.open.take()) {
            (Some((group, end)), open) => {
                later.open = open;
                (group, Some(end))
            }
            (None, Some(group)) => (group, None),
            // The piece held no row.
            (None, None) => return,
        };
        let continues = self.open.as_ref().is_some_and(|open| open.index == first.index);
        match &mut self.open {
            Some(open) if continues => open.rows += first.rows,
            _ => {
                self.close_through(first.first_start, first.index);
                self.open = Some(first);
            }
        }
        for (mine, theirs) in self.columns.iter_mut().zip(&mut later.columns) {
            let (Some(mine), Some(theirs)) = (mine, theirs) else { continue };
            let state = match first_end {
                Some(_) => theirs.head.take().expect("a piece past its first group holds it"),
                None => theirs.take_group(),
            };
            if continues {
                mine.group.absorb(state);
            } else {
                mine.group = state;
            }
            for (mine, theirs) in mine.bounds.iter_mut().zip(&mut theirs.bounds) {
                if let (Some(mine), Some(theirs)) = (mine, theirs) {
                    mine.rows.absorb(mem::take(&mut theirs.rows));
                }
            }
        }
        self.carried = later.held();
        self.charge_held();
        // The piece moved past its first group, so the group closes where the
        // piece's next row starts and the piece's own closed groups follow.
        let Some(end) = first_end else { return };
        let open = self.open.take().expect("the joined group is open");
        self.push_closed(open, end);
        reserve_charged(&mut self.groups, later.groups.len(), &mut self.charge);
        self.groups.extend_from_slice(&later.groups);
        later.groups = Vec::new();
        self.open = later.open.take();
        self.carried = later.held();
        self.charge_held();
        for at in 0..self.columns.len() {
            let theirs = later.columns.get_mut(at).and_then(Option::as_mut);
            if let (Some(mine), Some(theirs)) = (self.columns[at].as_mut(), theirs) {
                mine.append(theirs, &mut self.charge);
            }
        }
        self.carried = later.held();
        self.charge_held();
    }
}

/// Under the instrument, an observer's fields are dropped inside a statistics
/// scope, so what it frees is attributed as what it allocated was
/// (`crate::instrument`); its charge, declared last, is released after.
#[cfg(feature = "introspect")]
impl Drop for Gatherer {
    fn drop(&mut self) {
        let _attributed = StatisticsScope::enter();
        drop(mem::take(&mut self.columns));
        drop(mem::take(&mut self.groups));
        drop(self.pieces.take());
        (self.head, self.open) = (None, None);
    }
}

impl BlockObserver for Gatherer {
    fn observe_row(&mut self, offset: u64, raw: &[u8]) {
        self.rows += 1;
        if self.refused.is_some() {
            return;
        }
        // **Checked before it is gathered, and whether or not gathering has
        // declined**: a strict parse reads every field of every row.
        let line = self.rows;
        if let Some(refusal) = self.checks.as_deref().and_then(|c| check_row(c, offset, line, raw))
        {
            let _attributed = StatisticsScope::enter();
            return self.refuse(refusal);
        }
        if self.stopped() || !self.gathers {
            return;
        }
        let _attributed = StatisticsScope::enter();
        let index = offset / self.group_size;
        match &mut self.open {
            Some(group) if group.index == index => group.rows += 1,
            _ => {
                self.close_through(offset, index);
                // Closing may have merged the groups, coarsening the index.
                let index = offset / self.group_size;
                self.open = Some(OpenGroup { index, first_start: offset, rows: 1 });
                self.fit_cap(false);
            }
        }
        if !self.splits {
            self.decline_if_over();
            return;
        }
        for (at, (field, column)) in split_fields(raw).zip(self.columns.iter_mut()).enumerate() {
            let Some(column) = column else { continue };
            let refusal = |column: &ColumnGatherer| FieldRefusal {
                offset,
                line: self.rows,
                column: at,
                declared_type: column.declared_type.clone().unwrap_or_default(),
                value: decode_field(field).ok().flatten().map(Cow::into_owned),
            };
            match column.observe(field) {
                Ok(Observed { grew, ignored }) => {
                    self.uncharged += grew;
                    if ignored {
                        IgnoredRefusals::add(&mut self.ignored, refusal(column));
                    }
                }
                Err(Refused) => {
                    let refusal = refusal(column);
                    return self.refuse(refusal);
                }
            }
        }
        if self.uncharged >= STATISTICS_ACCOUNT_CHARGE_STEP as i64 {
            self.charge_held();
        }
        // **The row is whole before the decline**, so nothing is freed
        // half-observed; every path above that charges leaves `over` set for
        // it (`docs/design/decisions.md`, "D85").
        self.decline_if_over();
    }

    /// **The observer's charge becomes the block's retained bytes** once the
    /// interning maps are freed, in one update of the account — and a block
    /// that declined answers the allowance it declined under, holding nothing
    /// to retain.
    fn finish(mut self: Box<Self>, end: u64) -> BlockGathered {
        let _attributed = StatisticsScope::enter();
        debug_assert!(!self.piece, "a piece is joined, never finished");
        if let Some(refusal) = self.refused.take() {
            return BlockGathered::Refused(refusal);
        }
        if !self.gathers {
            return BlockGathered::Checked;
        }
        if let Some(allowance) = self.declined {
            return BlockGathered::Declined { allowance };
        }
        if let Some(group) = &self.open {
            let next = group.index + 1;
            self.close_through(end, next);
        }
        self.fit_cap(true);
        self.fit_density();
        let columns = mem::take(&mut self.columns);
        let statistics = BlockStatistics {
            group_size: self.group_size,
            sizing: self.record,
            groups: mem::take(&mut self.groups),
            columns: columns.into_iter().map(|c| c.map(ColumnGatherer::finish)).collect(),
        };
        self.charge.retain(statistics.heap_bytes());
        BlockGathered::Gathered(statistics)
    }

    fn piece(&self) -> Box<dyn BlockObserver> {
        let _attributed = StatisticsScope::enter();
        let columns = self.columns.iter().map(|c| c.as_ref().map(ColumnGatherer::fresh)).collect();
        let charge = Charge::new(Arc::clone(self.charge.account()), Term::Pieces);
        let sizing = Sizing {
            group_size: self.group_size,
            cap: None,
            min_rows: None,
            max_rows: None,
            record: self.record,
        };
        let mut piece = Gatherer::block(sizing, columns, charge);
        piece.piece = true;
        piece.gathers = self.gathers;
        piece.checks = self.checks.clone();
        // A piece of a block that has already stopped gathers nothing: it
        // still reads its rows for the census and the block's extent, and
        // holds no statistic while it does.
        piece.declined = self.declined;
        piece.refused = self.refused.clone();
        let pieces = self.pieces.get_or_init(Arc::default);
        piece.pieces = OnceLock::from(Arc::clone(pieces));
        piece.charge_held();
        Box::new(piece)
    }

    /// **A declined piece declines its block**, the block having lost that
    /// piece's rows and being unable to state statistics over the rest
    /// (`docs/design/decisions.md`, "D85"); a block that declined while the
    /// window ran drops every piece it is handed instead of folding it.
    ///
    /// **A refusal outranks a decline, and the earlier row's outranks a later
    /// one's**: a block that refused drops every piece, its rows all
    /// following the one it refused, and one that has not takes a piece's,
    /// its line numbered past this block's rows. Every other piece's rows are
    /// counted, folded or dropped.
    fn absorb(&mut self, later: Box<dyn BlockObserver>) {
        let _attributed = StatisticsScope::enter();
        let mut later = later.into_any().downcast::<Gatherer>().expect("a piece of this observer");
        if self.refused.is_some() {
            drop(later);
            return;
        }
        if let Some(mut refusal) = later.refused.take() {
            drop(later);
            refusal.line += self.rows;
            return self.refuse(refusal);
        }
        let mut ignored = later.ignored.take();
        for column in ignored.iter_mut().flat_map(|ignored| &mut ignored.columns) {
            column.first.line += self.rows;
        }
        IgnoredRefusals::fold(&mut self.ignored, ignored);
        self.rows += later.rows;
        if !self.gathers {
            return drop(later);
        }
        match (self.declined, later.declined) {
            (Some(_), _) => drop(later),
            (None, Some(allowance)) => {
                drop(later);
                self.decline(allowance);
            }
            (None, None) => {
                self.join(*later);
                self.fit_cap(false);
                self.decline_if_over();
            }
        }
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }

    fn take_ignored(&mut self) -> Option<IgnoredRefusals> {
        self.ignored.take()
    }

    fn checks_every_field(&self) -> bool {
        self.checks.is_some()
    }
}

/// How many times a block whose groups hold `rows` merges pairwise before its
/// **upper middle** group ([`min_rows_group`]) holds `min_rows`, or it is one
/// group: at each merge the rows per group are the sums of adjacent pairs, a
/// last odd group standing alone.
///
/// Reading the upper middle group rather than the nearest-rank `⌈G/2⌉`-th is
/// what makes the predicate **monotone in size**: a merged group falls short
/// only where both its parts did, so of `S ≤ ⌊G/2⌋` short groups at most
/// `⌊S/2⌋` pairs fall short, plus a short odd tail — within `⌊⌈G/2⌉/2⌋` at
/// every `G` (by cases on `G` mod 4), the same predicate at the coarser size.
/// `the_density_predicate_is_monotone_in_size` holds it.
///
/// **`max_rows` stops the merging first**, wherever the next size would put
/// more than that many rows in its 90th-percentile group ([`max_rows_group`]):
/// so a stated maximum outranks the minimum. Nothing here can make a block
/// *finer*: a block breaking `max_rows` at the size it gathered from is
/// re-read instead ([`StatisticsRequest::backfill`]).
///
/// deficiency: KD43 — the maximum's predicate is monotone in size only where
/// every group pairs. A last odd group stands alone, so a pairwise sum can
/// rank below the parts it replaces: at 19, 39, 59… groups a block already
/// past the maximum can merge once (`[10, 10, 0 × 17]` under a maximum of 5
/// becomes `[20, 0 × 9]`, whose 90th-percentile group is 0), and stopping at
/// the first merge that breaks the predicate can miss a coarser size that
/// would hold it. `an_unpaired_last_group_lets_a_block_past_the_maximum_merge`
/// pins the example. **(c) unowned**; promoted by a stated maximum seen to
/// leave a block's groups past it.
fn density_merges(mut rows: Vec<u64>, min_rows: u64, max_rows: Option<u64>) -> u32 {
    let mut merges = 0;
    while rows.len() > 1 && min_rows_group(&rows) < min_rows {
        let coarser: Vec<u64> = rows.chunks(2).map(|pair| pair.iter().sum()).collect();
        if max_rows.is_some_and(|max_rows| max_rows_group(&coarser) > max_rows) {
            break;
        }
        rows = coarser;
        merges += 1;
    }
    merges
}

/// How a column's values are summed: parsed as the typed read parses them
/// (`crate::batch`), and widened to the 128 bits every sum is kept in
/// (`docs/design/decisions.md`, "D91").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Summand {
    Int16,
    Int32,
    Int64,
    UInt32,
    /// A `numeric(p,s)`'s value put through its typmod, unscaled at `s`
    /// places.
    Decimal128 {
        precision: u16,
        scale: i8,
    },
}

impl Summand {
    /// How a column the typed read emits as `data_type`, filled as `plan`
    /// says, is summed, or `None` for a type that keeps no sum.
    fn of(data_type: &DataType, plan: &NestedPlan) -> Option<Self> {
        match data_type {
            DataType::Int16 => Some(Self::Int16),
            DataType::Int32 => Some(Self::Int32),
            DataType::Int64 => Some(Self::Int64),
            DataType::UInt32 => Some(Self::UInt32),
            DataType::Decimal128(arrow, scale) => Some(Self::Decimal128 {
                precision: match plan {
                    NestedPlan::Decimal { precision } => *precision,
                    _ => u16::from(*arrow),
                },
                scale: *scale,
            }),
            _ => None,
        }
    }

    /// `text` as the integer the typed read appends, or `None` where the read
    /// would refuse it.
    fn value(self, text: &str) -> Option<i128> {
        match self {
            Self::Int16 => text.parse::<i16>().ok().map(i128::from),
            Self::Int32 => text.parse::<i32>().ok().map(i128::from),
            Self::Int64 => text.parse::<i64>().ok().map(i128::from),
            Self::UInt32 => text.parse::<u32>().ok().map(i128::from),
            Self::Decimal128 { precision, scale } => {
                typmod_unscaled_digits(text, precision, i16::from(scale)).ok()?.parse().ok()
            }
        }
    }
}

struct ColumnGatherer {
    declared_type: Option<String>,
    collation: Option<String>,
    null_counts: Vec<u64>,
    /// How a value is summed, for a column keeping sums.
    summand: Option<Summand>,
    /// Each closed group's sum, `None` for a column keeping none and once a
    /// value has failed to decode ([`ColumnStatistics::sums`]).
    sums: Option<Vec<i128>>,
    /// Each closed group's text bytes ([`ColumnStatistics::value_bytes`]).
    value_bytes: Vec<u64>,
    /// Which tier a value is past, for a column whose type cannot hold every
    /// value its declared type can (`crate::unrepresentable`).
    tier: Option<ColumnTier>,
    /// Each closed group's count of such values, for a column `tier` tests
    /// ([`ColumnStatistics::unrepresentable`]).
    unrepresentable: Option<Vec<Unrepresentable>>,
    /// One per [`crate::model::BoundsSet`], in its order.
    bounds: [Option<BoundsGatherer>; 2],
    dictionary: Option<DictionaryGatherer>,
    /// The column over the open group.
    group: GroupState,
    /// The column over a piece's held first group ([`Gatherer::head`]).
    head: Option<GroupState>,
    /// Whether a field its type's `*_in` refuses stops the block, or is kept
    /// out of the group's statistics ([`Self::observe`]).
    invalid: PostgresInvalidValues,
    /// Whether the column's declaration makes it `NOT NULL`, so a NULL is
    /// refused as such a field is (I76).
    not_null: bool,
}

impl ColumnGatherer {
    /// A set of bounds under each of `bounds`, the kinds some semantics
    /// orders the column by exactly (`crate::ResolvedSchema::bounds_kinds`),
    /// and a dictionary where the comparison equates exactly
    /// (`docs/design/decisions.md`, "D79"); a sum per group where the typed
    /// read emits `data_type` as an integer or a `Decimal128`; a count per
    /// group of the values past `tier`, where the type cannot hold every one;
    /// and, as for every tracked column, its NULLs and text bytes per group
    /// ([`ColumnStatistics`]).
    #[allow(clippy::too_many_arguments)]
    fn new(
        declared_type: Option<String>,
        collation: Option<String>,
        bounds: [Option<CompareKind>; 2],
        comparison: &ComparisonPlan,
        plan: &NestedPlan,
        data_type: &DataType,
        tier: Option<ColumnTier>,
        invalid: PostgresInvalidValues,
    ) -> Self {
        let postgres = ComparisonSemantics::Postgres;
        let dictionary = match comparison {
            ComparisonPlan::Compared { kind, .. } if plan.is_scalar() => {
                comparison.dictionary_answers_in(postgres).then(|| DictionaryGatherer::new(kind))
            }
            _ => None,
        };
        let bounds = bounds.map(|kind| kind.map(BoundsGatherer::new));
        let summand = Summand::of(data_type, plan);
        Self::with(declared_type, collation, bounds, dictionary, summand, tier, invalid)
    }

    fn with(
        declared_type: Option<String>,
        collation: Option<String>,
        bounds: [Option<BoundsGatherer>; 2],
        dictionary: Option<DictionaryGatherer>,
        summand: Option<Summand>,
        tier: Option<ColumnTier>,
        invalid: PostgresInvalidValues,
    ) -> Self {
        let group = GroupState::fresh(&bounds);
        Self {
            declared_type,
            collation,
            null_counts: Vec::new(),
            summand,
            sums: summand.map(|_| Vec::new()),
            value_bytes: Vec::new(),
            unrepresentable: tier.as_ref().map(|_| Vec::new()),
            tier,
            bounds,
            dictionary,
            group,
            head: None,
            invalid,
            not_null: false,
        }
    }

    /// The heap this column holds, as its structure and its interning map,
    /// its open group and a held head included. O(1) but for a keyed bound's
    /// key, which [`DICTIONARY_ENTRY_MAX_BYTES`] bounds.
    fn held(&self) -> (u64, u64) {
        let named = self.declared_type.iter().chain(&self.collation).map(text_heap).sum::<u64>();
        let head = self.head.as_ref().map_or(0, GroupState::heap_bytes);
        let per_group = vec_heap(&self.null_counts)
            + self.sums.as_ref().map_or(0, vec_heap)
            + vec_heap(&self.value_bytes)
            + self.unrepresentable.as_ref().map_or(0, vec_heap);
        let mut structure = named + per_group + self.group.heap_bytes() + head;
        for bounds in self.bounds.iter().flatten() {
            structure += vec_heap(&bounds.groups)
                + vec_heap(&bounds.flags)
                + vec_heap(&bounds.tiers)
                + bounds.stored
                + bounds.rows.heap_bytes();
        }
        let mut interned = 0;
        if let Some(dictionary) = &self.dictionary {
            let (entries, map) = dictionary.held();
            structure += entries;
            interned = map;
        }
        (structure, interned)
    }

    /// The heap a row can change: the open group and the row order.
    fn open_heap(&self) -> u64 {
        self.group.heap_bytes()
            + self.bounds.iter().flatten().map(|b| b.rows.heap_bytes()).sum::<u64>()
    }

    /// A column of this one's kind that has gathered nothing: what a piece
    /// starts from.
    fn fresh(&self) -> Self {
        let fresh = Self::with(
            self.declared_type.clone(),
            self.collation.clone(),
            self.bounds.each_ref().map(|b| b.as_ref().map(BoundsGatherer::fresh)),
            self.dictionary.as_ref().map(DictionaryGatherer::fresh),
            self.summand,
            self.tier.clone(),
            self.invalid,
        );
        Self { not_null: self.not_null, ..fresh }
    }

    /// The open group's state, leaving a fresh one open.
    fn take_group(&mut self) -> GroupState {
        let fresh = GroupState::fresh(&self.bounds);
        mem::replace(&mut self.group, fresh)
    }

    fn hold_head(&mut self) {
        self.head = Some(self.take_group());
    }

    /// Observe one field, answering what the column's heap grew by and
    /// whether it went past a value PostgreSQL refuses, or [`Refused`] where
    /// keying it found one, or it is a NULL the column's `NOT NULL` refuses,
    /// and [`Self::invalid`] refuses it.
    fn observe(&mut self, field: &[u8]) -> Result<Observed, Refused> {
        let before = self.open_heap();
        let mut ignored = false;
        match decode_field(field) {
            // A NULL its `NOT NULL` refuses and that is gone past is counted,
            // the read taking it as the NULL it is: no statistic contradicts
            // it (D103).
            Ok(None) => {
                if self.not_null {
                    if self.invalid != PostgresInvalidValues::Ignore {
                        return Err(Refused);
                    }
                    ignored = true;
                }
                self.group.nulls += 1;
                return Ok(Observed { grew: 0, ignored });
            }
            Ok(Some(text)) => {
                let tier = self.tier.as_ref().and_then(|tier| tier.of(&text));
                if let Some(tier) = tier {
                    self.group.unrepresentable.add(tier);
                }
                self.group.value_bytes += text.len() as u64;
                // A value the type cannot hold is left out, as a NULL is: the
                // sum is of the values the type holds, read only in a view
                // taking the rest as NULL (`docs/design/decisions.md`, "D98").
                if let Some(summand) = self.summand.filter(|_| tier.is_none()) {
                    self.group.sum = self
                        .group
                        .sum
                        .zip(summand.value(&text))
                        .map(|(sum, value)| sum.wrapping_add(value));
                }
                let mut refused = false;
                for (bounds, group) in self.bounds.iter_mut().zip(&mut self.group.bounds) {
                    if let (Some(bounds), Some(group)) = (bounds, group) {
                        refused |= bounds.observe(group, &text, tier).is_err();
                    }
                }
                if refused {
                    if self.invalid != PostgresInvalidValues::Ignore {
                        return Err(Refused);
                    }
                    // Ignored: no statistic a read could contradict (D103).
                    ignored = true;
                    self.group.sum = None;
                    for (bounds, group) in self.bounds.iter_mut().zip(&mut self.group.bounds) {
                        if let (Some(bounds), Some(group)) = (bounds, group) {
                            bounds.lose_value(group, tier);
                        }
                    }
                    self.group.lose_texts();
                } else if let Some(dictionary) = &self.dictionary {
                    dictionary.observe(&mut self.group, &text);
                }
            }
            // Not text at all, so neither a key nor an entry: the group can
            // cover the row with neither, and the block's order is lost. No
            // decoding is longer than the escaped field, and nothing sums it.
            Err(_) => {
                self.group.value_bytes += field.len() as u64;
                self.group.sum = None;
                for (bounds, group) in self.bounds.iter_mut().zip(&mut self.group.bounds) {
                    if let (Some(bounds), Some(group)) = (bounds, group) {
                        bounds.lose_value(group, None);
                    }
                }
                self.group.lose_texts();
            }
        }
        Ok(Observed { grew: self.open_heap() as i64 - before as i64, ignored })
    }

    /// Close the open group, and move what it changed onto `charge` before
    /// the next column closes: **a close's growth is charged column by
    /// column**, so a wide block's account never waits on all its columns.
    /// A dictionary's interning map growing here is charged as it grows
    /// (`DictionaryGatherer::intern`), which the column's own difference then
    /// takes in rather than adding twice.
    fn close_group(&mut self, charge: &mut Charge) {
        let before = self.held();
        let base = charge.charged();
        let group = self.take_group();
        push_charged(&mut self.null_counts, group.nulls, charge);
        push_charged(&mut self.value_bytes, group.value_bytes, charge);
        if let Some(counts) = &mut self.unrepresentable {
            push_charged(counts, group.unrepresentable, charge);
        }
        match (&mut self.sums, group.sum) {
            (Some(sums), Some(sum)) => push_charged(sums, sum, charge),
            // The column held a value that is not its type: it has no sum.
            (sums, None) => *sums = None,
            (None, Some(_)) => {}
        }
        for (bounds, group) in self.bounds.iter_mut().zip(group.bounds) {
            if let (Some(bounds), Some(group)) = (bounds, group) {
                bounds.close_group(group, charge);
            }
        }
        if let Some(dictionary) = &mut self.dictionary {
            dictionary.close_group(group.texts, charge);
        }
        let after = self.held();
        charge.set(base.0 + after.0 - before.0, base.1 + after.1 - before.1);
    }

    /// Merge every closed group with the one after it
    /// ([`Gatherer::merge_pairs`]), moving `charge` by what the column frees
    /// before its dictionary renumbers, and again after.
    fn merge_pairs(&mut self, charge: &mut Charge) {
        let (before, base) = (self.held(), charge.charged());
        merge_adjacent(&mut self.null_counts, |a, b| a + b);
        if let Some(sums) = &mut self.sums {
            merge_adjacent(sums, i128::wrapping_add);
        }
        merge_adjacent(&mut self.value_bytes, |a, b| a + b);
        if let Some(counts) = &mut self.unrepresentable {
            merge_adjacent(counts, |mut a, b| {
                a.merge(&b);
                a
            });
        }
        for bounds in self.bounds.iter_mut().flatten() {
            bounds.merge_pairs();
        }
        if let Some(dictionary) = &mut self.dictionary {
            merge_adjacent(&mut dictionary.groups, union_indices);
            dictionary.indices = dictionary.groups.iter().flatten().map(vec_heap).sum();
        }
        let after = self.held();
        charge.set(base.0 + after.0 - before.0, base.1 + after.1 - before.1);
        if let Some(dictionary) = &mut self.dictionary {
            let (before, base) = (dictionary.held(), charge.charged());
            dictionary.renumber(charge);
            let after = dictionary.held();
            charge.set(base.0 + after.0 - before.0, base.1 + after.1 - before.1);
        }
    }

    /// Take the last closed group back into the open group, which follows it
    /// ([`Gatherer::reopen_last`]), moving `charge` by the difference.
    fn reopen_last(&mut self, charge: &mut Charge) {
        let (before, base) = (self.held(), charge.charged());
        let nulls = self.null_counts.pop().expect("a closed group counts its NULLs");
        // A column that has lost its sums reopens a group without one, which
        // loses nothing further.
        let sum = self.sums.as_mut().map(|sums| sums.pop().expect("a closed group sums"));
        let value_bytes = self.value_bytes.pop().expect("a closed group counts its text");
        let unrepresentable =
            self.unrepresentable.as_mut().map_or_else(Unrepresentable::default, |counts| {
                counts.pop().expect("a closed group counts what its type cannot hold")
            });
        let bounds = self.bounds.each_mut().map(|b| b.as_mut().map(BoundsGatherer::reopen_last));
        let texts = match &mut self.dictionary {
            Some(dictionary) => dictionary.reopen_last(),
            None => Some(Vec::new()),
        };
        let text_bytes = texts.iter().flatten().map(text_heap).sum();
        let mut group =
            GroupState { nulls, sum, value_bytes, unrepresentable, bounds, texts, text_bytes };
        group.absorb(mem::replace(&mut self.group, GroupState::fresh(&[None, None])));
        self.group = group;
        let after = self.held();
        charge.set(base.0 + after.0 - before.0, base.1 + after.1 - before.1);
    }

    /// Append `later`'s closed groups, which follow this column's, and take
    /// its open group as this one's — **draining `later` in place**, and after
    /// each part moves or is freed moving `charge` by the pair's difference,
    /// so the account follows the piece's structures across rather than
    /// releasing them all at the end.
    fn append(&mut self, later: &mut ColumnGatherer, charge: &mut Charge) {
        let pair = |mine: &Self, theirs: &Self| {
            let ((a, b), (c, d)) = (mine.held(), theirs.held());
            (a + c, b + d)
        };
        let before = pair(self, later);
        let base = charge.charged();
        let resync = |now: (u64, u64), charge: &mut Charge| {
            charge.set(base.0 + now.0 - before.0, base.1 + now.1 - before.1);
        };
        reserve_charged(&mut self.null_counts, later.null_counts.len(), charge);
        self.null_counts.extend_from_slice(&later.null_counts);
        later.null_counts = Vec::new();
        resync(pair(self, later), charge);
        reserve_charged(&mut self.value_bytes, later.value_bytes.len(), charge);
        self.value_bytes.extend_from_slice(&later.value_bytes);
        later.value_bytes = Vec::new();
        resync(pair(self, later), charge);
        if let (Some(mine), Some(theirs)) = (&mut self.unrepresentable, &mut later.unrepresentable)
        {
            reserve_charged(mine, theirs.len(), charge);
            mine.append(theirs);
            *theirs = Vec::new();
            resync(pair(self, later), charge);
        }
        match (&mut self.sums, &mut later.sums) {
            (Some(mine), Some(theirs)) => {
                reserve_charged(mine, theirs.len(), charge);
                mine.append(theirs);
                *theirs = Vec::new();
            }
            // The piece met a value that is not the column's type.
            (sums, None) => *sums = None,
            (None, Some(_)) => {}
        }
        resync(pair(self, later), charge);
        for set in 0..self.bounds.len() {
            let (Some(mine), Some(theirs)) = (&mut self.bounds[set], &mut later.bounds[set]) else {
                continue;
            };
            mine.append_tiers(theirs, charge);
            reserve_charged(&mut mine.groups, theirs.groups.len(), charge);
            reserve_charged(&mut mine.flags, theirs.flags.len(), charge);
            mine.groups.append(&mut theirs.groups);
            mine.flags.append(&mut theirs.flags);
            mine.stored += mem::take(&mut theirs.stored);
            (theirs.groups, theirs.flags) = (Vec::new(), Vec::new());
            resync(pair(self, later), charge);
        }
        let fresh = GroupState::fresh(&later.bounds);
        self.group = mem::replace(&mut later.group, fresh);
        if let (Some(mine), Some(theirs)) = (&mut self.dictionary, &mut later.dictionary) {
            mine.append(theirs, charge);
        }
        resync(pair(self, later), charge);
    }

    fn finish(self) -> ColumnStatistics {
        let [primary, datafusion] = self.bounds;
        ColumnStatistics {
            declared_type: self.declared_type,
            collation: self.collation,
            null_counts: self.null_counts,
            sums: self.sums,
            value_bytes: self.value_bytes,
            bounds: primary.map(BoundsGatherer::finish),
            datafusion_bounds: datafusion.map(BoundsGatherer::finish),
            dictionary: self.dictionary.map(DictionaryGatherer::finish),
            unrepresentable: self
                .unrepresentable
                .filter(|counts| counts.iter().any(|count| !count.is_zero())),
        }
    }
}

/// One column over one group while the group is open.
struct GroupState {
    nulls: u64,
    /// The values' wrapping sum so far, `None` once one did not decode as
    /// the column's type; read only for a column keeping sums.
    sum: Option<i128>,
    /// The values' text bytes so far.
    value_bytes: u64,
    /// The values past what the column's type holds so far, by tier.
    unrepresentable: Unrepresentable,
    /// One per [`crate::model::BoundsSet`], `None` for a set the column does not keep.
    bounds: [Option<GroupBounds>; 2],
    /// The group's distinct texts in first-seen order, `None` once past a
    /// cap, at a field that is not text or at one an ignoring parse went past;
    /// read only for a column keeping a dictionary.
    texts: Option<Vec<String>>,
    /// The heap `texts`' strings hold, summed as each is pushed.
    text_bytes: u64,
}

impl GroupState {
    fn fresh(bounds: &[Option<BoundsGatherer>; 2]) -> Self {
        Self {
            nulls: 0,
            sum: Some(0),
            value_bytes: 0,
            unrepresentable: Unrepresentable::default(),
            bounds: bounds.each_ref().map(|b| b.as_ref().map(BoundsGatherer::fresh_group)),
            texts: Some(Vec::new()),
            text_bytes: 0,
        }
    }

    /// No distinct texts for this group: a cap passed, a field not text, or
    /// one an ignoring parse went past.
    fn lose_texts(&mut self) {
        self.texts = None;
        self.text_bytes = 0;
    }

    /// The heap this group's state holds: its distinct texts and its running
    /// bounds.
    fn heap_bytes(&self) -> u64 {
        let texts = self.texts.as_ref().map_or(0, |texts| vec_heap(texts) + self.text_bytes);
        let bounds: u64 = self
            .bounds
            .iter()
            .flatten()
            .map(|bounds| match bounds {
                GroupBounds::Bytewise { min, max, .. } => {
                    [min, max].into_iter().flatten().map(|c| text_heap(&c.head)).sum()
                }
                GroupBounds::Keyed(group) => group.heap_bytes(),
            })
            .sum();
        texts + bounds
    }

    /// Fold `later`, the same column over the same group's following rows.
    /// The texts fold as the rows would have: each new one appended until a
    /// cap is passed.
    fn absorb(&mut self, later: GroupState) {
        self.nulls += later.nulls;
        self.sum = self.sum.zip(later.sum).map(|(sum, more)| sum.wrapping_add(more));
        self.value_bytes += later.value_bytes;
        self.unrepresentable.merge(&later.unrepresentable);
        for (mine, theirs) in self.bounds.iter_mut().zip(later.bounds) {
            if let (Some(mine), Some(theirs)) = (mine, theirs) {
                mine.absorb(theirs);
            }
        }
        let Some(theirs) = later.texts else {
            self.lose_texts();
            return;
        };
        let Some(mine) = &mut self.texts else { return };
        for text in theirs {
            if mine.contains(&text) {
                continue;
            }
            if mine.len() == DICTIONARY_MAX_ENTRIES {
                self.lose_texts();
                return;
            }
            self.text_bytes += text_heap(&text);
            mine.push(text);
        }
    }
}

/// How a column's values are ordered while gathering.
#[derive(Clone)]
enum Order {
    /// By the text itself, bytewise, once put in [`Canonical`] form: never
    /// keyed, since a key copies the whole value and a value may be hundreds
    /// of megabytes, and a bound past [`DICTIONARY_ENTRY_MAX_BYTES`] is truncated.
    Bytewise(Canonical),
    /// By [`ValueKey::stored_order`]: the key a filter orders by, a float's
    /// zeros told apart as Arrow's order tells them. A value past
    /// [`DICTIONARY_ENTRY_MAX_BYTES`] leaves its group unbounded and its block unordered,
    /// no truncation of it being a bound.
    Keyed(CompareKind),
}

/// The bytewise kinds, each by the text whose bytes order as its key does.
/// A text's `length` is its typmod's, past which a field is refused
/// ([`char_typmod_refuses`]) and which places none.
#[derive(Clone, Copy)]
enum Canonical {
    /// `text`, `varchar`, `name`, bounded bytewise in any collation: the text.
    Text { length: Option<u32> },
    /// `character`: the text without its trailing blanks, which its comparison
    /// ignores — so a bound is stored unpadded.
    PaddedText { length: Option<u32> },
    /// `bytea`: `\x` and lowercase hex pairs, which order as the bytes do —
    /// an `escape` value's head put in that form, as much as a bound reads
    /// ([`BYTEA_ESCAPE_HEAD_BYTES`]). A value in any other spelling is not
    /// placed.
    Bytea,
}

impl Canonical {
    /// The text whose bytes order `text` among its column's values, `None`
    /// where this kind cannot place it.
    fn of(self, text: &str) -> Option<Cow<'_, str>> {
        match self {
            Self::Text { .. } => Some(Cow::Borrowed(text)),
            Self::PaddedText { .. } => Some(Cow::Borrowed(text.trim_end_matches(' '))),
            Self::Bytea => match text.strip_prefix("\\x") {
                Some(hex) => (hex.len() % 2 == 0
                    && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
                .then_some(Cow::Borrowed(text)),
                None => decode_bytea_escape(text, BYTEA_ESCAPE_HEAD_BYTES)
                    .map(|head| Cow::Owned(render_bytea(&head))),
            },
        }
    }
}

/// How many of an `escape` value's bytes [`Canonical::of`] renders: enough
/// that a value cut there renders past [`CLIP_BYTES`], so its head is the
/// whole value's and it reads as no whole value, as the value itself does
/// (`docs/design/decisions.md`, "D76").
const BYTEA_ESCAPE_HEAD_BYTES: usize = CLIP_BYTES / 2;

/// How much of a bytewise value is kept: past the cap by one character's
/// width, so a kept head longer than the cap says its value is too.
const CLIP_BYTES: usize = DICTIONARY_ENTRY_MAX_BYTES + char::MAX.len_utf8();

/// A bytewise value's first [`CLIP_BYTES`], which is all a stored bound is
/// taken from (`docs/design/decisions.md`, "D76").
///
/// **Two values whose heads cannot be told apart store the same bound.** A
/// head that is not its whole value is at least `CLIP_BYTES` less one
/// character long, so two values it cannot place against each other share
/// more bytes than [`clipped_bounds`] reads of either. That is what lets a
/// group's bounds be joined from two halves' heads ([`GroupBounds::absorb`])
/// and still store what one pass over the group's values stores: each keeps a
/// value no other value in its half is certainly beyond, and that value can
/// differ from the true extreme only by bytes no stored bound holds.
#[derive(Clone)]
struct Clipped {
    head: String,
    /// Whether `head` is the whole value.
    whole: bool,
}

impl Clipped {
    fn of(text: &str) -> Self {
        let head = text_prefix(text, CLIP_BYTES);
        Self { head: head.to_owned(), whole: head.len() == text.len() }
    }

    /// Where `text` orders against the value this was clipped from, `None`
    /// when `text` begins with a head that is not the whole value. Two such
    /// values share every byte a stored bound reads, so either stands for
    /// the other in the group's bounds; only their row order is unknown.
    fn locate(&self, text: &str) -> Option<Ordering> {
        self.locate_bytes(text.as_bytes(), text.len())
    }

    /// [`Self::locate`] for a text known by its length and its first
    /// [`CLIP_BYTES`] bytes (all of them, if it is shorter) — as much as the
    /// answer reads — so a piece's first value is placed exactly as a row is.
    fn locate_bytes(&self, prefix: &[u8], len: usize) -> Option<Ordering> {
        let head = self.head.as_bytes();
        let shared = head.len().min(len);
        match prefix[..shared].cmp(&head[..shared]) {
            Ordering::Equal if len > head.len() => self.whole.then_some(Ordering::Greater),
            Ordering::Equal if len == head.len() && self.whole => Some(Ordering::Equal),
            Ordering::Equal => Some(Ordering::Less),
            unequal => Some(unequal),
        }
    }

    /// Where `other`'s value orders against this one's, known only by their
    /// heads: `None` exactly where the shorter head agrees with the longer and
    /// is not its whole value, which [`Clipped`] says stores the same bound.
    fn order(&self, other: &Clipped) -> Option<Ordering> {
        let (mine, theirs) = (self.head.as_bytes(), other.head.as_bytes());
        let shared = mine.len().min(theirs.len());
        match theirs[..shared].cmp(&mine[..shared]) {
            Ordering::Equal => {}
            unequal => return Some(unequal),
        }
        match theirs.len().cmp(&mine.len()) {
            Ordering::Less => other.whole.then_some(Ordering::Less),
            Ordering::Greater => self.whole.then_some(Ordering::Greater),
            Ordering::Equal => match (self.whole, other.whole) {
                (true, true) => Some(Ordering::Equal),
                (true, false) => Some(Ordering::Greater),
                (false, true) => Some(Ordering::Less),
                (false, false) => None,
            },
        }
    }
}

struct BoundsGatherer {
    order: Order,
    /// Per closed group, **as a merge needs it rather than as it is stored**:
    /// a keyed group's stored bounds, and a bytewise group's two extremes'
    /// heads ([`Clipped`]), `min_exact` and `max_exact` saying whether each
    /// head is its whole value — stored bounds only once [`Self::finish`]
    /// clips them, since a clipped upper bound no longer orders as its value
    /// does (`docs/design/decisions.md`, "D82"). **Over the values every
    /// layer's type holds**: a value past a tier is kept apart, in
    /// [`Self::tiers`].
    groups: Vec<Option<Bounds>>,
    /// Per closed group, [`LOST`].
    flags: Vec<u8>,
    /// Per closed group, its values past each tier as `groups` and `flags`
    /// hold them, `None` for a group holding none — **empty until a group
    /// holds one**, then one per closed group, so a column never meeting such
    /// a value pays nothing for the views (`docs/design/decisions.md`, "D97").
    tiers: Vec<Option<Box<ClosedTiers>>>,
    /// The text every entry of `groups` and `tiers` holds, summed as they are
    /// pushed, and each boxed entry of `tiers`.
    stored: u64,
    rows: ViewOrders,
}

/// One closed group's values past each tier, in [`tier_slot`] order, as
/// [`BoundsGatherer::groups`] and [`BoundsGatherer::flags`] hold a group's:
/// `(None, 0)` for a tier it holds no value past.
type ClosedTiers = [(Option<Bounds>, u8); 2];

/// A closed group held a value its bounds could not cover, so it has none —
/// where a group with `None` and no flag held no non-NULL value.
const LOST: u8 = 1;

/// Where a tier's extremes sit among a group's: the engine's, then the
/// format spec's.
fn tier_slot(tier: UnrepresentableTier) -> usize {
    match tier {
        UnrepresentableTier::Engine => 0,
        UnrepresentableTier::Format => 1,
    }
}

/// One group's running bounds.
enum GroupBounds {
    Bytewise { min: Option<Clipped>, max: Option<Clipped>, lost: bool },
    Keyed(KeyedGroup),
}

/// A keyed group's running bounds: over the values every layer holds, and
/// apart from them each tier's, which only a group holding one allocates.
#[derive(Default)]
struct KeyedGroup {
    values: Extremes,
    tiers: Option<Box<[Extremes; 2]>>,
}

/// The extremes of some keyed values, each with its text.
#[derive(Default)]
struct Extremes {
    min: Option<(ValueKey, String)>,
    max: Option<(ValueKey, String)>,
    lost: bool,
}

impl Extremes {
    fn observe(&mut self, key: &ValueKey, text: &str) {
        if self.min.as_ref().is_none_or(|(m, _)| key.stored_order(m) == Ordering::Less) {
            self.min = Some((key.clone(), text.to_owned()));
        }
        if self.max.as_ref().is_none_or(|(m, _)| key.stored_order(m) == Ordering::Greater) {
            self.max = Some((key.clone(), text.to_owned()));
        }
    }

    /// Fold `later`, the extremes of the values following these. The earliest
    /// of equal keys is kept, as a pass does.
    fn absorb(&mut self, later: Extremes) {
        self.lost |= later.lost;
        if let Some(value) = later.min
            && self.min.as_ref().is_none_or(|(m, _)| value.0.stored_order(m) == Ordering::Less)
        {
            self.min = Some(value);
        }
        if let Some(value) = later.max
            && self.max.as_ref().is_none_or(|(m, _)| value.0.stored_order(m) == Ordering::Greater)
        {
            self.max = Some(value);
        }
    }

    fn heap_bytes(&self) -> u64 {
        [&self.min, &self.max]
            .into_iter()
            .flatten()
            .map(|(key, text)| key.heap_bytes() + text_heap(text))
            .sum()
    }

    fn closed(self) -> (Option<Bounds>, u8) {
        match self {
            Extremes { lost: false, min: Some((_, min)), max: Some((_, max)) } => {
                (Some(Bounds { min, max, min_exact: true, max_exact: true }), 0)
            }
            Extremes { lost, .. } => (None, if lost { LOST } else { 0 }),
        }
    }

    /// Closed extremes as they stood before they closed.
    fn reopen(kind: &CompareKind, bounds: Option<Bounds>, flags: u8) -> Self {
        match bounds {
            Some(Bounds { min, max, .. }) => {
                let key = |text: &str| ValueKey::of(kind, text).expect("a stored bound keyed");
                Extremes { min: Some((key(&min), min)), max: Some((key(&max), max)), lost: false }
            }
            None => Extremes { lost: flags & LOST != 0, ..Extremes::default() },
        }
    }
}

impl KeyedGroup {
    /// The extremes a value past `tier` joins — a value no tier's joining
    /// [`Self::values`].
    fn extremes(&mut self, tier: Option<UnrepresentableTier>) -> &mut Extremes {
        match tier {
            None => &mut self.values,
            Some(tier) => &mut self.tiers.get_or_insert_with(Box::default)[tier_slot(tier)],
        }
    }

    fn absorb(&mut self, later: KeyedGroup) {
        self.values.absorb(later.values);
        let Some(theirs) = later.tiers else { return };
        match &mut self.tiers {
            Some(mine) => {
                let [engine, format] = *theirs;
                mine[0].absorb(engine);
                mine[1].absorb(format);
            }
            None => self.tiers = Some(theirs),
        }
    }

    fn heap_bytes(&self) -> u64 {
        let tiers = self.tiers.as_ref().map_or(0, |tiers| {
            size_of::<[Extremes; 2]>() as u64 + tiers.iter().map(Extremes::heap_bytes).sum::<u64>()
        });
        self.values.heap_bytes() + tiers
    }
}

impl GroupBounds {
    /// Fold `later`, the same group's bounds over its following rows. Keyed
    /// extremes keep the earliest of equal keys, as a pass does; bytewise ones
    /// are placed by their heads alone ([`Clipped`]).
    fn absorb(&mut self, later: GroupBounds) {
        match (self, later) {
            (
                Self::Bytewise { min, max, lost },
                Self::Bytewise { min: later_min, max: later_max, lost: later_lost },
            ) => {
                *lost |= later_lost;
                if let Some(value) = later_min
                    && min.as_ref().is_none_or(|m| m.order(&value) == Some(Ordering::Less))
                {
                    *min = Some(value);
                }
                if let Some(value) = later_max
                    && max.as_ref().is_none_or(|m| m.order(&value) == Some(Ordering::Greater))
                {
                    *max = Some(value);
                }
            }
            (Self::Keyed(mine), Self::Keyed(later)) => mine.absorb(later),
            _ => unreachable!("one column's groups keep one kind of bounds"),
        }
    }
}

/// A column's row order over the rows observed so far: the block's
/// [`Sortedness`] once every row is in.
#[derive(Clone)]
struct RowOrder {
    /// The first value placed, carried to a join as much as places it against
    /// the value before it.
    first: Option<FirstValue>,
    /// The last value placed.
    previous: Option<Previous>,
    never_decreased: bool,
    never_increased: bool,
    /// A value the order cannot place was seen.
    lost: bool,
}

/// A column's first value, kept for the join.
#[derive(Clone)]
enum FirstValue {
    /// Its length and its first [`CLIP_BYTES`] bytes, cut at a byte rather
    /// than a character: [`Clipped::locate_bytes`] reads no more.
    Bytewise {
        prefix: Vec<u8>,
        len: usize,
    },
    Keyed(ValueKey),
}

#[derive(Clone)]
enum Previous {
    Bytewise(Clipped),
    Keyed(ValueKey),
}

impl Default for RowOrder {
    fn default() -> Self {
        Self {
            first: None,
            previous: None,
            never_decreased: true,
            never_increased: true,
            lost: false,
        }
    }
}

impl RowOrder {
    /// The heap the first and the last value placed hold.
    fn heap_bytes(&self) -> u64 {
        let first = match &self.first {
            None => 0,
            Some(FirstValue::Bytewise { prefix, .. }) => vec_heap(prefix),
            Some(FirstValue::Keyed(key)) => key.heap_bytes(),
        };
        let previous = match &self.previous {
            None => 0,
            Some(Previous::Bytewise(clipped)) => text_heap(&clipped.head),
            Some(Previous::Keyed(key)) => key.heap_bytes(),
        };
        first + previous
    }

    /// Fold one step of the row order: `step` is where the value orders
    /// against the previous one, `None` where that is unknown.
    fn step(&mut self, step: Option<Ordering>) {
        match step {
            Some(Ordering::Less) => self.never_decreased = false,
            Some(Ordering::Greater) => self.never_increased = false,
            Some(Ordering::Equal) => {}
            None => self.lost = true,
        }
    }

    /// Place a keyed value after the one placed before it.
    fn place_keyed(&mut self, key: ValueKey) {
        let step = match &self.previous {
            Some(Previous::Keyed(previous)) => Some(key.stored_order(previous)),
            Some(Previous::Bytewise(_)) => unreachable!("a keyed order places no head"),
            None => {
                self.first = Some(FirstValue::Keyed(key.clone()));
                None
            }
        };
        self.previous = Some(Previous::Keyed(key));
        if let Some(step) = step {
            self.step(Some(step));
        }
    }

    /// Fold `later`, the order over the rows following these: one step across
    /// the join, then its own.
    fn absorb(&mut self, later: RowOrder) {
        if let (Some(previous), Some(first)) = (&self.previous, &later.first) {
            let step = match (previous, first) {
                (Previous::Bytewise(previous), FirstValue::Bytewise { prefix, len }) => {
                    previous.locate_bytes(prefix, *len)
                }
                (Previous::Keyed(previous), FirstValue::Keyed(first)) => {
                    Some(first.stored_order(previous))
                }
                _ => unreachable!("one column's values are ordered one way"),
            };
            self.step(step);
        }
        self.never_decreased &= later.never_decreased;
        self.never_increased &= later.never_increased;
        self.lost |= later.lost;
        if self.first.is_none() {
            self.first = later.first;
        }
        if later.previous.is_some() {
            self.previous = later.previous;
        }
    }

    fn sortedness(&self) -> Sortedness {
        if self.lost {
            Sortedness::Unsorted
        } else if self.never_decreased {
            Sortedness::Ascending
        } else if self.never_increased {
            Sortedness::Descending
        } else {
            Sortedness::Unsorted
        }
    }
}

/// A column's row order in each view of its values
/// (`crate::model::StatisticsView`): over the values every layer holds,
/// and — **once the rows observed hold a value past a tier, and not before** —
/// over those and the engine's tier, and over every value. Until then the
/// three are one order, which is what a fork copies.
#[derive(Default)]
struct ViewOrders {
    plain: RowOrder,
    /// Over the values the format spec holds, then over every value.
    wider: Option<Box<[RowOrder; 2]>>,
}

impl ViewOrders {
    fn fork(&mut self) -> &mut [RowOrder; 2] {
        let plain = &self.plain;
        self.wider.get_or_insert_with(|| Box::new([plain.clone(), plain.clone()]))
    }

    /// Place `key`, a value past `tier`, in each order that takes it.
    fn place_keyed(&mut self, key: ValueKey, tier: Option<UnrepresentableTier>) {
        match tier {
            None => {
                if let Some([representable, every]) = self.wider.as_deref_mut() {
                    representable.place_keyed(key.clone());
                    every.place_keyed(key.clone());
                }
                self.plain.place_keyed(key);
            }
            Some(UnrepresentableTier::Engine) => {
                let [representable, every] = self.fork();
                representable.place_keyed(key.clone());
                every.place_keyed(key);
            }
            Some(UnrepresentableTier::Format) => self.fork()[1].place_keyed(key),
        }
    }

    /// A value past `tier` no order that takes it can place.
    fn lose(&mut self, tier: Option<UnrepresentableTier>) {
        match tier {
            None => {
                self.plain.lost = true;
                for order in self.wider.iter_mut().flat_map(|wider| wider.iter_mut()) {
                    order.lost = true;
                }
            }
            Some(UnrepresentableTier::Engine) => {
                for order in self.fork() {
                    order.lost = true;
                }
            }
            Some(UnrepresentableTier::Format) => self.fork()[1].lost = true,
        }
    }

    fn absorb(&mut self, mut later: ViewOrders) {
        if self.wider.is_some() || later.wider.is_some() {
            self.fork();
            later.fork();
        }
        self.plain.absorb(later.plain);
        if let (Some(mine), Some(theirs)) = (&mut self.wider, later.wider) {
            let [representable, every] = *theirs;
            mine[0].absorb(representable);
            mine[1].absorb(every);
        }
    }

    fn heap_bytes(&self) -> u64 {
        let wider = self.wider.as_ref().map_or(0, |wider| {
            size_of::<[RowOrder; 2]>() as u64 + wider.iter().map(RowOrder::heap_bytes).sum::<u64>()
        });
        self.plain.heap_bytes() + wider
    }

    /// Each view's sortedness: over the values the format spec holds, over
    /// every value, and over those the engine holds as well.
    fn sortedness(&self) -> [Sortedness; 3] {
        let plain = self.plain.sortedness();
        match self.wider.as_deref() {
            None => [plain; 3],
            Some([representable, every]) => [representable.sortedness(), every.sortedness(), plain],
        }
    }
}

impl BoundsGatherer {
    fn new(kind: CompareKind) -> Self {
        let order = match kind {
            CompareKind::Text { length } => Order::Bytewise(Canonical::Text { length }),
            CompareKind::PaddedText { length } => Order::Bytewise(Canonical::PaddedText { length }),
            CompareKind::Bytea => Order::Bytewise(Canonical::Bytea),
            kind => Order::Keyed(kind),
        };
        Self::of(order)
    }

    fn of(order: Order) -> Self {
        Self {
            order,
            groups: Vec::new(),
            flags: Vec::new(),
            tiers: Vec::new(),
            stored: 0,
            rows: ViewOrders::default(),
        }
    }

    fn fresh(&self) -> Self {
        Self::of(self.order.clone())
    }

    fn fresh_group(&self) -> GroupBounds {
        match self.order {
            Order::Bytewise(_) => GroupBounds::Bytewise { min: None, max: None, lost: false },
            Order::Keyed(_) => GroupBounds::Keyed(KeyedGroup::default()),
        }
    }

    /// A value past `tier` that does not key, or that no stored bound could
    /// cover: no bounds for its group, and no order for its block, in each
    /// view that takes it.
    fn lose_value(&mut self, group: &mut GroupBounds, tier: Option<UnrepresentableTier>) {
        match group {
            GroupBounds::Bytewise { lost, .. } => *lost = true,
            GroupBounds::Keyed(group) => group.extremes(tier).lost = true,
        }
        self.rows.lose(tier);
    }

    /// Observe `text`, a value past `tier` where it is past one — which only a
    /// keyed kind's column can hold. **A value its type's `*_in` refuses is
    /// [`Refused`]**, where keying it, placing a `bytea` or holding a text to
    /// its length finds that; no other value is, one this build cannot read
    /// losing the group's bounds and its block's order.
    fn observe(
        &mut self,
        group: &mut GroupBounds,
        text: &str,
        tier: Option<UnrepresentableTier>,
    ) -> Result<(), Refused> {
        match &self.order {
            Order::Bytewise(canonical) => {
                debug_assert!(tier.is_none(), "a bytewise kind's type holds every value");
                let canonical = *canonical;
                if let Canonical::Text { length: Some(length) }
                | Canonical::PaddedText { length: Some(length) } = canonical
                    && char_typmod_refuses(text, length)
                {
                    self.lose_value(group, None);
                    return Err(Refused);
                }
                let Some(text) = canonical.of(text) else {
                    self.lose_value(group, None);
                    return match canonical {
                        Canonical::Bytea if bytea_unread(text) == Unread::Refused => Err(Refused),
                        _ => Ok(()),
                    };
                };
                let text = text.as_ref();
                let GroupBounds::Bytewise { min, max, .. } = group else {
                    unreachable!("a bytewise order keeps bytewise bounds")
                };
                if min.as_ref().is_none_or(|m| m.locate(text) == Some(Ordering::Less)) {
                    *min = Some(Clipped::of(text));
                }
                if max.as_ref().is_none_or(|m| m.locate(text) == Some(Ordering::Greater)) {
                    *max = Some(Clipped::of(text));
                }
                let rows = &mut self.rows.plain;
                let step = match &rows.previous {
                    Some(Previous::Bytewise(previous)) => Some(previous.locate(text)),
                    Some(Previous::Keyed(_)) => unreachable!("a bytewise order places no key"),
                    None => {
                        let prefix = text.as_bytes()[..text.len().min(CLIP_BYTES)].to_vec();
                        rows.first = Some(FirstValue::Bytewise { prefix, len: text.len() });
                        None
                    }
                };
                rows.previous = Some(Previous::Bytewise(Clipped::of(text)));
                if let Some(step) = step {
                    rows.step(step);
                }
            }
            Order::Keyed(kind) => {
                if text.len() > DICTIONARY_ENTRY_MAX_BYTES {
                    self.lose_value(group, tier);
                    return Ok(());
                }
                let key = match ValueKey::of_field(kind, text) {
                    Ok(key) => key,
                    Err(unread) => {
                        self.lose_value(group, tier);
                        return if unread == Unread::Refused { Err(Refused) } else { Ok(()) };
                    }
                };
                let GroupBounds::Keyed(group) = group else {
                    unreachable!("a keyed order keeps keyed bounds")
                };
                group.extremes(tier).observe(&key, text);
                self.rows.place_keyed(key, tier);
            }
        }
        Ok(())
    }

    fn close_group(&mut self, group: GroupBounds, charge: &mut Charge) {
        let (bounds, flags, tiers) = closed(group);
        self.stored += closed_heap(&bounds) + tiers_heap(&tiers);
        push_charged(&mut self.groups, bounds, charge);
        push_charged(&mut self.flags, flags, charge);
        if tiers.is_some() || !self.tiers.is_empty() {
            let wanted = self.groups.len() - self.tiers.len();
            reserve_charged(&mut self.tiers, wanted, charge);
            self.tiers.resize_with(self.groups.len() - 1, || None);
            self.tiers.push(tiers);
        }
    }

    /// A closed group's running bounds again, as they stood when it closed.
    fn reopen(
        &self,
        bounds: Option<Bounds>,
        flags: u8,
        tiers: Option<Box<ClosedTiers>>,
    ) -> GroupBounds {
        match &self.order {
            Order::Bytewise(_) => match bounds {
                Some(Bounds { min, max, min_exact, max_exact }) => GroupBounds::Bytewise {
                    min: Some(Clipped { head: min, whole: min_exact }),
                    max: Some(Clipped { head: max, whole: max_exact }),
                    lost: false,
                },
                None => GroupBounds::Bytewise { min: None, max: None, lost: flags & LOST != 0 },
            },
            Order::Keyed(kind) => GroupBounds::Keyed(KeyedGroup {
                values: Extremes::reopen(kind, bounds, flags),
                tiers: tiers.map(|tiers| {
                    Box::new((*tiers).map(|(bounds, flags)| Extremes::reopen(kind, bounds, flags)))
                }),
            }),
        }
    }

    /// Closed group `index`, taken out of the lists, which keep a hole.
    fn take_closed(&mut self, index: usize) -> (Option<Bounds>, u8, Option<Box<ClosedTiers>>) {
        let tiers = self.tiers.get_mut(index).and_then(Option::take);
        (self.groups[index].take(), mem::take(&mut self.flags[index]), tiers)
    }

    /// The last closed group's running bounds, no longer listed.
    fn reopen_last(&mut self) -> GroupBounds {
        let bounds = self.groups.pop().expect("a closed group lists its bounds");
        let flags = self.flags.pop().expect("a closed group flags its bounds");
        let tiers = self.tiers.pop().flatten();
        self.stored -= closed_heap(&bounds) + tiers_heap(&tiers);
        self.reopen(bounds, flags, tiers)
    }

    /// Merge every closed group with the one after it, as one pass over both
    /// groups' values closes them.
    fn merge_pairs(&mut self) {
        let closed_groups = self.groups.len();
        let merged = closed_groups.div_ceil(2);
        let tiered = !self.tiers.is_empty();
        for j in 0..merged {
            let first = self.take_closed(2 * j);
            let (bounds, flags, tiers) = if 2 * j + 1 < closed_groups {
                let second = self.take_closed(2 * j + 1);
                let mut group = self.reopen(first.0, first.1, first.2);
                group.absorb(self.reopen(second.0, second.1, second.2));
                closed(group)
            } else {
                first
            };
            (self.groups[j], self.flags[j]) = (bounds, flags);
            if tiered {
                self.tiers[j] = tiers;
            }
        }
        self.groups.truncate(merged);
        self.flags.truncate(merged);
        if tiered {
            self.tiers.truncate(merged);
        }
        self.stored = self.groups.iter().map(closed_heap).sum::<u64>()
            + self.tiers.iter().map(tiers_heap).sum::<u64>();
    }

    /// Append `later`'s tiers, which follow this gatherer's, ahead of its
    /// groups: both lists keep one per closed group where either holds any.
    fn append_tiers(&mut self, later: &mut BoundsGatherer, charge: &mut Charge) {
        if self.tiers.is_empty() && later.tiers.is_empty() {
            return;
        }
        let total = self.groups.len() + later.groups.len();
        let wanted = total - self.tiers.len();
        reserve_charged(&mut self.tiers, wanted, charge);
        self.tiers.resize_with(self.groups.len(), || None);
        if later.tiers.is_empty() {
            self.tiers.resize_with(total, || None);
        } else {
            self.tiers.append(&mut later.tiers);
        }
        later.tiers = Vec::new();
    }

    fn finish(self) -> ColumnBounds {
        let [sortedness, every_order, displayable_order] = self.rows.sortedness();
        let mut groups = self.groups;
        match &self.order {
            Order::Bytewise(canonical) => {
                for group in groups.iter_mut() {
                    if let Some(Bounds { min, max, min_exact, max_exact }) = group.take() {
                        let min = Clipped { head: min, whole: min_exact };
                        *group = clipped_bounds(
                            *canonical,
                            min,
                            Clipped { head: max, whole: max_exact },
                        );
                    }
                }
                ColumnBounds { sortedness, groups, every: None, displayable: None }
            }
            Order::Keyed(_) if self.tiers.is_empty() => {
                ColumnBounds { sortedness, groups, every: None, displayable: None }
            }
            Order::Keyed(kind) => {
                let seen =
                    |(bounds, flags): &(Option<Bounds>, u8)| bounds.is_some() || flags & LOST != 0;
                let (mut every, mut displayable) = (Vec::new(), Vec::new());
                let (mut any_every, mut any_displayable) = (false, false);
                let mut flags = self.flags.into_iter();
                for (group, tiers) in groups.iter_mut().zip(self.tiers) {
                    let base_flags = flags.next().expect("a closed group flags its bounds");
                    let Some(tiers) = tiers else {
                        every.push(None);
                        displayable.push(None);
                        continue;
                    };
                    let [engine, format] = *tiers;
                    let (engine_seen, format_seen) = (seen(&engine), seen(&format));
                    let values = group.take();
                    let mut representable = Extremes::reopen(kind, values.clone(), base_flags);
                    representable.absorb(Extremes::reopen(kind, engine.0, engine.1));
                    let (representable, representable_flags) = representable.closed();
                    displayable.push(values.filter(|_| engine_seen));
                    any_displayable |= engine_seen;
                    every.push(
                        format_seen
                            .then(|| {
                                let mut all = Extremes::reopen(
                                    kind,
                                    representable.clone(),
                                    representable_flags,
                                );
                                all.absorb(Extremes::reopen(kind, format.0, format.1));
                                all.closed().0
                            })
                            .flatten(),
                    );
                    any_every |= format_seen;
                    *group = representable;
                }
                let view = |any: bool, sortedness, groups| {
                    any.then_some(BoundsView { sortedness, groups })
                };
                ColumnBounds {
                    sortedness,
                    every: view(any_every, every_order, every),
                    displayable: view(any_displayable, displayable_order, displayable),
                    groups,
                }
            }
        }
    }
}

/// The text a closed group's bounds hold.
fn closed_heap(bounds: &Option<Bounds>) -> u64 {
    bounds.as_ref().map_or(0, |b| text_heap(&b.min) + text_heap(&b.max))
}

/// The heap a closed group's tiers hold: their box and their bounds' text.
fn tiers_heap(tiers: &Option<Box<ClosedTiers>>) -> u64 {
    tiers.as_ref().map_or(0, |tiers| {
        size_of::<ClosedTiers>() as u64
            + tiers.iter().map(|(bounds, _)| closed_heap(bounds)).sum::<u64>()
    })
}

/// The bounds one group of `values` is stored with under `kind`'s order, or
/// `None` where it keeps none — for a test outside this module that holds
/// stored bounds against another order.
#[cfg(test)]
pub(crate) fn one_group_bounds(kind: CompareKind, values: &[&str]) -> Option<Bounds> {
    let mut gatherer = BoundsGatherer::new(kind);
    let mut group = gatherer.fresh_group();
    for value in values {
        gatherer.observe(&mut group, value, None).expect("a value the server wrote");
    }
    let mut charge = Charge::new(Arc::default(), Term::Gathering);
    gatherer.close_group(group, &mut charge);
    gatherer.finish().groups.pop().flatten()
}

/// A closed group's bounds as [`BoundsGatherer::groups`],
/// [`BoundsGatherer::flags`] and [`BoundsGatherer::tiers`] hold them.
fn closed(group: GroupBounds) -> (Option<Bounds>, u8, Option<Box<ClosedTiers>>) {
    match group {
        GroupBounds::Bytewise { lost: false, min: Some(min), max: Some(max) } => {
            let (min_exact, max_exact) = (min.whole, max.whole);
            (Some(Bounds { min: min.head, max: max.head, min_exact, max_exact }), 0, None)
        }
        GroupBounds::Bytewise { lost, .. } => (None, if lost { LOST } else { 0 }, None),
        GroupBounds::Keyed(KeyedGroup { values, tiers }) => {
            let (bounds, flags) = values.closed();
            (bounds, flags, tiers.map(|tiers| Box::new((*tiers).map(Extremes::closed))))
        }
    }
}

/// Merge every entry of `items` with the one after it, a last one alone, in
/// place.
fn merge_adjacent<T: Default>(items: &mut Vec<T>, mut merge: impl FnMut(T, T) -> T) {
    let len = items.len();
    for j in 0..len.div_ceil(2) {
        let first = mem::take(&mut items[2 * j]);
        let pair = match items.get_mut(2 * j + 1) {
            Some(second) => merge(first, mem::take(second)),
            None => first,
        };
        items[j] = pair;
    }
    items.truncate(len.div_ceil(2));
}

/// A bytewise group's bounds as stored: each exact where its value fits the
/// cap, otherwise a prefix below and a successor above.
fn clipped_bounds(canonical: Canonical, min: Clipped, max: Clipped) -> Option<Bounds> {
    let fits = |c: &Clipped| c.whole && c.head.len() <= DICTIONARY_ENTRY_MAX_BYTES;
    let min_exact = fits(&min);
    let lower = if min_exact {
        min.head
    } else {
        text_prefix(&min.head, DICTIONARY_ENTRY_MAX_BYTES).to_owned()
    };
    if fits(&max) {
        return Some(Bounds { min: lower, max: max.head, min_exact, max_exact: true });
    }
    let upper = match canonical {
        Canonical::Text { .. } => text_upper(&max.head, false)?,
        Canonical::PaddedText { .. } => text_upper(&max.head, true)?,
        Canonical::Bytea => bytea_upper(&max.head)?,
    };
    Some(Bounds { min: lower, max: upper, min_exact, max_exact: false })
}

/// The longest prefix of `text` no longer than `cap` bytes that ends on a
/// character boundary — a lower bound on `text` bytewise.
fn text_prefix(text: &str, cap: usize) -> &str {
    let mut end = cap.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A text no longer than [`DICTIONARY_ENTRY_MAX_BYTES`] ordering above `text` bytewise:
/// a prefix with its last character replaced by the next one. UTF-8 orders
/// bytewise as it orders code points, so the successor is above every text
/// the prefix begins. `padded` is `character`, whose comparison drops trailing
/// blanks first: the prefix gives its own up, and a successor that is a blank
/// is skipped. `None` when no character of the prefix has a successor.
fn text_upper(text: &str, padded: bool) -> Option<String> {
    // Room for a successor one byte longer than the character it replaces.
    let mut prefix: String =
        text_prefix(text, DICTIONARY_ENTRY_MAX_BYTES - char::MAX.len_utf8() + 1).to_owned();
    if padded {
        prefix.truncate(prefix.trim_end_matches(' ').len());
    }
    while let Some(last) = prefix.pop() {
        let Some(next) = successor(last) else { continue };
        let next = if padded && next == ' ' { '!' } else { next };
        prefix.push(next);
        return Some(prefix);
    }
    None
}

/// The next Unicode scalar value, skipping the surrogate range.
fn successor(c: char) -> Option<char> {
    match c {
        '\u{D7FF}' => Some('\u{E000}'),
        char::MAX => None,
        _ => char::from_u32(c as u32 + 1),
    }
}

/// The most bytes a rendered `\x` bound can carry within the cap.
const BYTEA_CAP_BYTES: usize = (DICTIONARY_ENTRY_MAX_BYTES - 2) / 2;

/// A rendered `bytea` above the value: its prefix with trailing `0xFF` bytes
/// dropped and the last byte incremented. `None` when every byte of that
/// prefix is `0xFF`.
fn bytea_upper(text: &str) -> Option<String> {
    let mut bytes = decode_bytea(text)?;
    bytes.truncate(BYTEA_CAP_BYTES);
    while bytes.last() == Some(&0xFF) {
        bytes.pop();
    }
    *bytes.last_mut()? += 1;
    Some(render_bytea(&bytes))
}

/// Make room in `vec` for `additional` more, **charging the growth ahead of
/// the allocation** ([`Charge::ahead`]): at least doubling, as the vector's
/// own growth would, but reserved exactly so the charge is the capacity.
fn reserve_charged<T>(vec: &mut Vec<T>, additional: usize, charge: &mut Charge) {
    let wanted = vec.len() + additional;
    let capacity = vec.capacity();
    if wanted <= capacity {
        return;
    }
    let grown = wanted.max(capacity * 2).max(4);
    let bytes = ((grown - capacity) * size_of::<T>()) as u64;
    charge.ahead((bytes, 0), (0, 0), || vec.reserve_exact(grown - vec.len()));
}

/// Push `item` onto `vec`, charging a growth it forces ahead of it.
fn push_charged<T>(vec: &mut Vec<T>, item: T, charge: &mut Charge) {
    reserve_charged(vec, 1, charge);
    vec.push(item);
}

/// The heap a `HashMap<K, V>` of `capacity` allocates, as the standard
/// library's table lays one out: a slot per bucket, then a control byte per
/// bucket and one probe group's worth past the end. The layout is the
/// toolchain's, not a guarantee (`docs/design/runtime-invariants.md`, "RT11").
fn map_heap<K, V>(capacity: usize) -> u64 {
    if capacity == 0 {
        return 0;
    }
    let buckets = if capacity < 8 { capacity + 1 } else { (capacity * 8 / 7).next_power_of_two() };
    table_heap::<K, V>(buckets)
}

/// The heap the table a full `HashMap<K, V>` of `capacity` grows into
/// allocates: the table sized for one more entry than it holds.
fn grown_map_heap<K, V>(capacity: usize) -> u64 {
    let wanted = capacity + 1;
    let buckets = if wanted < 15 {
        match wanted.max(3) {
            3 => 4,
            4..=7 => 8,
            _ => 16,
        }
    } else {
        (wanted * 8 / 7).next_power_of_two()
    };
    table_heap::<K, V>(buckets)
}

fn table_heap<K, V>(buckets: usize) -> u64 {
    let probe = if cfg!(any(target_arch = "x86", target_arch = "x86_64")) { 16 } else { 8 };
    let align = align_of::<(K, V)>().max(probe);
    let slots = (size_of::<(K, V)>() * buckets).next_multiple_of(align);
    (slots + buckets + probe) as u64
}

/// A column's dictionary. A `character` entry is its text without the
/// trailing blanks its comparison ignores, deduplicated and measured against
/// [`DICTIONARY_ENTRY_MAX_BYTES`] as such (`docs/design/decisions.md`, "D34").
struct DictionaryGatherer {
    /// Whether the column is `character`, whose entries give up their padding.
    padded: bool,
    entries: Vec<String>,
    /// The text every entry holds, summed as each is interned — and so the
    /// text the interning map's keys hold too.
    entry_text: u64,
    interned: HashMap<String, u32>,
    groups: Vec<Option<Vec<u32>>>,
    /// The heap every group's index list holds, summed as each is pushed.
    indices: u64,
}

impl DictionaryGatherer {
    fn new(kind: &CompareKind) -> Self {
        Self {
            padded: matches!(kind, CompareKind::PaddedText { .. }),
            entries: Vec::new(),
            entry_text: 0,
            interned: HashMap::new(),
            groups: Vec::new(),
            indices: 0,
        }
    }

    fn fresh(&self) -> Self {
        Self {
            padded: self.padded,
            entries: Vec::new(),
            entry_text: 0,
            interned: HashMap::new(),
            groups: Vec::new(),
            indices: 0,
        }
    }

    /// The heap this dictionary holds, as its entries and index lists and as
    /// its interning map.
    fn held(&self) -> (u64, u64) {
        let entries =
            vec_heap(&self.entries) + self.entry_text + vec_heap(&self.groups) + self.indices;
        (entries, map_heap::<String, u32>(self.interned.capacity()) + self.entry_text)
    }

    /// Add `text` to `group`'s distinct texts, which a cap passed leaves
    /// `None`.
    fn observe(&self, group: &mut GroupState, text: &str) {
        let Some(texts) = &mut group.texts else { return };
        let text = if self.padded { text.trim_end_matches(' ') } else { text };
        if texts.iter().any(|seen| seen == text) {
            return;
        }
        if text.len() > DICTIONARY_ENTRY_MAX_BYTES || texts.len() == DICTIONARY_MAX_ENTRIES {
            group.lose_texts();
            return;
        }
        let text = text.to_owned();
        group.text_bytes += text_heap(&text);
        texts.push(text);
    }

    /// **A map about to grow is charged its larger table before the insert
    /// allocates it**, and released of the smaller once the insert has freed
    /// it: the two are allocated at once while the table rehashes, and a
    /// close interning into several full maps would otherwise hold each
    /// growth uncharged until its observer next updates the account.
    fn intern(&mut self, text: &str, charge: &mut Charge) -> u32 {
        if let Some(&id) = self.interned.get(text) {
            return id;
        }
        let id = self.entries.len() as u32;
        push_charged(&mut self.entries, text.to_owned(), charge);
        let capacity = self.interned.capacity();
        let key = text.to_owned();
        if self.interned.len() == capacity {
            let grown = grown_map_heap::<String, u32>(capacity);
            let smaller = map_heap::<String, u32>(capacity);
            charge.ahead((0, grown), (0, smaller), || self.interned.insert(key, id));
        } else {
            self.interned.insert(key, id);
        }
        self.entry_text += text.len() as u64;
        id
    }

    fn close_group(&mut self, texts: Option<Vec<String>>, charge: &mut Charge) {
        let indices: Option<Vec<u32>> =
            texts.map(|texts| texts.iter().map(|text| self.intern(text, charge)).collect());
        self.indices += indices.as_ref().map_or(0, vec_heap);
        push_charged(&mut self.groups, indices, charge);
    }

    /// Append `later`'s groups, which follow this dictionary's, interning
    /// their entries here in the order a pass closing them would have —
    /// draining `later`'s index lists as they are re-interned, and moving
    /// `charge` by the pair's difference whenever what this one gained passes
    /// [`STATISTICS_ACCOUNT_CHARGE_STEP`]. `later`'s entries and map stay until it is dropped.
    fn append(&mut self, later: &mut DictionaryGatherer, charge: &mut Charge) {
        let pair = |mine: &Self, theirs: &Self| {
            let ((a, b), (c, d)) = (mine.held(), theirs.held());
            (a + c, b + d)
        };
        let before = pair(self, later);
        let base = charge.charged();
        let mut synced = self.entry_text * 2 + self.indices;
        for at in 0..later.groups.len() {
            let indices: Option<Vec<u32>> = later.groups[at].take().map(|ids| {
                later.indices -= vec_heap(&ids);
                ids.iter().map(|&id| self.intern(&later.entries[id as usize], charge)).collect()
            });
            self.indices += indices.as_ref().map_or(0, vec_heap);
            push_charged(&mut self.groups, indices, charge);
            let gained = self.entry_text * 2 + self.indices;
            if gained - synced >= STATISTICS_ACCOUNT_CHARGE_STEP {
                let now = pair(self, later);
                charge.set(base.0 + now.0 - before.0, base.1 + now.1 - before.1);
                synced = gained;
            }
        }
        later.groups = Vec::new();
        let now = pair(self, later);
        charge.set(base.0 + now.0 - before.0, base.1 + now.1 - before.1);
    }

    /// The last closed group's distinct texts, no longer listed.
    fn reopen_last(&mut self) -> Option<Vec<String>> {
        let ids = self.groups.pop().expect("a closed group lists its texts")?;
        self.indices -= vec_heap(&ids);
        Some(ids.iter().map(|&id| self.entries[id as usize].clone()).collect())
    }

    /// **Renumber the entries the groups still name in the order a pass
    /// closing these groups interns them, and let every other go**: a merge
    /// past [`DICTIONARY_MAX_ENTRIES`] leaves entries no group names, and a merged
    /// group lists its second half's new texts after its first's. The
    /// renumbering's scratch is charged ahead of it, and **the interning map
    /// is rebuilt rather than pruned**, its new table charged ahead and the old
    /// one released in one update with every text let go: a table pruned in
    /// place keeps its size while its reported capacity stops naming it
    /// (`docs/design/runtime-invariants.md`, "RT11").
    fn renumber(&mut self, charge: &mut Charge) {
        let scratch = (self.entries.len() * size_of::<u32>()) as u64;
        let mut to = charge.ahead((scratch, 0), (0, 0), || vec![u32::MAX; self.entries.len()]);
        let mut kept = 0u32;
        for id in self.groups.iter_mut().flatten().flatten() {
            let slot = &mut to[*id as usize];
            if *slot == u32::MAX {
                *slot = kept;
                kept += 1;
            }
            *id = *slot;
        }
        let old_table = map_heap::<String, u32>(self.interned.capacity());
        let new_table = match kept {
            0 => 0,
            kept => grown_map_heap::<String, u32>(kept as usize - 1),
        };
        charge.ahead_freeing((0, new_table), || {
            let mut interned = HashMap::with_capacity(kept as usize);
            for (key, id) in mem::take(&mut self.interned) {
                if to[id as usize] != u32::MAX {
                    interned.insert(key, to[id as usize]);
                }
            }
            self.interned = interned;
            // Every entry no group names goes past the kept ones, then the
            // permutation is applied in place, cycle by cycle.
            for (past, slot) in (kept..).zip(to.iter_mut().filter(|slot| **slot == u32::MAX)) {
                *slot = past;
            }
            for at in 0..to.len() {
                while to[at] as usize != at {
                    let there = to[at] as usize;
                    self.entries.swap(at, there);
                    to.swap(at, there);
                }
            }
            drop(to);
            let dropped: u64 =
                self.entries[kept as usize..].iter().map(|entry| entry.len() as u64).sum();
            self.entries.truncate(kept as usize);
            self.entry_text -= dropped;
            ((), (scratch + dropped, old_table + dropped))
        });
    }

    fn finish(self) -> ColumnDictionary {
        ColumnDictionary { entries: self.entries, groups: self.groups }
    }
}

/// One group's distinct texts, as indices, merged with the following group's
/// as one pass over both groups' values gathers them: the first's, then the
/// second's not already listed, and none past [`DICTIONARY_MAX_ENTRIES`].
fn union_indices(first: Option<Vec<u32>>, second: Option<Vec<u32>>) -> Option<Vec<u32>> {
    let (first, second) = (first?, second?);
    let added = second.iter().filter(|id| !first.contains(id)).count();
    if added == 0 {
        return Some(first);
    }
    if first.len() + added > DICTIONARY_MAX_ENTRIES {
        return None;
    }
    let mut union = Vec::with_capacity(first.len() + added);
    union.extend_from_slice(&first);
    union.extend(second.iter().filter(|id| !first.contains(id)));
    Some(union)
}

#[cfg(test)]
mod tests;
