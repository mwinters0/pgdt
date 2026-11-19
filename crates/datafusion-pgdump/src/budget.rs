//! The session's byte budget: one allowance every pgdump scan the session runs
//! draws from.

use std::sync::{Arc, Mutex, OnceLock};

use datafusion::catalog::Session;
use datafusion::execution::memory_pool::MemoryLimit;
use datafusion::prelude::SessionContext;
use pgdt_io::{Allowance, Arrangement, ArrangementRequest, Jobs, WorkerMemory};

/// What the pgdump scans of one DataFusion session may hold resident between
/// them, and how much of it the scans running now have drawn.
///
/// **One object per session**, set on its [`datafusion::prelude::SessionConfig`]
/// as an extension — a provider registered by hand into a session without one
/// sharing the process's (`ScanBudget::of`) — the way DataFusion holds its own bounded shared objects:
/// a scan planned while others are live draws what they leave rather than the
/// whole, so a join of two dumps' tables is bounded by one number.
/// *Rejected: a budget per registered table*, which no DataFusion source has
/// and which multiplies by the tables a join names; *and no budget*, sizing
/// each scan by its worker count, which gives up bounded memory on a
/// compressed source. DataFusion has nothing to divide instead: its memory
/// pool does not cover data sources, and nothing in it reads a cgroup — as
/// of DataFusion 55.1.0, which no test here checks. The
/// allowance means what `pgdt --memory` means — resident, the number a
/// container is given — and each scan carves its read-buffer budget from it
/// through [`Arrangement::resolve`], as `pgdt` does, the reserve once for the
/// process.
///
/// **What the session holds besides its scans is billed too**, before any scan
/// draws: a memory pool stating a `Finite` limit — what `datafusion-cli
/// --memory-limit` grants DataFusion's own operators, in the same container —
/// and the statistics each dump registered against this budget holds in its
/// resident map, billed as `pgdt` bills the statistics a cache hands a mapping
/// pass ([`pgdt_model::DumpIndex::statistics_heap_bytes`]), and each live
/// scan's longest line, which its own ledger keeps room for
/// (`docs/design/decisions.md`, "D160"), so no later scan's ledger lends it
/// again, and what the process's allocator keeps of what it frees, where the
/// binary choosing it states that ([`ScanBudget::retaining`],
/// `docs/design/decisions.md`, "D165"). Where an allowance was found, all
/// four come off the margin's ceiling alone, as [`ArrangementRequest::held`] and its `retained` do; with none found there is no
/// margin and they come off nothing:
/// the cap is the reserve's, standing for the scans' own excess over their
/// budgets, and none of them is any of it. So they lower a scan's count
/// first, and whatever of them the ceiling's room cannot absorb at that count
/// comes off its budget — a plain dump's scan, or one reader, whose count
/// cannot fall, still leaves the margin, or where one reader alone overruns it,
/// overruns it no further. The statistics, the line charges and the
/// allocator's retention come off what a scan's ledger leaves one reader
/// too, the pool's grant never
/// (`docs/design/decisions.md`, "D161"). *Rejected: taking them off the
/// allowance the margin is a fraction of*: the container's limit is still the
/// whole allowance, and a margin taken of less would let the predicted
/// resident leave less than `pgdt_io`'s margin percentage of the
/// container unused, short by that fraction of what the session holds.
/// *Rejected: taking them off the cap as well*, as another scan's draw comes
/// off, which bills them against the reserve's excess a second time; *and a
/// plain source left alone*, what a plain budget is worth being `KD25`'s and
/// `KD32`'s question.
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
    allowance: Allowance,
    /// What the process's allocator keeps of what it frees, as its binary
    /// states it ([`ScanBudget::retaining`]).
    retained: u64,
    held: Mutex<Held>,
}

