//! TimescaleDB's catalog recorded by the mapping pass and read by its recipe
//! (`pgdt_model::timescaledb`), at each pin, held to what the fixture's own
//! DDL says: each chunk an inheritance child of its hypertable (TS1), each
//! compressed chunk's relation a table the dump holds (TS4, TS5), each
//! dropped row naming no table (TS6), each aggregate's materialization a
//! hypertable (TS7) — and the release each pin states or its catalog bounds
//! (TS3), the catalog found with the extension (TS2) or without it; the
//! rows each hypertable's root holds as its restore holds them, its own
//! block's (TS8); and the findings the map carries of both, and of the chunks
//! a catalog lists that the file lacks (TS10).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use pgdt_dump::cache::{self, CacheMode, Held};
use pgdt_dump::{ReadOptions, ScanOptions};
use pgdt_io::{Allowance, Arrangement, ArrangementRequest, ByteRangeSource, Jobs, LocalFileSource};
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_mapping::{build_index, map_file};
use pgdt_model::timescaledb::{Era, Recipe, Release, Resolution, Resolved, TimescaleDb, Version};
use pgdt_model::{DatabaseMetadata, DiagnosticKind, DumpIndex, PostgresInvalidValues};
use tokio_util::sync::CancellationToken;

use pgdt_fixtures::{Answer, answers, sandboxed, timescaledb_fixture};

/// Each pin's major, the release its catalog states — `None` before the
/// version row (2.14) — and the recipe that release reads by.
const PINS: [(u32, Option<&str>, Recipe); 5] = [
    (13, None, Recipe::ChunkConstraint),
    (15, Some("2.21.4"), Recipe::ChunkConstraint),
    (16, Some("2.27.2"), Recipe::ChunkConstraint),
    (17, Some("2.28.3"), Recipe::SliceChunkId),
    (18, Some("2.31.0"), Recipe::Relid),
];

async fn index_of(version: u32, flag_set: &str) -> DumpIndex {
    let source = LocalFileSource::open(timescaledb_fixture(version, flag_set)).unwrap();
    build_index(&source, &ScanOptions::default()).await.unwrap()
}

fn ddl(index: &DumpIndex) -> &DatabaseMetadata {
    &index.metadata.as_ref().expect("a whole map states its metadata").databases[0]
}

fn resolved(db: &TimescaleDb) -> &Resolved {
    match &db.resolution {
        Resolution::Resolved(resolved) => resolved,
        other => panic!("the catalog does not read: {other:?}"),
    }
}

#[tokio::test]
async fn each_pin_states_or_bounds_its_release_and_reads_by_its_recipe() {
    for (version, stated, recipe) in PINS {
        let index = index_of(version, "default").await;
        let db = TimescaleDb::of(&index, None).expect("a TimescaleDB database");
        assert!(db.extension, "{version}");
        match stated {
            Some(stated) => {
                assert_eq!(db.version, Version::Stated { version: stated.to_owned() }, "{version}")
            }
            None => {
                let pin = Release { major: 2, minor: 13 };
                let era = Era { from: pin, through: Some(pin) };
                assert_eq!(db.version, Version::Inferred { era }, "{version}");
            }
        }
        assert_eq!(resolved(&db).recipe, recipe, "{version}");
    }
}

#[tokio::test]
async fn each_pin_s_chunks_are_its_hypertables_inheritance_children() {
    for (version, ..) in PINS {
        let index = index_of(version, "default").await;
        let ddl = ddl(&index);
        let db = TimescaleDb::of(&index, None).unwrap();
        let mut chunks = 0;
        for hypertable in resolved(&db).hypertables.iter().filter(|h| !h.internal) {
            let mut listed: Vec<&str> =
                hypertable.chunks.iter().filter(|c| !c.dropped).map(|c| c.table.as_str()).collect();
            listed.sort_unstable();
            let mut children: Vec<&str> = ddl
                .tables
                .iter()
                .filter(|(_, def)| def.parents.contains(&hypertable.table))
                .map(|(name, _)| name.as_str())
                .collect();
            children.sort_unstable();
            assert_eq!(listed, children, "{version}: {}", hypertable.table);
            for chunk in hypertable.chunks.iter().filter(|c| !c.dropped) {
                assert_eq!(
                    chunk.slices.len(),
                    hypertable.dimensions.len(),
                    "{version}: {} has a range in each dimension",
                    chunk.table
                );
            }
            chunks += listed.len();
        }
        assert!(chunks > 0, "{version}: no chunk found");
    }
}

