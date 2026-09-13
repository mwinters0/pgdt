# P19 — efficient defaults for a parallel scan: notes

What the phase built is in [`architecture.md`](architecture.md), filed by
subject: the worker default and the budget rule under "Execution model and API
surface", the two parallel arrangements under "The interior split" and
"Partitioned replay", the compressed source's charge under "The compressed
source", the instrument under "What the binary can report about itself", and
the status lines under "Status output". The spec it was measured against is
[`roadmap-P19-efficient-defaults.md`](roadmap-P19-efficient-defaults.md).

This doc holds what subject-filing has no home for: **the readings that exist
nowhere else**, the phase's **negative results**, and what it hands the phases
after it. The twenty-seven slices' own notes docs were consolidated here at the
wrap.

---

## What the phase shipped

Three defaults, and a fourth thing nobody asked for.

- **The worker count comes from the source.** `ByteRangeSource::default_workers`
  — a plain file answers one, an `.xz` file `available_parallelism()` capped at
  its own block count. A stated `--jobs` wins outright in both directions.
- **The memory limit is discovered.** `discover_memory_limit` reads the cgroup
  limit that actually binds, taking the minimum over ancestors, and
  `Parallelism::discover` takes `limit − MEMORY_RESERVE` as the budget ceiling.
- **The count answers to a criterion, not to the core count.**
  `MEMORY_MARGIN_PERCENT` refuses a count whose predicted resident —
  `at(n) + io::MEMORY_UNPOOLED_BOUND` — leaves under a fifth of the limit.
- **A `parse` says what it delivered.** The `scan arrangement` line names the
  delivered reader count beside the announced one, which rule cut it, and the
  number that would buy the refused arrangement back.

Two constants carry the whole rule and both are readings rather than fits:
`MEMORY_RESERVE` = **384 MiB** and `io::MEMORY_UNPOOLED_BOUND` = **256 MiB**.
The sections below are where each came from.

---

## The reserve constant, read off five builds

Five binaries differing in nothing but `MEMORY_RESERVE` — 256, 320, 384, 448,
512 MiB — over eight legs (two files × four container limits), ten reps each:
400 runs. The criterion is that the **worst** rep leave at least 20% of the
limit, with nothing exiting non-zero and nothing OOM-killed. Worst-rep headroom
per candidate and leg, `<<` marking a leg under the floor, resolved reader
count in parentheses:

| leg | r256 | r320 | **r384** | r448 | r512 |
|---|---|---|---|---|---|
| xz24 512m | 23.9% (4) | 35.0% (3) | **50.8% (2)** | 87.7% (1) | 97.0% (1, declined) |
| xz24 1g | 9.0% << (13) | 19.2% << (12) | **20.2% (11)** | 36.1% (9) | 39.2% (8) |
| xz24 1536m | 11.4% << (22) | 16.7% << (20) | **20.3% (19)** | 18.6% << (18) | 21.8% (17) |
| xz24 2g | 27.0% (24) | 26.3% (24) | **26.0% (24)** | 26.2% (24) | 24.9% (24) |
| xz128 512m | 97.0% (1, declined) | 97.0% | **97.0%** | 97.0% | 97.0% |
| xz128 1g | 21.7% (2) | 21.7% (2) | **21.7% (2)** | 21.7% (2) | 72.7% (1) |
| xz128 1536m | 29.8% (4) | 29.9% (4) | **29.9% (4)** | 29.9% (4) | 38.8% (3) |
| xz128 2g | 21.5% (6) | 21.6% (6) | **21.6% (6)** | 21.5% (6) | 34.4% (5) |
| **verdict** | fails | fails | **PASSES** | fails | PASSES |

**384 is the pick and the reading cannot separate it from a failure.** Its
margin at the two deciding legs is 0.2 and 0.3 percentage points, and the
apparatus's own noise at a comparable arrangement is 2.1. The gate is also
**non-monotonic** — 384 passes, 448 fails, 512 passes — which is what a
criterion this close to the noise looks like. It was put to review on
2026-09-12 and **384 stands**: reconstructed against the criterion it is the
only candidate on this grid inside both deciding legs' admissible ranges.

