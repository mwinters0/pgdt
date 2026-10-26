//! What a complete map's statistics say about a whole table: its rows, and
//! per column its NULLs, its extremes in one comparison semantics, its
//! distinct values where the dictionaries hold them all, its sum and its
//! values' text bytes; and the order a replay's partitions emit a column in,
//! where the map proves one ([`partition_orders`]).
//!
//! [`crate::prune`] reads the same stored sets per group to skip one; this
//! folds every group of every block of a table into one answer, for an
//! embedder that plans over a table rather than reads it
//! (`docs/design/decisions.md`, "D89").
//!
//! **A summary says whether it is complete, and never how nearly.** A caller
//! that answers a query from it needs to know that nothing was left out; a
//! caller estimating does not care by how much (`docs/design/decisions.md`,
//! "D89").

use std::collections::{HashMap, HashSet};

use arrow::array::ArrayRef;

use crate::render::decode_field;
use pgdt_codec::ResolvedSchema;
use pgdt_codec::compare::{CompareKind, ComparisonSemantics, bounds_set_keyed_by};
use pgdt_codec::internal::{OrderKey, column_tiers, compare_keys_stored, order_key};
use pgdt_mapping::internal::{
    believed_under, declared_column, dictionary_holds_field_text, keeps_sums, stored_resolution,
};
use pgdt_model::{
    Bounds, ColumnDictionary, CopyBlock, DumpMetadata, Sortedness, StatisticsView,
    UnrepresentableTier,
};

/// What a table's stored statistics amount to, one entry per column of the
/// schema it was summarized against.
#[derive(Debug, Clone, Default)]
pub struct TableSummary {
    /// Rows over every block of the table, which a complete map counts
    /// exactly whether or not any block gathered statistics.
    pub rows: u64,
    /// Positional, parallel to the resolved schema's fields.
    pub columns: Vec<ColumnSummary>,
}

/// One column's statistics over every block of a table.
#[derive(Debug, Clone, Default)]
pub struct ColumnSummary {
    /// NULLs over every group that counted them.
    pub nulls: u64,
    /// Whether every block counted them, so that [`Self::nulls`] is the
    /// column's NULLs rather than a floor under them.
    pub nulls_complete: bool,
    /// The least and greatest value the stored bounds reach, decoded as the
    /// column emits them; `None` where no group was bounded, or where the
    /// bound's text does not read back as a value the column emits
    /// ([`crate::decode_field`]).
    pub min: Option<Bound>,
    pub max: Option<Bound>,
    /// Whether every group of every block contributed — an unbounded group
    /// holding nothing but NULLs counts as contributing, having no value to
    /// bound. `false` makes [`Self::min`] and [`Self::max`] the extremes of
    /// the groups that contributed, estimates of the table's rather than its
    /// own.
    pub bounds_complete: bool,
    /// How many distinct non-NULL values the column emits, where every group
    /// of every block kept a dictionary of its field texts: the size of their
    /// union. `None` wherever one group did not (`docs/design/decisions.md`,
    /// "D89"), and for a `character` column, whose entries give up the
    /// trailing blanks it emits.
    ///
    /// Distinct text is a distinct value in the column's Arrow type as in its
    /// text: each type is written in one form (I4, I40, I41), a
    /// `timestamptz` in one zone a table (I48).
    pub distinct: Option<u64>,
    /// The column's non-NULL values summed as integers — a decimal's unscaled
    /// — and wrapped at 128 bits, which reduces exactly to any narrower
    /// wrapping sum ([`pgdt_model::ColumnStatistics::sums`];
    /// `docs/design/decisions.md`, "D91"): where
    /// every group of every block kept one, under the DDL it was gathered
    /// under, and the column is emitted in the type it was summed as. `None`
    /// elsewhere, and where the column holds no non-NULL value, whose sum is
    /// no number.
    pub sum: Option<i128>,
    /// The summed text bytes of the column's non-NULL values, where every
    /// group of every block counted them
    /// ([`pgdt_model::ColumnStatistics::value_bytes`]); `None`
    /// elsewhere. Read off the text, so believed whatever the DDL now says,
    /// as the NULL counts are.
    pub value_bytes: Option<u64>,
}

