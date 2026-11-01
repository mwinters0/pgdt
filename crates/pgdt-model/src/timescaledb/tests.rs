//! The catalog read over hand-built blocks: detection, the version and its
//! era, each recipe chosen and two read, and a catalog short of a column. The
//! pins' own catalogs are `pgdt-mapping`'s `tests/timescaledb.rs`.

use std::sync::Arc;

use pgdt_core::CopyHeader;

use super::catalog::{Catalog, regclass_name};
use super::*;
use crate::{
    CopyBlock, DataBlock, DatabaseMetadata, DumpIndex, DumpMetadata, Extension, Span, SpanBody,
};

/// A catalog block of `table` naming `columns`, holding `rows`, each row's
/// fields tab-separated as `COPY` writes them.
fn block(table: &str, columns: &str, rows: &[&str]) -> Span {
    let header = CopyHeader {
        schema: Some(CATALOG_SCHEMA.to_owned()),
        table: table.to_owned(),
        columns: columns.split(", ").map(str::to_owned).collect(),
    };
    let recorded_rows = records_rows(&header).then(|| {
        let mut recorded = RecordedRows::default();
        for row in rows {
            recorded.observe_row(row.as_bytes());
        }
        Box::new(recorded)
    });
    let block = CopyBlock {
        header,
        header_offset: 0,
        data_offset: 0,
        terminator_offset: 0,
        end_offset: 0,
        row_count: rows.len() as u64,
        partition_root: None,
        statistics: None,
        statistics_declined: None,
        ignored_refusals: None,
        checked_in_full: false,
        census: None,
        recorded_rows,
        longest_line: None,
    };
    Span {
        start: 0,
        end: 0,
        database: None,
        text: None,
        toc: None,
        toc_owned: false,
        body: SpanBody::Data(DataBlock::Copy(Box::new(block))),
    }
}

fn index(spans: Vec<Span>, extension: bool) -> DumpIndex {
    let extensions = match extension {
        true => vec![Extension { name: EXTENSION.to_owned(), schema: Some("public".to_owned()) }],
        false => vec![],
    };
    let database = DatabaseMetadata {
        name: None,
        preamble_complete: true,
        server_version: None,
        pg_dump_version: None,
        extensions,
        types: vec![],
        collations: vec![],
        tables: Default::default(),
    };
    DumpIndex {
        spans,
        scanned_through: 0,
        metadata: Some(Arc::new(DumpMetadata { databases: vec![database] })),
        roles: Default::default(),
        tablespaces: Default::default(),
        diagnostics: vec![],
    }
}

const HYPERTABLE: &str = "id, schema_name, table_name, compression_state";
const DIMENSION: &str = "id, hypertable_id, column_name, num_slices";

/// 2.31's shape: `chunk.relid`, slices naming their chunk, compressed
/// relations through `compression_settings`, and the version row.
fn relid_catalog() -> Vec<Span> {
    vec![
        block("hypertable", HYPERTABLE, &["1\tpublic\tmetrics\t1"]),
        block("dimension", DIMENSION, &["1\t1\ttime\t\\N", "2\t1\tdevice\t2"]),
        block(
            "chunk",
            "id, relid, hypertable_id, status, osm_chunk, creation_time",
            &[
                "2\t_timescaledb_internal._hyper_1_2_chunk\t1\t9\tf\tx",
                "1\t\"Odd Schema\".\"A\"\"b\"\t1\t0\tf\tx",
            ],
        ),
        block(
            "dimension_slice",
            "id, chunk_id, dimension_id, range_start, range_end",
            &["1\t1\t1\t0\t10", "2\t1\t2\t-9223372036854775808\t1073741823", "3\t2\t1\t10\t20"],
        ),
        block(
            "compression_settings",
            "relid, compress_relid",
            &[
                "public.metrics\t\\N",
                "_timescaledb_internal._hyper_1_2_chunk\t_timescaledb_internal._hyper_1_2_chunk_compressed",
            ],
        ),
        block("metadata", "key, value", &["timescaledb_version\t2.31.0", "exported_uuid\tx"]),
    ]
}

