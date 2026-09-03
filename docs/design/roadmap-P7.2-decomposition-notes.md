# P7.2 — The decomposition, published

What the rest of P7 inherits: a cost decomposition that is **filed by subject,
not here** — [`architecture.md`](architecture.md), "Where a scan's time goes" —
plus the three things this slice had to settle before that section could be
written honestly. No library code changed and no figure was taken.

Read this doc for the apparatus and for what the numbers are *not*; read the
architecture section for what they are.

## Module map

| File | What it is |
|---|---|
| [`architecture.md`](architecture.md), "Where a scan's time goes" | the deliverable: `parse`, `strings`, `typed`, the `INSERT` path, the double read, `attach_text` |
| [`architecture.md`](architecture.md), "Bulk regions" | the `INSERT` fast path's layer, settled, and the +10% attribution |
| [`measurements.md`](measurements.md), "Two builds of one source can differ by layout" | the standing rule the attribution produced |
| `runs/profile-parse-insert_run-{after,before}.{data,txt}` | the differential pair, frame-pointer build |
| `runs/profile-parse-insert_run-release-{after,before}.{data,txt}` | the same pair, `release` build, flat |
| `runs/profile-parse-blocks4000.{data,txt}` | 4000 blocks — `KD5` and `attach_text` |
| `runs/profile-release-{parse,strings,typed}-control.{data,txt}` | the measured build's flat profiles of the three control shapes |
| `runs/pgdq-b70589f-{release,profiling}` | the "before" binaries, built in a throwaway worktree |

## The one correction that invalidates every earlier reading of these profiles

**`perf_event_paranoid = 2` samples user space only.** The event is
`cpu/cycles/Pu`, so a profile's 100% is the process's *user* time. A warm
3.00 GiB `parse` is 0.60 s of wall made of 0.30 s system and 0.29 s user, and
`dd` over the same file is 0.29 s that is entirely system — so a `parse`
profile describes slightly under half the scan, and every share in it must be
multiplied by the user time before it can be set beside a `measurements.md`
figure.

That is not a caveat, it is the reconciliation. `memchr::Two` reads 17.2% of
the `parse` profile, which sounds like a third of the census's own figure being
missing; times 0.29 s it is 0.050 s, against the +0.030 s the census-on/off
subtraction measures. Uncorrected it looks like a contradiction; corrected it is
agreement, and the same correction is what keeps 7.1's "nearly half of a warm
`parse` is `memmove` plus `memset`" from being read as half the wall.

## Three things about the instrument, all of which cost a wrong answer

**The `profiling` build is not the `release` build, and on one input the sign
flips.** `insert_run` at the two commits: `release` puts the newer binary 10.8%
slower, the frame-pointer build puts it 3.8% *faster*. So a profile is read for
proportions and never for a wall-time comparison between two builds — which is
what the phase already said, now with the counterexample. The *proportions*
travel fine: the `release` flat profiles taken here agree with 7.1's
frame-pointer ones to a few points on every bucket.

**A share is not a removable cost.** `memchr::Two` is 17.2% of the `parse`
profile and removing it saves 0.030 s of a 0.60 s scan. Cycles attributed to a
pass that overlaps with others in an out-of-order core do not all disappear when
the pass does. Where a subtraction exists, the subtraction is the answer to
"what would this save"; the profile answers "where does the time sit".

**`--children` needs the call graph, and the call graph needs frame pointers.**
So the structure in the architecture section (`poll_next` against
`print_batch`) comes from the `profiling` build and the proportions from
`release`. The two are cross-checked bucket by bucket rather than assumed
compatible.

## The `INSERT` +10%: settled, and it is not a code change

The spec asked for a differential profile and said that two indistinguishable
profiles would themselves be the answer. They were nearly indistinguishable,
and `perf stat` is what turned that into an attribution:

| `release`, warm, 3.00 GiB `insert_run` | `b70589f` | `ba2fc12`..`HEAD` |
|---|---|---|
| instructions | 114,619,683,721 | 114,655,786,452 |
| cycles | 35,787,121,684 | 39,969,651,317 |
| wall (median of 5) | 8.51 s | 9.43 s |

Same instructions to 0.03%; +11.7% cycles. Branch misses, cache misses,
L1-icache misses and frontend stalls are flat or *lower* on the slower binary,
and `cycles / task-clock` is 4.33 against 4.31 GHz, so it is not the clock.
The whole 4.2 G-cycle difference is inside `preamble::scan_buf` — 58.2% of the
newer profile against 53.3% of the older, over event counts of 40.0 G and
36.2 G — and that function's source is **identical** across the range.
`objdump` confirms the machine code is too: 293 instructions each, the same
opcodes, the same intra-function offsets, differing only in address
(`0x23fe60` against `0x1eef80`).

The confirming experiment: build both with
`-C llvm-args=-align-all-functions=6` and the gap collapses to −1.5%, with
*both* landing below the faster of the two unaligned builds (32.8 G and 33.3 G
cycles, 7.76 s and 7.88 s). The same flag moves the control's `parse`,
`strings` and `typed` shapes not at all — two reps each, differences inside the
run-to-run spread — so **this is one tight character loop's placement and not a
general lever**, which is why no lever row was added for it and why nothing
proposes adopting the flag.

