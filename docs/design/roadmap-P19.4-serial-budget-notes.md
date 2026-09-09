# P19.4 — `Serial` carries an optional budget

`Parallelism::Serial` is now `Serial { memory_bytes: Option<u64> }`, so
`Parallelism::workers(1, 400 << 20)` answers a serial state holding 400 MiB
instead of one holding nothing. `KD16` is struck: index line, the paragraph in
[`architecture.md`](architecture.md)'s "Execution model and API surface", and
the code marker on `ParallelArgs::resolve`, in this change.

The mechanism and what it refused are filed beside it in `architecture.md`;
what follows is what the next slice inherits.

## The collapse is on the count, and only the count

`workers()`'s `_` arm builds `Serial { memory_bytes: Some(memory_bytes) }`. So
the value keeps the property the collapse existed for — "is this parallel" is
`matches!(self, Serial { .. })`, never a comparison against one — and loses the
side effect nobody chose, which was that the *other* number went down with it.
`jobs()` is still 1 through either variant and `memory_bytes()` reads the field
through both, which is why nothing downstream branches on the shape: every pool,
`worker_count`, `plan_partitions` and `compressed_block_path_declined` were
already asking through those two methods.

Two consequences worth knowing before touching this again:

- **`worker_count(Serial { memory_bytes: Some(_) }, n)` is 1**, because `jobs()`
  caps it before the budget divides anything. A serial caller with a large
  budget gets one worker holding a lot, which is the intent.
- **`plan_partitions` raises no `parallelism_budget_limited` note on the serial
  path**, since `workers == requested == 1`. A stated budget that cannot afford
  a second sub-stream is not a shortfall when only one was asked for.

## `None` now means exactly one thing, and the CLI is what protects it

`memory_bytes() == None` is `Parallelism::default()` and nothing else. That
matters because the status line renders it: `io::memory_budget_display` prints
`67108864 (default)` for `None` and a bare quantity otherwise, and `(default)`
is a claim about what the *caller* asked for.

`ParallelArgs::resolve` therefore answers `Parallelism::default()` when neither
flag was given, rather than passing its own `DEFAULT_MEMORY_BUDGET` fallback
down and letting the line claim 64 MiB was requested. The CLI's fallback and the
library's constant being the same number is what makes that distinction
invisible unless it is preserved deliberately.

**A pre-existing asymmetry survives and is not this slice's to close**: at
`--jobs 4` with no `--parallel-memory`, `resolve` still fills in 64 MiB and the
line prints it bare, because `Workers` has no shape for an unstated budget. It
read the same way before this change. Giving `Workers::memory_bytes` an `Option`
too would close it and is a change to the type's own decision, not a fix to make
in passing.

## The tests, and which one is load-bearing

- `io.rs`: `one_worker_and_none_are_both_the_serial_state` now pins that the
  budget rides through the collapse, and
  `a_serial_budget_sizes_the_source_it_is_announced_to` drives
  `hint_parallelism` on a real `LocalFileSource` — a value that carries a budget
  nothing is sized from would pass the first and fail the second.
- `pgdump_query-cli/tests/parallelism.rs`:
  `a_stated_budget_decides_the_read_path_at_the_default_job_count` is the one
  that matters. It runs the binary with `--parallel-memory` and no `--jobs`, and
  asserts both directions — the block path declined below the fixture's block
  unit, and taken back above it — because a budget carried correctly in the
  value and dropped somewhere on the way to `ScanOptions`/`QueryOptions` leaves
  every unit test in the tree passing.
- `status_output.rs`: `(default)` and its absence are both pinned now, on the
  same command shape apart from the flag.

## What `19.11`'s sweep inherits

`measurements.md` carries **two** published tables whose one-job row was taken
under the behaviour this slice removed, not the one the spec names:

- `parallel-scan-throughput` — the spec's "The figure moves" paragraph. Its
  prose says the one-job row "runs at the library's 64 MiB default" *because*
  `--jobs 1` states no budget; the harness states `--parallel-memory` on every
  row, so at the next sitting that row runs at 1.00 GiB like the rest.
- `parallel-peak-rss` — **not named in the spec, and the movement is larger
  here.** Its prose makes the same claim, and its 128 MiB-block leg's one-job row
  reads through the streaming fallback *only* because the stated 1.00 GiB did not
  reach the source. At the next sitting that row block-decodes, so the leg's
  whole "and every row above it block-decodes" contrast disappears.

Both are already red in `--stale` for `pgdump_query/src/io.rs` and were before
this change, so this slice adds no new red and owes no acknowledgement. Neither
prose block is corrected here: it lives in `scripts/measure.py` and is folded in
by re-rendering a sitting, and re-rendering the existing one would attach a
sentence about the current code to numbers the previous code produced. The
closing sweep replaces both tables and both paragraphs at once.
