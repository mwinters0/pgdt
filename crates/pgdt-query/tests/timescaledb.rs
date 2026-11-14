//! A TimescaleDB hypertable read under its own name, held to its restore at
//! every pin and flag set: read with its descendants as its root's own rows
//! as the restore holds them (TS8) and its present chunks', a chunk the file
//! lacks read as absent (TS10), and read alone as its root's own; a chunk
//! whose rows are compressed refused wherever it is reached (`KD159`), the
//! compressed relation read as its batches; every other table as the server
//! answers it. Over a complete map and a cold query's pass alike — and over
//! a catalog that does not read, which refuses its hypertables the file holds
//! a chunk of read with their descendants. Each read says what the map finds
//! of it: a root's own rows, absent chunks, a read unchecked for compression,
//! chunks that cannot be known absent (D135).

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pgdt_codec::SchemaMode;
use pgdt_core::Error;
use pgdt_dump::ReadOptions;
use pgdt_io::{ByteRangeSource, LocalFileSource};
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::timescaledb::{Release, Resolution, TimescaleDb};
use pgdt_model::{DatabaseMetadata, DiagnosticKind, DumpIndex};
use pgdt_query::{Dump, Inheritance, PlanNoteKind, QueryOptions, Table, TableSelector};

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

/// **What a compressed chunk refuses is the map's finding**, found by one
/// derivation: each hypertable with a compressed chunk the file holds is
/// named with exactly those chunks, refuses read with its descendants naming
/// them, and each refuses read by its own name under either mode; a chunk
/// refused by its own name is one a finding names. A dump holding none of
/// its chunks finds none, and every other finds one.
#[tokio::test]
async fn what_a_compressed_chunk_refuses_is_the_map_s_finding() {
    for (version, flag_set) in FIXTURES {
        let at = format!("{version} {flag_set}");
        let path = timescaledb_fixture(version, flag_set);
        let dump = mapped(source(&path), &StatisticsRequest::DATA).await;
        let mut found = BTreeSet::new();
        for diagnostic in &dump.index().diagnostics {
            let DiagnosticKind::TimescaleChunksCompressed { database, hypertable, chunks } =
                diagnostic.kind()
            else {
                continue;
            };
            assert_eq!(*database, None, "{at}");
            let refused = refusal(&dump, hypertable).await;
            let Some(Error::CompressedChunks { chunks: reached, .. }) = refused else {
                panic!("{at}: {hypertable} {refused:?}")
            };
            let reached: BTreeSet<&String> = reached.iter().collect();
            assert_eq!(reached, chunks.iter().collect(), "{at}: {hypertable}");
            for chunk in chunks {
                let name = dump.tables().iter().find(|n| n.qualified_name() == *chunk);
                let table = dump.table(&TableSelector::from(name.expect(chunk))).unwrap().unwrap();
                for inheritance in [Inheritance::Descendants, Inheritance::Only] {
                    let read = read(&table, SchemaMode::Strings, inheritance).await;
                    assert!(matches!(read, Err(Error::CompressedChunks { .. })), "{at}: {chunk}");
                }
                assert!(found.insert(chunk.clone()), "{at}: {chunk} found twice");
            }
        }
        let (_, compressed) = catalog(dump.index());
        let held: BTreeSet<String> =
            compressed.into_iter().filter(|c| ddl(dump.index()).tables.contains_key(c)).collect();
        assert_eq!(found, held, "{at}");
        let chunkless = matches!(flag_set, "exclude-chunks" | "schema-public");
        assert_eq!(found.is_empty(), chunkless, "{at}");
    }
}

/// A cold query's pass refuses, reads and notes as a complete map's table
/// does: a chunk runs it to the end, its catalog's blocks being anywhere past
/// it.
#[tokio::test]
async fn a_cold_query_reads_a_hypertable_and_its_chunks() {
    for version in [13, 18] {
        let path = timescaledb_fixture(version, "default");
        let answers = answers(&path);
        let dump = mapped(source(&path), &StatisticsRequest::DATA).await;
        for (qualified, reading_) in answers.tables.keys() {
            let at = format!("{version} {qualified} {reading_:?} cold");
            let expected = expected(dump.index(), &answers, qualified, *reading_, &at);
            let table = Cold::default().table(source(&path), qualified, SchemaMode::Typed).await;
            let table = table.unwrap().unwrap_or_else(|| panic!("{at}: not found"));
            assert_eq!(reading(&table, SchemaMode::Typed, *reading_, &at).await, expected, "{at}");
            let name = dump.tables().iter().find(|n| n.qualified_name() == *qualified).unwrap();
            let whole = dump.table(&TableSelector::from(name)).unwrap().unwrap();
            let mode = inheritance(*reading_);
            let said = timescaledb_notes(&table, mode);
            assert_eq!(said, timescaledb_notes(&whole, mode), "{at}: notes");
        }
    }
}

