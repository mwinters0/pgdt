use super::*;
use std::path::PathBuf;
use std::sync::Arc;

/// A source recommending whatever it is built with, so that
/// [`ParallelArgs::resolve`]'s two halves — ask the source, or honour the
/// flag — can be told apart without a real file of either shape. The three
/// required methods answer nothing; `resolve` touches no byte.
struct Recommends {
    jobs: usize,
    memory: Option<pgdt_io::WorkerMemory>,
}

impl Recommends {
    /// A source that recommends a worker count and no cost of its own —
    /// the plain file's shape, which asks for no budget.
    fn jobs(jobs: usize) -> Self {
        Self { jobs, memory: None }
    }

    /// A source that recommends both, which is what a compressed one does:
    /// a count, and what **one** of those workers holds.
    fn reader(jobs: usize, per_worker: u64) -> Self {
        Self { jobs, memory: Some(pgdt_io::WorkerMemory::per_worker(per_worker)) }
    }

    /// A block-decoding source's shape: a per-worker charge, and the
    /// retention list the pool shares over a `depth`-slot pool.
    fn block_reader(jobs: usize, per_worker: u64, unit: u64, depth: usize) -> Self {
        Self {
            jobs,
            memory: Some(pgdt_io::WorkerMemory::per_worker(per_worker).pooling(unit, depth)),
        }
    }
}

impl ByteRangeSource for Recommends {
    fn read_range(
        &self,
        _offset: u64,
        _len: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = pgdt_core::Result<bytes::Bytes>> + Send + '_>,
    > {
        Box::pin(async { Ok(bytes::Bytes::new()) })
    }
    fn size(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = pgdt_core::Result<u64>> + Send + '_>>
    {
        Box::pin(async { Ok(0) })
    }
    fn modified(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = pgdt_core::Result<Option<std::time::SystemTime>>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async { Ok(None) })
    }
    fn default_workers(&self) -> usize {
        self.jobs
    }
    fn default_worker_memory(&self) -> Option<pgdt_io::WorkerMemory> {
        self.memory
    }
}

/// One of the committed runtime roots
/// (`crates/pgdt/tests/data/runtime/README.md`) — a filesystem tree shaped like a
/// Linux one, so a resolution can be pinned against an environment this
/// machine is not in.
fn runtime_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/runtime").join(name)
}

/// One reader's charge in the flat shape every source but the
/// block-decoding one states; that shape is `BLOCK_READER` beside its
/// unit.
const READER: u64 = 58 << 20;

/// **A discovered limit is what a flagless run resolves inside, on both
/// cgroup versions** — the v1 arm being the one no machine here can
/// produce (`RT4`, `RT6`), reached through `mountinfo`. The pair is
/// consistent in both: the count is what the allowance affords under the
/// margin and the budget is what that many readers spend, never the cap
/// (`docs/design/roadmap.md`, "A default runs as fast as the allocation
/// permits").
#[test]
fn a_flagless_run_resolves_inside_a_discovered_limit_on_either_cgroup_version() {
    let flagless = ParallelArgs { jobs: None, memory: None };

    // v2, a 1 GiB `memory.max`: 640 MiB after the reserve, and 563.2 MiB
    // once the margin is left as well — nine readers of 58 MiB.
    let v2 = flagless.resolve_in(&runtime_root("v2-limit"), &Recommends::reader(24, READER));
    assert_eq!(v2.parallelism().jobs(), 9);
    assert_eq!(v2.parallelism().memory_bytes(), Some(9 * READER));
    assert_eq!(v2.limit.as_ref().map(|l| l.bytes), Some(1 << 30));

    // v1, a 512 MiB `memory.limit_in_bytes`: 128 MiB after the reserve,
    // and the margin is inert below the 640 MiB crossover — it allows
    // 153.6, which is more than the cap — so the cap is what two readers
    // of 58 MiB are fitted inside.
    let v1 = flagless.resolve_in(&runtime_root("v1-limit"), &Recommends::reader(24, READER));
    assert_eq!(v1.parallelism().jobs(), 2);
    assert_eq!(v1.parallelism().memory_bytes(), Some(2 * READER));
    assert_eq!(
        v1.limit.as_ref().map(|l| l.read_from.clone()),
        Some(runtime_root("v1-limit").join("sys/fs/cgroup/memory/svc/memory.limit_in_bytes")),
        "the v1 mount is located through mountinfo, not assumed"
    );

    // A plain source recommends nothing and is left where it was, capped
    // by the same limit.
    let plain = flagless.resolve_in(&runtime_root("v2-limit"), &Recommends::jobs(1));
    assert!(plain.parallelism().is_serial());
    assert_eq!(plain.parallelism().memory_bytes(), Some(pgdt_io::DEFAULT_MEMORY_BUDGET));
}