/// One end of a column's range.
#[derive(Debug, Clone)]
pub struct Bound {
    /// A one-element array of the column's Arrow type ([`decode_field`]).
    pub value: ArrayRef,
    /// Whether the value is one the column holds, rather than a bound outside
    /// every value ([`pgdt_model::Bounds::min_exact`],
    /// [`pgdt_model::Bounds::max_exact`]).
    pub exact: bool,
}

/// Summarize a table over `blocks`, every block of it in a complete map, in
/// `database`, against the schema a query of it resolves and in the semantics that query compares in, reading each
/// column in the view that query reads a value its type cannot hold in
/// ([`crate::QueryOptions::statistics_view`]).
///
/// **A block's bounds are read from the set pruning reads** — the set
/// gathering stored under the kind a term in `semantics` compares by, and
/// only where the DDL the column was gathered under still stands
/// (`docs/design/decisions.md`, "D78", "D79"). A block with no statistics
/// leaves every column incomplete and its rows counted all the same.
///
/// **A value `reading` takes as NULL is one**: counted among the NULLs, left
/// out of the bounds and the distinct values, and out of the sum, which a
/// view taking every value reads nowhere such a value is
/// (`docs/design/decisions.md`, "D97", "D98").
pub(crate) fn table_summary(
    database: Option<&str>,
    blocks: &[CopyBlock],
    metadata: Option<&DumpMetadata>,
    resolved: &ResolvedSchema,
    semantics: ComparisonSemantics,
    reading: StatisticsView,
) -> TableSummary {
    let fields = resolved.schema().fields();
    let mut accumulators: Vec<Accumulator> = (0..fields.len())
        .map(|i| {
            let kind = resolved.columns()[i]
                .comparison()
                .bounds_read_by(semantics)
                // A `character(n)`'s set is stored without the trailing
                // blanks its comparison ignores, so its bounds are not values
                // the column emits and no extreme can be read off them.
                .filter(|kind| !matches!(kind, CompareKind::PaddedText { .. }));
            Accumulator {
                kind,
                nulls: 0,
                nulls_complete: true,
                min: None,
                max: None,
                complete: true,
                distinct: Some(HashSet::new()),
                sum: Some(0),
                value_bytes: Some(0),
            }
        })
        .collect();
    let mut rows = 0;
    for block in blocks {
        rows += block.row_count;
        let Some(statistics) = block.statistics.as_deref() else {
            accumulators.iter_mut().for_each(Accumulator::uncounted);
            continue;
        };
        if statistics.columns.len() != block.header.columns.len() {
            accumulators.iter_mut().for_each(Accumulator::uncounted);
            continue;
        }
        let qualified = block.header.qualified_name();
        let gathered = stored_resolution(&block.header, metadata, database);
        let tiers = column_tiers(&gathered);
        let mut seen = vec![false; fields.len()];
        for (c, name) in block.header.columns.iter().enumerate() {
            let Some(i) = fields.iter().position(|field| field.name() == name) else { continue };
            seen[i] = true;
            let accumulator = &mut accumulators[i];
            let Some(column) = statistics.columns[c].as_ref() else {
                accumulator.uncounted();
                continue;
            };
            // The bounds, the row order and the dictionary are believed only
            // under the DDL they were gathered under; the NULL counts are
            // read off the text and are believed regardless, bar a value a
            // view adds to them, counted under that DDL too (D78, D97).
            let believed =
                believed_under(column, declared_column(metadata, database, &qualified, name));
            let groups = statistics.groups.len();
            let counted = column.counted_groups();
            let nulls_view = if believed { reading } else { StatisticsView::Every };
            accumulator.nulls +=
                (0..counted).filter_map(|g| column.null_count(g, nulls_view)).sum::<u64>();
            if counted != groups || (!believed && reading != StatisticsView::Every) {
                accumulator.nulls_complete = false;
            }
            accumulator.value_bytes = accumulator
                .value_bytes
                .zip(column.value_bytes_where(groups, |_| true))
                .map(|(held, block)| held + block);
            // A sum is of the values the type holds, so it is the column's
            // only in a view taking the rest as NULL, or where there are none.
            let summed = reading.reads_as_null(Some(UnrepresentableTier::Format))
                || !column.holds_unrepresentable(UnrepresentableTier::Format);
            let sums =
                column.sums.as_ref().filter(|sums| summed && believed && sums.len() == groups);
            accumulator.sum = accumulator
                .sum
                .zip(sums)
                .map(|(held, sums)| sums.iter().fold(held, |sum, &group| sum.wrapping_add(group)));
            let dictionary = column
                .dictionary
                .as_ref()
                .filter(|_| believed && dictionary_holds_field_text(&gathered, c));
            let tier = tiers.get(c).cloned().flatten().filter(|_| reading != StatisticsView::Every);
            accumulator.union(dictionary, statistics.groups.len(), |entry| {
                tier.as_ref().is_some_and(|tier| reading.reads_as_null(tier.of(entry)))
            });
            let set = accumulator
                .kind
                .as_ref()
                .filter(|_| believed)
                .and_then(|kind| bounds_set_keyed_by(&gathered.columns()[c].bounds_kinds(), kind))
                // A set not one to a group describes other groups and is not
                // read, nor is one beside NULL counts that are not, whose
                // total is then incomplete.
                .filter(|&set| column.listed_groups(set) == Some(groups) && counted == groups);
            let Some(set) = set else {
                accumulator.complete = false;
                continue;
            };
            for (g, group) in statistics.groups.iter().enumerate() {
                match column.group_bounds(set, reading, g) {
                    Some(bounds) => accumulator.fold(bounds),
                    // A group whose every row is NULL has no value to bound,
                    // so it leaves the column's extremes complete; one that
                    // lost a value to gathering does not.
                    None if column.null_count(g, reading) == Some(group.rows) => {}
                    None => accumulator.complete = false,
                }
            }
        }
        for (accumulator, seen) in accumulators.iter_mut().zip(seen) {
            if !seen {
                accumulator.uncounted();
            }
        }
    }
    let columns = accumulators
        .into_iter()
        .enumerate()
        .map(|(i, accumulator)| accumulator.finish(resolved, i, rows))
        .collect();
    TableSummary { rows, columns }
}

