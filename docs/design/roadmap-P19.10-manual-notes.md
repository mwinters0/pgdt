# P19.10 — The manual, and the two flags' help text

What the user-facing surface now claims about this phase's defaults, and what
is deliberately still unsaid. Documents only: no library or CLI behaviour
changed, and the only code touched is two doc comments that are also `--help`
text.

## What this slice found, and it is the part worth inheriting

**The manual stated the discovered budget as the allowance itself.** "A
container given 512 MiB scans inside 256 MiB and one given 3 GiB inside
2.75 GiB" reads the reserve subtraction as the answer, where
`Parallelism::discover_for` treats `limit − MEMORY_RESERVE` as a **ceiling** and
takes what the source asks for inside it — so a plain dump in a 3 GiB container
runs at 64 MiB and a 24 MiB-block `.xz` at about 1.4 GiB, neither of them at
2.75 ([`architecture.md`](architecture.md), "The byte budget's default is the
environment's"). Both flags' help text carried the same omission, its
`--parallel-memory` paragraph having been written when the subtraction *was* the
whole rule. The manual now states the ceiling and the file's ask as two steps,
and says that where the ceiling affords fewer readers the **count** comes down
with the budget — which is the half a reader cannot infer from a byte number.

That is the shape of error to look for when the next slice audits a user-facing
claim: not a number that moved, but a rule that gained a second term while the
sentence describing it kept only the first.

## The three sentences the phase's own code had falsified

- **"`largest block` is what `--parallel-memory` has to clear twice over"** —
  true of `19.7`'s threshold, understated after `19.14` charged the chunk
  buffer and `Reader::decode_footprint()` on top of the two blocks. It now says
  twice over *plus roughly 10 MB*, and points at the section that decomposes it.
- **"a file of 24 MiB blocks adds about 48 MiB resident for every worker"** —
  the two blocks alone, where the charge a budget is divided by is ~58 MiB. The
  `--chunk-size` section now names the same per-reader number the
  `--parallel-memory` section does, which is the only way a reader can reconcile
  the two.
- **"each worker also holds the block it is decoding at that moment"**, offered
  as why a compressed scan holds more than the budget names. After `19.14` that
  block *is* charged; what a scan holds above the budget is the fixed part —
  threads, decoder state outside the pools, arena retention — which is what the
  callout below it was already saying. The two callouts no longer give
  contradictory accounts of the same excess.

## `MALLOC_ARENA_MAX`, as `M76` and `19.12` leave it

The recommendation says the cap **gives real memory back on both shapes, in
different places** — off the fixed term on a compressed scan (so it does not
scale with the budget and is no substitute for sizing the cgroup), and the whole
of the per-worker growth on a plain one — and states no number, because every
reading behind it is diagnostic and unpublishable
([`roadmap-P19.12-reserve-retake-notes.md`](roadmap-P19.12-reserve-retake-notes.md)).
The existing "roughly 8 MB more for each worker you allow" stands: that is
`KD18`'s property statement, landed with the register entry rather than quoted
off a sitting. **`19.11` is what can put a number here**, and if its sitting
publishes one, this paragraph is the consumer to re-read.

## What was added rather than corrected

`19.9`'s below-floor `PlanNote` had reached no user-facing document. The
small-allocation paragraph now says `query` names the budget in force beside
what one reader holds — the sentence that keeps a slow run inside a tight
container from being silent about why. `parse` emits no such note (plan notes
hang off `TableStream`), so the manual credits `query` alone.

## The help text is still a shared surface, and that is a call to review

The two flags' doc comments are simultaneously rustdoc and `clap` long help, so
`pgdq parse --help` prints `(docs/design/architecture.md, "…")` citations and
`**bold**` markers at a user who may have neither the tree nor a renderer. This
slice corrected what those paragraphs *claim* and left the convention alone:
every long help text in the CLI is written this way, so splitting rustdoc from
help (a `long_help` per flag) is a change to the CLI's user surface rather than
to this row's claims. Filed under `STATUS.md`'s "Decisions worth another look"
as the decision it is.

## What no document claims yet

The plain path's four-worker concurrency ceiling is still stated as the buffer
pool's depth, which is what `LocalFileSource::hint_parallelism` still says
(`POOL_DEPTH` slots, whatever `--jobs` asks) — checked against the code rather
than inherited, because `19.5` changed the partition's *size* and `19.7` changed
whether its buffer is kept, and neither moved the slot count the wait is bounded
by.
