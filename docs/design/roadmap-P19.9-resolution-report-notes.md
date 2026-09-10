# P19.9 — the resolution is tested, and a run says what it resolved

What the remaining slices inherit from the default becoming *visible*. The spec
row is [`roadmap-P19-efficient-defaults.md`](roadmap-P19-efficient-defaults.md),
"Slices"; how the mechanism works now is
[`architecture.md`](architecture.md), "Status output" and "Execution model and
API surface".

## What landed

- **The mode report.** `Resolved` (CLI) carries the arrangement plus its
  provenance, and `Resolved::announce` prints one line per scanning command,
  before the scan opens: `running inside a stated memory allocation` naming
  `limit_bytes`, or `no memory limit found: nothing is enforcing one on this
  process`. Both numbers carry their origin — `jobs=N (stated)` /
  `(recommended by the source)` / `(recommended by the source; lowered from M
  by the allocation)`, and `memory_bytes=N` with four spellings, `(stated)`,
  `(discovered: <file> states a limit of N byte(s))`, `(no limit found: what
  this source asks for)` and `(default: no limit found)`.
- **`MemoryLimit`** — `discover_memory_limit` answers the bytes *and* the file
  that stated them, since the walk minimises over `memory.max`, `memory.high`
  and every ancestor and only the file that bound you is actionable.
- **`PlanNoteKind::AllocationBelowFloor`** — a budget affording less than one
  reader of the source, keyed on `memory_bytes < Partitioning::partition_bytes`.
- **The three root-taking seams are public**: `discover_memory_limit_in`,
  `available_memory_in`, `Parallelism::discover_in`, plus the CLI's
  `ParallelArgs::resolve_in`.
- **The fixture tree**, `pgdump_query-cli/tests/data/runtime/` — five committed
  roots (`v2-limit`, `v1-limit`, `no-limit`, `cramped`, `below-reserve`) with a
  `README.md` saying what each states.
- **Tests**: five resolution tests in `main.rs` over those roots, two mode-report
  tests in `tests/status_output.rs` against the real binary, two below-floor
  tests in `pgdump_query/tests/partitioned_replay.rs`, and one in `io.rs` for the
  limit's own provenance.
- **The manual's falsified claim**, corrected here rather than at `19.10`:
  "`(default)` … is the one case pgdq can currently tell 'nobody asked' from
  'you asked for exactly that' in" stopped being true the moment the report
  landed.

## The call the spec did not get to make: provenance is the CLI's

The spec put the three-way distinction on "a slot that already exists" —
`scan started`'s `memory_bytes=… (default)`. That slot is the **library's**,
and the library cannot honestly fill it. Provenance splits in two and neither
half survives the trip: whether a flag was *typed* is knowable only at the CLI,
and whether a limit was *read* only inside the walk that read it. An embedder
handing over `Parallelism::default()` did no looking at all, so
`(default: no limit found)` there would assert something about an environment
nobody consulted.

So the strings the spec named are all printed, on a line whose layer can
produce them: a second `tracing::info!` from the CLI, at resolve time, ahead of
the library's own. `scan started` is unchanged and still says what bound
applies. The two alternatives are filed beside the mechanism
([`architecture.md`](architecture.md), "Status output"): widening `Parallelism`
to carry provenance — a `Copy`, `PartialEq` type matched on in every read loop,
for a display fact nothing branches on — and putting a provenance field on
`ScanOptions`, which is the same cost paid in the options struct instead.

**Flagged for review** under `STATUS.md`'s "Decisions worth another look",
because it is the one place this slice reads its spec row differently from how
the row is worded.

## Two things a later slice should not re-derive

**The limit is read even where `--parallel-memory` was stated.** The mode is a
fact about the run, not about the flag: a person who pinned a budget inside a
512 MiB cgroup is still owed the sentence. It costs a handful of small `/sys`
and `/proc` reads, and it is why `Resolved` holds `limit` separately from the
arrangement rather than deriving it.

**`query` resolves once and announces once.** It needed the same arrangement in
`QueryOptions` and in its mapping pass and used to call `resolve` twice, which
under discovery would have read the environment twice and printed the report
twice. `scan_options` now takes the already-resolved `Resolved` rather than the
flags and the source, which is what makes the single call structural instead of
remembered. `tests/status_output.rs` asserts exactly one report line on
`query`.

## The below-floor note, and why the span term is not in its key

It fires on `memory_bytes < footprint`, where `footprint` is the widest touched
partition's `partition_bytes` — **not** the divisor `worker_count` uses, which
adds the batch's span on a chunk-shaped source. Adding the span would fire the
warning on every ordinary plain `query` at the 64 MiB default, since 8 MiB of
partition plus a 64 MiB span already exceeds it. That is a property of the
default query arrangement, not a starved allocation, and a warning that is
always on is no warning.

It also cannot be keyed on anything about the *limit*, because
`plan_partitions` is library code and is not told where its budget came from.
The message says so in as many words — "the budget in force is what bound it,
and where nothing stated one it is the memory limit this process is running
under" — and the mode report is what supplies the other half.

**Two notes fire together on a starved `.xz`**, this one and
`CompressedBlockPathDeclined`, and that is intended: one says how every read of
the file is served, the other says why there is only one reader.

## What `19.15` inherits

Nothing about the rule changed — `Parallelism::fit`, `MEMORY_RESERVE` and the
composition are `19.13`'s, untouched. What is new is that its containers now
**report what they resolved**, so the probe reads a stated mode and a stated
provenance off each leg instead of inferring them from `memory_bytes` alone.
Two readings taken while landing this, on `fixtures/16/types/default.sql`, as a
smoke check and not the probe:

| container | line |
|---|---|
| `-m 512m` | `running inside a stated memory allocation jobs=1 (recommended by the source) memory_bytes=67108864 (discovered: /sys/fs/cgroup/memory.max states a limit of 536870912 byte(s)) limit_bytes=536870912` |
| `-m 256m` | the same shape at `memory_bytes=0`, and a `query` on it warns `a memory budget of 0 byte(s) is less than the 8388608 byte(s) one reader of this source holds` |

## What `19.10` still owns

The `MALLOC_ARENA_MAX` recommendation as `M76` leaves it, both flags' help text
as a whole, and the moved whole-block-decode threshold. The status-output
section of `docs/manual/dump-inspection.md` was rewritten here because its
claim about `(default)` went false; nothing else in the manual was touched.
