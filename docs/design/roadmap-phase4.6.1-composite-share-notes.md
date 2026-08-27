# Phase 4.6.1 — the composite's end-to-end share, and what the instrument can resolve

What the phase wrap and Phase 7 inherit from the slice that finished 4.6's
end-to-end figure. The figures are in [`measurements.md`](measurements.md), "A
typed query over nested columns"; this doc holds what
is not in them.

## What landed

- A third generated input — `scripts/generate_perf_data.py --composite`
  without `--arrays`, 3.00 GiB, `--seed 42` — and the `typed`-against-`strings`
  pair run on it. No generator change: `M10` had already split the flags.
- `measurements.md`'s nested-query section rewritten around **three** rows
  taken in one interleaved sweep, plus a new subsection recording what the
  cross-file subtraction can and cannot resolve.
- A third case in `pgdump_query-cli/tests/perf_generator_fidelity.rs`:
  `--composite` alone now gets the same two assertions the control and
  `--arrays --composite` already had, because it is now an input a recorded
  figure is taken on.
- A [`roadmap-phase7-inbox.md`](roadmap-phase7-inbox.md) entry for the
  instrument's floor, and the figures in that file's nested-copying entry
  restated from the re-taken table.

No library code changed.

## The answer is a bound, and the bound is the instrument's

**The composite column's end-to-end share is not measurable this way.** Three
readings of it: −0.10 µs/row (the recorded five-rep sweep), −0.49 µs/row (an
eight-rep interleaved pair of control against composite), and — the reading
that interprets the other two — **+0.22 µs/row between two files holding the
same sixteen columns and differing only in RNG seed**. A quantity that should
be exactly zero measures +0.22 with a per-rep spread of ±0.39, so anything
under about ±0.5 µs/row out of this instrument is apparatus. Two of the three
readings are negative, which a cost cannot be.

So what the slice delivers is a bound — the composite costs under ~0.5 µs of
every row, under 4% of the 15.1 µs the three nested columns cost together —
and the attribution that bound buys: **the two array columns carry
essentially all of the nested end-to-end cost.** That agrees with the micro,
where the composite is 0.43 µs of the nested group's 6.3 µs and the arrays
have 54 elements to the composite's two fields.

**Why the subtraction has a floor at all**, since it is worth not
re-deriving. Each file's own `typed` minus its own `strings` cancels the
untyped baseline, but the per-*row* normalization that follows does not
cancel across files: the three inputs hold the same bytes and therefore
different row counts (817,024 / 806,322 / 701,287), so any per-byte component
of the typed path lands differently on each file's per-row figure. On top of
that the machine drifts upward slowly over a long run of containers, which is
why the sweep interleaves the files rather than running them in blocks — the
first form of this measurement ran control and composite in blocks and put
the whole drift on the difference.

**The instrument that would resolve it is named and not built.** Generate the
same rows twice, declaring `v_comp` as `public.perf_comp` in one file and as
`text` in the other: byte-identical data sections, identical row counts, and
the only difference is whether that column is decoded. It needs a generator
knob that writes a deliberately weaker declaration than the value's real type,
which is a new instrument rather than this slice's, and nothing yet needs a
figure that sharp. Recorded beside the figure and in the phase-7 inbox.

## The whole table was re-taken, and the ratios moved

The recorded control and array rows came from `M10`'s session an hour earlier;
this slice re-ran both alongside the new file rather than adding one row
across sessions, because a cross-file attribution taken in two sessions is
exactly what `measurements.md`'s "say which regime, and stay in it" rule
forbids. The consequence is that two figures nobody queued for re-measurement
moved: the ratios went **3.45× → 3.08×** (nested file) and **2.26× → 2.11×**
(control), because this session's `strings` leg is ~9.6 s on all three files
where `M10`'s was 8.0–8.8 s and varied by file.

**The derived figures barely moved**, which is what says the change is in the
baseline and not in the typed path: the three nested columns cost 14.6 µs/row
in `M10`'s session and 15.1 µs/row here, and the sixteen scalar columns 13.5
against 13.2. A ratio against a `strings` leg that moves 15% between sessions
is the fragile way to state this figure; the per-row difference is the durable
one, and the section leads with it.

**`M10`'s between-file baseline gap was apparatus, and its explanation is
retracted.** That session read 8.80 s (control) against 8.04 s (nested) and
attributed the 9% to the untyped path being partly per-row, the control
holding 817,024 rows to the nested file's 701,287. This slice's sweep reads
9.74 against 9.56 — 2% apart, with the row-count spread unchanged — so the gap
was not per-row cost. The difference in method is the answer: `M10` ran the
comparison a file at a time, which maps a session's own drift onto file
identity. Reviewed 2026-08-27; `measurements.md` now carries "Re-take a
comparison table whole, in one interleaved sweep" as a standing rule, and the
sibling rule that a parsing-CPU figure belongs on tmpfs rather than on a
page-cache-warm filesystem read.

## What the next slice inherits

The Phase 4 wrap consolidates this doc with the other twelve. Two things in it
are not restatements of `measurements.md` and should survive consolidation:
the instrument's floor (already filed for Phase 7, since that is where it
bites), and the negative result that the composite's share cannot be separated
by differencing whole files — which is what stops a later session from
re-running the same pair with more repetitions and expecting a number.
