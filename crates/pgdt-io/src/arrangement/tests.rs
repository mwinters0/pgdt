use std::path::{Path, PathBuf};

use crate::internal::MEMORY_RESERVE;
use crate::source::pool::POOL_DEPTH;
use crate::test_support::*;

use super::*;

/// One of the committed runtime roots (`tests/data/runtime/README.md`) — a
/// filesystem tree shaped like a Linux one, so a resolution can be pinned
/// against an environment this machine is not in.
fn runtime_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/runtime").join(name)
}

/// One reader's charge in the flat shape every source but the block-decoding
/// one states.
const READER: u64 = 58 << 20;

/// What a source recommending `jobs` readers of `memory` each resolves to,
/// flagless, inside the allowance the root `name` states.
fn flagless(name: &str, jobs: usize, memory: Option<WorkerMemory>) -> Arrangement {
    let allowance = discover_allowance_in(&runtime_root(name));
    Arrangement::resolve(&allowance, ArrangementRequest::recommended(jobs, memory))
}

/// **Each root states the allowance its README names, and says where it came
/// from**: a limit and the file stating it, the v1 mount located through
/// `mountinfo` rather than assumed; half of `MemAvailable` with no limit; and
/// nothing where neither can be read.
#[test]
fn each_root_states_its_allowance_and_where_it_came_from() {
    let limit = |name: &str, bytes: u64, file: &str| {
        let read_from = runtime_root(name).join(file);
        Allowance::Limit(MemoryLimit { bytes, read_from })
    };
    assert_eq!(
        discover_allowance_in(&runtime_root("v2-limit")),
        limit("v2-limit", 1 << 30, "sys/fs/cgroup/pgdt/memory.max")
    );
    assert_eq!(
        discover_allowance_in(&runtime_root("v1-limit")),
        limit("v1-limit", 512 << 20, "sys/fs/cgroup/memory/svc/memory.limit_in_bytes")
    );
    assert_eq!(
        discover_allowance_in(&runtime_root("below-reserve")),
        limit("below-reserve", 256 << 20, "sys/fs/cgroup/pgdt/memory.max")
    );
    assert_eq!(
        discover_allowance_in(&runtime_root("no-limit")),
        Allowance::HalfAvailable(20002184 * 1024 / 2)
    );
    assert_eq!(
        discover_allowance_in(&runtime_root("cramped")),
        Allowance::HalfAvailable(256 << 20)
    );

    let blind = FakeRoot::new();
    assert_eq!(discover_allowance_in(blind.path()), Allowance::NoneFound);
    assert_eq!(Allowance::NoneFound.bytes(), None);
    blind.write("proc/meminfo", "MemAvailable: 0 kB\n");
    assert_eq!(discover_allowance_in(blind.path()), Allowance::HalfAvailable(0));
}

/// **What a flagless run resolves on every root**, the arms differing in
/// which term binds — a discovered ceiling on either cgroup version, `RT8`'s
/// half-`MemAvailable` cap, the reserve leaving nothing. The count is what the
/// allowance affords and the budget what that many readers spend, never the
/// cap (`docs/design/roadmap.md`, "A default runs as fast as the allocation
/// permits").
#[test]
fn a_flagless_resolution_on_every_root() {
    let reader = Some(WorkerMemory::per_worker(READER));
    let resolved: Vec<(usize, Option<u64>)> =
        ["v2-limit", "v1-limit", "no-limit", "cramped", "below-reserve"]
            .iter()
            .map(|name| {
                let parallelism = flagless(name, 24, reader).parallelism();
                (parallelism.jobs(), parallelism.memory_bytes())
            })
            .collect();
    // v2: 1 GiB, 640 MiB after the reserve, 563.2 MiB once the margin is
    // left — nine readers. v1: 512 MiB, 128 MiB after the reserve, the
    // margin inert below its crossover — two. No limit: half of ~19 GiB
    // holds all twenty-four; half of 512 MiB, four. Under the reserve: one
    // reader on nothing, the floor being on the count.
    assert_eq!(
        resolved,
        vec![
            (9, Some(9 * READER)),
            (2, Some(2 * READER)),
            (24, Some(24 * READER)),
            (4, Some(4 * READER)),
            (1, Some(0)),
        ]
    );

    // A source recommending nothing is left where it was: capped by a
    // limit, on the constant; on nothing at all, stating no budget; and
    // under the reserve, on nothing.
    let plain = flagless("v2-limit", 1, None).parallelism();
    assert_eq!(plain, Parallelism::Serial { memory_bytes: Some(DEFAULT_MEMORY_BUDGET) });
    assert_eq!(flagless("no-limit", 1, None).parallelism(), Parallelism::default());
    assert_eq!(
        flagless("no-limit", 8, None).parallelism(),
        Parallelism::workers(8, DEFAULT_MEMORY_BUDGET)
    );
    assert_eq!(flagless("below-reserve", 1, None).parallelism().memory_bytes(), Some(0));
}

