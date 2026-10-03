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
use std::future::Future;
use std::num::NonZeroU64;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bytes::Bytes;
use pgdump_query::cache::{self, CacheMode, CacheStatus};
use pgdump_query::{
    BLOCK_MAX_ROW_GROUPS, BlockStatistics, ByteRangeSource, ColumnRefusals, CopyBlock,
    DEFAULT_MEMORY_BUDGET, DICTIONARY_ENTRY_MAX_BYTES, DICTIONARY_MAX_ENTRIES, DumpIndex,
    FieldRefusal, GroupSizing, IgnoredRefusals, LocalFileSource, MapRun, Parallelism,
    PostgresInvalidValues, ROW_GROUP_DEFAULT_MIN_ROWS, ROW_GROUP_DEFAULT_SIZE_BYTES, ScanOptions,
    Sortedness, StatisticsBackfill, StatisticsLevel, StatisticsRequest, StatisticsSelection,
    StatisticsTarget, bounded_columns, gather_block_statistics, map_file,
};

mod common;
use common::{VERSIONS, sandboxed, statistics_fixture, types_fixture};

/// A group size several `ordered` rows long and well under its block, so
/// `ordered` has several groups, some holding more distinct `high_card` texts
/// than [`DICTIONARY_MAX_ENTRIES`].
const SMALL_GROUP: u64 = 4096;

/// The `default` flag set's blocks: one per table
/// `scripts/fixture_schema_statistics.sql` fills.
const BLOCKS: usize = 9;

/// Every table at the metadata level but `targets`, at the data level.
fn only(targets: Vec<StatisticsTarget>) -> StatisticsSelection {
    StatisticsSelection {
        default: StatisticsLevel::Metadata,
        overrides: targets.into_iter().map(|target| (target, StatisticsLevel::Data)).collect(),
    }
}

fn request(selection: StatisticsSelection, group_size: u64) -> StatisticsRequest {
    let group_size = Some(NonZeroU64::new(group_size).unwrap());
    StatisticsRequest { selection, group_size, min_rows: None, max_rows: None }
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
    let run = map_file(&source, options, &CacheMode::DISABLED, statistics).await.unwrap();
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

/// Whether `declared` is blank-padded, its trailing blanks insignificant: a
/// `character(n)`, or a bare `bpchar`.
fn blank_padded(declared: &str) -> bool {
    declared == "bpchar" || (declared.starts_with("character") && !declared.contains("varying"))
}

/// A field as the statistics of a `declared` column hold it: a blank-padded
/// one without its trailing blanks, which PostgreSQL does not compare.
fn held<'a>(declared: &str, value: &'a str) -> &'a str {
    if blank_padded(declared) { value.trim_end_matches(' ') } else { value }
}

