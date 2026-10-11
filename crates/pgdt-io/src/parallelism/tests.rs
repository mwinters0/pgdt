use std::num::NonZeroUsize;

use crate::source::pool::POOL_DEPTH;
use crate::test_support::*;
use crate::*;

use super::*;

/// One worker **is** the serial path, so it is spelled that way rather
/// than as a `Workers` of one nobody can tell from it; zero is a caller's
/// own arithmetic and reads as one, exactly as `xz_seek::Bulk::new` reads
/// it.
///
/// **The collapse takes the worker count down and not the budget**: a
/// caller asking for one worker inside a stated budget has still stated
/// it, and every pool sized from `memory_bytes` sees it
/// (`docs/design/decisions.md`, "D1").
#[test]
fn one_worker_and_none_are_both_the_serial_state() {
    let serial_in_budget = Parallelism::Serial { memory_bytes: Some(512 << 20) };
    assert_eq!(Parallelism::workers(0, 512 << 20), serial_in_budget);
    assert_eq!(Parallelism::workers(1, 512 << 20), serial_in_budget);
    assert!(serial_in_budget.is_serial());
    assert_eq!(serial_in_budget.jobs(), 1);
    assert_eq!(serial_in_budget.memory_bytes(), Some(512 << 20));
    let eight = Parallelism::workers(8, 512 << 20);
    assert!(!eight.is_serial());
    assert_eq!(
        eight,
        Parallelism::Workers { jobs: NonZeroUsize::new(8).unwrap(), memory_bytes: 512 << 20 }
    );
}

/// **A discovered limit caps what a source asked for; it does not become
/// it.** The reserve comes off the top and the source's recommendation is
/// taken no higher, while a recommendation that already fits is left
/// alone. The count is held to [`MEMORY_MARGIN_PERCENT`] besides, the
/// tighter of the two conditions at this limit; the budget is still what
/// the resolved count spends.
#[test]
fn a_discovered_limit_caps_the_recommendation_at_the_limit_less_the_reserve() {
    let root = FakeRoot::new();
    root.v2("/leaf").v2_limits("/leaf", Some("1073741824"), None);
    let cap = (1024 << 20) - MEMORY_RESERVE;

    // Far more than the cap affords, so the count comes down with the
    // budget rather than being printed beside one it cannot spend. The
    // budget is still `cap`-bounded and not margin-bounded, so the floor
    // arrangement reports what one reader holds.
    let squeezed =
        Parallelism::discover_in(root.path(), 8, Some(WorkerMemory::per_worker(512 << 20)));
    assert_eq!(squeezed.memory_bytes(), Some(512 << 20));
    assert_eq!(squeezed.jobs(), 1);

    // Inside the margin allowance as well as inside the cap, so both
    // numbers stand.
    let roomy = Parallelism::discover_in(root.path(), 8, Some(WorkerMemory::per_worker(16 << 20)));
    assert_eq!(roomy.memory_bytes(), Some(128 << 20));
    assert_eq!(roomy.jobs(), 8);

    // And in between, the count is what the margin affords and the budget
    // is exactly what that many readers spend — never the cap itself,
    // which is the over-ask this pairing exists to remove. One more reader
    // would fit the cap and is refused.
    let fitted =
        Parallelism::discover_in(root.path(), 8, Some(WorkerMemory::per_worker(100 << 20)));
    assert_eq!(fitted.jobs(), 5);
    assert_eq!(fitted.memory_bytes(), Some(500 << 20));
    assert!(fitted.memory_bytes() < Some(cap), "the cap itself would be the over-ask");
}

