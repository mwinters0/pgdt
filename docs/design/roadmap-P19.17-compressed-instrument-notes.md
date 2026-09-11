# P19.17 — the compressed account's instrument

The spec gave the plain path an account before its default and never wrote the
compressed half; this slice registers the instrument that fills it. The spec's
paragraph is
[`roadmap-P19-efficient-defaults.md`](roadmap-P19-efficient-defaults.md), "The
compressed path is accounted for before its default is chosen", and the figure
it grows is `measure.py`'s `reserve` entry, still in `measure.UNTAKEN`.

**No library code changes, and no reading is taken here.** `19.18` is the
sitting; this is what it runs, reviewable cold against
`scripts/test_measure.py`.

## What the figure is now

Four families under one `reserve`, and the two new axes answer different
questions — which is the whole reason both exist. Under discovery
`budget = jobs × per_worker` exactly, so a fit over flagless legs alone cannot
say whether a byte of resident is owed to a reader or to a budget byte; the
stated legs pin the count and vary the budget, and are what make the pair
decomposable.

| Family | Legs | What it answers |
|---|---|---|
| stated budget (unchanged) | 2 sources × 2 arena settings × 4 budgets, `--jobs 24` in `4g` | a budget byte's cost, the count held still |
| **flagless** | 2 block sizes × 4 container limits, **no flags** | what the shipped default resolves and then holds |
| **mechanism** | jemalloc, mimalloc, `MALLOC_ARENA_MAX=2` — one block size, one limit | what each one moves |
| **path step** | one byte either side of `reader_bytes`, same limit | the block path against the streaming fallback |
| serial baseline (unchanged) | `control` at `--jobs 1` | `peak-rss`'s own `control` row |

Thirty legs at three reps. The renderer emits the flagless table, the fitted
pair per block size, the mechanism table, the step, the stated table and the
baseline paragraph, in that order.

## The four calls that were not obvious

**A flagless leg's container limit is on the `RunSpec`, not in the command
shape.** Everything else this harness varies is an argv fact and rides in the
shape, where `_script` parses it and refuses a value the figure does not carry.
A container limit is a `nerdctl run` argument, so `RunSpec` gained an optional
`memory` and `RunSpec.key` carries it **only when it is set** — which keeps a
past sitting's `raw.json` renderable, every other spec keying exactly as before,
and stops two flagless legs differing in nothing else from collapsing into one
another's reps. `time_run` reads the spec's limit ahead of the figure's.

**The reader count is read back off the run, never computed.** Under discovery
the count is `ParallelArgs::resolve`'s answer to the allocation, so the harness
could only have it by reimplementing the rule under test — the second authority
`QUERY_SUBSTREAM_CAP` refuses by name. `parse_resolution` takes `jobs=` and
`memory_bytes=` off the library's own `scan started` line, for **every** shape
rather than for the flagless family alone: a stated leg whose resolved pair is
not the pair it stated is exactly the apparatus failure the worker-count
reconciliation cannot see. A test holds `stream.rs`'s event to carrying those two
fields in that order, since that line is the one input here that is not a
constant.

**The flagless family is exempt from the worker-count rule and declares itself
at both ends.** `worker_count_problems` skips it by a declared prefix
(`_NO_FLAGS`), and `flagless_flag_problems` is the other side: it fails a shape
in that family that states `--jobs` or `--parallel-memory` after all. So the
exemption is from *stating a count*, never a licence to pin one quietly — which
matters because such a shape would still pass the count check, being exempt, and
would publish a stated arrangement under a heading that says discovered. This is
**not** the figure the spec refused under "The default is pinned by a test, not
by a figure": what was refused there was publishing a *throughput* table off
unpinned shapes, the throughput of whatever the default lands on being already
measured at a stated count by `parallel-scan-throughput`.

