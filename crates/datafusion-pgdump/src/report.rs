//! What registration and planning report to the caller's sink.
//!
//! **What a dump or a table is, is reported when it is registered**: the
//! file-level channel once per registration, and each table's per-column notes and
//! its columns' divergence from PostgreSQL in the semantics every scan asks
//! for — or, for a table the catalog does not list, its [`RefusedTable`] —
//! save the columns compared bytewise for want of a recorded collation, which
//! are one [`UncollatedColumns`] finding per registration, after the rest. **What a scan's plan settled is reported when it is planned**, from
//! `scan()`, to the sink the table was registered with. A query raises nothing
//! on the comparison channel here — a pushed filter is one the library answers
//! as DataFusion does, so that channel is empty
//! (`pgdt_query::Table::divergences` said it already, per column) — and what
//! a scan finds while reading is a plan metric, not a finding
//! ([`crate::exec`]). **A plan note quoting a budget is wrapped in a
//! [`BudgetedPlanNote`]**, which says where that budget came from.
//!
//! The sink is the caller's because DataFusion 55 offers a provider no
//! non-error channel — a `Diagnostic` rides only on an error — and `log` is
//! hidden at `datafusion-cli`'s default level. *Rejected: reporting what a
//! query raised after its statement*, which would mean owning
//! `datafusion-cli`'s REPL loop; *a scan's counts drained per partition as
//! `Info` findings*, which a stderr sink never prints, where a metric is what
//! Parquet's pruning already is.

use std::any::Any;
use std::collections::BTreeSet;
use std::sync::Arc;

use pgdt_codec::compare::ComparisonDivergence;
use pgdt_io::Allowance;
use pgdt_model::{DiagnosticSink, Finding, Severity, TableName};
use pgdt_query::{ComparisonNote, PlanLever, PlanNote};

use crate::budget::BudgetAccount;
use crate::dump::PgDump;
use crate::error::Error;

/// A finding with what it is about named in front of its sentence: the dump,
/// or a table as SQL names it. The library's findings name a column at most
/// (`pgdt_codec::ColumnNote`, `pgdt_query::ComparisonNote`), and one sink
/// hears every table of every registered dump.
///
/// [`Finding::as_any`] is the wrapped finding's, so a sink downcasting to the
/// library's own record still reaches it — save a plan note quoting a budget,
/// which is reached inside its [`BudgetedPlanNote`]; the subject is in the
/// sentence.
#[derive(Debug)]
pub(crate) struct Located<'a> {
    pub(crate) subject: &'a str,
    pub(crate) finding: &'a dyn Finding,
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
    pub(crate) subject: String,
    pub(crate) sink: Arc<dyn DiagnosticSink>,
}

/// A table of the dump its catalog does not list, because the library's plan
/// of it refuses — its `COPY` blocks naming different columns, say — with
/// that refusal. It is what [`crate::catalog::register_dump`] hands the sink in place
/// of the table's findings, and the only place its refusal is said: a query
/// naming the table finds no such table ([`crate::catalog::PgDumpCatalog`]).
#[derive(Debug)]
pub struct RefusedTable {
    /// The table, as the dump names it.
    pub table: TableName,
    /// What planning it raised.
    pub error: Error,
}

impl Finding for RefusedTable {
    /// An error: the dump holds rows no query of this session can reach.
    fn severity(&self) -> Severity {
        Severity::Error
    }

    fn message(&self) -> String {
        format!("not listed, since no query of it can be planned: {}", self.error)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl RefusedTable {
    /// Hand this refusal to `sink`, prefixed with `subject`, the name SQL
    /// would have reached the table by.
    pub(crate) fn report(&self, subject: &str, sink: &dyn DiagnosticSink) {
        sink.report(&Located { subject, finding: self });
    }
}

/// **The columns one registration compares bytewise because the dump records
/// no collation for them** ([`ComparisonDivergence::UnknownCollation`]), as
/// one finding in place of a [`ComparisonNote`] a column: on a real dump that
/// is every text column of every table. [`crate::register_dump`] reports one
/// for the dump, every catalog's tables in it, and [`crate::PgDumpTable::report`]
/// one for its table — the factory's registration of a statement's table.
/// None is reported where no column is uncollated.
///
/// Its message is the one sentence for them all, counting columns and tables;
/// a caller wanting its own reads the fields by downcasting
/// ([`Finding::as_any`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UncollatedColumns {
    /// What the registration is named by: the dump's origin, or the table as
    /// the statement registering it names it.
    pub subject: String,
    /// `(table, column)`, each table as SQL reaches it; a nested column
    /// diverging at several positions is one column.
    pub columns: BTreeSet<(String, String)>,
    /// Why each is compared bytewise, the library's own clause
    /// ([`ComparisonNote::bytewise_reason`]).
    pub reason: String,
}

impl Finding for UncollatedColumns {
    /// A warning, as each column's note would have been.
    fn severity(&self) -> Severity {
        Severity::Warning
    }

