# P10.10 — The statistics figures: notes

What the rest of this slice, and the slices after, inherit. The spec is
[`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md),
"Measurements". **The instrument landed and the readings did not**: a figure
published outside a sweep names the commit it was taken at, and a sitting run
from a tree carrying its own uncommitted apparatus has none
([`measurements.md`](measurements.md), "A figure may be published outside the
sweep"). No figure was taken.

## What exists

- **Both figures wait in `measure.UNTAKEN`**, each standing in no sharing edge,
  so `--figure` takes either from the commit that lands them and the table
  enters `measurements.md` under its own sitting marker.
- **`statistics-gathering`** times `parse-statistics-{none,all}-rss` over the
  three scan-throughput inputs, warm, with a `dd` floor per input and resident
  from the wrapper, in `measure.STATISTICS_MEMORY` rather than the register's
  512 MB. Its `none` leg is its own shape, not a borrow of `peak-rss`'s
  `parse-rss`: the container differs, and a borrow would confine it to a sweep.
- **`statistics-pruning`** times
  `query-pruning-{range,dictionary,unnarrowed}-{none,all}` over `pruning`, a
  new input from `scripts/generate_pruning_bench.py`: the control's rows, byte
  for byte, with `v_category text` appended. Each run builds its cache in the
  container with an untimed gathering `parse`, so the two legs of a row differ
  by `--statistics` alone.
- **The third filter is `v_smallint=0`** (`measure.PRUNING_UNNARROWED`), the
  one its statistics cannot narrow: a `smallint` drawn uniformly per row
  keeps bounds spanning its range in every group, holding the midrange `0`, and
  more distinct values than a dictionary keeps, so every group's bounds are
  read and none is skipped. Over the 3.00 GiB input at seed 42, recomputed from
  the generator's draws without writing the file: 13 rows match, and the
  smallest of its 3,072 groups holds 244 rows, none bounded away from `0`. A
  `smallint` rather than a wider column so the filter still returns rows.
- **Both state `measure.GATHER_STATISTICS`** — `--statistics all` and the
  group size, mirrored from `DEFAULT_STATISTICS_GROUP_SIZE` and tested against
  it — where they gather; `statistics_flag_problems` admits that request inside
  the two families and nowhere else.
- **The query's own notes are read into `reported`** (`parse_query_notes`):
  skipped groups and bytes, a stop's unread bytes, and rows returned, `no rows
  found` read as zero. `pruning_problems` refuses a sitting whose unpruned leg
  skipped anything, whose pruned leg skipped no group, or whose two legs
  returned different row counts — except the third filter's pruned leg,
  refused instead where it printed no pruning note (so consulted no
  statistics), skipped a group, or stopped a read.

## Tests

- `scripts/test_measure.py`'s `StatisticsFigures`: the exemption admits the
  gathering request and nothing else, the legs differ by one flag, the builder
  is untimed, the regexes read the CLI's and library's wording, each
  refusal fires, and the third filter is a midrange equality on the
  generator's `smallint`, stated identically in the Rust test below.
- `scripts/test_generate_pruning_bench.py`: every row is the control's with the
  label appended, and the labels run and cycle.
- `pgdump_query-cli/tests/perf_generator_fidelity.rs`'s
  `the_pruning_generator_writes_the_statistics_its_figure_prices`, at the
  figure's 1 MiB group size over 24 MiB: `id` ascending with bounds in every
  group, `v_category` a dictionary in every group and no bounds, `v_smallint`
  bounds in every group and a dictionary in none, and `v_smallint=0` ruling
  out no group and stopping no read. A `COLLATE "C"` on `v_category` failed it.
- A `--dry-run` and a 0.05 GiB one-rep sitting of both rendered; the second is
  what found the zero-row wording.

## For the rest of this slice

- **Take the readings from the commit**: `cd scripts && uv run measure.py
  --figure statistics-gathering --figure statistics-pruning`. It generates the
  3.00 GiB `pruning` input on its first run, and each pruning rep runs a
  gathering `parse` before its timer. Then move both into `measure.FIGURES`,
  paste the two sections under their markers, and re-read the consumers the
  run names.
- **The pruned legs read close to the bash timer's resolution** by a probe's
  account, not a figure's; the spread column is what says whether six reps
  resolve them.

## Negative results

- **`v_bool` cannot be the dictionary leg**: every group holds both values, so
  a dictionary proves nothing absent. A label drawn per row fails the same way,
  hence the runs.
- **A `--dqcache none` query prunes nothing**, having no statistics; the pruning
  legs need a cache, which is why they build one.
- **Text under no stated collation gets a dictionary and no bounds**, so the
  equality leg is the dictionary's alone; an integer label would be pruned by
  its bounds as well.
- **A group of at most a dictionary's cap in distinct values keeps a
  dictionary**, and one lacking `0` skips the group, so the third filter skips
  nothing only where every group holds more values than that — which a group of
  at most 64 rows cannot. The 3.00 GiB input is a whole number
  of MiB, so its last group is full; a sitting at a size that is not can end on
  a short group, which `pruning_problems` refuses. The fidelity test is taken
  at the figure's group size for the same reason: a 64 KiB group holds too few
  rows.
- **A text column was not the third filter**: with no bounds and, past the
  cap, no dictionary, its filter consults only NULL counts, pricing less of the
  consulting a bounded column's filter pays.
