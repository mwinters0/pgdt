//! What `pgdq info --detail` says about gathered statistics, from the cache
//! alone (`docs/design/decisions.md`, "D61").
//!
//! **Counts, rolled up per table and column; never a group's values**, which
//! `--json` exports block by block with no rollup (`docs/design/decisions.md`,
//! "D67"). A rollup needs no merge rule, being sums: every block whose header
//! names the table counts toward it — a partition root's leaves included where
//! the dump names the root in their headers (`--load-via-partition-root`),
//! which is what a query of the root reads — and a column is matched across
//! blocks by name.
//!
//! **An empty group is outside every share.** A group no row starts in holds no
//! value to bound or list, so the groups a column's bounds and dictionary are
//! counted against are the ones holding a row, and the table's observed rows
//! and bytes per group are averaged over those alone.

use std::collections::HashMap;

use pgdump_query::{CopyBlock, DumpIndex, Sortedness};

/// One table's statistics over the blocks of it the map holds.
#[derive(Debug, PartialEq, Eq)]
pub struct TableStatistics<'a> {
    pub database: &'a Option<String>,
    pub table: String,
    /// The table's blocks in the map.
    pub blocks: u64,
    /// Of those, the blocks carrying statistics.
    pub gathered_blocks: u64,
    /// Every group size those blocks were gathered at, ascending, once each.
    pub group_sizes: Vec<u64>,
    /// Their groups, empty ones included.
    pub groups: u64,
    /// Of those, the groups no row starts in.
    pub empty_groups: u64,
    /// The rows those groups hold.
    pub rows: u64,
    /// The bytes those groups extend over, each from its first row's start to
    /// its last row's end.
    pub bytes: u64,
    /// Every column any of the table's blocks names, in first-seen order.
    pub columns: Vec<ColumnStatisticsSummary<'a>>,
}

/// One column's statistics over the table's blocks.
#[derive(Debug, PartialEq, Eq)]
pub struct ColumnStatisticsSummary<'a> {
    pub name: &'a str,
    /// The blocks gathering this column.
    pub gathered_blocks: u64,
    /// Their groups holding a row: what the two counts below are shares of.
    pub groups_with_rows: u64,
    /// Of those, the groups carrying bounds — none of a group holding only
    /// NULLs, so an all-NULL column reads bounds in none of its groups.
    pub groups_with_bounds: u64,
    /// Of those, the groups carrying a dictionary.
    pub groups_with_dictionary: u64,
    /// The gathering blocks that keep bounds for the column, by its row order
    /// over each; all zero where no block keeps bounds.
    pub sortedness: SortednessCounts,
    /// The gathering blocks that keep a dictionary for the column.
    pub dictionary_blocks: u64,
}

/// Blocks per [`Sortedness`] state.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SortednessCounts {
    pub ascending: u64,
    pub descending: u64,
    pub unsorted: u64,
}

impl SortednessCounts {
    fn total(&self) -> u64 {
        self.ascending + self.descending + self.unsorted
    }
}

/// Every table the map holds a block of, in the file order of its first block.
pub fn table_statistics(index: &DumpIndex) -> Vec<TableStatistics<'_>> {
    let mut tables: Vec<TableStatistics<'_>> = Vec::new();
    let mut position: HashMap<(&Option<String>, String), usize> = HashMap::new();
    for block in index.blocks() {
        let key = (&block.database, block.header.qualified_name());
        let at = *position.entry(key).or_insert_with_key(|(database, table)| {
            tables.push(TableStatistics {
                database,
                table: table.clone(),
                blocks: 0,
                gathered_blocks: 0,
                group_sizes: Vec::new(),
                groups: 0,
                empty_groups: 0,
                rows: 0,
                bytes: 0,
                columns: Vec::new(),
            });
            tables.len() - 1
        });
        tables[at].add(block);
    }
    tables
}

impl<'a> TableStatistics<'a> {
    fn add(&mut self, block: &'a CopyBlock) {
        self.blocks += 1;
        let indices: Vec<usize> =
            block.header.columns.iter().map(|name| self.column_index(name)).collect();
        let Some(statistics) = &block.statistics else { return };
        self.gathered_blocks += 1;
        if let Err(at) = self.group_sizes.binary_search(&statistics.group_size) {
            self.group_sizes.insert(at, statistics.group_size);
        }
        let with_rows: Vec<bool> = statistics.groups.iter().map(|g| g.rows > 0).collect();
        self.groups += statistics.groups.len() as u64;
        self.empty_groups += with_rows.iter().filter(|&&r| !r).count() as u64;
        self.rows += statistics.groups.iter().map(|g| g.rows).sum::<u64>();
        self.bytes += statistics.groups.iter().map(|g| g.bytes).sum::<u64>();
        let counted = |present: &mut dyn Iterator<Item = bool>| {
            present.zip(&with_rows).filter(|&(present, &rows)| present && rows).count() as u64
        };
        for (&at, column) in indices.iter().zip(&statistics.columns) {
            let Some(column) = column else { continue };
            let summary = &mut self.columns[at];
            summary.gathered_blocks += 1;
            summary.groups_with_rows += with_rows.iter().filter(|&&r| r).count() as u64;
            if let Some(bounds) = &column.bounds {
                summary.groups_with_bounds +=
                    counted(&mut bounds.groups.iter().map(Option::is_some));
                match bounds.sortedness {
                    Sortedness::Ascending => summary.sortedness.ascending += 1,
                    Sortedness::Descending => summary.sortedness.descending += 1,
                    Sortedness::Unsorted => summary.sortedness.unsorted += 1,
                }
            }
            if let Some(dictionary) = &column.dictionary {
                summary.dictionary_blocks += 1;
                summary.groups_with_dictionary +=
                    counted(&mut dictionary.groups.iter().map(Option::is_some));
            }
        }
    }