    fn message(&self) -> String {
        let tables = self.columns.iter().map(|(table, _)| table).collect::<BTreeSet<_>>().len();
        format!(
            "{}: {} column(s) in {tables} table(s) are each compared bytewise: {}",
            self.subject,
            self.columns.len(),
            self.reason
        )
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl UncollatedColumns {
    /// None yet, of the registration named `subject`.
    pub(crate) fn of(subject: &str) -> Self {
        Self { subject: subject.to_string(), columns: BTreeSet::new(), reason: String::new() }
    }

    /// Count `note` under `table` if it is an uncollated column's, saying
    /// whether it was.
    pub(crate) fn count(&mut self, table: &str, note: &ComparisonNote) -> bool {
        if note.divergence != ComparisonDivergence::UnknownCollation {
            return false;
        }
        self.columns.insert((table.to_string(), note.column.clone()));
        if self.reason.is_empty() {
            self.reason = note.bytewise_reason().unwrap_or_default().to_string();
        }
        true
    }

    /// Hand this to `sink`, if it counted any column.
    pub(crate) fn report(&self, sink: &dyn DiagnosticSink) {
        if !self.columns.is_empty() {
            sink.report(self);
        }
    }
}

/// A plan note quoting a budget ([`PlanNote::budget_bytes`]), with the
/// account of what that budget was carved from. The library states a budget and never where it
/// came from (`docs/design/decisions.md`, "D64"), so this is the provider's
/// half, as `pgdt`'s `plan_note_origin` is `pgdt`'s.
///
/// **Its message is the note's followed by one clause**: the allowance and
/// its origin, the three holdings that came off it, and the settings that
/// would move what the note reports — each a key a user types as it stands,
/// and one for each lever the note lists ([`PlanNote::levers`]), so a key
/// inert for this source and budget is never offered: `pgdump.memory` for a
/// larger allowance, with `datafusion.runtime.memory_limit` beside it where
/// the session's pool states a limit the allowance lost; `pgdump.chunk_size`
/// for a smaller read chunk; `datafusion.execution.target_partitions` for
/// fewer sub-streams. The levers are the library's, computed where the
/// budget is carved and the reader sized, so nothing here re-derives either.
/// What live scans drew is named and has no key (`KD38`). A caller wanting
/// its own words reads [`BudgetedPlanNote::note`] and
/// [`BudgetedPlanNote::account`] by downcasting ([`Finding::as_any`]).
///
/// *Rejected: an English clause appended in `report_plan`*, leaving an
/// embedder to parse prose; *the binary's sink wording it*, which has no
/// account to read; *every key on every note*, which tells the reader nothing
/// about this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetedPlanNote {
    /// The library's note, as its plan settled it.
    pub note: PlanNote,
    /// What the scan's budget was carved from, when the scan drew it.
    pub account: BudgetAccount,
}

impl Finding for BudgetedPlanNote {
    fn severity(&self) -> Severity {
        self.note.severity()
    }