/// **The resolved count answers to the criterion rather than to the host's
/// width**: a recommendation above what the margin allows resolves the
/// same arrangement whatever the recommendation is
/// (`docs/design/decisions.md`, "D3").
#[test]
fn the_count_answers_to_the_margin_and_not_to_the_recommendation() {
    let root = FakeRoot::new();
    root.v2("/leaf").v2_limits("/leaf", Some(&(2u64 << 30).to_string()), None);
    // What one reader of a large-block `.xz` holds, and the retention list
    // the pool keeps beside it — a block size at which this limit resolves
    // well inside either host's core count, so what binds is unambiguous.
    let reader = 138 << 20;
    let memory = WorkerMemory::per_worker(reader).pooling(128 << 20, POOL_DEPTH);

    let narrow = Parallelism::discover_in(root.path(), 24, Some(memory));
    let wide = Parallelism::discover_in(root.path(), 64, Some(memory));
    assert_eq!(narrow.jobs(), wide.jobs(), "the host's core count is not the answer");
    assert_eq!(narrow.memory_bytes(), wide.memory_bytes());

    // And what it resolves to leaves the margin, where the cap alone would
    // have admitted another reader.
    let limit = 2u64 << 30;
    let charge = wide.memory_bytes().unwrap();
    assert!(
        charge + MEMORY_UNPOOLED_BOUND <= limit / 100 * (100 - MEMORY_MARGIN_PERCENT),
        "predicted resident {} breaches the margin",
        charge + MEMORY_UNPOOLED_BOUND
    );
    assert!(
        memory.at(wide.jobs() + 1) + MEMORY_UNPOOLED_BOUND
            > limit / 100 * (100 - MEMORY_MARGIN_PERCENT),
        "one more reader would still have fitted, so the margin is not what bound it"
    );
    assert!(
        memory.at(wide.jobs() + 1) <= limit - MEMORY_RESERVE,
        "the cap alone would have afforded more, which is what the margin is for"
    );
}

/// **A margin the smallest arrangement cannot meet costs nothing.** The
/// floor is one worker at whatever the cap is, so a limit whose margin
/// allowance is under one reader's charge still resolves that reader and
/// still reports the budget it spends — which is what
/// `BlockCache::affordable` reads, so the compressed block path is not
/// declined by the margin at any limit.
///
/// The arrangement needs a limit whose margin allowance falls under one
/// reader's charge while the cap stays above it, which a large-block file
/// reaches and a small-block one does not.
#[test]
fn the_margin_never_takes_the_last_reader_or_the_budget_it_spends() {
    let root = FakeRoot::new();
    // A limit whose margin allowance is under what one block-decoding
    // reader of this file is charged, while its cap is above it.
    root.v2("/leaf").v2_limits("/leaf", Some(&(960u64 << 20).to_string()), None);
    let memory = WorkerMemory::per_worker(138 << 20).pooling(128 << 20, POOL_DEPTH);
    let tight = Parallelism::discover_in(root.path(), 24, Some(memory));
    assert_eq!(tight.jobs(), 1);
    assert_eq!(tight.memory_bytes(), Some(memory.at(1)));
    assert!(memory.at(1) > margin_allowance(960 << 20), "the margin cannot afford it");
}

/// **The margin binds at a large limit and is inert at a small one**, which
/// is the shape it exists for: a constant reserve leaves a shrinking
/// *share* as the limit grows, so the fraction is needed at the top end and
/// the subtraction already covers the bottom. The crossover is arithmetic
/// rather than a reading — `limit − MEMORY_RESERVE` and
/// `(100 − margin)% × limit − MEMORY_UNPOOLED_BOUND` are equal at
/// `5 × (MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND)` — and it is pinned here
/// because nothing else would notice either constant moving past the other.
#[test]
fn the_margin_is_the_tighter_condition_only_above_the_crossover() {
    let crossover = 5 * (MEMORY_RESERVE - MEMORY_UNPOOLED_BOUND);
    assert_eq!(crossover, 640 << 20);
    // Equal there but for the rounding: `limit / 100` discards under a
    // hundred bytes, so the allowance errs small by that much and never
    // the other way.
    assert!(crossover - MEMORY_RESERVE - margin_allowance(crossover) < 100);
    // Below it the cap is what a count is solved against, so the margin
    // can lower nothing; above it the margin is, at every limit.
    assert!(margin_allowance(crossover - (64 << 20)) > (crossover - (64 << 20)) - MEMORY_RESERVE);
    assert!(margin_allowance(crossover + (64 << 20)) < (crossover + (64 << 20)) - MEMORY_RESERVE);
    assert!(margin_allowance(4 << 30) < (4 << 30) - MEMORY_RESERVE);
}

