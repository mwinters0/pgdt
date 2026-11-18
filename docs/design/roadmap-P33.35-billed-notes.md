# P33.35 notes — what the purge delay keeps, billed

**`pgdt` bills 320 MiB beside every arrangement it resolves for what mimalloc
keeps under the 100 ms purge delay**: `alloc.rs`'s `RETAINED_BOUND`, carried
as `ArrangementRequest::retained` on every `parse` and `query` pass and, for
`pgdt sql`, as `ScanBudget::retaining` on the session budget
`datafusion_cli_pgdump::run` now builds (`decisions.md`, "D165"). It comes
off the margin's ceiling as a resident map does, and off the floor as what
stays beside a line, but off the ledger's room rather than opening its
statistics draw: a gathering parse's statistics account replaces that draw
with its own total, which would have lent the retention to its lines. An embedder
states its own, or none, which bills nothing; `pgdt`'s `system` and
`jemalloc` legs state none.

## The bound

Read off 33.34's grid (`runs/p33.34-delay-readers-20261118/readings.txt`) as
`MEMORY_UNPOOLED_BOUND` is read off `reserve`'s: the smallest 64 MiB step
(`measure.py`'s `UNPOOLED_BOUND_STEP`) covering the largest median the delay
keeps over the zero delay's. That median is five readers', 273.2 MiB, so the
bound is 320 MiB. The worst rep is left to the margin. No new reading was
taken.

- **The grid is one workload's**: 33.7's query run, `pgdt sql` over the `.xz`
  block holding the reference dump's longest line, at one to eight readers and
  at 2 GiB. A run whose 100 ms median keeps more than 320 MiB over a zero
  delay's, on any workload, moves the bound. A delay the environment states is
  billed the same 320 MiB; the manual says that a longer one keeps more.
- **The bound is the process's at any count**, so a run that keeps less than
  that (77 MiB at one reader on this grid) is over-billed by the difference.

## What it moves

- **Every carve with an allowance loses readers to it**: held comes off the
  margin's ceiling, so a bill costs a quarter more than itself in allowance,
  400 MiB (`Parallelism::within_shared`). The ceiling binds at every
  allowance once more than `MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND` is held,
  and the bill is more than that, so the cap no longer binds for any `pgdt`
  run with an allowance. The zero line, below which statistics gather
  nothing, moves from 320 MiB to 720 MiB of allowance. An uncarved estimate's
  count (half of `MemAvailable`, `pgdt`'s with no limit) is untouched, holding
  nothing off, though its ledger's room and floor lose the bill. The manual's
  worked allowances (`dump-inspection.md`, "`--jobs` and `--memory`") are
  re-computed under it: a 1 GiB container reads with four 24 MiB-block
  readers where it read with ten.
- **The floor drops by 320 MiB.** By arithmetic, the smallest allowance
  holding the reference dump's longest line rises from 33.34's 1384 MiB to
  about 1704 MiB, and `LineOutgrowsMemory` names the bill in its `beside`.
- **33.7's run at 2 GiB carves as an unbilled run at 1648 MiB would**, which
  33.34's edges put at two readers (1612 MiB carved two, 1654 MiB three). That
  is arithmetic over the edges, not a reading: 33.7 reads the count off its
  `EXPLAIN`. At two readers 33.34's every 100 ms rep peaked under 2 GiB, at
  1612 MiB's narrower ledger.

## What 33.7 inherits

- **The query run at 2 GiB under this tree's build**, its count read off
  `EXPLAIN`; two readers is still "two or more", as the spec asks.
- **`long_line_allowance` adds nothing for the bill.** It sizes
  `long-line-resident`'s allowance so that it never binds on the lines, and
  the bill comes off the room those lines draw from. The re-take checks
  whether any leg now waits.

## Negative results

- **`pgdt sql`'s plan note names the bill only where it is billed**
  (`BudgetedPlanNote`): a budget stating none says nothing of an allocator.
- **`measure.py` mirrors none of it.** `discovered_budget` and
  `afforded_readers` are upper bounds the bill only lowers further, and
  `charge_model_problem` still reads a remainder against
  `MEMORY_UNPOOLED_BOUND` with the retention in it, which P23's inbox names.