/// **A shared pool lowers the recommended count at every allocation.** A
/// block-decoding source's pool retains a unit for every slot but the one
/// a reader is filling — `POOL_DEPTH - 1` below four readers and
/// `jobs - 1` above — so the allowance buys fewer than a division by the
/// per-reader charge would say. Pinned at both ends: billing no pool would
/// bind nowhere, billing two units a reader would over-bill.
#[test]
fn a_shared_pool_lowers_a_recommended_count_at_every_allocation() {
    let flagless = ParallelArgs { jobs: None, memory: None };
    const UNIT: u64 = 24 << 20;
    // What one reader of a 24 MiB-block dump holds: the block it is
    // decoding and then retains, the chunk buffer a straddling read is
    // assembled into, and the decoder.
    const BLOCK_READER: u64 = 34 << 20;
    let source = || Recommends::block_reader(24, BLOCK_READER, UNIT, 4);

    // v1, a 512 MiB limit: 128 MiB after the reserve, which a division by
    // the per-reader charge calls three readers. The three units the pool
    // retains beside a single reader leave too little for a second, so one
    // is the honest answer.
    let tight = flagless.resolve_in(&runtime_root("v1-limit"), &source());
    assert_eq!(tight.parallelism().jobs(), 1);
    assert_eq!(tight.parallelism().memory_bytes(), Some(BLOCK_READER + 3 * UNIT));

    // v2, a 1 GiB limit, once the reserve and the margin are both left. A
    // division says sixteen readers; the pool slot each of them past the
    // first also takes is what makes it ten.
    let roomy = flagless.resolve_in(&runtime_root("v2-limit"), &source());
    assert_eq!(roomy.parallelism().jobs(), 10);
    assert_eq!(roomy.parallelism().memory_bytes(), Some(10 * BLOCK_READER + 9 * UNIT));
}

/// **No limit found leaves the source's recommendation standing**, capped
/// only by half of `MemAvailable` (`RT8`) — a `min` against the fallback
/// constant would make a flagless compressed scan serial on the machine
/// most likely to run it.
#[test]
fn no_limit_found_takes_the_sources_own_answer_under_the_memavailable_cap() {
    let flagless = ParallelArgs { jobs: None, memory: None };

    // Enough available that half of it does not bind, so all twenty-four
    // readers stand.
    let roomy = flagless.resolve_in(&runtime_root("no-limit"), &Recommends::reader(24, READER));
    assert_eq!(roomy.parallelism().jobs(), 24);
    assert_eq!(roomy.parallelism().memory_bytes(), Some(24 * READER));
    assert!(roomy.limit.is_none(), "every limit file states max");

    // Less available on the same unlimited arrangement: half of it is
    // four readers — the count coming down with the budget rather than
    // being printed beside one it cannot spend.
    let cramped = flagless.resolve_in(&runtime_root("cramped"), &Recommends::reader(24, READER));
    assert_eq!(cramped.parallelism().jobs(), 4);
    assert_eq!(cramped.parallelism().memory_bytes(), Some(4 * READER));

    // And a source recommending nothing on an unlimited host is the one
    // arrangement that states no budget at all — what the status line
    // renders `(default: no limit found)`.
    let plain = flagless.resolve_in(&runtime_root("no-limit"), &Recommends::jobs(1));
    assert_eq!(plain.parallelism(), Parallelism::default());
    assert_eq!(plain.budget_display(), format!("{} (default: no limit found)", 64 << 20));
}