/// **Below the reserve the budget goes to zero rather than to a floor**
/// (`docs/design/decisions.md`, "D3"). What zero produces is one reader's
/// worth on the streaming path, through the floors already in the
/// mechanism.
#[test]
fn a_limit_at_or_under_the_reserve_leaves_no_budget_at_all() {
    let root = FakeRoot::new();
    root.v2("/leaf").v2_limits("/leaf", Some(&MEMORY_RESERVE.to_string()), None);
    let starved = Parallelism::discover_in(root.path(), 4, Some(WorkerMemory::per_worker(1 << 30)));
    assert_eq!(starved.memory_bytes(), Some(0));
    // One worker at whatever the cap is, not one worker's worth of bytes:
    // the floor is on the count.
    assert_eq!(starved.jobs(), 1);

    let tighter = FakeRoot::new();
    tighter.v2("/leaf").v2_limits("/leaf", Some("134217728"), None);
    assert_eq!(Parallelism::discover_in(tighter.path(), 4, None).memory_bytes(), Some(0));
}

/// **A source with no recommendation is still capped by a discovered
/// limit**, at [`DEFAULT_MEMORY_BUDGET`] where the limit leaves room — the
/// plain path, which asks for nothing and is left where it was.
#[test]
fn a_source_that_recommends_nothing_is_capped_at_the_existing_default() {
    let root = FakeRoot::new();
    root.v2("/leaf").v2_limits("/leaf", Some("3221225472"), None);
    assert_eq!(
        Parallelism::discover_in(root.path(), 1, None).memory_bytes(),
        Some(DEFAULT_MEMORY_BUDGET)
    );
}

/// **The composition is not a `min`.** With no limit found there is no cap
/// from the environment at all, so a source's recommendation stands, where
/// a `min` against [`DEFAULT_MEMORY_BUDGET`] would have collapsed a
/// compressed scan to serial (`docs/design/decisions.md`, "D3").
#[test]
fn no_limit_found_leaves_the_recommendation_uncapped_but_for_memavailable() {
    let root = FakeRoot::new();
    root.v2("/leaf")
        .v2_limits("/leaf", Some("max"), None)
        .write("proc/meminfo", "MemAvailable:   20002184 kB\n");
    // Readers of an ordinary compressed dump, well under half of the
    // `MemAvailable` stated above.
    let per_worker = 58 << 20;
    let roomy =
        Parallelism::discover_in(root.path(), 24, Some(WorkerMemory::per_worker(per_worker)));
    assert_eq!(roomy.memory_bytes(), Some(per_worker * 24));
    assert_eq!(roomy.jobs(), 24);

    // Half of `MemAvailable`, because it is an estimate two processes
    // reading at once each see the whole of — and it binds wherever the
    // recommended count costs more than half of what is free, the count
    // then reduced to what half of it buys rather than left standing.
    let small = FakeRoot::new();
    small.v2("/leaf").write("proc/meminfo", "MemAvailable:     262144 kB\n");
    let squeezed =
        Parallelism::discover_in(small.path(), 24, Some(WorkerMemory::per_worker(per_worker)));
    assert_eq!(squeezed.jobs(), 2);
    assert_eq!(squeezed.memory_bytes(), Some(per_worker * 2));

    // `MemAvailable` unreadable is the one shape with a recommendation and
    // no ceiling over it: the whole of what the count asks for.
    let blind = FakeRoot::new();
    blind.v2("/leaf");
    assert_eq!(
        Parallelism::discover_in(blind.path(), 24, Some(WorkerMemory::per_worker(per_worker)))
            .memory_bytes(),
        Some(per_worker * 24)
    );
}

