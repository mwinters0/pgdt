//! Per-row-group column statistics, gathered by `map_file` and persisted in
//! the cache (`pgdump_query::statistics`).
//!
//! **Checked against the file, not against the gatherer.** Each group's rows
//! are re-read from the block's bytes by line, placed by where each line
//! starts, and ordered by a comparison written here for the column's type —
//! so a bound, a count or a dictionary is right or wrong about what the file
//! holds. The `statistics` fixture's shapes are themselves asserted by
//! `tests/statistics_fixture.rs`.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::num::NonZeroU64;
use std::path::Path;
use std::sync::Arc;

use pgdump_query::cache::{self, CacheMode, CacheStatus};
use pgdump_query::{
    BlockStatistics, CopyBlock, DEFAULT_MEMORY_BUDGET, DEFAULT_STATISTICS_GROUP_SIZE,
    DEFAULT_STATISTICS_MIN_ROWS, DICTIONARY_CAP, DumpIndex, GroupSizing, LocalFileSource, MapRun,
    Parallelism, STATISTICS_GROUP_CAP, STORED_VALUE_CAP, ScanOptions, Sortedness,
    StatisticsBackfill, StatisticsRequest, StatisticsSelection, StatisticsTarget,
    gather_block_statistics, map_file,
};

mod common;
use common::{VERSIONS, sandboxed, statistics_fixture};

/// A group size several `ordered` rows long and well under its block, so
/// `ordered` has several groups, some holding more distinct `high_card` texts
/// than [`DICTIONARY_CAP`].
const SMALL_GROUP: u64 = 4096;

fn request(selection: StatisticsSelection, group_size: u64) -> StatisticsRequest {
    let group_size = Some(NonZeroU64::new(group_size).unwrap());
    StatisticsRequest { selection, group_size, min_rows: None }
}

async fn gathered(dump: &Path, statistics: &StatisticsRequest) -> DumpIndex {
    gathered_with(dump, &ScanOptions::default(), statistics).await
}

async fn gathered_with(
    dump: &Path,
    options: &ScanOptions,
    statistics: &StatisticsRequest,
) -> DumpIndex {
    let source = LocalFileSource::open(dump).unwrap();
    let run = map_file(&source, options, &CacheMode::Disabled, statistics).await.unwrap();
    assert!(!run.interrupted);
    run.index
}

fn block<'a>(index: &'a DumpIndex, table: &str) -> &'a CopyBlock {
    index.blocks_for(table).next().unwrap_or_else(|| panic!("no block for {table}"))
}

fn statistics(block: &CopyBlock) -> &BlockStatistics {
    block.statistics.as_deref().unwrap_or_else(|| panic!("{} gathered nothing", block.header.table))
}

/// One row of a block as the file holds it: where its line starts relative to
/// the block's data, and its fields with `\N` as `None`. No fixture value needs
/// unescaping.
struct FileRow {
    offset: u64,
    fields: Vec<Option<String>>,
}

fn file_rows(dump: &Path, block: &CopyBlock) -> Vec<FileRow> {
    let bytes = std::fs::read(dump).unwrap();
    let data = &bytes[block.data_offset as usize..block.terminator_offset as usize];
    let mut offset = 0u64;
    let mut rows = Vec::new();
    for line in data.split_inclusive(|&b| b == b'\n') {
        let text = std::str::from_utf8(&line[..line.len() - 1]).unwrap();
        let fields = text.split('\t').map(|f| (f != "\\N").then(|| f.to_string())).collect();
        rows.push(FileRow { offset, fields });
        offset += line.len() as u64;
    }
    rows
}

/// PostgreSQL's order for the fixture's value types, written independently of
/// the library: integers and numerics by value with `NaN` above every number,
/// floats the same with `-0` equal to `0`, and text by its bytes.
fn order(declared: &str, a: &str, b: &str) -> Ordering {
    let number = |v: &str| match v {
        "NaN" => f64::NAN,
        "Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        _ => v.parse::<f64>().unwrap(),
    };
    match declared {
        "text" => a.as_bytes().cmp(b.as_bytes()),
        _ => {
            let (x, y) = (number(a), number(b));
            match (x.is_nan(), y.is_nan()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => x.partial_cmp(&y).unwrap(),
            }
        }
    }
}

