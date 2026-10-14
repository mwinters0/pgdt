# P36.1.1 notes — the count pair's reps

The instrument is `scripts/measure.py`'s `cmd_count_pair`, now counting each
shape `RELEASE_PROFILE_REPS` times a leg, and `count_pair_resolution` and
`count_pair_table` beside it; `CountPair` in `scripts/test_measure.py`
asserts them. Their docstrings say what each prints and why.

## The identical-source reading: no row resolves

`--count-pair HEAD~1` from `c4cf79f8`, which touched only `scripts/` and the
docs, the tree clean (`runs/count-pair-36.1.1-20261014-1805/log`, its
`HANDOFF.md` beside it). It exited 0 and printed **0 of 5 rows resolved**,
so the instrument stands and the hot-path slices (36.4, 36.5, 36.9) may be
counted with it (spec, "What a hot-path slice owes"). Every shape's runs
answered alike, both legs reported the `release` profile (the check is
silent unless it refuses) and both sit under the glibc floor.

**The legs differ where the falsification looks.** The two binaries are the
same size, differ in 26,890 bytes and carry distinct build ids, and
mimalloc's `__TIME__` reads 18:14:18 in the parent and 18:26:56 in the tree:
two layouts of one Rust source, as 36.1's notes describe. Both strings date
from 36.1's cold builds on 2026-10-13, since each leg's target directory kept
its compiled mimalloc. The tree leg rebuilt nothing (0.44 s), so it is 36.1's
tree binary; the parent relinked every workspace crate in a fresh worktree.

Each leg's spread, fewest to most, and the tree's median against the
parent's, in `instructions:u`:

| Row | parent | tree | median change |
|---|---|---|---|
| `parse`, metadata level | 30,114 | 85,677 | +2,057 |
| `parse`, data level | 769,886 | 62,662 | +51 |
| `query` strings | 94,254 | 139,182 | −47,538 |
| `query` typed | 176,907 | 148,177 | +65,453 |
| `pgdt sql` | 128,179 | 186,403 | +30,190 |

**36.1's change column was run-to-run, not layout.** Each of its five
single-count differences is smaller than both legs' spreads on its row
here, and on every row the two layouts' medians differ by less than either
leg's spread. At the data level each leg's other four reps fall within 1,489
(parent) and 718 (tree) of each other and the two clusters overlap, so no
layout effect shows there at that resolution either; 36.1's −12,752 there
is its parent's single count lying above this pair's parent range (below).

**A five-rep range does not bound another sitting's counts of the same
binary.** Three of 36.1's ten single counts lie outside this pair's range for
their leg: the parent's data level 11,732 above it, the parent's `strings`
35,971 below, and the tree's `pgdt sql` 27,856 below — and the tree is the
same file. The pair is sound because it counts both legs interleaved in one
sitting; a spread taken in another sitting is not its noise, as the spec
already says.

### The columns beside the verdict

- **The data level's rep 2 fell on both legs**, 768,397 below the parent's
  other four and 61,944 below the tree's. The two runs were consecutive.
  Unattributed: a per-thread count or a profile of a low rep against a high
  one would attribute it.
- **`pgdt sql` never reached its upper value.** All ten counts sit at the
  lower of the two `release-profile` shows
  ([`measurements.md`](measurements.md), "What link-time optimization buys
  the shipped build"), spread as 36.1's notes found within it. Why those
  runs reach the upper value and these did not is unattributed.
- **No `nerdctl` broken-pipe warning** after the parent's 9m 41s build, so
  they do not follow every build; what closes the pipe stays unattributed
  (36.1's notes).
- **The pair took 15.4 min**, of which the parent's build was 9m 41s and the
  tree's a no-op. The parent leg rebuilds every workspace crate on every
  pair, its worktree being new each time, so it costs one fat link however
  little changed.

## What the next round inherits

- **A hot-path pair is two fat links plus the counts**: the parent's as
  above, and the tree's over the crates the slice changed. It is launched
  detached, and the round after reads it.
- **A rise resolves only with every tree rep above the parent's most.** One
  low tree rep, like the data level's rep 2 here, hides any rise smaller
  than its fall; a low parent rep hides nothing. On the data level, 36.4's
  row, that is the excursion a rise must clear.
- **One test of resolution, shared.** `resolved_above` is the fewest above
  the most, and `downstream_verdict` reads both its gaps through it, so the
  pair's rule and the register's cannot drift apart.
- **Each row says the direction** — a rise, a fall, or unresolved. A slice
  is refused on a resolved rise alone; the identical pair on either.
- **The pair runs both legs each rep**, the first leg reversed for the
  second half of the reps, as `_sweep_one` orders a figure's, and refuses a
  shape any of whose runs answers differently from another, not only across
  legs.
- **`--reps` does not reach the pair.** `main` builds its `Config` from
  `--dry-run` alone, so the pair's spread is always `release-profile`'s
  rep count, the comparison the spec names.
- **A reading is taken from a commit**, never with `--count-pair HEAD` over
  a tree carrying its own uncommitted apparatus
  ([`measurements.md`](measurements.md), "A figure may be published outside
  the sweep").