/// **An allocation at or under the reserve resolves to a budget of zero,
/// a resolved value rather than an error.** Zero produces one reader on
/// the streaming path, and the `PlanNote` beside it tells the user their
/// allocation bound the scan (`crates/pgdt-query/tests/partitioned_replay.rs`,
/// `a_budget_below_one_readers_worth_says_the_allocation_bound_it`).
#[test]
fn an_allocation_under_the_reserve_resolves_to_one_reader_and_no_bytes() {
    let root = runtime_root("below-reserve");
    let flagless = ParallelArgs { jobs: None, memory: None };

    for source in [Recommends::reader(24, READER), Recommends::jobs(1)] {
        let resolved = flagless.resolve_in(&root, &source);
        assert_eq!(resolved.parallelism().memory_bytes(), Some(0));
        assert_eq!(resolved.parallelism().jobs(), 1, "the floor is on the count");
    }

    // A stated allowance still wins outright here: it *replaces* the
    // allocation discovery found rather than being capped by it. It pays
    // the same reserve and the same margin, whose ceiling binds below the
    // 400 MiB past the reserve, and the count beside it is nobody's statement and is cut to
    // what those bytes afford.
    let stated = ParallelArgs { jobs: None, memory: Some(pgdt_io::MEMORY_RESERVE + (400 << 20)) };
    let resolved = stated.resolve_in(&root, &Recommends::reader(24, READER));
    assert_eq!(resolved.parallelism().jobs(), 6);
    assert_eq!(resolved.parallelism().memory_bytes(), Some(6 * READER));
}

/// **A recommended count answers to the allowance however it arrived**,
/// the rule being scoped to the absence of `--jobs`
/// (`docs/design/roadmap.md`, "A default runs as fast as the allocation
/// permits").
///
/// **The bytes move too, and that is what `--memory` changed**: the
/// allowance is carved, so neither number the run reports is the one that
/// was typed (`docs/design/decisions.md`, "D83").
#[test]
fn a_stated_allowance_lowers_a_recommended_count_and_is_carved_not_kept() {
    let root = runtime_root("no-limit");
    let source = Recommends::reader(24, READER);
    // An allowance leaving 32 MiB after the reserve — and 32 MiB affords
    // no whole reader at all, which is the floor: one.
    let allowance = pgdt_io::MEMORY_RESERVE + (32 << 20);

    let tight = ParallelArgs { jobs: None, memory: Some(allowance) };
    let resolved = tight.resolve_in(&root, &source);
    assert_eq!(resolved.parallelism().jobs(), 1);
    assert_eq!(resolved.parallelism().memory_bytes(), Some(32 << 20));
    assert_eq!(
        resolved.jobs_display(),
        "1 (recommended by the source; lowered from 24 by the stated allowance)",
        "the clause names the flag to raise, not a cgroup nobody set"
    );
    assert_eq!(
        resolved.budget_display(),
        format!("{} (stated: --memory allows {allowance} resident byte(s))", 32u64 << 20),
        "the line carries both numbers, the typed one no longer being the budget"
    );

    // Room for more than the source asked for leaves the recommendation
    // standing, and the line says nothing about a lowering.
    let roomy = ParallelArgs { jobs: None, memory: Some(64 << 30) };
    let resolved = roomy.resolve_in(&root, &source);
    assert_eq!(resolved.parallelism().jobs(), 24);
    assert_eq!(resolved.jobs_display(), "24 (recommended by the source)");

    // A stated count is still printed as typed and never lowered, with
    // the same budget beside it — the asymmetry this arm preserves.
    let both = ParallelArgs { jobs: Some(24), memory: Some(allowance) };
    let resolved = both.resolve_in(&root, &source);
    assert_eq!(resolved.parallelism().jobs(), 24);
    assert_eq!(resolved.jobs_display(), "24 (stated)");
    assert_eq!(resolved.parallelism().memory_bytes(), Some(32 << 20));

    // A source recommending no per-worker cost has nothing to divide by,
    // and is left on the library's constant under the cap — exactly what a
    // discovered limit leaves it.
    let plain = tight.resolve_in(&root, &Recommends::jobs(1));
    assert_eq!(plain.parallelism().jobs(), 1);
    assert_eq!(plain.parallelism().memory_bytes(), Some(32 << 20));
}

