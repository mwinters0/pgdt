# P7.8.1 — the I/O defaults

What the next slice inherits: **all three I/O-defaults levers are decided, and
none of them landed a scheme.** The chunk size stays at 1 MiB, measured rather
than assumed; double-buffered readahead and `posix_fadvise` are refused against
7.8's arithmetic and this slice's own table. What did land is the instrument
that decided the first of the three — `pgdq --chunk-size`, and the `chunk-size`
figure over it — plus one fact about the read path that no figure had shown
before: the buffer pool's ceiling is a cliff, not a taper.

The figure itself is filed by subject
([`measurements.md`](measurements.md), "What the read chunk size is worth");
this doc holds the apparatus, the two refusals and what they rest on.

## Module map

| File | What it is |
|---|---|
| `pgdump_query/src/scan.rs`, `DEFAULT_CHUNK_SIZE` | the constant, named and exported — what the figure's rows are read against |
| `pgdump_query-cli/src/main.rs`, `parse_chunk_size` | `--chunk-size`, on `parse` and `query`; zero refused at the parser |
| `pgdump_query-cli/src/main.rs`, `scan_options` | the one place the flag becomes `ScanOptions`, so both commands cannot drift |
| `pgdump_query-cli/tests/chunk_size.rs` | three: the rows and the listing are the same at every size, and zero is refused |
| `scripts/measure.py`, `CHUNK_SIZES` / `CHUNK_DEFAULT` | six sizes bracketing the read path's pool ceiling, and the row the ratios are against |
| `scripts/measure.py`, `CHUNK_REGIMES` | three regimes in one table — the class that can win, and the two that say what changing the default would cost |
| `scripts/measure.py`, `run_chunk_size` | the figure: 18 specs, nine reps |
| `scripts/test_measure.py`, `ChunkSize` | seven, every one aimed at a table that formats perfectly while every row measured the default |

## The instrument is a flag, not a build per size

`ScanOptions::chunk_size` was a library field the CLI never passed, so the size
could only be varied by building a binary per value — a worktree, a patched
constant, and a build cache, on the `pgdq-nocensus` and `pgdq-before-throttle`
precedents. A flag is the cheaper instrument: the value is in the recorded
command line of every rep, where the harness's own `raw.json` carries it,
rather than in a build step.

**It is not the better-provenanced one, and the first version of this
paragraph said it was.** The failure mode a build per size is supposed to have
— six identical binaries and a beautifully flat table — is one this project has
already closed twice. The harness builds `pgdq-before-throttle` itself, in a
temporary git worktree, and the `allocator` figure's three legs are
interrogated with `pgdq --version` before being timed, precisely so that a leg
that did not take cannot masquerade as a null result. A chunk-size constant
could have been carried the same way. What the flag actually saved is the step
that would have made the two equal: teaching `--version` to report the compiled
chunk size. That is a real saving and a small one. **So the flag is not
justified as a necessary instrument, and stands or falls on what it is worth to
an operator.**

It also discharges a sentence the spec already carried. "`ScanOptions::chunk_size`
stays tunable and the per-device-class numbers are published, so an operator
can pick" was not true of anyone using the CLI, which is every user this
project has. That the slice added a knob in order to do the measuring, against
a spec bullet saying the work was "measuring the right defaults per device
class rather than adding a knob", was reviewed and the flag was kept — on
operator value and on the figure, not on the instrument argument above.
The reasoning is beside the mechanism
([`architecture.md`](architecture.md), "Execution model and API surface") and
the evidence is in [2026-09-04](../status/history/2026-09-04.md), "The
chunk-size flag is reviewed and kept, and the cliff behind it is a proxy".

*Rejected: an environment variable.* It is the same surface with none of the
discoverability — `--help` would not carry it, so the only people who could
tune the read path would be the ones who had read this file.

*Rejected: sweeping chunk sizes in a criterion bench instead.* `ScanOptions` is
public, so a bench could vary the field with no CLI surface at all — but a
bench is warm by construction, and the whole question is what a chunk size does
to a cold read. It would have answered the one regime whose answer nobody
needed.

## The pool ceiling is a cliff, and the sweep is what shows it

`io::POOL_MAX_BYTES` is 8 MiB, so a chunk larger than that is refused by
`BufferPool::give` and every chunk becomes a fresh `vec![0u8; len]` — the
`calloc` that 7.13 removed, back again, once per chunk. The sweep brackets that
boundary deliberately (8 MiB and 16 MiB rows), which is why the table can state
it rather than leave a reader to assume the curve continues.

**It is not a property of the read path, and the first version of this
paragraph called it one.** `POOL_MAX_BYTES`' own doc comment says what it is
for: `attach_text`'s coalesced span read, which "happens once per map and never
again", and which would trade the flat ~9 MiB RSS this design is built around
"for an allocation nothing is going to ask for twice". A chunk buffer at the
configured size is the exact opposite — asked for once per chunk, for the whole
scan. **The pool filters by size as a proxy for one-off-ness**, and a
deliberately large chunk buffer is caught as collateral: a caller who asked for
16 MiB has already accepted 16 MiB of buffer, and gets the memory *and* the
`calloc`, which is the worst of both.

