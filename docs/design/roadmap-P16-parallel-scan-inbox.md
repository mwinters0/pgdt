# P16 inbox — facts filed for its grilling

Evidence for P16 (parallel scan and extraction), the phase carved out of P7 at
P7's grilling. **This is a queue, not a document**: when P16 is grilled, walk
every entry, fold it into that phase's spec or discard it as stale, and delete
this file. See `docs/process.md`, "Inboxes: facts filed by destination".

Each entry says what the fact is, why *this* phase cares, and where it came
from — go re-check the origin rather than trusting an entry that has aged.
Three of them were filed at P7 and moved here whole when the parallel half
became its own phase; their "why this phase cares" paragraphs are rewritten
only where they named P7 for work P7 no longer does.

---

## `stream::splice` assumes coverage is a contiguous prefix

**Fact.** A query's mapping pass rebuilds `DumpIndex::spans` as `prefix ++
built ++ [Unscanned tail]`, where `prefix` is every span with `end <=
seg_start`. The seam between the two is closed by **extending the last prefix
span** to where the first newly-built span starts. Both halves of that depend
on the prefix being a complete tiling of `[0, seg_start)`: if coverage had an
interior hole, `prefix` would not tile, and extending its last span across the
seam would silently paper over the wrong range.

**Why this phase cares.** This is the *one* place the P3 spec's "coverage is
a prefix, by construction" stopped being an observation and became an
assumption in code. That spec section already names P7's device-aware
parallelism as "the plausible future source of interior holes" and argues a
span list (rather than a watermark) is what makes them expressible. It is
right that the *format* allows them — but `splice` does not, and out-of-order
NVMe scanning is exactly what would produce them. Whatever this phase does about
scan ordering has to either keep coverage prefix-shaped or rework `splice`'s
seam rule, and that should be a decision, not a discovery. P7 owes this phase
a written statement of which — see
[`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md), "What this
phase is not".

**Origin.** 2026-08-24. See
[`architecture.md`](architecture.md),
"What the next slice inherits".

---

## A seekable compressed source hands this phase parallel discovery for free

**Fact.** This phase's "Parallelism" section splits the problem in two:
row extraction parallelizes trivially once boundaries are known, while
*discovery* is hard, because from a cold start you cannot tell whether a random
offset is inside a `COPY` block — which is why the speculative scheme is
prototype-and-measure work rather than a plan.

A seekable compressed source removes exactly that half. An `.xz` stream's index
gives every block's uncompressed offset before a byte is decoded, so workers can
be handed real, self-contained ranges rather than speculative ones. Measured on
the koji `.xz` (31,150 self-contained streams, ~24 MiB uncompressed each): one
core decodes ~446 MB/s of plaintext, four concurrent per-stream decodes reach
~1.48 GB/s at 397% CPU, and `xz`'s own `-T8` on that file gains nothing, its
threaded decoder parallelising blocks within a stream where each stream holds
one. Numbers and method: `roadmap-P13-compressed-input-inbox.md`, "xz decodes at
~446 MB/s of plaintext per core" — probes, not figures.

**Why this phase cares.** P13 lands the decompressing source and deliberately
does *not* take parallel decode, leaving it to this phase. So this phase inherits a second parallelism case with a
different bottleneck — CPU-bound at ~450 MB/s a core where the plain path is
device-bound at ~240 MB/s on the same HDD — and a different unit of work: a
compressed block rather than a byte range resynced to the next LF. A device-awareness rule therefore needs a second axis, since the right worker count for a
compressed source is set by cores and decode rate, not by
`/sys/block/<dev>/queue/rotational`.

**Origin.** 2026-09-02, sketching P13.

**Contingent on** P13 landing first, which is this table's order. If it slips
behind this phase, the entry becomes a constraint on defaults rather than a
case to implement.

---

## Parallel xz decode is worth ~3.3x, and the seekable-xz crate is being shaped to allow it

**Fact.** On a 300 MiB stream-aligned slice of the koji `.xz`: one `xz -dc -T1`
process reaches ~446 MB/s of plaintext at 108% CPU; `xz -dc -T8` on the same
slice reaches **the same 446 MB/s**, because xz's threaded decoder parallelises
blocks *within* a stream and every stream in that file holds one block; four
`xz -dc -T1` processes on four stream-aligned pieces reach **~1.48 GB/s** at
397% CPU, near-linear. Probes, not figures — no harness, no `drop_caches`.

**Why this phase cares.** A compressed source inverts the arithmetic behind
"device-bound": it reads 19x fewer bytes and pays for them in CPU, so the
readahead, chunk-size and parallelism defaults this phase sets have two source
shapes to satisfy rather than one. The discovery half of parallelism is already
solved for a seekable stream — block boundaries are known up front from the
index — so this is the case where only the easy half is left. The external `xz-seek` crate P13 is blocked on is being specified to
*defer* parallel decode but not design it out: its requirements say the block
decoders stay independent of one another and of any shared cursor, behind the
same positioned-read interface.

**Origin.** 2026-09-02, grilling P13. Re-check
`/mnt/wd12t/fedora/experiments/xz-seek/docs/design/historical/initial.md` for what
the crate actually committed to.

---

## The sparse row index is this phase's to build, and P10 needs the same interval

**Fact.** The index — the byte offset of every Nth row — was P7's until P7's
grilling found it had no consumer there. `ResumeToken` already carries a byte
offset and an in-block row count, so resuming a query does not rescan a block
from its start, and nothing in the library or the CLI exposes a row-range seek.
koji puts the scale on it: 19,575,829,920 rows is ~2.4M checkpoints at ~19 MB
against ~157 GB for a dense 8-byte-per-row index. `CopyBlock::sparse_index` is
reserved and always `None`, so adding it is not a cache-format break.

**Why this phase cares.** It is the first phase that reads one: known row
boundaries turn a speculative split into a real one and remove the resync scan
entirely. The interval is not a free choice — P10 attaches per-row-group
statistics to it, so whichever of the two phases runs first settles it and the
other inherits it. Decide it against both, not against splitting alone.

**Origin.** P7's grilling, 2026-09-03
([`../status/history/2026-09-03.md`](../status/history/2026-09-03.md), "P7's
grilling").
