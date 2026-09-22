# P6.11.1 — `SET pgdump.memory = 0` returns to the discovered allowance: notes

An earned follow-up to 6.11, built by the amendment to
[`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md), "Workers and memory"
([`../status/history/2026-09-22.md`](../status/history/2026-09-22.md), "A
session returns to the discovered allowance by `SET pgdump.memory = 0`").
`PgDumpSettings::set` maps a `0` allowance to `None`, and its rustdoc says why
`pgdt --memory` still refuses one. The checks are
`datafusion-pgdump/tests/settings.rs`'s
`a_zero_allowance_returns_to_the_discovered_one` and the unit test in
`datafusion-pgdump/src/settings.rs`. The user's text is
[`../manual/datafusion-cli-pgdump.md`](../manual/datafusion-cli-pgdump.md),
"Scan settings".

## What 6.12 and the wrap inherit

- **`PgDumpSettings::memory` is `None` both before any `SET` and after `SET
  pgdump.memory = 0`.** Nothing downstream can tell the two apart, so 6.12's
  account of where an allowance came from has two origins to name, stated or
  discovered, not three.
- **`SHOW ALL` lists an un-stated allowance as unset**, the same as one never
  set, because `entries` reports `None`.
- **`RESET pgdump.memory` is still refused.** `tests/settings.rs` asserts the
  refusal in the new test, next to the `0` it is the reason for, so the test
  fails at the DataFusion pin whose `RESET` reaches extensions. That pin is
  `roadmap.md`'s Future item.

## Negative results

- **The provider's tests do not pin `pgdt`'s refusal.** `datafusion-pgdump`
  cannot run `pgdt`, and `pgdt/tests/parallelism.rs` already asserts that
  `--memory 0` is refused with "no room to run in". The provider's test cites
  that file and does not repeat it.
- **No `D<k>` entry.** The spec holds the decision, and the rustdoc on
  `PgDumpSettings` carries the reason `pgdt` differs. No figure was taken
  and no cache format moved.
