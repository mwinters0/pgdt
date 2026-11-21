//! What a mapping pass records of TimescaleDB's catalog: which blocks, and
//! their rows as text, persisted on each block's `BlockRecord::recorded_rows`
//! (`docs/design/decisions.md`, "D133") as `crate::persisted::timescaledb`'s
//! shape.

use pgdt_core::{CopyHeader, unescape_field};

/// The schema TimescaleDB's catalog tables live in.
pub const CATALOG_SCHEMA: &str = "_timescaledb_catalog";

/// The catalog tables whose rows a pass records: those a recipe, the version
/// or a continuous aggregate reads. Every other catalog block is read by its
/// header alone, for the fingerprint (`Era`).
const RECORDED: [&str; 8] = [
    "hypertable",
    "dimension",
    "dimension_slice",
    "chunk",
    "chunk_constraint",
    "compression_settings",
    "continuous_agg",
    "metadata",
];

/// Whether a pass records the rows of the block `header` opens: one of
/// [`CATALOG_SCHEMA`]'s tables a conclusion reads.
// timescaledb: TS2 — the catalog reaches the dump as COPY blocks under its own schema.
pub fn records_rows(header: &CopyHeader) -> bool {
    header.schema.as_deref() == Some(CATALOG_SCHEMA) && RECORDED.contains(&header.table.as_str())
}

/// One still-escaped field of a recorded row as `RecordedRows` keeps it:
/// its text with its `COPY` escapes undone, a NULL as `None`, and one that is
/// not UTF-8 once unescaped as its escaped bytes read lossily. The block's
/// recorder hands it each field of a row it records
/// (`crate::internal::Recorder::row`).
pub(crate) fn recorded_field(field: &[u8]) -> Option<String> {
    match unescape_field(field) {
        Ok(text) => text.map(|text| text.into_owned()),
        Err(_) => Some(String::from_utf8_lossy(field).into_owned()),
    }
}
