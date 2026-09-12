# `P19.23` — the count answers to the criterion

What `19.25` and the closing sweep inherit. The spec row is
[`roadmap-P19-efficient-defaults.md`](roadmap-P19-efficient-defaults.md),
"Slices"; the mechanism is [`architecture.md`](architecture.md), "Execution
model and API surface", under *The count answers to the criterion*.

## What landed

`io::MEMORY_MARGIN_PERCENT` (20) and `io::margin_allowance(limit)`, and a
fourth argument on `Parallelism::fit`:

```rust
fn fit(jobs, memory, cap, charge_ceiling: Option<u64>) -> (usize, u64)
```

`discover_in` fills `charge_ceiling` with `margin_allowance(limit)` —
`(100 − margin)% of the limit`, less `MEMORY_RESERVE` — for a **discovered**
limit and with `None` for the `MemAvailable`/2 arm, which has no limit to take
a share of. `recommended_within` passes `None`: the margin is a statement about
a limit the environment set, and a `--parallel-memory` somebody typed is not
one.

Three properties are what keep the change to the count alone, and each is
asserted:

- **The ceiling bounds the count, never the budget.** `fit` still returns
  `cap.min(memory.at(affords))`, so a lowered count reports what it spends and
  a limit whose margin allowance is under one reader's charge still reports
  `cap.min(at(1))`.
- **It cannot take the last reader.** `WorkerMemory::affords` is never zero, so
  the margin lowers a count and never declines the one reader the mechanism's
  floors deliver — and therefore never declines the compressed block path,
  which `BlockCache::affordable` decides from the same budget as before.
- **It is host-independent by construction.** The count a limit resolves is now
  the same whether the recommendation is 24 or 64, which is what the row was
  for.

## Why the predicted term is `MEMORY_RESERVE`, and what that costs

The criterion is about **resident** — worst rep leaves ≥20% of the limit — so
enforcing it in the library needs a predicted resident, which is
`WorkerMemory::at(n)` plus whatever a scan holds outside its pools. The only
bound this crate has on that second term is `MEMORY_RESERVE`; it is also the
bound `scripts/measure.py`'s `charge_model` holds the `reserve` figure's every
cell to (`unnamed ≤ MEMORY_RESERVE`), so the library and the harness are
predicting the same thing.

**That is conservative, and by a knowable amount.** 384 MiB was picked as the
*smallest* constant meeting the criterion on `19.16`'s grid, where the flat
term it covers measured 83.6–214.6 MiB. Predicting with 384 therefore refuses
counts the readings admit — the 1 GiB leg's eleven readers measured 20.2%
headroom and now resolve seven. A second calibrated constant for the flat term
(≈215 MiB) would resolve ten there, and was refused: it is a number picked off
a `runs/` probe in order to *loosen* a safety rule, on the phase's one axis
where being wrong kills the process. It is filed under STATUS's "Decisions
worth another look".

## What the sweep inherits

The `reserve` figure's flagless axis moves, and this is the arithmetic rather
than a prediction of the sitting — every count below is `min(recommendation,
fit)` before this host's 24-core clamp, and every budget is what that count
spends:

| leg | before | after |
|---|---|---|
| 24 MiB, `512m` | 1 (128.0 MiB, streaming) | unchanged |
| 24 MiB, `544m` | 1 (130.0, block) | unchanged |
| 24 MiB, `1g` | 11 (638.4) | **7 (406.2)** |
| 24 MiB, `1088m` | 12 (696.4) | **8 (464.3)** |
| 24 MiB, `1536m` | 19 (1102.6) | **14 (812.5)** |
| 24 MiB, `2g` | 28 → 24 clamped (1624.9) | **21 (1218.7)** |
| 128 MiB, `512m`/`544m`/`1g` | 1, streaming | unchanged |
| 128 MiB, `1088m` | 1 (650.0, block) | unchanged |
| 128 MiB, `1536m` | 4 (1064.1) | **2 (788.1)** |
| 128 MiB, `2g` | 6 (1596.2) | **4 (1064.1)** |

Three consequences for `19.11`:

- **`M89`'s two floor legs are untouched.** `544m` and `1088m` resolve one
  reader on the block path before and after, because the margin cannot take the
  last reader and the budget stays `cap`-bounded. The acceptance gate that the
  evaluated set bills a nonzero pool floor on each block size therefore still
  has its cells — and gains one, the 128 MiB `1536m` leg now resolving two
  readers and billing `2 × 128 MiB` of floor where it billed none.
- **`reserve_floor_problems` still passes and its reasoning is unchanged**,
  because both of its mirrors argue from `discovered_budget` — the cap — which
  the margin does not move. The margin only lowers the resolved count, which
  *raises* the pool floor, so the window stays a sufficient condition and the
  distinct-count comparison stays an upper bound. Both docstrings now say the
  margin is a third thing left out and why.
- **The margin is what the sitting will now be reading against.** Every
  flagless cell's headroom should come out above 20% by construction rather
  than by the grid's accident; a cell that does not is a finding about the
  predicted-resident model, not about the constant.

## What `19.25` inherits

`19.25` reports the **delivered** count against the announced one. The margin
adds a third way for the two to differ, and it differs from the other two in
where it is visible: the decline and `worker_count`'s division both happen
after the source is open, while the margin has already lowered the *recommended*
count before `ParallelArgs::resolve` returns — so the mode report's
`(recommended by the source; lowered from N by the allocation)` clause already
names it and `19.25` has nothing to add there. What `19.25` still owes is the
`.xz` `parse` that is serial whatever `--jobs` says.

## One document deliberately left alone

[`roadmap.md`](roadmap.md)'s "A default runs as fast as the allocation permits"
says that **where a limit is discovered the default fills it**. That was
already not an arithmetic claim — `MEMORY_RESERVE` has come off the top since
`19.13` and the rule does not mention it — and read as the directional
statement it is (fill a stated allocation; stay modest where nothing stated
one) the margin does not falsify it. It is also a standing rule, which an
unattended session does not amend; if a reading of it says otherwise, the
amendment belongs to a grilling and not to this row.

## What no longer needs saying

`MEMORY_RESERVE`'s "one thing it was not shown to do" — validated to 2 GiB on a
24-core host, predicted to breach on a wider one — is closed and its doc
comment is rewritten. The claim was never that the constant was wrong; it was
that the criterion held at that limit for a reason the constant did not supply.