/// Every statistic of `block` against the file: the groups' extents and row
/// counts, and per column its NULL counts, that each bound is on the right side
/// of every value in its group — an exact one being a value there — and that
/// each dictionary is exactly the group's distinct texts or absent past a cap.
fn assert_describes_the_file(dump: &Path, block: &CopyBlock, label: &str) {
    let stats = statistics(block);
    let rows = file_rows(dump, block);
    let n = stats.group_size;
    assert_eq!(stats.groups.iter().map(|g| g.rows).sum::<u64>(), block.row_count, "{label}");
    assert_eq!(
        stats.groups.iter().map(|g| g.bytes).sum::<u64>(),
        block.terminator_offset - block.data_offset,
        "{label}: the groups' extents tile the data"
    );
    let last = rows.last().map_or(0, |r| r.offset / n + 1);
    assert_eq!(stats.groups.len() as u64, last, "{label}: a group per index to the last row's");

    let mut start = 0usize;
    for (k, group) in stats.groups.iter().enumerate() {
        let members: Vec<&FileRow> =
            rows[start..].iter().take_while(|r| r.offset / n == k as u64).collect();
        start += members.len();
        assert_eq!(group.rows, members.len() as u64, "{label}: group {k}'s rows");
        if let Some(first) = members.first() {
            let end =
                rows.get(start).map_or(block.terminator_offset - block.data_offset, |r| r.offset);
            assert_eq!(group.bytes, end - first.offset, "{label}: group {k}'s extent");
        }
        for (c, column) in stats.columns.iter().enumerate() {
            let Some(column) = column else { continue };
            let declared = column.declared_type.as_deref().unwrap();
            let values: Vec<&str> = members.iter().filter_map(|r| r.fields[c].as_deref()).collect();
            let what = format!("{label}: group {k}, column {c} ({declared})");
            assert_eq!(column.null_counts[k], (members.len() - values.len()) as u64, "{what}");
            if let Some(bounds) = &column.bounds {
                match &bounds.groups[k] {
                    None => assert!(
                        values.is_empty() || values.iter().any(|v| v.len() > STORED_VALUE_CAP),
                        "{what}: values and no bounds"
                    ),
                    Some(b) => {
                        assert!(b.min.len() <= STORED_VALUE_CAP && b.max.len() <= STORED_VALUE_CAP);
                        for v in &values {
                            assert_ne!(
                                order(declared, &b.min, v),
                                Ordering::Greater,
                                "{what}: min {v}"
                            );
                            assert_ne!(
                                order(declared, &b.max, v),
                                Ordering::Less,
                                "{what}: max {v}"
                            );
                        }
                        assert!(
                            values.contains(&b.min.as_str())
                                || values.iter().any(|v| v.len() > STORED_VALUE_CAP),
                            "{what}: min"
                        );
                        assert_eq!(
                            b.max_exact,
                            values.contains(&b.max.as_str()),
                            "{what}: max_exact"
                        );
                    }
                }
            }
            if let Some(dictionary) = &column.dictionary {
                let distinct: BTreeSet<&str> = values.iter().copied().collect();
                let fits = distinct.len() <= DICTIONARY_CAP
                    && distinct.iter().all(|v| v.len() <= STORED_VALUE_CAP);
                match &dictionary.groups[k] {
                    Some(indices) => {
                        assert!(fits, "{what}: a dictionary past a cap");
                        let held: BTreeSet<&str> = indices
                            .iter()
                            .map(|&i| dictionary.entries[i as usize].as_str())
                            .collect();
                        assert_eq!(held, distinct, "{what}: dictionary");
                        assert_eq!(held.len(), indices.len(), "{what}: an entry twice");
                    }
                    None => assert!(!fits, "{what}: no dictionary where one fits"),
                }
            }
        }
    }
}

fn sortedness(block: &CopyBlock, column: &str) -> Option<Sortedness> {
    let at = block.header.columns.iter().position(|c| c == column).unwrap();
    statistics(block).columns[at].as_ref().unwrap().bounds.as_ref().map(|b| b.sortedness)
}

/// Every block of the fixture gathers at a small group size, on every major,
/// and every statistic describes the file.
#[tokio::test]
async fn every_statistic_describes_the_file() {
    for version in VERSIONS {
        let dump = statistics_fixture(version, "default");
        let index = gathered(&dump, &request(StatisticsSelection::All, SMALL_GROUP)).await;
        assert_eq!(index.blocks().count(), 3);
        for block in index.blocks() {
            assert_describes_the_file(
                &dump,
                block,
                &format!("{} on {version}", block.header.table),
            );
        }
        // Where the caps bind: `ordered`'s groups hold more distinct
        // `high_card` texts than a dictionary may, and never more `low_card`.
        let ordered = statistics(block(&index, "public.ordered"));
        assert!(ordered.groups.len() > 1, "ordered on {version}");
        let dictionary =
            |c: usize| ordered.columns[c].as_ref().unwrap().dictionary.as_ref().unwrap();
        assert!(dictionary(8).groups.iter().all(Option::is_some), "low_card on {version}");
        assert!(dictionary(9).groups.iter().any(Option::is_none), "high_card on {version}");
    }
}

/// The block-level order of every column that gets bounds, as the fixture's
/// schema states each one, and no bounds on text under the database's
/// default collation — which still gets a dictionary.
#[tokio::test]
async fn sortedness_is_the_blocks_row_order() {
    use Sortedness::{Ascending, Descending, Unsorted};
    for version in VERSIONS {
        let dump = statistics_fixture(version, "default");
        let index = gathered(&dump, &request(StatisticsSelection::All, SMALL_GROUP)).await;
        let ordered = block(&index, "public.ordered");
        for (column, expected) in [
            ("id", Ascending),
            ("reversed", Descending),
            ("stepped", Ascending),
            ("unsorted", Unsorted),
            ("constant", Ascending),
            ("all_null", Ascending),
            ("gappy", Ascending),
            ("single", Ascending),
            ("low_card", Unsorted),
            ("high_card", Unsorted),
            ("c_text", Ascending),
        ] {
            assert_eq!(sortedness(ordered, column), Some(expected), "{column} on {version}");
        }
        assert_eq!(sortedness(ordered, "default_text"), None, "default_text on {version}");
        let default_text = statistics(ordered).columns[11].as_ref().unwrap();
        assert!(default_text.dictionary.is_some(), "default_text's equality is exact");
        assert_eq!(default_text.collation, None);
        assert_eq!(
            statistics(ordered).columns[10].as_ref().unwrap().collation.as_deref(),
            Some("pg_catalog.\"C\"")
        );

        let specials = block(&index, "public.specials");
        for (column, expected) in
            [("f8", Ascending), ("f8_unsorted", Unsorted), ("f4", Ascending), ("n", Ascending)]
        {
            assert_eq!(sortedness(specials, column), Some(expected), "{column} on {version}");
        }
        assert_eq!(sortedness(block(&index, "public.long_value"), "v"), Some(Ascending));
    }
}

