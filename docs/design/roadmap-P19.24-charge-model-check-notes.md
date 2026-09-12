# `P19.24` — the harness checks the model

What `19.22`, `19.23` and the closing sweep inherit. The spec row is
[`roadmap-P19-efficient-defaults.md`](roadmap-P19-efficient-defaults.md),
"Slices"; the charge it mirrors is `XzSource::block_reader_bytes` and the
constant it bounds against is `pgdump_query::io::MEMORY_RESERVE`.

No library code, no sitting, no published table. What lands is arithmetic the
`reserve` renderer now does at every cell, which `19.16` did by hand over
`readings.json` after four hundred runs had been spent searching for one
integer.

## What the check is

`scripts/measure.py` gains three pure functions and one section of the
`reserve` figure's emitted prose.

- `pool_floor_bytes(unit, jobs)` — `(POOL_DEPTH − jobs)⁺ × unit`, the block
  pool's floor, which the per-reader charge does not bill.
- `charge_model(unit, jobs, held)` — `(billed, floor, unnamed)`: what the rule
  charged, what the model names on top of it, and what neither names.
- `charge_model_problem(unit, jobs, held)` — the two-sided criterion, or
  `None`.

The renderer walks the flagless legs, prints a row per cell — readers, billed,
pool floor, worst rep held, unnamed remainder, criterion — and ends on a
verdict that either says the model holds at every cell or names each cell that
refutes it.

**The criterion is stated before the answer, and neither side is a tolerance
anybody picked.** The unnamed remainder must be **non-negative**, because a
negative one is an over-bill: bytes the rule charged that nothing holds, and so
a reader the allocation would have afforded. And it must be **no larger than
`MEMORY_RESERVE`**, because the reserve is by construction what covers
everything the charge does not bill — so a remainder above it is the gate
failing, said per cell rather than per sitting.

**The over-bill side is the half a grid search cannot report at all.** A charge
that is too large shows up in a headroom sweep as headroom, which reads as the
constant being adequate. That asymmetry is why the row exists.

## Seeded, not fitted

Every number the check produces is arithmetic from the source evaluated at a
cell (`.claude/skills/evidence/SKILL.md`, rule 1). What "seeded from readings
already in the tree" bought is the test: `19.16`'s notes commit five cells of
`control_xz128` — reader count, worst rep, resolved budget and the residual
that slice computed by hand — and `test_the_model_reproduces_the_readings_it_was_seeded_from`
puts them back through `charge_model`. It agrees to a tenth of a mebibyte on
all five, including the resolved budget, which is the column a run prints for
itself.

`ChargeModelSection` renders the whole figure over a fixture whose flagless
legs carry `19.16`'s r384 grid at both block sizes. That table is what the
section will look like when the sitting is real:

| leg | readers | billed | pool floor | worst held | unnamed |
|---|---:|---:|---:|---:|---:|
| 24 MiB, `512m` | 2 | 116.1 | 48.0 | 251.9 | 87.8 |
| 24 MiB, `1g` | 11 | 638.4 | — | 817.2 | 178.8 |
| 24 MiB, `1536m` | 19 | 1102.6 | — | 1224.2 | 121.6 |
| 24 MiB, `2g` | 24 | 1392.8 | — | 1515.5 | 122.7 |
| 128 MiB, `512m` | 2 | 532.1 | 256.0 | 802.1 | 14.0 |
| 128 MiB, `1g` | 3 | 798.1 | 128.0 | 939.9 | 13.8 |
| 128 MiB, `1536m` | 4 | 1064.1 | — | 1077.7 | 13.6 |
| 128 MiB, `2g` | 5 | 1330.2 | — | 1342.6 | 12.4 |

It is a fixture and not a reading — the 24 MiB rows are inverted out of
`19.16`'s headroom column — so no document may quote it. What it demonstrates
is the shape: the remainder is flat at 11–14 MiB where the pool floor is
named, and 88–179 MiB on the 24 MiB file, which is the arena retention the
reserve exists for.