/// What one scan's budget was carved from, taken when the scan drew it: the
/// allowance and where it came from — the half of a budget-quoting plan note
/// the library does not tell, provenance being the caller's fact
/// (`docs/design/decisions.md`, "D64"), and what [`crate::BudgetedPlanNote`]
/// names — and the holdings that came off it ([`ScanBudget::draw`]'s
/// terms).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetAccount {
    /// The resident allowance carved, and where it came from:
    /// [`Allowance::Setting`] for `pgdump.memory` ([`crate::PgDumpSettings`]),
    /// [`Allowance::Stated`] for the number the budget was built with
    /// ([`ScanBudget::new`]), else what [`ScanBudget::discover`] found.
    pub allowance: Allowance,
    /// The session memory pool's `Finite` limit, `0` where it states none.
    pub pool_limit: u64,
    /// The statistics the dumps billed to the budget held resident.
    pub resident: u64,
    /// The read-buffer bytes the scans still alive had drawn.
    pub drawn: u64,
    /// The longest-line charges the scans still alive had carved
    /// (`pgdt_query::Table::line_charge`), each held off the margin's
    /// ceiling for as long as its scan lives.
    pub lines: u64,
    /// What the process's allocator is billed for keeping of what it frees
    /// ([`ScanBudget::retaining`]), `0` where its binary states none.
    pub retained: u64,
}

/// What a [`ScanBudget`]'s holders have taken of it.
#[derive(Debug, Default)]
struct Held {
    /// The read-buffer budgets of the scans alive.
    scans: u64,
    /// The longest-line charges the scans alive carved.
    lines: u64,
    /// The statistics of the dumps billed to it and not yet dropped.
    maps: u64,
}

impl ScanBudget {
    /// The allowance this process runs under, found as `pgdt` finds it
    /// ([`Allowance::discover`]): the memory limit that binds, else half of
    /// what the machine reports available (`docs/design/roadmap.md`, "A
    /// default runs as fast as the allocation permits"). A host that answers
    /// neither leaves no allowance, and its scans draw on
    /// [`pgdt_io::DEFAULT_MEMORY_BUDGET`] between them instead, an estimate
    /// with no reserve and no margin ([`Arrangement::resolve`]).
    ///
    /// **The halved figure is carved as an allowance, reserve and margin
    /// included** ([`ArrangementRequest::carve_estimate`]), where `pgdt`
    /// leaves it uncarved: the reserve here stands for what DataFusion holds resident
    /// beyond any pool limit it states, which a `pgdt` process does not hold.
    pub fn discover() -> Self {
        Self::of_allowance(Allowance::discover())
    }

    /// A stated resident allowance, in bytes, carved exactly as a discovered
    /// one is.
    pub fn new(allowance: u64) -> Self {
        Self::of_allowance(Allowance::Stated(allowance))
    }

    fn of_allowance(allowance: Allowance) -> Self {
        Self { allowance, retained: 0, held: Mutex::default() }
    }

    /// **This budget, billing every scan `bytes` the process's allocator
    /// keeps resident of what it frees**: held beside the scans for the
    /// session's whole life, off the margin's ceiling and off what a scan's
    /// ledger leaves one reader, as a resident map is. The binary that chose
    /// the allocator states it — `pgdt sql` for mimalloc's purge delay — and
    /// an embedder states its own allocator's, or nothing, which bills
    /// nothing (`docs/design/decisions.md`, "D13", "D165").
    pub fn retaining(self, bytes: u64) -> Self {
        Self { retained: bytes, ..self }
    }

    /// What every scan is billed for the allocator's retention
    /// ([`ScanBudget::retaining`]).
    pub fn retained(&self) -> u64 {
        self.retained
    }

