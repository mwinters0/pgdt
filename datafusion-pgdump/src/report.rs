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
//! ([`crate::exec`]). **A plan note quoting a budget is wrapped in a
//! [`BudgetedPlanNote`]**, which says where that budget came from.

use std::any::Any;
use std::sync::Arc;

use pgdump_query::{
    ComparisonSemantics, DiagnosticSink, Finding, PlanLever, PlanNote, Severity, column_divergences,
};

use crate::budget::{AllowanceOrigin, BudgetAccount};
use crate::dump::PgDump;
use crate::table::PgDumpTable;

/// A finding with what it is about named in front of its sentence: the dump,
/// or a table as SQL names it. The library's findings name a column at most
/// (`pgdump_query::ColumnNote`, `pgdump_query::ComparisonNote`), and one sink
/// hears every table of every registered dump.
///
/// [`Finding::as_any`] is the wrapped finding's, so a sink downcasting to the
/// library's own record still reaches it — save a plan note quoting a budget,
/// which is reached inside its [`BudgetedPlanNote`]; the subject is in the
/// sentence.
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
    /// the scan is planned, one quoting a budget wrapped in a
    /// [`BudgetedPlanNote`]. A table reported again reports its scans to the
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
    /// to, if it was, each note quoting a budget wrapped with `account`, the
    /// scan's own.
    pub(crate) fn report_plan(&self, notes: &[PlanNote], account: &BudgetAccount) {
        // Taken out of the lock, so a sink that reports this table again
        // does not wait on itself.
        let Some(reporting) = self.reporting().lock().unwrap().clone() else { return };
        let subject = reporting.subject.as_str();
        for note in notes {
            if note.budget_bytes().is_some() {
                let finding = &BudgetedPlanNote { note: note.clone(), account: account.clone() };
                reporting.sink.report(&Located { subject, finding });
            } else {
                reporting.sink.report(&Located { subject, finding: note });
            }
        }
    }
}

/// A plan note quoting a budget ([`PlanNote::budget_bytes`]), with the
/// account of what that budget was carved from (`docs/design/roadmap-P6-datafusion.md`,
/// "Diagnostics: one sink"). The library states a budget and never where it
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
        let BudgetAccount { allowance, origin, pool_limit, resident, drawn } = &self.account;
        let carved = match allowance {
            Some(allowance) => {
                let origin = match origin {
                    AllowanceOrigin::Setting => "stated by pgdump.memory".to_string(),
                    AllowanceOrigin::Stated => {
                        "stated when the session's budget was built".to_string()
                    }
                    AllowanceOrigin::Limit { read_from } => {
                        format!("the limit {} states", read_from.display())
                    }
                    AllowanceOrigin::HalfAvailable => {
                        "half of what the machine reports available, no memory limit being found"
                            .to_string()
                    }
                    // `ScanBudget` never pairs an allowance with this origin.
                    AllowanceOrigin::NoneFound => "whose origin was not recorded".to_string(),
                };
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
            None => "no allowance was found — no memory limit, and the machine reports no free \
                     memory — so that budget is the library's own and nothing the session holds \
                     came off it"
                .to_string(),
        };
        let mut keys = Vec::new();
        for lever in &self.note.levers {
            match lever {
                PlanLever::LargerAllowance => {
                    keys.push("pgdump.memory (the allowance)");
                    if allowance.is_some() && *pool_limit > 0 {
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

    use pgdump_query::PlanNoteKind;

    use super::*;

    fn account(origin: AllowanceOrigin, pool_limit: u64) -> BudgetAccount {
        BudgetAccount { allowance: Some(4 << 30), origin, pool_limit, resident: 7, drawn: 11 }
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
        let wrapped = BudgetedPlanNote {
            note: floor.clone(),
            account: account(AllowanceOrigin::Limit { read_from }, 1 << 20),
        };
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
            account: account(AllowanceOrigin::Setting, 0),
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
                &[PlanLever::FewerSubStreams],
            ),
            account: account(AllowanceOrigin::HalfAvailable, 1 << 20),
        };
        let message = narrowed.message();
        assert!(message.contains("half of what the machine reports available"), "{message}");
        assert_eq!(keys(&message), [COUNT], "no allowance key, so no pool key: {message}");

        let unfound = BudgetedPlanNote {
            note: floor,
            account: BudgetAccount {
                allowance: None,
                origin: AllowanceOrigin::NoneFound,
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
                    memory_bytes: pgdump_query::DEFAULT_MEMORY_BUDGET,
                },
                &[PlanLever::SmallerReadChunk],
            ),
            account: account(AllowanceOrigin::Setting, 1 << 20),
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