/// The TimescaleDB kinds among the notes a plan of `table` under
/// `inheritance` raises, `None` where it refuses; a stream resumed from the
/// plan's start raising the same.
fn timescaledb_notes(table: &Table, inheritance: Inheritance) -> Option<Vec<PlanNoteKind>> {
    let options = QueryOptions { inheritance, ..QueryOptions::default() };
    let plan = table.plan(ReadOptions::default(), options, &pgdt_fixtures::arranged(1)).ok()?;
    let ours = |kind: &PlanNoteKind| {
        matches!(
            kind,
            PlanNoteKind::HypertableRootRows { .. }
                | PlanNoteKind::HypertableChunksAbsent { .. }
                | PlanNoteKind::CompressionUnchecked { .. }
                | PlanNoteKind::HypertableChunksUnknown { .. }
        )
    };
    let notes: Vec<PlanNoteKind> =
        plan.plan_notes().iter().map(|n| n.kind.clone()).filter(ours).collect();
    let token = plan.in_file_order(8192).resume_token();
    let resumed = plan.resume(token, 8192).unwrap().plan_notes();
    let resumed: Vec<PlanNoteKind> = resumed.into_iter().map(|n| n.kind).filter(ours).collect();
    assert_eq!(resumed, notes, "{} {inheritance:?}: resumed", table.name().qualified_name());
    Some(notes)
}

/// **Each read says what the map finds of it**, counted alike (D135): a
/// hypertable read with its descendants the rows its root holds, which its
/// restore's `held` answer holds, and the chunks its catalog lists that the
/// file lacks; read alone its root's rows where the release's `ONLY` reads
/// it empty (TS8); every other read, nothing. Over every fixture, each
/// finding said by some read.
#[tokio::test]
async fn each_read_says_what_the_map_finds_of_it() {
    let mut said = BTreeSet::new();
    for (version, flag_set) in FIXTURES {
        let path = timescaledb_fixture(version, flag_set);
        let answers = answers(&path);
        let dump = mapped(source(&path), &StatisticsRequest::DATA).await;
        let index = dump.index();
        let empty_alone =
            TimescaleDb::of(index, None).is_some_and(|db| db.version.at_least(Release::new(2, 28)));
        for (qualified, reading_) in answers.tables.keys() {
            let at = format!("{version} {flag_set} {qualified} {reading_:?}");
            let name = dump.tables().iter().find(|n| n.qualified_name() == *qualified);
            let table = dump.table(&TableSelector::from(name.expect(&at))).unwrap().unwrap();
            let Some(notes) = timescaledb_notes(&table, inheritance(*reading_)) else { continue };
            let mut expected = Vec::new();
            for diagnostic in &index.diagnostics {
                match diagnostic.kind() {
                    DiagnosticKind::TimescaleRootRows { hypertable, rows, .. }
                        if hypertable == qualified =>
                    {
                        let held = rows_of_held(&answers, qualified, &at);
                        assert_eq!(*rows, held, "{at}: the root's rows against its restore");
                        if *reading_ == Reading::Descendants || empty_alone {
                            let (table, rows) = (qualified.clone(), *rows);
                            expected.push(PlanNoteKind::HypertableRootRows { table, rows });
                            said.insert(("root", *reading_));
                        }
                    }
                    DiagnosticKind::TimescaleChunksAbsent { hypertable, chunks, .. }
                        if hypertable == qualified && *reading_ == Reading::Descendants =>
                    {
                        let (table, chunks) = (qualified.clone(), chunks.clone());
                        expected.push(PlanNoteKind::HypertableChunksAbsent { table, chunks });
                        said.insert(("absent", *reading_));
                    }
                    _ => {}
                }
            }
            assert_eq!(notes, expected, "{at}");
        }
    }
    let all = [("absent", Reading::Descendants), ("root", Reading::Descendants)];
    let all = all.into_iter().chain([("root", Reading::Only)]).collect::<BTreeSet<_>>();
    assert_eq!(said, all);
}