What the phase should take from it: a bisect over that range would have landed
on whichever commit shifted the binary and explained nothing, and `instructions:u`
is the number that holds still when layout moves. The rule is in
[`measurements.md`](measurements.md).

## The `INSERT` fast path goes in `map.rs`

Not close. A warm `release` `parse` of the `INSERT`-run file spends 58.2% in
`preamble::scan_buf`, reached from `statement_complete` ← `Builder::step` ←
`Builder::feed_line`, and 17.3% in `Utf8Chunks::next` under `feed_line`'s
opening `String::from_utf8_lossy(raw).into_owned()`. `CopyScanner::next_event`
is **0.6%**. A third region state in `scan.rs` — the candidate the `KD9` inbox
entry named — would therefore buy under a percent, before considering that it
would need `parse_insert_target` at L1 to know a run had begun.

So 7.5's target is the accumulator: `push_stmt_line` copying every line into a
`String` and `statement_complete` re-walking the whole accumulated statement
with `chars().peekable()` on every line, plus the per-line validate-and-allocate
in front of it. `feed_line`'s lossy conversion is the second-largest bucket in
the whole scan and is not part of the statement-end problem at all, so it is
worth taking separately even if the quote-aware primitive slips.

## The three measure-only levers

**`attach_text` — rejected, with a reading.** Zero samples in a
100,000-sample, 20-second profile of the 4000-block `parse`, which is the input
built to make per-block costs visible. It runs at most twice per map (after the
preamble prepass and at the end), not once per block, and each run coalesces
contiguous text-storing spans into one `read_range`. Nothing to do.

**The double read — one extra pass, and only without a cache.** Counted with
`strace`, not estimated: a `--dqcache none` `query` over the control issues
6,443,503,731 bytes of `pread64` against a 3,221,227,790-byte file, exactly
2.0000×. The same query against a cache reads 1.0000×, and `parse` reads
1.0003×. So the lever is worth one whole pass of the file, it is only ever paid
by a query that has no cache, and `pgdq parse` is the remedy the user already
has. Note the consequence for every published `query` figure: they are all
`--dqcache none`, so the mapping pass and its extra pass are inside them.

**The census — priced on both shapes.** On the brace-free control the
pre-filter is 17.2% of the `parse` profile, 0.050 s corrected, against the
+0.030 s the subtraction reads: the same measurement. On the `--arrays
--composite` file `map::Builder::on_row` is **81.4%** of the profile. Both were
already figures; what the profile adds is that the brace-free cost is one SIMD
pass sitting beside the scanner's own (`memchr::One`, 17.5%) at almost exactly
the same price, which is the shape of the trade if the needle-search discovery
scheme ever displaces it.

## What was found that the lever table did not name

`io::LocalFileSource::read_range` does `vec![0u8; len]` per chunk — the kernel
zeroes a fresh 1 MiB page range, `read_exact_at` then overwrites all of it —
and `scan::scan` copies the result into a second buffer it owns
with `extend_from_slice`, then `drain`s the consumed prefix down. In the `parse`
profile that is `__memset_avx2_unaligned_erms` 23.2% (on the `spawn_blocking`
thread) and `__memmove_avx_unaligned_erms` 31.4%: **54.6% of a warm `parse`'s
user time, 0.158 s of the 0.60 s scan it was taken beside, about half of the
0.31 s separating that from the `dd` floor in the same session.** In a `strings` query the read thread's `memset`
is 7.1% and the chunk copy 3.9% under `poll_next`, together ~0.39 s of 4.48 s.
It is a lever row now; it has no slice, and that is one of the things
`STATUS.md`'s "Decisions worth another look" asks about.

**`print_batch` is the other one, and it is not a lever at all** — it is the
CLI's own output path, 34.0% of a `strings` query's user time and 62.8% of a
typed one's. It matters because it is inside every published `query` figure:
79% of the typed-minus-strings gap is `render_field`, so the subtraction that
sized slices 7.10 and 7.11 was measuring the CLI. Both rows are re-stated in the
spec's table against the profile.

## Deliberately not done

- **No figure was taken and no sweep was run.** Every number here is either a
  profile proportion, a `perf stat` counter, an `strace` byte count or an
  existing `measurements.md` table. The wall times quoted for the `insert_run`
  pair are medians of five interleaved reps on the host, taken to establish
  that a 10% effect exists at all — they are not figures and do not belong in
  `measurements.md`.
- **The alignment flag was not adopted and got no lever row.** It moves the two
  headline shapes not at all; it earned its place as the *proof* of an
  attribution, and treating it as a lever would put a codegen flag into the
  apparatus on the strength of one input.
- **The spec's "Where the `INSERT` fast path lives is for the profile to
  settle" section was left as written.** It records what was open at spec time,
  which is what a spec is for; the answer is beside the mechanism in
  `architecture.md` and in `KD9`'s index line.
- **No slice was added, re-scoped or re-ordered.** The lever table is amended
  because the spec authorises exactly that; the slice list is the maintainer's.
