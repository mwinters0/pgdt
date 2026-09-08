# P16.2 — `KD14`'s attribution, and `peak-rss` re-taken

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md).

## What exists now

- **[`measurements.md`](measurements.md), "What a scan holds resident, per byte
  and per block"** — the `peak-rss` figure, re-taken and published with
  `41c96bb` inside its own marker.
- **[`measurements.md`](measurements.md), "What the per-block resident growth is
  made of"** — a section *outside* the register, declared in
  `measure.NOT_OURS`, carrying the attribution's nine legs at two block counts.
- **`measure.run_rss_attribution`** — the instrument that takes them, and the
  `rss-attribution` figure it belongs to. The nine legs landed as a standalone
  `scripts/rss_attribution.py` importing `measure`'s apparatus; `M65` folded
  them into the harness itself, where the figure waits in `measure.UNTAKEN`
  until a sweep publishes its table.

No library code, as the spec's row says.

## The findings

**The re-take moved nothing.** 5.90 / 5.85 / 9.73 / 43.78 MiB against the
`7ee5db5` sitting's 5.88 / 5.64 / 9.53 / 43.59, every one inside the other's
spread — after three rounds of read-path, compressed-source and cache work that
were all genuinely under the figure's declared paths. The staleness was real and
the reading was not wrong; that is a useful data point about what `depends`
edges buy, which is a *prompt to re-take*, not a prediction.

**The growth is mostly none of the three mechanisms `KD14` named.** Every leg is
taken at 500 and 4,000 blocks, so each reading is a slope rather than an
absolute with a fixed baseline folded in:

- **Live structure per table: +6,002 B a block.** `parse --preamble-only` stops
  at the end of the schema section, before a data block is read, and already
  carries three fifths of the full `parse`'s +10,390. An `info` over the finished
  cache — an index deserialized rather than built — costs +5,722, agreeing to
  within 5% by an entirely different route.
- **The whole-list clone: one to two kilobytes a block.** `query --dqcache none`
  against its cached twin, differing in that flag alone. Real, and a fifth of
  the growth rather than its cause. It is the noisiest leg here — two sittings
  an hour apart read +1,303 and +2,278 where every other leg moved by under 3%
  — so it is published as a range.
- **The allocator: not it.** Under the shipped shape glibc has the *lowest*
  slope of three — +10,390 against mimalloc's +11,415 and jemalloc's +13,703 —
  so an allocator that returns more does not shrink it. Only with the throttle
  off does retention show at all, and there mimalloc recovers ~4 KB a block.
- **The remainder, ~4.5 KB a block**, is the scan's own working set over what
  holding the finished index costs.

**What that means for the rest of the phase.** The dominant term is per *table*
and is paid by the preamble, which the leader parses once before any worker
exists — so N workers do not multiply it. What they multiply is the block-sized
decode slot, which 16.4 budgets deliberately. A memory bound stated for the
parallel scan is therefore `preamble + N × slot`, not `N × (what one scan
holds)`.

**jemalloc's baseline is 118 MiB where glibc's is 9.65**, at 500 blocks. It says
nothing about this project — arena reservation is resident by design — but it is
worth knowing before anyone reads the `allocator` figure, which is a *timing*
table, as saying anything about memory.

## The calls worth knowing about

**The attribution was landed as a diagnostic, not a registered figure**, and
that is the one call here a reviewer weighed — the review reversed it, and
`M65` ([`roadmap.md`](roadmap.md), "Out-of-band work") is folding it into the
register. Three things argued for it. It answers *which of several mechanisms* — a
proportion, read once, not a number the design quotes and differences. Its legs
are diagnostic ones no sweep would take: two more allocators, a scan stopped at
the preamble, an index merely loaded. And a resident set is not a timing, so
none of the apparatus a figure exists to control is doing anything for it. That
is the standing the profiling recipe already has.

**The spec's pairing does not name two runnable commands, and the reason is a
refusal.** It says "`query --dqcache none` against `query-nomatch`", but
`query-nomatch` *is* a `query --dqcache none`. What the harness turns out to
have is one RSS-carrying shape (`parse-rss`), and `parse` **refuses
`--dqcache none` outright** — "cache is disabled, but `parse` requires a cache
file" — so the un-throttled splice is reachable only through `query`. The pair
that actually isolates the clone is therefore `query --table <no match>` with a
cache path against the same command with `--dqcache none`: one flag apart, and
both of them `query`.

**Two block counts, not one.** A single absolute at 4,000 blocks folds in a
baseline that differs by 110 MiB between allocators; jemalloc would have read as
catastrophic and mimalloc as bad, when their *slopes* are within 35% of glibc's.
Every leg is taken at 500 and 4,000 and reported as a per-block difference for
that reason.

**The `info` leg needs a cache the container can read**, so the runner builds
one per input into the mounted `/out` (a tmpfs directory) before the reps start,
with the same binary. It is outside the timed legs and outside the reps.

**`rss_wrapper` already ends in the `--` that terminates perl's option
parsing.** Appending another one makes `--` the program perl `exec`s, which
fails as `exec: No such file or directory` and looks exactly like a missing
mount.

## Artifacts

- `runs/measure-20260907T034812/` — the published `peak-rss` sitting's
  `tables.md` and `raw.json`.
- `runs/pgdq-alloc-{jemalloc,mimalloc}` — rebuilt at `41c96bb` for the two
  allocator legs, by the same recipe `measure.ensure_allocator_binary` uses,
  which is what the folded-in figure now calls: which binary a leg is is the
  `allocator` figure's rule and not this one's to duplicate.
- `/dev/shm/pgdq-rss/` — the staging directory the standalone script used, its
  own rather than the sweep's `warm_dir`. The folded-in figure stages through
  the sweep's own directory like every other, and its `info` leg builds its
  cache inside its own container instead.