    fn message(&self) -> String {
        format!("{} — {}", self.note.message(), self.clause())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl BudgetedPlanNote {
    /// Where the budget came from, and what moves it.
    fn clause(&self) -> String {
        let BudgetAccount { allowance, pool_limit, resident, drawn } = &self.account;
        let origin = match allowance {
            Allowance::Setting(_) => Some("stated by pgdump.memory".to_string()),
            Allowance::Stated(_) => Some("stated when the session's budget was built".to_string()),
            Allowance::Limit(limit) => {
                Some(format!("the limit {} states", limit.read_from.display()))
            }
            Allowance::HalfAvailable(_) => Some(
                "half of what the machine reports available, no memory limit being found"
                    .to_string(),
            ),
            Allowance::NoneFound => None,
        };
        let carved = match (allowance.bytes(), origin) {
            (Some(allowance), Some(origin)) => {
                let pool = match pool_limit {
                    0 => "the session's memory pool states no limit".to_string(),
                    bytes => format!("the session's memory pool is granted {bytes} byte(s)"),
                };
                format!(
                    "that budget was carved from an allowance of {allowance} resident byte(s), \
                     {origin}, against which {pool}, the attached dumps' statistics hold \
                     {resident} byte(s) and the scans still running had drawn {drawn} byte(s)"
                )
            }
            _ => "no allowance was found — no memory limit, and the machine's available memory \
                     could not be read — so that budget is the library's own and nothing the \
                     session holds came off it"
                .to_string(),
        };
        let mut keys = Vec::new();
        for lever in &self.note.levers {
            match lever {
                PlanLever::LargerAllowance => {
                    keys.push("pgdump.memory (the allowance)");
                    if allowance.bytes().is_some() && *pool_limit > 0 {
                        keys.push("datafusion.runtime.memory_limit (the pool's grant)");
                    }
                }
                PlanLever::SmallerReadChunk => keys.push("pgdump.chunk_size (the read chunk)"),
                PlanLever::FewerSubStreams => {
                    keys.push("datafusion.execution.target_partitions (the sub-streams asked for)");
                }
            }
        }
        match keys.as_slice() {
            [] => format!("{carved}; no setting moves it for this source and budget"),
            keys => format!("{carved}; the settings that move it: {}", keys.join(", ")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pgdt_query::PlanNoteKind;

    use super::*;

    fn account(allowance: Allowance, pool_limit: u64) -> BudgetAccount {
        BudgetAccount { allowance, pool_limit, resident: 7, drawn: 11 }
    }

    fn note(kind: PlanNoteKind, levers: &[PlanLever]) -> PlanNote {
        PlanNote { kind, levers: levers.to_vec() }
    }

    const ALLOWANCE: &str = "pgdump.memory";
    const POOL: &str = "datafusion.runtime.memory_limit";
    const CHUNK: &str = "pgdump.chunk_size";
    const COUNT: &str = "datafusion.execution.target_partitions";

    /// The keys the clause's list names, the origin's own words aside.
    fn keys(message: &str) -> Vec<&'static str> {
        let listed = message.split_once("the settings that move it: ").map_or("", |(_, list)| list);
        [ALLOWANCE, POOL, CHUNK, COUNT].into_iter().filter(|key| listed.contains(key)).collect()
    }

    /// **The note's sentence, then one clause**: the allowance and where it
    /// came from, every holding by its number, and a key for each lever the
    /// note lists and no other — the pool's beside the allowance's only where
    /// it states a limit.
    #[test]
    fn the_clause_names_the_account_and_the_keys_that_move_it() {
        let floor = note(
            PlanNoteKind::AllocationBelowFloor { unit_bytes: 8, memory_bytes: 0 },
            &[PlanLever::LargerAllowance],
        );
        let read_from = PathBuf::from("/sys/fs/cgroup/memory.max");
        let limit = Allowance::Limit(pgdt_io::MemoryLimit { bytes: 4 << 30, read_from });
        let wrapped = BudgetedPlanNote { note: floor.clone(), account: account(limit, 1 << 20) };
        let message = wrapped.message();
        assert!(message.starts_with(&format!("{} — ", floor.message())), "{message}");
        for term in [
            "an allowance of 4294967296 resident byte(s), the limit /sys/fs/cgroup/memory.max states",
            "the session's memory pool is granted 1048576 byte(s)",
            "statistics hold 7 byte(s)",
            "had drawn 11 byte(s)",
        ] {
            assert!(message.contains(term), "{term}: {message}");
        }
        assert_eq!(keys(&message), [ALLOWANCE, POOL], "{message}");
        assert_eq!(wrapped.severity(), floor.severity());

        let limited = BudgetedPlanNote {
            note: note(
                PlanNoteKind::ParallelismBudgetLimited {
                    requested: 4,
                    planned: 1,
                    footprint: 8,
                    max_source_span: None,
                    memory_bytes: 8,
                },
                &[PlanLever::LargerAllowance, PlanLever::SmallerReadChunk],
            ),
            account: account(Allowance::Setting(4 << 30), 0),
        };
        let message = limited.message();
        for term in ["stated by pgdump.memory", "the session's memory pool states no limit"] {
            assert!(message.contains(term), "{term}: {message}");
        }
        assert_eq!(keys(&message), [ALLOWANCE, CHUNK], "{message}");

        let narrowed = BudgetedPlanNote {
            note: note(
                PlanNoteKind::BatchSpanNarrowed {
                    stated_bytes: 16,
                    planned_bytes: 8,
                    workers: 2,
                    memory_bytes: 8,
                },
                &[PlanLever::SmallerReadChunk, PlanLever::FewerSubStreams],
            ),
            account: account(Allowance::HalfAvailable(4 << 30), 1 << 20),
        };
        let message = narrowed.message();
        assert!(message.contains("half of what the machine reports available"), "{message}");
        assert_eq!(keys(&message), [CHUNK, COUNT], "no allowance key, so no pool key: {message}");

        let unfound = BudgetedPlanNote {
            note: floor,
            account: BudgetAccount {
                allowance: Allowance::NoneFound,
                pool_limit: 1 << 20,
                resident: 0,
                drawn: 0,
            },
        };
        let message = unfound.message();
        assert!(message.contains("no allowance was found"), "{message}");
        assert_eq!(keys(&message), [ALLOWANCE], "{message}");
    }

    /// **A plain source's floor at `DEFAULT_MEMORY_BUDGET` offers the chunk
    /// alone**: no allowance raises that budget, so neither `pgdump.memory`
    /// nor the pool's key is named.
    #[test]
    fn a_floor_no_allowance_raises_names_only_the_chunk() {
        let capped = BudgetedPlanNote {
            note: note(
                PlanNoteKind::AllocationBelowFloor {
                    unit_bytes: 128 << 20,
                    memory_bytes: pgdt_io::DEFAULT_MEMORY_BUDGET,
                },
                &[PlanLever::SmallerReadChunk],
            ),
            account: account(Allowance::Setting(4 << 30), 1 << 20),
        };
        let message = capped.message();
        assert_eq!(keys(&message), [CHUNK], "{message}");
        let unmoved = BudgetedPlanNote {
            note: note(capped.note.kind.clone(), &[]),
            account: capped.account.clone(),
        };
        assert!(unmoved.message().ends_with("no setting moves it for this source and budget"));
    }
}
