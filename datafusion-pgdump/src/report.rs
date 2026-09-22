//! What registration and planning report to the caller's sink
//! (`docs/design/roadmap-P6-datafusion.md`, "Diagnostics: one sink").
//!
//! **What a dump or a table is, is reported when it is registered**: the
//! file-level channel once per dump, and each table's per-column notes and
//! its columns' divergence from PostgreSQL in the semantics every scan asks
//! for. **What a scan's plan settled is reported when it is planned**, from
//! `scan()`, to the sink the table was registered with. A query raises nothing
//! on the comparison channel here — a pushed filter is one the library answers
//! as DataFusion does, so that channel is empty
//! (`pgdump_query::column_divergences` said it already, per column) — and what
//! a scan finds while reading is a plan metric, not a finding
//! ([`crate::exec`]).

use std::any::Any;
use std::sync::Arc;

use pgdump_query::{
    ComparisonSemantics, DiagnosticSink, Finding, PlanNote, Severity, column_divergences,
};

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

/// Where a registered table's scans report: the name SQL reaches it by, and
/// the sink it was registered with.
pub(crate) struct Reporting {
    subject: String,
    sink: Arc<dyn DiagnosticSink>,
}

impl PgDumpTable {
    /// Hand this table's findings to `sink`, each prefixed with `subject` —
    /// the name SQL reaches the table by: how each column resolved, and how
    /// each column's comparison in DataFusion's semantics diverges from
    /// PostgreSQL's.
    ///
    /// **`sink` is kept, and hears every scan's plan notes from then on**
    /// (`pgdump_query::PlanNote`), under the same `subject`: what the memory
    /// budget declined and what statistics let the scan skip, settled when
    /// the scan is planned. A table reported again reports its scans to the
    /// latest sink; one never reported reports no scan.
    pub fn report(&self, subject: &str, sink: Arc<dyn DiagnosticSink>) {
        let resolved = self.resolved_schema();
        for finding in &resolved.notes {
            sink.report(&Located { subject, finding });
        }
        for finding in &column_divergences(resolved, ComparisonSemantics::Arrow) {
            sink.report(&Located { subject, finding });
        }
        let reporting = Reporting { subject: subject.to_string(), sink };
        *self.reporting().lock().unwrap() = Some(Arc::new(reporting));
    }

    /// Hand what one scan's plan settled to the sink this table was reported
    /// to, if it was.
    pub(crate) fn report_plan(&self, notes: &[PlanNote]) {
        // Taken out of the lock, so a sink that reports this table again
        // does not wait on itself.
        let Some(reporting) = self.reporting().lock().unwrap().clone() else { return };
        for finding in notes {
            reporting.sink.report(&Located { subject: &reporting.subject, finding });
        }
    }
}
