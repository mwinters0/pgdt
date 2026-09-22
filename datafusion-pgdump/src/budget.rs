//! The session's byte budget: one allowance every pgdump scan the session runs
//! draws from (`docs/design/roadmap-P6-datafusion.md`, "Workers and memory").

use std::sync::{Arc, Mutex, OnceLock};

use datafusion::catalog::Session;
use datafusion::execution::memory_pool::MemoryLimit;
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
/// **What the session holds besides its scans is drawn too**, before any scan
/// draws: a memory pool stating a `Finite` limit — what `datafusion-cli
/// --memory-limit` grants DataFusion's own operators, in the same container —
/// and the statistics each dump registered against this budget holds in its
/// resident map, billed as `pgdt` bills the statistics a cache hands a mapping
/// pass ([`pgdump_query::DumpIndex::statistics_heap_bytes`]). Both come off
/// as another scan's draw would, off the cap and the margin's ceiling alike,
/// never off the allowance the margin is a fraction of: the container's limit
/// is still the whole allowance, and a margin taken of less would shrink with
/// every dump registered.
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
    held: Mutex<Held>,
}

/// What a [`ScanBudget`]'s holders have taken of it.
#[derive(Debug, Default)]
struct Held {
    /// The read-buffer budgets of the scans alive.
    scans: u64,
    /// The statistics of the dumps billed to it and not yet dropped.
    maps: u64,
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
    /// what DataFusion holds resident beyond any pool limit it states, which a
    /// `pgdt` process does not hold.
    pub fn discover() -> Self {
        let allowance = pgdump_query::discover_memory_limit()
            .map(|limit| limit.bytes)
            .or_else(|| pgdump_query::available_memory().map(|free| free / 2));
        Self { allowance, held: Mutex::default() }
    }

    /// A stated resident allowance, in bytes, carved exactly as a discovered
    /// one is.
    pub fn new(allowance: u64) -> Self {
        Self { allowance: Some(allowance), held: Mutex::default() }
    }

    /// The resident allowance, or `None` where nothing stated or discovered
    /// one.
    pub fn allowance(&self) -> Option<u64> {
        self.allowance
    }

    /// The read-buffer bytes the scans alive now hold between them.
    pub fn drawn(&self) -> u64 {
        self.held.lock().unwrap().scans
    }

    /// The statistics bytes the dumps billed to this budget hold in their
    /// resident maps between them, returned as each dump is dropped.
    pub fn resident(&self) -> u64 {
        self.held.lock().unwrap().maps
    }

    /// Bill `bytes` of a resident map's statistics to this budget until the
    /// returned [`Hold`] drops.
    pub(crate) fn hold(self: &Arc<Self>, bytes: u64) -> Hold {
        self.held.lock().unwrap().maps += bytes;
        Hold { budget: Arc::clone(self), bytes }
    }

    /// The arrangement one more scan takes: as many of `jobs` workers as what
    /// the live scans, the billed maps and `pool` — the session's memory
    /// pool's finite limit, [`pool_limit`] — leave affords, at what that many
    /// spend. Held until the returned [`Draw`] drops.
    ///
    /// Deficiency register: `deficiency: KD38` — first planned, first served:
    /// a join plans both its tables before either runs, so the first can take
    /// `target_partitions` readers and the whole allowance and the second one
    /// reader on nothing, the floor spending past the budget by its one slot.
    /// **(c) unowned.** Closing it means a share fixed before either draws —
    /// the pgdump scans of a physical plan counted and the allowance split
    /// between them — which nothing has yet measured a reason to build.
    pub(crate) fn draw(
        self: &Arc<Self>,
        jobs: usize,
        memory: Option<WorkerMemory>,
        pool: u64,
    ) -> Draw {
        let mut held = self.held.lock().unwrap();
        let others = held.scans.saturating_add(held.maps).saturating_add(pool);
        let parallelism = match self.allowance {
            Some(allowance) => Parallelism::within_shared(jobs, memory, allowance, others),
            None => Parallelism::discover_for(jobs, memory),
        };
        let bytes = match self.allowance {
            Some(_) => parallelism.memory_bytes().unwrap_or(0),
            None => 0,
        };
        held.scans += bytes;
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

/// The bytes `state`'s memory pool is limited to, where it states a `Finite`
/// limit; an unbounded or unknown pool grants DataFusion's operators nothing
/// the scans could otherwise have held, and draws nothing.
pub(crate) fn pool_limit(state: &dyn Session) -> u64 {
    match state.runtime_env().memory_pool.memory_limit() {
        MemoryLimit::Finite(bytes) => bytes as u64,
        MemoryLimit::Infinite | MemoryLimit::Unknown => 0,
    }
}

/// A resident map's statistics billed to a [`ScanBudget`], returned to it on
/// drop.
#[derive(Debug)]
pub(crate) struct Hold {
    budget: Arc<ScanBudget>,
    bytes: u64,
}

impl Hold {
    /// Whether this is `budget`'s.
    pub(crate) fn bills(&self, budget: &Arc<ScanBudget>) -> bool {
        Arc::ptr_eq(&self.budget, budget)
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        let mut held = self.budget.held.lock().unwrap();
        held.maps = held.maps.saturating_sub(self.bytes);
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
        let mut held = self.budget.held.lock().unwrap();
        held.scans = held.scans.saturating_sub(self.bytes);
    }
}
