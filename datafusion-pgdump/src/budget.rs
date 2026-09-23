//! The session's byte budget: one allowance every pgdump scan the session runs
//! draws from (`docs/design/roadmap-P6-datafusion.md`, "Workers and memory").

use std::path::{Path, PathBuf};
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
/// **What the session holds besides its scans is billed too**, before any scan
/// draws: a memory pool stating a `Finite` limit — what `datafusion-cli
/// --memory-limit` grants DataFusion's own operators, in the same container —
/// and the statistics each dump registered against this budget holds in its
/// resident map, billed as `pgdt` bills the statistics a cache hands a mapping
/// pass ([`pgdump_query::DumpIndex::statistics_heap_bytes`]). Both come off
/// the margin's ceiling alone, as `held` in [`Parallelism::within_shared`]:
/// the cap is the reserve's, standing for the scans' own excess over their
/// budgets, and neither holding is any of it. So they lower a scan's count
/// first, and whatever of them the ceiling's room cannot absorb at that count
/// comes off its budget — a plain dump's scan, or one reader, whose count
/// cannot fall, still leaves the margin. *Rejected: taking them off the
/// allowance the margin is a fraction of*: the container's limit is still the
/// whole allowance, and a margin taken of less would let the predicted
/// resident leave less than [`pgdump_query::MEMORY_MARGIN_PERCENT`] of the
/// container unused, short by that fraction of what the session holds.
/// *Rejected: taking them off the cap as well*, as another scan's draw comes
/// off, which bills them against the reserve's excess a second time.
///
/// **An allowance the session states overrides the one the budget holds**,
/// read at each draw (`pgdump.memory`, [`crate::PgDumpSettings`]): one `SET`
/// mid-session binds the scans planned after it, and what live scans drew
/// stays drawn against it, as it would against the budget's own.
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
    origin: AllowanceOrigin,
    held: Mutex<Held>,
}

/// Where the allowance a scan's budget was carved from came from: the half of
/// a budget-quoting plan note the library does not tell, provenance being the
/// caller's fact (`docs/design/decisions.md`, "D64"), and what
/// [`crate::BudgetedPlanNote`] names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowanceOrigin {
    /// `pgdump.memory`, stated in the session ([`crate::PgDumpSettings`]).
    Setting,
    /// Stated by whoever built the budget ([`ScanBudget::new`]).
    Stated,
    /// A memory limit this process runs under, and the file that states it
    /// ([`pgdump_query::MemoryLimit`]).
    Limit { read_from: PathBuf },
    /// Half of what the machine reports available, no limit being found.
    HalfAvailable,
    /// Nothing: no limit found, and no free memory reported.
    NoneFound,
}

/// What one scan's budget was carved from, taken when the scan drew it: the
/// allowance and where it came from, and the three holdings that came off it
/// ([`ScanBudget::draw`]'s terms).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetAccount {
    /// The resident allowance carved, or `None` where nothing stated or
    /// discovered one, and the library's own discovery sized the scan.
    pub allowance: Option<u64>,
    /// Where [`BudgetAccount::allowance`] came from.
    pub origin: AllowanceOrigin,
    /// The session memory pool's `Finite` limit, `0` where it states none.
    pub pool_limit: u64,
    /// The statistics the dumps billed to the budget held resident.
    pub resident: u64,
    /// The read-buffer bytes the scans still alive had drawn.
    pub drawn: u64,
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
        Self::discover_in(Path::new("/"))
    }

    /// [`ScanBudget::discover`] against an arbitrary filesystem root, the
    /// seam [`pgdump_query::discover_memory_limit_in`] offers, so each origin
    /// can be pinned on a machine that has only one of them.
    pub fn discover_in(root: &Path) -> Self {
        let (allowance, origin) = match pgdump_query::discover_memory_limit_in(root) {
            Some(limit) => {
                (Some(limit.bytes), AllowanceOrigin::Limit { read_from: limit.read_from })
            }
            None => match pgdump_query::available_memory_in(root) {
                Some(free) => (Some(free / 2), AllowanceOrigin::HalfAvailable),
                None => (None, AllowanceOrigin::NoneFound),
            },
        };
        Self { allowance, origin, held: Mutex::default() }
    }

    /// A stated resident allowance, in bytes, carved exactly as a discovered
    /// one is.
    pub fn new(allowance: u64) -> Self {
        Self { allowance: Some(allowance), origin: AllowanceOrigin::Stated, held: Mutex::default() }
    }

    /// Where [`ScanBudget::allowance`] came from.
    pub fn origin(&self) -> &AllowanceOrigin {
        &self.origin
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
    /// pool's finite limit, [`pool_limit`] — leave of `stated`, else of this
    /// budget's own allowance, affords, at what that many spend less whatever
    /// of the holdings the margin cannot fit beside them
    /// ([`Parallelism::within_shared`]). Held until the returned [`Draw`]
    /// drops, which keeps the [`BudgetAccount`] of what it was carved from.
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
        stated: Option<u64>,
    ) -> Draw {
        let mut held = self.held.lock().unwrap();
        let holdings = held.maps.saturating_add(pool);
        let allowance = stated.or(self.allowance);
        let origin = match stated {
            Some(_) => AllowanceOrigin::Setting,
            None => self.origin.clone(),
        };
        let account = BudgetAccount {
            allowance,
            origin,
            pool_limit: pool,
            resident: held.maps,
            drawn: held.scans,
        };
        let parallelism = match allowance {
            Some(allowance) => {
                Parallelism::within_shared(jobs, memory, allowance, held.scans, holdings)
            }
            None => Parallelism::discover_for(jobs, memory),
        };
        let bytes = match allowance {
            Some(_) => parallelism.memory_bytes().unwrap_or(0),
            None => 0,
        };
        held.scans += bytes;
        Draw { budget: Arc::clone(self), bytes, parallelism, account }
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
    account: BudgetAccount,
}

