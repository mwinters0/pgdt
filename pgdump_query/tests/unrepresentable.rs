//! The unrepresentable count a data-level mapping pass takes beside the
//! census (`docs/design/decisions.md`, "D96").
//!
//! `src/unrepresentable.rs`'s unit tests pin what one field is. These pin
//! what only a real dump shows: that `fixtures/*/types/default.sql`'s blocks
//! count the values their typed columns cannot hold, by column and tier, and
//! nothing else — a `text` field reading `infinity` and a bare `numeric`'s
//! `NaN` among the nothing — that the metadata level counts none and a
//! later data-level `parse` counts what a cold one does — and that statistics
//! count the same per group and keep each view where it differs
//! (`docs/design/decisions.md`, "D97"). Which values those are is held to
//! DataFusion's own path by `datafusion-pgdump`'s
//! `every_extreme_is_held_by_arrow_or_recorded`.

use std::collections::BTreeMap;
use std::path::Path;

use pgdump_query::cache::{self, CacheMode};
use pgdump_query::{
    BoundsSet, CopyBlock, DumpIndex, LocalFileSource, ScanOptions, StatisticsRequest,
    StatisticsView, Unrepresentable, map_file,
};

mod common;
use common::{VERSIONS, types_fixture};

/// `index`'s count for every column holding one, as `table.column`.
fn counts(index: &DumpIndex) -> BTreeMap<String, Unrepresentable> {
    let mut out = BTreeMap::new();
    for block in index.blocks() {
        let counts = block.unrepresentable.as_deref().unwrap_or_else(|| {
            panic!("{}: a data-level block is counted", block.header.qualified_name())
        });
        assert_eq!(counts.len(), block.header.columns.len());
        for (column, count) in block.header.columns.iter().zip(counts) {
            if !count.is_zero() {
                let key = format!("{}.{column}", block.header.table);
                out.entry(key).or_insert_with(Unrepresentable::default).merge(count);
            }
        }
    }
    out
}

/// A `parse` of `fixture`'s copy at `request`, answering its index.
async fn parsed(fixture: &Path, dir: &Path, request: &StatisticsRequest) -> DumpIndex {
    let dump = dir.join("dump.sql");
    if !dump.exists() {
        std::fs::copy(fixture, &dump).unwrap();
    }
    let source = LocalFileSource::open(&dump).unwrap();
    let mode = CacheMode::enabled(cache::colocated_path(&dump));
    map_file(&source, &ScanOptions::default(), &mode, request).await.unwrap().index
}

fn count(format: u64, engine: u64) -> Unrepresentable {
    Unrepresentable { format, engine }
}

/// **Every column the `types` fixture's typed tables count, and nothing
/// else**, outside the extremes: `NaN` under a typmod, the infinities and
/// PostgreSQL's greatest timestamp, `24:00:00`. `t_numeric.v_untyped`'s `NaN`
/// is text, which holds it, and `t_time.v_timetz`'s `24:00:00+00` is too.
#[tokio::test]
async fn the_types_fixture_counts_what_its_typed_columns_cannot_hold() {
    for version in VERSIONS {
        let dir = tempfile::tempdir().unwrap();
        let index =
            parsed(&types_fixture(version, "default"), dir.path(), &StatisticsRequest::DATA).await;
        let found: BTreeMap<String, Unrepresentable> =
            counts(&index).into_iter().filter(|(key, _)| !key.starts_with("t_extremes")).collect();
        let expected = BTreeMap::from([
            ("t_numeric.v_small".to_string(), count(1, 0)),
            ("t_date.v_date".to_string(), count(2, 0)),
            ("t_timestamp.v_ts".to_string(), count(3, 0)),
            ("t_timestamp.v_tstz".to_string(), count(3, 0)),
            ("t_time.v_time".to_string(), count(1, 0)),
        ]);
        assert_eq!(found, expected, "pg_dump {version}");
    }
}

/// **A leaf is its own declared type's**: the composite whose `text` field
/// reads `infinity` beside a finite `date` counts nothing, and each nested
/// value holding one counts once, whatever else it holds.
#[tokio::test]
async fn a_nested_value_counts_once_by_its_leaves_own_types() {
    for version in VERSIONS {
        let dir = tempfile::tempdir().unwrap();
        let index =
            parsed(&types_fixture(version, "default"), dir.path(), &StatisticsRequest::DATA).await;
        let found = counts(&index);
        for column in ["v_date_array", "v_daterange", "v_dated", "v_interval_array"] {
            let key = format!("t_extremes_nested.{column}");
            assert_eq!(found.get(&key), Some(&count(1, 0)), "pg_dump {version}: {key}");
        }
    }
}

/// **The metadata level counts nothing, and a data-level `parse` over it
/// counts what a cold one does**: the back-fill re-reads each block for its
/// census, and the count rides the same read.
#[tokio::test]
async fn a_data_level_parse_over_the_metadata_level_counts_what_a_cold_one_does() {
    let fixture = types_fixture(16, "default");
    let cold = tempfile::tempdir().unwrap();
    let cold = parsed(&fixture, cold.path(), &StatisticsRequest::DATA).await;

    let dir = tempfile::tempdir().unwrap();
    let metadata = parsed(&fixture, dir.path(), &StatisticsRequest::METADATA).await;
    assert!(metadata.blocks().all(|block| block.unrepresentable.is_none()));
    let backfilled = parsed(&fixture, dir.path(), &StatisticsRequest::DATA).await;
    assert_eq!(counts(&backfilled), counts(&cold));
    assert!(!counts(&cold).is_empty());
}

