# P16.15 — the stated budget bounds both memory terms

`STATUS.md`'s row, as amended by the review of 2026-09-08
([`../status/history/2026-09-08.md`](../status/history/2026-09-08.md), "The
block pool's bound is a divisor's job, not an acquisition's"), committed this
slice to four things: the divisor formula itself, a decision on whether
`BlockCache`'s retained cap and `BufferPool`'s free-list cap become one count,
a re-derivation of `POOL_DEPTH`'s four-worker plain-file ceiling, and a re-take
of `parallel-peak-rss`. This slice lands the first three; the fourth is
**earned as `16.15.1`**, because a figure's sitting needs a commit to name and
this round leaves no commit — the same seam `16.13` split on
([`roadmap-P16.13.1-parallel-figures-sitting-notes.md`](roadmap-P16.13.1-parallel-figures-sitting-notes.md)).

## The divisor

`stream::plan_partitions` — the query replay's sub-stream planner — now caps
`worker_count` against `partition_bytes + max_source_span` rather than
`partition_bytes` alone: what a concurrent reader costs the source to decode,
plus what a sub-stream's held batch may pin before it flushes
(`QueryOptions::max_source_span`, 64 MiB by default). `None` (an unbounded
span) falls back to the decode footprint alone — the caller has already opted
out of a batch-size bound, so there is no second number to add.

**This is the query replay's own call site, not `worker_count` itself.** The
function stays a one-argument division; the caller sums the two costs before
calling it, because a discovery worker (`leader::scan_region`, the mapping
pass) retains nothing past its own `read_range` and has no second term to add
— folding the sum into `worker_count` would give the mapping pass's call a
term it does not owe.

**Consequence: `pgdq query --jobs N` with `--parallel-memory` left at its
64 MiB default now always runs one sub-stream**, however large `N` is, because
the span term alone already meets the budget. Raising `--parallel-memory` past
roughly 65 MiB is what buys a second sub-stream. This is the corrected reading
of a bound that previously priced only the decode side and silently ignored
what a batch pins — the spec's own amendment names this exact consequence and
says the flags' defaults are unchanged, only what a person who states neither
gets. `--jobs` and `--parallel-memory` help text and
[`../manual/dump-inspection.md`](../manual/dump-inspection.md) both say so now.
`parse` is untouched: it builds no batches, so its own worker admission
(`leader::scan_region`) has nothing to add and its ceiling is unchanged.

Tests: `pgdump_query/tests/partitioned_replay.rs`'s
`a_tight_budget_hands_out_fewer_sub_streams_than_jobs` was rewritten to state
the two terms explicitly rather than rely on the single-term arithmetic it
was written against; `an_unbounded_span_falls_back_to_the_decode_footprint_alone`
is new, covering the `None` case. Every other test in that file uses a budget
(1 GiB) large enough that the 64 MiB default span term does not bind below the
job counts those tests state, so none needed touching.

## Two decisions, made rather than left open

**`BlockCache`'s retained cap and `BufferPool`'s free-list cap stay separate.**
The 09-08 review found the pool's real ceiling is `2 × slots × unit`, not the
`slots × unit` two documents had claimed — coupling the two counts would tighten
that transient ceiling, but it is a rework of `BufferPool::release`'s hot path
for a bound that binds only in the specific window between a live view's
release and the next acquisition, not in steady state, and the actual
accounting gap this slice exists to close — a query's held batches not being
priced into how many concurrent readers are admitted — is what the divisor
above fixes from the outside. Filed as a rejected alternative beside the
mechanism: [`architecture.md`](architecture.md), "Execution model and API
surface" (`*Rejected: coupling the two counts…*`).

**`POOL_DEPTH` stays fixed at 4.** It bounds the chunk pool's depth, which is a
different call site from the one this slice's divisor touches: a leader
worker on the mapping pass (`parse`, or a query's own mapping scan) retains
nothing past its `read_range`, so the resident-set argument against raising it
with `jobs` — a query's `RetainedChunks` pinning `(jobs - POOL_DEPTH)` idle
chunks once a batch flushes — has no analog there. The four-worker plain-file
ceiling this produces is a property of the chunk pool's own depth, unrelated
to what a query's sub-stream count divides by. Re-derivation, not a change:
[`architecture.md`](architecture.md), "Execution model and API surface"
(`**POOL_DEPTH stays fixed…**`).

## What this touches beyond what the row anticipated

**`parallel-scan-throughput`'s typed-query legs are reachable and affected,
not only `parallel-peak-rss`.** The row's text named only the RSS figure,
written when the review centered on memory retention; the divisor change lands
on the same call site the throughput figure's typed-query rows exercise.
`measure.py`'s `PARALLEL_BUDGET` (1 GiB) was sized against the *old* one-term
arithmetic — its own comment computes "24 workers over 24 MiB blocks want
~600 MiB" — and the new divisor adds 64 MiB a worker: 24 workers now want
`24 × (25 MiB + 64 MiB) ≈ 2.1 GiB` against the same 1 GiB, so `worker_count`
caps the achieved sub-stream count at roughly 11 for that leg rather than the
stated 24. `parallel-peak-rss` is unreachable from this slice — its command
shape is `pgdq parse` (`parse-rss-jobs-N`), which never calls
`plan_partitions` — so its numbers are not expected to move; both figures are
carried into `16.15.1` regardless, since the row committed to the RSS one and
the throughput one now needs the same treatment for the same reason.

**Whether `PARALLEL_BUDGET` should rise to keep testing the full `--jobs 24`
axis, or the table should show the corrected narrower achievable range, is
filed under `STATUS.md`'s "Decisions worth another look"** — it is the
apparatus's call, not this slice's, and it should be settled before `16.15.1`
runs rather than guessed at.

## What `16.15.1` inherits

- **A commit to name.** Neither figure can be folded into
  [`measurements.md`](measurements.md) from a dirty tree —
  `scripts/measure.py`'s own dirty-tree accounting is why, and it is the same
  reason `16.13` split into `16.13.1`. Once this diff is committed, run
  `cd scripts && uv run measure.py --figure parallel-scan-throughput --figure
  parallel-peak-rss` (after the `PARALLEL_BUDGET` question above is settled)
  and fold both tables in with the commit inside each marker.
- **A predicted direction for each table.** `parallel-scan-throughput`'s
  plain-`parse` and `.xz`-`parse` rows, and its plain-`query` rows, should be
  unchanged (LocalFileSource's `partitions()` answer has no span term folded
  in — a plain source's `partition_bytes` is already what `worker_count` divides
  by, on both the leader's call and this one, so nothing there moved).
  `.xz`-typed-`query` rows above roughly eight jobs should show a *lower*
  achieved sub-stream count than published, unless `PARALLEL_BUDGET` is raised
  first. `parallel-peak-rss`'s two `parse` legs at every job count should
  reproduce the `e29939c` numbers inside normal rep spread — a repeat measuring
  the same code path.
- **`--stale` already carries the mechanical evidence.** `cd scripts && uv run
  measure.py --stale` names `pgdump_query-cli/src/main.rs` and
  `pgdump_query/src/stream.rs` as what moved both figures past their `e29939c`
  sitting; nothing else is owed before the sitting except the commit itself.