/// A value past the stored-value cap is bounded by a truncated text: a prefix
/// below it and a successor above it marked inexact, neither past the cap —
/// and a group size under the megabyte row leaves groups no row starts in.
#[tokio::test]
async fn a_long_value_is_bounded_by_truncated_texts() {
    for version in VERSIONS {
        let dump = statistics_fixture(version, "default");
        let index = gathered(&dump, &request(StatisticsSelection::All, 64)).await;
        let long = block(&index, "public.long_value");
        assert_describes_the_file(&dump, long, &format!("long_value on {version}"));
        let stats = statistics(long);
        assert!(stats.groups.iter().any(|g| g.rows == 0), "empty groups on {version}");
        let v = stats.columns[1].as_ref().unwrap();
        let truncated: Vec<_> =
            v.bounds.as_ref().unwrap().groups.iter().flatten().filter(|b| !b.max_exact).collect();
        assert_eq!(truncated.len(), 2, "the 300-byte and the megabyte value on {version}");
        assert!(v.dictionary.as_ref().unwrap().groups.iter().any(Option::is_none));
    }
}

/// **The default request gathers every statistic**: every column of every
/// block, at the mebibyte default group size, at which `ordered` is a single
/// group and the block holding the megabyte row is not — until the density
/// minimum, which no group of a few rows reaches, makes that block one group
/// too, and which a minimum of zero turns off. The request each block was
/// sized under is recorded beside its size.
#[tokio::test]
async fn the_default_request_gathers_every_column_at_a_mebibyte() {
    let dump = statistics_fixture(16, "default");
    assert_eq!(StatisticsRequest::default(), StatisticsRequest::ALL);
    let index = gathered(&dump, &StatisticsRequest::default()).await;
    let default_sizing = GroupSizing::Density { min_rows: DEFAULT_STATISTICS_MIN_ROWS };
    for block in index.blocks() {
        let statistics = statistics(block);
        assert_eq!(statistics.columns.len(), block.header.columns.len());
        assert!(statistics.columns.iter().all(Option::is_some), "{}", block.header.table);
        assert_eq!(statistics.sizing, default_sizing, "{}", block.header.table);
        assert_eq!(statistics.groups.len(), 1, "{}", block.header.table);
    }
    let ordered = statistics(block(&index, "public.ordered"));
    assert_eq!(ordered.group_size, pgdump_query::DEFAULT_STATISTICS_GROUP_SIZE);
    let long_value = statistics(block(&index, "public.long_value"));
    assert!(long_value.group_size > pgdump_query::DEFAULT_STATISTICS_GROUP_SIZE);

    let uncoarsened = StatisticsRequest { min_rows: Some(0), ..StatisticsRequest::ALL };
    let index = gathered(&dump, &uncoarsened).await;
    let long_value = statistics(block(&index, "public.long_value"));
    assert_eq!(long_value.group_size, pgdump_query::DEFAULT_STATISTICS_GROUP_SIZE);
    assert!(long_value.groups.len() > 1);
    assert_eq!(long_value.sizing, GroupSizing::Density { min_rows: 0 });
}

/// A selection tracks only what it names: a table, or one column, whose
/// other columns carry nothing, and every block it does not name gathers
/// nothing at all.
#[tokio::test]
async fn a_selection_gathers_only_what_it_names() {
    let dump = statistics_fixture(16, "default");
    let selection = StatisticsSelection::Only(vec![
        StatisticsTarget::Table("specials".to_string()),
        StatisticsTarget::Column { table: "public.ordered".to_string(), column: "id".to_string() },
    ]);
    let index = gathered(&dump, &request(selection, SMALL_GROUP)).await;
    let ordered = statistics(block(&index, "public.ordered"));
    assert!(ordered.columns[0].is_some());
    assert!(ordered.columns[1..].iter().all(Option::is_none));
    assert!(statistics(block(&index, "public.specials")).columns.iter().all(Option::is_some));
    assert!(block(&index, "public.long_value").statistics.is_none());
}

/// A request stating none gathers none — what a query's mapping pass always
/// asks.
#[tokio::test]
async fn a_request_stating_none_gathers_nothing() {
    let index = gathered(&statistics_fixture(16, "default"), &StatisticsRequest::NONE).await;
    assert!(index.blocks().all(|b| b.statistics.is_none()));
}

/// Statistics persist in the one cache file and load back exactly, and a
/// clone of the map shares each block's statistics rather than copying them.
#[tokio::test]
async fn statistics_round_trip_through_the_cache() {
    let (_dir, dump) = sandboxed(&statistics_fixture(16, "default"), "statistics.sql");
    let source = LocalFileSource::open(&dump).unwrap();
    let mode = CacheMode::Enabled(cache::colocated_path(&dump));
    let wanted = request(StatisticsSelection::All, SMALL_GROUP);
    let index = map_file(&source, &ScanOptions::default(), &mode, &wanted).await.unwrap().index;
    let clone = index.clone();
    for (a, b) in index.blocks().zip(clone.blocks()) {
        assert!(Arc::ptr_eq(a.statistics.as_ref().unwrap(), b.statistics.as_ref().unwrap()));
    }
    let CacheStatus::Valid { index: loaded, .. } =
        cache::load(&cache::colocated_path(&dump), &source).await.unwrap()
    else {
        panic!("a finished parse leaves a valid cache");
    };
    assert_eq!(loaded.spans, index.spans);
}