#[tokio::test]
async fn each_compressed_chunk_names_a_relation_the_dump_holds() {
    for (version, ..) in PINS {
        let index = index_of(version, "default").await;
        let ddl = ddl(&index);
        let blocks: Vec<String> = index.blocks().map(|b| b.header.qualified_name()).collect();
        let db = TimescaleDb::of(&index, None).unwrap();
        let mut compressed = 0;
        for chunk in resolved(&db).hypertables.iter().flat_map(|h| &h.chunks) {
            assert_eq!(
                chunk.compressed(),
                chunk.compressed_relation.is_some(),
                "{version}: {chunk:?}"
            );
            assert!(!chunk.partially_compressed() || chunk.compressed(), "{version}: {chunk:?}");
            let Some(relation) = &chunk.compressed_relation else { continue };
            assert!(ddl.tables.contains_key(relation), "{version}: {relation} declared");
            assert!(blocks.contains(relation), "{version}: {relation} has a block");
            compressed += 1;
        }
        assert!(compressed >= 4, "{version}: {compressed} compressed chunks");
    }
}

#[tokio::test]
async fn a_dropped_row_names_no_table() {
    let index = index_of(13, "default").await;
    let db = TimescaleDb::of(&index, None).unwrap();
    let dropped: Vec<&str> = resolved(&db)
        .hypertables
        .iter()
        .flat_map(|h| &h.chunks)
        .filter(|c| c.dropped)
        .map(|c| c.table.as_str())
        .collect();
    assert_eq!(dropped.len(), 2, "{dropped:?}");
    for table in dropped {
        assert!(!ddl(&index).tables.contains_key(table), "{table}");
    }
}

#[tokio::test]
async fn each_continuous_aggregate_names_its_materialization_hypertable() {
    for (version, ..) in PINS {
        let index = index_of(version, "default").await;
        let db = TimescaleDb::of(&index, None).unwrap();
        let resolved = resolved(&db);
        let aggregates = resolved.continuous_aggregates.as_ref().expect("readable");
        let views: Vec<&str> = aggregates.iter().map(|a| a.view.as_str()).collect();
        assert_eq!(views, ["public.metrics_daily", "public.metrics_weekly"], "{version}");
        for aggregate in aggregates {
            assert_eq!(aggregate.raw.as_deref(), Some("public.metrics"), "{version}");
            let materialization = aggregate.materialization.as_deref().expect("listed");
            assert!(
                resolved.hypertables.iter().any(|h| h.table == materialization && !h.internal),
                "{version}: {materialization}"
            );
        }
    }
}

#[tokio::test]
async fn a_dump_excluding_the_chunks_lists_them_in_its_catalog() {
    for version in [13, 18] {
        let index = index_of(version, "exclude-chunks").await;
        let db = TimescaleDb::of(&index, None).expect("the catalog is dumped");
        let metrics = resolved(&db).hypertables.iter().find(|h| h.table == "public.metrics");
        let chunks = &metrics.expect("public.metrics").chunks;
        assert!(!chunks.is_empty(), "{version}");
        for chunk in chunks {
            assert!(!ddl(&index).tables.contains_key(&chunk.table), "{version}: {}", chunk.table);
        }
    }
}

#[tokio::test]
async fn a_schema_dump_holds_nothing_to_detect() {
    for version in [13, 18] {
        let index = index_of(version, "schema-public").await;
        assert_eq!(TimescaleDb::of(&index, None), None, "{version}");
    }
}

#[tokio::test]
async fn a_tables_dump_is_detected_by_its_catalog_alone() {
    for version in [13, 18] {
        let index = index_of(version, "tables").await;
        let db = TimescaleDb::of(&index, None).expect("the catalog is selected");
        assert!(!db.extension, "{version}");
        let default = TimescaleDb::of(&index_of(version, "default").await, None).unwrap();
        assert_eq!(db.version, default.version, "{version}");
        assert_eq!(db.resolution, default.resolution, "{version}");
    }
}

/// Every TimescaleDB fixture's flag sets, at the majors each runs at.
const FIXTURES: [(u32, &str); 11] = [
    (13, "default"),
    (13, "exclude-chunks"),
    (13, "tables"),
    (15, "default"),
    (16, "default"),
    (17, "default"),
    (18, "default"),
    (18, "exclude-chunks"),
    (18, "tables"),
    (13, "schema-public"),
    (18, "schema-public"),
];

/// **A root's rows as its restore holds them are its own block's**: the
/// answers read one for each hypertable the catalog lists and the DDL
/// declares, and each is exactly the rows of the block naming the root —
/// the stray row from 2.27 included, which the server's own reads leave out
/// (TS8). A dump holding no catalog answers none.
#[tokio::test]
async fn each_root_holds_its_own_block_s_rows_as_restored() {
    for (version, flag_set) in FIXTURES {
        let at = format!("{version} {flag_set}");
        let index = index_of(version, flag_set).await;
        let held = answers(&timescaledb_fixture(version, flag_set)).held;
        let Some(db) = TimescaleDb::of(&index, None) else {
            assert!(held.is_empty(), "{at}: {held:?}");
            continue;
        };
        let roots: BTreeSet<&str> = resolved(&db)
            .hypertables
            .iter()
            .map(|h| h.table.as_str())
            .filter(|table| ddl(&index).tables.contains_key(*table))
            .collect();
        assert_eq!(held.keys().map(String::as_str).collect::<BTreeSet<_>>(), roots, "{at}");
        for (root, answer) in &held {
            let Answer::Rows(rows) = answer else { panic!("{at}: {root} {answer:?}") };
            let own: u64 = index
                .blocks()
                .filter(|b| b.header.qualified_name() == *root)
                .map(|b| b.row_count)
                .sum();
            assert_eq!(rows.len() as u64, own, "{at}: {root}");
        }
        let stray = held.get("public.metrics").map(|answer| match answer {
            Answer::Rows(rows) => rows.len(),
            Answer::Error(_) => unreachable!(),
        });
        assert_eq!(stray, Some(usize::from(version >= 16)), "{at}: the stray root row");
    }
}