/// **The statistics a cache hands the scan are carved around, not after**
/// (`docs/design/decisions.md`, "D85"): billed as held against the
/// margin's ceiling, they lower a recommended count first, and where the
/// count cannot fall — a source recommending no cost — the budget, so the
/// statistics allowance still covers what the cache holds. A discovered
/// limit and the same number stated carve them alike.
#[test]
fn a_cache_s_statistics_lower_the_count_and_then_the_budget() {
    let reader = Recommends::reader(24, READER);
    let limited = runtime_root("v2-limit");
    let flagless = ParallelArgs { jobs: None, memory: None };
    assert_eq!(flagless.discover_in(&limited).resolve(&reader, 0).parallelism().jobs(), 9);
    // Two readers' worth held: the 563.2 MiB ceiling less 116 MiB affords
    // seven, and the room seven leave absorbs the holding whole.
    let holding = flagless.discover_in(&limited).resolve(&reader, 2 * READER);
    assert_eq!(holding.parallelism().jobs(), 7);
    assert_eq!(holding.parallelism().memory_bytes(), Some(7 * READER));

    let allowance = 1u64 << 30;
    let unlimited = runtime_root("no-limit");
    let stated = ParallelArgs { jobs: None, memory: Some(allowance) };
    assert_eq!(
        stated.discover_in(&unlimited).resolve(&reader, 2 * READER).parallelism(),
        holding.parallelism(),
        "the same number stated is the same carving"
    );

    // A plain source's count cannot fall, so what the room beside its
    // budget cannot absorb comes off the budget, and the statistics
    // allowance is then exactly what the cache holds.
    let plain = Recommends::jobs(1);
    let budget = pgdt_io::DEFAULT_MEMORY_BUDGET;
    let room = pgdt_io::statistics_allowance(allowance, budget);
    let absorbed = stated.discover_in(&unlimited).resolve(&plain, room);
    assert_eq!(absorbed.parallelism().memory_bytes(), Some(budget));
    let short = 16 << 20;
    let squeezed = stated.discover_in(&unlimited).resolve(&plain, room + short);
    assert_eq!(squeezed.parallelism().memory_bytes(), Some(budget - short));
    assert_eq!(squeezed.statistics_allowance(), Some(room + short));
}

/// **`query`'s replay is carved with nothing held**, its mapping pass
/// around the cache's statistics: the same holding that lowers the
/// mapping pass's count leaves the replay the arrangement a cache without
/// statistics would, under a discovered limit and a stated one alike.
#[test]
fn a_query_s_replay_is_carved_around_no_statistics() {
    let reader = Recommends::reader(24, READER);
    let flagless = ParallelArgs { jobs: None, memory: None };
    let limited = runtime_root("v2-limit");
    let stated = ParallelArgs { jobs: None, memory: Some(1 << 30) };
    let unlimited = runtime_root("no-limit");
    for discovered in [flagless.discover_in(&limited), stated.discover_in(&unlimited)] {
        let passes = discovered.resolve_query(&reader, 2 * READER);
        let bare = discovered.resolve(&reader, 0).parallelism();
        assert_eq!(
            passes.mapping.parallelism(),
            discovered.resolve(&reader, 2 * READER).parallelism()
        );
        assert_eq!(passes.mapping.parallelism().jobs(), 7);
        assert_eq!(passes.replay.parallelism(), bare);
        assert_eq!(bare.jobs(), 9);
        // What the mapping span's `replay_jobs=` says: seven of the nine,
        // cut by the statistics; with none held the passes agree.
        assert_eq!(passes.mapping_shortfall(), Some((7, 9)));
        assert_eq!(discovered.resolve_query(&reader, 0).mapping_shortfall(), None);
    }
    // A stated count is never lowered, so only its budget can differ.
    let typed = ParallelArgs { jobs: Some(9), memory: None };
    let passes = typed.discover_in(&limited).resolve_query(&reader, 2 * READER);
    assert_eq!(passes.mapping_shortfall(), None);
}