**What it costs is one arrangement.** Median wall clock on the 24 MiB-block
file: the 512 MiB container goes 4 readers → 2 and 1.77× slower, and every
other leg is within 8% of what 256 does. Going on to 512 would cost a further
16% at 1g, 20% at xz128 1536m and 23% at xz128 2g.

**The rule resolves exactly, at every one of the forty cells.** The resolved
count is `min(24, max(1, floor((limit − reserve) / per_reader)))` and the
budget requested is `count × per_reader` **to the byte**, never the cap.

### The block path buys nothing below three readers, on this file

The 512 MiB column above is a clean reader-count sweep — one limit, one file,
only the count varying. Four readers is 1.66× the streaming fallback and three
is 1.31×; **two readers is 6% *slower* than declining and one reader is 3%
slower**, while holding 250 MiB and 63 MiB respectively against the fallback's
15. Below three readers the block path costs memory and returns no time. It is
a [`roadmap.md`](roadmap.md) Future item; nothing in this phase acted on it.

### Arena retention is inside the charge, not above it

Over the block-path regime the worst-resident slope is **57.28 MiB a reader
against the 58.03 billed — 0.987**, so the per-reader charge was right to 1.3%
under that charge. The excess `worst − 58.03 × jobs` is 135.7 MiB at two
readers and 145.4 at twenty-four, wandering **83.6–214.6 MiB with no trend
across jobs 2…24**. So `19.18`'s "retention rises with the count the allowance
affords" is true of `fordblks` and does not reach the reserve, and a reserve
computed from an arena count would be computing the term that is already
billed.

### The charge under-bills the pool floor

Read out of this sitting's own `readings.json` with no new run. Below four
readers `BufferPool::slots` clamped the block pool at `POOL_DEPTH` while
`block_reader_bytes` billed `2 × unit` a reader, so the pool held units nobody
charged for — `(POOL_DEPTH − jobs) × unit`, on `control_xz128`:

| jobs | worst RSS | budget | excess | `(4−jobs) × unit` | residual |
|---:|---:|---:|---:|---:|---:|
| 2 | 802.1 | 532.1 | 270.0 | 256.0 | 14.0 |
| 3 | 939.9 | 798.1 | 141.8 | 128.0 | 13.8 |
| 4 | 1077.7 | 1064.1 | 13.6 | 0.0 | 13.6 |
| 5 | 1342.6 | 1330.2 | 12.4 | 0.0 | 12.4 |
| 6 | 1607.5 | 1596.2 | 11.3 | 0.0 | 11.3 |

Residual flat at 11–14 MiB. The term is **unbounded in the block size** —
96 MiB at 24 MiB blocks, 384 at 128, 2 GiB at 512 — which is why no reserve
absorbs it. `19.22` made it a charge (`io::WorkerMemory`) and `M93` restated it
as what the pool actually holds, `(POOL_DEPTH.max(jobs) − 1) × unit`, billed at
every count.

---

## The margin constant, derived by arithmetic

`io::MEMORY_UNPOOLED_BOUND` = **256 MiB** was derived over the 400 runs above
under today's charge, not measured again. The remainder — what a leg held above
what the charge bills — has **no trend in the reader count**, so it is
bracketed rather than fitted: 256 MiB is the next 64 MiB step above the worst of
them. It is 10.9–13.8 MiB at 128 MiB blocks and larger at 24, and a
per-block-size bound was refused for that reason: a constant that varies with
the file is one the margin cannot state before it opens the file.

**A negative tail exists and the check never sees it.** Two of the 270
block-path reps read the remainder negative. They are inside the instrument's
own scatter and are not an over-bill; they are recorded because a later session
computing the same remainder will meet them.

---

## The budget rule against real cgroups

`19.15` ran the shipped rule inside real containers and **it did not pass**: a
flagless `.xz` `parse` in a 512 MiB allocation left 0.6% of the limit at the
worst rep, and it was the **fixed** term rather than the per-reader one that
exceeded the reserve. That is what convened `19.16`. Two of that probe's
findings are load-bearing and are not re-derivable from the shipped code:

- **`19.12`'s fit is refuted in both terms.** `403 MiB + 31.2 MiB a reader`,
  fitted over 3–24 readers, predicts 436 MiB at one reader against **62.9 MiB
  measured**. A concave curve fitted over a window that excludes the origin puts
  the excluded curvature into the intercept. This is the worked instance behind
  the `evidence` skill's rule 2.
