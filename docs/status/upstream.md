# Upstream fixes we are waiting on

A defect or limit whose fix belongs to a dependency, what we do until the fix
ships, and what to change once it does. What an entry holds, when one is
added and when it is struck are
[`.claude/skills/upstream-issue/SKILL.md`](../../.claude/skills/upstream-issue/SKILL.md);
every dependency upgrade reads it
([`.claude/skills/upgrade-deps/SKILL.md`](../../.claude/skills/upgrade-deps/SKILL.md)).
A deficiency here may also be a `KD<k>` in [`deficiencies.md`](deficiencies.md),
which keeps the mechanism's detail; this file keeps the upstream side.

Each site to revisit carries an `upstream: UF<k>` comment, and
`cd scripts && uv run upstream.py` resolves every entry to its markers and
back. **`UF1`–`UF3` are allocated, and none is reused.**
<!-- upstream-watermark: UF3 -->

## UF1 — DataFusion's ungrouped-aggregate dynamic filter loses a column's bounds

- **The issue.** In DataFusion 55.1 an ungrouped `MIN`/`MAX` query's dynamic
  filter drops a column's `MIN` side once one partition's batch holds no value
  of it, and publishes other columns' bounds while one has none, so a scan
  pruning under it answers a `MIN` too high, or NULL for a column holding
  values. Default settings meet it. Ours: `KD56`.
- **Upstream.** Issue
  [apache/datafusion#25147](https://github.com/apache/datafusion/issues/25147),
  open. Fixed by
  [apache/datafusion#25579](https://github.com/apache/datafusion/pull/25579),
  merged to `main` as `0af68188` on 2026-09-29, which covers both cases;
  [#25149](https://github.com/apache/datafusion/pull/25149), open, covers the
  typed NULL alone and is not enough. Not on `branch-55` on 2026-09-30.
- **Fixed when.** The pinned `datafusion` release contains commit `0af68188`
  or a backport of it.
- **Workaround.** `datafusion-cli-pgdump` sets
  `datafusion.optimizer.enable_aggregate_dynamic_filter_pushdown` false unless
  the environment states it. The library cannot: a session is its caller's,
  and a pushed filter does not say which operator made it, so an aggregate's
  cannot be told from a TopK's and declined, so an embedder is told to turn
  it off in the crate's docs (`datafusion-pgdump/src/lib.rs`), and a shell
  user what it costs to turn on in
  [`../manual/datafusion-cli-pgdump.md`](../manual/datafusion-cli-pgdump.md),
  "Types". Rejected: patching DataFusion in this workspace, which fixes our
  builds and no consumer's.
- **Watch.** `datafusion-pgdump/tests/aggregate_bounds.rs` asserts the wrong
  answers with the filter on, and fails at the pin carrying the fix.
- **When it lands.** Drop the shell's default and its test; turn
  `aggregate_bounds.rs`'s filter-on cases into ones asserting the answer;
  strike `KD56`, index and marker; drop the crate docs' and the manual's
  paragraphs; remove the harness's `KD56` exclusion
  (`datafusion-pgdump/tests/unrepresentable.rs`, `Config::meets_kd56`); and
  turn the aggregate filter back on in
  `datafusion-pgdump/tests/statistics.rs`'s `sessions()`.

## UF2 — `arrow-cast` prints a `date` or timestamp only to `262142-12-31`

- **The issue.** `arrow-cast` formats `Date32` and `Timestamp` through
  `chrono::NaiveDate`, whose calendar ends at `262142-12-31`, so DataFusion
  cannot print, or cast to text, a valid value past it (`RT21` in
  [`../design/runtime-invariants.md`](../design/runtime-invariants.md)). Ours:
  the engine tier of the unrepresentable count.
- **Upstream.** No issue or discussion found for the display side (searched
  2026-09-30: apache/arrow-rs issues and PRs for "Date32 display", "262143",
  "extended year", "chrono out of range display"; chronotope/chrono for "year
  range", "MAX_YEAR"). `chrono`'s bound is its design, the year packed into
  `NaiveDate`'s high bits. The parse side was
  [apache/arrow-rs#9960](https://github.com/apache/arrow-rs/issues/9960),
  fixed by [#9961](https://github.com/apache/arrow-rs/pull/9961)
  (`2f923f72`), already in our `arrow-cast` 59.2.0; its `display.rs` still
  converts through `chrono`.
- **Fixed when.** `arrow-cast`'s display formats a `Date32` or `Timestamp`
  without `chrono`, or `chrono`'s `NaiveDate::MAX` moves.
- **Workaround.** The typed mode reads such a value as NULL and the text mode
  as its text ([`../design/decisions.md`](../design/decisions.md), "D98");
  `pgdt query` prints it with a renderer of its own (`decode.rs`,
  `render_date32_into`).
- **Watch.** `every_extreme_is_held_by_arrow_or_recorded`
  (`datafusion-pgdump/tests/unrepresentable.rs`), whose fixture rows sit
  either side of the bound; `RT21`'s re-verify commands.
- **When it lands.** `pgdump_query::calendar_end` reads `chrono`'s bound, so
  a display no longer through `chrono` needs it read from `arrow-cast` or the
  calendar tier retired (`decisions.md`, "D96" and "D98"); a moved bound
  refuses every cache counted under the old one, so the release note says to
  re-parse; rewrite `RT21`; the manual's "Types" names the date.

## UF3 — `RESET` cannot reach a provider's `pgdump.*` settings

- **The issue.** DataFusion 55's `ConfigOptions::reset` refuses a key outside
  `datafusion.`, and `ExtensionOptions` has no `reset`, so `RESET
  pgdump.memory` fails.
- **Upstream.** No issue, PR or discussion found (searched 2026-09-30:
  apache/datafusion issues and PRs for "RESET extension options",
  "ExtensionOptions reset", "ConfigOptions reset extension", "RESET custom
  config"). Unchanged on `main` at `0576a0b40`.
- **Fixed when.** `ExtensionOptions` gains a `reset` that
  `ConfigOptions::reset` routes an extension's key to.
- **Workaround.** `SET pgdump.memory = 0` returns to the budget's own
  allowance, as `target_partitions = 0` does to the machine's parallelism.
- **Watch.** `a_zero_allowance_returns_to_the_budgets_own`
  (`datafusion-pgdump/tests/settings.rs`) asserts `RESET` fails.
- **When it lands.** Implement `reset` on `PgDumpSettings` beside `0`, and
  strike [`../design/roadmap.md`](../design/roadmap.md)'s Future item
  "`RESET` for a provider's session settings, upstream"; the test above then
  asserts `RESET` returns the setting to unstated.