/// **A shared pool lowers the recommended count at every allocation.** A
/// block-decoding source's pool retains a unit for every slot but the one a
/// reader is filling, so the allowance buys fewer than a division by the
/// per-reader charge says. Pinned at both ends: billing no pool would bind
/// nowhere, billing two units a reader would over-bill.
#[test]
fn a_shared_pool_lowers_a_recommended_count_at_every_allocation() {
    const UNIT: u64 = 24 << 20;
    const BLOCK_READER: u64 = 34 << 20;
    let memory = Some(WorkerMemory::per_worker(BLOCK_READER).pooling(UNIT, POOL_DEPTH));

    // 128 MiB after the reserve: a division calls it three readers, and the
    // three units the pool retains beside one leave too little for a second.
    let tight = flagless("v1-limit", 24, memory).parallelism();
    assert_eq!((tight.jobs(), tight.memory_bytes()), (1, Some(BLOCK_READER + 3 * UNIT)));

    // 1 GiB under the margin: a division says sixteen, the slot each reader
    // past the first takes makes it ten.
    let roomy = flagless("v2-limit", 24, memory).parallelism();
    assert_eq!((roomy.jobs(), roomy.memory_bytes()), (10, Some(10 * BLOCK_READER + 9 * UNIT)));
}

/// **A stated count is kept, with the budget the allowance affords beside
/// it**, where a recommended one is lowered; and only a recommendation is
/// echoed as one.
#[test]
fn a_stated_count_is_kept_and_a_recommended_one_lowered() {
    let reader = Some(WorkerMemory::per_worker(READER));
    // 32 MiB past the reserve affords no whole reader: one, on 32 MiB.
    let allowance = Allowance::Stated(MEMORY_RESERVE + (32 << 20));
    let lowered = Arrangement::resolve(&allowance, ArrangementRequest::recommended(24, reader));
    assert_eq!(lowered.parallelism(), Parallelism::Serial { memory_bytes: Some(32 << 20) });
    assert_eq!(lowered.recommended_jobs(), Some(24));

    let request = ArrangementRequest { jobs: Jobs::Stated(24), ..lowered_request(reader) };
    let kept = Arrangement::resolve(&allowance, request);
    assert_eq!(kept.parallelism(), Parallelism::workers(24, 32 << 20));
    assert_eq!(kept.recommended_jobs(), None);

    // A stated count over a source recommending nothing, under no limit,
    // carries the constant bare: `Workers` has nowhere to say nobody stated a
    // budget.
    let unlimited = Allowance::HalfAvailable(8 << 30);
    let request = ArrangementRequest { jobs: Jobs::Stated(8), ..lowered_request(None) };
    let typed = Arrangement::resolve(&unlimited, request);
    assert_eq!(typed.parallelism(), Parallelism::workers(8, DEFAULT_MEMORY_BUDGET));
    let request = ArrangementRequest { jobs: Jobs::Stated(1), ..lowered_request(None) };
    assert_eq!(Arrangement::resolve(&unlimited, request).parallelism(), Parallelism::default());
}