/// PostgreSQL's order for the fixture's value types, written independently of
/// the library: integers and numerics by value with `NaN` above every number,
/// floats the same with `-0` equal to `0`, `false` below `true`, and text —
/// blank-padded or varying — by its bytes. A `timestamptz` is written in the
/// dump's one zone at one width, so its text is in its order too. The enum
/// `public.mood` by its labels' declared positions.
fn order(declared: &str, a: &str, b: &str) -> Ordering {
    let mood = |v: &str| ["sad", "ok", "happy"].iter().position(|label| *label == v).unwrap();
    match declared {
        "text" | "timestamp with time zone" => a.as_bytes().cmp(b.as_bytes()),
        "public.mood" => mood(a).cmp(&mood(b)),
        "boolean" => (a == "t").cmp(&(b == "t")),
        _ if blank_padded(declared) || declared.starts_with("character varying") => {
            a.as_bytes().cmp(b.as_bytes())
        }
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

/// A number as the fixture writes one, `NaN` and the infinities included.
fn number(v: &str) -> f64 {
    match v {
        "NaN" => f64::NAN,
        "Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        _ => v.parse::<f64>().unwrap(),
    }
}

/// The scale a `declared` column's values are summed at, where the typed read
/// emits it as an integer or a `Decimal128` — `int2`, `int4`, `int8`, `oid`,
/// or a `numeric(p,s)` of at most 38 digits — and `None` for every other.
fn summed_scale(declared: &str) -> Option<u32> {
    match declared {
        "smallint" | "integer" | "bigint" | "oid" => Some(0),
        _ => {
            let typmod = declared.strip_prefix("numeric(")?.strip_suffix(')')?;
            let (precision, scale) = typmod.split_once(',').unwrap_or((typmod, "0"));
            let precision: u32 = precision.trim().parse().unwrap();
            (precision <= 38).then(|| scale.trim().parse().unwrap())
        }
    }
}

/// A fixture value of a column summed at `scale` as the integer it sums as:
/// a `numeric(p,s)` is written with exactly `s` fractional digits.
fn unscaled(value: &str, scale: u32) -> i128 {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    assert_eq!(fraction.len() as u32, scale, "{value} at scale {scale}");
    format!("{whole}{fraction}").parse().unwrap()
}

/// Every statistic of `block` against the file: the groups' extents and row
/// counts, and per column its NULL counts, its sum wrapped at 128 bits and its
/// values' text bytes, that each bound is on the right side of every value in
/// its group — an exact one being a value there, a float's in IEEE
/// `totalOrder` as well, which tells its zeros apart — and that each
/// dictionary is exactly the group's distinct texts or absent past a cap.
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
            // A blank-padded column's own set, and its dictionary, hold a value
            // without its padding; its DataFusion set holds the text it emits.
            let unpadded: Vec<&str> = values.iter().map(|v| held(declared, v)).collect();
            let what = format!("{label}: group {k}, column {c} ({declared})");
            assert_eq!(column.null_counts[k], (members.len() - values.len()) as u64, "{what}");
            match (&column.sums, summed_scale(declared)) {
                (Some(sums), Some(scale)) => {
                    let sum =
                        values.iter().fold(0i128, |sum, v| sum.wrapping_add(unscaled(v, scale)));
                    assert_eq!(sums[k], sum, "{what}: sum");
                }
                (None, None) => {}
                (sums, scale) => panic!("{what}: sums {sums:?} for a column summed at {scale:?}"),
            }
            let length = values.iter().map(|v| v.len() as u64).sum::<u64>();
            assert_eq!(column.value_bytes[k], length, "{what}: text bytes");
            // The fixture's columns keeping a second set are a bare `numeric`
            // and an enum, whose DataFusion order is their text's.
            let sets = [
                (declared, &column.bounds, &unpadded),
                ("text", &column.datafusion_bounds, &values),
            ];
            for (declared, bounds, values) in sets {
                let Some(bounds) = bounds else { continue };
                match &bounds.groups[k] {
                    None => assert!(
                        values.is_empty()
                            || values.iter().any(|v| v.len() > DICTIONARY_ENTRY_MAX_BYTES),
                        "{what}: values and no bounds"
                    ),
                    Some(b) => {
                        assert!(
                            b.min.len() <= DICTIONARY_ENTRY_MAX_BYTES
                                && b.max.len() <= DICTIONARY_ENTRY_MAX_BYTES
                        );
                        for v in values.iter() {
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
                                || values.iter().any(|v| v.len() > DICTIONARY_ENTRY_MAX_BYTES),
                            "{what}: min"
                        );
                        assert_eq!(
                            b.min_exact,
                            values.contains(&b.min.as_str()),
                            "{what}: min_exact"
                        );
                        if matches!(declared, "real" | "double precision") {
                            let (min, max) = (number(&b.min), number(&b.max));
                            for v in values.iter().map(|v| number(v)) {
                                assert!(min.total_cmp(&v).is_le(), "{what}: min {v} in totalOrder");
                                assert!(max.total_cmp(&v).is_ge(), "{what}: max {v} in totalOrder");
                            }
                        }
                        assert_eq!(
                            b.max_exact,
                            values.contains(&b.max.as_str()),
                            "{what}: max_exact"
                        );
                    }
                }
            }
            if let Some(dictionary) = &column.dictionary {
                let distinct: BTreeSet<&str> = unpadded.iter().copied().collect();
                let fits = distinct.len() <= DICTIONARY_MAX_ENTRIES
                    && distinct.iter().all(|v| v.len() <= DICTIONARY_ENTRY_MAX_BYTES);
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
        let index = gathered(&dump, &request(StatisticsSelection::DATA, SMALL_GROUP)).await;
        assert_eq!(index.blocks().count(), BLOCKS);
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
/// schema states each one — text under the database's default collation
/// included, ordered bytewise, which is not the order the server sees it in
/// (`docs/design/decisions.md`, "D79") — and a dictionary for that text too.
#[tokio::test]
async fn sortedness_is_the_blocks_row_order() {
    use Sortedness::{Ascending, Descending, Unsorted};
    for version in VERSIONS {
        let dump = statistics_fixture(version, "default");
        let index = gathered(&dump, &request(StatisticsSelection::DATA, SMALL_GROUP)).await;
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
            ("default_text", Unsorted),
        ] {
            assert_eq!(sortedness(ordered, column), Some(expected), "{column} on {version}");
        }
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

        // A float's row order tells its zeros apart as Arrow sorts them: `0`
        // then `-0` ascends in PostgreSQL's order and not in `totalOrder`.
        let zeros = block(&index, "public.zeros");
        for (column, expected) in [("min_pos_first", Unsorted), ("min_neg_first", Ascending)] {
            assert_eq!(sortedness(zeros, column), Some(expected), "{column} on {version}");
        }

        // An enum descends in its declared order and ascends in DataFusion's, its
        // labels' text.
        let moods = block(&index, "public.moods");
        let m = statistics(moods).columns[1].as_ref().unwrap();
        assert_eq!(m.bounds.as_ref().map(|b| b.sortedness), Some(Descending), "m on {version}");
        assert_eq!(
            m.datafusion_bounds.as_ref().map(|b| b.sortedness),
            Some(Ascending),
            "{version}"
        );
    }
}

/// A value past the stored-value cap is bounded by truncated texts: a prefix
/// below it and a successor above it, each marked inexact, neither past the cap —
/// and a group size under the megabyte row leaves groups no row starts in.
#[tokio::test]
async fn a_long_value_is_bounded_by_truncated_texts() {
    for version in VERSIONS {
        let dump = statistics_fixture(version, "default");
        let index = gathered(&dump, &request(StatisticsSelection::DATA, 64)).await;
        let long = block(&index, "public.long_value");
        assert_describes_the_file(&dump, long, &format!("long_value on {version}"));
        let stats = statistics(long);
        assert!(stats.groups.iter().any(|g| g.rows == 0), "empty groups on {version}");
        let v = stats.columns[1].as_ref().unwrap();
        let groups = || v.bounds.as_ref().unwrap().groups.iter().flatten();
        let truncated: Vec<_> = groups().filter(|b| !b.max_exact).collect();
        assert_eq!(truncated.len(), 2, "the 300-byte and the megabyte value on {version}");
        let prefixed: Vec<_> = groups().filter(|b| !b.min_exact).collect();
        assert_eq!(
            prefixed.len(),
            1,
            "the megabyte value, the 300-byte one sharing a group with `a-short`, on {version}"
        );
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
    assert_eq!(StatisticsRequest::default(), StatisticsRequest::DATA);
    let index = gathered(&dump, &StatisticsRequest::default()).await;
    let default_sizing =
        GroupSizing::Density { min_rows: ROW_GROUP_DEFAULT_MIN_ROWS, max_rows: None };
    for block in index.blocks() {
        let statistics = statistics(block);
        assert_eq!(statistics.columns.len(), block.header.columns.len());
        assert!(statistics.columns.iter().all(Option::is_some), "{}", block.header.table);
        assert_eq!(statistics.sizing, default_sizing, "{}", block.header.table);
        assert_eq!(statistics.groups.len(), 1, "{}", block.header.table);
    }
    let ordered = statistics(block(&index, "public.ordered"));
    assert_eq!(ordered.group_size, pgdump_query::ROW_GROUP_DEFAULT_SIZE_BYTES);
    let long_value = statistics(block(&index, "public.long_value"));
    assert!(long_value.group_size > pgdump_query::ROW_GROUP_DEFAULT_SIZE_BYTES);

    let uncoarsened = StatisticsRequest { min_rows: Some(0), ..StatisticsRequest::DATA };
    let index = gathered(&dump, &uncoarsened).await;
    let long_value = statistics(block(&index, "public.long_value"));
    assert_eq!(long_value.group_size, pgdump_query::ROW_GROUP_DEFAULT_SIZE_BYTES);
    assert!(long_value.groups.len() > 1);
    assert_eq!(long_value.sizing, GroupSizing::Density { min_rows: 0, max_rows: None });
}

/// A selection tracks only what it names: a table, or one column, whose
/// other columns carry nothing, and every block it does not name gathers
/// nothing at all.
#[tokio::test]
async fn a_selection_gathers_only_what_it_names() {
    let dump = statistics_fixture(16, "default");
    let selection = only(vec![
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
    let index = gathered(&statistics_fixture(16, "default"), &StatisticsRequest::METADATA).await;
    assert!(index.blocks().all(|b| b.statistics.is_none()));
}

/// Statistics persist in the one cache file and load back exactly, and a
/// clone of the map shares each block's statistics rather than copying them.
#[tokio::test]
async fn statistics_round_trip_through_the_cache() {
    let (_dir, dump) = sandboxed(&statistics_fixture(16, "default"), "statistics.sql");
    let source = LocalFileSource::open(&dump).unwrap();
    let mode = CacheMode::enabled(cache::colocated_path(&dump));
    let wanted = request(StatisticsSelection::DATA, SMALL_GROUP);
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
/// `pgdt/tests/determinism.rs`, byte for byte.
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
    let wanted = request(StatisticsSelection::DATA, 256);
    let serial = ScanOptions { chunk_size_bytes: 64, ..ScanOptions::default() };
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
    let index = gathered(&dump, &request(StatisticsSelection::DATA, 16)).await;
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

/// **A column's stored groups are read only where they are one to a group**:
/// a NULL count listing a group the block does not hold describes other
/// groups, so an unbounded group it calls all NULL leaves the extremes
/// incomplete rather than `Exact` over the groups that kept a bound.
#[tokio::test]
async fn a_summary_reads_no_column_whose_groups_disagree_with_the_block() {
    use pgdump_query::map::{DataBlock, SpanBody};
    use pgdump_query::{
        ComparisonSemantics, QueryOptions, StatisticsView, TableName, table_schema, table_summary,
    };

    let mut index = gathered(
        &statistics_fixture(16, "default"),
        &request(StatisticsSelection::DATA, SMALL_GROUP),
    )
    .await;
    let name = TableName::of(block(&index, "public.ordered"));
    let summary = |index: &DumpIndex| {
        let resolved = table_schema(index, &name, &QueryOptions::default()).unwrap();
        let id = resolved.schema.index_of("id").unwrap();
        let (semantics, reading) = (ComparisonSemantics::DataFusion, StatisticsView::Every);
        table_summary(index, &name, &resolved, semantics, reading).columns[id].clone()
    };
    assert!(summary(&index).bounds_complete, "every group of `id` is bounded");

    for span in &mut index.spans {
        if let SpanBody::Data(DataBlock::Copy(block)) = &mut span.body
            && block.header.table == "ordered"
        {
            let mut held = BlockStatistics::clone(block.statistics.as_deref().unwrap());
            assert!(held.groups.len() > 1);
            let first = held.groups[0].rows;
            let c = block.header.columns.iter().position(|c| c == "id").unwrap();
            let column = held.columns[c].as_mut().unwrap();
            for set in [&mut column.bounds, &mut column.datafusion_bounds].into_iter().flatten() {
                set.groups[0] = None;
            }
            column.null_counts[0] = first;
            column.null_counts.push(0);
            block.statistics = Some(Arc::new(held));
        }
    }
    assert!(!summary(&index).bounds_complete);
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
    let index = gathered(&dump, &request(StatisticsSelection::DATA, 1 << 20)).await;
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
    let mode = CacheMode::enabled(cache::colocated_path(dump));
    let run = map_file(&source, options, &mode, statistics).await.unwrap();
    assert!(!run.interrupted);
    run
}

/// A local file that records the offset of every read it answers — so a test
/// can count how often one block was read, a serial re-read of a block
/// starting at its `data_offset` and nothing else starting there once the
/// map is complete.
struct RecordingSource {
    inner: LocalFileSource,
    offsets: Mutex<Vec<u64>>,
}

impl RecordingSource {
    fn open(dump: &Path) -> Self {
        Self { inner: LocalFileSource::open(dump).unwrap(), offsets: Mutex::default() }
    }

    /// How many reads started at `block`'s first row.
    fn reads_of(&self, block: &CopyBlock) -> usize {
        self.offsets.lock().unwrap().iter().filter(|&&at| at == block.data_offset).count()
    }
}

impl ByteRangeSource for RecordingSource {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = pgdump_query::Result<Bytes>> + Send + '_>> {
        self.offsets.lock().unwrap().push(offset);
        self.inner.read_range(offset, len)
    }

    fn size(&self) -> Pin<Box<dyn Future<Output = pgdump_query::Result<u64>> + Send + '_>> {
        self.inner.size()
    }

    fn modified(
        &self,
    ) -> Pin<Box<dyn Future<Output = pgdump_query::Result<Option<SystemTime>>> + Send + '_>> {
        self.inner.modified()
    }
}

/// [`mapped_into_cache`] through a [`RecordingSource`], serially, and the
/// source, to count what the run read.
async fn mapped_recording(
    dump: &Path,
    statistics: &StatisticsRequest,
) -> (MapRun, RecordingSource) {
    let source = RecordingSource::open(dump);
    let mode = CacheMode::enabled(cache::colocated_path(dump));
    let run = map_file(&source, &ScanOptions::default(), &mode, statistics).await.unwrap();
    assert!(!run.interrupted);
    (run, source)
}

/// **A block mapped without the statistics asked for is re-read for them, into
/// exactly what one gathering pass stores**: from no statistics, from the
/// default size re-asked at a stated one, from a table selection and from a
/// single column, on every major — and once it holds them, asking again re-reads
/// nothing.
#[tokio::test]
async fn a_block_lacking_the_requested_statistics_is_reread_into_what_one_pass_gathers() {
    let wanted = request(StatisticsSelection::DATA, SMALL_GROUP);
    let ordered = || "public.ordered".to_string();
    let earlier = [
        (StatisticsRequest::METADATA, BLOCKS),
        (StatisticsRequest::DATA, BLOCKS),
        (request(only(vec![StatisticsTarget::Table(ordered())]), SMALL_GROUP), BLOCKS - 1),
        (
            request(
                only(vec![StatisticsTarget::Column { table: ordered(), column: "id".to_string() }]),
                SMALL_GROUP,
            ),
            BLOCKS,
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
        only(vec![StatisticsTarget::Column {
            table: "public.ordered".to_string(),
            column: "id".to_string(),
        }]),
        SMALL_GROUP,
    );
    let (_dir, dump) = sandboxed(&dump_of(), "s.sql");
    mapped_into_cache(&dump, &ScanOptions::default(), &one_column).await;
    let run = mapped_into_cache(&dump, &ScanOptions::default(), &StatisticsRequest::DATA).await;
    assert_eq!((run.lacking_statistics, run.backfilled), (BLOCKS, BLOCKS));
    let small = gathered(&dump_of(), &request(StatisticsSelection::DATA, SMALL_GROUP)).await;
    let default = gathered(&dump_of(), &StatisticsRequest::DATA).await;
    for table in ["public.long_value", "public.ordered", "public.specials"] {
        let expected = if table == "public.ordered" { &small } else { &default };
        assert_eq!(
            block(&run.index, table).statistics,
            block(expected, table).statistics,
            "{table}"
        );
    }

    let (_dir, dump) = sandboxed(&dump_of(), "s.sql");
    mapped_into_cache(&dump, &ScanOptions::default(), &request(StatisticsSelection::DATA, 64))
        .await;
    let run = mapped_into_cache(&dump, &ScanOptions::default(), &StatisticsRequest::DATA).await;
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
    mapped_into_cache(&dump, &options, &request(StatisticsSelection::DATA, SMALL_GROUP)).await;
    let one_column = request(
        only(vec![StatisticsTarget::Column {
            table: "public.ordered".to_string(),
            column: "id".to_string(),
        }]),
        64,
    );
    let run = mapped_into_cache(&dump, &options, &one_column).await;
    assert_eq!((run.lacking_statistics, run.backfilled), (1, 1));
    let whole_at_64 = gathered(&dump_of(), &request(StatisticsSelection::DATA, 64)).await;
    let small = gathered(&dump_of(), &request(StatisticsSelection::DATA, SMALL_GROUP)).await;
    for table in ["public.long_value", "public.ordered", "public.specials"] {
        let expected = if table == "public.ordered" { &whole_at_64 } else { &small };
        assert_eq!(
            block(&run.index, table).statistics,
            block(expected, table).statistics,
            "{table}"
        );
    }
}

/// **A text column held without bounds is re-read for them**: a cache whose
/// `default_text` — on the database's collation — was gathered with a
/// dictionary and no bounds, as a build bounding only exact orders stored it,
/// is re-read by a `parse` asking for nothing more, into exactly what one
/// gathering pass now stores, and nothing is re-read after that. No other
/// block lacks anything, and a column whose declared type is bounded nowhere
/// never reads as lacking.
#[tokio::test]
async fn a_text_column_held_without_bounds_is_reread_for_them() {
    use pgdump_query::map::{DataBlock, SpanBody};
    let (_dir, dump) = sandboxed(&statistics_fixture(16, "default"), "unbounded.sql");
    let options = ScanOptions::default();
    let fresh = mapped_into_cache(&dump, &options, &StatisticsRequest::DATA).await.index;
    let ordered = block(&fresh, "public.ordered");
    let bounded = bounded_columns(ordered, fresh.metadata.as_ref());
    assert!(bounded.iter().all(|&b| b), "every column of `ordered` is bounded: {bounded:?}");
    assert_eq!(bounded_columns(ordered, None), bounded, "no DDL, bounded as its text");

    let mut older = fresh.clone();
    for span in &mut older.spans {
        if let SpanBody::Data(DataBlock::Copy(block)) = &mut span.body
            && block.header.table == "ordered"
        {
            let mut held = BlockStatistics::clone(block.statistics.as_deref().unwrap());
            held.columns[11].as_mut().unwrap().bounds = None;
            block.statistics = Some(Arc::new(held));
        }
    }
    let unbounded = block(&older, "public.ordered");
    let backfill = StatisticsRequest::DATA
        .backfill(unbounded, &bounded, None)
        .expect("a bounded column held without bounds lacks them");
    let held = statistics(unbounded);
    assert_eq!((backfill.group_size, backfill.sizing), (held.group_size, held.sizing));
    assert_eq!(
        StatisticsRequest::DATA.backfill(unbounded, &vec![false; bounded.len()], None),
        None,
        "a column bounded nowhere lacks nothing"
    );
    let source = LocalFileSource::open(&dump).unwrap();
    cache::save(&cache::colocated_path(&dump), &source, &older).await.unwrap();

    let run = mapped_into_cache(&dump, &options, &StatisticsRequest::DATA).await;
    assert_eq!((run.lacking_statistics, run.backfilled), (1, 1));
    assert_eq!(run.index.spans, fresh.spans);
    let again = mapped_into_cache(&dump, &options, &StatisticsRequest::DATA).await;
    assert_eq!((again.lacking_statistics, again.backfilled), (0, 0));
}

/// **A column no DDL declared, held without bounds, is re-read for them**: a
/// `--data-only` dump's cache as a build bounding only declared columns
/// stored it — every column's bounds absent — is re-read by a `parse` asking
/// for nothing more, into exactly what one gathering pass now stores, which
/// bounds every column of every block as its text
/// (`docs/design/decisions.md`, "D79").
#[tokio::test]
async fn an_undeclared_column_held_without_bounds_is_reread_for_them() {
    use pgdump_query::map::{DataBlock, SpanBody};
    let (_dir, dump) = sandboxed(&types_fixture(16, "data-only"), "undeclared.sql");
    let options = ScanOptions::default();
    let fresh = mapped_into_cache(&dump, &options, &StatisticsRequest::DATA).await.index;
    let mut older = fresh.clone();
    let mut blocks = 0;
    for span in &mut older.spans {
        if let SpanBody::Data(DataBlock::Copy(block)) = &mut span.body {
            let Some(held) = block.statistics.as_deref() else { continue };
            let mut held = BlockStatistics::clone(held);
            for column in held.columns.iter_mut().flatten() {
                assert!(column.bounds.is_some(), "{}", block.header.table);
                assert!(column.declared_type.is_none() && column.datafusion_bounds.is_none());
                column.bounds = None;
            }
            block.statistics = Some(Arc::new(held));
            blocks += 1;
        }
    }
    assert!(blocks > 0);
    let source = LocalFileSource::open(&dump).unwrap();
    cache::save(&cache::colocated_path(&dump), &source, &older).await.unwrap();

    let run = mapped_into_cache(&dump, &options, &StatisticsRequest::DATA).await;
    assert_eq!((run.lacking_statistics, run.backfilled), (blocks, blocks));
    assert_eq!(run.index.spans, fresh.spans);
}

/// A request naming one table re-reads that table's block alone; a request
/// gathering nothing re-reads nothing and keeps what the blocks hold.
#[tokio::test]
async fn a_backfill_rereads_only_the_blocks_its_request_tracks() {
    let (_dir, dump) = sandboxed(&statistics_fixture(16, "default"), "s.sql");
    let options = ScanOptions::default();
    mapped_into_cache(&dump, &options, &StatisticsRequest::METADATA).await;
    let specials = StatisticsRequest {
        selection: only(vec![StatisticsTarget::Table("specials".to_string())]),
        ..StatisticsRequest::DATA
    };
    let run = mapped_into_cache(&dump, &options, &specials).await;
    assert_eq!((run.lacking_statistics, run.backfilled), (1, 1));
    assert!(block(&run.index, "public.specials").statistics.is_some());
    assert!(block(&run.index, "public.ordered").statistics.is_none());

    let none = mapped_into_cache(&dump, &options, &StatisticsRequest::METADATA).await;
    assert_eq!((none.lacking_statistics, none.backfilled), (0, 0));
    assert_eq!(none.index.spans, run.index.spans);
}

/// **The per-block entry point is the back-fill**: one block of a map holding
/// no statistics, re-read through `gather_block_statistics` under what its
/// request answers for it, holds what a gathering pass stores for it.
#[tokio::test]
async fn one_block_is_gathered_on_its_own() {
    let dump = statistics_fixture(16, "default");
    let wanted = request(StatisticsSelection::DATA, SMALL_GROUP);
    let bare = gathered(&dump, &StatisticsRequest::METADATA).await;
    let reference = gathered(&dump, &wanted).await;
    let source = LocalFileSource::open(&dump).unwrap();
    let ordered = block(&bare, "public.ordered");
    let backfill = wanted
        .backfill(ordered, &bounded_columns(ordered, bare.metadata.as_ref()), None)
        .expect("a block with no statistics lacks them");
    let gathered = gather_block_statistics(
        &source,
        &ScanOptions::default(),
        bare.metadata.as_ref(),
        ordered,
        &backfill,
    )
    .await
    .unwrap()
    .expect("nothing cancelled it")
    .gathered
    .gathered()
    .expect("an unbounded allowance declines nothing");
    assert_eq!(Some(&gathered), block(&reference, "public.ordered").statistics.as_deref());
    assert_eq!(
        wanted.backfill(
            block(&reference, "public.ordered"),
            &bounded_columns(block(&reference, "public.ordered"), reference.metadata.as_ref()),
            None,
        ),
        None
    );
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
        |min_rows| StatisticsRequest { min_rows: Some(min_rows), ..StatisticsRequest::DATA };
    let records = |run: &MapRun| {
        ["public.ordered", "public.specials", "public.long_value"]
            .map(|table| statistics(block(&run.index, table)).sizing)
    };
    let density = |min_rows| GroupSizing::Density { min_rows, max_rows: None };
    let default = density(ROW_GROUP_DEFAULT_MIN_ROWS);
    let only_id = StatisticsRequest {
        selection: only(vec![StatisticsTarget::Column {
            table: "public.ordered".to_string(),
            column: "id".to_string(),
        }]),
        ..with_min(0)
    };

    let run = mapped_into_cache(&dump, &options, &only_id).await;
    assert_eq!(statistics(block(&run.index, "public.ordered")).sizing, density(0));
    let run = mapped_into_cache(&dump, &options, &StatisticsRequest::DATA).await;
    assert_eq!(run.backfilled, BLOCKS, "ordered lacks columns, the others everything");
    assert_eq!(records(&run), [density(0), default, default], "ordered kept its record");

    let run = mapped_into_cache(&dump, &options, &with_min(ROW_GROUP_DEFAULT_MIN_ROWS)).await;
    assert_eq!(run.backfilled, 1, "only ordered was sized under another minimum");
    assert_eq!(records(&run), [default; 3]);
    let long_value = |run: &MapRun| statistics(block(&run.index, "public.long_value")).clone();
    let coarse = long_value(&run);

    let run = mapped_into_cache(&dump, &options, &with_min(0)).await;
    assert_eq!(run.backfilled, BLOCKS);
    assert_eq!(records(&run), [density(0); 3]);
    assert!(long_value(&run).groups.len() > coarse.groups.len());
    for request in [StatisticsRequest::DATA, with_min(0)] {
        let run = mapped_into_cache(&dump, &options, &request).await;
        assert_eq!(run.backfilled, 0, "{request:?}");
    }

    let at_default_size = request(StatisticsSelection::DATA, ROW_GROUP_DEFAULT_SIZE_BYTES);
    let run = mapped_into_cache(&dump, &options, &at_default_size).await;
    assert_eq!(run.backfilled, 0, "every block is at the size stated, whatever sized it");
    let run = mapped_into_cache(&dump, &options, &request(StatisticsSelection::DATA, 4096)).await;
    assert_eq!(run.backfilled, BLOCKS);
    assert_eq!(records(&run), [GroupSizing::Stated; 3]);
    let run = mapped_into_cache(&dump, &options, &StatisticsRequest::DATA).await;
    assert_eq!(run.backfilled, 0, "an unstated minimum keeps a stated size");
    let run = mapped_into_cache(&dump, &options, &with_min(ROW_GROUP_DEFAULT_MIN_ROWS)).await;
    assert_eq!(run.backfilled, BLOCKS);
    assert_eq!(records(&run), [default; 3]);
    assert_eq!(long_value(&run), coarse, "re-read from the first row, as gathered cold");
}

/// The rows the 90th-percentile group of `statistics` holds — the group a
/// stated maximum is read at, the `⌈9G/10⌉`-th smallest — written here rather
/// than read from the library.
fn densest_group(statistics: &BlockStatistics) -> u64 {
    let mut rows: Vec<u64> = statistics.groups.iter().map(|group| group.rows).collect();
    rows.sort_unstable();
    rows[(9 * rows.len()).div_ceil(10) - 1]
}

/// A dump of two `COPY` blocks, each over a mebibyte so that the default group
/// size cuts them: `dense`, whose rows are spread evenly over two megabytes,
/// and `clustered`, whose rows all start in the first few kilobytes and are
/// followed by one row a mebibyte long.
fn dense_and_clustered(dir: &Path) -> std::path::PathBuf {
    let dump = dir.join("dense.sql");
    let mut text = String::from("CREATE TABLE public.dense (\n    id integer,\n    v text\n);\n\n");
    text.push_str("COPY public.dense (id, v) FROM stdin;\n");
    for i in 0..20_000 {
        text.push_str(&format!("{i}\t{}\n", "x".repeat(90)));
    }
    text.push_str("\\.\n\nCREATE TABLE public.clustered (\n    id integer,\n    v text\n);\n\n");
    text.push_str("COPY public.clustered (id, v) FROM stdin;\n");
    for i in 0..5_000 {
        text.push_str(&format!("{i}\tshort\n"));
    }
    text.push_str(&format!("5000\t{}\n", "y".repeat(1 << 20)));
    text.push_str("\\.\n\nSELECT 1;\n");
    std::fs::write(&dump, &text).unwrap();
    dump
}

/// **A block whose groups break a stated maximum is re-read at the size its
/// own rows per group predict, and never again**: no merge makes a block
/// finer, so the maximum is reachable only by reading the block again, and
/// what that read gives is what the block keeps. `dense` meets the maximum
/// there; `clustered`, whose rows all start in one stretch, does not, and is
/// left as it is rather than read once more. **The reads are counted**: a
/// block an earlier, flagless run gathered is read twice in the run stating
/// the maximum — at the default size, its sizing being another request's,
/// then at the size those groups predict — and a block the run maps itself
/// once. Asking again reads neither, and a split scan gathers what the serial
/// one does.
#[tokio::test]
async fn a_block_breaking_a_stated_maximum_is_reread_at_the_size_it_predicts() {
    const MAX_ROWS: u64 = 3_000;
    let dir = tempfile::tempdir().unwrap();
    let dump = dense_and_clustered(dir.path());
    let options = ScanOptions::default();
    let bounded = StatisticsRequest { max_rows: Some(MAX_ROWS), ..StatisticsRequest::DATA };
    let sizing =
        GroupSizing::Density { min_rows: ROW_GROUP_DEFAULT_MIN_ROWS, max_rows: Some(MAX_ROWS) };
    let tables = ["public.dense", "public.clustered"];

    // Flagless first: both blocks are at the default size, and both break the
    // maximum nobody has stated yet.
    let run = mapped_into_cache(&dump, &options, &StatisticsRequest::DATA).await;
    for table in tables {
        let held = statistics(block(&run.index, table));
        assert_eq!(held.group_size, ROW_GROUP_DEFAULT_SIZE_BYTES, "{table}");
        assert!(densest_group(held) > MAX_ROWS, "{table}: {}", densest_group(held));
    }

    let (run, source) = mapped_recording(&dump, &bounded).await;
    assert_eq!((run.lacking_statistics, run.backfilled), (2, 2), "both lacked the maximum");
    for table in tables {
        assert_eq!(source.reads_of(block(&run.index, table)), 2, "{table}: default, then finer");
    }
    let dense = statistics(block(&run.index, "public.dense")).clone();
    let clustered = statistics(block(&run.index, "public.clustered")).clone();
    for (table, held) in tables.iter().zip([&dense, &clustered]) {
        assert!(held.group_size < ROW_GROUP_DEFAULT_SIZE_BYTES, "{table}: {}", held.group_size);
        assert_eq!(held.sizing, sizing, "{table}");
    }
    assert!(densest_group(&dense) <= MAX_ROWS, "dense: {}", densest_group(&dense));
    assert!(densest_group(&clustered) > MAX_ROWS, "clustered keeps what the re-read gave");

    // The last read is the last one, for the block that met the maximum and
    // for the block that did not.
    let (run, source) = mapped_recording(&dump, &bounded).await;
    assert_eq!((run.lacking_statistics, run.backfilled), (0, 0));
    for table in tables {
        assert_eq!(source.reads_of(block(&run.index, table)), 0, "{table}");
    }
    assert_eq!(statistics(block(&run.index, "public.dense")), &dense);
    assert_eq!(statistics(block(&run.index, "public.clustered")), &clustered);

    // A block the run maps itself was gathered under the maximum already, so
    // the finer size is its only re-read, and gathers what the two reads did.
    let cold = tempfile::tempdir().unwrap();
    let cold_dump = cold.path().join("dense.sql");
    std::fs::copy(&dump, &cold_dump).unwrap();
    let (run, source) = mapped_recording(&cold_dump, &bounded).await;
    assert_eq!((run.lacking_statistics, run.backfilled), (2, 2), "both broke it once mapped");
    for (table, held) in tables.iter().zip([&dense, &clustered]) {
        assert_eq!(source.reads_of(block(&run.index, table)), 1, "{table}: the finer size only");
        assert_eq!(statistics(block(&run.index, table)), held, "{table}: cold");
    }

    // What the re-read gathered is what gathering exactly at that size
    // gathers, and what a split scan gathers.
    for (table, held) in tables.iter().zip([&dense, &clustered]) {
        let exact = gathered(&dump, &request(StatisticsSelection::DATA, held.group_size)).await;
        let exact = statistics(block(&exact, table));
        assert_eq!((&exact.groups, &exact.columns), (&held.groups, &held.columns), "{table}");
    }
    let parallel = ScanOptions {
        parallelism: Parallelism::workers(4, DEFAULT_MEMORY_BUDGET),
        chunk_size_bytes: 1 << 16,
        ..ScanOptions::default()
    };
    let split = gathered_with(&dump, &parallel, &bounded).await;
    for (table, held) in tables.iter().zip([&dense, &clustered]) {
        assert_eq!(statistics(block(&split, table)), held, "{table}: split");
    }
}

/// **A stated maximum turns the per-block group cap off and outranks the
/// minimum**: a block meeting neither bound ends at the coarsest size its
/// maximum allows, however far short of the minimum that leaves its median
/// group — only a caller who stated a maximum reaches the conflict, and a
/// default does not quietly overrule what they asked for.
#[tokio::test]
async fn a_stated_maximum_outranks_the_cap_and_the_minimum() {
    const MAX_ROWS: u64 = 750;
    let dir = tempfile::tempdir().unwrap();
    let dump = dense_and_clustered(dir.path());
    let options = ScanOptions::default();
    assert_eq!(StatisticsRequest::DATA.group_cap(), Some(BLOCK_MAX_ROW_GROUPS));
    let bounded = StatisticsRequest { max_rows: Some(MAX_ROWS), ..StatisticsRequest::DATA };
    assert_eq!(bounded.group_cap(), None, "a stated maximum is a size a caller asked for");

    let run = mapped_into_cache(&dump, &options, &bounded).await;
    let held = statistics(block(&run.index, "public.dense"));
    assert!(densest_group(held) <= MAX_ROWS, "{}", densest_group(held));
    let mut rows: Vec<u64> = held.groups.iter().map(|group| group.rows).collect();
    rows.sort_unstable();
    let median = rows[rows.len() / 2];
    assert!(median < ROW_GROUP_DEFAULT_MIN_ROWS, "the minimum is unmet at {median} rows");
    let flagless = gathered(&dump, &StatisticsRequest::DATA).await;
    let coarse = statistics(block(&flagless, "public.dense"));
    assert!(
        held.groups.len() > coarse.groups.len(),
        "{} groups against the {} a flagless parse keeps",
        held.groups.len(),
        coarse.groups.len()
    );
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
    let wanted = request(StatisticsSelection::DATA, 256);
    let serial = ScanOptions { chunk_size_bytes: 64, ..ScanOptions::default() };
    let reference = gathered_with(&dump, &serial, &wanted).await;
    for jobs in [2, 4, 8] {
        let _ = std::fs::remove_file(cache::colocated_path(&dump));
        mapped_into_cache(&dump, &serial, &StatisticsRequest::METADATA).await;
        let parallel = ScanOptions {
            parallelism: Parallelism::workers(jobs, DEFAULT_MEMORY_BUDGET),
            ..serial.clone()
        };
        let run = mapped_into_cache(&dump, &parallel, &wanted).await;
        assert_eq!(run.backfilled, 1, "{jobs} jobs");
        assert_eq!(run.index.spans, reference.spans, "{jobs} jobs");
    }
}

/// **A parse fails on the first field PostgreSQL refuses, whatever the worker
/// count and on a back-fill too**, naming the line it is on as `COPY` numbers
/// it and by its offset (`docs/design/roadmap.md`, "A literal is guaranteed
/// in `*_out`'s form and never read past `*_in`'s"): a `smallint` past its
/// width, then a day its month lacks, the block the one
/// [`a_gathering_scan_is_the_serial_scan_whatever_the_worker_count`] splits.
/// A spelling this build does not read, which the server does — a blank
/// before a number, a run-together zone — fails nothing: its group loses its
/// bounds. Hand-written, as no `pg_dump` output holds the first two.
#[tokio::test]
async fn a_parse_fails_at_the_first_field_postgresql_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("refused.sql");
    let mut text = String::from(
        "CREATE TABLE public.t (\n    a smallint,\n    d date,\n    z timestamp with time zone\n);\n\n",
    );
    text.push_str("COPY public.t (a, d, z) FROM stdin;\n");
    let mut first = None;
    for i in 0..2000 {
        let (a, d) = match i {
            700 => ("70000".to_string(), "2020-01-01"),
            1500 => (i.to_string(), "2020-02-30"),
            _ => (format!("{}{i}", if i % 7 == 0 { " " } else { "" }), "2020-01-01"),
        };
        if i == 700 {
            first = Some(text.len() as u64);
        }
        text.push_str(&format!("{a}\t{d}\t2020-01-01 00:00:00+0530\n"));
    }
    text.push_str("\\.\n\nSELECT 1;\n");
    std::fs::write(&dump, &text).unwrap();
    let first = first.unwrap();
    let refused = |run: pgdump_query::Result<MapRun>, at: &str| match run {
        Err(pgdump_query::Error::FieldRefused {
            table,
            column,
            declared_type,
            line,
            line_offset,
            value,
        }) => {
            assert_eq!(
                (table.as_str(), column.as_str(), declared_type.as_str()),
                ("public.t", "a", "smallint"),
                "{at}"
            );
            // The 701st data line, the block's rows counting from 1 as a
            // restore counts them.
            assert_eq!((line, line_offset, value.as_str()), (701, first, "70000"), "{at}");
        }
        other => panic!("{at}: expected the parse to refuse `70000`, got {other:?}"),
    };
    let wanted = request(StatisticsSelection::DATA, 256);
    let serial = ScanOptions { chunk_size_bytes: 64, ..ScanOptions::default() };
    let source = LocalFileSource::open(&dump).unwrap();
    refused(map_file(&source, &serial, &CacheMode::DISABLED, &wanted).await, "serial");
    for jobs in [2, 4, 8] {
        let parallel = ScanOptions {
            parallelism: Parallelism::workers(jobs, DEFAULT_MEMORY_BUDGET),
            ..serial.clone()
        };
        let at = format!("{jobs} jobs");
        refused(map_file(&source, &parallel, &CacheMode::DISABLED, &wanted).await, &at);
        let _ = std::fs::remove_file(cache::colocated_path(&dump));
        mapped_into_cache(&dump, &serial, &StatisticsRequest::METADATA).await;
        let mode = CacheMode::enabled(cache::colocated_path(&dump));
        refused(map_file(&source, &parallel, &mode, &wanted).await, &format!("{at}, back-fill"));
    }

    // Past the first, the next: the day 2020-02-30.
    let fixed = text.replacen("\n70000\t", "\n700\t", 1);
    std::fs::write(&dump, &fixed).unwrap();
    let source = LocalFileSource::open(&dump).unwrap();
    match map_file(&source, &serial, &CacheMode::DISABLED, &wanted).await {
        Err(pgdump_query::Error::FieldRefused { column, value, line, .. }) => {
            assert_eq!((column.as_str(), value.as_str(), line), ("d", "2020-02-30", 1501));
        }
        other => panic!("expected the parse to refuse `2020-02-30`, got {other:?}"),
    }

    // And with neither, the shortfalls fail nothing.
    std::fs::write(&dump, fixed.replacen("\t2020-02-30\t", "\t2020-02-28\t", 1)).unwrap();
    let index = gathered_with(&dump, &serial, &wanted).await;
    let t = statistics(block(&index, "public.t"));
    for at in [0, 2] {
        let column = t.columns[at].as_ref().unwrap();
        assert!(column.bounds.as_ref().unwrap().groups.iter().any(Option::is_none), "{at}");
    }
}

/// **Told to ignore them, a parse goes on past every field PostgreSQL
/// refuses**, serially, at every worker count and on a back-fill, and the
/// group each sits in keeps no bounds or dictionary of its column, nor the
/// column a sum, so no statistic says what a read of it would not: every other
/// group keeps its own (`docs/design/decisions.md`, "D103"). **The block
/// records what it went past per column**, each column's first in full and a
/// count, alike in every arrangement.
#[tokio::test]
async fn a_parse_ignoring_refused_fields_keeps_no_statistic_of_one() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("ignored.sql");
    let mut text =
        String::from("CREATE TABLE public.t (\n    a smallint,\n    f double precision\n);\n\n");
    text.push_str("COPY public.t (a, f) FROM stdin;\n");
    let data = text.len() as u64;
    let mut refused_groups = Vec::new();
    let mut refused_offsets = Vec::new();
    for i in 0..2000 {
        let (a, f) = match i {
            700 => ("70000".to_string(), "1.5".to_string()),
            1500 => (i.to_string(), "1.79769313486232e+308".to_string()),
            _ => (i.to_string(), format!("{i}.5")),
        };
        if i == 700 || i == 1500 {
            refused_offsets.push(text.len() as u64 - data);
            refused_groups.push(((text.len() as u64 - data) / 256) as usize);
        }
        text.push_str(&format!("{a}\t{f}\n"));
    }
    text.push_str("\\.\n\nSELECT 1;\n");
    std::fs::write(&dump, &text).unwrap();
    let wanted = request(StatisticsSelection::DATA, 256);
    let serial = ScanOptions {
        chunk_size_bytes: 64,
        postgres_invalid_values: PostgresInvalidValues::Ignore,
        ..ScanOptions::default()
    };
    let check = |index: &DumpIndex, at: &str| {
        let t = statistics(block(index, "public.t"));
        for (column, refused) in [(0, refused_groups[0]), (1, refused_groups[1])] {
            let gathered = t.columns[column].as_ref().unwrap();
            let bounds = &gathered.bounds.as_ref().unwrap().groups;
            let unbounded: Vec<usize> =
                (0..bounds.len()).filter(|&g| bounds[g].is_none()).collect();
            assert_eq!(unbounded, [refused], "{at}: column {column}'s groups without bounds");
            if let Some(dictionary) = &gathered.dictionary {
                let lost: Vec<usize> = (0..dictionary.groups.len())
                    .filter(|&g| dictionary.groups[g].is_none())
                    .collect();
                assert_eq!(lost, [refused], "{at}: column {column}'s groups without a dictionary");
            }
        }
        assert_eq!(t.columns[0].as_ref().unwrap().sums, None, "{at}: `a`'s sums");
        let first = |column: usize, line: u64, declared_type: &str, value: &str| ColumnRefusals {
            first: FieldRefusal {
                offset: refused_offsets[column],
                line,
                column,
                declared_type: declared_type.to_string(),
                value: value.to_string(),
            },
            count: 1,
        };
        let columns = vec![
            first(0, 701, "smallint", "70000"),
            first(1, 1501, "double precision", "1.79769313486232e+308"),
        ];
        let recorded = block(index, "public.t").ignored_refusals.clone();
        assert_eq!(recorded.as_deref(), Some(&IgnoredRefusals { columns }), "{at}");
    };
    check(&gathered_with(&dump, &serial, &wanted).await, "serial");
    let source = LocalFileSource::open(&dump).unwrap();
    for jobs in [2, 4, 8] {
        let parallel = ScanOptions {
            parallelism: Parallelism::workers(jobs, DEFAULT_MEMORY_BUDGET),
            ..serial.clone()
        };
        let at = format!("{jobs} jobs");
        check(&gathered_with(&dump, &parallel, &wanted).await, &at);
        let _ = std::fs::remove_file(cache::colocated_path(&dump));
        mapped_into_cache(&dump, &serial, &StatisticsRequest::METADATA).await;
        let mode = CacheMode::enabled(cache::colocated_path(&dump));
        let run = map_file(&source, &parallel, &mode, &wanted).await.unwrap();
        check(&run.index, &format!("{at}, back-fill"));
    }
}

/// **A parse refusing fields PostgreSQL refuses fails over a cache recording
/// one an ignoring parse went past**, with the recorded refusal, naming the
/// cache, before reading the block or writing the cache — so a clean verdict
/// does not depend on which run gathered the cache. A request leaving the
/// table at the metadata level keys none of its fields and fails nothing, and
/// an ignoring parse over the cache keeps the record.
#[tokio::test]
async fn a_parse_refusing_fields_fails_with_what_an_ignoring_parse_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("recorded.sql");
    let mut text = String::from("CREATE TABLE public.t (\n    a smallint\n);\n\n");
    text.push_str("COPY public.t (a) FROM stdin;\n");
    let mut first = 0;
    for i in 0..200 {
        if i == 70 {
            first = text.len() as u64;
            text.push_str("70000\n");
        } else {
            text.push_str(&format!("{i}\n"));
        }
    }
    text.push_str("\\.\n\nSELECT 1;\n");
    std::fs::write(&dump, &text).unwrap();
    let ignoring = ScanOptions {
        postgres_invalid_values: PostgresInvalidValues::Ignore,
        ..ScanOptions::default()
    };
    let mapped = mapped_into_cache(&dump, &ignoring, &StatisticsRequest::DATA).await;
    let recorded = block(&mapped.index, "public.t").clone();
    assert_eq!(recorded.ignored_refusals.as_ref().map(|r| r.columns[0].count), Some(1));
    let path = cache::colocated_path(&dump);
    let saved = std::fs::read(&path).unwrap();

    let source = RecordingSource::open(&dump);
    let mode = CacheMode::enabled(path.clone());
    let run = map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA).await;
    let Err(pgdump_query::Error::FieldRefusedRecorded { refused, cache }) = run else {
        panic!("expected the recorded refusal, got {run:?}");
    };
    assert_eq!(&*cache, path.as_path());
    match *refused {
        pgdump_query::Error::FieldRefused {
            table,
            column,
            declared_type,
            line,
            line_offset,
            value,
        } => {
            assert_eq!(
                (table.as_str(), column.as_str(), declared_type.as_str(), value.as_str()),
                ("public.t", "a", "smallint", "70000")
            );
            assert_eq!((line, line_offset), (71, first));
        }
        other => panic!("expected the refusal as the dump's read raises it, got {other:?}"),
    }
    assert_eq!(source.reads_of(&recorded), 0, "nothing was re-read");
    assert_eq!(std::fs::read(&path).unwrap(), saved, "nothing was written");

    let metadata = StatisticsRequest::METADATA;
    let refusing = mapped_into_cache(&dump, &ScanOptions::default(), &metadata).await;
    assert_eq!(block(&refusing.index, "public.t"), &recorded);
    let again = mapped_into_cache(&dump, &ignoring, &StatisticsRequest::DATA).await;
    assert_eq!(block(&again.index, "public.t"), &recorded);
}

