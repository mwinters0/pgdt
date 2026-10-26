# P15.17.2 notes — a speculated chunk cheaper

A speculation's marked decode now costs well under two of the engine's
inflates of the same bytes on one thread, where it cost three. Nothing it
decides changed: every guess, decode and index is the one before. The row's
gate did not hold, and it moved to 15.17.4 by a split.

What changed, in `gz-seek`'s `native/` and `speculate/`:

- **The sinks write in place** into a buffer held at its length (`Held`), a
  chunk of values at a time, its slice bounds checked once a copy. No copy
  calls `memmove`, and no value is pushed.
- **A marked copy never looks at its values.** It moves the clean mark to its
  end where its source began before the mark, and a block's end finds the last
  marker among the last 32 KiB (`Sink::block_ended`). So a decode comes clean
  no sooner than its values do, and at most a block later.
- **A code's table entry carries its base and extra bits** (`Values`), so a
  length or distance costs no second lookup.
- **The symbol loop** compares once for the limit and coming clean, takes a
  second literal from the same bits, and reads a register refilled a word at
  a time (`Bits`). It is never inlined.
- **The BGZF header search** looks a chunk of bytes at a time.
- **`Input::push` reserves before it grows**, and a speculation's input starts
  with room for two chunks.

The evidence is the existing decode tests, which resolve every marker against
the engine's plaintext from every block boundary. Two tests are new.
`a_marked_sink_comes_clean_within_a_block_of_its_last_marker` replaces the
exact-mark test and holds the mark to the last marker at each block's end.
`the_first_byte_is_found_where_a_byte_by_byte_search_finds_it` covers the
chunked search.

## The account

All of it outside the apparatus; `runs/p15.17.2/README.txt` maps every
label and file.

- **At four jobs, from 15.17.1's profile** (`runs/p15.17.1/profile/`), about
  three fifths of the CPU was the speculation. Within it, its copies' calls to
  `memmove` were about a fifth, and `Input::push`'s growth a few percent. The
  engine's inflate was about a quarter, the frontier's a little over half of
  that. A chunk decode zeroing its buffer was about a percent (15.17.3).
- **On one thread** a speculation hand probe, `spec_cost`, times every
  chunk's speculation against the serial pass (`runs/p15.17.2/compare.txt`).
  HEAD's cost about three serial passes and this tree's under two. Each term
  above was cut one change at a time, each timed pinned against the last.
  Writing in place and the clean mark by source region bought the most.
- **What a speculation now spends** (`prof-slice1/`, `prof-region1/`): a fifth
  on its copies, their loads from a 16-bit window twice the engine's; a fifth
  on table lookups; the block search and the header search a few percent;
  the rest the symbol loop. Instructions bind, not the memory: its IPC is
  above three.

## What the probe found

`runs/p15.17.2/table.txt`: 15.17.1's probe over `control_gz`, seven reps
alternating this tree's build and HEAD's.

- **Four jobs is level with serial, not faster beyond the spread.** Its median
  is below serial's and its reps lie at the low edge of serial's spread, which
  is wide. The pass spends noticeably less CPU than HEAD's.
- **Twenty-four jobs is faster than HEAD's**, at about two thirds of its CPU.
  Twelve is faster than twenty-four, by about the spread.
- **What holds four jobs** (`runs/p15.17.2/profile/profile-j4.txt`): the
  workers decode about half the chunks. The frontier's engine is about half
  the serial pass's, yet its wall time is the same, so the frontier is idle
  for much of it. The `par4` emulation (`bench.txt`) shows why. A chunk's
  decode is queued when its window resolves, but it starts only when a worker
  ends a speculation, and the frontier waits for it. The workers are also
  idle about a fifth of the time, while guesses fill the room `ahead.rs`'s
  `top_up` holds the pass to, twice the tasks. In the emulation, doubling
  that room cuts four jobs well below serial.

## What 15.17.4 inherits

- The gate: four jobs faster than serial beyond the spread, and the default
  worker count settled again. Twelve now beats twenty-four, which D2's
  rejection of a cap at the physical cores says it does not.
- The two terms above, both in scheduling rather than the speculation: a
  decode waiting for a worker, and the room. The room counts a held guess as
  a decode is counted, and `Chunking::task_bytes` charges two guesses a task
  (`KD141`, 15.18's). Widening it changes that charge.
- `spec_cost`'s `par` mode is the cheap instrument for both. It rebuilds in
  seconds where `pgdt` takes a quarter of an hour, but it has no scan thread.

## Negative results

- **An exact clean mark a copy** costs a pass over every copy's values.
  Folding them after the copy, scanning for the last marker, and ORing lanes
  as they are copied were each slower than the source-region mark.
- **Keeping a worker for decodes** (`par4` with `RESERVE`) is slower than
  serial. Two speculating workers do not land guesses close enough to chain.
- **On `control_gz` the marked decode almost never comes clean.** Nearly
  every copy reads a marker (`bench.txt`, the `diag` line), so the plain path
  is not what a speculation spends.