/// One column's extremes while the blocks are walked, each as the stored text
/// and its key under the kind the set was chosen by, so two blocks' bounds
/// are compared exactly as gathering ordered them ([`compare_keys_stored`],
/// `docs/design/decisions.md`, "D79"); and the union of its
/// dictionaries, borrowed from the map, while every group so far has kept one
/// — billed to nothing, since it holds at most a slot per entry the map
/// already keeps, and only while one table is summarized.
struct Accumulator<'a> {
    /// The kind bounds are read by, or `None` where this semantics believes
    /// none for the column or reads a `character(n)`'s, whose stored set holds
    /// no value the column emits.
    kind: Option<CompareKind>,
    nulls: u64,
    nulls_complete: bool,
    min: Option<(OrderKey, String, bool)>,
    max: Option<(OrderKey, String, bool)>,
    complete: bool,
    distinct: Option<HashSet<&'a str>>,
    sum: Option<i128>,
    value_bytes: Option<u64>,
}

impl<'a> Accumulator<'a> {
    /// A block said nothing about this column: neither its NULLs, its
    /// extremes nor its distinct values cover the table any more.
    fn uncounted(&mut self) {
        self.nulls_complete = false;
        self.complete = false;
        self.distinct = None;
        self.sum = None;
        self.value_bytes = None;
    }

    /// Add a block's dictionary to the union, but each entry `nulled` says the
    /// query reads as NULL, or give the union up where the block kept none of
    /// field text or any of its `groups` groups kept none.
    fn union(
        &mut self,
        dictionary: Option<&'a ColumnDictionary>,
        groups: usize,
        nulled: impl Fn(&str) -> bool,
    ) {
        let Some(distinct) = &mut self.distinct else { return };
        let Some(dictionary) = dictionary.filter(|d| d.groups.len() == groups) else {
            self.distinct = None;
            return;
        };
        // Every group's own entries, not the block's list: a list may keep an
        // entry no group still names.
        for group in &dictionary.groups {
            let Some(indices) = group else {
                self.distinct = None;
                return;
            };
            for &index in indices {
                match dictionary.entries.get(index as usize) {
                    Some(entry) if nulled(entry) => false,
                    Some(entry) => distinct.insert(entry.as_str()),
                    None => {
                        self.distinct = None;
                        return;
                    }
                };
            }
        }
    }