fn resolved(db: &TimescaleDb) -> &Resolved {
    match &db.resolution {
        Resolution::Resolved(resolved) => resolved,
        other => panic!("not resolved: {other:?}"),
    }
}

#[test]
fn a_database_with_neither_the_extension_nor_its_catalog_is_not_timescaledb() {
    assert_eq!(TimescaleDb::of(&index(vec![], false), None), None);
}

#[test]
fn the_extension_alone_detects_it() {
    let db = TimescaleDb::of(&index(vec![], true), None).expect("detected");
    assert!(db.extension);
}

#[test]
fn the_catalog_alone_detects_it() {
    let db = TimescaleDb::of(&index(relid_catalog(), false), None).expect("detected");
    assert!(!db.extension);
}

#[test]
fn the_relid_recipe_joins_chunks_ranges_and_compressed_relations() {
    let db = TimescaleDb::of(&index(relid_catalog(), true), None).expect("detected");
    assert_eq!(db.version, Version::Stated { version: "2.31.0".to_owned() });
    let resolved = resolved(&db);
    assert_eq!(resolved.recipe, Recipe::Relid);
    let [metrics] = resolved.hypertables.as_slice() else { panic!("one hypertable") };
    assert_eq!(metrics.table, "public.metrics");
    assert_eq!(metrics.dimensions.iter().map(|d| d.hashed).collect::<Vec<_>>(), [false, true]);
    let [odd, compressed] = metrics.chunks.as_slice() else { panic!("two chunks") };
    assert_eq!(odd.table, "Odd Schema.A\"b");
    assert_eq!(odd.slices.len(), 2);
    assert!(!odd.compressed());
    assert_eq!(
        compressed.compressed_relation.as_deref(),
        Some("_timescaledb_internal._hyper_1_2_chunk_compressed")
    );
    assert!(compressed.compressed() && compressed.partially_compressed());
    assert_eq!(compressed.slices, [Slice { dimension: 1, start: 10, end: 20 }]);
}

#[test]
fn the_chunk_constraint_recipe_reads_dropped_rows_and_compressed_chunk_rows() {
    let spans = vec![
        block(
            "hypertable",
            "id, schema_name, table_name, compression_state, replication_factor",
            &["1\tpublic\tm\t1\t\\N", "2\t_timescaledb_internal\t_compressed_hypertable_2\t2\t\\N"],
        ),
        block("dimension", DIMENSION, &["1\t1\ttime\t\\N"]),
        block(
            "chunk",
            "id, hypertable_id, schema_name, table_name, compressed_chunk_id, dropped, status, osm_chunk, creation_time",
            &[
                "1\t1\t_timescaledb_internal\t_hyper_1_1_chunk\t\\N\tt\t0\tf\tx",
                "2\t1\t_timescaledb_internal\t_hyper_1_2_chunk\t3\tf\t1\tf\tx",
                "3\t2\t_timescaledb_internal\tcompress_hyper_2_3_chunk\t\\N\tf\t0\tf\tx",
            ],
        ),
        block("dimension_slice", "id, dimension_id, range_start, range_end", &["7\t1\t0\t10"]),
        block("chunk_constraint", "chunk_id, dimension_slice_id", &["2\t7", "2\t\\N"]),
        block("hypertable_compression", "hypertable_id", &[]),
        block("metadata", "key, value", &["exported_uuid\tx"]),
    ];
    let db = TimescaleDb::of(&index(spans, true), None).expect("detected");
    let era = Era {
        from: Release { major: 2, minor: 13 },
        through: Some(Release { major: 2, minor: 13 }),
    };
    assert_eq!(db.version, Version::Inferred { era });
    let resolved = resolved(&db);
    assert_eq!(resolved.recipe, Recipe::ChunkConstraint);
    let [m, internal] = resolved.hypertables.as_slice() else { panic!("two hypertables") };
    assert!(!m.internal && internal.internal);
    let [dropped, compressed] = m.chunks.as_slice() else { panic!("two chunks") };
    assert!(dropped.dropped);
    assert_eq!(
        compressed.compressed_relation.as_deref(),
        Some("_timescaledb_internal.compress_hyper_2_3_chunk")
    );
    assert_eq!(compressed.slices, [Slice { dimension: 1, start: 0, end: 10 }]);
}

