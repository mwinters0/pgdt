//! The array-shape census a full scan records on every `COPY` block
//! (`docs/design/decisions.md`, "D35").
//!
//! `src/index.rs`'s unit tests pin what a single field contributes. These
//! pin the four things only a real dump can show: that the shapes recorded
//! for `fixtures/*/types/default.sql` are the ones its literals actually
//! carry, that every mapping pass a query makes records them whatever its
//! `ScanExtent`, that a query's schema is retyped from them before its
//! first batch, and that a table mapped at the metadata level, which records
//! none, is censused by the query that reads it.

use std::path::Path;
use std::sync::Arc;

use futures::StreamExt;

use pgdump_query::cache::{self, CacheLoad, CacheMode, SourceWatch, StrictIdentity};
use pgdump_query::index::ArrayShape;
use pgdump_query::resolve::{ColumnResolution, SchemaMode};
use pgdump_query::{
    ByteRangeSource, Error, LocalFileSource, QueryOptions, ScanExtent, ScanOptions,
    StatisticsRequest, TablePartitions, build_index, map_file, table_schema, table_stream,
    union_census,
};

mod common;
use common::{VERSIONS, types_fixture};

/// The census recorded for `table`'s block, by column name.
async fn census_of(path: &Path, table: &str) -> Vec<(String, ArrayShape)> {
    let source = LocalFileSource::open(path).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    let block = index.blocks_for(table).next().unwrap_or_else(|| panic!("no block for {table}"));
    block.header.columns.iter().cloned().zip(block.array_shapes.iter().flatten().copied()).collect()
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

/// **Every mapping pass a query makes censuses, under either `ScanExtent`.** A cold query
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
        let cache = CacheMode::enabled(dir.path().join("dump.dtcache"));
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
        assert_eq!(
            block.array_shapes.as_ref().map(Vec::len),
            Some(block.header.columns.len()),
            "{extent:?}"
        );
        // `v_empty` holds `{}` then `{1,2,3}`: a real shape, recorded by a
        // pass that stopped at this table as much as by one that ran to EOF.
        let census: Vec<(String, ArrayShape)> = block
            .header
            .columns
            .iter()
            .cloned()
            .zip(block.array_shapes.iter().flatten().copied())
            .collect();
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
        let cache = CacheMode::enabled(dir.path().join("dump.dtcache"));
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
    let union = union_census(&blocks[0].header.columns, blocks.iter().copied());
    assert_eq!(union.len(), blocks[0].header.columns.len());
    // No column in this fixture holds an array, so the union constrains
    // nothing — which is the answer that leaves every column optimistically
    // typed.
    assert!(union.iter().all(|s| *s == ArrayShape::default()));
}

/// **A table the cache holds at the metadata level is censused by the query
/// that reads it, for that query alone, and refused by a plan over a map the
/// caller holds** (`docs/design/decisions.md`, "D35"). `parse` at the
/// metadata level records no block's census; a query of `t_array_shape` reads
/// its rows once more and retypes as a data-level map would, writing nothing,
/// where a census-less union would leave every array column optimistically
/// typed and the two-dimensional values refusing where a read reached them.
/// `SchemaMode::Strings` reads no census, and is planned over either.
#[tokio::test]
async fn a_metadata_level_table_is_censused_by_its_query_and_refused_by_a_held_plan() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("dump.sql");
    std::fs::copy(types_fixture(16, "default"), &dump).unwrap();
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&dump).unwrap());
    let path = cache::colocated_path(&dump);
    let mode = CacheMode::enabled(path.clone());
    let metadata = StatisticsRequest::METADATA;
    let run = map_file(source.as_ref(), &ScanOptions::default(), &mode, &metadata).await.unwrap();
    assert!(run.index.blocks().all(|block| block.array_shapes.is_none()));
    let written = std::fs::read(&path).unwrap();

    let mut stream = table_stream(
        source.as_ref(),
        "public.t_array_shape",
        ScanOptions::default(),
        QueryOptions::default(),
        None,
        mode.clone(),
    );
    while let Some(batch) = stream.next().await {
        batch.unwrap();
    }
    assert_eq!(
        &stream.resolved_schema().columns[1..],
        &[
            ColumnResolution::Mapped,
            ColumnResolution::VaryingArrayShape,
            ColumnResolution::VaryingArrayShape,
        ]
    );
    drop(stream);
    assert_eq!(std::fs::read(&path).unwrap(), written, "the query wrote nothing");

    let table = run.index.tables().into_iter().find(|t| t.qualified() == "public.t_array_shape");
    let table = table.expect("the map holds the table");
    let refused = |result: pgdump_query::Result<_>| matches!(result, Err(Error::TableAtMetadataLevel { table }) if table == "public.t_array_shape");
    assert!(refused(table_schema(&run.index, &table, &QueryOptions::default()).map(|_| ())));
    let watch =
        Arc::new(SourceWatch::open(source.as_ref(), StrictIdentity::ADVISORY).await.unwrap());
    let plan = |options: QueryOptions| {
        TablePartitions::plan(
            Arc::clone(&source),
            &run.index,
            Arc::clone(&watch),
            &table,
            ScanOptions::default(),
            options,
        )
    };
    assert!(refused(plan(QueryOptions::default()).await.map(|_| ())));
    let strings = QueryOptions { schema_mode: SchemaMode::Strings, ..QueryOptions::default() };
    table_schema(&run.index, &table, &strings).unwrap();
    plan(strings).await.unwrap();
}