/// **A gathered block the leader splits holds the serial scan's statistics.**
/// The block is one the leader splits at this chunk and count
/// (`tests/map_file.rs`, `a_cancelled_parallel_region_banks_nothing_and_stays_resumable`;
/// `tests/wait_policy.rs` shows a gathering scan reaching the scheduler), and
/// at 256 bytes a group nearly every cut falls inside a group: the joined
/// bounds, dictionaries and row order — an ascending column and a descending
/// one — are the serial pass's. Every fixture is swept the same way by
/// `pgdump_query-cli/tests/determinism.rs`, byte for byte.
#[tokio::test]
async fn a_gathering_scan_is_the_serial_scan_whatever_the_worker_count() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("long_block.sql");
    let mut text = String::from(
        "CREATE TABLE public.t (\n    a integer,\n    b text COLLATE pg_catalog.\"C\",\n    c text\n);\n\n",
    );
    text.push_str("COPY public.t (a, b, c) FROM stdin;\n");
    for i in 0..2000 {
        text.push_str(&format!("{i}\t{:05}\tv{}\n", 2000 - i, i % 3));
    }
    text.push_str("\\.\n\nSELECT 1;\n");
    std::fs::write(&dump, &text).unwrap();
    let wanted = request(StatisticsSelection::All, 256);
    let serial = ScanOptions { chunk_size: 64, ..ScanOptions::default() };
    let serial_index = gathered_with(&dump, &serial, &wanted).await;
    for jobs in [2, 4, 8] {
        let parallel = ScanOptions {
            parallelism: Parallelism::workers(jobs, DEFAULT_MEMORY_BUDGET),
            ..serial.clone()
        };
        let index = gathered_with(&dump, &parallel, &wanted).await;
        let t = block(&index, "public.t");
        assert_eq!(statistics(t).groups.iter().map(|g| g.rows).sum::<u64>(), 2000);
        assert_eq!(sortedness(t, "a"), Some(Sortedness::Ascending), "{jobs} jobs");
        assert_eq!(sortedness(t, "b"), Some(Sortedness::Descending), "{jobs} jobs");
        assert_eq!(index.spans, serial_index.spans, "{jobs} jobs");
    }
}

/// **A value that does not key leaves its group without bounds on that
/// column, and its block `Unsorted` there**: a bound omitting it would not
/// cover its row. Its NULL count and dictionary stand. Hand-written, since no
/// `pg_dump` output holds such a value and the generated check cannot.
#[tokio::test]
async fn a_value_that_does_not_key_leaves_its_group_unbounded() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("unkeyed.sql");
    let mut text = String::from("CREATE TABLE public.t (\n    id integer\n);\n\n");
    text.push_str("COPY public.t (id) FROM stdin;\n");
    for line in ["0000001", "0000002", "000000x", "\\N", "0003", "0000004", "0000005"] {
        text.push_str(line);
        text.push('\n');
    }
    text.push_str("\\.\n\n");
    std::fs::write(&dump, &text).unwrap();
    // Sixteen bytes a group: the first two rows, then `x`, the NULL and `0003`,
    // then the last two.
    let index = gathered(&dump, &request(StatisticsSelection::All, 16)).await;
    let id = statistics(block(&index, "public.t")).columns[0].as_ref().unwrap();
    let bounds = id.bounds.as_ref().unwrap();
    assert_eq!(bounds.sortedness, Sortedness::Unsorted);
    assert!(bounds.groups[0].is_some());
    assert!(bounds.groups[1].is_none(), "the group holding `x`");
    assert!(bounds.groups[2].is_some());
    assert_eq!(id.null_counts, vec![0, 1, 0]);
    let dictionary = id.dictionary.as_ref().unwrap();
    let group: Vec<&str> = dictionary.groups[1]
        .as_ref()
        .unwrap()
        .iter()
        .map(|&i| dictionary.entries[i as usize].as_str())
        .collect();
    assert_eq!(group, vec!["000000x", "0003"]);
}

/// **A `character` dictionary entry is stored and measured without its
/// trailing blanks**, as its bounds are: a `character(300)` column of short
/// values keeps a dictionary, one entry per value however it is padded, where
/// a `character varying(300)` column holding the same padded texts is past the
/// cap. Hand-written, since no fixture declares a `character` column wider
/// than the cap.
#[tokio::test]
async fn a_character_dictionary_entry_drops_its_padding() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("padded.sql");
    let mut text = String::from(
        "CREATE TABLE public.t (\n    c character(300),\n    v character varying(300)\n);\n\n",
    );
    text.push_str("COPY public.t (c, v) FROM stdin;\n");
    for value in ["ab", "cd", "ab", "\\N"] {
        let padded = if value == "\\N" { value.to_string() } else { format!("{value:<300}") };
        text.push_str(&format!("{padded}\t{padded}\n"));
    }
    text.push_str("\\.\n\n");
    std::fs::write(&dump, &text).unwrap();
    let index = gathered(&dump, &request(StatisticsSelection::All, 1 << 20)).await;
    let columns = &statistics(block(&index, "public.t")).columns;
    let c = columns[0].as_ref().unwrap();
    let dictionary = c.dictionary.as_ref().expect("equality on `character` is exact");
    assert_eq!(dictionary.entries, vec!["ab", "cd"]);
    assert_eq!(dictionary.groups, vec![Some(vec![0, 1])]);
    assert_eq!(c.null_counts, vec![1]);
    let v = columns[1].as_ref().unwrap();
    assert_eq!(
        v.dictionary.as_ref().unwrap().groups,
        vec![None],
        "a padded `varchar` is past the cap"
    );
}

