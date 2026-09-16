# P20.6 — `--memory`, the resident allowance

What 20.7 inherits, and what this slice found that the spec did not anticipate.
The decision is [`decisions.md`](decisions.md), "D83"; the rule it implements is
[`roadmap.md`](roadmap.md), "Two tunables fit pgdq to hardware: memory and
parallelism".

## What the next slice inherits

**One carving, one function.** `Parallelism::within(jobs, memory, allowance)` is
the whole of it: reserve off the top for the cap, `margin_allowance` for the
count's ceiling, `fit`, `workers`. `discover_in` calls it with the discovered
limit and `Discovered::resolve` calls it with `--memory`, so "a stated one is
treated exactly as a discovered limit" is a shared call rather than two
implementations agreeing. `Parallelism::recommended_within` — the old
stated-budget path, which took the bytes whole and applied no margin — is gone.

**`Parallelism` still carries the resolved pool budget, not the allowance.** The
allowance reaches the type and is consumed there; nothing downstream can recover
it. 20.7 needs the allowance to carve the statistics' share from, and the CLI has
it in `Resolved::allowance_stated` beside `Resolved::limit`, which is where a
decline resolved CLI-side can read it. **The library has no such carrier**, so a
library decline — "stating none declines nothing", the spec's
"Memory means resident, and is one flag" — needs one added, and that is a
resolver touch 20.7 owns.

**Provenance still never enters `Parallelism`** (D64). What changed is that the
CLI's `Resolved` now keeps the stated number rather than a `bool`, because the
budget printed is no longer the number typed and the status line has to carry
both.

## Negative results, and what moved that the spec did not say

**A stated allowance cannot raise a plain dump's read buffers.** `fit` with no
`WorkerMemory` returns `DEFAULT_MEMORY_BUDGET.min(cap)`, and `LocalFileSource`
recommends none (`KD25`). So `--memory 8g` on a plain dump leaves the pools on
the 64 MiB constant, where `--parallel-memory 8g` gave them 8 GiB. It follows
from "treated exactly as a discovered limit" — a discovered limit already
behaved this way — but it retires a recourse the manual used to name: **a plain
`query` now plans one sub-stream at every `--jobs`**, the 64 MiB batch span each
one is charged already exceeding the 64 MiB budget, and no flag reaches that
span. The manual says so now; `--chunk-size` is what is left for a plain read's
buffers. Two legs of `pgdump_query-cli/tests/statistics.rs` were exercising a
three-sub-stream plain `query` through `--parallel-memory 1g` and now run one;
their comments say so, and the split stays covered at the library, where
`pgdump_query/tests/pruning.rs` hands `Parallelism::workers` a pool budget
directly. Filed under STATUS's "Decisions worth another look".

**The margin now binds a typed number, and that moves figures.** A stated budget
used to carry no ceiling; an allowance does. The crossover is a budget of
`4 × MEMORY_RESERVE − 5 × MEMORY_UNPOOLED_BOUND` = 256 MiB, above which
`margin_allowance` is tighter than the cap, so no allowance reproduces an old
stated arrangement exactly above that. `scripts/measure.py`'s
`stated_allowance(budget)` states `budget + MEMORY_RESERVE` so every registered
figure still names the buffer budget it was registered for; the legs above
256 MiB resolve fewer readers than their published cells were taken at.
`RESERVE_BUDGETS`' 512 MiB leg and both `parallel-*` figures are in that band,
and the `parallel-*` plain legs are hit by the paragraph above as well.
**20.9 re-takes them** — its row already says "every figure whose arrangement
moves with the reserve", and after this slice that is every figure that states
memory at all. Nothing was re-taken here.

**`MEMORY_RESERVE` did not move**, as the row requires: 384 MiB, 20.8's to set.

## The allowlist check

`every_numeric_flag_is_classified` in `pgdump_query-cli/src/main.rs` walks
`Cli::command()` and every subcommand, keeps the value-taking options, and
closes three ways: a value name in neither the numeric nor the non-numeric list
fails, a numeric flag with no classification fails, and a classification naming
no flag fails. The fourth assertion is the rule's own content — the hardware
class is exactly `{jobs, memory}`, so a third hardware knob fails here before
anyone argues about it. A `SetTrue` switch is skipped because `clap` gives every
argument a value name, the flag's own spelling upper-cased where none was
stated, so booleans would otherwise flood the first list.