/// **A stated allowance is the discovered limit's own carving**, which is
/// the whole of what `--memory` promises: the same number reached either
/// way resolves to the same pair (`docs/design/decisions.md`, "D83"). The
/// allowances span the margin's crossover, so the check covers the band
/// where the cap binds and the band where the ceiling does.
#[test]
fn a_stated_allowance_resolves_as_the_same_number_discovered_would() {
    let memory = Some(WorkerMemory::per_worker(58 << 20).pooling(24 << 20, POOL_DEPTH));
    for allowance in [512u64 << 20, 640 << 20, 1024 << 20, 4096 << 20] {
        let root = FakeRoot::new();
        root.v2("/leaf").v2_limits("/leaf", Some(&allowance.to_string()), None);
        for jobs in [1, 4, 24] {
            assert_eq!(
                Parallelism::within(jobs, memory, allowance),
                Parallelism::discover_in(root.path(), jobs, memory),
                "{allowance} bytes at {jobs} jobs",
            );
        }
    }
}

/// **The typed number bounds the process, and what comes back bounds the
/// pools**: the reserve is off the top before a reader is counted, and an
/// allowance the reserve swallows resolves to a budget of zero rather than
/// to a floor — which the floors inside the mechanism turn into one reader
/// on the streaming path.
#[test]
fn a_stated_allowance_pays_the_reserve_before_anything_else() {
    let per_worker = 58 << 20;
    let memory = Some(WorkerMemory::per_worker(per_worker));

    // Six readers fit inside the 400 MiB *left* by the allowance; the
    // seventh does not. The budget is what those six spend, never the
    // number typed.
    let cut = Parallelism::within(24, memory, MEMORY_RESERVE + (400 << 20));
    assert_eq!(cut.jobs(), 6);
    assert_eq!(cut.memory_bytes(), Some(6 * per_worker), "the carved spend, not the allowance");

    // Room for the whole recommendation leaves it standing.
    let roomy = Parallelism::within(24, memory, 64 << 30);
    assert_eq!(roomy.jobs(), 24);
    assert_eq!(roomy.memory_bytes(), Some(24 * per_worker));

    // At and below the reserve there is nothing left to spend.
    let starved = Parallelism::within(24, memory, MEMORY_RESERVE);
    assert_eq!(starved, Parallelism::Serial { memory_bytes: Some(0) });

    // Nothing to divide by leaves the count where it is, on the constant.
    let blind = Parallelism::within(8, None, 64 << 30);
    assert_eq!(blind.jobs(), 8);
    assert_eq!(blind.memory_bytes(), Some(DEFAULT_MEMORY_BUDGET));
}

/// **A share of an allowance others are drawing on spends only what they
/// leave.** Inside 10 GiB the margin affords seven 1 GiB readers: the first
/// arrangement takes the four it asks for, the second the three that are
/// left, and one arriving after the whole has been drawn gets one reader on
/// nothing.
#[test]
fn a_shared_allowance_leaves_each_arrangement_what_the_others_have_not_drawn() {
    let per_worker = 1 << 30;
    let memory = Some(WorkerMemory::per_worker(per_worker));
    let allowance = 10 << 30;
    assert_eq!(
        Parallelism::within_shared(24, memory, allowance, 0, 0),
        Parallelism::within(24, memory, allowance),
        "nothing drawn is `within` itself"
    );
    assert_eq!(Parallelism::within(24, memory, allowance).jobs(), 7);

    let first = Parallelism::within_shared(4, memory, allowance, 0, 0);
    assert_eq!(first.jobs(), 4);
    let drawn = first.memory_bytes().unwrap();
    let second = Parallelism::within_shared(4, memory, allowance, drawn, 0);
    assert_eq!(second.jobs(), 3);
    assert_eq!(second.memory_bytes(), Some(3 * per_worker));

    let exhausted = Parallelism::within_shared(4, memory, allowance, allowance, 0);
    assert_eq!(exhausted, Parallelism::Serial { memory_bytes: Some(0) });
}

