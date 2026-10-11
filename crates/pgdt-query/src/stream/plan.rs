//! What a replay plans before it reads: each matched block resolved for the
//! query, the row groups its filter prunes, and the columns whose values the
//! front end cannot hold.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use pgdt_codec::{ResolvedSchema, SchemaMode};
use pgdt_codec::{
    UnrepresentableMode,
    internal::{UnrepresentableRead, unrepresentable_reads, unrepresentable_tests},
};
use pgdt_core::CopyHeader;
use pgdt_core::{Error, Result};
use pgdt_dump::ScanOptions;
use pgdt_model::{CopyBlock, DumpMetadata, Unrepresentable, UnrepresentableTier};

use super::note::{PlanNote, PlanNoteKind};
use super::resolve::project;
use super::segment::{Segment, SegmentEntry};
use super::table::{TableColumns, refuse_metadata_level, resolve_block};
use crate::batch::QueryOptions;
use crate::predicate::{ComparisonNote, ResolvedExpr, resolve_expr};
use crate::prune::{SortedStop, prune_block};

/// Everything a replay needs that the mapping pass produced, shared unchanged
/// by every sub-stream of a partitioned replay
/// (`docs/design/decisions.md`, "D51"). Held behind an `Arc`, `metadata` being
/// the whole dump's DDL, and never mutated once shared with a sub-stream —
/// `table_stream_partitions` writes the derived span onto `query_options`
/// after the mapping pass and before the `Arc`: the table's columns
/// in particular — their order, and their census, the union over **every**
/// block the query will replay — are settled once, so two partitions of one
/// table cannot order its columns or resolve its arrays differently
/// (`docs/design/decisions.md`, "D35").
#[derive(Clone)]
pub(super) struct ReplayPlan {
    pub(super) scan_options: ScanOptions,
    pub(super) query_options: QueryOptions,
    pub(super) metadata: Option<DumpMetadata>,
    pub(super) table: TableColumns,
    /// Every matched block, resolved and keyed by the
    /// block's `header_offset` — see [`plan_blocks`], which refuses the whole
    /// plan on any block's resolution refusal.
    pub(super) blocks: BTreeMap<u64, PlannedBlock>,
    /// The runs of row groups each pruned block keeps, keyed as `blocks` is
    /// ([`crate::prune::BlockPruning::kept`]); a block absent here is read
    /// whole.
    pub(super) kept: BTreeMap<u64, Vec<Range<u64>>>,
    /// Where each block sorted on a column the filter bounds stops being read,
    /// keyed as `blocks` is ([`SortedStop`]).
    pub(super) stops: BTreeMap<u64, SortedStop>,
    /// What pruning skipped, where any block's statistics were consulted.
    pruned: Option<PlanNote>,
    /// Each materialized column's values read as NULL
    /// ([`PlanNoteKind::ReadAsNull`]).
    read_as_null: Vec<PlanNote>,
    /// At least the rows the replay can emit ([`Pruned::rows`]).
    pub(super) kept_rows: u64,
    /// At least the text bytes the replay emits of each projected field
    /// ([`Pruned::value_bytes`]).
    pub(super) kept_value_bytes: Vec<Option<u64>>,
}

impl ReplayPlan {
    /// The plan for replaying `matches`: the table's columns
    /// ([`TableColumns::settle`]), and every one of its blocks resolved
    /// against them and the DDL before any sub-stream exists — or the table's
    /// refusal, or else the first refusing block's ([`plan_blocks`]), or else,
    /// under the refuse mode, its first materialized column holding a value
    /// the query cannot hold ([`materialized_unrepresentable`]).
    pub(super) fn new(
        scan_options: ScanOptions,
        query_options: QueryOptions,
        matches: &[CopyBlock],
        metadata: Option<DumpMetadata>,
    ) -> Result<Self> {
        refuse_metadata_level(matches, &query_options)?;
        let table = TableColumns::settle(matches, metadata.as_ref(), &query_options)?;
        let blocks = plan_blocks(matches, &query_options, metadata.as_ref(), &table)?;
        let mut unrepresentable = materialized_unrepresentable(matches, &blocks, &query_options);
        if query_options.unrepresentable == UnrepresentableMode::Refuse
            && !unrepresentable.is_empty()
        {
            let MaterializedUnrepresentable { table, column, declared_type, values } =
                unrepresentable.swap_remove(0);
            return Err(Error::Unrepresentable { table, column, declared_type, values });
        }
        let read_as_null = read_as_null(unrepresentable, &query_options);
        let Pruned { kept, stops, note: pruned, rows: kept_rows, value_bytes: kept_value_bytes } =
            prune_blocks(matches, &blocks, &query_options, metadata.as_ref());
        Ok(Self {
            scan_options,
            query_options,
            metadata,
            table,
            blocks,
            kept,
            stops,
            pruned,
            read_as_null,
            kept_rows,
            kept_value_bytes,
        })
    }