    fn column_index(&mut self, name: &'a str) -> usize {
        if let Some(at) = self.columns.iter().position(|c| c.name == name) {
            return at;
        }
        self.columns.push(ColumnStatisticsSummary {
            name,
            gathered_blocks: 0,
            groups_with_rows: 0,
            groups_with_bounds: 0,
            groups_with_dictionary: 0,
            sortedness: SortednessCounts::default(),
            dictionary_blocks: 0,
        });
        self.columns.len() - 1
    }

    /// The table's line: over how many of its blocks statistics exist, and
    /// the group size configured beside the rows and bytes a group was
    /// observed to hold.
    pub fn line(&self) -> String {
        let over = format!(
            "{}: statistics over {} of {} block(s)",
            self.table, self.gathered_blocks, self.blocks
        );
        if self.gathered_blocks == 0 {
            return over;
        }
        let sizes: Vec<String> = self.group_sizes.iter().map(u64::to_string).collect();
        let with_rows = self.groups - self.empty_groups;
        let mut line = format!(
            "{over}, group size {} bytes; {} rows and {} bytes per group over {} group(s)",
            sizes.join(", "),
            mean(self.rows, with_rows),
            mean(self.bytes, with_rows),
            self.groups,
        );
        if self.empty_groups > 0 {
            line.push_str(&format!(", {} empty", self.empty_groups));
        }
        line
    }
}

impl ColumnStatisticsSummary<'_> {
    /// The column's line, `blocks` being the table's.
    pub fn line(&self, blocks: u64) -> String {
        if self.gathered_blocks == 0 {
            return format!("{}: not gathered", self.name);
        }
        let groups = self.groups_with_rows;
        let bounds = match self.sortedness.total() {
            0 => "no bounds".to_string(),
            kept => format!(
                "{}, bounds in {} of {groups} group(s)",
                self.sortedness.label(kept, self.gathered_blocks),
                self.groups_with_bounds
            ),
        };
        let dictionary = match self.dictionary_blocks {
            0 => "no dictionary".to_string(),
            _ => format!("dictionary in {} of {groups} group(s)", self.groups_with_dictionary),
        };
        format!(
            "{}: over {} of {blocks} block(s), {bounds}, {dictionary}",
            self.name, self.gathered_blocks
        )
    }
}

impl SortednessCounts {
    /// One state's word where every gathering block keeps bounds in that
    /// state, and otherwise each state's count of the gathering blocks.
    fn label(&self, kept: u64, gathered: u64) -> String {
        let states = [
            ("ascending", self.ascending),
            ("descending", self.descending),
            ("unsorted", self.unsorted),
        ];
        let present: Vec<(&str, u64)> = states.into_iter().filter(|&(_, n)| n > 0).collect();
        if let [(word, _)] = present[..]
            && kept == gathered
        {
            return word.to_string();
        }
        let parts: Vec<String> = present.iter().map(|(word, n)| format!("{word} in {n}")).collect();
        format!("{} of {gathered} block(s)", parts.join(", "))
    }
}

