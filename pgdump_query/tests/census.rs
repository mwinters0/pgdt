//! The array-shape census a full scan records on every `COPY` block
//! (`docs/design/architecture.md`, "The array shape census").
//!
//! `src/index.rs`'s unit tests pin what a single field contributes. These
//! pin the two things only a real dump can show: that the shapes recorded
//! for `fixtures/*/types/default.sql` are the ones its literals actually
//! carry, and that a census exists exactly where a scan reached EOF.

use std::path::{Path, PathBuf};

use futures::StreamExt;

use pgdump_query::cache::CacheMode;
use pgdump_query::index::ArrayShape;
use pgdump_query::{
    BatchOptions, LocalFileSource, ScanExtent, ScanOptions, build_index, table_stream,
};

fn types_fixture(version: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(version)
        .join("types/default.sql")
}

/// Every major we generate fixtures for. The census reads a literal, not a
/// catalog, and `array_out`'s output shape is identical across all of them
/// (I25) — running the same assertions on each is what says so.
const VERSIONS: [&str; 6] = ["13", "14", "15", "16", "17", "18"];

/// The census recorded for `table`'s block, by column name.
async fn census_of(path: &Path, table: &str) -> Vec<(String, ArrayShape)> {
    let source = LocalFileSource::open(path).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    let block = index.blocks_for(table).next().unwrap_or_else(|| panic!("no block for {table}"));
    let shapes = block.array_shapes.as_ref().expect("a full scan censuses every block it maps");
    block.header.columns.iter().cloned().zip(shapes.iter().copied()).collect()
}

fn shape(census: &[(String, ArrayShape)], column: &str) -> ArrayShape {
    census.iter().find(|(name, _)| name == column).unwrap_or_else(|| panic!("no column {column}")).1
}

/// `t_array_shape` is the fixture column set 4.1 added for exactly this
/// slice — one uniformly 2-D column, one that mixes 1-D and 2-D across rows,
/// and one carrying `[lb:ub]=` prefixes. It is an input to the census, not to
/// the decoder.
#[tokio::test]
async fn the_shape_fixture_records_the_dimensions_its_literals_carry() {
    for version in VERSIONS {
        let path = types_fixture(version);
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
        let path = types_fixture(version);
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
    let census = census_of(&types_fixture("16"), "public.t_composite").await;
    assert_eq!(shape(&census, "v_point"), ArrayShape::default());
    assert_eq!(shape(&census, "v_tagged"), ArrayShape::default());
    // The array *of* composites is a real array, one dimension deep.
    assert_eq!(shape(&census, "v_points").dims, Some((1, 1)));
}

/// **The two paths, as a property of the block rather than of the file.**
/// Only a scan that will reach EOF censuses, so a cold query leaves the
/// blocks it maps with no census at all — which is not the same answer as a
/// census that saw nothing, and is why the field is an `Option`.
///
/// `t_array` rather than `t_array_shape`: the latter is the column set the
/// optimistic path deliberately refuses, so streaming it is a `FieldDecode`
/// error until a census is consumed, which is 4.5.1's.
#[tokio::test]
async fn only_a_scan_that_reaches_eof_censuses() {
    let path = types_fixture("16");
    let source = LocalFileSource::open(&path).unwrap();

    for (extent, censused) in [(ScanExtent::UntilTargetSettled, false), (ScanExtent::Full, true)] {
        // A fresh cache per pass: the index a query leaves is only readable
        // through the cache it persisted, and a shared one would let the
        // first pass answer for the second.
        let dir = tempfile::tempdir().unwrap();
        let cache = CacheMode::Enabled(dir.path().join("dump.dqcache"));
        let options = BatchOptions { scan_extent: extent, ..Default::default() };
        let mut stream = table_stream(
            &source,
            "public.t_array",
            ScanOptions::default(),
            options,
            None,
            None,
            cache.clone(),
        );
        while let Some(batch) = stream.next().await {
            batch.unwrap();
        }
        let index = cache.load(&source).await.unwrap().unwrap();
        let block = index.blocks_for("public.t_array").next().unwrap();
        assert_eq!(block.array_shapes.is_some(), censused, "{extent:?}");
    }
}