    /// What the plan settled before any row is read, as every sub-stream
    /// carries it ([`TableStream::plan_notes`](super::table::TableStream::plan_notes)) bar what cutting it into
    /// sub-streams adds.
    pub(super) fn notes(&self) -> impl Iterator<Item = PlanNote> + '_ {
        self.read_as_null.iter().chain(&self.pruned).cloned()
    }

    /// The segments that replay `block` whole, or the runs of groups its
    /// statistics keep: the run holding group 0 starts at the block's header,
    /// as a whole block does, and every other starts inside its data.
    pub(super) fn segments(&self, block: &CopyBlock) -> Vec<Segment> {
        let Some(kept) = self.kept.get(&block.header_offset) else {
            return vec![Segment::new(
                block,
                block.header_offset,
                block.end_offset,
                SegmentEntry::Header,
            )];
        };
        kept.iter().map(|run| Segment::over(block, run.clone())).collect()
    }
}

/// What [`prune_blocks`] settles for a [`ReplayPlan`].
struct Pruned {
    kept: BTreeMap<u64, Vec<Range<u64>>>,
    stops: BTreeMap<u64, SortedStop>,
    note: Option<PlanNote>,
    /// The rows of every group pruning kept, plus the `row_count` of every
    /// block it did not consult: never fewer than the replay emits, and
    /// nothing guessed below that — where a sorted block stops is found only
    /// as its rows are read.
    rows: u64,
    /// Per projected field, the text bytes of its values in the groups `rows`
    /// counts ([`pgdt_model::ColumnStatistics::value_bytes`]) — `None`
    /// wherever a block counted none — bounding what the replay emits of it
    /// as `rows` bounds its rows.
    value_bytes: Vec<Option<u64>>,
}

/// Settle which row groups of `matches` the query's filter skips
/// ([`prune_block`]), where a sorted block's rows stop being read, and the
/// [`PlanNote`] saying what was skipped. Nothing is pruned or stopped where
/// the caller turned statistics off or the filter reads no field — no
/// statistic can rule out a row of a filter that keeps every one.
///
/// **A block whose statistics keep every group is left out of `kept`**, so
/// it is replayed exactly as an unpruned query replays it, bar its stop.
fn prune_blocks(
    matches: &[CopyBlock],
    blocks: &BTreeMap<u64, PlannedBlock>,
    query_options: &QueryOptions,
    metadata: Option<&DumpMetadata>,
) -> Pruned {
    let (mut kept, mut stops) = (BTreeMap::new(), BTreeMap::new());
    let mut rows = 0;
    let fields = blocks.values().next().map_or(0, |planned| planned.resolved.schema.fields().len());
    let mut value_bytes = vec![Some(0); fields];
    let (mut consulted, mut groups, mut skipped_groups, mut skipped_bytes) = (false, 0, 0, 0);
    for block in matches {
        let pruning = blocks
            .get(&block.header_offset)
            .filter(|planned| query_options.use_statistics && planned.filter.reads_fields())
            .and_then(|planned| {
                prune_block(block, &planned.filter, metadata, query_options.statistics_view())
            });
        let kept_groups = pruning.as_ref().map(|pruning| pruning.kept_groups.as_slice());
        let block_bytes = field_value_bytes(block, blocks.get(&block.header_offset), kept_groups);
        for (field, held) in value_bytes.iter_mut().enumerate() {
            *held = held.zip(block_bytes.get(field).copied().flatten()).map(|(a, b)| a + b);
        }
        let Some(pruning) = pruning else {
            rows += block.row_count;
            continue;
        };
        consulted = true;
        rows += pruning.kept_rows;
        groups += pruning.groups;
        skipped_groups += pruning.skipped_groups;
        skipped_bytes += pruning.skipped_bytes;
        if let Some(stop) = pruning.stop {
            stops.insert(block.header_offset, stop);
        }
        if pruning.skipped_groups > 0 {
            kept.insert(block.header_offset, pruning.kept);
        }
    }
    let bytes = matches.iter().map(|b| b.terminator_offset.saturating_sub(b.data_offset)).sum();
    let note = consulted.then_some(PlanNote {
        kind: PlanNoteKind::StatisticsPruned { skipped_groups, groups, skipped_bytes, bytes },
        levers: Vec::new(),
    });
    Pruned { kept, stops, note, rows, value_bytes }
}

/// A column the query materializes that holds values its front end cannot
/// hold, with the map's count of them in the tiers the query's semantics
/// cannot hold over every block of the table, not the groups a filter keeps.
struct MaterializedUnrepresentable {
    table: String,
    column: String,
    declared_type: String,
    values: u64,
}