/// **What a mapping pass's line reads as, in the format `pgdt` installs**:
/// `held_bytes` wherever the cache holds statistics, `replay_jobs` only
/// where they cut the count, and no span at all where it holds none. The
/// binary's own test (`a_query_s_mapping_pass_says_what_it_was_carved_around`)
/// reaches this through the real `await` on a plain source, whose count
/// cannot fall; the cut count is reached here, under a runtime root's
/// limit.
#[test]
fn the_mapping_span_carries_the_holding_and_a_cut_count() {
    #[derive(Clone, Default)]
    struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let rendered = |passes: &QueryPasses| -> String {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_target(false)
            .without_time()
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            passes.mapping_span().in_scope(|| tracing::info!(jobs = 7, "scan started"));
        });
        String::from_utf8(captured.0.lock().unwrap().clone()).unwrap()
    };

    let reader = Recommends::reader(24, READER);
    let limited = runtime_root("v2-limit");
    let flagless = ParallelArgs { jobs: None, memory: None }.discover_in(&limited);
    let cut = rendered(&flagless.resolve_query(&reader, 2 * READER));
    let held = 2 * READER;
    assert!(
        cut.contains(&format!("mapping{{held_bytes={held} replay_jobs=9}}: scan started jobs=7")),
        "{cut}"
    );

    let typed = ParallelArgs { jobs: Some(9), memory: None }.discover_in(&limited);
    let uncut = rendered(&typed.resolve_query(&reader, held));
    assert!(uncut.contains(&format!("mapping{{held_bytes={held}}}: scan started")), "{uncut}");

    let none = rendered(&flagless.resolve_query(&reader, 0));
    assert!(!none.contains("mapping"), "{none}");
    assert!(none.contains("scan started"), "{none}");
}

/// **The check `introspect` owes: the instrument must not move the plan it
/// reports on.** It changes what the process *holds*, which is the point;
/// what it must not change is what the process *resolves*.
///
/// Compiled into both configurations against the same literals, so
/// `cargo test -p pgdt` and `cargo test -p pgdt --features introspect` are
/// the two halves of the comparison. The roots are the committed ones
/// rather than this machine's `/` (`tests/data/runtime/README.md`), and
/// every one of them is swept: the arms differ in which term binds — a
/// discovered ceiling, `RT8`'s half-`MemAvailable` cap, the reserve
/// leaving nothing.
#[test]
fn the_instrument_build_resolves_what_the_default_build_resolves() {
    let flagless = ParallelArgs { jobs: None, memory: None };
    let source = Recommends::reader(24, READER);

    let resolved: Vec<(usize, Option<u64>)> =
        ["v2-limit", "v1-limit", "no-limit", "cramped", "below-reserve"]
            .iter()
            .map(|name| {
                let r = flagless.resolve_in(&runtime_root(name), &source);
                (r.parallelism().jobs(), r.parallelism().memory_bytes())
            })
            .collect();

    assert_eq!(
        resolved,
        vec![
            (9, Some(9 * READER)),
            (2, Some(2 * READER)),
            (24, Some(24 * READER)),
            (4, Some(4 * READER)),
            (1, Some(0)),
        ],
        "the resolved arrangement moved: `--features introspect` and the default build must \
             read this identically"
    );
}

/// **`jobs=` reads differently by provenance, and the report says so**: a
/// recommended count is printed lowered to what the allowance affords, a
/// stated `--jobs` as typed — so the same two readers can appear under
/// either, and only the recommended line says what will run.
#[test]
fn the_report_says_which_of_the_two_counts_a_reader_is_looking_at() {
    let root = runtime_root("cramped");
    let source = Recommends::reader(24, READER);

    let flagless = ParallelArgs { jobs: None, memory: None };
    let recommended = flagless.resolve_in(&root, &source);
    assert_eq!(recommended.parallelism().jobs(), 4);
    assert_eq!(
        recommended.jobs_display(),
        "4 (recommended by the source; lowered from 24 by the allocation)"
    );

    let asked = ParallelArgs { jobs: Some(24), memory: None };
    let stated = asked.resolve_in(&root, &source);
    assert_eq!(stated.parallelism().jobs(), 24, "a stated count is not lowered");
    assert_eq!(stated.jobs_display(), "24 (stated)");
    // Both arrangements hold the same bytes, which is why the two lines
    // have to read differently.
    assert_eq!(stated.parallelism().memory_bytes(), recommended.parallelism().memory_bytes());
}

