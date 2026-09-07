# P16.16 — `--jobs` defaults to 1

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md). How the mechanism works is
[`architecture.md`](architecture.md), "Execution model and API surface".

## What exists now

**`DEFAULT_JOBS` is a constant, 1, where `default_jobs()` called
`available_parallelism()`.** `ParallelArgs::resolve` reads it, so `pgdq parse`
and `pgdq query` both run the serial path unless a person states `--jobs`. The
flags themselves are untouched — same names, same parser, same zero refusal,
same `--parallel-memory` — and **no library code moved**: `Parallelism::Serial`
is still what `Parallelism::workers(1, …)` answers, and the two entry points a
CLI command reaches (`table_stream_partitions` and the leader-carrying
`map_forward`) are the same calls they were, taking one sub-stream and declining
every region.

**The help text states what is asked for, not a ceiling.** `--jobs`' doc comment
and [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--jobs`
and `--parallel-memory`", now both say that three things hold the delivered
count below the stated one: a single-block `.xz`, an `INSERT` run, and — the new
half — the chunk pool on a plain file, where `hint_parallelism` clamps
`BufferPool::slots()` to `POOL_DEPTH` and a fifth fused worker blocks for a
slot, so `parse` above `--jobs 4` runs four and queues the rest. The manual
folds that into the paragraph it already had about plain files rather than
opening a second one, because the two say the same thing from different ends:
the ceiling is real and it costs almost nothing on the shape that has it.

## The calls worth knowing about

**A constant, not `Option::unwrap_or_else` over a function returning 1.** The
default is now a value rather than a computation, so nothing in the binary asks
the platform how many cores it has. That is the point of the row: a flagless
invocation must not measure the machine it ran on, and a function whose body
happens to return 1 would put the question back the first time somebody
"restored" it.

**The reason lives in the constant's doc comment, not only in the spec.** It
names the two costs that are known — read depth unmeasured on every device
class, and N sub-streams on a HDD being N separated offsets read at once — and
the one gain that is not yet measured, which is what `parallel-scan-throughput`
is asked to license. A future session raising this number should be doing so
because that figure exists.

**`POOL_DEPTH` was deliberately not moved.** The four-worker plain-file ceiling
is a memory-accounting number, and raising it with a worker count grows a
query's resident set by `(jobs - POOL_DEPTH)` chunks the moment `RetainedChunks`
releases a flushed batch's buffers. Saying what the ceiling *is* costs nothing;
moving it belongs with the work that makes one stated number bound both memory
terms (`16.15`).

## What this cost elsewhere

**Four rationale sites in the harness argued from the old default and were
corrected in the same change.** `measure.SWEEP_JOBS`, `_script`,
`worker_count_problems`, `--check`'s unpinned-shape message, plus
`rss_attribution.py`'s `JOBS` and two `test_measure.py` docstrings, all said
"the CLI's default is the machine's available parallelism" as the reason a shape
must pin a count. The reason is now that the default is **not apparatus and has
moved twice**, which is the durable form of the argument and the one that
survives the next flip. Nothing mechanical changed: `SWEEP_JOBS` is still 1 for
the reason it always was — it reproduces the published sitting — and its
agreeing with the CLI today is a coincidence the comments now say is a
coincidence.

**The default is pinned by a unit test, because nothing else can see it.**
`stating_no_parallelism_flag_is_the_serial_path` asserts that
`ParallelArgs { jobs: None, .. }.resolve()` is `Parallelism::Serial` and that a
stated 8 is not. That test exists precisely because the phase's central promise
— a partitioned run and a serial one produce the same bytes — makes the default
invisible from outside the binary: every integration test in the tree would pass
with the default back at `available_parallelism()`. Which is the failure `M69`
was admitted for, one level down.

**No existing test changed its assertion.** `parallelism.rs`'s reference runs were
already pinned at `--jobs 1` by `16.9`, so the flip makes the reference and the
default coincide without either moving; the doc comment saying *why* the
reference is stated rather than inherited was reworded to survive that. The
`vec![]` legs of the two parity tests are now serial rather than parallel, which
is a weaker leg than it was — the parallel coverage they carry is the explicit
`--jobs 8` legs beside them, and `leader.rs`'s own tests, which state their job
counts.

## What the next slice inherits

**`16.11` is next**, and nothing here touches the failure path. The error
ordering `pgdq query` has is still one fill round's — `join_all` in partition
order — and the flip only means the default invocation has one sub-stream, so a
test for it states `--jobs`.

**`16.12`'s determinism test now compares a default against a stated flag as
well as `--jobs 1` against `--jobs 8`.** Since the default is the serial path, a
`--jobs 1` leg and a flagless leg are the same execution; the test's value is
entirely in the `--jobs 8` leg, and asserting the flagless one buys nothing it
does not already have.

**`16.13` is what licenses raising this back.** The figure's plain legs must be
read against the four-worker chunk-pool ceiling — the fact is in the spec's
`16.13` row — and a raise afterwards is a decision change owing grilling, a spec
amendment and its own number, deliberately not pre-allocated.

## Figures

**No figure went red that was not, and none moves.** The diff touches two
declared paths: `pgdump_query-cli/src/main.rs`, which every figure declaring it
was already red on, and `scripts/measure.py`, which `session-drift` already
holds red. The eight figures that do not declare the CLI —
`census-brace-free`, `census-arrays`, the three `scan-throughput-*`,
`per-block-quadratic`, `peak-rss` and `preamble-prepass` — are unmoved by it,
and `nested-decode-micro` is still the one sweep figure green.
Reachability does not excuse it — the CLI is executed by
every registered shape — but there is nothing for it to excuse: since `M69`,
**no command shape inherits the CLI default**, so what the flip changes is what a
person who states no flag gets and nothing the register measures. That was the
whole reason `M69` was ordered in front of this row.
