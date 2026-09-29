# P27.11 — The figures price the switch: notes

What the rest of 27.11 and the phase's wrap inherit. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md), "Slices",
item 11. The instrument landed at `d6b3660` and both figures were taken from
`aff3a0e`, a sitting of their own (`runs/measure-20260929T040546/`), a figure
being taken from a commit and never from a tree carrying its own apparatus
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

## What the pair read

**"D93"'s refusal of on by default stands, now read off the pair the spec
names**: the costing row's rows leg sits above the default's with the spreads
apart, and the unclustered row's below it, apart too
([`measurements.md`](measurements.md), "What DataFusion's dynamic filters buy
a query", which states both). "D93"'s **Rejected** and **Reopens** cite the
costing row alone, since the unclustered win is why the setting exists and
not a condition of its default.

**Neither win is the setting's**: the clustered join's and the TopK's rows
legs lie inside their on legs' spreads, group pruning doing both.

**The rows leg reproduces `5e02bf9`'s on leg**, taken when rows were evaluated
by default, so the account in that section, which argues from that sitting,
holds for the setting as shipped.

**The default's own Δ on the costing row resolves nothing**, its legs' spreads
overlapping; the unattributed +0.021 s the `da05a72` sweep read there is not
a finding to carry.
