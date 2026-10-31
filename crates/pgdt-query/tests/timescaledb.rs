//! A TimescaleDB hypertable read under its own name, held to its restore at
//! every pin and flag set: read with its descendants as its root's own rows
//! as the restore holds them (TS8) and its present chunks', a chunk the file
//! lacks read as absent (TS10), and read alone as its root's own; a chunk
//! whose rows are compressed refused wherever it is reached (`KD159`), the
//! compressed relation read as its batches; every other table as the server
//! answers it. Over a complete map and a cold query's pass alike — and over
//! a catalog that does not read, which refuses its hypertables read with
//! their descendants.

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pgdt_codec::SchemaMode;
use pgdt_core::Error;
use pgdt_io::{ByteRangeSource, LocalFileSource};
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::timescaledb::{Resolution, TimescaleDb};
use pgdt_model::{DatabaseMetadata, DiagnosticKind, DumpIndex};
use pgdt_query::{Dump, Inheritance, Table, TableSelector};

use common::{Answer, Answers, Cold, Reading, answers, mapped, read, timescaledb_fixture};

/// Every TimescaleDB fixture's flag sets, at the majors each runs at.
const FIXTURES: [(u32, &str); 11] = [
    (13, "default"),
    (13, "exclude-chunks"),
    (13, "tables"),
    (13, "schema-public"),
    (15, "default"),
    (16, "default"),
    (17, "default"),
    (18, "default"),
    (18, "exclude-chunks"),
    (18, "tables"),
    (18, "schema-public"),
];

fn source(path: &Path) -> Arc<dyn ByteRangeSource> {
    Arc::new(LocalFileSource::open(path).unwrap())
}

fn inheritance(reading: Reading) -> Inheritance {
    match reading {
        Reading::Descendants => Inheritance::Descendants,
        Reading::Only => Inheritance::Only,
    }
}

/// The one database's DDL.
fn ddl(index: &DumpIndex) -> &DatabaseMetadata {
    &index.metadata.as_ref().expect("a whole map states its metadata").databases[0]
}

/// What a read of a table is held to: its rows, or a refusal for the chunks
/// named compressed.
#[derive(Debug, PartialEq, Eq)]
enum Expected {
    Rows(Vec<String>),
    Compressed(Vec<String>),
}

/// The rows an answer holds, which the test expects to be rows.
fn rows(answer: Option<&Answer>, at: &str) -> Vec<String> {
    match answer {
        Some(Answer::Rows(rows)) => rows.clone(),
        other => panic!("{at}: no rows answered: {other:?}"),
    }
}

/// The catalog's hypertables, and its chunks whose rows are compressed.
fn catalog(index: &DumpIndex) -> (BTreeSet<String>, BTreeSet<String>) {
    let Some(db) = TimescaleDb::of(index, None) else { return Default::default() };
    let Resolution::Resolved(resolved) = db.resolution else { panic!("unreadable: {db:?}") };
    let hypertables = resolved.hypertables.iter().map(|h| h.table.clone()).collect();
    let compressed = resolved
        .hypertables
        .iter()
        .flat_map(|h| &h.chunks)
        .filter(|c| !c.dropped && (c.compressed() || c.partially_compressed()))
        .map(|c| c.table.clone())
        .collect();
    (hypertables, compressed)
}

/// What a read of `qualified` the way `reading` says is held to, over a dump
/// whose answers are `answers`.
fn expected(
    index: &DumpIndex,
    answers: &Answers,
    qualified: &str,
    reading: Reading,
    at: &str,
) -> Expected {
    let (hypertables, compressed) = catalog(index);
    let descendants = ddl(index).descendants(qualified);
    if compressed.contains(qualified) {
        return Expected::Compressed(vec![qualified.to_owned()]);
    }
    if !hypertables.contains(qualified) || !ddl(index).tables.contains_key(qualified) {
        return Expected::Rows(rows(answers.tables.get(&(qualified.to_owned(), reading)), at));
    }
    let mut held = rows(answers.held.get(qualified), at);
    if reading == Reading::Only {
        return Expected::Rows(held);
    }
    let reached: Vec<String> =
        descendants.iter().filter(|d| compressed.contains(**d)).map(|d| d.to_string()).collect();
    if !reached.is_empty() {
        return Expected::Compressed(reached);
    }
    for descendant in descendants {
        held.extend(rows(answers.tables.get(&(descendant.to_owned(), Reading::Only)), at));
    }
    held.sort();
    Expected::Rows(held)
}

