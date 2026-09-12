# `P19.26` — the margin predicts with its own constant

What `19.25` and the closing sweep inherit. The spec row is
[`roadmap-P19-efficient-defaults.md`](roadmap-P19-efficient-defaults.md),
"Slices"; the mechanism is [`architecture.md`](architecture.md), "Execution
model and API surface", under *The criterion is enforced once*.

No sitting. Every number below is arithmetic over readings already in the tree
(`.claude/skills/evidence/SKILL.md`, rule 1).

## What landed

`io::MEMORY_UNPOOLED_BOUND` — **256 MiB**, public and re-exported — and one
subtraction changed:

```rust
fn margin_allowance(limit: u64) -> u64 {
    (limit / 100).saturating_mul(100 - MEMORY_MARGIN_PERCENT).saturating_sub(MEMORY_UNPOOLED_BOUND)
}
```

`MEMORY_RESERVE` keeps the cap (`limit − reserve`, which is what
`BlockCache::affordable` still reads) and its doc comment stops claiming to
bound the flat term. Nothing else in the library reads the new constant.

## The bound, derived

`19.16`'s `runs/19.16-reserve-constant-20260911-2210/readings.json` — 400 runs,
five reserve builds × two block sizes × four container limits × ten reps — read
back under **today's** charge (`WorkerMemory::at`, which since `19.22` bills the
block pool's floor). Each run states its own resolved count and budget, so the
charge is evaluated at the arrangement that actually ran rather than at the one
the limit would resolve now. 270 of the 400 took the block path
(`block_path_afforded` over the reported budget); none exited non-zero and none
was OOM-killed.

The remainder is `held − at(jobs)`. Worst rep of ten, by reader count:

| 24 MiB blocks | 3r | 4r | 8r | 9r | 11r | 12r | 13r | 17r | 18r | 19r | 20r | 22r | 24r |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| MiB | 134.9 | 157.6 | 158.6 | 131.7 | 178.5 | 130.9 | 177.5 | **214.6** | 205.7 | 121.8 | 118.8 | 83.5 | 145.3 |

| 128 MiB blocks | 3r | 4r | 5r | 6r |
|---|---:|---:|---:|---:|
| MiB | 13.8 | 13.6 | 12.4 | 11.3 |

**256 MiB is the next 64 MiB step above the worst of those**, 64 MiB being the
granularity of the candidate grid the reading comes off. The 41.4 MiB it adds is
the same order as that apparatus's own scatter, which `19.16` measured at 2.1
percentage points of the limit — 32 MiB at 1536m, 43 at 2 GiB — on an
arrangement the constant provably could not move.

**Three properties of the remainder, and each rules something out.** It has no
trend across 3–24 readers, so it is not a per-reader term the charge is missing.
It is an order of magnitude *smaller* where the blocks are larger, which is the
opposite of anything scaled by the unit — so neither a division by the block
size nor a second per-reader coefficient describes it. And it is **unattributed**
beyond `19.18`'s reading that it is glibc's arena retention: no term table sums
to it, and the code comment, the architecture section and the harness's prose
all say so in those words (rule 6).

**The negative tail is two reps and the check never sees them.** Two of the 270
legs read 0.70 and 0.75 MiB *under* their bill — reps 5 and 7 of
`control_xz128` at `-m 2g` on the 512 MiB build, five readers, 0.06% of a
1330.2 MiB charge — while the other eight reps of that same cell run +4.3 to
+12.4. `charge_model` is evaluated at `max(readings)`, so a cell sitting exactly
at its bill cannot present as an over-bill; the two are a reading about how
tight that cell is, not about the charge.

## The criterion, checked inside the data

The library predicts `at(n) + 256 MiB` and requires it under four fifths of the
limit. Substituting each family's **worst measured** remainder for the constant
is the check that the prediction is not merely self-consistent:

| blocks | limit | readers | charge | + worst remainder | headroom |
|---|---|---:|---:|---:|---:|
| 24 MiB | `544m` | 1 | 130.0 | 344.6 | 36.6% |
| 24 MiB | `1g` | 9 | 522.3 | 736.9 | 28.0% |
| 24 MiB | `1088m` | 10 | 580.3 | 794.9 | 26.9% |
| 24 MiB | `1536m` | 16 | 928.5 | 1143.1 | 25.6% |
| 24 MiB | `2g` | 23 | 1334.8 | 1549.4 | 24.3% |
| 128 MiB | `1088m` | 1 | 650.0 | 663.8 | 39.0% |
| 128 MiB | `1536m` | 3 | 926.1 | 939.9 | 38.8% |
| 128 MiB | `2g` | 5 | 1330.2 | 1344.0 | 34.4% |

Every cell clears 20% with room, and the two thinnest are also covered by a
reading rather than by substitution. The 1 GiB cell is the *same arrangement*
`19.16` measured on its r448 build — nine readers on a 522.3 MiB budget, the
reserve entering only through the cap and not through what the process holds —
and that leg read **36.1%** worst-rep headroom. The 2 GiB cell is bracketed
rather than matched: `19.16` measured 24.9–27.0% at **24** readers there, and
the new rule resolves 23, which holds one reader less. Same apparatus and same
arithmetic as the derivation above, so this is the model evaluated inside its
own data (rule 2) and not a second, independent reading of it.

