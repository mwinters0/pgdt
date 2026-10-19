# P36 notes — the embedder API

What the phase leaves the next one, and what it tried and refused. The spec is
[`roadmap-P36-embedder-api.md`](roadmap-P36-embedder-api.md); the surface's
rule is [`decisions.md`](decisions.md), "D106", and a refusal's wording
"D107"; each crate's part is [`concepts.md`](concepts.md). The per-slice notes
went at the wrap, and git holds them at `53937fe5`. Each slice's decision
reached the register in the slice that made it — D11, D26, D40, D48, D53,
D54, D56, D77, D83's `Rejected`, D90 and D107 — or sits beside its mechanism
in code. `KD29`, `KD92`, `KD115`–`KD119` and `KD121` closed inside the
phase, `KD120` and `KD123` by out-of-band items it raised (`M224`, `M227`); no `KD<k>` was
owned by P36, and no out-of-band row names it.

## What the next phase inherits

### The surface an embedder starts from

- **`crates/pgdt-query/tests/embedder.rs` is the reference walk**, held to
  the embedders' rule by `scripts/layering.py` (`EMBEDDER_TEST`): open and
  map, load and judge a cache, list tables and Arrow types, stream four
  ways, summarise — naming the seven libraries' API and no `pgdt_fixtures`.
  Every journey passed against the surface as it stood, with no library
  change.
- **A `Dump` is complete by construction** (`Dump::opened`, `Dump::mapped`),
  and a `Table` owns what it plans over, outliving the `Dump`. The cold path
  is `Table::settle`, the warm path's plan with a mapping pass in front.
  `pgdt info` stays off the `Dump`: it reports an incomplete cache and a
  cache with no source, which a `Dump` holds neither of (D61, D67).
- **An allowance below the carve's reserve gathers no statistics and plans
  serially, and says so** — `statistics_declined` on the block,
  `AllocationBelowFloor` and `ParallelismBudgetLimited` among the plan's
  notes. The reserve is `pgdt_io::internal`'s (D11), so a binding surfaces
  those signals rather than checking a floor.
- **The arrangement keeps one divergence by a request field.** Under half of
  `MemAvailable`, `pgdt` caps its pools there with no reserve or margin and
  the provider carves it as a stated allowance
  (`ArrangementRequest::carve_estimate`); merging them would have changed
  behaviour, which the phase did not do.
- **Nothing in the workspace holds a `ValueKey`.** The gatherer, the summary
  and `partition_orders` hold `internal`'s `OrderKey` keyed by the kind they
  chose a stored set by (D79); `ValueKey` is the embedder's, and
  `compare_value_keys` is the adapter's answer across orders in DataFusion's
  semantics (D56, D40). `CompareKind` still reaches an embedder through
  `ComparisonPlan::bounds_kinds` and `bounds_read_by`.
- **The adapter cannot tell a stated config option from its default**:
  DataFusion 55.1's `ConfigOptions` keeps values only, so `ready_session`
  turns the aggregate's dynamic filter off whatever the config says, and a
  caller wanting it on sets it after (`../status/upstream.md`, "UF1").

### Counting a hot-path change

- **`measure.py --count-pair <rev>` is the instrument**, its rules in its
  docstrings and `CountPair` in `scripts/test_measure.py`; the reading rule
  is the spec's "What a hot-path slice owes", which a later phase adopts by
  its own spec or not at all. A pair is two fat links plus five reps a shape
  a leg, 15–27 minutes, so it is launched detached and the next round reads
  its log. `<rev>` is the tree's parent: `HEAD~1` after committing, `HEAD`
  over an uncommitted tree, and a reading is taken from a commit.
- **Two builds of one source are two layouts** — mimalloc embeds `__TIME__`
  — but on the identical-source pair no row resolved: the change column is
  run-to-run. A five-rep range does not bound another sitting's counts of
  the same binary, so a pair is read only against itself.
- **A sampled profile cannot attribute a rise of this size**, whether of the
  profiling build (line tables move the code) or of the shipped one (one
  sampling grid per build). Callgrind's `Ir` on the two shipped legs can, to
  within 0.3% of the native count, in four to six minutes, so it is waited
  on. The apparatus is `runs/attr-36.4.3-20261016-callgrind/` (`run.sh`,
  `analyze.py`, its reading written before the count), with
  `runs/attr-36.4.2-20261015-2200/codediff.py` (code identity, checked
  before any reasoning about inlining),
  `runs/attr-36.4.4-20261016-callgrind/perinstr.py` and
  `runs/attr-36.9-20261019-callgrind/`'s `callers.py` and `cls.py`. A pair's
  legs are copied out of `runs/pgdt-count-pair-*` before the next pair
  overwrites them.