fn lowered_request(memory: Option<WorkerMemory>) -> ArrangementRequest {
    ArrangementRequest::recommended(24, memory)
}

/// **A source is asked for a count only where none was stated**, and always
/// for what a worker holds.
#[test]
fn a_request_asks_the_source_only_for_what_was_not_stated() {
    assert_eq!(ArrangementRequest::of(&BareSource, None).jobs, Jobs::Recommended(1));
    let stated = ArrangementRequest::of(&BareSource, Some(6));
    assert_eq!(stated.jobs, Jobs::Stated(6));
    assert_eq!(stated, ArrangementRequest { jobs: Jobs::Stated(6), ..lowered_request(None) });
}

/// **The statistics a cache hands a pass are carved around, not after**
/// (`docs/design/decisions.md`, "D85"): held against the margin's ceiling,
/// they lower a recommended count first, and where the count cannot fall the
/// budget, so the statistics allowance still covers what the cache holds. A
/// discovered limit and the same number stated carve them alike.
#[test]
fn a_holding_lowers_the_count_and_then_the_budget() {
    let reader = Some(WorkerMemory::per_worker(READER));
    let limit = discover_allowance_in(&runtime_root("v2-limit"));
    let holding = |allowance: &Allowance, memory, held| {
        Arrangement::resolve(
            allowance,
            ArrangementRequest { statistics: held, ..ArrangementRequest::recommended(24, memory) },
        )
    };
    assert_eq!(holding(&limit, reader, 0).parallelism().jobs(), 9);
    // Two readers' worth held: the ceiling less 116 MiB affords seven, and
    // the room seven leave absorbs the holding whole.
    let held = holding(&limit, reader, 2 * READER);
    assert_eq!(held.parallelism(), Parallelism::workers(7, 7 * READER));
    let stated = Allowance::Stated(1 << 30);
    assert_eq!(holding(&stated, reader, 2 * READER).parallelism(), held.parallelism());

    // What the allocator retains is carved around as the statistics are
    // (`docs/design/decisions.md`, "D165"), and comes off the ledger's room
    // rather than opening the statistics' draw, which a gathering pass's
    // account replaces with its own total: so it is never lent again.
    let request = ArrangementRequest::recommended(24, reader);
    let retaining =
        Arrangement::resolve(&limit, ArrangementRequest { retained: 2 * READER, ..request });
    assert_eq!(retaining.parallelism(), held.parallelism());
    let (kept, statistics) = (retaining.ledger(), held.ledger());
    assert_eq!(kept.statistics(), 0);
    assert_eq!(kept.capacity().unwrap(), statistics.capacity().unwrap() - 2 * READER);
    let allowance = |arrangement: &Arrangement| arrangement.statistics_allowance().unwrap();
    assert_eq!(allowance(&retaining), allowance(&held) - 2 * READER);
    let lends = |ledger: &crate::Ledger| ledger.capacity().unwrap() - ledger.statistics();
    assert_eq!(lends(kept), lends(statistics));
    kept.draw_statistics(0);
    statistics.draw_statistics(0);
    assert_eq!(
        lends(kept) + 2 * READER,
        lends(statistics),
        "the account's draw lends no retention"
    );

    // A plain source's count cannot fall, so what the room beside its
    // budget cannot absorb comes off the budget, and the statistics
    // allowance is then exactly what the cache holds.
    let room = margin_room(1 << 30, DEFAULT_MEMORY_BUDGET);
    let absorbed = holding(&stated, None, room);
    assert_eq!(absorbed.parallelism().memory_bytes(), Some(DEFAULT_MEMORY_BUDGET));
    let short = 16 << 20;
    let squeezed = holding(&stated, None, room + short);
    assert_eq!(squeezed.parallelism().memory_bytes(), Some(DEFAULT_MEMORY_BUDGET - short));
    assert_eq!(squeezed.statistics_allowance(), Some(room + short));
}