    /// Fold one group's bounds into the extremes. Only a set chosen by
    /// [`Self::kind`] is folded, so every key is of that one kind, as
    /// [`compare_keys_stored`] asserts.
    ///
    /// **On a tie an exact bound replaces a truncated one**, and otherwise the
    /// first folded is kept: a truncated lower bound orders below every value
    /// of its group and a truncated upper bound above every one ([`Bounds`]),
    /// so an exact bound keyed alike is a value the column holds at its
    /// extreme.
    fn fold(&mut self, bounds: &Bounds) {
        let Some(kind) = self.kind.as_ref() else { return };
        for (end, text, exact, further) in [
            (&mut self.min, &bounds.min, bounds.min_exact, std::cmp::Ordering::Less),
            (&mut self.max, &bounds.max, bounds.max_exact, std::cmp::Ordering::Greater),
        ] {
            let Some(key) = order_key(kind, text) else {
                self.complete = false;
                continue;
            };
            let replaces = end.as_ref().is_none_or(|(held, _, held_exact)| {
                let ordering = compare_keys_stored(&key, held);
                ordering == further
                    || (ordering == std::cmp::Ordering::Equal && exact && !held_exact)
            });
            if replaces {
                *end = Some((key, text.clone(), exact));
            }
        }
    }

    fn finish(self, resolved: &ResolvedSchema, i: usize, rows: u64) -> ColumnSummary {
        let mut complete = self.complete && self.kind.is_some();
        let column = &resolved.columns()[i];
        let mut decode = |end: Option<(OrderKey, String, bool)>| {
            let (_, text, exact) = end?;
            match decode_field(column, &text) {
                Some(value) => Some(Bound { value, exact }),
                None => {
                    complete = false;
                    None
                }
            }
        };
        let min = decode(self.min);
        let max = decode(self.max);
        let valued = self.nulls_complete && self.nulls < rows;
        let sum = self.sum.filter(|_| valued && keeps_sums(column.data_type()));
        ColumnSummary {
            nulls: self.nulls,
            nulls_complete: self.nulls_complete,
            min,
            max,
            bounds_complete: complete,
            distinct: self.distinct.map(|distinct| distinct.len() as u64),
            sum,
            value_bytes: self.value_bytes,
        }
    }
}

/// **The order every partition of a replay emits each column of `resolved`
/// in, under Arrow's comparator** — the order DataFusion sorts by — or
/// [`Sortedness::Unsorted`] where the map proves none. `matches` are the
/// table's blocks in `database`, statistics included, and `runs` each partition's blocks by
/// header offset, in the order it replays them; a block may repeat, once per
/// piece of it.
///
/// A column is proved `Ascending` or `Descending` only where all three hold:
///
/// - **Every block holding a row recorded that order, or `Constant`**, in the
///   stored set keyed by the kind DataFusion orders the column by and under
///   the DDL it was gathered under, as [`table_summary`] reads its bounds. A
///   constant block is in both orders, so where every block is, the boundaries
///   alone decide, `Ascending` before `Descending`: DataFusion keeps one
///   ordering a column.
/// - **No group of any block holds a NULL.** A block's row order skips its
///   NULLs, and an ordering says where they fall.
/// - **Each boundary a partition crosses is in that order**: ascending, the
///   upper bound of the earlier block's last group holding a row is at most
///   the lower bound of the later block's first. A sorted block's extremes
///   are in those groups, and a clipped lower bound or a successor for an
///   upper one still bounds. A block that holds no row is not between the
///   two; one the plan pruned whole is, its header's empty run keeping it in
///   the partition.
///
/// A float qualifies like any other column: its bounds and row order are
/// kept with its zeros told apart as Arrow sorts them
/// ([`compare_keys_stored`]). Partitions are not cut at block boundaries
/// to spare the third condition (`docs/design/decisions.md`, "D51").
pub(crate) fn partition_orders(
    matches: &[CopyBlock],
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    resolved: &ResolvedSchema,
    runs: &[Vec<u64>],
    reading: StatisticsView,
) -> Vec<Sortedness> {
    (0..resolved.schema().fields().len())
        .map(|i| {
            column_order(matches, metadata, database, resolved, i, runs, reading)
                .unwrap_or(Sortedness::Unsorted)
        })
        .collect()
}