/// How many rows hypertable `qualified`'s root holds as its restore holds
/// them.
fn rows_of_held(answers: &Answers, qualified: &str, at: &str) -> u64 {
    rows(answers.held.get(qualified), at).len() as u64
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
/// descendants**, naming what it lacks: those its `hypertable` rows name
/// that the file holds a chunk of, and where those do not read, every table
/// with a descendant. Read alone,
/// each reads its root's own rows, and each chunk reads under its own name,
/// a compressed one's block unchecked, as the map's finding of the
/// catalog says and each such read says too: a table below a hypertable the
/// catalog's rows name, or below any table where they name none.
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
            // Read alone, a named hypertable says its root's rows, 2.31's
            // `ONLY` reading it empty, and nothing of compression.
            let held = rows_of_held(&answers, qualified, &at);
            let root = PlanNoteKind::HypertableRootRows { table: qualified.into(), rows: held };
            let expected = match named && held > 0 {
                true => vec![root],
                false => Vec::new(),
            };
            let notes = timescaledb_notes(&table, Inheritance::Only);
            assert_eq!(notes, Some(expected), "{at}: {qualified} alone");
            for chunk in ddl.descendants(qualified) {
                assert_eq!(refusal(&dump, chunk).await.map(|e| e.to_string()), None, "{at}");
                let name = dump.tables().iter().find(|n| n.qualified_name() == chunk).unwrap();
                let table = dump.table(&TableSelector::from(name)).unwrap().unwrap();
                for inheritance in [Inheritance::Descendants, Inheritance::Only] {
                    let notes = timescaledb_notes(&table, inheritance).unwrap();
                    let [PlanNoteKind::CompressionUnchecked { table, missing, .. }] = &notes[..]
                    else {
                        panic!("{at}: {chunk} {inheritance:?}: {notes:?}")
                    };
                    assert_eq!(table, chunk, "{at}");
                    assert!(missing.contains(&"dimension_slice.chunk_id".to_owned()), "{at}");
                }
            }
        }
        // A compressed relation inherits nothing at 2.31, so it is no
        // hypertable's descendant and reads, and nothing it reaches may be a
        // chunk.
        let compressed = "_timescaledb_internal._hyper_1_2_chunk_compressed";
        assert!(refusal(&dump, compressed).await.is_none(), "{at}");
        let name = dump.tables().iter().find(|n| n.qualified_name() == compressed).unwrap();
        let table = dump.table(&TableSelector::from(name)).unwrap().unwrap();
        assert_eq!(timescaledb_notes(&table, Inheritance::Descendants), Some(vec![]), "{at}");
    }
}

/// **A named hypertable the file holds no chunk of reads under an unreadable
/// catalog**, as a plain table: read with its descendants it holds its
/// root's own rows, as its restore's `held` answer does, and says which of
/// its chunks the file lacks cannot be known, beside its root's rows where it
/// holds any; nothing it reaches may be a chunk. Where even the names do not
/// read, it is indistinguishable from a plain table and says nothing.
#[tokio::test]
async fn an_unreadable_catalogs_named_hypertable_with_no_chunk_held_reads() {
    let path = timescaledb_fixture(18, "exclude-chunks");
    let answers = answers(&path);
    let dir = tempfile::tempdir().unwrap();
    let slice = "COPY _timescaledb_catalog.dimension_slice (id, chunk_id,";
    let renamed = "COPY _timescaledb_catalog.dimension_slice (id, chunk_ident,";
    let hypertable = "COPY _timescaledb_catalog.hypertable (id, schema_name, table_name,";
    let unnamed = "COPY _timescaledb_catalog.hypertable (id, schema_name, table_label,";
    for renames in [vec![(slice, renamed)], vec![(slice, renamed), (hypertable, unnamed)]] {
        let at = format!("{renames:?}");
        let named = renames.len() == 1;
        let edited = edited(&path, dir.path(), &renames);
        let dump = mapped(source(&edited), &StatisticsRequest::DATA).await;
        let mut rooted = false;
        for qualified in ["public.metrics", "public.counters"] {
            let at = format!("{at} {qualified}");
            assert!(ddl(dump.index()).descendants(qualified).is_empty(), "{at}: a chunk held");
            let name = dump.tables().iter().find(|n| n.qualified_name() == qualified).unwrap();
            let table = dump.table(&TableSelector::from(name)).unwrap().unwrap();
            let read = read(&table, SchemaMode::Strings, Inheritance::Descendants).await;
            assert_eq!(read.unwrap(), rows(answers.held.get(qualified), &at), "{at}");
            let held = rows_of_held(&answers, qualified, &at);
            let mut expected = Vec::new();
            if named && held > 0 {
                rooted = true;
                expected
                    .push(PlanNoteKind::HypertableRootRows { table: qualified.into(), rows: held });
            }
            if named {
                let notes = timescaledb_notes(&table, Inheritance::Descendants).unwrap();
                let Some(PlanNoteKind::HypertableChunksUnknown { table, missing, .. }) =
                    notes.last()
                else {
                    panic!("{at}: {notes:?}")
                };
                assert_eq!(table, qualified, "{at}");
                assert!(missing.contains(&"dimension_slice.chunk_id".to_owned()), "{at}");
                expected.push(notes.last().unwrap().clone());
            }
            let notes = timescaledb_notes(&table, Inheritance::Descendants);
            assert_eq!(notes, Some(expected), "{at}: with its descendants");
        }
        assert_eq!(rooted, named, "{at}: no root holding rows said so");
    }
}