/// **A recorded refusal fails exactly the refusing parses tracking its
/// column**, quoting the first among the columns tracked, in row and then
/// column order — the field a read keying those columns meets first — and a
/// request tracking none of them passes, keeping the record. So does one
/// whose back-fill re-reads the block, gathering the refused columns again
/// only because the block held them: it goes past their fields and records
/// them as before, whichever runs gathered the cache.
#[tokio::test]
async fn a_recorded_refusal_fails_exactly_the_parses_tracking_its_column() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("per-column.sql");
    let mut text = String::from("CREATE TABLE public.t (\n");
    text.push_str("    a smallint,\n    b smallint,\n    c smallint,\n    d smallint\n);\n\n");
    text.push_str("COPY public.t (a, b, c, d) FROM stdin;\n");
    let mut offsets = Vec::new();
    for i in 0..200 {
        offsets.push(text.len() as u64);
        let row = match i {
            30 | 150 => format!("{i}\t{i}\t70000\t{i}"),
            60 => format!("70000\t-70000\t{i}\t{i}"),
            _ => format!("{i}\t{i}\t{i}\t{i}"),
        };
        text.push_str(&row);
        text.push('\n');
    }
    text.push_str("\\.\n\nSELECT 1;\n");
    std::fs::write(&dump, &text).unwrap();
    let ignoring = ScanOptions {
        postgres_invalid_values: PostgresInvalidValues::Ignore,
        ..ScanOptions::default()
    };
    let mapped = mapped_into_cache(&dump, &ignoring, &StatisticsRequest::DATA).await;
    let recorded = block(&mapped.index, "public.t").clone();
    let counts: Vec<(usize, u64)> = recorded
        .ignored_refusals
        .iter()
        .flat_map(|r| &r.columns)
        .map(|c| (c.first.column, c.count))
        .collect();
    assert_eq!(counts, [(0, 1), (1, 1), (2, 2)], "each refused column, in column order");
    let path = cache::colocated_path(&dump);
    let mode = CacheMode::enabled(path.clone());
    let source = LocalFileSource::open(&dump).unwrap();
    let tracking = |columns: &[&str]| StatisticsRequest {
        selection: only(
            columns
                .iter()
                .map(|c| StatisticsTarget::Column {
                    table: "public.t".to_string(),
                    column: (*c).to_string(),
                })
                .collect(),
        ),
        ..StatisticsRequest::DATA
    };
    // The columns tracked, and the column, line and value the refusal quotes.
    type Case = (&'static [&'static str], Option<(&'static str, u64, &'static str)>);
    let cases: [Case; 6] = [
        (&["a", "b", "c", "d"], Some(("c", 31, "70000"))),
        (&["a", "b"], Some(("a", 61, "70000"))),
        (&["b", "d"], Some(("b", 61, "-70000"))),
        (&["c"], Some(("c", 31, "70000"))),
        (&["d"], None),
        (&[], None),
    ];
    for (columns, expected) in cases {
        let run = map_file(&source, &ScanOptions::default(), &mode, &tracking(columns)).await;
        let Some((column, line, value)) = expected else {
            let run = run.unwrap_or_else(|e| panic!("{columns:?}: tracks no refused column: {e}"));
            assert_eq!(
                block(&run.index, "public.t").ignored_refusals,
                recorded.ignored_refusals,
                "{columns:?}: the record is kept"
            );
            continue;
        };
        let Err(pgdump_query::Error::FieldRefusedRecorded { refused, .. }) = run else {
            panic!("{columns:?}: expected the recorded refusal, got {run:?}");
        };
        let pgdump_query::Error::FieldRefused {
            column: named,
            line: at,
            line_offset,
            value: v,
            ..
        } = *refused
        else {
            panic!("{columns:?}: expected the refusal as a read raises it, got {refused:?}");
        };
        let quoted = (named.as_str(), at, v.as_str(), line_offset);
        assert_eq!(quoted, (column, line, value, offsets[line as usize - 1]), "{columns:?}");
    }

    // A back-fill: another group size re-reads the block, gathering `a`, `b`
    // and `c` because the block held them, and fails on none of them.
    let recording = RecordingSource::open(&dump);
    let resized =
        StatisticsRequest { group_size: Some(NonZeroU64::new(128).unwrap()), ..tracking(&["d"]) };
    let run = map_file(&recording, &ScanOptions::default(), &mode, &resized).await.unwrap();
    let reread = block(&run.index, "public.t");
    assert!(recording.reads_of(reread) > 0, "the block was re-read");
    assert_eq!(statistics(reread).group_size, 128);
    assert!(statistics(reread).columns.iter().all(Option::is_some), "every held column kept");
    assert_eq!(reread.ignored_refusals, recorded.ignored_refusals, "the record is renewed alike");
    let refusing = map_file(&source, &ScanOptions::default(), &mode, &tracking(&["c"])).await;
    assert!(
        matches!(refusing, Err(pgdump_query::Error::FieldRefusedRecorded { .. })),
        "{refusing:?}"
    );
}

