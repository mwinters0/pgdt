//! What a complete map's statistics say about a whole table: its rows, and
//! per column its NULLs, its extremes in one comparison semantics, and its
//! distinct values where the dictionaries hold them all.
//!
//! [`crate::prune`] reads the same stored sets per group to skip one; this
//! folds every group of every block of a table into one answer, for an
//! embedder that plans over a table rather than reads it
//! (`docs/design/decisions.md`, "D89").
//!
//! **A summary says whether it is complete, and never how nearly.** A caller
//! that answers a query from it needs to know that nothing was left out; a
//! caller estimating does not care by how much, and a partial extreme is
//! still a bound on the whole table.

use std::collections::HashSet;

use arrow::array::ArrayRef;

use crate::batch::decode_field;
use crate::gather::{clips, declared_columns, dictionary_holds_field_text, stored_resolution};
use crate::index::{DumpIndex, TableName};
use crate::pgtype::{CompareKind, ComparisonSemantics, bounds_set_keyed_by};
use crate::predicate::ValueKey;
use crate::resolve::ResolvedSchema;
use crate::statistics::{Bounds, ColumnDictionary};

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
    /// bound's text does not read back as a value of the column's type.
    pub min: Option<Bound>,
    pub max: Option<Bound>,
    /// Whether every group of every block contributed — an unbounded group
    /// holding nothing but NULLs counts as contributing, having no value to
    /// bound. `false` makes [`Self::min`] and [`Self::max`] bounds on the
    /// table rather than its extremes.
    pub bounds_complete: bool,
    /// How many distinct non-NULL values the column emits, where every group
    /// of every block kept a dictionary of its field texts: the size of their
    /// union. `None` wherever one group did not — the union being only a
    /// floor there — and for a `character` column, whose entries give up the
    /// trailing blanks it emits (`docs/design/decisions.md`, "D89").
    ///
    /// Distinct text is a distinct value in the column's Arrow type as in its
    /// text: each type is written in one form (I4, I40, I41), a
    /// `timestamptz` in one zone a table (I48).
    pub distinct: Option<u64>,
}

/// One end of a column's range.
#[derive(Debug, Clone)]
pub struct Bound {
    /// A one-element array of the column's Arrow type ([`decode_field`]).
    pub value: ArrayRef,
    /// Whether the value is one the column holds, rather than a bound outside
    /// every value ([`crate::statistics::Bounds::max_exact`]).
    pub exact: bool,
}

