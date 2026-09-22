//! What registration reports to the caller's sink
//! (`docs/design/roadmap-P6-datafusion.md`, "Diagnostics: one sink").
//!
//! **Everything is reported when a dump or a table is registered**: the
//! file-level channel once per dump, and each table's per-column notes and
//! its columns' divergence from PostgreSQL in the semantics every scan asks
//! for. A query raises nothing on the library's three channels here — a
//! pushed filter is one the library answers as DataFusion does, so its
//! comparison channel is empty (`pgdump_query::column_divergences` said it
//! already, per column).

use std::any::Any;

use pgdump_query::{ComparisonSemantics, DiagnosticSink, Finding, Severity, column_divergences};

use crate::dump::PgDump;
use crate::table::PgDumpTable;

/// A finding with what it is about named in front of its sentence: the dump,
/// or a table as SQL names it. The library's findings name a column at most
/// (`pgdump_query::ColumnNote`, `pgdump_query::ComparisonNote`), and one sink
/// hears every table of every registered dump.
///
/// [`Finding::as_any`] is the wrapped finding's, so a sink downcasting to the
/// library's own record still reaches it; the subject is in the sentence.
#[derive(Debug)]
struct Located<'a> {
    subject: &'a str,
    finding: &'a dyn Finding,
}

impl Finding for Located<'_> {
    fn severity(&self) -> Severity {
        self.finding.severity()
    }

    fn message(&self) -> String {
        format!("{}: {}", self.subject, self.finding.message())
    }

    fn as_any(&self) -> &dyn Any {
        self.finding.as_any()
    }
}

impl PgDump {
    /// Hand what the file-level channel says about this dump and its cache
    /// to `sink`, each finding prefixed with the dump's origin.
    pub fn report(&self, sink: &dyn DiagnosticSink) {
        for finding in self.diagnostics() {
            sink.report(&Located { subject: self.origin(), finding });
        }
    }
}

impl PgDumpTable {
    /// Hand this table's findings to `sink`, each prefixed with `subject` —
    /// the name SQL reaches the table by: how each column resolved, and how
    /// each column's comparison in DataFusion's semantics diverges from
    /// PostgreSQL's.
    pub fn report(&self, subject: &str, sink: &dyn DiagnosticSink) {
        let resolved = self.resolved_schema();
        for finding in &resolved.notes {
            sink.report(&Located { subject, finding });
        }
        for finding in &column_divergences(resolved, ComparisonSemantics::Arrow) {
            sink.report(&Located { subject, finding });
        }
    }
}