/// **The budget's provenance has four spellings and each names a different
/// way of arriving at a number** — the flag, a limit that was read, a
/// source's own answer under no limit, and the library's constant
/// (`docs/design/decisions.md`, "D64").
#[test]
fn the_budget_line_names_where_its_number_came_from() {
    let reader = Recommends::reader(24, READER);
    let plain = Recommends::jobs(1);

    let allowance = pgdt_io::MEMORY_RESERVE + (400 << 20);
    let stated = ParallelArgs { jobs: None, memory: Some(allowance) };
    assert_eq!(
        stated.resolve_in(&runtime_root("no-limit"), &reader).budget_display(),
        format!("{} (stated: --memory allows {allowance} resident byte(s))", 6 * READER)
    );

    let flagless = ParallelArgs { jobs: None, memory: None };
    let discovered = flagless.resolve_in(&runtime_root("v2-limit"), &reader);
    let line = discovered.budget_display();
    assert!(line.starts_with(&format!("{} (discovered:", 9 * READER)), "{line}");
    assert!(line.contains("sys/fs/cgroup/pgdt/memory.max"), "{line}");
    assert!(line.contains(&format!("{}", 1u64 << 30)), "{line}");

    assert_eq!(
        flagless.resolve_in(&runtime_root("no-limit"), &reader).budget_display(),
        format!("{} (no limit found: what this source asks for)", 24 * READER)
    );
    assert_eq!(
        flagless.resolve_in(&runtime_root("no-limit"), &plain).budget_display(),
        format!("{} (default: no limit found)", 64 << 20)
    );
}

/// **The statistics line reads `(none: …)` only where `MemAvailable`
/// cannot be read**: a host reporting none available still states a
/// number, half of nothing, and says where it came from.
#[test]
fn the_statistics_line_is_none_only_where_available_memory_is_unreadable() {
    let flagless = ParallelArgs { jobs: None, memory: None };
    let reader = Recommends::reader(24, READER);

    let unreadable = tempfile::tempdir().unwrap();
    assert_eq!(
        flagless.resolve_in(unreadable.path(), &reader).statistics_allowance_display(),
        "(none: no limit found and the machine's available memory could not be read)"
    );

    let exhausted = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(exhausted.path().join("proc")).unwrap();
    std::fs::write(exhausted.path().join("proc/meminfo"), "MemAvailable: 0 kB\n").unwrap();
    let line = flagless.resolve_in(exhausted.path(), &reader).statistics_allowance_display();
    assert!(line.ends_with("(half of what the machine reports available)"), "{line}");
}

/// **A plan note names the budget that bound the plan; the clause beside
/// it names where that budget came from** — under discovery the number to
/// change is the *allocation*, which the note cannot know about
/// (`docs/design/decisions.md`, "I/O, memory and parallelism").
///
/// **A stated allowance earns one too**, the note naming the carved budget
/// rather than the number that person typed
/// (`docs/design/decisions.md`, "D83").
#[test]
fn a_plan_note_says_where_the_budget_that_bound_it_came_from() {
    let reader = Recommends::reader(24, READER);

    let flagless = ParallelArgs { jobs: None, memory: None };
    let clause = flagless.resolve_in(&runtime_root("v1-limit"), &reader).plan_note_origin();
    assert!(clause.starts_with(" — the budget in force is "), "{clause}");
    assert!(clause.contains("memory.limit_in_bytes"), "the file that stated it: {clause}");
    assert!(clause.contains(&format!("{}", 512u64 << 20)), "the limit itself: {clause}");

    // No limit found still earns a clause: the budget is the source's own
    // ask, equally not the user's.
    let unlimited = flagless.resolve_in(&runtime_root("no-limit"), &reader);
    assert_eq!(
        unlimited.plan_note_origin(),
        format!(
            " — the budget in force is {} (no limit found: what this source asks for)",
            24 * READER
        )
    );

    let allowance = pgdt_io::MEMORY_RESERVE + (400 << 20);
    let stated = ParallelArgs { jobs: None, memory: Some(allowance) };
    assert_eq!(
        stated.resolve_in(&runtime_root("v1-limit"), &reader).plan_note_origin(),
        format!(
            " — the budget in force is {} (stated: --memory allows {allowance} resident byte(s))",
            6 * READER
        )
    );
}