Calling it a property is what let the remedy be a sentence in `--help` rather
than a fix, and "the remedy is already the user's" is circular — the hazard is
the user's only because the flag put it there. What is true, and is what the
docs should say, is narrower: **the ceiling is a size proxy the chunk buffer
can walk off, the remedy today is not to set `--chunk-size` above 8 MiB, and a
pool that kept a buffer whose length is the configured `chunk_size` would
remove the cliff at a bounded, honest cost** — `POOL_SLOTS` is 4, so a 16 MiB
chunk could retain up to 64 MiB against the ~9 MiB this design advertises.
That trade is out-of-band work — it moves no published number, since every
figure is taken at the default — and is admitted as `M55`
([`roadmap.md`](roadmap.md)'s out-of-band ledger), with `Blocks` empty.

Until it lands, every description states the behaviour as built, and the
remedy in `--help` is still the real one. The characterization is filed in
[`architecture.md`](architecture.md), "Execution model and API surface", and in
`POOL_MAX_BYTES`' own doc comment, which is where a session would meet it.

## Nine reps, because five could not have resolved this

The cold-NVMe throughput table spreads 1.285–1.499 s over five reps — about 15%
of its median, against a total envelope for all three levers of 8.9%. A
five-rep chunk-size table would have been unable to tell *flat* from
*unresolved*, and "we measured and saw nothing" is only an argument when the
instrument could have seen something.

## The reading, and why it decides all three

Nine reps, taken alone on 2026-09-04, `drop_caches` before every cold run. The
whole table is in [`measurements.md`](measurements.md); three readings out of it
are what the decisions rest on.

**1 MiB is the fastest row, and its two neighbours are ties.** Cold on the
NVMe, 256 KiB (1.03×) and 4 MiB (1.05×) are inside the reps' own spread of the
default, and everything further out is clearly slower — 1.18× at 64 KiB, 1.15×
at 8 MiB, 1.37× at 16 MiB, none of whose spreads reach the default's. Cold on
the SATA SSD every row is 1.00×. So the lever is worth **nothing**, not "at
most 8.9%": nothing beats what ships, and what comes close is indistinguishable
from it. The one cell below the default anywhere is 8 MiB warm, at 0.97×,
which is 3% of a regime that has no device in it and 15% the wrong way on the
regime that does.

**A deeper read is not a faster one.** The 16 MiB row is a deeper prefetch than
`POSIX_FADV_SEQUENTIAL` would arrange, issued while the parser is idle, and it
is the slowest row cold on the NVMe. Nothing in this table is limited by how
deeply the device is being asked to read ahead, which is the direct measurement
`fadvise` needed and 7.8 could not supply.

**The pool ceiling is worth 2.07× warm.** 0.386 s at 8 MiB against 0.825 s at
16 MiB, on the same file and the same binary: above `io::POOL_MAX_BYTES` the
buffer is not kept, so 7.13's `calloc` returns once per chunk. That is the
largest single effect in the table and it is not about the chunk size at all.

## What settles the other two levers

Neither `posix_fadvise(SEQUENTIAL)` nor double-buffered readahead is
implemented, and neither is left as an open question.

**Both are bounded by the same 8.9%** — 7.8's arithmetic: no scheme that
overlaps I/O with parsing can put a cold scan below the time the device takes
to deliver the bytes, and on the fastest device this project owns a cold `COPY`
scan exceeds that floor by 0.125 s of 1.406 s. On the SATA SSD the same
subtraction is ~1% and on the HDD the scan is device-bound by a factor of
several. That is the ceiling on the two schemes *together*, not each.

**The chunk sweep is also a readahead-depth experiment**, and that is the part
7.8 could not do. If cold time were limited by how deeply the device was being
asked to read ahead, the large chunks would be faster than the small ones; they
are slower, monotonically, from 1 MiB up. So there is no depth left to buy, and
a hint that asks for more depth is asking for the thing the table says costs.

**`posix_fadvise` would also cost a dependency.** `pgdump_query` has no direct
`libc`/`rustix` dependency, and the call cannot be made without one — so the
lever is not the six lines it looks like from the spec's bullet. Adding a
platform crate to L1 for a hint the measurement says buys nothing is the trade
that decides it.

**Double-buffered readahead costs more than a dependency.** It is a rework of
the three read loops 7.13.1 has just reworked, a second in-flight buffer
against a design built on flat ~9 MiB RSS, and a second thing for the interrupt
guard and the query path's chunk retention to be correct about — for a prize
bounded at 8.9% on one device class and ~0 on the two the project's own sample
dumps live on. 7.8's notes already recorded that most of the parse CPU is
hidden behind the read on that device; overlapping harder cannot recover what
is already overlapped.

## What is left open

**Nothing in this slice measured a device faster than the 970 EVO Plus.** The
8.9% ceiling is a fact about the fastest disk we own, and a device on which
parse CPU exceeded read time would reopen both schemes at once. That is not a
deficiency and has no owner: it is a bound stated with its apparatus, and the
thing that would promote it is hardware, not a decision.

**Removing the flag would delete the figure with it.** The entry under
`STATUS.md`'s "Decisions worth another look" prices removal as costing the
`chunk-size` figure its command shape "and nothing else". That understates it:
`CLAUDE.md`'s standing rule is that a figure whose regeneration command is gone
should be deleted rather than kept, and the flag *is* that command. So removal
deletes the published evidence that 1 MiB is right and that the lever is worth
nothing — evidence `DEFAULT_CHUNK_SIZE`'s doc comment and the manual both cite,
and which cost a six-size, three-regime, nine-rep sitting to get. That is the
decisive cost on that side, and the entry did not name it.
