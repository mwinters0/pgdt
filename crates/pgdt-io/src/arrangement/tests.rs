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
            ArrangementRequest { held, ..ArrangementRequest::recommended(24, memory) },
        )
    };
    assert_eq!(holding(&limit, reader, 0).parallelism().jobs(), 9);
    // Two readers' worth held: the ceiling less 116 MiB affords seven, and
    // the room seven leave absorbs the holding whole.
    let held = holding(&limit, reader, 2 * READER);
    assert_eq!(held.parallelism(), Parallelism::workers(7, 7 * READER));
    let stated = Allowance::Stated(1 << 30);
    assert_eq!(holding(&stated, reader, 2 * READER).parallelism(), held.parallelism());

    // A plain source's count cannot fall, so what the room beside its
    // budget cannot absorb comes off the budget, and the statistics
    // allowance is then exactly what the cache holds.
    let room = statistics_allowance(1 << 30, DEFAULT_MEMORY_BUDGET);
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
/// With nothing found a recommendation stands whole (`KD113`), and nothing
/// drawn or held moves it.
#[test]
fn an_estimate_is_a_cap_unless_carved() {
    let reader = Some(WorkerMemory::per_worker(READER));
    let half = Allowance::HalfAvailable(1 << 30);
    let request = ArrangementRequest { held: 4 * READER, ..lowered_request(reader) };
    let capped = Arrangement::resolve(&half, request);
    assert_eq!(capped.parallelism(), Parallelism::workers(17, 17 * READER));

    let carved = ArrangementRequest { carve_estimate: true, ..request };
    assert_eq!(
        Arrangement::resolve(&half, carved).parallelism(),
        Arrangement::resolve(&Allowance::Stated(1 << 30), request).parallelism()
    );

    let shared = ArrangementRequest { drawn: 1 << 30, held: 1 << 30, ..lowered_request(reader) };
    let blind = Arrangement::resolve(&Allowance::NoneFound, shared);
    assert_eq!(blind.parallelism(), Parallelism::workers(24, 24 * READER));
    assert_eq!(blind.statistics_allowance(), None, "nothing found declines nothing");
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
    assert_eq!(
        second.statistics_allowance(),
        Some(statistics_allowance(10 << 30, 3 * per_worker) - drawn)
    );
    assert_eq!(second.allowance(), &allowance);
}