/// A read of `table` as `reading` says, in `schema_mode`, as what it is held
/// to.
async fn reading(table: &Table, schema_mode: SchemaMode, reading: Reading, at: &str) -> Expected {
    match read(table, schema_mode, inheritance(reading)).await {
        Ok(rows) => Expected::Rows(rows),
        Err(Error::CompressedChunks { table: refused, chunks }) => {
            assert_eq!(refused, table.name().qualified_name(), "{at}");
            Expected::Compressed(chunks)
        }
        Err(error) => panic!("{at}: {error}"),
    }
}

/// Every table each restore answers, read each way the server read it over a
/// complete map in both schema modes, held to [`expected`]; and every pin
/// refuses some compressed chunk, read by its name and through its
/// hypertable.
#[tokio::test]
async fn each_hypertable_reads_as_its_restore_holds_it() {
    for (version, flag_set) in FIXTURES {
        let path = timescaledb_fixture(version, flag_set);
        let answers = answers(&path);
        let dump = mapped(source(&path), &StatisticsRequest::DATA).await;
        let mut refused = BTreeSet::new();
        for (qualified, reading_) in answers.tables.keys() {
            let reading_ = *reading_;
            let at = format!("{version} {flag_set} {qualified} {reading_:?}");
            let expected = expected(dump.index(), &answers, qualified, reading_, &at);
            let name = dump.tables().iter().find(|n| n.qualified_name() == *qualified);
            let table = dump.table(&TableSelector::from(name.expect(&at))).unwrap().unwrap();
            for schema_mode in [SchemaMode::Typed, SchemaMode::Strings] {
                let at = format!("{at} {schema_mode:?}");
                assert_eq!(reading(&table, schema_mode, reading_, &at).await, expected, "{at}");
            }
            if let Expected::Compressed(_) = expected {
                refused.insert((qualified.clone(), reading_));
            }
        }
        if flag_set == "default" {
            let by_name = refused.iter().any(|(t, r)| t != "public.metrics" && *r == Reading::Only);
            let through = refused.contains(&("public.metrics".to_owned(), Reading::Descendants));
            assert!(by_name && through, "{version}: {refused:?}");
        }
    }
}

/// A cold query's pass refuses and reads as a complete map's table does: a
/// chunk runs it to the end, its catalog's blocks being anywhere past it.
#[tokio::test]
async fn a_cold_query_reads_a_hypertable_and_its_chunks() {
    for version in [13, 18] {
        let path = timescaledb_fixture(version, "default");
        let answers = answers(&path);
        let index = mapped(source(&path), &StatisticsRequest::DATA).await.index().clone();
        for (qualified, reading_) in answers.tables.keys() {
            let at = format!("{version} {qualified} {reading_:?} cold");
            let expected = expected(&index, &answers, qualified, *reading_, &at);
            let table = Cold::default().table(source(&path), qualified, SchemaMode::Typed).await;
            let table = table.unwrap().unwrap_or_else(|| panic!("{at}: not found"));
            assert_eq!(reading(&table, SchemaMode::Typed, *reading_, &at).await, expected, "{at}");
        }
    }
}