/// **An estimate caps the pools unless the request carves it**: half of
/// `MemAvailable` is a cap with no reserve, no margin and no holding, the
/// halving being the margin there; carved, it is the same number stated.
/// With `MemAvailable` unreadable `DEFAULT_MEMORY_BUDGET` is the estimate: a
/// cap nothing drawn or held moves, and carved, one what is drawn comes off,
/// the holdings having no margin to come off.
#[test]
fn an_estimate_is_a_cap_unless_carved() {
    let reader = Some(WorkerMemory::per_worker(READER));
    let half = Allowance::HalfAvailable(1 << 30);
    let request = ArrangementRequest { statistics: 4 * READER, ..lowered_request(reader) };
    let capped = Arrangement::resolve(&half, request);
    assert_eq!(capped.parallelism(), Parallelism::workers(17, 17 * READER));

    let carved = ArrangementRequest { carve_estimate: true, ..request };
    assert_eq!(
        Arrangement::resolve(&half, carved).parallelism(),
        Arrangement::resolve(&Allowance::Stated(1 << 30), request).parallelism()
    );

    let small = Some(WorkerMemory::per_worker(16 << 20));
    let shared = ArrangementRequest { drawn: 1 << 30, granted: 1 << 30, ..lowered_request(small) };
    let blind = Arrangement::resolve(&Allowance::NoneFound, shared);
    assert_eq!(blind.parallelism(), Parallelism::workers(4, DEFAULT_MEMORY_BUDGET));
    assert_eq!(blind.statistics_allowance(), None, "nothing found declines nothing");

    // A reader costing more than the default is the floor, at the default.
    let wide = Some(WorkerMemory::per_worker(2 * DEFAULT_MEMORY_BUDGET));
    let one = Arrangement::resolve(&Allowance::NoneFound, lowered_request(wide));
    assert_eq!(one.parallelism(), Parallelism::workers(1, DEFAULT_MEMORY_BUDGET));

    // Carved, what is drawn comes off it, and what is held does not.
    let drawn = 16 << 20;
    let carved = ArrangementRequest { carve_estimate: true, drawn, granted: 1 << 30, ..shared };
    let rest = Arrangement::resolve(&Allowance::NoneFound, carved);
    assert_eq!(rest.parallelism(), Parallelism::workers(3, DEFAULT_MEMORY_BUDGET - drawn));
    let spent = ArrangementRequest { drawn: DEFAULT_MEMORY_BUDGET, ..carved };
    let floor = Arrangement::resolve(&Allowance::NoneFound, spent);
    assert_eq!(floor.parallelism(), Parallelism::workers(1, 0));

    // A source recommending nothing keeps the library's own default.
    let plain = Arrangement::resolve(&Allowance::NoneFound, lowered_request(None));
    assert_eq!(plain.parallelism(), Parallelism::workers(24, DEFAULT_MEMORY_BUDGET));
}

/// **What others drew comes off this arrangement and its statistics**: a
/// setting carves as a stated allowance does, and the margin it leaves for
/// statistics is what the budget and the draws leave.
#[test]
fn what_others_drew_comes_off_the_count_and_the_statistics() {
    let per_worker = 1u64 << 30;
    let memory = Some(WorkerMemory::per_worker(per_worker));
    let allowance = Allowance::Setting(10 << 30);
    assert_eq!(
        Arrangement::resolve(&allowance, ArrangementRequest::recommended(24, memory))
            .parallelism()
            .jobs(),
        7
    );
    let drawn = 4 * per_worker;
    let request = ArrangementRequest { drawn, ..ArrangementRequest::recommended(24, memory) };
    let second = Arrangement::resolve(&allowance, request);
    assert_eq!(second.parallelism(), Parallelism::workers(3, 3 * per_worker));
    assert_eq!(second.statistics_allowance(), Some(margin_room(10 << 30, 3 * per_worker) - drawn));
    assert_eq!(second.allowance(), &allowance);
}