/// The map's TimescaleDB findings: each root's own rows, and each
/// hypertable's absent chunks, by hypertable; every finding of a plain dump's
/// database, which names none.
fn findings(index: &DumpIndex) -> (BTreeMap<String, u64>, BTreeMap<String, Vec<String>>) {
    let (mut rows, mut absent) = (BTreeMap::new(), BTreeMap::new());
    for diagnostic in &index.diagnostics {
        match diagnostic.kind() {
            DiagnosticKind::TimescaleRootRows { database: None, hypertable, rows: n } => {
                assert!(rows.insert(hypertable.clone(), *n).is_none(), "{hypertable}");
            }
            DiagnosticKind::TimescaleChunksAbsent { database: None, hypertable, chunks } => {
                assert!(
                    absent.insert(hypertable.clone(), chunks.clone()).is_none(),
                    "{hypertable}"
                );
            }
            DiagnosticKind::TimescaleCatalogUnreadable { .. } => panic!("{diagnostic:?}"),
            _ => {}
        }
    }
    (rows, absent)
}

/// **The map names each root holding rows of its own, and each chunk its
/// catalog lists that the file lacks**: a root's finding exactly where its
/// restore holds rows in it, with their count (TS8); a hypertable's absent
/// chunks every one it lists in a dump excluding the chunks' schema, and
/// none where the file holds them all (TS10).
#[tokio::test]
async fn the_map_names_each_root_s_rows_and_each_absent_chunk() {
    for (version, flag_set) in FIXTURES {
        let at = format!("{version} {flag_set}");
        let index = index_of(version, flag_set).await;
        let held = answers(&timescaledb_fixture(version, flag_set)).held;
        let (rows, absent) = findings(&index);
        let restored: BTreeMap<String, u64> = held
            .iter()
            .filter_map(|(root, answer)| match answer {
                Answer::Rows(rows) if !rows.is_empty() => Some((root.clone(), rows.len() as u64)),
                Answer::Rows(_) => None,
                Answer::Error(error) => panic!("{at}: {root} {error}"),
            })
            .collect();
        assert_eq!(rows, restored, "{at}");
        if flag_set != "exclude-chunks" {
            assert_eq!(absent, BTreeMap::new(), "{at}");
            continue;
        }
        let db = TimescaleDb::of(&index, None).unwrap();
        let metrics = resolved(&db).hypertables.iter().find(|h| h.table == "public.metrics");
        let listed: Vec<String> = metrics
            .unwrap()
            .chunks
            .iter()
            .filter(|c| !c.dropped)
            .map(|c| c.table.clone())
            .collect();
        assert!(!listed.is_empty(), "{at}");
        assert_eq!(absent.get("public.metrics"), Some(&listed), "{at}");
    }
}

/// A pass cutting blocks among workers reads the catalog's on its own loop,
/// so it records what the serial pass does, and the cache keeps it; the
/// findings over it are the serial map's, and a load derives them again.
#[tokio::test]
async fn a_parallel_pass_records_the_catalog_and_its_cache_keeps_it() {
    let serial_index = index_of(18, "default").await;
    let serial = TimescaleDb::of(&serial_index, None).unwrap();
    let serial_findings = findings(&serial_index);
    assert_ne!(serial_findings.0, BTreeMap::new(), "2.31's stray root row");
    let (_dir, dump) = sandboxed(&timescaledb_fixture(18, "default"), "default.sql");
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&dump).unwrap());
    source.hint_read_size(64);
    let mode = CacheMode::enabled(cache::colocated_path(&dump));
    let request =
        ArrangementRequest { jobs: Jobs::Stated(4), ..ArrangementRequest::recommended(4, None) };
    let run = map_file(
        mode.open_source(Arc::clone(&source)).await.unwrap(),
        ReadOptions { chunk_size_bytes: 64, ..ReadOptions::default() },
        &Arrangement::resolve(&Allowance::NoneFound, request),
        PostgresInvalidValues::Default,
        &StatisticsRequest::DATA,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(TimescaleDb::of(&run.index, None).as_ref(), Some(&serial));
    assert_eq!(findings(&run.index), serial_findings);
    let Held::Loaded(loaded) = mode.open_source(source).await.unwrap().cache else {
        panic!("the pass left a cache")
    };
    assert_eq!(TimescaleDb::of(&loaded.index, None), Some(serial));
    assert_eq!(findings(&loaded.index), serial_findings, "the load derives them again");
}