/// The budget a flagless resolution lands on **here**, on whatever machine
/// the suite is running on: `None` — nobody stated one — only where no
/// memory limit is discovered, and the discovered allowance capped at the
/// library's own constant where one is.
///
/// **The branch is the thing being pinned, not the number**: the answer
/// depends on the cgroup the test process is in, and a test naming the
/// constant would pass on a bare host and fail in a container. Every
/// precedence claim below is asserted against this rather than around it.
fn flagless_budget() -> Option<u64> {
    pgdt_io::discover_memory_limit().map(|limit| {
        pgdt_io::DEFAULT_MEMORY_BUDGET.min(limit.bytes.saturating_sub(pgdt_io::MEMORY_RESERVE))
    })
}

/// **Stating neither flag asks the source**, which is the one thing about
/// `--jobs` no integration test can see: a partitioned run and a serial
/// one produce the same bytes, so a default that silently reverted to a
/// constant would pass every other test in the tree
/// (`docs/design/measurements.md`, "The apparatus"). Which source
/// recommends which count is `ByteRangeSource::default_workers`'s test.
#[test]
fn stating_no_parallelism_flag_asks_the_source() {
    let budget = flagless_budget();
    let filled = budget.unwrap_or(pgdt_io::DEFAULT_MEMORY_BUDGET);
    let stated = ParallelArgs { jobs: None, memory: None };

    // A source recommending the serial path gets it, carrying whatever
    // the environment allows — and `None`, rendered `(default)`, where no
    // limit was found.
    assert!(stated.resolve(&Recommends::jobs(1)).parallelism().is_serial());
    assert_eq!(stated.resolve(&Recommends::jobs(1)).parallelism().memory_bytes(), budget);

    // `Parallelism::Workers` has nowhere to record that nobody stated a
    // budget, so it carries the fallback bare.
    assert_eq!(stated.resolve(&Recommends::jobs(8)).parallelism(), Parallelism::workers(8, filled));

    // A stated flag wins outright over a recommendation in either
    // direction.
    let asked = ParallelArgs { jobs: Some(8), memory: None };
    assert_eq!(asked.resolve(&Recommends::jobs(1)).parallelism(), Parallelism::workers(8, filled));
    let serial = ParallelArgs { jobs: Some(1), memory: None };
    assert!(serial.resolve(&Recommends::jobs(24)).parallelism().is_serial());
    assert_eq!(serial.resolve(&Recommends::jobs(24)).parallelism().memory_bytes(), budget);
}

/// **A carved budget survives a serial worker count**: the collapse that
/// makes `--jobs 1` serial takes the *worker count* down and not the bytes
/// beside it, so `--memory` alone is the whole recourse for a
/// compressed file whose blocks the default cannot hold
/// (`docs/design/decisions.md`, "I/O, memory and parallelism").
///
/// The source here recommends nothing, which is the plain-file shape, so
/// the budget it lands on is the library's constant under the allowance's
/// cap — the same number a discovered limit of that size leaves it
/// (`docs/design/decisions.md`, "D83").
#[test]
fn a_carved_budget_reaches_the_library_at_a_serial_job_count() {
    let allowance = pgdt_io::MEMORY_RESERVE + (400 << 20);
    let stated = ParallelArgs { jobs: None, memory: Some(allowance) };
    let serial = Recommends::jobs(1);
    assert!(
        stated.resolve(&serial).parallelism().is_serial(),
        "one worker is still the serial path"
    );
    assert_eq!(
        stated.resolve(&serial).parallelism().memory_bytes(),
        Some(pgdt_io::DEFAULT_MEMORY_BUDGET)
    );

    // Stated explicitly rather than taken from the source: the same
    // value either way.
    let one = ParallelArgs { jobs: Some(1), memory: Some(allowance) };
    assert_eq!(
        one.resolve(&Recommends::jobs(24)).parallelism(),
        stated.resolve(&serial).parallelism()
    );

    // A stated count with no budget beside it is the other half of the
    // pair, and the budget then comes from the environment.
    let jobs_only = ParallelArgs { jobs: Some(1), memory: None };
    assert_eq!(jobs_only.resolve(&serial).parallelism().memory_bytes(), flagless_budget());
}