- **It is not arena retention alone.** Capping arenas moves the term by a
  fraction of it; the rest is the pool floor above, which `19.16` then found by
  arithmetic rather than by another sitting.

---

## The compressed path's resident account

`19.18` put the introspection build on the same flagless shape and read the
program's own live bytes rather than differencing two binaries.

- **The program has no fixed term**: `live_peak = 4 MiB + 49.0 MiB a reader`,
  evaluated inside its own window at one reader.
- **The charge is right to 2%**: 49.0 MiB of Rust plus `liblzma`'s 8.0 MiB
  dictionary against what `reader_bytes` billed.
- **The excess above the budget is glibc arena retention**, named by
  `mallinfo`'s `fordblks` rather than inferred from a subtraction, and the arena
  cap moves it in the place the instrument predicted.
- **Do not read a fixed term off the black-box fit.** It reports 129 MiB fixed
  where the instrument reports 4; the difference is the pool floor and the
  retention, both of which the black-box subtraction cannot separate.

---

## The cut width, decided by measurement

`19.20` measured three cut widths against each other and shipped **neither
extreme**: `BOUNDARIED_PARTITION_UNITS` stays at **1**, and the read shape
became the source's own statement. Three things a later slice must not
re-derive:

- **The account holds at two readers and the wasted decode is the term** — each
  worker decodes its successor's block for a chunk-sized tail (`KD20`), which a
  wider cut amortises at `1/k`. At two stated readers the wall clock follows
  that arithmetic to 1.64× at eight units.
- **At the flagless default the ordering inverts**, and that is what decided it:
  the wide cuts are half the speed there. One unit stays.
- **A width that varies with the resolved count** is the shape the evidence
  points at and it was not taken — it makes the cut size a function of two
  things the charge already couples, which is the confusion `19.19` had just
  separated.

**On the plain path the same change is worth 22×** in resident bytes, 209.2 →
9.4 MiB, which is why the read shape moved even though the width did not.

---

## The per-file term, and two window traps

`19.19` separated a partition's **cut size** from its **memory charge** —
`Partitioning::window_end` is the first and `charged_chunk_bytes` the second —
and struck `KD18`. Two things a later slice must not re-derive, because the
wrong form is the one anyone reaches for first:

**The window's first piece is not a stub.** It looks like one: a window that
starts mid-block appears to begin with a fragment. It does not, because the
previous window's last piece read *past* its own limit to finish a row, so the
frontier sits a few hundred bytes into a block rather than most of the way
through it — and the first piece is a whole block less epsilon.

**Do not size a window at `workers × unit`.** It is the obvious form and it is
off by one: from a mid-block frontier that window contains `workers`
boundaries, `cut` is asked for `workers` pieces, and the thinning drops the
first — which puts two blocks in the first piece and reintroduces exactly the
defect the slice removed. The correct rule is the `want × k`-th boundary
**strictly past** `start`, which is what `window_end`'s doc comment states and
`a_block_decoding_partition_spans_at_most_the_cut_width` holds.

---

## The reserve figure's first sitting

`19.6` registered the figure and took it diagnostically. Its finding is the one
the whole phase rests on: **on a plain source the reserve is a constant, and on
a block-decoding `.xz` source it is not** — the plain leg is flat in both the
budget and the worker count at 37.4 MiB, and the compressed one is not, so
`budget = limit − reserve` cannot bound a compressed parallel scan with a
single number unless the per-reader term is charged separately. Everything from
`19.13` onward is that separation.

---

## The closing sweep, and what it took two attempts to buy

`19.11` published `reserve` and `rss-attribution` for the first time and
re-took every other table with them. **It needed two sittings**, and the
failure is the durable part.

