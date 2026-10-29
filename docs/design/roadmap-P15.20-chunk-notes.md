# P15.20 notes — the chunk priced at the default count

**The instrument is landed; the reading is not.** `gzip-first-pass`'s chunk
leg is read at the physical count (`measure.GZPASS_CHUNK_JOBS`), the count a
local first pass recommends (`decisions.md`, "D2"), in place of four jobs. A
figure is taken from a commit, never from a tree carrying its own apparatus
(`measurements.md`, "A figure may be published outside the sweep"), so the
re-take is the next round's:

```sh
cd scripts && uv run measure.py --figure gzip-first-pass
```

It builds the shipped binary and the `gzip-4mib-chunks` leg in the release
image, then four sweeps of seven reps; the level-1 file's sweep dominates.
Run it detached per `CLAUDE.md`, "Long-running processes".

## What the instrument settles, written before the reading

- **The bar is in the harness**, `measure.gzpass_chunk_verdict`, so the
  sitting's table says which chunk ships rather than leaving the read to
  whoever renders it: the leg's chunk replaces the shipped one where it beats
  it, by `gzpass_beats` at `GZPASS_CHUNK_JOBS`, on at least
  `GZPASS_CHUNK_WINS` of `GZPASS_CHUNK_WRITERS` (the three level-6 files) and
  is beaten on none of the four, the level-1 file's included (D121's
  `Reopens`). A win on the level-1 file counts toward nothing.
- **On "replaces"** the slice lands the switch: `ahead.rs`'s shipped
  `CHUNK_BYTES` becomes 4 MiB, the feature inverts to cut 1 MiB (its name,
  `GZPASS_CHUNK_BYTES`, the `--version` marker and
  `test_the_chunk_leg_is_the_library_s_feature_and_its_marker` with it), and
  D121 is rewritten to the new length. **On "stands"** D121's `Rejected` line
  is rewritten to the physical-count reading, and the leg stays as it is.
- **The four-job row of the shipped build stays.** It is the optimization
  baseline (`roadmap.md`, "Four workers is the optimization baseline;
  twenty-four is the guard"), not the chunk leg's; only the chunk pair at
  four goes, which the table's chunk column replaces at twelve.
- **The budget already affords the leg at the physical count**:
  `GZPASS_BUDGET` affords the logical count at either chunk
  (`test_the_budget_affords_the_widest_row_at_either_chunk`), and every run's
  `scan started` line is held to its row's count, so a leg the allowance
  clamped refuses the figure rather than reading a lower count.

## What the reading cannot say

The 4 MiB leg is a separate build, so its layout is a second difference no
pair removes (`measurements.md`, "Two builds of one source can differ by
layout"); the figure's prose says so. And it affords every count its memory,
so it never shows the longer chunk's second cost, every task charged it twice
(D121's `Rejected`): the bar prices time alone.
