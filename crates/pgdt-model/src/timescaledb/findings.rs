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
use super::{Hypertable, Resolution, TimescaleDb};

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
    let root_rows = |hypertable: &str| {
        let rows = root_rows(index, database.as_deref(), hypertable);
        (rows > 0)
            .then(|| Diagnostic::timescale_root_rows(database.clone(), hypertable.into(), rows))
    };
    match resolution {
        Resolution::Resolved(resolved) => {
            for hypertable in &resolved.hypertables {
                findings.extend(root_rows(&hypertable.table));
                let absent = ddl.map(|ddl| absent_chunks(ddl, hypertable)).unwrap_or_default();
                findings.extend((!absent.is_empty()).then(|| {
                    Diagnostic::timescale_chunks_absent(
                        database.clone(),
                        hypertable.table.clone(),
                        absent,
                    )
                }));
            }
        }
        Resolution::Unreadable { missing, malformed, hypertables, .. } => {
            for hypertable in hypertables.iter().flatten() {
                findings.extend(root_rows(hypertable));
            }
            let unreadable =
                Diagnostic::timescale_catalog_unreadable(database, missing, malformed, hypertables);
            findings.insert(0, unreadable);
        }
    }
    findings
}

/// **The rows `hypertable`'s root holds of its own**, every block of it in
/// `database` summed: what its finding counts where there are any, a root
/// holding none answering as the server's reads do, and what a read of it
/// says it includes (`docs/design/decisions.md`, "D135").
// timescaledb: TS8 — a restore holds the root's rows, and TimescaleDB's planner never reads them.
pub fn root_rows(index: &DumpIndex, database: Option<&str>, hypertable: &str) -> u64 {
    index
        .blocks_with_database()
        .filter(|(db, block)| *db == database && block.header.qualified_name() == hypertable)
        .map(|(_, block)| block.row_count)
        .sum()
}

/// **The chunks of `hypertable` whose tables `ddl`, its database's, does not
/// declare**, each `schema.table`, its dropped rows left out: read as absent,
/// as a restore of the file lacks them — what its finding lists, and what a
/// read of it says it holds no row of (`docs/design/decisions.md`, "D135").
/// Where the database's DDL is not held nothing declares them, so a caller
/// concludes none.
// timescaledb: TS6 — a dropped row names no table, so it is no chunk.
// timescaledb: TS10 — a restore holds no row of a chunk the file lacks.
pub fn absent_chunks(ddl: &DatabaseMetadata, hypertable: &Hypertable) -> Vec<String> {
    hypertable
        .chunks
        .iter()
        .filter(|chunk| !chunk.dropped && !ddl.tables.contains_key(chunk.table.as_str()))
        .map(|chunk| chunk.table.clone())
        .collect()
}