/// `path`'s dump with each of `renames` applied to the one line it is found
/// on, written into `dir`.
fn edited(path: &Path, dir: &Path, renames: &[(&str, &str)]) -> PathBuf {
    let mut text = std::fs::read_to_string(path).unwrap();
    for (from, to) in renames {
        assert_eq!(text.matches(from).count(), 1, "{from}");
        text = text.replace(from, to);
    }
    let out = dir.join("edited.sql");
    std::fs::write(&out, text).unwrap();
    out
}

/// The refusal a read of `qualified` with its descendants meets over `dump`,
/// or `None` where it reads.
async fn refusal(dump: &Dump, qualified: &str) -> Option<Error> {
    let name = dump.tables().iter().find(|n| n.qualified_name() == qualified).unwrap();
    let table = dump.table(&TableSelector::from(name)).unwrap().unwrap();
    read(&table, SchemaMode::Strings, Inheritance::Descendants).await.err()
}

/// **A catalog that does not read refuses its hypertables read with their
/// descendants**, naming what it lacks: those its `hypertable` rows name,
/// and where those do not read, every table with a descendant. Read alone,
/// each reads its root's own rows, and each chunk reads under its own name,
/// a compressed one's block unchecked, as the map's finding of the
/// catalog says.
#[tokio::test]
async fn an_unreadable_catalog_refuses_its_hypertables() {
    let path = timescaledb_fixture(18, "default");
    let answers = answers(&path);
    let dir = tempfile::tempdir().unwrap();
    let slice = "COPY _timescaledb_catalog.dimension_slice (id, chunk_id,";
    let renamed = "COPY _timescaledb_catalog.dimension_slice (id, chunk_ident,";
    let hypertable = "COPY _timescaledb_catalog.hypertable (id, schema_name, table_name,";
    let unnamed = "COPY _timescaledb_catalog.hypertable (id, schema_name, table_label,";
    for renames in [vec![(slice, renamed)], vec![(slice, renamed), (hypertable, unnamed)]] {
        let at = format!("{renames:?}");
        let edited = edited(&path, dir.path(), &renames);
        let dump = mapped(source(&edited), &StatisticsRequest::DATA).await;
        let ddl = ddl(dump.index());
        let finding = dump.index().diagnostics.iter().find_map(|d| match d.kind() {
            DiagnosticKind::TimescaleCatalogUnreadable {
                database, missing, hypertables, ..
            } => Some((database.clone(), missing.clone(), hypertables.clone())),
            _ => None,
        });
        let Some((None, missing, hypertables)) = finding else { panic!("{at}: {finding:?}") };
        assert!(missing.contains(&"dimension_slice.chunk_id".to_owned()), "{at}: {missing:?}");
        let named = hypertables.is_some_and(|names| names.contains(&"public.metrics".to_owned()));
        assert_eq!(named, renames.len() == 1, "{at}");
        for qualified in ["public.metrics", "public.counters"] {
            let refused = refusal(&dump, qualified).await;
            let Some(Error::TimescaleCatalogUnreadable { table, missing, .. }) = refused else {
                panic!("{at}: {qualified} {refused:?}")
            };
            assert_eq!(table, qualified, "{at}");
            assert!(missing.contains(&"dimension_slice.chunk_id".to_owned()), "{at}: {missing:?}");
            let name = dump.tables().iter().find(|n| n.qualified_name() == qualified).unwrap();
            let table = dump.table(&TableSelector::from(name)).unwrap().unwrap();
            let alone = read(&table, SchemaMode::Strings, Inheritance::Only).await.unwrap();
            assert_eq!(alone, rows(answers.held.get(qualified), &at), "{at}: {qualified}");
            for chunk in ddl.descendants(qualified) {
                assert_eq!(refusal(&dump, chunk).await.map(|e| e.to_string()), None, "{at}");
            }
        }
        // A compressed relation inherits nothing at 2.31, so it is no
        // hypertable's descendant and reads.
        let compressed = "_timescaledb_internal._hyper_1_2_chunk_compressed";
        assert!(refusal(&dump, compressed).await.is_none(), "{at}");
    }
}