/// **What the process holds besides its arrangements comes off the
/// margin's ceiling alone.** Where the ceiling binds, a holding lowers
/// the count exactly as a draw of the same size does; where the cap binds
/// and the holding fits under the gap between them, it changes nothing a
/// draw would. **Where the count cannot fall, what the ceiling's room
/// cannot absorb comes off the budget**: one worker, or a source
/// recommending nothing, which keeps its count, spends only what the
/// holding leaves under the ceiling, and nothing once it fills it.
#[test]
fn a_holding_comes_off_the_ceiling_and_never_the_cap() {
    let per_worker = 1 << 30;
    let memory = Some(WorkerMemory::per_worker(per_worker));
    let allowance = 10 << 30;
    let held = 2 << 30;
    let holding = Parallelism::within_shared(24, memory, allowance, 0, held);
    assert_eq!(holding, Parallelism::within_shared(24, memory, allowance, held, 0));
    assert_eq!(holding.jobs(), 5);

    // Under `5 × (MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND)` the cap binds,
    // and a holding no larger than the ceiling's lead over it is free.
    let small = 600 << 20;
    let cap = small - MEMORY_RESERVE;
    let unit = Some(WorkerMemory::per_worker(cap / 4));
    let lead = margin_allowance(small) - cap;
    assert!(lead > 0);
    assert_eq!(Parallelism::within(64, unit, small).jobs(), 4);
    assert_eq!(
        Parallelism::within_shared(64, unit, small, 0, lead),
        Parallelism::within(64, unit, small),
    );
    assert_eq!(Parallelism::within_shared(64, unit, small, lead, 0).jobs(), 3);

    // One worker, the floor: the ceiling's room at one reader absorbs
    // what it can, and the rest comes off that reader's budget.
    let ceiling = margin_allowance(allowance);
    let short = 256 << 20;
    let floor = Parallelism::within_shared(4, memory, allowance, 0, ceiling - per_worker + short);
    assert_eq!(floor, Parallelism::Serial { memory_bytes: Some(per_worker - short) });
    let filled = Parallelism::within_shared(4, memory, allowance, 0, allowance);
    assert_eq!(filled, Parallelism::Serial { memory_bytes: Some(0) });

    // A source recommending nothing keeps its count and its constant
    // until the holding leaves less than the constant under the ceiling.
    let blind = Parallelism::within(8, None, allowance);
    assert_eq!(blind.memory_bytes(), Some(DEFAULT_MEMORY_BUDGET));
    let room = ceiling - DEFAULT_MEMORY_BUDGET;
    assert_eq!(Parallelism::within_shared(8, None, allowance, 0, room), blind);
    let left = 10 << 20;
    assert_eq!(
        Parallelism::within_shared(8, None, allowance, 0, ceiling - left),
        Parallelism::workers(8, left),
    );
    assert_eq!(
        Parallelism::within_shared(8, None, allowance, 0, allowance),
        Parallelism::workers(8, 0),
    );

    // Nothing held is `fit`'s budget, even past the ceiling: one worker
    // costing more than the cap is budgeted the cap.
    let narrow = 1 << 30;
    let alone = Parallelism::within_shared(1, memory, narrow, 0, 0);
    assert_eq!(alone.memory_bytes(), Some(narrow - MEMORY_RESERVE));
    assert!(narrow - MEMORY_RESERVE > margin_allowance(narrow));
}

/// **An unlimited environment falls back to the shipped constant**, and at
/// a serial count all the way back to [`Parallelism::default`], the state
/// that says nobody asked for a budget at all. A `Workers` arrangement has
/// nowhere to record that, so it carries the constant bare.
#[test]
fn nothing_discovered_and_nothing_recommended_is_todays_default() {
    let root = FakeRoot::new();
    root.v2("/leaf").write("proc/meminfo", "MemAvailable:   20002184 kB\n");
    assert_eq!(Parallelism::discover_in(root.path(), 1, None), Parallelism::default());
    assert_eq!(
        Parallelism::discover_in(root.path(), 8, None),
        Parallelism::workers(8, DEFAULT_MEMORY_BUDGET)
    );
}