/// `ScanOptions` reading under `invalid`, serially in 64-byte chunks or at
/// `jobs` workers.
fn reading(invalid: PostgresInvalidValues, jobs: usize) -> ScanOptions {
    let parallelism = if jobs > 1 {
        Parallelism::workers(jobs, DEFAULT_MEMORY_BUDGET)
    } else {
        ScanOptions::default().parallelism
    };
    ScanOptions {
        chunk_size_bytes: 64,
        parallelism,
        postgres_invalid_values: invalid,
        ..ScanOptions::default()
    }
}

/// One table `public.t (a smallint, v smallint[], n numeric(10,2), r
/// int4range, j json, b bit(3))` of `rows` clean rows, row `bad` (from 0) given `field` in
/// column `column` (by position) where it is `Some`.
fn strict_dump(dir: &Path, rows: usize, bad: Option<(usize, usize, &str)>) -> std::path::PathBuf {
    let dump = dir.join("strict.sql");
    let mut text = String::from(
        "CREATE TABLE public.t (\n    a smallint,\n    v smallint[],\n    n numeric(10,2),\n    r int4range,\n    j json,\n    b bit(3)\n);\n\n",
    );
    text.push_str("COPY public.t (a, v, n, r, j, b) FROM stdin;\n");
    for i in 0..rows {
        let mut fields = [
            i.to_string(),
            format!("{{{i},1}}"),
            format!("{i}.50"),
            format!("[{i},{})", i + 1),
            format!("{{\"i\": [{i}]}}"),
            format!("{:03b}", i % 8),
        ];
        if let Some((row, column, field)) = bad
            && row == i
        {
            fields[column] = field.to_string();
        }
        text.push_str(&fields.join("\t"));
        text.push('\n');
    }
    text.push_str("\\.\n\nSELECT 1;\n");
    std::fs::write(&dump, &text).unwrap();
    dump
}

