# P19.19 — the per-file term, billed where the file is open

What the next slices of `P19` inherit from the repair. How the mechanism works
now is beside it —
[`architecture.md`](architecture.md), "Execution model and API surface" and "The
compressed source"; this doc holds what those sections do not say and the next
reading needs.

## What moved

Two edits, one arithmetic.

**`Partitioning::window_end` is the cut size, and it is no longer
`partition_bytes`.** A boundaried source's window now ends at the `workers`-th
boundary strictly past the frontier, which leaves `workers - 1` boundaries
inside it — so `stream::cut` takes every one instead of thinning, and every
piece lies inside a single block. `PartitionBoundaries::Anywhere` keeps the old
arithmetic exactly (`start + workers × partition_bytes`, clamped), so **nothing
on the plain path moved**, byte for byte.

**`XzSource::charged_chunk_bytes` is the chunk term in every charge this source
states.** It is the length a read loop announced, or `DEFAULT_CHUNK_SIZE` where
none has, replacing `BufferPool::slot_bytes()`'s `POOL_MAX_BYTES` fallback at
three call sites: `block_reader_bytes` (hence `default_memory_per_worker` and
`block_decode_bytes`), `block_path`'s affordability gate, and `partitions()`.
Only the first of those is ever asked before a read loop exists, so the
behavioural change is exactly the resolve-time recommendation: 68,192,032 →
60,852,000 for a 24 MiB-block file.

## Two things a later slice must not re-derive

**The window's first piece is not a stub.** It looked like one: a window
`[frontier, b_want)` gives pieces `[frontier, b_1)`, `[b_1, b_2)`, …, and the
first covers only the remainder of the block the frontier is in. But the
previous window's last piece read *past* its own limit to finish a row, so the
frontier sits a few hundred bytes into a block rather than most of the way
through it, and the first piece is a whole block less epsilon. The windows tile
the region at `workers` blocks apiece.

**Do not size a window at `workers × unit`.** It is the obvious form and it is
off by one: from a mid-block frontier that window contains `workers` boundaries,
`cut` is asked for `workers` pieces, and the thinning drops the first — which
puts two blocks in the first piece and reintroduces exactly the defect. The
window has to be defined by the boundary list, not by a byte count. This is what
`a_block_decoding_partition_never_crosses_a_block_boundary` walks a *mid-block*
frontier for; an aligned start passes under either rule.

## What the readings say, and what they hand forward

Both probes are `runs/` artifacts, not figures, and no document may quote them
as measurements.

**`runs/19.19-flagless-acceptance-probe.py`** — the acceptance gate. Both
compressed inputs, flagless, at the four registered container limits, three
reps, with the kill oracle read out of the container's own
`/sys/fs/cgroup/memory.events` rather than off an exit code the `rss_wrapper`
collapses. Every leg passed: nothing failed and nothing was killed.

| limit | 24 MiB blocks | 128 MiB blocks |
|---|---|---|
| 512m | 4 readers, worst 357.6 MiB (30.1% left) | declines the block path, 14.9 MiB |
| 1g | 13 readers, worst 905.2 MiB (11.6% left) | 2 readers, worst 802.1 MiB (21.7% left) |
| 1536m | 22 readers, worst 1377.9 MiB (10.3% left) | 4 readers, worst 1076.8 MiB (29.9% left) |
| 2g | 24 readers, worst 1467.5 MiB (28.3% left) | 6 readers, worst 1607.6 MiB (21.5% left) |

**The thin allocation has moved, and `19.16` picks its constant against the new
shape.** `19.15` read 512 MiB as the thin point — 0.6% of the limit left at the
worst of thirteen reps. It now leaves **30.1%**, and the tightest cells are
1 GiB (11.6%) and 1.5 GiB (10.3%), which are the ones the stated criterion —
worst rep leaves ≥20% — still fails. The failure has moved from the smallest
allocation that reaches the block path to the middle of the range, so a
candidate reserve chosen from `19.15`'s curve is aimed at the wrong end. Note
also that the criterion is now failed by *three* reps rather than thirteen; a
worst-rep gate wants `19.15`'s rep count before it is called.

**The resolved counts are arithmetic, not a fit.** 512 MiB gives cap
`536,870,912 − 268,435,456 = 268,435,456`, and `268,435,456 / 60,852,000 = 4.41`
→ four readers, where `/ 68,192,032 = 3.94` → three. Every count in the table
above is that division; none of them needs a model.

**`runs/19.18-blocksize-charge-probe.py`** — re-run on the repaired build; the
pre-repair sitting is kept beside it as
`runs/19.18-blocksize-charge-probe.pre-19.19.{log,json}`, since the script
overwrites its own output. Resident fell at every cell above one reader: 24 MiB blocks 331.8 →
251.8 / 499.5 → 319.7 / 566.7 → 459.7 MiB at two / four / six readers, and
128 MiB blocks 1322.8 → 801.7 / 1826.4 → 1076.9 / 2039.3 → 1605.9. At 128 MiB
the charge now **covers** what a reader holds (1.09×) where it covered 0.83×.

## The column nobody was studying: wall clock

Read this before treating the repair as a pure win.

At *stated* low reader counts the scan got **slower**: the 24 MiB control runs
17.8 / 17.9 / 9.9 / 7.4 s at one / two / four / six readers, against
16.9 / 11.2 / 8.9 / 8.9 before. Two readers now buy nothing at all. What they
buy at six is new — the pre-repair build was flat from four, which is the
ceiling the acceptance asked to see lifted, and it is lifted.

The term is named rather than left as a residual: **`KD20`**, each worker
decoding its successor's block for its chunk-sized tail read, with no in-flight
map to make the two misses one decode. At `w` workers a window advances `w`
blocks in the time of two block decodes, so the speedup is about `w/2` — which
is 1.0 at two readers, and matches 1.8 measured at four and 2.4 at six. It is
not a regression the repair introduced so much as one it restored: the
arrangement before `19.14` did the same thing, and the published
`parallel-scan-throughput` leg (5.82× at twenty-four workers) was taken under
it.

**At the default — which is what the phase is about — it is a large win**,
because a flagless run resolves many readers rather than two: against `19.15`'s
flagless legs the same shapes run 10.12 → 4.66 s at 1 GiB and 9.90 → 3.78 s at
1.5 GiB, roughly 2.1× and 2.6×, with the count up from 11 → 13 and 19 → 22.

## For `19.17.1`

Its premise is worth re-testing before it is built. It exists to record a killed
leg and continue; the repair may leave no leg that dies — the 128 MiB family,
the one that could previously neither survive nor decline, now does one or the
other at every registered limit with at least 21.5% of the allocation left. Its
own row already says the question is for the session after `19.19`; this is the
answer that session needs.

## For `19.18`

The three mechanism legs were already dead (the `POOL_DEPTH` floor is refuted
arithmetically). The other axes are now measuring a different arrangement: the
per-reader term fell by a third at 24 MiB blocks and by a quarter at 128, and
the fixed term with it. Any candidate carried over from the pre-repair fits is
stale, including the 256 MiB constant the code currently ships.