/// [`partition_orders`] for column `i`, read in `reading` — a value it takes
/// as NULL being one, so a column holding one is proved in no order
/// (`docs/design/decisions.md`, "D98") — and `None` wherever it proves
/// nothing.
fn column_order(
    matches: &[CopyBlock],
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    resolved: &ResolvedSchema,
    i: usize,
    runs: &[Vec<u64>],
    reading: StatisticsView,
) -> Option<Sortedness> {
    let kind =
        resolved.columns()[i].comparison().bounds_read_by(ComparisonSemantics::DataFusion)?;
    let name = resolved.schema().field(i).name();
    let mut order = None;
    // Each block's first and last group holding a row, by header offset. A
    // block holding no row has none, and orders nothing.
    let mut edges: HashMap<u64, (&Bounds, &Bounds)> = HashMap::new();
    for block in matches {
        let statistics = block.statistics.as_deref()?;
        if statistics.columns.len() != block.header.columns.len() {
            return None;
        }
        let c = block.header.columns.iter().position(|column| column == name)?;
        let column = statistics.columns[c].as_ref()?;
        let qualified = block.header.qualified_name();
        if !believed_under(column, declared_column(metadata, database, &qualified, name)) {
            return None;
        }
        // deficiency: KD146 — a column holding a NULL is declared in no order
        // even where every NULL leads or trails its values, as `NULLS FIRST`
        // or `NULLS LAST` would state: a block's row order is gathered over its
        // non-NULL values only (pgdt-mapping's `RowOrder`), and nothing records
        // where its NULLs fall against them. So such a column orders no
        // partition, and a Parquet export's `sorting_columns` cannot name it.
        // NULLs among the values leave no order to declare. **(b) owned by
        // P37**: a block recording whether its NULLs lead, trail or interleave
        // its values, and the declaration stating which.
        if column.counted_groups() != statistics.groups.len()
            || (0..statistics.groups.len()).any(|g| column.null_count(g, reading) != Some(0))
        {
            return None;
        }
        let gathered = stored_resolution(&block.header, metadata, database);
        let set = bounds_set_keyed_by(&gathered.columns()[c].bounds_kinds(), &kind)?;
        if column.listed_groups(set)? != statistics.groups.len() {
            return None;
        }
        let sortedness = column.sortedness(set, reading)?;
        let mut holding = statistics
            .groups
            .iter()
            .enumerate()
            .filter(|(_, group)| group.rows > 0)
            .map(|(g, _)| column.group_bounds(set, reading, g));
        let Some(first) = holding.next() else { continue };
        let last = holding.next_back().unwrap_or(first);
        match (sortedness, order) {
            (Sortedness::Unsorted, _) => return None,
            (Sortedness::Constant, _) => {}
            (recorded, None) => order = Some(recorded),
            (recorded, Some(held)) if recorded != held => return None,
            _ => {}
        }
        edges.insert(block.header_offset, (first?, last?));
    }
    // Where every block holding a row is constant, the edges alone decide a
    // direction, ascending first.
    let directions = match order {
        Some(order) => vec![order],
        None if edges.is_empty() => return None,
        None => vec![Sortedness::Ascending, Sortedness::Descending],
    };
    directions.into_iter().find(|&order| {
        edges_in_order(&edges, runs, order, |text| order_key(&kind, text)).unwrap_or(false)
    })
}

/// Whether each boundary a partition of `runs` crosses between two of
/// `edges`' blocks is in `order`, `None` where a bound does not key.
fn edges_in_order(
    edges: &HashMap<u64, (&Bounds, &Bounds)>,
    runs: &[Vec<u64>],
    order: Sortedness,
    key: impl Fn(&str) -> Option<OrderKey>,
) -> Option<bool> {
    for run in runs {
        let mut offsets = run.clone();
        offsets.dedup();
        let blocks: Vec<_> = offsets.iter().filter_map(|offset| edges.get(offset)).collect();
        for pair in blocks.windows(2) {
            let ((_, earlier), (later, _)) = (pair[0], pair[1]);
            let (high, low) = match order {
                Sortedness::Descending => (&later.max, &earlier.min),
                _ => (&earlier.max, &later.min),
            };
            if compare_keys_stored(&key(high)?, &key(low)?) == std::cmp::Ordering::Greater {
                return Some(false);
            }
        }
    }
    Some(true)
}

#[cfg(test)]
mod tests;
