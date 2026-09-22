# P6.5.2 — The session's holdings come off the margin alone: notes

A slice earned after 6.5.1 of
[`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md), from the review of its
call ([`../status/history/2026-09-22.md`](../status/history/2026-09-22.md),
"Which bound the holdings come off"). `Parallelism::within_shared` takes a
fifth argument, `held`, which comes off `margin_allowance`'s ceiling and not
the cap; `ScanBudget::draw` passes the billed maps and the pool limit there and
the live scans' draws as `drawn`, as before. The reasoning is on both items'
rustdoc. The checks are `a_holding_comes_off_the_ceiling_and_never_the_cap`
(`pgdump_query/src/io.rs`), `holdings_lower_the_count_and_never_the_budget`
and `a_finite_pool_limit_is_read_off_the_session`
(`datafusion-pgdump/src/budget.rs`).

## What later slices inherit

- **A holding lowers a scan's count, never the budget that count spends.**
  `Parallelism::fit` bounds the count by the ceiling and the budget by the cap
  (D3), so a scan's budget is still what its count spends however much the
  session holds.
- **A source recommending no per-reader memory does not see the holdings at
  all.** `fit` keeps such a source's count and caps its budget at
  `DEFAULT_MEMORY_BUDGET` against the cap alone, so a plain dump's scan draws
  its default whatever the pool limit and the maps leave; the integration test
  `the_resident_statistics_are_billed_and_leave_a_budget_alone` pins it. What a
  plain scan's budget is worth is `KD25` and `KD32`.
- **Holdings filling the margin leave one reader at what one reader costs**,
  where scans having drawn the whole allowance leave one reader on nothing.
  The floor is `affords`'s one worker, as ever.
- **`Parallelism::within`, `pgdt`'s carving, passes `held = 0`**: statistics
  are carved there after the workers (`statistics_allowance`, D85).

## Negative results

- **The provider's end-to-end test can no longer see a holding move a draw**:
  every fixture is plain, and a plain scan's count is not ceiling-bounded. The
  arithmetic is pinned in the two crates' unit tests instead, `draw` being
  crate-private and taking the source's `WorkerMemory` directly. An `.xz`
  fixture would let the integration test see it; none exists.
- **No `D<k>` entry.** The register is at its cap, and the shape and both
  rejected alternatives are on `ScanBudget`'s rustdoc.
- **No manual page, no figure, and no cache format change.**
