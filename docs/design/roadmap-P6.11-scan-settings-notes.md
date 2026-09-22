# P6.11 — The `pgdump.*` scan settings: notes

The slice built by the amendment to
[`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md), "Workers and memory"
([`../status/history/2026-09-22.md`](../status/history/2026-09-22.md), "What a
provider's plan note says about its budget"). The checks are
`datafusion-pgdump/tests/settings.rs`, the unit tests in
`datafusion-pgdump/src/settings.rs` and `budget.rs`, and
`datafusion-cli-pgdump/tests/cli.rs`'s `a_scan_setting_is_set_by_sql`. The
user's text is [`../manual/datafusion-cli-pgdump.md`](../manual/datafusion-cli-pgdump.md),
"Scan settings".

## What 6.12 and the wrap inherit

- **`PgDumpSettings` is a `ConfigExtension` under `pgdump`**, in the
  session's `ConfigOptions`: `memory` (`Option<u64>`), `chunk_size` and
  `max_line_bytes`. It is the same prefix as `PgDumpTableOptions`, which is a
  `TableOptions` extension. The two registries are separate, so `SET
  pgdump.table` and `OPTIONS ('pgdump.memory' …)` are both refused as
  unknown keys.
- **`register_dump` and `register_table_factory` install it** beside the
  budget (`session_budget`), and they leave settings a session already
  carries alone. A provider registered by hand in a session that has none
  plans under the defaults. `SET pgdump.…` then fails with DataFusion's "could
  not find config namespace", and an embedder fixes that with
  `SessionConfig::with_option_extension(PgDumpSettings::default())`.
- **`scan()` reads the settings once, before it draws.** `ScanBudget::draw`
  takes the stated allowance as a fourth argument and carves it in place of
  its own, so live draws still come off it. `ScanBudget::allowance()` still
  answers the budget's own, not the one in force. 6.12's account has to take
  the stated one from the draw, not from the budget.
- **`ScanOptions` comes from `PgDumpSettings::scan_options`**, which leaves
  every other field at its default. The provider sets no statistics
  allowance, because it never gathers.
- **The zero refusals copy `pgdt`'s parsers** (`parse_chunk_size` and
  `parse_max_line_bytes` in `pgdt/src/main.rs`; `pgdump.memory`'s `0`
  un-states instead, [6.11.1](roadmap-P6.11.1-memory-zero-notes.md)). The
  library does not validate `ScanOptions`, so each caller carries the rule.
  Values are plain byte counts, as `pgdt`'s flags take them, not
  `datafusion.runtime.memory_limit`'s `4G`.

## Negative results

- **`RESET pgdump.memory` is refused by DataFusion 55.** `ConfigOptions`
  resets only its own `datafusion.` namespace, and `ExtensionOptions` has no
  reset, so `SET` alone cannot un-state an allowance; `SET pgdump.memory = 0`
  is the way back ([6.11.1](roadmap-P6.11.1-memory-zero-notes.md)), and
  `tests/settings.rs` fails when `RESET` starts to reach it.
- **A line limit binds only a line that crosses a read.** Over the fixture's
  1 MiB default chunk, `max_line_bytes = 8` answers every row. It refuses
  only once `chunk_size` is shorter than a row, as `pgdt`'s manual says of
  `--max-line-bytes`.
- **A plan's unit lags the stated chunk by one read** (`KD41`, found here).
  `plan_partitions` announces the budget and not the chunk, so a plain
  source's unit follows the last read announced on the dump's shared source.
  A `SET pgdump.chunk_size` narrows the next plan's batch span at once, but
  its floor and its count only from the scan after. `pgdt query --chunk-size`
  over a complete cache never moves it. The fix is in the library, so it is
  filed rather than taken.
- **No `D<k>` entry.** The spec holds the decision, and each shape is in the
  rustdoc beside it. No figure was taken and no cache format moved.