#[test]
fn a_catalog_missing_a_column_its_recipe_reads_is_unreadable() {
    let mut spans = relid_catalog();
    spans[3] = block("dimension_slice", "id, dimension_id, range_start, range_end", &[]);
    let db = TimescaleDb::of(&index(spans, true), None).expect("detected");
    assert_eq!(
        db.resolution,
        Resolution::Unreadable {
            recipe: Recipe::Relid,
            missing: vec!["dimension_slice.chunk_id".to_owned()],
            malformed: vec![],
            hypertables: Some(vec!["public.metrics".to_owned()]),
        }
    );
}

/// Where even a hypertable's name does not read, an unreadable catalog names
/// none.
#[test]
fn an_unreadable_catalog_names_its_hypertables_where_their_names_read() {
    let mut spans = relid_catalog();
    spans[0] = block("hypertable", "id, schema_name, compression_state", &["1\tpublic\t1"]);
    let db = TimescaleDb::of(&index(spans, true), None).expect("detected");
    let Resolution::Unreadable { hypertables, .. } = db.resolution else { panic!("readable") };
    assert_eq!(hypertables, None);
}

#[test]
fn a_field_that_does_not_read_as_its_type_is_unreadable() {
    let mut spans = relid_catalog();
    spans[0] = block("hypertable", HYPERTABLE, &["one\tpublic\tmetrics\t1"]);
    let db = TimescaleDb::of(&index(spans, true), None).expect("detected");
    let Resolution::Unreadable { malformed, .. } = db.resolution else { panic!("readable") };
    assert!(malformed.contains(&"hypertable.id".to_owned()), "{malformed:?}");
}

#[test]
fn without_a_version_row_the_shape_chooses_the_recipe() {
    let mut spans = relid_catalog();
    spans.pop();
    let db = TimescaleDb::of(&index(spans, true), None).expect("detected");
    let era = Era { from: Release { major: 2, minor: 29 }, through: None };
    assert_eq!(db.version, Version::Inferred { era });
    assert_eq!(resolved(&db).recipe, Recipe::Relid);
}

#[test]
fn columns_no_release_holds_together_are_an_unknown_version() {
    let spans =
        vec![block("chunk", "id, relid, dropped", &[]), block("metadata", "key, value", &[])];
    let db = TimescaleDb::of(&index(spans, true), None).expect("detected");
    assert_eq!(db.version, Version::Unknown);
}

#[test]
fn a_stated_release_decides_the_recipe() {
    let catalog = Catalog { extension: true, tables: Default::default() };
    let stated = |v: &str| version::recipe(&Version::Stated { version: v.to_owned() }, &catalog);
    assert_eq!(stated("2.27.2"), Recipe::ChunkConstraint);
    assert_eq!(stated("2.28.0-dev"), Recipe::SliceChunkId);
    assert_eq!(stated("2.29.0"), Recipe::Relid);
    assert_eq!(stated("3.1"), Recipe::Relid);
}

#[test]
fn a_recorded_row_undoes_copy_escapes() {
    let mut recorded = RecordedRows::default();
    recorded.observe_row(b"a\\tb\t\\N\tc\\\\d");
    assert_eq!(recorded.rows, [vec![Some("a\tb".to_owned()), None, Some("c\\d".to_owned())]]);
}

#[test]
fn a_regclass_reads_as_its_qualified_name() {
    assert_eq!(regclass_name("public.t").as_deref(), Some("public.t"));
    assert_eq!(regclass_name("\"S\".\"x.y\"").as_deref(), Some("S.x.y"));
    assert_eq!(regclass_name("t"), None);
    assert_eq!(regclass_name("a.b.c"), None);
}

#[test]
fn only_the_catalog_tables_a_conclusion_reads_are_recorded() {
    let header = |schema: &str, table: &str| CopyHeader {
        schema: Some(schema.to_owned()),
        table: table.to_owned(),
        columns: vec![],
    };
    assert!(records_rows(&header(CATALOG_SCHEMA, "chunk")));
    assert!(!records_rows(&header(CATALOG_SCHEMA, "bgw_job")));
    assert!(!records_rows(&header("public", "chunk")));
}
