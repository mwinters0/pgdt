# P15.1.2 notes — the spacing figure

**The instrument is landed and `gzseek-spacing` is registered untaken**
(`measure.UNTAKEN`); its readings, and the spacing constant they set, are the
next round's. A figure is taken from a commit, never from a tree carrying its
own apparatus ([`measurements.md`](measurements.md), "The apparatus"), so the
readings are `cd scripts && uv run measure.py --figure gzseek-spacing` at the
commit carrying this, about ten minutes warm, needing no handoff.

- **What it prices is gzip's own terms, on the shipped engine** (zlib-rs,
  [`decisions.md`](decisions.md), "D112"), at 1 to 128 MiB in powers of two
  (`measure.GZSPACING_MIB`): the first pass against zlib-ng's serial inflate;
  a resume's fixed cost; the window section; and every unit decoded whole.
  What a unit's size moves that xz shares is read off xz's figures
  ([`../status/history/2026-10-22.md`](../status/history/2026-10-22.md), "The
  spacing figure, reordered"). The falsification criterion is written in
  `measure.run_gzseek_spacing`'s docstring, ahead of any reading.
- **A resume's fixed cost is its own shape, `starts`**, not the `units` row
  less the first pass: `gzseek-backend` showed that difference cannot
  separate 96 resumes from its two rows' spread
  ([`measurements.md`](measurements.md), "Which engine `gz-seek` reads gzip
  through"). `starts` is the first pass and then every unit begun from its
  checkpoint and window and fed until its first plaintext byte, its first
  4 KiB read positionally; over the first pass, divided by the unit count, it
  is a fresh inflate state, the window set, the bits primed, a read and the
  first block's header — an upper bound, the header being read by a whole
  unit decode anyway. Both of one binary in one sweep, so the subtraction is
  within a sitting.
- **The window section is counted, not computed**: `gz_read` prints
  `windows=`, every checkpoint's window as the first pass copied it, which a
  checkpoint at a member start holds none of.
- **What the harness refuses**: a `delivered=` other than `control`'s size
  (the first pass's in `starts`), an `engine=` other than zlib-rs, three
  stages at one spacing disagreeing on the unit count or the window bytes, a
  unit count rising with the spacing, and a `starts` run that began fewer
  units than its index holds (`started=`).
- **Smoke, untimed**: a host build of the example read `control_gz` in a
  512 MB container in every stage at 1 MiB, in `starts` at 32 MiB and in
  `units` at 128 MiB, each delivering `control`'s size, with 3072, 96 and 24
  units; those two stages at the ends of the range hold the most — the
  1 MiB row's windows and the 128 MiB row's unit buffers.
- **`gzseek-backend` goes red** on `gz_read.rs`, which it declares; nothing
  re-takes it, D112 being settled.

## What the readings round inherits

- **The spacing constant has no code home before 15.5**, which writes the
  `deflate` adapter that passes it; whether the readings round states it in
  a `D<k>` alone and 15.5 lands the constant, or lands it now, is that
  round's call.
- **Read the per-unit column only where its numerator clears the spread**
  of the two rows it is the difference of; where it does not, the row bounds
  the cost and does not price it (`.claude/skills/evidence/SKILL.md`, rules 2
  and 6).
