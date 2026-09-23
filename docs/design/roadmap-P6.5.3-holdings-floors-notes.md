# P6.5.3 — The holdings at the floors: notes

A slice earned after 6.5.2 of
[`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md), from the review of its
call ([`../status/history/2026-09-22.md`](../status/history/2026-09-22.md),
"The holdings at the floors"). `Parallelism::within_shared` now takes off the
budget whatever of `held` the margin's ceiling cannot absorb beside the budget
its resolved count spends, for every source; the cap is untouched and nothing
held changes nothing. The reasoning and the rejected plain-source exception are
on its rustdoc. The checks are `a_holding_comes_off_the_ceiling_and_never_the_cap`
(`pgdump_query/src/io.rs`), `holdings_lower_the_count_before_the_budget`
(`datafusion-pgdump/src/budget.rs`), and
`the_resident_statistics_are_billed_and_lower_a_budget_the_margin_cannot_hold`
(`datafusion-pgdump/tests/provider.rs`), which reads a plain fixture's rows
under the lowered budget.

## What later slices inherit

- **Budget plus holdings stays under the ceiling wherever the budget can
  give the room.** Where the budget already passes the ceiling with nothing
  held — one reader at `at(1)` on a narrow allowance — the whole holding comes
  off it and the margin is still passed by that excess, as `pgdt`'s one-reader
  floor is today.
- **A plain scan's draw now moves with the holdings**, down to zero; the
  integration test sees it, where 6.5.2's could not. A budget that small still
  reads a plain fixture; what a plain budget buys is `KD25`'s and `KD32`'s.
- **A compressed scan at one reader loses block decode** once the holdings
  push its budget below a block (D4); nothing here measures that and no `.xz`
  fixture exercises it through the provider.
- **`pgdt` has the same rule** (`M130`): a cache's statistics are its
  `held`, through `within_shared`, D85's order changing with it.

## Negative results

- **No `D<k>` entry.** The register is at its cap, and the rule is on
  `within_shared`'s rustdoc; the spec's "Workers and memory" already stated it.
- **No manual page, no figure, and no cache format change.**