**`reader_bytes` is mirrored into the harness, hand-checked once.** The path
step's two budgets are `2 × unit + chunk + decoder` and that minus one, and the
flagless table needs the same number to say which legs took the block path at
all — `parse` emits no decline note, that being a `PlanNote` on a query's
`TableStream`. At 24 MiB blocks it is **60,852,000 B = 58.03 MiB**, which is the
number `19.14` shipped and `19.12` measured, and a test asserts the literal. The
chunk term is a second name for `CHUNK_DEFAULT`'s value, deliberately: one is
the chunk-size figure's reference row and the other is what a reader holds a
buffer of, and a test holds them equal so that the day either moves, the figure
that reads it is not quietly wrong.

## What `19.18` inherits

- **The limits are `512m`, `1g`, `1536m`, `2g`**, which is `19.15`'s axis minus
  its two ends: 256 MiB never reaches the block path and 8 GiB is the unlimited
  arm, neither of which is a point on a line through the reader count. At
  24 MiB blocks they resolve 3, 11, 19 and 24 readers; at 128 MiB blocks the
  first declines the block path and the rest resolve 1, 2, 4 and 6.
- **A flagless leg may be OOM-killed, and that is a reading rather than an
  apparatus failure.** The rule aims resident at the limit by construction, so
  every flagless leg sits near its own ceiling — `19.15`'s worst rep left three
  megabytes of 512 — and the 128 MiB legs sit closest, their per-reader charge
  being four times the other leg's. The sitting fails on the rep that crosses,
  which is minutes in rather than an hour; what moves then is the limit set, and
  the kill itself is the finding the constant is being chosen against.
- **The mechanism legs are at `512m` and 24 MiB blocks**, which is the thin
  point: the smallest allocation reaching the block path, where the fixed term is
  the largest share of resident and a leg that moves it is visible at all.
  `19.15` read `MALLOC_ARENA_MAX=2` as worth 61.6 MiB of 474 there — a sixth —
  so the standing arena account is already known not to explain the term, and
  what the allocator legs are for is the hypothesis beside it:
  `BlockCache::slot`'s evicted-but-viewed block, which no pool bounds.
  Fragmentation moves under jemalloc and a retained view does not.
- **The fit refuses a single point.** `_least_squares` raises unless two reader
  counts differ, because an intercept asserted off one arrangement is exactly
  what `19.15` found in `19.12`'s extrapolation. Where a block size has only one
  block-decoding leg the table says "no fit" and names what declined.
- **The sitting stays diagnostic** — `--alone`, NOT PUBLISHABLE, as `19.6` and
  `19.12` were. `MEMORY_RESERVE` lives in `io.rs`, which this figure declares, so
  a table published before `19.13` ships the new constant goes stale the moment
  it does.
- **No figure moved.** The slice touches `scripts/measure.py` and
  `scripts/test_measure.py` only; `session-drift` is the one figure declaring that
  path and has been red on it since the harness took the derived direction of the
  borrow graph.

## The edges, and the one that is a non-edge

The spec row asks for the `shares` edges against `peak-rss` and
`rss-attribution` to be re-declared. What the leg set supports is **one edge and
one stated non-edge**, and the declaration beside the figure now says so:

- onto **`peak-rss`**: the serial baseline is that figure's `control` row spec
  for spec, so the two must share a reading rather than take one each;
- onto **`rss-attribution`**: **none**. Every leg of that figure runs over
  `blocks500`/`blocks4000`, the two block-count shapes whose axis it is, where
  every leg here runs over `control`, `control_xz` or `control_xz128` — so no two
  of their runs are the same run, at any block size, and an edge would be a
  borrow of a reading that does not exist. What the three figures share is a
  *sitting*: all three read resident, which is the collapse `M74` is for.

A test computes both intersections over the spec keys rather than asserting them
in prose, so the day a leg is added that does coincide, the claim fails instead
of ageing. Whether the row wanted a genuine third edge — which would mean this
figure taking a leg over the block-count inputs, and measuring something else —
is filed under `STATUS.md`'s "Decisions worth another look".

Neither edge is *declared as a `Shared`* here, for the reason it never was: an
edge declared from `UNTAKEN` entangles `peak-rss`, which the doc carries from a
standalone `41c96bb` sitting, and fails `--check` on that sitting's marker with
no sweep yet to cure it. `19.11` declares it, and owes the `Session.borrow`
change that lets an **RSS** reading cross a share at all.
