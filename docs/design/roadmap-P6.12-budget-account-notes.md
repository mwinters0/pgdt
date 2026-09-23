# P6.12 — A plan note's budget account: notes

The slice built by the spec's "Diagnostics: one sink"
([`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md); amended in
[`../status/history/2026-09-22.md`](../status/history/2026-09-22.md), "What a
provider's plan note says about its budget"). The checks are the unit tests
in `datafusion-pgdump/src/budget.rs` and `report.rs`,
`datafusion-pgdump/tests/scan_reports.rs`'s
`a_budget_quoting_note_carries_the_scan_s_account`, the accounts
`tests/settings.rs` now reads, and `datafusion-cli-pgdump/tests/cli.rs`'s
two planning tests. The user's text is
[`../manual/datafusion-cli-pgdump.md`](../manual/datafusion-cli-pgdump.md),
"What it says on stderr".

## What the wrap inherits

- **`ScanBudget` keeps an `AllowanceOrigin`**: `Limit` with the file,
  `HalfAvailable` or `NoneFound` from `discover_in`, and `Stated` from `new`.
  A draw given `pgdump.memory` records `Setting` for that draw only. So
  `Stated` (the embedder built the budget) and `Setting` (the session `SET`
  it) are two origins, because each names a different lever.
- **`Draw` keeps a `BudgetAccount`**, taken under the budget's lock when the
  scan draws: the allowance and its origin, the pool's `Finite` limit (`0`
  for none), the resident statistics, and what live scans had drawn, not
  counting this scan's own draw. It is the account of that one draw. A later
  `SET` or a dropped scan does not change a plan's notes.
- **`report_plan` wraps every note whose `budget_bytes` is set** in a
  `BudgetedPlanNote`, and a note with none (`StatisticsPruned`) still reaches
  the sink as the library's `PlanNote`. A sink that downcast to `PlanNote`
  now misses the budget-quoting notes and must also downcast to
  `BudgetedPlanNote`. The two test sinks that did this were changed.
  `datafusion-cli-pgdump`'s `StderrSink` prints `message()` and needed no
  change.
- **The clause names all four terms, zeros included**, and one lever key for
  each that applies. The rule is in `BudgetedPlanNote`'s rustdoc.

## Negative results

- **A floored draw's `drawn` is `0` under a stated allowance at the
  reserve.** The floor draws a zero budget, so a second plan made while the
  first is alive still reads `0`. `drawn` is checked against a seated draw in
  the unit test and in `tests/settings.rs`.
- **`pgdt` was not touched.** Its `plan_note_origin` stays its own clause,
  and the two layers' wordings differ by design (D64).
- **No `D<k>` entry.** The spec holds the decision, and the lever rule is in
  the rustdoc beside the clause. No figure was taken and no cache format
  moved.
