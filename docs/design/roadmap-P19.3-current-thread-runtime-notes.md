# P19.3 — the CLI runs a `current_thread` runtime

`pgdq`'s `main` is `#[tokio::main(flavor = "current_thread")]` and neither of
`pgdump_query-cli/Cargo.toml`'s two `tokio` lines names `rt-multi-thread` any
more. Nothing else in the tree adds it, so the feature is gone from the
workspace: the library already kept `tokio` at `rt` + `sync`, and the binary was
the only thing turning the multi-threaded flavour on.

This is the second of the spec's four binding orderings — *"the `current_thread`
runtime before the reserve is measured"* — so what `19.6` measures is a reserve
for the thread count below, not for the one that shipped through `19.2`.

## What the threads now are, measured

At `--jobs 4` on this 24-CPU machine, over the 3.00 GiB plain control
(`/mnt/ssd/fedora/scratch/pgdump_query/measure/control43.sql`), sampling
`/proc/<pid>/status`:

| build | `Threads` |
|---|---|
| `runs/pgdq-19.2-stock` (`5b6e413`) | 26 rising to 34 |
| this one | 2 rising to 8 |

Both wrote a **byte-identical** cache (`cmp`), which is the property the
determinism test already asserts across `--jobs`; the run above is the same
check against a runtime change rather than against a worker count.

The 8 is main plus the blocking pool, and the pool is created **on demand**, so
the number is a function of how much work is in flight — not of `--jobs`
exactly, and not of the CPU count at all. That is the whole result: **the
process no longer sizes itself from a number nobody stated.**

## It is a thread-count result, and the memory claim is not made

The spec warns that a slice selling this as a memory result owes a reading, and
the reading does not support one. Peak resident over the same control was
~10.5 MiB before and ~9.8 MiB after — a plain 3 GiB file at `--jobs 4` never
grows the arena set enough for it to show, and the shape where it *does* show is
koji's 24 MiB-block `.xz` under a 512 MB cgroup, which is an hour's run. The
existing probe already says fewer arenas is not proportionally fewer bytes: 24
arenas at ~536 MiB against 8 at ~476 MiB
([`architecture.md`](architecture.md), "Execution model and API surface").

So `19.6` still has to measure the reserve, and it measures it uncapped, exactly
as specified. What this slice changes for it is the apparatus, not the argument.

## The interrupt guard was the risk, and it was checked rather than reasoned

`install_interrupt_guard` registers its `SIGINT`/`SIGTERM` handlers as
`tokio::spawn`ed tasks. On a multi-threaded runtime those get a thread of their
own; on a `current_thread` one they share the thread with `main`'s future, so
they run only when the runtime is parked in `block_on` or the main task yields.
That is satisfied by construction — `leader::scan_partition` awaits a
`spawn_blocking` join for every piece, and the serial loop awaits `read_range`,
which is also `spawn_blocking` — but "by construction" is what this project
distrusts about a signal path, so it was run:

```
pgdq parse --source control43.sql --dqcache ./c.dqcache --jobs 4 \
    --parallel-memory 1073741824 &
sleep 0.35; kill -TERM $!
```

→ exit **143**, `interrupted at byte 425 of 3221226366 — the cache at
./c.dqcache holds the scan so far`, and the resume line. Unchanged from the
multi-threaded build.

**There is still no automated test for it.** The CLI's suite constructs the
interrupted-cache *state* directly (`tests/partial_reporting.rs`,
`tests/status_output.rs`) rather than signalling a process, and this slice did
not add one — a test that races a signal against a scan fast enough to finish
first is a flake, and the shape that is slow enough to signal reliably is a
multi-gigabyte input no test may stage. The check above is the evidence, and
koji's stop-report-resume-compare wrap is what exercises it at scale.

## What went stale, and why nothing was acknowledged

`measure.py --stale` reports every figure declaring `pgdump_query-cli/src/` or
`pgdump_query-cli/Cargo.toml` as red, which is most of the register — but all of
them were already red at `af15eac` from earlier commits, so this slice adds no
new stanza and there is nothing an acknowledgement could turn green. The
`--verify-additive` oracle does not apply either: it settles generator changes,
and this is a binary change. The closing sweep (`19.11`) is what discharges
them.

Two figures are worth naming for whoever takes that sweep, because their
apparatus genuinely moved rather than merely being declared stale:
`parallel-peak-rss` and `peak-rss` both read resident on a binary that used to
start a thread per CPU and now does not.

## Claims corrected in the same change

Three documents asserted the old runtime in present tense, so they were fixed
here rather than left to `19.10`:

- **[`../manual/dump-inspection.md`](../manual/dump-inspection.md)**, the
  container-memory callout — it said *"pgdq's runtime starts one worker thread
  per CPU it can see"*, and *"restricting the container's CPUs … reduces the
  thread count"*, which is now not even a lever. The `MALLOC_ARENA_MAX`
  recommendation stands untouched; only the mechanism sentence changed, and the
  536/328 MiB figures are re-attributed to the arena *count* they were measured
  at rather than to the host's CPU count. This is the doc where the rule binds
  absolutely (`../process.md`, "Where does this fact go?").
- **[`architecture.md`](architecture.md)**, "Execution model and API surface" —
  the CLI's flavour is now stated beside the library's refusal to impose one,
  and the arena paragraph says outright that its readings were taken when the
  thread count was the CPU count. Those readings are still true of the arena
  mechanism; what they no longer describe is the count pgdq reaches by default.
- **[`roadmap-P14-remote-input-inbox.md`](roadmap-P14-remote-input-inbox.md)**,
  "The sync/async seam bites here and nowhere else" — it said the library leaves
  `rt-multi-thread` "to the binary". The entry's conclusion is unaffected (the
  pre-fetched window still composes on a single-threaded runtime; concurrent
  ranged GETs are futures, not threads), so the clause was corrected in place
  rather than the entry withdrawn.

[`measurements.md`](measurements.md)'s koji section was **left alone**: its
`--cpus 4` sentence argues about the apparatus of a run taken at `f5768e7`, and
that run's binary did size its runtime from the CPU count. It is already
declared stale at that commit.

## What `19.4` and later inherit

- **`--jobs` is now the only thing that moves the thread count**, which is what
  makes `19.7`'s budget rule about a number the process can actually be held
  to. Before this, a discovered budget would have been divided among pools while
  an undeclared thread-per-CPU set sat on top of it.
- **No test pins the thread count.** It is asserted nowhere; the table above is
  a probe. If a later slice wants it pinned, `/proc/self/status` is readable
  from an integration test, but the number is scheduling-dependent (2 → 8 as the
  pool fills) so an assertion would have to be an upper bound, not an equality.
- **`cargo test` now compiles the binary under the same tokio feature set the
  shipped binary gets**, because the dev-dependency was trimmed to `rt` too.
  Every `#[tokio::test]` in the crate takes the default `current_thread`
  flavour, so nothing needed the multi-threaded one; a future test that does
  must put `rt-multi-thread` back in `[dev-dependencies]` and will silently
  re-enable it for the binary's test build when it does.