/// `map_file` over a cache at `dump`'s colocated path, and the run.
async fn mapped_into_cache(
    dump: &Path,
    options: &ScanOptions,
    statistics: &StatisticsRequest,
) -> MapRun {
    let source = LocalFileSource::open(dump).unwrap();
    let mode = CacheMode::Enabled(cache::colocated_path(dump));
    let run = map_file(&source, options, &mode, statistics).await.unwrap();
    assert!(!run.interrupted);
    run
}

/// **A block mapped without the statistics asked for is re-read for them, into
/// exactly what one gathering pass stores**: from no statistics, from the
/// default size re-asked at a stated one, from a table selection and from a
/// single column, on every major — and once it holds them, asking again re-reads
/// nothing.
#[tokio::test]
async fn a_block_lacking_the_requested_statistics_is_reread_into_what_one_pass_gathers() {
    let wanted = request(StatisticsSelection::All, SMALL_GROUP);
    let ordered = || "public.ordered".to_string();
    let earlier = [
        (StatisticsRequest::NONE, 3),
        (StatisticsRequest::ALL, 3),
        (
            request(
                StatisticsSelection::Only(vec![StatisticsTarget::Table(ordered())]),
                SMALL_GROUP,
            ),
            2,
        ),
        (
            request(
                StatisticsSelection::Only(vec![StatisticsTarget::Column {
                    table: ordered(),
                    column: "id".to_string(),
                }]),
                SMALL_GROUP,
            ),
            3,
        ),
    ];
    for version in VERSIONS {
        let reference = gathered(&statistics_fixture(version, "default"), &wanted).await;
        for (first, lacking) in &earlier {
            let label = format!("{first:?} then all at {SMALL_GROUP} on {version}");
            let (_dir, dump) = sandboxed(&statistics_fixture(version, "default"), "s.sql");
            let options = ScanOptions::default();
            let mapped = mapped_into_cache(&dump, &options, first).await;
            assert_eq!((mapped.lacking_statistics, mapped.backfilled), (0, 0), "{label}");

            let run = mapped_into_cache(&dump, &options, &wanted).await;
            assert_eq!((run.lacking_statistics, run.backfilled), (*lacking, *lacking), "{label}");
            assert_eq!(run.index.spans, reference.spans, "{label}");
            let source = LocalFileSource::open(&dump).unwrap();
            let CacheStatus::Valid { index: loaded, .. } =
                cache::load(&cache::colocated_path(&dump), &source).await.unwrap()
            else {
                panic!("{label}: a back-filled parse leaves a valid cache");
            };
            assert_eq!(loaded.spans, reference.spans, "{label}: the cache holds the back-fill");

            let again = mapped_into_cache(&dump, &options, &wanted).await;
            assert_eq!((again.lacking_statistics, again.backfilled), (0, 0), "{label}");
        }
    }
}

/// **An unstated group size re-reads only a missing column, at the size the
/// block already holds**, and keeps every column the block had; a block
/// holding no statistics is gathered at the default. A block gathered at any
/// size lacks nothing an unstated request asks.
#[tokio::test]
async fn an_unstated_size_keeps_the_size_a_block_was_gathered_at() {
    let dump_of = || statistics_fixture(16, "default");
    let one_column = request(
        StatisticsSelection::Only(vec![StatisticsTarget::Column {
            table: "public.ordered".to_string(),
            column: "id".to_string(),
        }]),
        SMALL_GROUP,
    );
    let (_dir, dump) = sandboxed(&dump_of(), "s.sql");
    mapped_into_cache(&dump, &ScanOptions::default(), &one_column).await;
    let run = mapped_into_cache(&dump, &ScanOptions::default(), &StatisticsRequest::ALL).await;
    assert_eq!((run.lacking_statistics, run.backfilled), (3, 3));
    let small = gathered(&dump_of(), &request(StatisticsSelection::All, SMALL_GROUP)).await;
    let default = gathered(&dump_of(), &StatisticsRequest::ALL).await;
    for table in ["public.long_value", "public.ordered", "public.specials"] {
        let expected = if table == "public.ordered" { &small } else { &default };
        assert_eq!(
            block(&run.index, table).statistics,
            block(expected, table).statistics,
            "{table}"
        );
    }

    let (_dir, dump) = sandboxed(&dump_of(), "s.sql");
    mapped_into_cache(&dump, &ScanOptions::default(), &request(StatisticsSelection::All, 64)).await;
    let run = mapped_into_cache(&dump, &ScanOptions::default(), &StatisticsRequest::ALL).await;
    assert_eq!((run.lacking_statistics, run.backfilled), (0, 0));
}

/// **A back-fill keeps every column the block already held**: a block
/// gathered whole and then asked for one column at another size is re-read
/// whole at that size, not narrowed to the column.
#[tokio::test]
async fn a_backfill_never_drops_a_column_the_block_held() {
    let dump_of = || statistics_fixture(16, "default");
    let (_dir, dump) = sandboxed(&dump_of(), "s.sql");
    let options = ScanOptions::default();
    mapped_into_cache(&dump, &options, &request(StatisticsSelection::All, SMALL_GROUP)).await;
    let one_column = request(
        StatisticsSelection::Only(vec![StatisticsTarget::Column {
            table: "public.ordered".to_string(),
            column: "id".to_string(),
        }]),
        64,
    );
    let run = mapped_into_cache(&dump, &options, &one_column).await;
    assert_eq!((run.lacking_statistics, run.backfilled), (1, 1));
    let whole_at_64 = gathered(&dump_of(), &request(StatisticsSelection::All, 64)).await;
    let small = gathered(&dump_of(), &request(StatisticsSelection::All, SMALL_GROUP)).await;
    for table in ["public.long_value", "public.ordered", "public.specials"] {
        let expected = if table == "public.ordered" { &whole_at_64 } else { &small };
        assert_eq!(
            block(&run.index, table).statistics,
            block(expected, table).statistics,
            "{table}"
        );
    }
}