/// **The ledger is what the margin's ceiling leaves the readers**
/// (`docs/design/decisions.md`, "D157"), less what others drew, wherever an
/// allowance is found, an estimate's included, and what is held opens the
/// statistics' draw on it; nothing found, a ledger without limit.
#[test]
fn the_ledger_is_what_the_ceiling_leaves_the_readers() {
    const GIB: u64 = 1 << 30;
    let memory = Some(WorkerMemory::per_worker(GIB));
    let allowance = Allowance::Stated(10 * GIB);

    let alone = Arrangement::resolve(&allowance, ArrangementRequest::recommended(24, memory));
    assert_eq!(alone.parallelism(), Parallelism::workers(7, 7 * GIB));
    assert_eq!(alone.ledger().capacity(), Some(margin_room(10 * GIB, 7 * GIB)));
    assert!(margin_room(10 * GIB, 7 * GIB) < 10 * GIB - MEMORY_RESERVE - 7 * GIB, "not the cap");
    assert_eq!(alone.ledger().drawn(), 0);
    assert_eq!(alone.statistics_allowance(), alone.ledger().capacity());

    // What is held lowers the count, and the statistics' draw holds it.
    let request =
        ArrangementRequest { statistics: GIB, ..ArrangementRequest::recommended(24, memory) };
    let holding = Arrangement::resolve(&allowance, request);
    assert_eq!(holding.parallelism(), Parallelism::workers(6, 6 * GIB));
    let capacity = margin_room(10 * GIB, 6 * GIB);
    assert_eq!(holding.ledger().capacity(), Some(capacity));
    assert_eq!((holding.ledger().statistics(), holding.ledger().drawn()), (GIB, GIB));
    assert_eq!(holding.statistics_allowance(), Some(capacity));

    let request = ArrangementRequest { drawn: GIB, ..ArrangementRequest::recommended(24, memory) };
    let shared = Arrangement::resolve(&allowance, request);
    assert_eq!(shared.parallelism(), Parallelism::workers(6, 6 * GIB));
    assert_eq!(shared.ledger().capacity(), Some(margin_room(10 * GIB, 6 * GIB) - GIB));

    // An estimate's pools ignore what is held, and its ledger is carved
    // under the ceiling all the same, carved or not.
    let reader = Some(WorkerMemory::per_worker(READER));
    let request = ArrangementRequest { statistics: 4 * READER, ..lowered_request(reader) };
    let estimate = Arrangement::resolve(&Allowance::HalfAvailable(8 * GIB), request);
    assert_eq!(estimate.parallelism(), Parallelism::workers(24, 24 * READER));
    assert_eq!(estimate.ledger().capacity(), Some(margin_room(8 * GIB, 24 * READER)));
    assert_eq!(estimate.ledger().statistics(), 4 * READER);
    let carved = ArrangementRequest { carve_estimate: true, ..request };
    let carved = Arrangement::resolve(&Allowance::HalfAvailable(8 * GIB), carved);
    let budget = carved.parallelism().budget_bytes();
    assert_eq!(carved.ledger().capacity(), Some(margin_room(8 * GIB, budget)));

    for carve_estimate in [false, true] {
        let request =
            ArrangementRequest { carve_estimate, statistics: GIB, ..lowered_request(reader) };
        let blind = Arrangement::resolve(&Allowance::NoneFound, request);
        assert_eq!(blind.ledger().capacity(), None, "nothing found carves no limit");
        assert_eq!(blind.ledger().drawn(), 0);
    }
}