/// **A strict parse fails at a field PostgreSQL refuses wherever it sits**,
/// where a default one leaves it to a query and passes: in a column the
/// request leaves at the metadata level, an array's element, a value too long
/// to key, a range's bound order, a `json` held as its text, a `bit(3)` of
/// four bits, ordered by nothing here, and the rows of a
/// block whose statistics declined — serially and at four workers, naming the field as a read of the
/// dump does. A clean strict parse gathers what a default one gathers and
/// records every block checked in full, the metadata-level table staying at
/// its level (`PostgresInvalidValues::Strict`).
#[tokio::test]
async fn a_strict_parse_refuses_the_fields_a_default_one_leaves_to_a_query() {
    let dir = tempfile::tempdir().unwrap();
    let long = format!("1{}", "0".repeat(DICTIONARY_ENTRY_MAX_BYTES + 1));
    let only_v = StatisticsRequest {
        selection: only(vec![StatisticsTarget::Column {
            table: "public.t".to_string(),
            column: "v".to_string(),
        }]),
        ..StatisticsRequest::DATA
    };
    // What is bad, where, and the request and allowance a default parse
    // passes it under.
    let cases: [(&str, usize, &str, &StatisticsRequest, Option<u64>); 7] = [
        ("a column at the metadata level", 0, "70000", &only_v, None),
        ("an array's element", 1, "{1,70000}", &StatisticsRequest::DATA, None),
        ("a value too long to key", 2, &long, &StatisticsRequest::DATA, None),
        ("a range's bound order", 3, "[5,1]", &StatisticsRequest::DATA, None),
        ("a declined block's row", 0, "70000", &StatisticsRequest::DATA, Some(1)),
        ("a json held as its text", 4, "{\"i\": [1,]}", &StatisticsRequest::DATA, None),
        ("a bit string past its length", 5, "1010", &StatisticsRequest::DATA, None),
    ];
    let names = ["a", "v", "n", "r", "j", "b"];
    for (what, column, field, wanted, allowance) in cases {
        let dump = strict_dump(dir.path(), 400, Some((300, column, field)));
        let source = LocalFileSource::open(&dump).unwrap();
        for jobs in [1, 4] {
            let at = format!("{what}, {jobs} job(s)");
            let default = ScanOptions {
                statistics_allowance_bytes: allowance,
                ..reading(PostgresInvalidValues::Default, jobs)
            };
            let passed = map_file(&source, &default, &CacheMode::DISABLED, wanted).await;
            let passed = passed.unwrap_or_else(|e| panic!("{at}: a default parse passes: {e}"));
            let held = block(&passed.index, "public.t");
            assert!(!held.checked_in_full, "{at}");
            assert_eq!(held.statistics_declined.is_some(), allowance.is_some(), "{at}");
            let strict =
                ScanOptions { postgres_invalid_values: PostgresInvalidValues::Strict, ..default };
            match map_file(&source, &strict, &CacheMode::DISABLED, wanted).await {
                Err(pgdump_query::Error::FieldRefused { column: named, line, value, .. }) => {
                    assert_eq!((named.as_str(), line, value.as_str()), (names[column], 301, field));
                }
                other => panic!("{at}: expected the strict parse to refuse it, got {other:?}"),
            }
        }
    }

    let dump = strict_dump(dir.path(), 400, None);
    let source = LocalFileSource::open(&dump).unwrap();
    for jobs in [1, 4] {
        let at = format!("{jobs} job(s)");
        let default = reading(PostgresInvalidValues::Default, jobs);
        let strict = reading(PostgresInvalidValues::Strict, jobs);
        for wanted in [StatisticsRequest::DATA, StatisticsRequest::METADATA] {
            let read = map_file(&source, &default, &CacheMode::DISABLED, &wanted).await.unwrap();
            let checked = map_file(&source, &strict, &CacheMode::DISABLED, &wanted).await.unwrap();
            let (read, checked) =
                (block(&read.index, "public.t"), block(&checked.index, "public.t"));
            assert!(checked.checked_in_full, "{at}");
            let unmarked = CopyBlock { checked_in_full: false, ..checked.clone() };
            assert_eq!(&unmarked, read, "{at}: a strict parse records what a default one does");
        }
    }
}

