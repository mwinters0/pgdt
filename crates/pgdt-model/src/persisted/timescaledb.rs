//! TimescaleDB's catalog rows as the cache persists them on their blocks
//! (`docs/design/decisions.md`, "D133"). Which blocks a pass records, and
//! what the rows conclude, are `crate::timescaledb`'s.

use serde::{Deserialize, Serialize};

/// The rows of one block a pass recorded ([`records_rows`](crate::timescaledb::records_rows)), each field's
/// text with its `COPY` escapes undone and a NULL as `None`, in the order of
/// the block's header. A field that is not UTF-8 once unescaped is kept as
/// its escaped bytes read lossily, as the DDL around it is read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedRows {
    pub rows: Vec<Vec<Option<String>>>,
}