    /// The resident allowance, and where it came from.
    pub fn allowance(&self) -> &Allowance {
        &self.allowance
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
    /// the live scans' budgets and line charges, the billed maps, the
    /// allocator's retention ([`ScanBudget::retaining`]) and `pool` —
    /// the session's memory pool's finite limit, [`pool_limit`] — leave of
    /// `stated`, else of this
    /// budget's own allowance, affords, at what that many spend less whatever
    /// of the holdings the margin cannot fit beside them
    /// ([`Arrangement::resolve`]), `line_charge` — the scan's longest line
    /// at its query's factor (`pgdt_query::Table::line_charge`) — carved
    /// before its readers. **The scan's ledger is carved here**, inside its
    /// draw, from what the margin's ceiling leaves its budget and the live
    /// scans' (`docs/design/decisions.md`, "D157"), the live scans' line
    /// charges opening its statistics' draw as the other holdings do, so it
    /// lends none of their bytes. Budget and line charge are held until the
    /// returned [`Draw`] drops, which keeps the arrangement and the
    /// [`BudgetAccount`] of what it was carved from.
    ///
    /// Deficiency register: `deficiency: KD38` — first planned, first served:
    /// a join plans both its tables before either runs, so the first can take
    /// `target_partitions` readers and the whole allowance and the second
    /// nothing — one reader where it states a cost per worker, its own count
    /// where it states none — the pools spending past the budget at their floor.
    /// **(c) unowned.** Closing it means a share fixed before either draws —
    /// the pgdump scans of a physical plan counted and the allowance split
    /// between them — which nothing has yet measured a reason to build.
    pub(crate) fn draw(
        self: &Arc<Self>,
        jobs: usize,
        memory: Option<WorkerMemory>,
        pool: u64,
        stated: Option<u64>,
        line_charge: u64,
    ) -> Draw {
        let mut held = self.held.lock().unwrap();
        let allowance = stated.map_or_else(|| self.allowance.clone(), Allowance::Setting);
        let request = ArrangementRequest {
            jobs: Jobs::Recommended(jobs),
            worker_memory: memory,
            drawn: held.scans,
            statistics: held.maps,
            granted: pool,
            lines: held.lines,
            retained: self.retained,
            carve_estimate: true,
            line_charge,
        };
        let arrangement = Arrangement::resolve(&allowance, request);
        // Nothing found draws on `DEFAULT_MEMORY_BUDGET`, the session's
        // estimate there (`docs/design/decisions.md`, "D11").
        let bytes = arrangement.parallelism().memory_bytes().unwrap_or(0);
        let account = BudgetAccount {
            allowance,
            pool_limit: pool,
            resident: held.maps,
            drawn: held.scans,
            lines: held.lines,
            retained: self.retained,
        };
        held.scans += bytes;
        held.lines += line_charge;
        Draw { budget: Arc::clone(self), bytes, line_charge, arrangement, account }
    }

    /// The budget `state` carries, or the process's own where a session was
    /// built without one — [`crate::session::install`] installs one, so this is a
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

/// `ctx`'s [`ScanBudget`], installed discovered from the process's allowance
/// where the session carries none yet ([`crate::session::install`]).
pub(crate) fn session_budget(ctx: &SessionContext) -> Arc<ScanBudget> {
    let state = ctx.state_ref();
    let mut state = state.write();
    match state.config().get_extension::<ScanBudget>() {
        Some(budget) => budget,
        None => {
            let budget = Arc::new(ScanBudget::discover());
            state.config_mut().set_extension(Arc::clone(&budget));
            budget
        }
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
    line_charge: u64,
    arrangement: Arrangement,
    account: BudgetAccount,
}

impl Draw {
    /// The arrangement the scan runs under: its parallelism, and the ledger
    /// its held lines draw on.
    pub(crate) fn arrangement(&self) -> &Arrangement {
        &self.arrangement
    }

    /// Its parallelism.
    #[cfg(test)]
    fn parallelism(&self) -> pgdt_io::Parallelism {
        self.arrangement.parallelism()
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
        held.lines = held.lines.saturating_sub(self.line_charge);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::execution::runtime_env::RuntimeEnvBuilder;
    use datafusion::prelude::{SessionConfig, SessionContext};
    use pgdt_io::Parallelism;

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
        assert_eq!(alone.draw(24, memory, 0, None, 0).parallelism().jobs(), 7);

        let mapped = Arc::new(ScanBudget::new(allowance));
        let _map = mapped.hold(2 << 30);
        let pooled = Arc::new(ScanBudget::new(allowance));
        let scanned = Arc::new(ScanBudget::new(allowance));
        let _scan = scanned.draw(2, memory, 0, None, 0);
        for budget in [&mapped, &pooled, &scanned] {
            let pool = if Arc::ptr_eq(budget, &pooled) { 2 << 30 } else { 0 };
            let draw = budget.draw(24, memory, pool, None, 0);
            assert_eq!(draw.parallelism().jobs(), 5);
            assert_eq!(draw.parallelism().memory_bytes(), Some(5 * per_worker));
        }

        let full = Arc::new(ScanBudget::new(allowance));
        let _map = full.hold(allowance);
        let draw = full.draw(4, memory, 0, None, 0);
        assert_eq!(draw.parallelism(), Parallelism::Serial { memory_bytes: Some(0) });
        assert_eq!(full.drawn(), 0);

        let drained = Arc::new(ScanBudget::new(allowance));
        let _all = drained.draw(24, Some(WorkerMemory::per_worker(allowance)), 0, None, 0);
        let starved = drained.draw(4, memory, 0, None, 0);
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
        assert_eq!(budget.draw(24, memory, 0, None, 0).parallelism().jobs(), 7);
        assert_eq!(budget.draw(24, memory, 0, Some(20 << 30), 0).parallelism().jobs(), 15);
        let first = budget.draw(24, memory, 0, Some(5 << 30), 0);
        assert_eq!(first.parallelism().jobs(), 3);
        assert_eq!(budget.drawn(), 3 * per_worker);
        // The live draw comes off the next one's, whichever allowance it is.
        assert_eq!(budget.draw(24, memory, 0, None, 0).parallelism().jobs(), 4);
        assert_eq!(budget.allowance(), &Allowance::Stated(10 << 30));

        let unfound = Arc::new(ScanBudget::of_allowance(Allowance::NoneFound));
        let stated = unfound.draw(24, memory, 0, Some(10 << 30), 0);
        assert_eq!(stated.parallelism().jobs(), 7);
        assert_eq!(unfound.drawn(), 7 * per_worker);
    }

    /// **With nothing found the session's scans share
    /// `DEFAULT_MEMORY_BUDGET`**, drawing it down as they draw a carved
    /// estimate, never each capped at it alone: a second scan gets what the
    /// first left, a third nothing once it is spent, and
    /// what a dropped scan drew is returned. What the session holds besides
    /// its scans comes off no margin, the default having none.
    #[test]
    fn with_nothing_found_the_scans_share_the_default_budget() {
        use pgdt_io::DEFAULT_MEMORY_BUDGET;
        let per_worker = 24u64 << 20;
        let memory = Some(WorkerMemory::per_worker(per_worker));
        let unfound = Arc::new(ScanBudget::of_allowance(Allowance::NoneFound));
        let _map = unfound.hold(1 << 30);

        let first = unfound.draw(24, memory, 1 << 30, None, 0);
        assert_eq!(first.parallelism(), Parallelism::workers(2, 2 * per_worker));
        let left = DEFAULT_MEMORY_BUDGET - 2 * per_worker;
        let second = unfound.draw(24, memory, 0, None, 0);
        assert_eq!(second.parallelism(), Parallelism::workers(1, left));
        assert_eq!(unfound.drawn(), DEFAULT_MEMORY_BUDGET);
        let starved = unfound.draw(4, None, 0, None, 0);
        assert_eq!(starved.parallelism(), Parallelism::workers(4, 0));

        drop((first, second, starved));
        assert_eq!(unfound.drawn(), 0);
        let plain = unfound.draw(4, None, 0, None, 0);
        assert_eq!(plain.parallelism(), Parallelism::workers(4, DEFAULT_MEMORY_BUDGET));
    }

    /// **A draw's allowance says where it came from**: the budget's own —
    /// `Stated` where it was built with a number, else what discovery found —
    /// and a `pgdump.memory` passed to a draw, `Setting`, for that draw
    /// alone. What discovery finds on each kind of host is `pgdt_io`'s test.
    #[test]
    fn a_draw_s_allowance_keeps_its_origin() {
        let stated = Arc::new(ScanBudget::new(10 << 30));
        assert_eq!(stated.allowance(), &Allowance::Stated(10 << 30));
        let draw = stated.draw(2, None, 0, Some(4 << 30), 0);
        assert_eq!(draw.account().allowance, Allowance::Setting(4 << 30));
        assert_eq!(
            stated.draw(2, None, 0, None, 0).account().allowance,
            Allowance::Stated(10 << 30)
        );
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
        let first = budget.draw(2, memory, 0, None, 0);
        assert_eq!(
            first.account(),
            &BudgetAccount {
                allowance: Allowance::Stated(10 << 30),
                pool_limit: 0,
                resident: 1 << 20,
                drawn: 0,
                lines: 0,
                retained: 0,
            }
        );
        let second = budget.draw(2, memory, 128 << 20, None, 0);
        assert_eq!(second.account().pool_limit, 128 << 20);
        assert_eq!(second.account().drawn, 2 * per_worker);
    }

    /// **A scan's ledger is carved inside its draw, around its longest
    /// line**: readers that would leave it less than the line are fewer, so
    /// the line fits its room, and the live scans' budgets come off that room
    /// as they come off the count (`docs/design/decisions.md`, "D157").
    #[test]
    fn a_draw_carves_its_scan_s_ledger_around_its_longest_line() {
        let per_worker = 1u64 << 30;
        let memory = Some(WorkerMemory::per_worker(per_worker));
        let line = 2 << 30;
        let budget = Arc::new(ScanBudget::new(10 << 30));
        let bare = budget.draw(24, memory, 0, None, 0);
        assert!(bare.arrangement().ledger().capacity().unwrap() < line);
        drop(bare);
        let lined = budget.draw(24, memory, 0, None, line);
        assert!(lined.arrangement().ledger().capacity().unwrap() >= line);
        assert!(lined.parallelism().jobs() < 7, "fewer readers than the line's absence affords");
        let later = budget.draw(24, memory, 0, None, 0);
        let ledger = later.arrangement().ledger();
        let room = ledger.capacity().unwrap() - ledger.statistics();
        let alone = Arc::new(ScanBudget::new(10 << 30)).draw(24, memory, 0, None, 0);
        assert!(room <= alone.arrangement().ledger().capacity().unwrap());
    }

    /// **Two live scans' ledgers lend no byte twice**: the second's room for
    /// lines, past what opens its statistics' draw, together with the first's
    /// longest line, both budgets and what the session holds, fits the
    /// margin's ceiling, so the line the first was carved around is never
    /// lent again; the second's account names that charge, and dropping the
    /// first returns it (`docs/design/decisions.md`, "D160").
    #[test]
    fn two_live_scans_ledgers_lend_no_byte_twice() {
        let memory = Some(WorkerMemory::per_worker(256 << 20));
        let allowance = 10u64 << 30;
        let (map, pool, first_line, second_line) = (64u64 << 20, 128 << 20, 2 << 30, 1 << 30);
        let budget = Arc::new(ScanBudget::new(allowance));
        let _map = budget.hold(map);

        let first = budget.draw(8, memory, pool, None, first_line);
        let second = budget.draw(24, memory, pool, None, second_line);
        assert_eq!(second.account().lines, first_line);
        let ledger = second.arrangement().ledger();
        let lends = ledger.capacity().unwrap() - ledger.statistics();
        assert!(lends >= second_line, "the second's own line fits its room");
        let budgets = [&first, &second].map(|draw| draw.parallelism().memory_bytes().unwrap());
        // Drawn on nothing, the first's room is the ceiling less its budget.
        let ceiling = first.arrangement().ledger().capacity().unwrap() + budgets[0];
        assert!(
            lends + first_line + budgets.iter().sum::<u64>() + map + pool <= ceiling,
            "the second lends {lends} beside the first's line {first_line}, budgets {budgets:?}"
        );

        drop((first, second));
        let alone = budget.draw(24, memory, pool, None, second_line);
        assert_eq!(alone.account().lines, 0, "a dropped scan's line is returned");
    }

    /// **A scan's floor takes off the resident statistics and the live scans'
    /// line charges, never the pool's grant** (`docs/design/decisions.md`,
    /// "D161"): each comes off what one reader is left by exactly its bytes,
    /// named as what stays beside the line, while a pool's limit, though it
    /// lowers the count as they do, leaves the floor where it was.
    #[test]
    fn a_draw_s_floor_takes_off_what_stays_beside_its_line() {
        let memory = Some(WorkerMemory::per_worker(256 << 20));
        let allowance = 10u64 << 30;
        let floor = |draw: &Draw| draw.arrangement().ledger().floor().unwrap();
        let bare = floor(&Arc::new(ScanBudget::new(allowance)).draw(8, memory, 0, None, 0));

        let pooled = Arc::new(ScanBudget::new(allowance)).draw(8, memory, 1 << 30, None, 0);
        assert_eq!(floor(&pooled), bare, "a grant is no holding");

        let (map, line) = (512u64 << 20, 1u64 << 30);
        let budget = Arc::new(ScanBudget::new(allowance));
        let _map = budget.hold(map);
        let first = budget.draw(1, memory, 0, None, line);
        let first_budget = first.parallelism().memory_bytes().unwrap();
        let second = budget.draw(8, memory, 1 << 30, None, 0);
        let expected = bare.holds - first_budget - map - line;
        assert_eq!((floor(&second).holds, floor(&second).beside), (expected, map + line));
    }

    /// **The allocator's retention is held as a resident map is**
    /// (`docs/design/decisions.md`, "D165"): a budget retaining some bytes
    /// carves every scan as one holding a map of them does — its count, what
    /// its ledger lends and its floor, `Floor::beside` naming them — and each
    /// draw's account names it; a budget stating none bills nothing.
    #[test]
    fn the_allocator_s_retention_is_held_as_a_resident_map_is() {
        let memory = Some(WorkerMemory::per_worker(512 << 20));
        let allowance = 10u64 << 30;
        let retained = 320u64 << 20;
        let mapped = Arc::new(ScanBudget::new(allowance));
        let _map = mapped.hold(retained);
        let held = mapped.draw(24, memory, 0, None, 1 << 30);

        let retaining = Arc::new(ScanBudget::new(allowance).retaining(retained));
        assert_eq!(retaining.retained(), retained);
        let draw = retaining.draw(24, memory, 0, None, 1 << 30);
        assert_eq!(draw.parallelism(), held.parallelism());
        let ledger = |draw: &Draw| {
            let ledger = draw.arrangement().ledger();
            (ledger.capacity().unwrap() - ledger.statistics(), ledger.floor())
        };
        assert_eq!(ledger(&draw), ledger(&held), "it lends what a map leaves, at its floor");
        assert_eq!(draw.arrangement().ledger().floor().unwrap().beside, retained);
        assert_eq!(draw.account().retained, retained);
        assert_eq!(draw.account().resident, 0, "no map is billed for it");

        let none = Arc::new(ScanBudget::new(allowance)).draw(24, memory, 0, None, 1 << 30);
        assert_eq!(none.account().retained, 0);
        assert!(none.parallelism().jobs() > draw.parallelism().jobs());
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
