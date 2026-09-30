//! The unrepresentable count a data-level mapping pass takes beside the
//! census (`docs/design/decisions.md`, "D96").
//!
//! `src/unrepresentable.rs`'s unit tests pin what one field is. These pin
//! what only a real dump shows: that `fixtures/*/types/default.sql`'s blocks
//! count the values their typed columns cannot hold, by column and tier, and
//! nothing else — a `text` field reading `infinity` and a bare `numeric`'s
//! `NaN` among the nothing — that the metadata level counts none and a
//! later data-level `parse` counts what a cold one does. Which values those
//! are is held to DataFusion's own path by `datafusion-pgdump`'s
//! `every_extreme_is_held_by_arrow_or_recorded`.

use std::collections::BTreeMap;
use std::path::Path;

use pgdump_query::cache::{self, CacheMode};
use pgdump_query::{
    DumpIndex, LocalFileSource, ScanOptions, StatisticsRequest, Unrepresentable, map_file,
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
