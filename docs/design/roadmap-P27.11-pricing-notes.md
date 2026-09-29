# P27.11 — The figures price the switch: notes

What the rest of 27.11 and the phase's wrap inherit. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md), "Slices",
item 11. **The instrument has landed; its readings have not**: a figure is
taken from a commit, never from a tree carrying its own apparatus
([`measurements.md`](measurements.md), "A figure may be published outside the
sweep").

## What exists

- **Three legs a row**, `measure.DYNFILTER_LEGS`: `off`, `on` and `rows`. The
  rows leg is the on leg with `-c 'SET pgdump.dynamic_filter_rows = true'`
  ahead of the query's `-c`, in the same process, so it and the on leg differ
  by the setting alone, and the on leg and off by the producer's flag alone —
  `test_measure.py`'s `test_each_leg_of_a_row_differs_from_the_next_by_one_lever`.
  All three are held to one answer (`dynfilter_problems`).
- **The table** gains `Rows evaluated` and `Δ, rows against on` beside the
  existing `Δ, on against off`: the second Δ is the comparison "D93" reads.
- **`--profile-recipe`'s costing pair is `DFCLI_ACCOUNT_LEGS`, the on leg
  against the rows leg**, profiled and introspected; the off leg is no longer
  in the recipe. Under the default the introspection build times no span but
  `Chunk`, so that leg is the control that only rows are timed.

## Facts the readings rest on

- **A `SET` prints nothing under `--format csv`**, so the answer read back
  (`result_rows`, `result_first`, `result_digest`) is the query's alone:
  checked by hand in the figures' image against the tree's release build, where
  `-c 'SET …' -c 'SELECT 1' -c 'SHOW pgdump.dynamic_filter_rows'` printed the
  select's and the show's rows and nothing for the `SET`, the show reading
  `true`. Upstream, `PrintFormat::print_batches` prints nothing for a result
  with no rows in any format but `Table` (`datafusion-cli/src/print_format.rs`,
  55.1.0). A release that printed one would fail loudly rather than skew a
  reading: the rows leg's digest would differ from the on leg's and
  `dynfilter_problems` refuse the figure.
- **A mistyped key fails the run** (exit 1, "is not a pgdump setting"), so the
  rows leg cannot time the default under its name;
  `test_the_rows_leg_states_a_setting_the_provider_takes` holds the key
  against `settings.rs`.

## What remains of the slice

From a commit carrying this change:

```sh
cd scripts && uv run measure.py --figure dynamic-filter-join
cd scripts && uv run measure.py --figure dynamic-filter-topk
```

Then fold both tables into `measurements.md`, "What DataFusion's dynamic
filters buy a query", re-reading the prose after them, which argues from
`5e02bf9`'s rows-evaluated readings; and rewrite "D93"'s **Rejected** and
**Reopens** to read the costing row's and the unclustered row's `Δ, rows
against on` against the spreads of the on and rows legs, as the spec's item 11
states. Only then is the box ticked.