**The first lost `per-block-quadratic` to a binary that predated a flag.** That
figure ran a second build — the pre-throttle "before" column pinned at a commit
453 behind HEAD — and `--jobs` had entered the CLI in between, so the leg exited
2 in 53 ms on `unexpected argument '--jobs'`. `preamble-prepass`, which borrows
its full-`parse` row, was skipped with it, and the two are each other's whole
sharing closure, so no partial sitting could publish either. The column was
**retired rather than repaired** (`M103`): a subtraction against a fixed commit
measures everything that differs between the two trees, and across 453 commits
that is the whole of what it measured. What the throttle bought is kept as a
historical reading beside its mechanism — 47.7 s at 4003 saves — rather than
re-measured every sitting. That is the seventh rule of the `evidence` skill,
and this is the instance behind it.

**The second finding is that a short sweep could still stamp the document**
(`M104`). `emit` computed `whole_sweep` from the *selection*, before the run, so
a sitting that selected all 22 and lost one wrote "All 23 figures below come
from that sitting" over 20 tables. The predicate is now decided after the figure
loop, and `--render` re-derives it from `raw["failures"]` so an older `raw.json`
cannot restore a stamp its run was not entitled to.

**The re-sweep bought one honest stamp over 22 figures with no figure standing
outside it**, which the document had never had. It also reversed one published
claim — the plain `parse` leg, above — and moved nothing else beyond the
instrument's own scatter.

**A test that fabricates input and picks a real register entry as its example
acquires that entry's other properties.** Seven `Sittings.*` assertions used
`peak-rss` as "a figure standing in no edge"; the register move gave it two, so
`sitting_problems` returned the share refusal first and every one of them failed
on a sentence it was not testing — which read as a doc problem and was not one.
They are re-targeted at `map-only` and the premise is now asserted directly.

---

## Negative results with no home beside a mechanism

- **A second binary built from inside `cargo test` was refused.** A `--features
  introspect` build needs its own target dir, which means a full dependency
  rebuild on every `cargo test --workspace`, and no test in this tree shells out
  to `cargo`. The standing cost is that `cargo test -p pgdump_query-cli
  --features introspect` is **not** run by `cargo test --workspace`, and is
  listed separately in `CLAUDE.md`.
- **Computing the delivered-count shortfall in `map_forward` was refused.**
  Taking it from `source.partitions(scanned_through..size)` before the loop
  avoids reshaping `scan_region`'s return, and it is a second copy of the
  decline rule — the failure the `reader_bytes` collapse was done to prevent.
- **Deriving `PLAIN_PARTITION_CHUNKS` from `POOL_MAX_BYTES` was refused.** It
  reads as though one causes the other and buries two independent
  justifications in one expression; the test is what guards the pair.
- **A descending scan over `1..=jobs` was the alternative to `affords`** and is
  equally correct, at O(`--jobs`) with `--jobs` a number a user states. The
  closed form was preferred for that reason alone.

---

## What the `runs/` artifacts hold, and what may be deleted

`runs/pgdq-19.16-r{256,320,384,448,512}` — the five reserve candidates — are
read by nothing in the repo and are deletable. **`runs/19.16-reserve-constant-20260911-2210/readings.json` is not**: both shipped
constants were derived from it, `MEMORY_RESERVE` by the grid above and
`MEMORY_UNPOOLED_BOUND` by arithmetic over the same 400 runs, and no sitting
since has re-taken those readings. Deleting it destroys the only evidence behind
a shipped constant.

---

## What the phase hands forward

Facts a later phase needs are filed in that phase's **inbox** rather than here —
[`roadmap-P6-embeddable-engine-inbox.md`](roadmap-P6-embeddable-engine-inbox.md),
[`roadmap-P14-remote-input-inbox.md`](roadmap-P14-remote-input-inbox.md),
[`roadmap-P15-gzip-inbox.md`](roadmap-P15-gzip-inbox.md) and
[`roadmap-P18-zstd-inbox.md`](roadmap-P18-zstd-inbox.md) each carry entries this
phase wrote. What is left open is in `STATUS.md`'s deficiency register: `KD17`
(a plain typed `query` is flat across the whole `--jobs` range and the pool is
refuted as the cause), `KD20` (the duplicate tail decode), `KD23`, `KD24`,
`KD25` and `KD26`.

**The one thing the phase measured and did not act on** is that a plain `parse`
now reads *above* serial warm — 1.41× at four workers, where the leg read 0.81×
before the `reader_bytes` repair. The default is unchanged and the reasoning is
under `STATUS.md`'s "Decisions worth another look".
