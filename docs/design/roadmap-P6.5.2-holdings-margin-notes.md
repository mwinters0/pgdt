# P6.5.2 — The session's holdings come off the margin alone: notes

A slice earned after 6.5.1 of
[`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md), from the review of its
call ([`../status/history/2026-09-22.md`](../status/history/2026-09-22.md),
"Which bound the holdings come off"). `Parallelism::within_shared` takes a
fifth argument, `held`, which comes off `margin_allowance`'s ceiling and not
the cap; `ScanBudget::draw` passes the billed maps and the pool limit there and
the live scans' draws as `drawn`, as before. The reasoning is on both items'
rustdoc. The checks are `a_holding_comes_off_the_ceiling_and_never_the_cap`
(`pgdump_query/src/io.rs`), `holdings_lower_the_count_before_the_budget`
and `a_finite_pool_limit_is_read_off_the_session`
(`datafusion-pgdump/src/budget.rs`).

## What later slices inherit

- **A holding lowers a scan's count before its budget.** `Parallelism::fit`
  bounds the count by the ceiling and the budget by the cap (D3); where the
  count cannot fall, what the ceiling cannot absorb comes off the budget, which
  6.5.3 built
  ([`roadmap-P6.5.3-holdings-floors-notes.md`](roadmap-P6.5.3-holdings-floors-notes.md)).
- **`pgdt` passes a cache's statistics as `held`** (`M130`), sized by
  `cache::claim` before its workers are carved (D85).

## Negative results

- **No `D<k>` entry.** The register is at its cap, and the shape and both
  rejected alternatives are on `ScanBudget`'s rustdoc.
- **No manual page, no figure, and no cache format change.**