/// **Each column the query materializes that holds a value its front end
/// cannot hold**, in the order the query emits them — typed, and never a
/// column read as `Utf8View`, whose text holds every value. What the refuse
/// mode refuses at planning ([`Error::Unrepresentable`],
/// `docs/design/decisions.md`, "D99") and the null mode warns of
/// ([`read_as_null`]).
fn materialized_unrepresentable(
    matches: &[CopyBlock],
    blocks: &BTreeMap<u64, PlannedBlock>,
    query_options: &QueryOptions,
) -> Vec<MaterializedUnrepresentable> {
    if query_options.schema_mode != SchemaMode::Typed {
        return Vec::new();
    }
    let (Some(first), Some(planned)) = (matches.first(), blocks.values().next()) else {
        return Vec::new();
    };
    let reach = query_options.unrepresentable_reach();
    let table = first.header.qualified_name();
    let fields = planned.resolved.schema.fields();
    let mut found = Vec::new();
    for (i, field) in fields.iter().enumerate() {
        if matches!(field.data_type(), arrow::datatypes::DataType::Utf8View) {
            continue;
        }
        let values = unrepresentable_values(matches, field.name(), reach);
        if values == 0 {
            continue;
        }
        found.push(MaterializedUnrepresentable {
            table: table.clone(),
            column: field.name().clone(),
            declared_type: planned.resolved.notes[i].declared.clone().unwrap_or_default(),
            values,
        });
    }
    found
}

/// The map's count of column `name`'s values past the tiers `reach` names,
/// over every one of `matches`, the blocks of one table.
pub(super) fn unrepresentable_values(
    matches: &[CopyBlock],
    name: &str,
    reach: UnrepresentableTier,
) -> u64 {
    matches
        .iter()
        .filter_map(|block| {
            let c = block.header.columns.iter().position(|column| column == name)?;
            let count = block.unrepresentable.as_ref()?.get(c)?;
            Some(count.format + if reach == UnrepresentableTier::Engine { count.engine } else { 0 })
        })
        .sum()
}

/// **Each column the query materializes that holds a value it reads as
/// NULL**, with the map's count ([`PlanNoteKind::ReadAsNull`]) — under
/// [`pgdt_codec::UnrepresentableMode::Null`] alone.
fn read_as_null(
    columns: Vec<MaterializedUnrepresentable>,
    query_options: &QueryOptions,
) -> Vec<PlanNote> {
    if query_options.unrepresentable != UnrepresentableMode::Null {
        return Vec::new();
    }
    columns
        .into_iter()
        .map(|MaterializedUnrepresentable { table, column, declared_type, values }| PlanNote {
            kind: PlanNoteKind::ReadAsNull {
                table,
                column,
                declared_type,
                values,
                semantics: query_options.semantics,
            },
            levers: Vec::new(),
        })
        .collect()
}

/// Per field of `planned`, the text bytes `block`'s statistics count of its
/// values in the groups `kept` keeps — every group where `kept` is `None` —
/// and `None` for a field the block's statistics did not count
/// ([`pgdt_model::ColumnStatistics::value_bytes_where`]).
fn field_value_bytes(
    block: &CopyBlock,
    planned: Option<&PlannedBlock>,
    kept: Option<&[bool]>,
) -> Vec<Option<u64>> {
    let Some(planned) = planned else { return Vec::new() };
    let mut bytes = vec![None; planned.resolved.schema.fields().len()];
    let Some(statistics) = block.statistics.as_deref() else { return bytes };
    if statistics.columns.len() != block.header.columns.len() {
        return bytes;
    }
    let groups = statistics.groups.len();
    let keep = |g: usize| kept.is_none_or(|kept| kept.get(g).copied().unwrap_or(false));
    for (column, target) in statistics.columns.iter().zip(&planned.field_targets) {
        if let (Some(column), Some(field)) = (column, *target) {
            bytes[field] = column.value_bytes_where(groups, keep);
        }
    }
    bytes
}

/// One block's schema, filter and projection, resolved against a query —
/// everything [`activate`](super::segment::activate) builds a [`RowBatcher`](crate::batch::RowBatcher) from, bar the batcher,
/// which holds rows and so is built per activation.
///
/// It records the two inputs the block itself contributes, so a lookup can
/// confirm it answers the activation asking rather than trusting the offset
/// alone; the rest are the [`ReplayPlan`]'s, and fixed.
#[derive(Debug, Clone)]
pub(super) struct PlannedBlock {
    pub(super) header: CopyHeader,
    pub(super) database: Option<String>,
    /// The **projected** schema, which is what the batches carry.
    pub(super) resolved: ResolvedSchema,
    pub(super) field_targets: Vec<Option<usize>>,
    /// Per field of the block, how the query reads its values their type
    /// cannot hold ([`unrepresentable_reads`]) — what the batcher and the
    /// filter both read them by.
    pub(super) unrepresentable: Vec<Option<UnrepresentableRead>>,
    pub(super) filter: Arc<ResolvedExpr>,
    pub(super) notes: Vec<ComparisonNote>,
}