/// **The closed form agrees with a search, over the whole grid.**
/// [`WorkerMemory::affords`] answers in one division above `pool_depth`
/// and by enumeration below it, which is two pieces of arithmetic where
/// the question is one — and the boundary between them is exactly where a
/// shape with a pool term stops being `n × (per_worker + pool_unit)`. So
/// it is checked against the definition it is an optimisation of: the
/// largest `n` in `1..=most` with `at(n) <= cap`, never zero.
#[test]
fn the_afforded_count_is_the_largest_one_the_cap_holds() {
    let shapes = [
        WorkerMemory::default(),
        WorkerMemory::per_worker(0).pooling(24 << 20, POOL_DEPTH),
        WorkerMemory::per_worker(34 << 20),
        WorkerMemory::per_worker(34 << 20).pooling(24 << 20, POOL_DEPTH),
        WorkerMemory::per_worker(138 << 20).pooling(128 << 20, POOL_DEPTH),
        WorkerMemory::per_worker(1).pooling(1, 1),
        WorkerMemory::per_worker(7).pooling(3, 9),
    ];
    for memory in shapes {
        for cap in [0u64, 1, 3, 7, 16, 100, 24 << 20, 128 << 20, 1 << 30, u64::MAX] {
            for most in [1usize, 2, 4, 5, 11, 64] {
                let want = (1..=most).rev().find(|n| memory.at(*n) <= cap).unwrap_or(1);
                assert_eq!(memory.affords(cap, most), want, "{memory:?} at cap {cap} most {most}");
            }
        }
    }
}

/// The bytes bind as well as the count, and they bind on the number of
/// **sub-streams**: eight workers against a budget that affords two
/// partitions is two.
#[test]
fn a_stated_budget_caps_the_worker_count_below_the_stated_jobs() {
    let eight = Parallelism::workers(8, 64 << 20);
    let per = WorkerMemory::per_worker;
    assert_eq!(worker_count(eight, per(32 << 20)), 2);
    assert_eq!(worker_count(eight, per(4 << 20)), 8);
    // A footprint larger than the whole budget still leaves one worker.
    assert_eq!(worker_count(eight, per(128 << 20)), 1);
    // A source that states no footprint is bounded by `jobs` alone.
    assert_eq!(worker_count(eight, per(0)), 8);
    // The serial state is one worker, whatever budget it carries.
    assert_eq!(worker_count(Parallelism::default(), per(32 << 20)), 1);
    assert_eq!(worker_count(Parallelism::workers(1, 1 << 30), per(32 << 20)), 1);
}

/// **A shared pool is billed, so the budget is solved rather than
/// divided.** The block pool retains a unit for every slot but the one a
/// reader is filling — `POOL_DEPTH - 1` of them below four readers and
/// `jobs - 1` above — which a per-worker charge does not carry.
#[test]
fn a_shared_pool_is_billed_before_the_count_is_handed_back() {
    let eight = Parallelism::workers(8, 64 << 20);
    let charge = WorkerMemory::per_worker(16 << 20).pooling(16 << 20, 4);
    // One reader costs 16 + 3 x 16 = 64 MiB, two cost 32 + 48 = 80 — so
    // this budget affords exactly one where the division said four.
    assert_eq!(worker_count(eight, charge), 1);
    // One byte short, and the serial floor is what is left: never zero.
    assert_eq!(worker_count(Parallelism::workers(8, (64 << 20) - 1), charge), 1);
    // Above the pool's own depth every reader takes a slot with it, so the
    // cost is `n x 32 - 16` MiB: 240 MiB affords eight, and 224 seven.
    assert_eq!(worker_count(Parallelism::workers(8, 240 << 20), charge), 8);
    assert_eq!(worker_count(Parallelism::workers(8, 224 << 20), charge), 7);
}
