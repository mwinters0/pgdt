//! The fixtures holding a field PostgreSQL's input function refuses, which a
//! data-level parse fails on (`docs/design/roadmap.md`, "A literal is
//! guaranteed in `*_out`'s form and never read past `*_in`'s"), and how a sweep
//! maps one past it. One list for every crate's sweeps: `datafusion-pgdump`'s
//! and `pgdt`'s tests include this file by path, so no copy of it drifts.
//!
//! `pgdump_query/tests/decode.rs`'s
//! `every_refused_field_fails_a_data_level_parse_reading_it` holds each row to
//! the refusal it names.

// Each test binary uses a subset of it.
#![allow(dead_code)]

use std::path::Path;

use pgdump_query::{StatisticsLevel, StatisticsRequest, StatisticsTarget};

/// A field PostgreSQL refuses, held by every major's copy of a fixture: a
/// data-level parse fails on it, naming its table and column, and a query
/// decoding it refuses it.
pub struct RefusedField {
    /// `<schema>/<flag-set>`.
    pub fixture: &'static str,
    pub table: &'static str,
    pub column: &'static str,
}

pub const REFUSED_FIELDS: &[RefusedField] = &[RefusedField {
    // `--extra-float-digits=0` rounds `DBL_MAX` past itself (I57), which
    // `float8in` refuses (I59).
    fixture: "types/extra-float-digits-0",
    table: "public.t_extremes",
    column: "v_double",
}];

/// The refused field `path` holds, where it is a copy of a fixture holding
/// one.
pub fn refused_field(path: &Path) -> Option<&'static RefusedField> {
    REFUSED_FIELDS.iter().find(|r| path.ends_with(Path::new(r.fixture).with_extension("sql")))
}

impl RefusedField {
    /// The `--statistics-level` entry putting the column at the metadata
    /// level, where nothing keys it.
    pub fn statistics_level(&self) -> String {
        format!("{}.{}=metadata", self.table, self.column)
    }
}

/// What a sweep maps `path` at: every table and column at the data level, but a
/// column holding a field PostgreSQL refuses ([`past_refused`]).
pub fn sweep_request(path: &Path) -> StatisticsRequest {
    past_refused(path, &StatisticsRequest::DATA)
}

/// `request`, but for a column of `path` holding a field PostgreSQL refuses,
/// which goes unkeyed at the metadata level while its table is censused and
/// gathered.
pub fn past_refused(path: &Path, request: &StatisticsRequest) -> StatisticsRequest {
    let mut request = request.clone();
    if let Some(refused) = refused_field(path) {
        let target = StatisticsTarget::Column {
            table: refused.table.to_string(),
            column: refused.column.to_string(),
        };
        request.selection.overrides.push((target, StatisticsLevel::Metadata));
    }
    request
}
