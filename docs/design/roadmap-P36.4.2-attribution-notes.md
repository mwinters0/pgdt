# P36.4.2 notes — attributing 36.4's count rise

No code landed. The job is `runs/attr-36.4.2-20261015-2200/`. Its
`HANDOFF.md` describes it, `run.py` and `lines.py` are the instrument, and
`log`, `counts.json` and `diff.txt` are what it printed. The four binaries are
kept under `/mnt/ssd/fedora/scratch/pgdump_query/attr-36.4.2/`, and
`diff.txt` resolves against them. `codediff.py` and `codediff.txt` were added
when the result was read. They compare the shipped pair's machine code symbol
by symbol, which needs no run.

**The rise is unattributed.** The profiling build is a different build from
the one whose count rose, so its line profile does not explain the rise. The
shipped pair reproduced 36.4's rise: +493,301,876 `instructions:u` on the
data-level `parse`, where 36.4's own pair gave +493,303,174.

## The gate: line tables move the code

Each `profiling` build was counted against its own commit's shipped build,
five reps a leg. A resolved row falsified line tables as codegen-neutral. The
median change, profiling minus shipped:

| Row | `a7b6ef0e` | `0588c9e8` |
|---|---|---|
| `parse`, metadata level | −51,350, resolved | −49,694, unresolved |
| `parse`, data level | +8,471,689, resolved | +8,473,310, resolved |
| `query` strings | +2,179,377, resolved | +2,180,869, resolved |
| `query` typed | +15,554,180, resolved | +15,450,506, resolved |
| `pgdt sql` | +4,290,755, resolved | +4,333,529, resolved |

The gate is falsified at both commits, so `diff.txt` describes the profiling
builds. Read on its own, it puts most of their rise in the gatherer:
`observe_row` +399 samples a rep and `GroupState::heap_bytes` +175. It also
puts +520 in unchanged decode symbols. The shipped profile does not agree
symbol by symbol: `observe_row` moves −28 there, in the same units.

**The two pairs' totals agree, and that does not rescue the profile.** The
profiling pair rises +493,303,497 on the data-level `parse`, 1,621 more than
the shipped pair. But agreeing totals say nothing about where the
instructions sit (`.claude/skills/evidence/SKILL.md`, rule 3), and the gate
was written in advance to refuse exactly this reading.

## What the code says, with no new run

- **36.4's suspect is refuted for every pgdt symbol it named.**
  `from_utf8`, `unescape_field`, `unescape_owned`, `parse_time_of_day` and
  `civil_days`, plus memchr's `find_avx2` and `Canonical::of`, have the same
  machine code in both shipped builds, apart from absolute addresses. The
  same holds in both profiling builds, and between the shipped and profiling
  build at either commit. No changed inlining reached them, and they are
  called the same way at both commits. No caller of `unescape_field` is in
  36.4's diff. The key code receives text that is already unescaped.
  `field_key`, which calls the datetime and numeric decoders, moved to
  `key/order.rs` without a line changed.
- **The statistics account is byte-identical at both commits.** Every
  profiled run logged `held bytes=11884774 peak_bytes=12356065`, so gathering
  made the same decisions about what to keep in both.
- **Of the shipped profile's +478 samples, +410 are in symbols whose code is
  identical** (`codediff.txt`). The symbols whose code changed net about +119.
  The largest is `GroupState::heap_bytes` at +142, and the key path's renamed
  symbols net close to zero. Identical code running on the same input retires
  the same instructions, with one exception: a path that depends on an
  address, such as the unaligned head in memchr or in UTF-8 validation. So
  the +410 is either sampling error or a change in data layout, and no
  profile taken here can tell which.
- **A rep of `perf record -c` does not give an independent sample.** This
  run is deterministic, so every rep lands on the same sampling grid: the
  totals were 77,211, 77,210 and 77,211 samples, then 78,196, 78,194 and
  78,200. A rebuild moves the grid, and the rep spread never sees that error,
  so a key marked `resolved` across three reps is resolved only against the
  rep spread. As an example, `observe_row` has the same code in the shipped
  and profiling parent builds and ran on the same input, yet it reads 9,086
  samples in one profile (the shipped count doubled to this period) and 8,786
  in the other.

## What would attribute it

An exact per-function count of the shipped pair: callgrind's `Ir` on the
data-level `parse`, run on the two kept `pgdt-shipped-*` binaries. That count
involves no sampling grid and no rebuild. Its gate is that the pair's total
`Ir` difference matches the native +493.3 M. Under valgrind the program runs
at different addresses, so a total that does not match is pricing a
different layout. Nobody has timed a callgrind run of this shape; one that
does not fit under ten minutes is launched detached.

## What 36.4's refusal becomes

The refusal stands. 36.4 stays unticked and its rise is unattributed.
36.4.3 takes the exact count below, and the spec's "What a hot-path slice
owes" says what its reading leads to.

## What the next round inherits

- **A sampled profile cannot attribute a count-pair rise of this size,**
  whether it samples the profiling build (different code) or the shipped
  build (one sampling grid per build). An exact count of the shipped pair
  can.
- **Code identity is checked before any reasoning about inlining.**
  `codediff.py` reads two kept binaries and runs nothing, and here it
  refuted the suspect a whole spike was admitted to test.
- **The pair's binaries are copied out of `runs/pgdt-count-pair-*`,** which
  the next `--count-pair` overwrites. The copy is in the scratch directory
  above.