impl Draw {
    /// The arrangement the scan runs under.
    pub(crate) fn parallelism(&self) -> Parallelism {
        self.parallelism
    }

    /// What the scan's budget was carved from.
    pub(crate) fn account(&self) -> &BudgetAccount {
        &self.account
    }
}

impl Drop for Draw {
    fn drop(&mut self) {
        let mut held = self.budget.held.lock().unwrap();
        held.scans = held.scans.saturating_sub(self.bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::execution::runtime_env::RuntimeEnvBuilder;
    use datafusion::prelude::{SessionConfig, SessionContext};

    /// **The session's holdings come off the margin's ceiling, and off a
    /// budget only where the count cannot fall**: a resident map, or a pool
    /// limit, lowers a scan's count exactly as a draw of the same size would
    /// where the ceiling binds, leaving the budget what the count spends;
    /// holdings filling the margin leave one reader on nothing, as scans
    /// having drawn the whole do.
    #[test]
    fn holdings_lower_the_count_before_the_budget() {
        let per_worker = 1u64 << 30;
        let memory = Some(WorkerMemory::per_worker(per_worker));
        let allowance = 10u64 << 30;

        let alone = Arc::new(ScanBudget::new(allowance));
        assert_eq!(alone.draw(24, memory, 0, None).parallelism().jobs(), 7);

        let mapped = Arc::new(ScanBudget::new(allowance));
        let _map = mapped.hold(2 << 30);
        let pooled = Arc::new(ScanBudget::new(allowance));
        let scanned = Arc::new(ScanBudget::new(allowance));
        let _scan = scanned.draw(2, memory, 0, None);
        for budget in [&mapped, &pooled, &scanned] {
            let pool = if Arc::ptr_eq(budget, &pooled) { 2 << 30 } else { 0 };
            let draw = budget.draw(24, memory, pool, None);
            assert_eq!(draw.parallelism().jobs(), 5);
            assert_eq!(draw.parallelism().memory_bytes(), Some(5 * per_worker));
        }

        let full = Arc::new(ScanBudget::new(allowance));
        let _map = full.hold(allowance);
        let draw = full.draw(4, memory, 0, None);
        assert_eq!(draw.parallelism(), Parallelism::Serial { memory_bytes: Some(0) });
        assert_eq!(full.drawn(), 0);

        let drained = Arc::new(ScanBudget::new(allowance));
        let _all = drained.draw(24, Some(WorkerMemory::per_worker(allowance)), 0, None);
        let starved = drained.draw(4, memory, 0, None);
        assert_eq!(starved.parallelism(), Parallelism::Serial { memory_bytes: Some(0) });
    }

    /// **A stated allowance is carved in place of the budget's own**, at each
    /// draw: larger or smaller, it decides the count, and what a live scan
    /// drew under the one stays drawn under the other.
    #[test]
    fn a_stated_allowance_overrides_the_budget_s_own_at_each_draw() {
        let per_worker = 1u64 << 30;
        let memory = Some(WorkerMemory::per_worker(per_worker));
        let budget = Arc::new(ScanBudget::new(10 << 30));
        assert_eq!(budget.draw(24, memory, 0, None).parallelism().jobs(), 7);
        assert_eq!(budget.draw(24, memory, 0, Some(20 << 30)).parallelism().jobs(), 15);
        let first = budget.draw(24, memory, 0, Some(5 << 30));
        assert_eq!(first.parallelism().jobs(), 3);
        assert_eq!(budget.drawn(), 3 * per_worker);
        // The live draw comes off the next one's, whichever allowance it is.
        assert_eq!(budget.draw(24, memory, 0, None).parallelism().jobs(), 4);
        assert_eq!(budget.allowance(), Some(10 << 30));

        let unfound = Arc::new(ScanBudget {
            allowance: None,
            origin: AllowanceOrigin::NoneFound,
            held: Mutex::default(),
        });
        let stated = unfound.draw(24, memory, 0, Some(10 << 30));
        assert_eq!(stated.parallelism().jobs(), 7);
        assert_eq!(unfound.drawn(), 7 * per_worker);
    }

    /// **A discovered allowance keeps where it came from**: the limit and the
    /// file stating it, else half of what the machine reports available, else
    /// nothing; a stated one is `Stated`, and a `pgdump.memory` passed to a
    /// draw is `Setting` for that draw alone.
    #[test]
    fn an_allowance_keeps_its_origin() {
        let root = tempfile::tempdir().unwrap();
        let write = |path: &str, text: &str| {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        let found = ScanBudget::discover_in(root.path());
        assert_eq!((found.allowance(), found.origin()), (None, &AllowanceOrigin::NoneFound));

        write("proc/meminfo", "MemAvailable: 1024 kB\n");
        let found = ScanBudget::discover_in(root.path());
        assert_eq!(
            (found.allowance(), found.origin()),
            (Some(512 << 10), &AllowanceOrigin::HalfAvailable)
        );

        write("proc/self/cgroup", "0::/svc\n");
        write("sys/fs/cgroup/svc/memory.max", "1073741824\n");
        let found = ScanBudget::discover_in(root.path());
        let read_from = root.path().join("sys/fs/cgroup/svc/memory.max");
        assert_eq!(
            (found.allowance(), found.origin()),
            (Some(1 << 30), &AllowanceOrigin::Limit { read_from })
        );

        let stated = Arc::new(ScanBudget::new(10 << 30));
        assert_eq!(stated.origin(), &AllowanceOrigin::Stated);
        let draw = stated.draw(2, None, 0, Some(4 << 30));
        assert_eq!(draw.account().origin, AllowanceOrigin::Setting);
        assert_eq!(draw.account().allowance, Some(4 << 30));
        assert_eq!(stated.draw(2, None, 0, None).account().origin, AllowanceOrigin::Stated);
    }

    /// **A draw's account is what the budget held when it drew**: the pool's
    /// limit, the maps billed and the scans alive before it, its own draw not
    /// among them.
    #[test]
    fn a_draw_s_account_is_what_it_was_carved_against() {
        let per_worker = 1u64 << 30;
        let memory = Some(WorkerMemory::per_worker(per_worker));
        let budget = Arc::new(ScanBudget::new(10 << 30));
        let _map = budget.hold(1 << 20);
        let first = budget.draw(2, memory, 0, None);
        assert_eq!(
            first.account(),
            &BudgetAccount {
                allowance: Some(10 << 30),
                origin: AllowanceOrigin::Stated,
                pool_limit: 0,
                resident: 1 << 20,
                drawn: 0,
            }
        );
        let second = budget.draw(2, memory, 128 << 20, None);
        assert_eq!(second.account().pool_limit, 128 << 20);
        assert_eq!(second.account().drawn, 2 * per_worker);
    }

    /// A pool stating a `Finite` limit bills that limit; the default,
    /// unbounded pool bills nothing.
    #[test]
    fn a_finite_pool_limit_is_read_off_the_session() {
        let limit = 128usize << 20;
        let runtime = RuntimeEnvBuilder::new().with_memory_limit(limit, 1.0).build_arc().unwrap();
        let bounded = SessionContext::new_with_config_rt(SessionConfig::new(), runtime);
        assert_eq!(pool_limit(&bounded.state()), limit as u64);
        assert_eq!(pool_limit(&SessionContext::new().state()), 0);
    }
}