/// Resolve one block for a query: its schema, the filter against
/// the unprojected schema, then the projection — in that order, which is the
/// order their refusals are raised in.
///
/// **An unprojected query projects the table's order** ([`TableColumns`]),
/// so the batches of a block listing its columns in another order come out
/// reordered by name, the same shape as every other block's.
///
/// `counts` is the block's unrepresentable count, where the map holds the
/// block ([`unrepresentable_reads`]).
pub(super) fn resolve_for_query(
    header: &CopyHeader,
    header_offset: u64,
    database: Option<&str>,
    query_options: &QueryOptions,
    metadata: Option<&DumpMetadata>,
    table: &TableColumns,
    counts: Option<&[Unrepresentable]>,
) -> Result<PlannedBlock> {
    let census = table.census_for(&header.columns);
    let text = table.text_for(&header.columns);
    let full = resolve_block(
        header,
        metadata,
        database,
        query_options.schema_mode,
        &census,
        &text,
        query_options.semantics,
    )?;
    let unrepresentable = query_reads(header, metadata, database, &full, counts, query_options);
    let tests = query_tests(header, metadata, database, counts, query_options)?;
    // Against the *unprojected* schema, in the block's own order: a term's
    // index numbers the raw row's fields, and a term may name a column the
    // projection dropped.
    let filter =
        resolve_expr(&query_options.filter, &full, header_offset, query_options.semantics)?
            .reading(&unrepresentable)
            .testing(&tests)
            .invalid_values(query_options.postgres_invalid_values);
    let notes = filter.comparison_notes();
    let projection = query_options.projection.as_deref().unwrap_or(&table.order);
    let (resolved, field_targets) = project(&full, Some(projection), header_offset)?;
    Ok(PlannedBlock {
        header: header.clone(),
        database: database.map(str::to_string),
        resolved,
        field_targets,
        unrepresentable,
        filter: Arc::new(filter),
        notes,
    })
}

/// [`unrepresentable_reads`] for `query_options`' mode and front end.
pub(super) fn query_reads(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    full: &ResolvedSchema,
    counts: Option<&[Unrepresentable]>,
    query_options: &QueryOptions,
) -> Vec<Option<UnrepresentableRead>> {
    unrepresentable_reads(
        header,
        metadata,
        database,
        full,
        counts,
        query_options.unrepresentable,
        query_options.unrepresentable_reach(),
    )
}

/// What `query_options`' unrepresentable tests test each column's text by
/// ([`unrepresentable_tests`]), and nothing where its filter holds none — or
/// its refusal, under [`SchemaMode::Strings`], which resolves no declared type
/// for a test to read (`docs/design/decisions.md`, "D101").
fn query_tests(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    counts: Option<&[Unrepresentable]>,
    query_options: &QueryOptions,
) -> Result<Vec<Option<UnrepresentableRead>>> {
    let Some(test) = query_options.filter.first_unrepresentable_test() else {
        return Ok(Vec::new());
    };
    if query_options.schema_mode == SchemaMode::Strings {
        return Err(Error::UnrepresentableTestUntyped {
            column: test.column.clone(),
            op: test.op.symbol(),
        });
    }
    let reach = query_options.unrepresentable_reach();
    Ok(unrepresentable_tests(header, metadata, database, counts, reach))
}

/// **The filter is resolved at plan time**, once per matched block and
/// before any byte of a block is replayed, rather than by each piece of each
/// block as a sub-stream reaches it — so the whole plan's resolved trees are
/// in hand before a segment is cut, and a block split into many pieces
/// resolves once.
///
/// **A resolution refusal is a plan fact**: the first block in file order
/// whose [`resolve_for_query`] refuses — its schema, its filter or its
/// projection, in that order — is the error, before any row of any block
/// (`docs/design/decisions.md`, "D54").
pub(super) fn plan_blocks(
    matches: &[CopyBlock],
    query_options: &QueryOptions,
    metadata: Option<&DumpMetadata>,
    table: &TableColumns,
) -> Result<BTreeMap<u64, PlannedBlock>> {
    matches
        .iter()
        .map(|block| {
            let planned = resolve_for_query(
                &block.header,
                block.header_offset,
                block.database.as_deref(),
                query_options,
                metadata,
                table,
                block.unrepresentable.as_deref(),
            )?;
            Ok((block.header_offset, planned))
        })
        .collect()
}