/// `total / count` rounded to the nearest whole number, zero over nothing.
fn mean(total: u64, count: u64) -> u64 {
    (total + count / 2).checked_div(count).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pgdump_query::{
        BlockStatistics, Bounds, ColumnBounds, ColumnDictionary, ColumnStatistics, CopyHeader,
        DataBlock, RowGroup, Span, SpanBody,
    };

    use super::*;

    fn header(table: &str, columns: &[&str]) -> CopyHeader {
        CopyHeader {
            schema: Some("public".into()),
            table: table.into(),
            columns: columns.iter().map(|c| c.to_string()).collect(),
        }
    }

    fn bounds() -> Option<Bounds> {
        Some(Bounds { min: "1".into(), max: "2".into(), max_exact: true })
    }

    /// A column keeping bounds in `bounded` of its groups, in `sortedness`, and
    /// a dictionary in every group.
    fn column(sortedness: Sortedness, bounded: &[bool]) -> Option<ColumnStatistics> {
        Some(ColumnStatistics {
            declared_type: Some("integer".into()),
            collation: None,
            null_counts: vec![0; bounded.len()],
            bounds: Some(ColumnBounds {
                sortedness,
                groups: bounded.iter().map(|&b| if b { bounds() } else { None }).collect(),
            }),
            dictionary: Some(ColumnDictionary {
                entries: vec!["1".into()],
                groups: bounded.iter().map(|_| Some(vec![0])).collect(),
            }),
        })
    }

    fn block(
        header: CopyHeader,
        database: Option<&str>,
        statistics: Option<BlockStatistics>,
    ) -> Span {
        Span {
            start: 0,
            end: 0,
            database: database.map(String::from),
            text: None,
            toc: None,
            toc_owned: false,
            body: SpanBody::Data(DataBlock::Copy(CopyBlock {
                header,
                database: database.map(String::from),
                header_offset: 0,
                data_offset: 0,
                terminator_offset: 0,
                end_offset: 0,
                row_count: 0,
                partition_root: None,
                statistics: statistics.map(Arc::new),
                array_shapes: Vec::new(),
            })),
        }
    }

    fn groups(rows: &[u64]) -> Vec<RowGroup> {
        rows.iter().map(|&rows| RowGroup { rows, bytes: rows * 10 }).collect()
    }

    #[test]
    fn a_tables_blocks_roll_up_and_an_empty_group_is_outside_every_share() {
        let index = DumpIndex {
            spans: vec![
                block(
                    header("t", &["id", "v"]),
                    None,
                    Some(BlockStatistics {
                        group_size: 64,
                        groups: groups(&[3, 0, 5]),
                        columns: vec![column(Sortedness::Ascending, &[true, false, true]), None],
                    }),
                ),
                block(header("other", &["x"]), None, None),
                block(
                    header("t", &["id", "v"]),
                    None,
                    Some(BlockStatistics {
                        group_size: 32,
                        groups: groups(&[4]),
                        columns: vec![column(Sortedness::Unsorted, &[false]), None],
                    }),
                ),
                block(header("t", &["id", "v"]), None, None),
            ],
            ..DumpIndex::default()
        };
        let tables = table_statistics(&index);
        assert_eq!(
            tables.iter().map(|t| t.table.as_str()).collect::<Vec<_>>(),
            ["public.t", "public.other"]
        );
        let t = &tables[0];
        assert_eq!((t.blocks, t.gathered_blocks, t.group_sizes.as_slice()), (3, 2, &[32, 64][..]));
        assert_eq!((t.groups, t.empty_groups, t.rows, t.bytes), (4, 1, 12, 120));
        assert_eq!(
            t.line(),
            "public.t: statistics over 2 of 3 block(s), group size 32, 64 bytes; 4 rows and 40 \
             bytes per group over 4 group(s), 1 empty"
        );
        let id = &t.columns[0];
        assert_eq!((id.gathered_blocks, id.groups_with_rows), (2, 3));
        assert_eq!((id.groups_with_bounds, id.groups_with_dictionary), (2, 3));
        assert_eq!(
            id.line(t.blocks),
            "id: over 2 of 3 block(s), ascending in 1, unsorted in 1 of 2 block(s), bounds in 2 \
             of 3 group(s), dictionary in 3 of 3 group(s)"
        );
        assert_eq!(t.columns[1].line(t.blocks), "v: not gathered");
        assert_eq!(tables[1].line(), "public.other: statistics over 0 of 1 block(s)");
    }

    #[test]
    fn one_state_is_named_alone_and_a_column_without_bounds_says_so() {
        let mut unbounded = column(Sortedness::Ascending, &[true]);
        if let Some(c) = &mut unbounded {
            c.bounds = None;
            c.dictionary = None;
        }
        let index = DumpIndex {
            spans: vec![
                block(
                    header("t", &["id", "doc"]),
                    Some("a"),
                    Some(BlockStatistics {
                        group_size: 1 << 20,
                        groups: groups(&[2]),
                        columns: vec![column(Sortedness::Descending, &[true]), unbounded],
                    }),
                ),
                block(header("t", &["id"]), Some("b"), None),
            ],
            ..DumpIndex::default()
        };
        let tables = table_statistics(&index);
        assert_eq!(tables.len(), 2, "a table is keyed by its database too");
        let t = &tables[0];
        assert_eq!(
            t.columns[0].line(t.blocks),
            "id: over 1 of 1 block(s), descending, bounds in 1 of 1 group(s), dictionary in 1 of \
             1 group(s)"
        );
        assert_eq!(
            t.columns[1].line(t.blocks),
            "doc: over 1 of 1 block(s), no bounds, no dictionary"
        );
    }

    #[test]
    fn a_mean_rounds_to_the_nearest_and_is_zero_over_nothing() {
        assert_eq!((mean(5, 2), mean(4, 3), mean(7, 0)), (3, 1, 0));
    }
}