## The three calls made inside the row

**The floor is a named column, not part of the remainder.** Folded in, it reads
as a flat term on the two block sizes this harness registers and as a breach on
a third: it is `(POOL_DEPTH − jobs) × unit`, so 96 MiB at koji's blocks, 384 at
128 MiB and 2 GiB at 512. Naming it is what turns the check's output from "a
large number" into "an under-bill", which is what the row asked for and what
`19.22` repairs.

**The check reports; it neither raises nor bars publication.** Those are two
levers rather than one, and `KILL_TOLERANT` pulls both — the sitting survives a
kill *and* `emit` refuses to publish the figure that lost the leg
(`19.17.1`'s row states it as two clauses). So the precedent settles the first
half and is silent on the second, which turns on what a cell is: a censored cell
is a bound on a peak the process never reached, so its table describes an
arrangement nobody measured, while every cell here is a real reading and what a
refutation falsifies is the library's claim about those readings. Barring
publication would leave `measurements.md` able to carry only tables agreeing
with the library. The gate is `19.11`'s acceptance instead, which is two-sided:
the sweep publishes, **and** this verdict reads that the model holds at every
evaluated cell.

**A declined or censored leg is absent rather than evaluated.** The streaming
fallback holds none of the model's terms, and a censored leg's reading is a
bound on a peak the process never reached. Both exclusions are read off the
leg's own reported budget and kill count, which is how the fit above already
makes them.

## The falsified claim this row found

`measure.reader_bytes`' docstring said `XzSource::default_memory_per_worker`
answers **7 MiB more per reader** than the affordability charge, "deliberately",
and gave a flagless run's resolved budget as 1,636,608,768 at 24 readers of
24 MiB blocks. `19.19` removed that split: `charged_chunk_bytes` falls back to
`DEFAULT_CHUNK_SIZE` rather than to `POOL_MAX_BYTES`, so the recommendation and
the gate are one number and the budget is 1,460,448,000. The paragraph was
written at `df170bc`, which precedes `19.19`.

It is corrected here because the model rests on it — a check built on a mirror
that claims two charges would be a model of neither — and
`test_the_recommendation_and_the_affordability_charge_are_one_number` pins the
`io.rs` line that makes it one, so a future split fails rather than falsifies.

**`M81`'s withdrawal row carried the same paragraph** and is now one line, as
the ledger's own rule requires: the row is an index entry, the account belongs
in the dated history entry it points at, and that entry already held every
sentence of it. Trimming removed the falsified mechanism from the ledger with no
new rule and no loss — `M82`'s row had the same defect and the same remedy.

## What `19.22` inherits

- **The model is the thing to change, and the check is what says so.**
  `19.22` makes `Parallelism::fit` solve for the largest affordable count
  instead of dividing by a per-worker scalar; when it lands, `pool_floor_bytes`
  stops being a term the *model* adds on top of the charge and becomes part of
  the charge, so `charge_model` collapses to `billed` and a remainder. The
  seeded test is what holds both shapes to the same five cells.
- **`LIBRARY_POOL_DEPTH` and `LIBRARY_MEMORY_RESERVE` are mirrors**, hardcoded
  on `QUERY_SUBSTREAM_CAP`'s argument and pinned against `io.rs` by
  `test_the_mirrored_pool_depth_and_reserve_are_the_librarys_own`. A constant
  moved in the library and not here checks the rule against a promise it no
  longer makes.
- **Nothing went stale.** The only paths this slice touches are
  `scripts/measure.py` and `scripts/test_measure.py`; the first is declared by
  `session-drift` alone, which was already red, and `reserve` declares neither.

## What `19.11` inherits

The section is emitted by `run_reserve`, so the closing sweep publishes it with
the rest of the figure. **A sitting whose model is refuted publishes the
refutation**, which is the intended behaviour and not a fold-in hazard: the
verdict is generated prose like every other sentence in that renderer and is
never hand-edited.
