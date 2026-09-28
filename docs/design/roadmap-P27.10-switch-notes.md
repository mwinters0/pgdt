# P27.10 — The switch for row evaluation: notes

What the phase's wrap inherits. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md), "Slices",
item 10. **Row evaluation is off by default, `pgdump.dynamic_filter_rows`
turns it on, and the result-equality harness runs both ways**
(`decisions.md`, "D93").

## What exists

- **The library states it per handle**:
  `TablePartitions::under(filter, RowEvaluation)`, `Off` being the enum's
  default. It is not a `QueryOptions` field, for the reason the filter is not
  one: `under` is the only place it is read. Under `Off`, a block's state is
  resolved without its row form (`DynamicBlock::read`), so `DynamicRead::rejects`
  returns before the instrument's row span and drops nothing. The cut at the
  first poll never builds the row form, whatever the setting.
- **The provider reads the setting when a scan is planned** (`PgDumpSettings`),
  as it reads the other `pgdump.*` settings, and carries it on the `Replay`.
  So each streaming exec built again over that replay, as filters arrive,
  keeps what was planned.
- **With rows not evaluated, the sorted stop is asked of every kept row in a
  group where it is armed.** Evaluating rows, a kept row cannot be past a
  bound the state requires, so the stop was only ever asked of a rejected
  row. Without evaluation nothing rejects a row, and the stop has to find the
  first row past the bound itself. `DynamicPruning::arm` arms it only in a
  group whose statistics say a row can reach the bound, so this costs one
  group's rows per stop, not the block's.

## Negative results

- **A static filter has no switch.** Its evaluation is what answers the
  filter `Exact` ("D88"). The switch covers only a filter whose producer
  re-checks its own rows.
- **No `pgdt` flag.** `pgdt query` holds no dynamic filter, so there is
  nothing for one to reach.

## Tests

- `pgdump_query/tests/dynamic_filter.rs`,
  `not_evaluating_rows_a_state_still_skips_groups_and_stops_blocks`: under
  `Off` the same groups are skipped as under `On`, no row is dropped, and a
  sorted block still stops at its first row past the bound, serial and split.
  Asking the stop only of rejected rows, as before, fails it.
- `tests/evaluation_instrument.rs` (`--features introspect`): under `Off` no
  part but `Chunk` is timed.
- `datafusion-pgdump/tests/dynamic_filters.rs`: `a_dynamic_filter_changes_no_answer`
  and the zeros test run flags-on under both settings, every answer equal to
  flags-off. Under either setting some query pruned and some stopped, and
  rows were dropped under `On` and never under `Off`.
  `setting_dynamic_filter_rows_binds_the_scans_planned_after_it` checks that
  a `SET` reaches the scans planned after it.
- `settings.rs`'s unit tests: the key, DataFusion's boolean spellings, the
  default, and the `SHOW ALL` line.

## The figures

**No figure is re-taken here.** The spec's row asks for none, and a figure
is taken from a commit (`measurements.md`, "A figure may be published outside
the sweep"). `dynamic-filter-join`'s and `dynamic-filter-topk`'s on legs time
the shipped default, so from this commit they time rows not evaluated, where
the readings at `5e02bf9` timed them evaluated. Whether the join figure
should gain a leg that states the setting is under STATUS's "Decisions worth
another look".
