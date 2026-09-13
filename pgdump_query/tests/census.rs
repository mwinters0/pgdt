//! The array-shape census a full scan records on every `COPY` block
//! (`docs/design/decisions.md`, "D35").
//!
//! `src/index.rs`'s unit tests pin what a single field contributes. These
//! pin the three things only a real dump can show: that the shapes recorded
//! for `fixtures/*/types/default.sql` are the ones its literals actually
//! carry, that every mapping pass records them whatever its
//! `ScanExtent`, and that a query's schema is retyped from them before its
//! first batch.

use std::path::Path;

use futures::StreamExt;

use pgdump_query::cache::{CacheLoad, CacheMode};
use pgdump_query::index::ArrayShape;
use pgdump_query::resolve::ColumnResolution;
use pgdump_query::{
    LocalFileSource, QueryOptions, ScanExtent, ScanOptions, build_index, table_stream, union_census,
};

mod common;
use common::{VERSIONS, types_fixture};

/// The census recorded for `table`'s block, by column name.
async fn census_of(path: &Path, table: &str) -> Vec<(String, ArrayShape)> {
    let source = LocalFileSource::open(path).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    let block = index.blocks_for(table).next().unwrap_or_else(|| panic!("no block for {table}"));
    block.header.columns.iter().cloned().zip(block.array_shapes.iter().copied()).collect()
}

fn shape(census: &[(String, ArrayShape)], column: &str) -> ArrayShape {
    census.iter().find(|(name, _)| name == column).unwrap_or_else(|| panic!("no column {column}")).1
}

/// `t_array_shape` holds one uniformly 2-D column, one that mixes 1-D and 2-D
/// across rows, and one carrying `[lb:ub]=` prefixes. It is an input to the
/// census, not to the decoder.
#[tokio::test]
async fn the_shape_fixture_records_the_dimensions_its_literals_carry() {
    for version in VERSIONS {
        let path = types_fixture(version, "default");
        let census = census_of(&path, "public.t_array_shape").await;

        // `{{1,2},{3,4}}`, `\N`, `{{5,6},{7,8}}` — uniformly two-dimensional,
        // so this column is the one a census can retype exactly.
        assert_eq!(shape(&census, "v_multidim").dims, Some((2, 2)), "v_multidim on {version}");

        // `{1,2}` then `{{1,2},{3,4}}`. The pair is what tells this apart
        // from the column above; with a maximum alone both read as 2-D.
        assert_eq!(shape(&census, "v_mixed_dim").dims, Some((1, 2)), "v_mixed_dim on {version}");

        // `[0:2]={7,8,9}` and `[-1:0]={10,11}` — uniformly 1-D, and
        // disqualified anyway: Arrow lists have nowhere to put an index
        // origin.
        let lbound = shape(&census, "v_lbound");
        assert!(lbound.lower_bound_prefix, "v_lbound on {version}");
        assert_eq!(lbound.dims, Some((1, 1)), "v_lbound on {version}");

        // A non-array column is censused too — it just never contributes.
        assert_eq!(shape(&census, "id"), ArrayShape::default(), "id on {version}");
    }
}

/// The quoting cases, out of a dump `pg_dump` actually wrote rather than a
/// literal typed into a test: `{}` beside a real value, and an element whose
/// *text* contains a brace.
#[tokio::test]
async fn quoted_elements_and_empty_arrays_do_not_disturb_the_count() {
    for version in VERSIONS {
        let path = types_fixture(version, "default");
        let census = census_of(&path, "public.t_array").await;

        // Row 1 is `{}`, row 2 is `{1,2,3}`: the empty array fits any depth
        // and leaves the answer to the row that has one.
        assert_eq!(shape(&census, "v_empty").dims, Some((1, 1)), "v_empty on {version}");

        // Holds `{"a,b","c{d}","e\"f","g\\h"}` — the element `c{d}` carries a
        // brace of its own and is quoted for it, so the run stays 1.
        assert_eq!(
            shape(&census, "v_text_special").dims,
            Some((1, 1)),
            "v_text_special on {version}"
        );

        // Entirely NULL in every row — the koji shape, and the reason a
        // census must distinguish "saw nothing" from "saw one dimension".
        assert_eq!(shape(&census, "v_null_array").dims, Some((1, 1)), "v_null_array on {version}");
    }
}

