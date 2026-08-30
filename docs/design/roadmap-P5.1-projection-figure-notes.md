# P5.1 — Register the figure: notes

What the next slices inherit from registering `projection-widths` before the
feature it measures exists. The spec is
[`roadmap-P5-pushdown.md`](roadmap-P5-pushdown.md); what has landed is
[`../status/STATUS.md`](../status/STATUS.md).

## What is there now

`scripts/measure.py` carries a second entry under `measure.UNTAKEN`:

- **`PROJECTION_WIDTHS`** — the five widths, `{0, 1, 16, 17, 19}`, each mapped
  to the column names that width asks for.
- **`projection_flags(width)`** — those names as `pgdq query` flags:
  `--no-columns` at zero, `--column <name>` repeated otherwise.
- **the `query-project-<width>` command shapes** in `_script`, parsed rather
  than matched, so an unregistered width raises `ValueError` exactly as an
  unknown command shape does.
- **`run_projection_widths`** and its `_PROJECTION_ROWS` — the table, one row
  per width, each carrying the median, the per-row cost, and the per-row
  difference against the row above.
- **`Figure(id="projection-widths", …)`** in `UNTAKEN`.

Nine tests in `test_measure.py`'s new `ProjectionWidths` class, plus one in
`Scripts`, hold the instrument's preconditions; the rest of the slice is the
harness reading the widths back.

## What the next slices must not break

**The CLI must accept `--column <name>` repeated and `--no-columns`, with
`--schema-mode typed`, on `pgdq query`.** `P5.4` is what makes the registered
command shapes executable; until then `--figure projection-widths` runs a
binary that rejects the flags. That is why the entry sits in `UNTAKEN`: a
sweep never reaches it, and `--figure` is the only way to invoke it.

**Every width's column list must stay a superset of the width above it.** The
table's whole claim is that the difference between two adjacent rows is the
cost of the columns they differ by; a width that dropped a column the row above
projected would silently make that difference two changes rather than one.
`test_each_width_contains_the_one_below_it` holds it.

**A width's key is its length.** The keys are the row labels the published
table carries, so `test_each_width_names_that_many_columns` is what stops a
column being added to a list while the row keeps advertising the old count.
That is why the keys are literals rather than `len(...)` expressions, which
would have kept the mapping self-consistent while renaming the rows.

## Calls made here, and why

**The column names are imported from `generate_perf_data.py`, not
transcribed.** `measure.py` now does `import generate_perf_data as perf` and
derives `_PERF_SCALARS` / `_PERF_ARRAYS` / `_PERF_COMPOSITE` from that module's
own lists. A projection names columns, and a name spelled independently here
would survive a rename in the generator right up to the point where a sweep
runs a query that errors — minutes in, with the figure lost. The module is
stdlib-only with everything executable behind its `__main__` guard, so the
import is free.

**The 17-column row is the scalars plus `v_comp`, skipping the arrays.** That
is not the file's column order taken as a prefix — the file writes the two
arrays before the composite. Every width is instead a *subsequence* of the
file's order, so no row introduces a reordering as a second variable, and the
19-column row is the file's full order.

**`depends` carries the scan path, which no other query figure does.**
`(*SCAN, *NESTED, *QUERY_CLI, *GEN_PERF)`. `nested-end-to-end` and the two
cross-file figures publish *differences*, out of which the scan cancels; this
figure's first row is an absolute reading of replay with nothing decoded, so a
change in what replay costs moves it directly. `--stale` iterates
`ALL_BY_ID` — `FIGURES` plus `DERIVED` — so this wider declaration prints no
staleness noise while the figure is untaken.

**Six reps**, matching `cross-file-floor` and `composite-isolated` rather than
`nested-end-to-end`'s five: the reading that matters is a paired per-rep
difference between adjacent rows.

**The figure is typed on every row.** `strings` builds every column the same
cheap way, so a width sweep taken that way would measure the walk and nothing
this figure is about. `test_every_run_is_typed` holds it.

## Two things a reader will otherwise re-derive

**`measurements.md` is deliberately untouched.** An untaken instrument has no
table and no marker — `--check` expects none, and would report a marker naming
a figure with no numbers under it as an error. The doc's one paragraph about
untaken work (the composite bound, under "Built, not taken: the instrument that
would resolve it") is `P5.7`'s to rewrite, when this figure is taken and the
cross-file apparatus it supersedes is re-scoped. Writing the supersession into
a live measurements doc now would state as fact a claim no reading yet
supports.

**The `time ` guard in `test_measure.py` is now a regex.**
`test_every_command_times_exactly_one_thing` counted the substring `"time "`,
and the perf table has a column named `v_time` — so `--column v_time` in a
projection's flags read as a second timed command. The guard now matches the
timer where a timed command can start (the head of the script, or after `; `),
which is what it was always checking; `parse-cache-out`'s `rm -f …; time …` is
the only shape with a second position and it still counts one.

## Staleness

This slice edits `scripts/measure.py`, which `session-drift` declares, so
`--stale` reads that figure stale until the commit carrying this slice is
acknowledged. It is acknowledgeable: no line any taken figure executes
differs — the additions are a new `UNTAKEN` entry, its command shapes and its
run function, none of which any published figure reaches. The entry cannot be
written here, because an acknowledgement names a commit sha and this slice is
handed over uncommitted; see
[`../status/history/2026-08-30.md`](../status/history/2026-08-30.md).