/// A request naming one table re-reads that table's block alone; a request
/// gathering nothing re-reads nothing and keeps what the blocks hold.
#[tokio::test]
async fn a_backfill_rereads_only_the_blocks_its_request_tracks() {
    let (_dir, dump) = sandboxed(&statistics_fixture(16, "default"), "s.sql");
    let options = ScanOptions::default();
    mapped_into_cache(&dump, &options, &StatisticsRequest::NONE).await;
    let specials = StatisticsRequest {
        selection: StatisticsSelection::Only(vec![StatisticsTarget::Table("specials".to_string())]),
        group_size: None,
        min_rows: None,
    };
    let run = mapped_into_cache(&dump, &options, &specials).await;
    assert_eq!((run.lacking_statistics, run.backfilled), (1, 1));
    assert!(block(&run.index, "public.specials").statistics.is_some());
    assert!(block(&run.index, "public.ordered").statistics.is_none());

    let none = mapped_into_cache(&dump, &options, &StatisticsRequest::NONE).await;
    assert_eq!((none.lacking_statistics, none.backfilled), (0, 0));
    assert_eq!(none.index.spans, run.index.spans);
}

/// **The per-block entry point is the back-fill**: one block of a map holding
/// no statistics, re-read through `gather_block_statistics` under what its
/// request answers for it, holds what a gathering pass stores for it.
#[tokio::test]
async fn one_block_is_gathered_on_its_own() {
    let dump = statistics_fixture(16, "default");
    let wanted = request(StatisticsSelection::All, SMALL_GROUP);
    let bare = gathered(&dump, &StatisticsRequest::NONE).await;
    let reference = gathered(&dump, &wanted).await;
    let source = LocalFileSource::open(&dump).unwrap();
    let ordered = block(&bare, "public.ordered");
    let backfill = wanted.backfill(ordered).expect("a block with no statistics lacks them");
    let gathered = gather_block_statistics(
        &source,
        &ScanOptions::default(),
        bare.metadata.as_ref(),
        ordered,
        &backfill,
    )
    .await
    .unwrap()
    .expect("nothing cancelled it");
    assert_eq!(Some(&gathered), block(&reference, "public.ordered").statistics.as_deref());
    assert_eq!(wanted.backfill(block(&reference, "public.ordered")), None);
}

/// **A block records the request its size was chosen under, and a back-fill
/// re-reads it only for a stated one that differs** (`D34`'s rule, per bound):
/// a stated minimum re-reads every block sized under another, from its first
/// row; an unstated minimum and size keep what a block holds, a block lacking
/// a column being re-read at its size and keeping its record; and a stated
/// size compares sizes alone.
#[tokio::test]
async fn a_stated_minimum_rereads_a_block_sized_under_another() {
    let (_dir, dump) = sandboxed(&statistics_fixture(16, "default"), "sized.sql");
    let options = ScanOptions::default();
    let with_min =
        |min_rows| StatisticsRequest { min_rows: Some(min_rows), ..StatisticsRequest::ALL };
    let records = |run: &MapRun| {
        ["public.ordered", "public.specials", "public.long_value"]
            .map(|table| statistics(block(&run.index, table)).sizing)
    };
    let density = |min_rows| GroupSizing::Density { min_rows };
    let default = density(DEFAULT_STATISTICS_MIN_ROWS);
    let only_id = StatisticsRequest {
        selection: StatisticsSelection::Only(vec![StatisticsTarget::Column {
            table: "public.ordered".to_string(),
            column: "id".to_string(),
        }]),
        ..with_min(0)
    };

    let run = mapped_into_cache(&dump, &options, &only_id).await;
    assert_eq!(statistics(block(&run.index, "public.ordered")).sizing, density(0));
    let run = mapped_into_cache(&dump, &options, &StatisticsRequest::ALL).await;
    assert_eq!(run.backfilled, 3, "ordered lacks columns, the others everything");
    assert_eq!(records(&run), [density(0), default, default], "ordered kept its record");

    let run = mapped_into_cache(&dump, &options, &with_min(DEFAULT_STATISTICS_MIN_ROWS)).await;
    assert_eq!(run.backfilled, 1, "only ordered was sized under another minimum");
    assert_eq!(records(&run), [default; 3]);
    let long_value = |run: &MapRun| statistics(block(&run.index, "public.long_value")).clone();
    let coarse = long_value(&run);

    let run = mapped_into_cache(&dump, &options, &with_min(0)).await;
    assert_eq!(run.backfilled, 3);
    assert_eq!(records(&run), [density(0); 3]);
    assert!(long_value(&run).groups.len() > coarse.groups.len());
    for request in [StatisticsRequest::ALL, with_min(0)] {
        let run = mapped_into_cache(&dump, &options, &request).await;
        assert_eq!(run.backfilled, 0, "{request:?}");
    }

    let at_default_size = request(StatisticsSelection::All, DEFAULT_STATISTICS_GROUP_SIZE);
    let run = mapped_into_cache(&dump, &options, &at_default_size).await;
    assert_eq!(run.backfilled, 0, "every block is at the size stated, whatever sized it");
    let run = mapped_into_cache(&dump, &options, &request(StatisticsSelection::All, 4096)).await;
    assert_eq!(run.backfilled, 3);
    assert_eq!(records(&run), [GroupSizing::Stated; 3]);
    let run = mapped_into_cache(&dump, &options, &StatisticsRequest::ALL).await;
    assert_eq!(run.backfilled, 0, "an unstated minimum keeps a stated size");
    let run = mapped_into_cache(&dump, &options, &with_min(DEFAULT_STATISTICS_MIN_ROWS)).await;
    assert_eq!(run.backfilled, 3);
    assert_eq!(records(&run), [default; 3]);
    assert_eq!(long_value(&run), coarse, "re-read from the first row, as gathered cold");
}