/// **A strict parse re-reads each block the cache holds that no strict parse
/// checked, once**, checking every field and recording that it did, and
/// gathering in the same read whatever statistics the block lacks; a strict
/// parse after it re-reads nothing, and a default one keeps the record. Over
/// an ignoring parse's cache it does not stop at the recorded refusal but
/// re-reads the block, failing at the first field there, in a column the
/// ignoring parse did not track.
#[tokio::test]
async fn a_strict_parse_checks_each_held_block_no_strict_parse_checked() {
    let dir = tempfile::tempdir().unwrap();
    let dump = strict_dump(dir.path(), 400, None);
    let strict = reading(PostgresInvalidValues::Strict, 1);
    let data = StatisticsRequest::DATA;
    let mode = CacheMode::enabled(cache::colocated_path(&dump));
    for (earlier, gathers) in [(&data, false), (&StatisticsRequest::METADATA, true)] {
        let _ = std::fs::remove_file(cache::colocated_path(&dump));
        let held = mapped_into_cache(&dump, &ScanOptions::default(), earlier).await;
        assert!(!block(&held.index, "public.t").checked_in_full);
        let source = RecordingSource::open(&dump);
        let run = map_file(&source, &strict, &mode, &data).await.unwrap();
        let checked = block(&run.index, "public.t");
        assert!(checked.checked_in_full, "{gathers}");
        assert_eq!(run.checked, 1, "{gathers}");
        assert_eq!(run.backfilled, usize::from(gathers), "{gathers}");
        assert_eq!(source.reads_of(checked), 1, "{gathers}: one read checks and gathers");
        let fresh = gathered(&dump, &data).await;
        assert_eq!(checked.statistics, block(&fresh, "public.t").statistics, "{gathers}");

        let source = RecordingSource::open(&dump);
        let again = map_file(&source, &strict, &mode, &data).await.unwrap();
        assert_eq!((again.checked, source.reads_of(checked)), (0, 0), "{gathers}");
        let default = mapped_into_cache(&dump, &ScanOptions::default(), &data).await;
        assert!(block(&default.index, "public.t").checked_in_full, "{gathers}: kept");
    }

    // An ignoring parse tracking `v` records the element it went past in row
    // 60, not the `smallint` in row 30 of `a`, which it did not track.
    let dump = strict_dump(dir.path(), 400, Some((60, 1, "{70000}")));
    let mut text = std::fs::read_to_string(&dump).unwrap();
    text = text.replacen("\n30\t{30,1}", "\n70000\t{30,1}", 1);
    std::fs::write(&dump, &text).unwrap();
    let _ = std::fs::remove_file(cache::colocated_path(&dump));
    let ignoring = reading(PostgresInvalidValues::Ignore, 1);
    let only_v = StatisticsRequest {
        selection: only(vec![StatisticsTarget::Column {
            table: "public.t".to_string(),
            column: "v".to_string(),
        }]),
        ..data
    };
    mapped_into_cache(&dump, &ignoring, &only_v).await;
    let source = LocalFileSource::open(&dump).unwrap();
    match map_file(&source, &strict, &mode, &data).await {
        Err(pgdump_query::Error::FieldRefused { column, line, value, .. }) => {
            assert_eq!((column.as_str(), line, value.as_str()), ("a", 31, "70000"));
        }
        other => panic!("expected the re-read's refusal, got {other:?}"),
    }
}