/// **A read's longest line is carved into the ledger before its readers**:
/// it comes off the margin's ceiling the count is solved against, so fewer
/// readers leave the room it needs, and off what statistics may hold beside
/// them, but never off the cap, so where the cap binds it costs no reader
/// the ceiling's lead over the cap can hold.
#[test]
fn a_line_charge_is_carved_into_the_ledger_off_the_ceiling() {
    const GIB: u64 = 1 << 30;
    let memory = Some(WorkerMemory::per_worker(GIB));
    let allowance = Allowance::Stated(10 * GIB);
    let line_charge = 2 * GIB;
    let request = ArrangementRequest { line_charge, ..ArrangementRequest::recommended(24, memory) };
    let lined = Arrangement::resolve(&allowance, request);
    assert_eq!(lined.parallelism(), Parallelism::workers(5, 5 * GIB));
    let capacity = lined.ledger().capacity().unwrap();
    assert_eq!(capacity, margin_room(10 * GIB, 5 * GIB));
    assert!(capacity >= line_charge, "the line is admitted without waiting");
    assert_eq!(lined.statistics_allowance(), Some(capacity - line_charge));

    // Under the crossover the cap binds: a line the ceiling's lead holds
    // costs no reader, where the same bytes drawn by another scan do.
    let reader = Some(WorkerMemory::per_worker(READER));
    let small = Allowance::Stated(512 << 20);
    let line_charge = 16 << 20;
    let request = ArrangementRequest { line_charge, ..lowered_request(reader) };
    let lined = Arrangement::resolve(&small, request);
    assert_eq!(lined.parallelism(), Parallelism::workers(2, 2 * READER));
    assert!(lined.ledger().capacity().unwrap() >= line_charge);
    let drawn = ArrangementRequest { drawn: line_charge, ..lowered_request(reader) };
    assert_eq!(Arrangement::resolve(&small, drawn).parallelism().jobs(), 1);

    // An estimate's readers are capped by what the line leaves of it, and
    // are not solved against the ceiling: its room is what they leave there.
    let request = ArrangementRequest { line_charge: 512 << 20, ..lowered_request(reader) };
    let estimate = Arrangement::resolve(&Allowance::HalfAvailable(GIB), request);
    assert_eq!(estimate.parallelism(), Parallelism::workers(8, 8 * READER));
    assert_eq!(estimate.ledger().capacity(), Some(margin_room(GIB, 8 * READER)));
}

/// **The floor is what the allowance leaves one reader, whatever the count**
/// (`docs/design/decisions.md`, "D161"): the cap less one reader's budget
/// above the crossover, the margin's ceiling less it below, what others drew
/// off both; an uncarved estimate's cap is the whole estimate; and nothing
/// found, no floor. What stays beside the line is the next test's.
#[test]
fn the_floor_is_what_the_allowance_leaves_one_reader() {
    const GIB: u64 = 1 << 30;
    let memory = Some(WorkerMemory::per_worker(GIB));
    let allowance = Allowance::Stated(10 * GIB);
    let floor = |request: ArrangementRequest| {
        Arrangement::resolve(&allowance, request).ledger().floor().unwrap()
    };
    let cap = 10 * GIB - MEMORY_RESERVE;
    for jobs in [1, 4, 24] {
        let at = floor(ArrangementRequest::recommended(jobs, memory));
        let expected = Floor { holds: cap - GIB, beside: 0, allowance: 10 * GIB };
        assert_eq!(at, expected, "at {jobs} reader(s)");
    }
    let drawn = floor(ArrangementRequest { drawn: 3 * GIB, ..lowered_request(memory) });
    assert_eq!(drawn.holds, cap - 3 * GIB - GIB);
    // A source recommending nothing is one reader at the default budget.
    assert_eq!(floor(lowered_request(None)).holds, cap - DEFAULT_MEMORY_BUDGET);

    // Below the crossover the ceiling lends past the cap, and the floor with it.
    let small = Allowance::Stated(448 << 20);
    let below = Arrangement::resolve(&small, lowered_request(None));
    let one = (448u64 << 20) - MEMORY_RESERVE;
    let holds = margin_allowance(448 << 20) - one;
    assert_eq!(below.ledger().floor().unwrap().holds, holds);
    assert!(holds >= below.ledger().capacity().unwrap(), "never less than the room");

    let request = lowered_request(Some(WorkerMemory::per_worker(READER)));
    let estimate = Arrangement::resolve(&Allowance::HalfAvailable(GIB), request);
    assert_eq!(estimate.ledger().floor().unwrap().holds, GIB - READER);
    let blind = Arrangement::resolve(&Allowance::NoneFound, request);
    assert_eq!(blind.ledger().floor(), None, "nothing found refuses nothing");
}