- **Accepted rises**, each counted exactly and none recoverable by a shape as
  clean: the summary's stored-bound comparison (36.4); `CompareKind::Enum`'s
  name widening a stride the gatherer indexes (36.4.4); one more move of each
  scanner event through `ChunkedScanner` (36.5); identical code's stack offset
  under a frame `observe_row` grew, and the provider's scan planning once a
  query (36.9).
- **Unattributed**: the data-level `parse`'s rep 2 falling on both legs of
  the identical pair; `pgdt sql` never reaching the upper of its two values
  in a pair; `nerdctl`'s broken-pipe warning after some builds; the `query`
  typed fall at 36.9.1; what moved `observe_row`'s frame at 36.9.

### Mechanisms

- **A block's database is stored once, on its span**, and every function
  resolving a block against the DDL takes `(metadata, database)`, the
  database its own argument after `metadata`. `CACHE_FORMAT_VERSION` 69. The
  DDL is shared by `Arc` from `DumpIndex.metadata` with no format change.
- **The seek table is opaque** (`pgdt_io::SeekTable`), persisted byte for
  byte as `xz_seek`'s under `pgdt-io`'s `serde` feature; `walk_seek_table` is
  `pub(crate)`, only the fetched path walking.
- **`ChunkedScanner`'s carry is two buffers swapped by `absorb`**, so an
  event borrows the chunk rather than the scanner and the replay can hold a
  span's validated prefix across rows.
- **A cold query's replay holds the queried table's statistics**, for a
  dynamic filter; `pgdt query` carves around them (`M227`). A plan still
  clones the table's blocks into `TablePartitions`, stripping statistics the
  query does not use.
- **`Table::settle`'s token is not handed to the source**, so a remote read
  in flight finishes before the next chunk's poll; one that fires answers
  `ScanCancelled` once the map is saved.
- **A resumed stream is serial whatever the plan's cut**, and its token is
  the whole table's, restamped with no partition's fingerprint (D51).

## Negative results

- **The count pair reuses no figure machinery**: `Session.time_run` stages,
  records telemetry and keys per figure, none of which a count needs, and a
  shape wrapped in `peak-rss` cannot be counted, so `statistics-gathering`'s
  data leg gained an unwrapped spelling.
- **Rework resting on codegen was refused throughout**: inlining attributes
  or a cold continuation for `ChunkedScanner`'s pass switch, an infallible
  `end_pass` (moving where a long line is refused, a semantic change for
  codegen), an enum's name and labels behind one `Arc` (a dependent load on
  every label scan). Boxing `Event::CopyStart` was not 36.5's code; it landed
  as `M225`, a fall on every row.
- **`ChunkedScanner` as one event stream across chunks, or a callback per
  event**: `map_forward` awaits in its handler and polls between chunks, and
  the replay consults its dynamic filter there. A lending `next_event` over
  one carry buffer does not compile — the borrow checker's conditional-return
  case.
- **Keys of one variant per order** (36.4.1) cost the hot path in register
  saves and key copies with nothing gained within a column; the order's
  identity moved onto `ValueKey`.
- **Opening `pgdt parse` and `pgdt query` through `CacheMode::open` before
  their passes took the open** would have decoded the cache twice and opened
  two watches; each moved when its pass took the `Opened` whole. `Held` as an
  `Option<LoadedCache>` is D22's rejected collapse.
- **The open's values passed apart** — `map_file` taking the source, watch and
  `Held` separately, or a `&CacheMode` beside the `Opened` — can name a cache
  other than the one judged. `Opened` generic over a borrowed source would
  put a lifetime on what the `Dump` owns.
- **`backfill_block` over `ScanOptions` and `GatherOptions`** states the
  parallelism and the allowance in two values that can disagree; over
  `&mut DumpIndex` and an offset it needs a lookup that can fail.
- **`Parallelism` inside `ReadOptions`**: `ChunkedScanner` does not read it,
  and a replay announces a parallelism other than its mapping pass's.
- **The DDL's `Arc` beside the index rather than in it** would leave
  `Dump::index().metadata` reading `None`, against `DumpIndex`'s own doc.
- **A `Table<'a>` borrowing its `Dump`**, reversed: an early-stopped pass
  yields a `Table` too.
- **A value-free copy of the filter operator** beside the value-carrying one
  — a public `Test` enum, or a private mirror for the resolved term — to save
  one clone a block; so `resolve_term` panics on a membership `resolve_leaf`
  never routes to it.
- **Re-partitioning a resumed stream** would need a cut starting inside a
  block at a token's in-copy state.
- **Renaming every `qualified_name` to `qualified`**: the longer name has
  three times the call sites and says what it returns.
