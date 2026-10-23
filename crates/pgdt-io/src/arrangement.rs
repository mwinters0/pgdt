//! The resolution: a resident allowance, stated or discovered, a source's
//! recommendation and what is already held, resolved into the arrangement a
//! pass runs under — the one rule `pgdt` and the DataFusion provider both call.

use std::path::Path;

use crate::ByteRangeSource;
use crate::memory::{
    DEFAULT_MEMORY_BUDGET, MemoryLimit, available_memory_in, discover_memory_limit_in,
    statistics_allowance,
};
use crate::parallelism::{Parallelism, WorkerMemory};

/// A resident allowance — the number a container is given — and where it
/// came from.
///
/// **Provenance is carried here and never into [`Parallelism`]**
/// (`docs/design/decisions.md`, "D64"): a front end's status line and a plan
/// note's clause read it beside the arrangement, while every pool reads only
/// the budget carved from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Allowance {
    /// Stated by whoever resolves: `pgdt --memory`, or the allowance a
    /// DataFusion session's budget was built with.
    Stated(u64),
    /// Stated by a setting for the arrangements resolved under it, in place
    /// of the caller's own — `datafusion-pgdump`'s `pgdump.memory`.
    Setting(u64),
    /// A memory limit this process runs under, and the file stating it.
    Limit(MemoryLimit),
    /// Half of what the machine reports available, no limit being found
    /// (`docs/design/runtime-invariants.md`, `RT8`).
    HalfAvailable(u64),
    /// Nothing: no limit found, and the machine's available memory
    /// unreadable.
    NoneFound,
}

impl Allowance {
    /// The allowance this process runs under: the memory limit that binds,
    /// else half of what the machine reports available, else none — **read
    /// once**, so every arrangement resolved from it carves the one number its
    /// caller announced (`docs/design/decisions.md`, "D11", "D64").
    ///
    /// Taking it is opting in: nothing in the library discovers unasked
    /// (`docs/design/decisions.md`, "D1").
    pub fn discover() -> Self {
        discover_allowance_in(Path::new("/"))
    }

    /// The resident bytes, or `None` where nothing stated or discovered any.
    pub fn bytes(&self) -> Option<u64> {
        match self {
            Self::Stated(bytes) | Self::Setting(bytes) | Self::HalfAvailable(bytes) => Some(*bytes),
            Self::Limit(limit) => Some(limit.bytes),
            Self::NoneFound => None,
        }
    }
}

/// [`Allowance::discover`] against an arbitrary filesystem root, the seam
/// [`discover_memory_limit_in`] is (`docs/design/decisions.md`, "D11").
pub fn discover_allowance_in(root: &Path) -> Allowance {
    match discover_memory_limit_in(root) {
        Some(limit) => Allowance::Limit(limit),
        None => match available_memory_in(root) {
            Some(available) => Allowance::HalfAvailable(available / 2),
            None => Allowance::NoneFound,
        },
    }
}

/// A worker count, and whether it may be lowered to fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jobs {
    /// A count somebody stated, never lowered: what the allowance affords
    /// still binds through the budget beside it (`pgdt_io`'s `worker_count`).
    Stated(usize),
    /// A recommendation — a source's, or a session's `target_partitions` —
    /// lowered to what the allowance affords (`docs/design/roadmap.md`, "A
    /// default runs as fast as the allocation permits").
    Recommended(usize),
}

/// What a caller asks of [`Arrangement::resolve`] besides the allowance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrangementRequest {
    /// The worker count, stated or recommended.
    pub jobs: Jobs,
    /// What one of those workers holds, where the source recommends a cost
    /// ([`ByteRangeSource::default_worker_memory`]).
    pub worker_memory: Option<WorkerMemory>,
    /// The read-buffer budget other arrangements drawing on the same
    /// allowance hold now — concurrent scans in one process.
    pub drawn: u64,
    /// What the process holds resident besides any arrangement: a cache's
    /// loaded statistics, or memory granted to something other than a scan.
    /// It comes off the margin's ceiling alone, where the allowance is carved
    /// with one (`docs/design/decisions.md`, "D85").
    pub held: u64,
    /// Whether [`Allowance::HalfAvailable`] is carved as a stated allowance
    /// is, reserve and margin included, rather than capping the pools alone.
    /// A process hosting more than its scans — DataFusion's operators past
    /// any pool limit they state — sets it, the reserve then standing for
    /// that; `pgdt` holds nothing of the kind and leaves it off, the halving
    /// being its margin.
    pub carve_estimate: bool,
}

impl ArrangementRequest {
    /// A recommended count of `jobs` workers, each holding `worker_memory`,
    /// with nothing drawn or held.
    pub fn recommended(jobs: usize, worker_memory: Option<WorkerMemory>) -> Self {
        Self {
            jobs: Jobs::Recommended(jobs),
            worker_memory,
            drawn: 0,
            held: 0,
            carve_estimate: false,
        }
    }

    /// What `source` recommends, with `stated` in place of its worker count
    /// where somebody stated one — the source is asked for a count only where
    /// none was (`docs/design/decisions.md`, "D64").
    pub fn of(source: &dyn ByteRangeSource, stated: Option<usize>) -> Self {
        let jobs = match stated {
            Some(jobs) => Jobs::Stated(jobs),
            None => Jobs::Recommended(source.default_workers()),
        };
        Self { jobs, ..Self::recommended(1, source.default_worker_memory()) }
    }
}