/// Summarize `table` over `index`, a complete map, against the schema a query
/// of it resolves and in the semantics that query compares in.
///
/// **A block's bounds are read exactly as pruning reads them** — the set
/// gathering stored under the kind a term in `semantics` compares by, and
/// only where the DDL the column was gathered under still stands
/// (`docs/design/decisions.md`, "D78", "D79"). A block with no statistics
/// leaves every column incomplete and its rows counted all the same.
pub fn table_summary(
    index: &DumpIndex,
    table: &TableName,
    resolved: &ResolvedSchema,
    semantics: ComparisonSemantics,
) -> TableSummary {
    let metadata = index.metadata.as_ref();
    let fields = resolved.schema.fields();
    let mut accumulators: Vec<Accumulator> = (0..fields.len())
        .map(|i| {
            let kind = resolved.comparisons[i]
                .bounds_read_by(semantics)
                // A `character(n)`'s set is stored without the trailing
                // blanks its comparison ignores, so its bounds are not values
                // the column emits and no extreme can be read off them.
                .filter(|kind| *kind != CompareKind::PaddedText);
            Accumulator {
                kind,
                nulls: 0,
                nulls_complete: true,
                min: None,
                max: None,
                complete: true,
                distinct: Some(HashSet::new()),
            }
        })
        .collect();
    let mut rows = 0;
    for block in index.blocks_of(table) {
        rows += block.row_count;
        let Some(statistics) = block.statistics.as_deref() else {
            accumulators.iter_mut().for_each(Accumulator::uncounted);
            continue;
        };
        if statistics.columns.len() != block.header.columns.len() {
            accumulators.iter_mut().for_each(Accumulator::uncounted);
            continue;
        }
        let declared =
            declared_columns(metadata, block.database.as_deref(), &block.header.qualified_name());
        let gathered = stored_resolution(&block.header, metadata, block.database.as_deref());
        let mut seen = vec![false; fields.len()];
        for (c, name) in block.header.columns.iter().enumerate() {
            let Some(i) = fields.iter().position(|field| field.name() == name) else { continue };
            seen[i] = true;
            let accumulator = &mut accumulators[i];
            let Some(column) = statistics.columns[c].as_ref() else {
                accumulator.uncounted();
                continue;
            };
            accumulator.nulls += column.null_counts.iter().sum::<u64>();
            if column.null_counts.len() != statistics.groups.len() {
                accumulator.nulls_complete = false;
            }
            // The bounds, the row order and the dictionary are believed only
            // under the DDL they were gathered under; the NULL counts above
            // are read off the text and are believed regardless (D78).
            let def = declared.and_then(|columns| columns.iter().find(|d| &d.name == name));
            let believed = column.declared_type.as_deref() == def.map(|d| d.declared_type.as_str())
                && column.collation.as_deref() == def.and_then(|d| d.collation.as_deref());
            let dictionary = column
                .dictionary
                .as_ref()
                .filter(|_| believed && dictionary_holds_field_text(&gathered, c));
            accumulator.union(dictionary, statistics.groups.len());
            let stored = accumulator
                .kind
                .as_ref()
                .filter(|_| believed)
                .and_then(|kind| bounds_set_keyed_by(&gathered.bounds_kinds(c), kind))
                .and_then(|set| column.bounds_in(set));
            let Some(stored) = stored else {
                accumulator.complete = false;
                continue;
            };
            let kind = accumulator.kind.clone().expect("a stored set was found for it");
            for (g, (group, bounds)) in statistics.groups.iter().zip(&stored.groups).enumerate() {
                match bounds {
                    Some(bounds) => accumulator.fold(&kind, bounds),
                    // A group whose every row is NULL has no value to bound,
                    // so it leaves the column's extremes complete; one that
                    // lost a value to gathering does not.
                    None if column.null_counts.get(g).copied() == Some(group.rows) => {}
                    None => accumulator.complete = false,
                }
            }
            if stored.groups.len() != statistics.groups.len() {
                accumulator.complete = false;
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
        .map(|(i, accumulator)| accumulator.finish(resolved, i))
        .collect();
    TableSummary { rows, columns }
}

/// One column's extremes while the blocks are walked, each as the stored text
/// and its key, so two blocks' bounds are compared exactly as the kind that
/// stored them orders; and the union of its dictionaries, borrowed from the
/// map, while every group so far has kept one.
struct Accumulator<'a> {
    /// The kind bounds are read by, or `None` where this semantics believes
    /// none for the column.
    kind: Option<CompareKind>,
    nulls: u64,
    nulls_complete: bool,
    min: Option<(ValueKey, String, bool)>,
    max: Option<(ValueKey, String, bool)>,
    complete: bool,
    distinct: Option<HashSet<&'a str>>,
}

impl<'a> Accumulator<'a> {
    /// A block said nothing about this column: neither its NULLs, its
    /// extremes nor its distinct values cover the table any more.
    fn uncounted(&mut self) {
        self.nulls_complete = false;
        self.complete = false;
        self.distinct = None;
    }

    /// Add a block's dictionary to the union, or give the union up where the
    /// block kept none of field text or any of its `groups` groups kept none.
    fn union(&mut self, dictionary: Option<&'a ColumnDictionary>, groups: usize) {
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
                    Some(entry) => distinct.insert(entry.as_str()),
                    None => {
                        self.distinct = None;
                        return;
                    }
                };
            }
        }
    }

    fn fold(&mut self, kind: &CompareKind, bounds: &Bounds) {
        // A keyed kind stores whole values; a bytewise one may have clipped
        // either end, and says so only of the upper (`gather::clips`).
        let min_exact = !clips(kind);
        if let Some(key) = ValueKey::of(kind, &bounds.min) {
            let lower = self
                .min
                .as_ref()
                .is_none_or(|(held, _, _)| key.compare(held) == std::cmp::Ordering::Less);
            if lower {
                self.min = Some((key, bounds.min.clone(), min_exact));
            }
        } else {
            self.complete = false;
        }
        if let Some(key) = ValueKey::of(kind, &bounds.max) {
            let higher = self
                .max
                .as_ref()
                .is_none_or(|(held, _, _)| key.compare(held) == std::cmp::Ordering::Greater);
            if higher {
                self.max = Some((key, bounds.max.clone(), bounds.max_exact));
            }
        } else {
            self.complete = false;
        }
    }

    fn finish(self, resolved: &ResolvedSchema, i: usize) -> ColumnSummary {
        let mut complete = self.complete && self.kind.is_some();
        let data_type = resolved.schema.field(i).data_type();
        let mut decode = |end: Option<(ValueKey, String, bool)>| {
            let (_, text, exact) = end?;
            match decode_field(data_type, &resolved.plans[i], &text) {
                Some(value) => Some(Bound { value, exact }),
                None => {
                    complete = false;
                    None
                }
            }
        };
        let min = decode(self.min);
        let max = decode(self.max);
        ColumnSummary {
            nulls: self.nulls,
            nulls_complete: self.nulls_complete,
            min,
            max,
            bounds_complete: complete,
            distinct: self.distinct.map(|distinct| distinct.len() as u64),
        }
    }
}