/// A source that trips a cancellation once a read starts at or past `trip`,
/// the read itself succeeding: an interrupt at a file offset.
struct CancelsPast {
    inner: LocalFileSource,
    trip: u64,
    cancel: Arc<pgdump_query::Cancellation>,
}

impl ByteRangeSource for CancelsPast {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = pgdump_query::Result<Bytes>> + Send + '_>> {
        if offset >= self.trip {
            self.cancel.cancel();
        }
        self.inner.read_range(offset, len)
    }

    fn size(&self) -> Pin<Box<dyn Future<Output = pgdump_query::Result<u64>> + Send + '_>> {
        self.inner.size()
    }

    fn modified(
        &self,
    ) -> Pin<Box<dyn Future<Output = pgdump_query::Result<Option<SystemTime>>> + Send + '_>> {
        self.inner.modified()
    }
}

/// **A strict parse resuming a map short of the end checks the blocks it
/// holds before it maps on**, so it fails at the first field PostgreSQL
/// refuses in file order, in a block an interrupted default parse mapped
/// rather than one past where it stopped — and over a clean held block,
/// checks it and maps the rest checked.
#[tokio::test]
async fn a_strict_parse_resuming_checks_what_the_cache_holds_first() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("two.sql");
    let ddl = "CREATE TABLE public.t1 (\n    a smallint\n);\n\n\
               CREATE TABLE public.t2 (\n    a smallint\n);\n\n";
    let table = |name: &str, bad: bool| {
        let mut text = format!("COPY public.{name} (a) FROM stdin;\n");
        for i in 0..200 {
            let field = if bad && i == 100 { "70000".to_string() } else { i.to_string() };
            text.push_str(&format!("{field}\n"));
        }
        text.push_str("\\.\n\n");
        text
    };
    for first_bad in [true, false] {
        let text = format!("{ddl}{}{}SELECT 1;\n", table("t1", first_bad), table("t2", true));
        std::fs::write(&dump, &text).unwrap();
        let _ = std::fs::remove_file(cache::colocated_path(&dump));
        let cancel = Arc::new(pgdump_query::Cancellation::new());
        let trip = text.find("COPY public.t2").unwrap() as u64;
        let tripping = CancelsPast {
            inner: LocalFileSource::open(&dump).unwrap(),
            trip,
            cancel: cancel.clone(),
        };
        let interrupted =
            ScanOptions { cancel: Some(cancel), ..reading(PostgresInvalidValues::Default, 1) };
        let mode = CacheMode::enabled(cache::colocated_path(&dump));
        let run =
            map_file(&tripping, &interrupted, &mode, &StatisticsRequest::METADATA).await.unwrap();
        assert!(run.interrupted, "{first_bad}");
        assert_eq!(run.index.blocks().count(), 1, "{first_bad}: the cache holds t1 alone");

        let source = LocalFileSource::open(&dump).unwrap();
        let strict = reading(PostgresInvalidValues::Strict, 1);
        let resumed = map_file(&source, &strict, &mode, &StatisticsRequest::DATA).await;
        match resumed {
            Err(pgdump_query::Error::FieldRefused { table, line, .. }) => {
                let expected = if first_bad { "public.t1" } else { "public.t2" };
                assert_eq!((table.as_str(), line), (expected, 101), "{first_bad}");
            }
            other => panic!("{first_bad}: expected a refusal, got {other:?}"),
        }
        // The held block's check was banked before the tail was mapped.
        if !first_bad {
            let Ok(cache::CacheLoad::Index(held)) = mode.load(&source).await else {
                panic!("the cache loads");
            };
            assert!(block(&held, "public.t1").checked_in_full);
        }
    }
}

/// **A field of a kind this build reads narrower than its input function
/// fails the parse where the server refuses it, and nowhere else** (I65–I69):
/// each column's refused spelling in turn, a `bytea` among them, whose bounds
/// are placed bytewise rather than keyed. A spelling the server reads where
/// this build does not fails nothing, and a network's is read outright.
/// Hand-written, as no `pg_dump` output holds any of them.
#[tokio::test]
async fn a_parse_fails_where_an_input_function_refuses_and_not_where_it_reads() {
    // Column, type, the dump's spelling, one the server reads, one it refuses.
    // A `bytea`'s backslash is doubled as COPY writes it.
    let columns = [
        ["b", "boolean", "t", "yes", "maybe"],
        ["o", "oid", "1", "0x1F", "4294967296"],
        ["i", "inet", "10.0.0.1", "10.1.2/24", "::1/08"],
        ["c", "cidr", "10.0.0.0/8", "10", "10.0.0.1/8"],
        ["m", "macaddr", "08:00:2b:01:02:03", "08-00-2b-01-02-03", "08:00:2b:01:02:100"],
        ["m8", "macaddr8", "08:00:2b:01:02:03:04:05", "08002b0102030405", "08:00:2b:01:02:03:04"],
        ["y", "bytea", "\\\\x00", "\\\\x 00", "\\\\x0"],
    ];
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("refused.sql");
    let names: Vec<&str> = columns.iter().map(|c| c[0]).collect();
    let declared: Vec<String> = columns.iter().map(|c| format!("    {} {}", c[0], c[1])).collect();
    // Every column in the dump's spelling but `at`, in spelling `k`.
    let row = |at: usize, k: usize| {
        let fields: Vec<&str> =
            columns.iter().enumerate().map(|(i, c)| c[if i == at { k } else { 2 }]).collect();
        fields.join("\t")
    };
    let dump_with = |refused: Option<usize>| {
        let mut text = format!(
            "CREATE TABLE public.t (\n{}\n);\n\nCOPY public.t ({}) FROM stdin;\n",
            declared.join(",\n"),
            names.join(", ")
        );
        for i in 0..40 {
            let line = match (i, refused) {
                (10..17, _) => row(i - 10, 3),
                (30, Some(at)) => row(at, 4),
                _ => row(usize::MAX, 2),
            };
            text.push_str(&line);
            text.push('\n');
        }
        text.push_str("\\.\n\nSELECT 1;\n");
        std::fs::write(&dump, text).unwrap();
    };
    let wanted = request(StatisticsSelection::DATA, 256);
    let serial = ScanOptions { chunk_size_bytes: 64, ..ScanOptions::default() };
    for (at, [column, declared_type, _, _, refused]) in columns.iter().enumerate() {
        dump_with(Some(at));
        let source = LocalFileSource::open(&dump).unwrap();
        match map_file(&source, &serial, &CacheMode::DISABLED, &wanted).await {
            Err(pgdump_query::Error::FieldRefused {
                column: c,
                declared_type: d,
                value,
                line,
                ..
            }) => {
                let unescaped = refused.replace("\\\\", "\\");
                assert_eq!(
                    (c.as_str(), d.as_str(), value.as_str(), line),
                    (*column, *declared_type, &*unescaped, 31)
                );
            }
            other => panic!("{column}: expected the parse to refuse `{refused}`, got {other:?}"),
        }
    }
    dump_with(None);
    let index = gathered_with(&dump, &serial, &wanted).await;
    let t = statistics(block(&index, "public.t"));
    for (at, [column, ..]) in columns.iter().enumerate() {
        let bounds = t.columns[at].as_ref().unwrap().bounds.as_ref().unwrap();
        // An unread spelling loses its group's bounds; a network is read whole.
        let read = matches!(*column, "i" | "c");
        assert_eq!(bounds.groups.iter().any(Option::is_none), !read, "{column}");
    }
}