/// What one pass runs under, resolved from an allowance: the
/// [`Parallelism`], what a gathering pass's statistics may hold, and where
/// the allowance came from.
///
/// **No `Default`, and no constructor but [`Self::resolve`]**, so no caller
/// pairs a parallelism with an allowance not carved from it: a parallelism
/// stated alone is `Allowance::NoneFound` with `Jobs::Stated`, declining
/// nothing (`docs/design/decisions.md`, "D1", "D83").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arrangement {
    parallelism: Parallelism,
    allowance: Allowance,
    recommended_jobs: Option<usize>,
    statistics_allowance: Option<u64>,
}

impl Arrangement {
    /// Resolve `request` inside `allowance`.
    ///
    /// **A stated or discovered allowance is carved one way**
    /// (`docs/design/decisions.md`, "D83"): [`MEMORY_RESERVE`](crate::internal::MEMORY_RESERVE)
    /// off the top, what is `drawn` off the cap and the margin's ceiling, what
    /// is `held` off the ceiling alone, the count solved against the tighter
    /// and the budget what that count spends (`Parallelism::within_shared`).
    ///
    /// **An estimate is not an allowance** unless the request says to carve
    /// it: half of what the machine reports available caps the pools with no
    /// reserve and no margin, and nothing drawn or held changes it, the
    /// halving being the margin there; with nothing found, a recommendation
    /// stands whole (`KD113`). A source recommending no cost, under no limit,
    /// is [`Parallelism::default`] at a serial count and
    /// [`DEFAULT_MEMORY_BUDGET`] above one — **the composition is not a
    /// `min`**, which would collapse a compressed scan to serial on an
    /// unlimited host (`docs/design/decisions.md`, "D3").
    ///
    /// **A stated count is kept**, with the budget the count the allowance
    /// affords would spend beside it.
    pub fn resolve(allowance: &Allowance, request: ArrangementRequest) -> Self {
        let ArrangementRequest { jobs, worker_memory, drawn, held, carve_estimate } = request;
        let asked = match jobs {
            Jobs::Stated(jobs) | Jobs::Recommended(jobs) => jobs,
        };
        let carved = match allowance {
            Allowance::Stated(bytes) | Allowance::Setting(bytes) => {
                Parallelism::within_shared(asked, worker_memory, *bytes, drawn, held)
            }
            Allowance::Limit(limit) => {
                Parallelism::within_shared(asked, worker_memory, limit.bytes, drawn, held)
            }
            Allowance::HalfAvailable(bytes) if carve_estimate => {
                Parallelism::within_shared(asked, worker_memory, *bytes, drawn, held)
            }
            Allowance::HalfAvailable(bytes) => estimated(asked, worker_memory, *bytes),
            Allowance::NoneFound => estimated(asked, worker_memory, u64::MAX),
        };
        let parallelism = match (jobs, carved.memory_bytes()) {
            (Jobs::Stated(stated), Some(bytes)) => Parallelism::workers(stated, bytes),
            _ => carved,
        };
        let statistics_allowance = allowance.bytes().map(|bytes| {
            statistics_allowance(bytes, parallelism.budget_bytes()).saturating_sub(drawn)
        });
        let recommended_jobs = match jobs {
            Jobs::Stated(_) => None,
            Jobs::Recommended(jobs) => Some(jobs),
        };
        Self { parallelism, allowance: allowance.clone(), recommended_jobs, statistics_allowance }
    }

    /// The arrangement the library's passes are handed.
    pub fn parallelism(&self) -> Parallelism {
        self.parallelism
    }

    /// The allowance it was carved from, and where that came from.
    pub fn allowance(&self) -> &Allowance {
        &self.allowance
    }

    /// The count the request recommended, before the allowance lowered it,
    /// or `None` where the count was stated.
    pub fn recommended_jobs(&self) -> Option<usize> {
        self.recommended_jobs
    }

    /// **What a gathering pass's statistics may hold**: what the margin
    /// leaves once the read-buffer budget and what others drew come off it,
    /// carved after the workers (`docs/design/decisions.md`, "D85"), which
    /// `pgdt_mapping::map_file` and `pgdt_mapping::backfill_block` bound their
    /// statistics by. `None`, where no allowance was found, declines nothing.
    pub fn statistics_allowance(&self) -> Option<u64> {
        self.statistics_allowance
    }
}

/// The arrangement under an estimate rather than an allowance: `cap` bounds
/// the pools, with no reserve and no margin ([`Arrangement::resolve`]).
fn estimated(jobs: usize, memory: Option<WorkerMemory>, cap: u64) -> Parallelism {
    if memory.is_none() {
        // Nothing recommended: no recommendation to cap, which is the
        // library's own default.
        return if jobs > 1 {
            Parallelism::workers(jobs, DEFAULT_MEMORY_BUDGET)
        } else {
            Parallelism::default()
        };
    }
    // deficiency: KD113 — with `MemAvailable` unreadable the cap is
    // `u64::MAX`, so the recommendation stands whole, at the count asked for,
    // and `datafusion-pgdump`'s `ScanBudget`, holding no allowance, lets each
    // scan resolve alone, sharing nothing. The standing rule is that unstated
    // is not unlimited (`docs/design/roadmap.md`, "A default runs as fast as
    // the allocation permits"); the fix is a choice between
    // `DEFAULT_MEMORY_BUDGET` here and refusing a recommendation.
    let (jobs, budget) = Parallelism::fit(jobs, memory, cap, None);
    Parallelism::workers(jobs, budget)
}

#[cfg(test)]
mod tests;