/// A composite value opens with `(`, not `{`, and its own text may contain
/// braces — `("a,b","{""x\\""y"",""p q"",NULL}")`. The census walks every
/// field of a row it has not typed, so it must decline this one on the
/// literal's shape alone.
#[tokio::test]
async fn a_composite_column_contributes_no_dimensionality() {
    let census = census_of(&types_fixture(16, "default"), "public.t_composite").await;
    assert_eq!(shape(&census, "v_point"), ArrayShape::default());
    assert_eq!(shape(&census, "v_tagged"), ArrayShape::default());
    // The array *of* composites is a real array, one dimension deep.
    assert_eq!(shape(&census, "v_points").dims, Some((1, 1)));
}

/// **Every mapping pass censuses, under either `ScanExtent`.** A cold query
/// that stops at its target already receives every row of every block it
/// maps, so the census rides a read that happens anyway — and the block it
/// leaves in the map is indistinguishable from the one a full scan leaves,
/// which is what lets a consumer read `array_shapes` without a second test
/// beside it.
#[tokio::test]
async fn every_mapping_pass_censuses_whatever_its_extent() {
    let path = types_fixture(16, "default");
    let source = LocalFileSource::open(&path).unwrap();

    for extent in [ScanExtent::UntilTargetSettled, ScanExtent::Full] {
        // A fresh cache per pass: the index a query leaves is only readable
        // through the cache it persisted, and a shared one would let the
        // first pass answer for the second.
        let dir = tempfile::tempdir().unwrap();
        let cache = CacheMode::Enabled(dir.path().join("dump.dqcache"));
        let options = QueryOptions { scan_extent: extent, ..Default::default() };
        let mut stream = table_stream(
            &source,
            "public.t_array",
            ScanOptions::default(),
            options,
            None,
            cache.clone(),
        );
        while let Some(batch) = stream.next().await {
            batch.unwrap();
        }
        let CacheLoad::Index(index) = cache.load(&source).await.unwrap() else {
            panic!("the query wrote a cache: {extent:?}")
        };
        let block = index.blocks_for("public.t_array").next().unwrap();
        assert_eq!(block.array_shapes.len(), block.header.columns.len(), "{extent:?}");
        // `v_empty` holds `{}` then `{1,2,3}`: a real shape, recorded by a
        // pass that stopped at this table as much as by one that ran to EOF.
        let census: Vec<(String, ArrayShape)> =
            block.header.columns.iter().cloned().zip(block.array_shapes.iter().copied()).collect();
        assert_eq!(shape(&census, "v_empty").dims, Some((1, 1)), "{extent:?}");
    }
}

/// **A streamed schema needs no completeness test.** `table_stream` fixes its
/// schema after mapping and before emitting, over exactly the blocks it will
/// replay — so a cold query that stopped at its target retypes as confidently
/// as a full scan, and the `FieldDecode` refusal a multi-dimensional value
/// would otherwise earn is unreachable for a top-level array column on either
/// path (`docs/design/decisions.md`, "D35").
#[tokio::test]
async fn a_cold_query_retypes_from_the_census_it_just_recorded() {
    let path = types_fixture(16, "default");
    let source = LocalFileSource::open(&path).unwrap();

    for extent in [ScanExtent::UntilTargetSettled, ScanExtent::Full] {
        let dir = tempfile::tempdir().unwrap();
        let cache = CacheMode::Enabled(dir.path().join("dump.dqcache"));
        let options = QueryOptions { scan_extent: extent, ..Default::default() };
        let mut stream = table_stream(
            &source,
            "public.t_array_shape",
            ScanOptions::default(),
            options,
            None,
            cache,
        );
        while let Some(batch) = stream.next().await {
            batch.unwrap_or_else(|e| panic!("{extent:?}: {e}"));
        }
        let resolved = stream.resolved_schema();
        assert_eq!(
            &resolved.columns[1..],
            &[
                ColumnResolution::Mapped,
                ColumnResolution::VaryingArrayShape,
                ColumnResolution::VaryingArrayShape,
            ],
            "{extent:?}"
        );
    }
}

/// The union a table spanning several blocks (I2) resolves against, read off
/// a real partitioned dump: `blocks_for` enumerates them and every one
/// carries a census, so the combined answer covers every row the table has.
#[tokio::test]
async fn a_partitioned_table_unions_the_censuses_of_all_its_blocks() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/16/partitions/default.sql");
    let source = LocalFileSource::open(&path).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();

    let blocks: Vec<_> = index.blocks_for("public.spread").collect();
    assert!(blocks.len() > 1, "`spread` is written as two blocks under one name (I2)");
    let union = union_census(blocks.iter().copied());
    assert_eq!(union.len(), blocks[0].header.columns.len());
    // No column in this fixture holds an array, so the union constrains
    // nothing — which is the answer that leaves every column optimistically
    // typed.
    assert!(union.iter().all(|s| *s == ArrayShape::default()));
}
