//! **What each TimescaleDB database's catalog says about the file**, as the
//! map's diagnostics carry it: the chunks a hypertable's catalog lists that
//! the file lacks, each root's own rows, and a catalog that does not read
//! (`docs/design/roadmap-P38-descendants.md`, "TimescaleDB"). Properties of
//! the file rather than of one read, so on the file-level channel
//! ([`crate::diagnostic`]'s module doc), derived wherever a whole map is
//! finished or loaded, beside the refusals a read settles over the same
//! conclusions.

use crate::diagnostic::{Diagnostic, DiagnosticExt};
use crate::{DatabaseMetadata, DumpIndex};

use super::recorded::CATALOG_SCHEMA;
use super::{Resolution, TimescaleDb};

/// Every TimescaleDB database's findings in `index`, the databases in the
/// order the DDL states them and then any whose catalog blocks alone detect
/// it (TS2), each database's in its hypertables' `id` order.
pub fn findings(index: &DumpIndex) -> Vec<Diagnostic> {
    let mut databases: Vec<Option<&str>> = Vec::new();
    let declared = index.metadata.iter().flat_map(|metadata| &metadata.databases);
    let catalogued = index
        .blocks_with_database()
        .filter(|(_, block)| block.header.schema.as_deref() == Some(CATALOG_SCHEMA))
        .map(|(database, _)| database);
    for database in declared.map(|db| db.name.as_deref()).chain(catalogued) {
        if !databases.contains(&database) {
            databases.push(database);
        }
    }
    let mut findings = Vec::new();
    for database in databases {
        let Some(timescaledb) = TimescaleDb::of(index, database) else { continue };
        let ddl = index
            .metadata
            .iter()
            .flat_map(|metadata| &metadata.databases)
            .find(|db| db.name.as_deref() == database);
        findings.extend(of_database(index, database, ddl, timescaledb.resolution));
    }
    findings
}

/// One database's findings: under a readable catalog each hypertable's root
/// rows and absent chunks; under one that does not read, the catalog's own
/// finding and the root rows of each hypertable its rows still name.
fn of_database(
    index: &DumpIndex,
    database: Option<&str>,
    ddl: Option<&DatabaseMetadata>,
    resolution: Resolution,
) -> Vec<Diagnostic> {
    let database = database.map(str::to_owned);
    let mut findings = Vec::new();
    match resolution {
        Resolution::Resolved(resolved) => {
            for hypertable in &resolved.hypertables {
                findings.extend(root_rows(index, &database, &hypertable.table));
                let chunks = hypertable.chunks.iter().filter(|c| !c.dropped).map(|c| &c.table);
                findings.extend(absent_chunks(ddl, &database, &hypertable.table, chunks));
            }
        }
        Resolution::Unreadable { missing, malformed, hypertables, .. } => {
            for hypertable in hypertables.iter().flatten() {
                findings.extend(root_rows(index, &database, hypertable));
            }
            let unreadable =
                Diagnostic::timescale_catalog_unreadable(database, missing, malformed, hypertables);
            findings.insert(0, unreadable);
        }
    }
    findings
}

/// The rows `hypertable`'s root holds of its own, every block of it summed,
/// where there are any: a root holding none answers as the server's reads do.
// timescaledb: TS8 — a restore holds the root's rows, and TimescaleDB's planner never reads them.
fn root_rows(index: &DumpIndex, database: &Option<String>, hypertable: &str) -> Option<Diagnostic> {
    let rows: u64 = index
        .blocks_with_database()
        .filter(|(db, block)| {
            *db == database.as_deref() && block.header.qualified_name() == hypertable
        })
        .map(|(_, block)| block.row_count)
        .sum();
    (rows > 0)
        .then(|| Diagnostic::timescale_root_rows(database.clone(), hypertable.to_owned(), rows))
}

/// The chunks among `chunks` — a hypertable's, its dropped rows left out —
/// whose tables the DDL does not declare, where there are any: read as
/// absent, as a restore of the file lacks them. `None` where the database's
/// DDL is not held, which declares nothing to tell them by.
// timescaledb: TS6 — a dropped row names no table, so it is no chunk.
// timescaledb: TS10 — a restore holds no row of a chunk the file lacks.
fn absent_chunks<'a>(
    ddl: Option<&DatabaseMetadata>,
    database: &Option<String>,
    hypertable: &str,
    chunks: impl Iterator<Item = &'a String>,
) -> Option<Diagnostic> {
    let ddl = ddl?;
    let absent: Vec<String> =
        chunks.filter(|chunk| !ddl.tables.contains_key(chunk.as_str())).cloned().collect();
    (!absent.is_empty()).then(|| {
        Diagnostic::timescale_chunks_absent(database.clone(), hypertable.to_owned(), absent)
    })
}