/// **An enum field naming no label of its type fails the parse where the
/// preamble holds the labels exactly** (I70), whether `CREATE TYPE` declares
/// them or a `--binary-upgrade` run of `ADD VALUE` does, **and goes on past
/// it, its group unbounded, where a statement that could change them was not
/// read** — each of which, here, makes the field one a restore reads.
#[tokio::test]
async fn a_parse_fails_at_an_undeclared_label_only_where_the_labels_are_exact() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("enum.sql");
    let declared = "CREATE TYPE public.mood AS ENUM (\n    'sad',\n    'ok'\n);";
    let dump_with = |preamble: &[&str]| {
        let mut text = preamble.join("\n\n");
        text.push_str("\n\nCREATE TABLE public.t (\n    m public.mood\n);\n\n");
        text.push_str("COPY public.t (m) FROM stdin;\n");
        for i in 0..40 {
            text.push_str(if i == 30 { "furious\n" } else { "sad\n" });
        }
        text.push_str("\\.\n\nSELECT 1;\n");
        std::fs::write(&dump, text).unwrap();
    };
    let wanted = request(StatisticsSelection::DATA, 64);
    let serial = ScanOptions { chunk_size_bytes: 64, ..ScanOptions::default() };
    let exact: [&[&str]; 2] = [
        &[declared],
        &[
            "CREATE TYPE public.mood AS ENUM (\n);",
            "ALTER TYPE public.mood ADD VALUE 'sad';",
            "ALTER TYPE public.mood ADD VALUE 'ok';",
        ],
    ];
    for preamble in exact {
        dump_with(preamble);
        let source = LocalFileSource::open(&dump).unwrap();
        match map_file(&source, &serial, &CacheMode::DISABLED, &wanted).await {
            Err(pgdump_query::Error::FieldRefused {
                column, declared_type, value, line, ..
            }) => assert_eq!(
                (column.as_str(), declared_type.as_str(), value.as_str(), line),
                ("m", "public.mood", "furious", 31)
            ),
            other => panic!("{preamble:?}: expected the parse to refuse `furious`, got {other:?}"),
        }
    }
    let inexact: [&[&str]; 4] = [
        &[declared, "ALTER TYPE public.mood RENAME VALUE 'ok' TO 'furious';"],
        &[declared, "ALTER TYPE public.mood ADD VALUE IF NOT EXISTS 'furious';"],
        &["CREATE TYPE public.mood AS ENUM ('sad', E'furious');"],
        &[declared, "ALTER TYPE mood ADD VALUE 'furious';"],
    ];
    for preamble in inexact {
        dump_with(preamble);
        let index = gathered_with(&dump, &serial, &wanted).await;
        let t = statistics(block(&index, "public.t"));
        let bounds = t.columns[0].as_ref().unwrap().bounds.as_ref().unwrap();
        assert!(bounds.groups.iter().any(Option::is_none), "{preamble:?}");
        assert!(bounds.groups.iter().any(Option::is_some), "{preamble:?}");
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
    let unstated = StatisticsRequest::DATA;
    let stated = request(StatisticsSelection::DATA, BASE);
    let serial = ScanOptions { chunk_size_bytes: 64, ..ScanOptions::default() };
    let (mut blocks, mut merged) = (0, 0);
    for fixture in common::all_fixtures() {
        let source = LocalFileSource::open(&fixture).unwrap();
        let (options, mode) = (ScanOptions::default(), CacheMode::DISABLED);
        let metadata = StatisticsRequest::METADATA;
        let run = map_file(&source, &options, &mode, &metadata);
        let index = run.await.unwrap().index;
        let refused = common::refused_field(&fixture).map(|r| r.table);
        // A block holding a field PostgreSQL refuses is refused by every
        // re-read (`tests/decode.rs`'s
        // `every_refused_field_fails_a_data_level_parse_reading_it`).
        for block in index.blocks().filter(|b| refused != Some(b.header.qualified_name().as_str()))
        {
            let backfill = unstated
                .backfill(block, &bounded_columns(block, index.metadata.as_ref()), None)
                .expect("the block holds no statistics");
            assert_eq!(backfill.group_cap, Some(BLOCK_MAX_ROW_GROUPS));
            assert_eq!(
                stated
                    .backfill(block, &bounded_columns(block, index.metadata.as_ref()), None)
                    .unwrap()
                    .group_cap,
                None
            );
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
                        .gathered
                        .gathered()
                        .expect("an unbounded allowance declines nothing")
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
    let unstated = StatisticsRequest::DATA;
    let stated = request(StatisticsSelection::DATA, BASE);
    let serial = ScanOptions { chunk_size_bytes: 64, ..ScanOptions::default() };
    let (mut blocks, mut coarsened) = (0, 0);
    for fixture in common::all_fixtures() {
        let source = LocalFileSource::open(&fixture).unwrap();
        let (options, mode) = (ScanOptions::default(), CacheMode::DISABLED);
        let metadata = StatisticsRequest::METADATA;
        let run = map_file(&source, &options, &mode, &metadata);
        let index = run.await.unwrap().index;
        let refused = common::refused_field(&fixture).map(|r| r.table);
        // A block holding a field PostgreSQL refuses is refused by every
        // re-read (`tests/decode.rs`'s
        // `every_refused_field_fails_a_data_level_parse_reading_it`).
        for block in index.blocks().filter(|b| refused != Some(b.header.qualified_name().as_str()))
        {
            let backfill = unstated
                .backfill(block, &bounded_columns(block, index.metadata.as_ref()), None)
                .expect("the block holds no statistics");
            let default_sizing =
                GroupSizing::Density { min_rows: ROW_GROUP_DEFAULT_MIN_ROWS, max_rows: None };
            assert_eq!(backfill.min_rows, Some(ROW_GROUP_DEFAULT_MIN_ROWS));
            assert_eq!(backfill.sizing, default_sizing);
            let exact_request = stated
                .backfill(block, &bounded_columns(block, index.metadata.as_ref()), None)
                .unwrap();
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
                        .gathered
                        .gathered()
                        .expect("an unbounded allowance declines nothing")
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
    let cache_path = dir.path().join("elsewhere").join("rewritten.dtcache");
    std::fs::create_dir(cache_path.parent().unwrap()).unwrap();
    let mode = CacheMode::enabled(cache_path.clone());
    let before = "COPY public.t (a) FROM stdin;\n11\n2\n\\.\nSELECT 1;\n";
    let after = "COPY public.t (a) FROM stdin;\n1\n\\.\n22\nSELECT 1;\n";
    assert_eq!(before.len(), after.len());
    std::fs::write(&dump, before).unwrap();
    let source = LocalFileSource::open(&dump).unwrap();
    let mapped = map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::METADATA)
        .await
        .unwrap()
        .index;
    let mapped_block = mapped.blocks().next().unwrap();
    let header_offset = mapped_block.header_offset;
    std::fs::write(&dump, after).unwrap();
    let source = LocalFileSource::open(&dump).unwrap();
    let err = map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA)
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

    let backfill = StatisticsRequest::DATA
        .backfill(mapped_block, &bounded_columns(mapped_block, mapped.metadata.as_ref()), None)
        .expect("it holds no statistics");
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

    // Cut short of the block's first row, which the per-block entry point is
    // the only one to reach: every scan entry point refuses the cache first,
    // by its stored size.
    std::fs::write(&dump, &before[..mapped_block.data_offset as usize - 1]).unwrap();
    let source = LocalFileSource::open(&dump).unwrap();
    let err = gather_block_statistics(
        &source,
        &ScanOptions::default(),
        mapped.metadata.as_ref(),
        mapped_block,
        &backfill,
    )
    .await
    .expect_err("the block no longer ends where the map says");
    assert!(
        matches!(
            &err,
            pgdump_query::Error::CachedBlockChanged { path: None, header_offset: at }
                if *at == header_offset
        ),
        "{err}"
    );
}

/// A dump of one long `COPY` block, written into `dir` — enough groups over
/// three tracked columns that an allowance a quarter of what gathering it
/// peaks at cannot hold them.
fn long_block(dir: &Path) -> std::path::PathBuf {
    let dump = dir.join("long_block.sql");
    let mut text = String::from(
        "CREATE TABLE public.t (\n    a integer,\n    b text COLLATE pg_catalog.\"C\",\n    c text\n);\n\n",
    );
    text.push_str("COPY public.t (a, b, c) FROM stdin;\n");
    for i in 0..4000 {
        text.push_str(&format!("{i}\t{:05}\tv{}\n", 4000 - i, i % 997));
    }
    text.push_str("\\.\n\nSELECT 1;\n");
    std::fs::write(&dump, &text).unwrap();
    dump
}

/// **A block whose statistics the allowance cannot hold declines, the scan
/// finishes, and the map records the allowance it declined under**
/// (`docs/design/decisions.md`, "D85"): a second run at the same allowance
/// re-reads nothing and says so again, and one stating no allowance
/// re-reads the block into exactly what an unbounded pass gathers, clearing
/// the record. Serial and at four workers, which declines the same way.
///
/// **Not vacuous**: the allowance is a quarter of the unbounded pass's own
/// peak, and that unbounded pass declines nothing on the same input.
#[tokio::test]
async fn a_block_past_the_allowance_declines_and_is_re_read_only_under_a_larger_one() {
    let dir = tempfile::tempdir().unwrap();
    let dump = long_block(dir.path());
    // A group size well under the block, so most of what the account sees
    // grows as the rows arrive rather than at the block's close: the decline
    // this exercises is the mid-scan one.
    let wanted = request(StatisticsSelection::DATA, 256);
    let source = LocalFileSource::open(&dump).unwrap();

    // The control: no allowance, so nothing declines, and the peak the
    // account reached gathering the whole block.
    let free =
        map_file(&source, &ScanOptions::default(), &CacheMode::DISABLED, &wanted).await.unwrap();
    assert_eq!(free.declined_statistics, 0, "an unbounded pass declines nothing");
    let reference = statistics(block(&free.index, "public.t")).clone();
    let allowance = free.statistics.peak / 4;
    assert!(allowance > 0);

    for jobs in [1, 4] {
        let (_cache_dir, dump) = sandboxed(&dump, "long_block.sql");
        let source = LocalFileSource::open(&dump).unwrap();
        let mode = CacheMode::enabled(cache::colocated_path(&dump));
        let tight = ScanOptions {
            parallelism: Parallelism::workers(jobs, DEFAULT_MEMORY_BUDGET),
            statistics_allowance_bytes: Some(allowance),
            chunk_size_bytes: 4096,
            ..ScanOptions::default()
        };
        let at = format!("{jobs} job(s)");

        // The scan runs to EOF and the block declines, holding nothing.
        let run = map_file(&source, &tight, &mode, &wanted).await.unwrap();
        assert!(!run.interrupted, "{at}");
        assert_eq!(run.declined_statistics, 1, "{at}");
        let declined = block(&run.index, "public.t");
        assert_eq!(declined.statistics_declined, Some(allowance), "{at}");
        assert!(declined.statistics.is_none(), "{at}");
        assert_eq!(run.statistics.now.total(), 0, "{at}: a declined pass holds nothing");

        // The same allowance re-reads nothing, and still says what it holds.
        let again = map_file(&source, &tight, &mode, &wanted).await.unwrap();
        assert_eq!((again.lacking_statistics, again.backfilled), (0, 0), "{at}");
        assert_eq!(again.declined_statistics, 1, "{at}");
        assert_eq!(block(&again.index, "public.t").statistics_declined, Some(allowance), "{at}");

        // No allowance re-reads it, into what the unbounded pass gave.
        let roomy = ScanOptions { statistics_allowance_bytes: None, ..tight.clone() };
        let wider = map_file(&source, &roomy, &mode, &wanted).await.unwrap();
        assert_eq!((wider.lacking_statistics, wider.backfilled), (1, 1), "{at}");
        assert_eq!(wider.declined_statistics, 0, "{at}");
        let filled = block(&wider.index, "public.t");
        assert_eq!(filled.statistics_declined, None, "{at}");
        assert_eq!(statistics(filled), &reference, "{at}");
    }
}
