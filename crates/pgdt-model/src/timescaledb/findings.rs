//! **What each TimescaleDB database's catalog says about the file**, as the
//! map's diagnostics carry it: the chunks the file holds whose rows are
//! compressed, the chunks a hypertable's catalog lists that the file lacks,
//! each root's own rows, and a catalog that does not read
//! (`docs/design/decisions.md`, "D135", "D136"). Properties of
//! the file rather than of one read, so on the file-level channel
//! ([`crate::diagnostic`]'s module doc), derived wherever a whole map is
//! finished or loaded, beside the refusals a read settles over the same
//! conclusions.

use crate::diagnostic::{Diagnostic, DiagnosticExt};
use crate::{DatabaseMetadata, DumpIndex};

use super::{Hypertable, Resolution, TimescaleDb};

/// Every TimescaleDB database's findings in `index`, the databases in
/// [`DumpIndex::databases`]' order, each database's in its hypertables' `id`
/// order, or where its catalog does not read, that finding first and then
/// its `hypertable` rows' order.
pub fn findings(index: &DumpIndex) -> Vec<Diagnostic> {
    let mut findings = Vec::new();
    for database in index.databases() {
        let Some(timescaledb) = TimescaleDb::of(index, database) else { continue };
        let ddl = index.ddl(database);
        findings.extend(of_database(index, database, ddl, timescaledb.resolution));
    }
    findings
}

/// One database's findings: under a readable catalog each hypertable's
/// compressed chunks, root rows and absent chunks; under one that does not
/// read, the catalog's own
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
                // A compressed chunk the file lacks refuses nothing (D134).
                let held = |chunk: &String| ddl.is_none_or(|ddl| ddl.tables.contains_key(chunk));
                let compressed: Vec<String> =
                    compressed_chunks(hypertable).into_iter().filter(held).collect();
                findings.extend((!compressed.is_empty()).then(|| {
                    Diagnostic::timescale_chunks_compressed(
                        database.clone(),
                        hypertable.table.clone(),
                        compressed,
                    )
                }));
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

/// **The chunks of `hypertable` whose rows are compressed or partially
/// compressed**, each `schema.table`, its dropped rows left out: what a read
/// reaching one refuses on, and, of those the file holds, what its finding
/// lists (`docs/design/decisions.md`, "D134", "D136").
// timescaledb: TS4 — bits 1 and 8 mark a chunk whose rows are compressed.
// timescaledb: TS6 — a dropped row names no table, so it is no chunk.
pub fn compressed_chunks(hypertable: &Hypertable) -> Vec<String> {
    hypertable
        .chunks
        .iter()
        .filter(|chunk| !chunk.dropped && (chunk.compressed() || chunk.partially_compressed()))
        .map(|chunk| chunk.table.clone())
        .collect()
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
        .map(|(_, block)| block.record.row_count)
        .sum()
}

/// **The chunks of `hypertable` whose tables `ddl`, its database's, does not
/// declare**, each `schema.table`, its dropped rows left out: read as absent,
/// as a restore of the file lacks them — what its finding lists, and what a
/// read of it says it holds no row of (`docs/design/decisions.md`, "D135").
/// Where the database's DDL is not held nothing declares them, so a caller
/// concludes none.
///
/// deficiency: KD163 — held means declared, so a dump whose DDL declares no
/// chunk though it holds their blocks — `--data-only`, whose segment's DDL
/// holds no table — reports every chunk absent, in this finding and in a
/// read's note, a hypertable read with its descendants answers its root's
/// rows alone, its `INHERITS` unknown, and a compressed chunk read by name
/// refuses while no finding names it, `of_database` keeping the
/// declared ones. Evidence: `fixtures/13/timescaledb/default.sql` with every
/// entry but `TABLE DATA` and `SEQUENCE SET` stripped. Closing it means
/// telling a chunk the file lacks from one it holds undeclared, which reopens
/// D134's "the DDL's walk". **(c) unowned**; promoted by a data-only
/// TimescaleDB dump met in use.
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