/// **A back-fill the leader splits gathers what the serial pass gathers**, at
/// the chunk and counts [`a_gathering_scan_is_the_serial_scan_whatever_the_worker_count`]
/// splits the same block at.
#[tokio::test]
async fn a_backfill_the_leader_splits_is_the_serial_scan() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("long_block.sql");
    let mut text = String::from(
        "CREATE TABLE public.t (\n    a integer,\n    b text COLLATE pg_catalog.\"C\",\n    c text\n);\n\n",
    );
    text.push_str("COPY public.t (a, b, c) FROM stdin;\n");
    for i in 0..2000 {
        text.push_str(&format!("{i}\t{:05}\tv{}\n", 2000 - i, i % 3));
    }
    text.push_str("\\.\n\nSELECT 1;\n");
    std::fs::write(&dump, &text).unwrap();
    let wanted = request(StatisticsSelection::All, 256);
    let serial = ScanOptions { chunk_size: 64, ..ScanOptions::default() };
    let reference = gathered_with(&dump, &serial, &wanted).await;
    for jobs in [2, 4, 8] {
        let _ = std::fs::remove_file(cache::colocated_path(&dump));
        mapped_into_cache(&dump, &serial, &StatisticsRequest::NONE).await;
        let parallel = ScanOptions {
            parallelism: Parallelism::workers(jobs, DEFAULT_MEMORY_BUDGET),
            ..serial.clone()
        };
        let run = mapped_into_cache(&dump, &parallel, &wanted).await;
        assert_eq!(run.backfilled, 1, "{jobs} jobs");
        assert_eq!(run.index.spans, reference.spans, "{jobs} jobs");
    }
}

/// **A block past its cap gathers, split by the leader, what the serial pass
/// gathers, and that is what gathering exactly at the size it reaches
/// gathers** — every block of every fixture, re-read at a group size of a few
/// bytes under a cap of two groups, so most blocks merge, many of them
/// several times over, at chunks the leader cuts inside a group. What reaches the cap
/// under a request is [`StatisticsRequest::backfill`]'s answer, asserted
/// first.
#[tokio::test]
async fn every_fixture_block_past_its_cap_gathers_what_its_final_size_gathers() {
    const BASE: u64 = 8;
    const CAP: usize = 2;
    let unstated = StatisticsRequest::ALL;
    let stated = request(StatisticsSelection::All, BASE);
    let serial = ScanOptions { chunk_size: 64, ..ScanOptions::default() };
    let (mut blocks, mut merged) = (0, 0);
    for fixture in common::all_fixtures() {
        let source = LocalFileSource::open(&fixture).unwrap();
        let (options, mode) = (ScanOptions::default(), CacheMode::Disabled);
        let run = map_file(&source, &options, &mode, &StatisticsRequest::NONE);
        let index = run.await.unwrap().index;
        for block in index.blocks() {
            let backfill = unstated.backfill(block).expect("the block holds no statistics");
            assert_eq!(backfill.group_cap, Some(STATISTICS_GROUP_CAP));
            assert_eq!(stated.backfill(block).unwrap().group_cap, None);
            let capped = StatisticsBackfill {
                group_size: BASE,
                group_cap: Some(CAP),
                min_rows: None,
                ..backfill
            };
            let gather = |options: ScanOptions, backfill: StatisticsBackfill| {
                let (source, metadata) = (&source, index.metadata.as_ref());
                async move {
                    gather_block_statistics(source, &options, metadata, block, &backfill)
                        .await
                        .unwrap()
                        .expect("nothing cancels the re-read")
                }
            };
            let reference = gather(serial.clone(), capped.clone()).await;
            let label = format!("{}: {}", fixture.display(), block.header.table);
            assert!(reference.groups.len() <= CAP, "{label}");
            let exact = StatisticsBackfill {
                group_size: reference.group_size,
                group_cap: None,
                ..capped.clone()
            };
            assert_eq!(gather(serial.clone(), exact).await, reference, "{label}");
            for jobs in [3, 8] {
                let parallelism = Parallelism::workers(jobs, DEFAULT_MEMORY_BUDGET);
                let parallel = ScanOptions { parallelism, ..serial.clone() };
                let split = gather(parallel, capped.clone()).await;
                assert_eq!(split, reference, "{label}: {jobs} jobs");
            }
            blocks += 1;
            merged += usize::from(reference.group_size > BASE);
        }
    }
    assert!(merged * 2 > blocks, "only {merged} of {blocks} blocks merged");
}

/// The merges the density minimum must choose over a block gathered exactly
/// at its base size, written as `scripts/row_density.py` chooses: the first
/// size, from the base to a single group, whose upper middle group holds
/// `min_rows`, else the single group.
fn chosen_merges(rows: &[u64], min_rows: u64) -> u32 {
    let mut level = rows.to_vec();
    let mut merges = 0;
    loop {
        let mut sorted = level.clone();
        sorted.sort_unstable();
        if level.len() <= 1 || sorted[sorted.len() / 2] >= min_rows {
            return merges;
        }
        level = level.chunks(2).map(|pair| pair.iter().sum()).collect();
        merges += 1;
    }
}