/// `table`'s one block in `index`.
fn table_block<'a>(index: &'a DumpIndex, table: &str) -> &'a CopyBlock {
    index.blocks_for(table).next().unwrap_or_else(|| panic!("no block for {table}"))
}

/// **A block's groups count what the block does, and its bounds keep a view
/// apart exactly where a group holds a value the view reads otherwise**:
/// every value's where one is past Arrow's format spec, the displayable
/// where one is within it and past the calendar.
#[tokio::test]
async fn statistics_count_by_group_what_the_census_counts_by_block() {
    for version in VERSIONS {
        let dir = tempfile::tempdir().unwrap();
        let index =
            parsed(&types_fixture(version, "default"), dir.path(), &StatisticsRequest::DATA).await;
        let mut viewed = 0;
        for block in index.blocks() {
            let census = block.unrepresentable.as_deref().unwrap();
            let statistics = block.statistics.as_deref().unwrap();
            for (c, column) in statistics.columns.iter().enumerate() {
                let Some(column) = column else { continue };
                let what = format!(
                    "pg_dump {version}: {}.{}",
                    block.header.table, block.header.columns[c]
                );
                let mut summed = Unrepresentable::default();
                for count in column.unrepresentable.iter().flatten() {
                    summed.merge(count);
                }
                assert_eq!(summed, census[c], "{what}");
                assert_eq!(
                    column.unrepresentable.is_some(),
                    !census[c].is_zero(),
                    "{what}: a count kept only where one is not zero"
                );
                for bounds in [&column.bounds, &column.datafusion_bounds].into_iter().flatten() {
                    assert_eq!(bounds.every.is_some(), summed.format > 0, "{what}");
                    assert_eq!(bounds.displayable.is_some(), summed.engine > 0, "{what}");
                    viewed += usize::from(bounds.every.is_some() || bounds.displayable.is_some());
                }
            }
        }
        assert!(viewed >= 10, "pg_dump {version}: only {viewed} sets kept a view apart");
    }
}

/// **Each view bounds the values it takes**: `t_date`'s infinities are its
/// every-value extremes and NULLs in the representable view; `t_extremes`'
/// greatest `date` is representable and past the calendar, and its every-value
/// timestamp bounds are its infinities, a value past what `i64` counts from
/// 1970 keying; its intervals past Arrow's nanoseconds key, and keep theirs.
#[tokio::test]
async fn each_view_of_the_types_fixture_bounds_the_values_it_takes() {
    use StatisticsView::{Displayable, Every, Representable};
    for version in VERSIONS {
        let dir = tempfile::tempdir().unwrap();
        let index =
            parsed(&types_fixture(version, "default"), dir.path(), &StatisticsRequest::DATA).await;
        let column = |table: &str, name: &str| {
            let block = table_block(&index, table);
            let at = block.header.columns.iter().position(|c| c == name).unwrap();
            let statistics = block.statistics.as_deref().unwrap();
            assert_eq!(statistics.groups.len(), 1, "{table}: one group");
            statistics.columns[at].clone().unwrap()
        };
        let extremes = |column: &pgdump_query::ColumnStatistics, view| {
            column.group_bounds(BoundsSet::Primary, view, 0).map(|b| (b.min.clone(), b.max.clone()))
        };
        let pair = |min: &str, max: &str| Some((min.to_string(), max.to_string()));
        let what = format!("pg_dump {version}");

        let date = column("t_date", "v_date");
        assert_eq!(extremes(&date, Every), pair("-infinity", "infinity"), "{what}");
        assert_eq!(extremes(&date, Representable), pair("0044-01-01 BC", "10000-01-01"), "{what}");
        assert_eq!(extremes(&date, Displayable), extremes(&date, Representable), "{what}");
        assert_eq!(date.null_count(0, Every), Some(1), "{what}");
        assert_eq!(date.null_count(0, Representable), Some(3), "{what}");

        let date = column("t_extremes", "v_date");
        assert_eq!(extremes(&date, Every), pair("-infinity", "infinity"), "{what}");
        assert_eq!(
            extremes(&date, Representable),
            pair("4714-11-24 BC", "5874897-12-31"),
            "{what}"
        );
        assert_eq!(extremes(&date, Displayable), pair("4714-11-24 BC", "262142-12-31"), "{what}");

        // PostgreSQL's greatest timestamp is past what `i64` counts from 1970
        // and keys all the same, counted from PostgreSQL's epoch (I49).
        let ts = column("t_extremes", "v_ts");
        assert_eq!(extremes(&ts, Every), pair("-infinity", "infinity"), "{what}");
        let tstz = column("t_extremes", "v_tstz");
        assert_eq!(extremes(&tstz, Every), pair("-infinity", "infinity"), "{what}");
        assert_eq!(
            extremes(&ts, Representable),
            pair("4714-11-24 00:00:00 BC", "294247-01-10 04:00:54.775807"),
            "{what}"
        );
        assert_eq!(
            extremes(&ts, Displayable),
            pair("4714-11-24 00:00:00 BC", "262142-12-31 23:59:59.999999"),
            "{what}"
        );

        // A time part past Arrow's nanoseconds keys in both orders, the span
        // and the fields being 128 bits, so every value's view keeps bounds.
        let interval = column("t_extremes", "v_interval");
        for set in [BoundsSet::Primary, BoundsSet::DataFusion] {
            let every = interval.group_bounds(set, Every, 0);
            assert!(every.is_some(), "{what}: {set:?}'s every-value interval bounds");
        }
    }
}