## The crossover, which is new behaviour and not a number

`limit − MEMORY_RESERVE` and `0.8 × limit − MEMORY_UNPOOLED_BOUND` are equal at
`5 × (MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND)` = **640 MiB**. Below that the cap
is the tighter condition and **the margin lowers nothing**; above it the margin
binds at every limit. That is the shape the fraction was added for, and the old
arrangement — one constant serving both — bound at every limit including the
ones where the subtraction already covered it: a 512 MiB allocation resolved one
reader of an ordinary 24 MiB-block `.xz` where its own cap affords two.

`the_margin_is_the_tighter_condition_only_above_the_crossover` pins it, because
nothing else would notice either constant moving past the other (rule 5). The
equality holds to under 100 bytes — `limit / 100` discards that much, and the
allowance errs small.

## What the closing sweep inherits

**The flagless axis moves, and this is arithmetic rather than a prediction of
the sitting** — `min(recommendation, fit)` before this host's 24-core clamp, and
the budget each count spends:

| leg | before | after |
|---|---|---|
| 24 MiB, `512m` | 1 (128.0 MiB, streaming) | unchanged |
| 24 MiB, `544m` | 1 (130.0, block) | unchanged |
| 24 MiB, `1g` | 7 (406.2) | **9 (522.3)** |
| 24 MiB, `1088m` | 8 (464.3) | **10 (580.3)** |
| 24 MiB, `1536m` | 14 (812.5) | **16 (928.5)** |
| 24 MiB, `2g` | 21 (1218.7) | **23 (1334.8)** |
| 128 MiB, `512m`/`544m`/`1g` | 1, streaming | unchanged |
| 128 MiB, `1088m` | 1 (650.0, block) | unchanged |
| 128 MiB, `1536m` | 2 (788.1) | **3 (926.1)** |
| 128 MiB, `2g` | 4 (1064.1) | **5 (1330.2)** |

Three consequences:

- **`M89`'s two floor legs are untouched**, so the acceptance gate that the
  evaluated set bills a nonzero pool floor at each block size keeps its cells.
  `544m` and `1088m` resolve one reader on the block path before and after, the
  margin never taking the last reader and the budget staying `cap`-bounded.
- **Both families still afford three distinct reader counts**, which is `M90`'s
  fit guard: the 24 MiB block-path legs now resolve `{1, 9, 10, 16, 23}` and the
  128 MiB ones `{1, 3, 5}`, the second still exactly on the bound.
  `reserve_floor_problems` passes unchanged — both of its mirrors argue from
  `discovered_budget`, the cap, which this row does not move.
- **`charge_model_problem` now has two ceilings**, and the sitting's verdict
  distinguishes them: a remainder above 256 MiB is a finding about the bound
  (the allocation still holds), one above 384 MiB is the rule not holding. The
  seeded fixture — `19.16`'s r384 grid — passes both, its largest remainder
  being 178.8 MiB.

## What `19.25` inherits

Nothing it has to change. `19.25` reports the **delivered** count against the
announced one; this row moves what is announced, not how. The mode report's
`(recommended by the source; lowered from N by the allocation)` clause already
covers the margin, as `19.23` left it.

## What was refused

**A bound fitted rather than bracketed.** The remainder has no trend in the
reader count, so a least-squares line through it would put its whole scatter
into an intercept and report a number tighter than the worst cell — the trap
that cost this phase three sessions
([`architecture.md`](architecture.md), "There is no fixed term"). The bound is
the worst observation rounded up, which is the only reading that is safe in the
direction a cgroup kills in.

**A per-block-size bound.** The remainder is 10.9–13.8 MiB at 128 MiB blocks
against 83.5–214.6 at 24, so a constant sized for the small-block case
over-predicts the large-block one by ~240 MiB — which at `1536m` and `2g` costs
the 128 MiB family a reader. It was refused for `MEMORY_RESERVE`'s reason and
the same one applies harder here: the number is wanted by
`Parallelism::discover`, which is reached with nothing open, and a source's own
answer is downstream of recognition, which is I/O. Over-predicting is the safe
direction; what it costs is named here rather than left to be rediscovered.

**Setting the harness's single threshold to the new constant.** That would make
a bound that is slightly low read as a failed rule. Two lines, both registered
before the sitting, is what separates *the number the count is predicted
against is wrong* from *the discovery cannot keep this arrangement inside its
allocation*.

## Nothing went stale

The paths this row touches — `pgdump_query/src/io.rs`, `pgdump_query/src/lib.rs`,
`pgdump_query-cli/src/main.rs`, `scripts/measure.py` — were all already red for
every figure that declares them, and the `reserve` figure has never been
published. `uv run measure.py --check` passes.