/// **A block sized by its density minimum gathers, split by the leader, what
/// the serial pass gathers, at the size the rows its base size gathered
/// choose, and that is what gathering exactly at that size gathers** — every
/// block of every fixture, re-read at a group size of a few bytes under a
/// minimum of a few rows, so most blocks coarsen. What sizes a block under a
/// request is [`StatisticsRequest::backfill`]'s answer, asserted first.
#[tokio::test]
async fn every_fixture_block_sized_by_its_minimum_gathers_what_its_final_size_gathers() {
    const BASE: u64 = 8;
    const MIN_ROWS: u64 = 3;
    let unstated = StatisticsRequest::ALL;
    let stated = request(StatisticsSelection::All, BASE);
    let serial = ScanOptions { chunk_size: 64, ..ScanOptions::default() };
    let (mut blocks, mut coarsened) = (0, 0);
    for fixture in common::all_fixtures() {
        let source = LocalFileSource::open(&fixture).unwrap();
        let (options, mode) = (ScanOptions::default(), CacheMode::Disabled);
        let run = map_file(&source, &options, &mode, &StatisticsRequest::NONE);
        let index = run.await.unwrap().index;
        for block in index.blocks() {
            let backfill = unstated.backfill(block).expect("the block holds no statistics");
            let default_sizing = GroupSizing::Density { min_rows: DEFAULT_STATISTICS_MIN_ROWS };
            assert_eq!(backfill.min_rows, Some(DEFAULT_STATISTICS_MIN_ROWS));
            assert_eq!(backfill.sizing, default_sizing);
            let exact_request = stated.backfill(block).unwrap();
            assert_eq!((exact_request.min_rows, exact_request.sizing), (None, GroupSizing::Stated));
            let sized = StatisticsBackfill {
                group_size: BASE,
                group_cap: None,
                min_rows: Some(MIN_ROWS),
                ..backfill
            };
            let gather = |options: ScanOptions, backfill: StatisticsBackfill| {
                let (source, metadata) = (&source, index.metadata.as_ref());
                async move {
                    gather_block_statistics(source, &options, metadata, block, &backfill)
                        .await
                        .unwrap()
                        .expect("nothing cancels the re-read")
                }
            };
            let label = format!("{}: {}", fixture.display(), block.header.table);
            let reference = gather(serial.clone(), sized.clone()).await;
            assert_eq!(reference.sizing, default_sizing, "{label}: the record is the plan's");
            let exact =
                |group_size| StatisticsBackfill { group_size, min_rows: None, ..sized.clone() };
            let base = gather(serial.clone(), exact(BASE)).await;
            let rows: Vec<u64> = base.groups.iter().map(|g| g.rows).collect();
            let merges = chosen_merges(&rows, MIN_ROWS);
            assert_eq!(reference.group_size, BASE << merges, "{label}: {rows:?}");
            assert_eq!(
                gather(serial.clone(), exact(reference.group_size)).await,
                reference,
                "{label}"
            );
            for jobs in [3, 8] {
                let parallelism = Parallelism::workers(jobs, DEFAULT_MEMORY_BUDGET);
                let parallel = ScanOptions { parallelism, ..serial.clone() };
                let split = gather(parallel, sized.clone()).await;
                assert_eq!(split, reference, "{label}: {jobs} jobs");
            }
            blocks += 1;
            coarsened += usize::from(merges > 0);
        }
    }
    assert!(coarsened * 2 > blocks, "only {coarsened} of {blocks} blocks coarsened");
}

/// **A block that no longer ends where the map says is refused**, rather than
/// given statistics of other bytes: the file is rewritten at its own size with
/// the block's terminator moved, which the cache's identity check cannot see.
/// The refusal names the cache the map came from — here not beside the dump,
/// so the colocated default would name the wrong file — and the per-block
/// entry point, handed a map with no cache, names none.
#[tokio::test]
async fn a_block_rewritten_at_the_same_size_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("rewritten.sql");
    let cache_path = dir.path().join("elsewhere").join("rewritten.dqcache");
    std::fs::create_dir(cache_path.parent().unwrap()).unwrap();
    let mode = CacheMode::Enabled(cache_path.clone());
    let before = "COPY public.t (a) FROM stdin;\n11\n2\n\\.\nSELECT 1;\n";
    let after = "COPY public.t (a) FROM stdin;\n1\n\\.\n22\nSELECT 1;\n";
    assert_eq!(before.len(), after.len());
    std::fs::write(&dump, before).unwrap();
    let source = LocalFileSource::open(&dump).unwrap();
    let mapped = map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::NONE)
        .await
        .unwrap()
        .index;
    let mapped_block = mapped.blocks().next().unwrap();
    let header_offset = mapped_block.header_offset;
    std::fs::write(&dump, after).unwrap();
    let source = LocalFileSource::open(&dump).unwrap();
    let err = map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::ALL)
        .await
        .expect_err("the block moved");
    assert!(
        matches!(
            &err,
            pgdump_query::Error::CachedBlockChanged { path: Some(at_path), header_offset: at }
                if *at_path == cache_path && *at == header_offset
        ),
        "{err}"
    );
    assert!(err.to_string().contains(&cache_path.display().to_string()), "{err}");

    let backfill = StatisticsRequest::ALL.backfill(mapped_block).expect("it holds no statistics");
    let err = gather_block_statistics(
        &source,
        &ScanOptions::default(),
        mapped.metadata.as_ref(),
        mapped_block,
        &backfill,
    )
    .await
    .expect_err("the block moved");
    assert!(
        matches!(
            &err,
            pgdump_query::Error::CachedBlockChanged { path: None, header_offset: at }
                if *at == header_offset
        ),
        "{err}"
    );
}