/// **The floor takes off what stays beside the line as the run is carved**
/// (`docs/design/decisions.md`, "D161"): the statistics the run keeps, other
/// live scans' line charges and what the allocator retains come off what one
/// reader is left, at any count, an uncarved estimate's included, and
/// `Floor::beside` names them; a session pool's grant does not, though all
/// four lower the count and open the statistics' draw alike. No line the
/// ledger lends room for is refused.
#[test]
fn the_floor_takes_off_what_stays_beside_the_line() {
    const GIB: u64 = 1 << 30;
    let memory = Some(WorkerMemory::per_worker(GIB));
    let allowance = Allowance::Stated(10 * GIB);
    let resolve = |request| Arrangement::resolve(&allowance, request);
    let bare = resolve(lowered_request(memory)).ledger().floor().unwrap();

    let (statistics, lines, retained, granted) = (GIB, 512 << 20, 320 << 20, 2 * GIB);
    let beside = ArrangementRequest { statistics, lines, retained, ..lowered_request(memory) };
    for request in [
        beside,
        ArrangementRequest { granted, ..beside },
        ArrangementRequest { jobs: Jobs::Stated(24), ..beside },
    ] {
        let arrangement = resolve(request);
        let ledger = arrangement.ledger();
        let floor = ledger.floor().unwrap();
        let beside = statistics + lines + retained;
        let expected = Floor { holds: bare.holds - beside, beside, allowance: 10 * GIB };
        assert_eq!(floor, expected, "{request:?}");
        assert_eq!(ledger.statistics(), request.held(), "the draw opens at all three");
        let lends = ledger.capacity().unwrap() - ledger.statistics();
        assert!(lends <= floor.holds, "the floor never under what the room lends a line");
    }
    let granted = resolve(ArrangementRequest { granted, ..lowered_request(memory) });
    assert_eq!(granted.ledger().floor().unwrap(), bare, "a grant is no holding");
    let fewer = resolve(lowered_request(memory)).parallelism().jobs() - 2;
    assert_eq!(granted.parallelism().jobs(), fewer, "though it lowers the count");

    // An uncarved estimate's floor takes them off its whole cap.
    let reader = Some(WorkerMemory::per_worker(READER));
    let half = Allowance::HalfAvailable(GIB);
    let request = ArrangementRequest {
        statistics: READER,
        lines: READER,
        retained: READER,
        ..lowered_request(reader)
    };
    let estimate = Arrangement::resolve(&half, request).ledger().floor().unwrap();
    assert_eq!(estimate, Floor { holds: GIB - 4 * READER, beside: 3 * READER, allowance: GIB });
}

/// **Every clone of an arrangement shares its ledger**: one run's.
#[test]
fn a_clone_shares_its_ledger() {
    let allowance = Allowance::Stated(1 << 30);
    let arrangement = Arrangement::resolve(&allowance, lowered_request(None));
    let clone = arrangement.clone();
    let mut holder = clone.ledger().holder();
    let admitted = futures::FutureExt::now_or_never(holder.draw(0, 1 << 20));
    assert_eq!(admitted, Some(crate::Admission::Held));
    assert_eq!(arrangement.ledger().drawn(), 1 << 20);
}
