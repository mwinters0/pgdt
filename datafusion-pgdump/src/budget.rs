//! The session's byte budget: one allowance every pgdump scan the session runs
//! draws from (`docs/design/roadmap-P6-datafusion.md`, "Workers and memory").

use std::sync::{Arc, Mutex, OnceLock};

use datafusion::catalog::Session;
use pgdump_query::{Parallelism, WorkerMemory};

/// What the pgdump scans of one DataFusion session may hold resident between
/// them, and how much of it the scans running now have drawn.
///
/// **One object per session**, set on its [`datafusion::prelude::SessionConfig`]
/// as an extension, the way DataFusion holds its own bounded shared objects:
/// a scan planned while others are live draws what they leave rather than the
/// whole, so a join of two dumps' tables is bounded by one number. The
/// allowance means what `pgdt --memory` means — resident, the number a
/// container is given — and each scan carves its read-buffer budget from it
/// through [`Parallelism::within_shared`], the reserve once for the process.
///
/// What a scan draws it holds until its plan and every stream it started are
/// dropped, not only while rows flow: a plan is a promise to run, and its
/// partition count is what the draw buys. *Rejected: drawing at `execute`*,
/// which would leave that count unpaid for — the library cuts a table from the
/// budget and the count at plan time (`docs/design/decisions.md`, "D84") and
/// refuses a unit rather than shrinking it ("D4").
#[derive(Debug)]
pub struct ScanBudget {
    allowance: Option<u64>,
    drawn: Mutex<u64>,
}

impl ScanBudget {
    /// The allowance this process runs under, found as `pgdt` finds it: the
    /// memory limit that binds, else half of what the machine reports
    /// available (`docs/design/roadmap.md`, "A default runs as fast as the
    /// allocation permits"). A host that answers neither leaves no allowance,
    /// and a scan then takes the library's own discovery for its count.
    ///
    /// **The halved figure is carved as an allowance, reserve and margin
    /// included**, where `pgdt` caps its buffers at it with neither, so the
    /// provider's budget there is the smaller: the reserve here stands for
    /// DataFusion's own resident memory, which a `pgdt` process does not hold.
    pub fn discover() -> Self {
        let allowance = pgdump_query::discover_memory_limit()
            .map(|limit| limit.bytes)
            .or_else(|| pgdump_query::available_memory().map(|free| free / 2));
        Self { allowance, drawn: Mutex::new(0) }
    }

    /// A stated resident allowance, in bytes, carved exactly as a discovered
    /// one is.
    pub fn new(allowance: u64) -> Self {
        Self { allowance: Some(allowance), drawn: Mutex::new(0) }
    }

    /// The resident allowance, or `None` where nothing stated or discovered
    /// one.
    pub fn allowance(&self) -> Option<u64> {
        self.allowance
    }

    /// The read-buffer bytes the scans alive now hold between them.
    pub fn drawn(&self) -> u64 {
        *self.drawn.lock().unwrap()
    }

    /// The arrangement one more scan takes: as many of `jobs` workers as what
    /// the live scans leave affords, at what that many spend. Held until the
    /// returned [`Draw`] drops.
    ///
    /// Deficiency register: `deficiency: KD38` — first planned, first served:
    /// a join plans both its tables before either runs, so the first can take
    /// `target_partitions` readers and the whole allowance and the second one
    /// reader on nothing, the floor spending past the budget by its one slot.
    /// **(c) unowned.** Closing it means a share fixed before either draws —
    /// the pgdump scans of a physical plan counted and the allowance split
    /// between them — which nothing has yet measured a reason to build.
    pub(crate) fn draw(self: &Arc<Self>, jobs: usize, memory: Option<WorkerMemory>) -> Draw {
        let mut drawn = self.drawn.lock().unwrap();
        let parallelism = match self.allowance {
            Some(allowance) => Parallelism::within_shared(jobs, memory, allowance, *drawn),
            None => Parallelism::discover_for(jobs, memory),
        };
        let bytes = match self.allowance {
            Some(_) => parallelism.memory_bytes().unwrap_or(0),
            None => 0,
        };
        *drawn += bytes;
        Draw { budget: Arc::clone(self), bytes, parallelism }
    }

    /// The budget `state` carries, or the process's own where a session was
    /// built without one — [`crate::register_dump`] installs one, so this is a
    /// provider registered by hand. Shared by every such session, which keeps
    /// the bound rather than multiplying it.
    pub(crate) fn of(state: &dyn Session) -> Arc<Self> {
        static PROCESS: OnceLock<Arc<ScanBudget>> = OnceLock::new();
        state
            .config()
            .get_extension::<ScanBudget>()
            .unwrap_or_else(|| Arc::clone(PROCESS.get_or_init(|| Arc::new(Self::discover()))))
    }
}

/// One scan's share of a [`ScanBudget`], returned to it on drop.
#[derive(Debug)]
pub(crate) struct Draw {
    budget: Arc<ScanBudget>,
    bytes: u64,
    parallelism: Parallelism,
}

impl Draw {
    /// The arrangement the scan runs under.
    pub(crate) fn parallelism(&self) -> Parallelism {
        self.parallelism
    }
}

impl Drop for Draw {
    fn drop(&mut self) {
        let mut drawn = self.budget.drawn.lock().unwrap();
        *drawn = drawn.saturating_sub(self.bytes);
    }
}
