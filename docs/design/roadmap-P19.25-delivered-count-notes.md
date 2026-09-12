# `P19.25` — a `parse` says what ran, not what was asked for

What the closing sweep (`19.11`) inherits. The spec row is
[`roadmap-P19-efficient-defaults.md`](roadmap-P19-efficient-defaults.md),
"Slices"; the mechanism is [`architecture.md`](architecture.md), "Status
output", under *`scan arrangement` is the correction*.

No sitting and no measurement. What the new `tracing::info!` call adds is a
fixed few fields per *scan*, on a path a sweep leg cannot reach: see "What
`19.11` inherits", below.

## What landed

One new status line, `scan arrangement`, emitted by `stream::map_forward` at
most once per scan:

```
INFO scan started     bytes=… resumed_from=… chunk_size=… jobs=24 memory_bytes=67108864
INFO scan arrangement jobs=1 asked=24 bound_by="source" would_hold_bytes=130023424
```

Three pieces:

- `leader::Shortfall` — `asked`, `delivered`, `bound_by`, `would_hold_bytes` —
  answered by `leader::shortfall`, a free function `scan_region` calls with the
  advice it has just read.
- `leader::RegionOutcome` — `scan_region`'s return reshaped from a bare
  `RegionScan` to `{ scan, shortfall }`. The shortfall is answered in **both**
  arms, a declined region and a cut one alike, because a cut region can also
  run fewer readers than were asked for.
- `stream::report_shortfall(&mut bool, Option<Shortfall>)` — the once-per-scan
  gate, beside the `CopyStart` arm that calls the leader.

## The three calls that are not obvious

**Silence is a claim, not an absence.** The line is a *correction* of
`scan started`'s `jobs=`, so it is emitted only where the two disagree. That
makes "no such line" mean "the announced count ran", which is what the manual
now says. The alternative — one line per scan unconditionally — restates a
number the line above already carries, on every scan pgdq ever runs.

**Two refusals are reported and a third is not.** `scan_region` declines for
three reasons: the caller's count or budget affords one reader, the source
advises a single partition, or the floor — what remains of the *file* from this
block's data start is shorter than one partition. The first two are properties
of the arrangement and are still true at the next block; the third is an end of
file condition, since the region's extent is not known here and the remainder
is the only bound available. Being serial on a tail shorter than one reader's
charge is the right arrangement, so there is nothing to correct, and a line
saying so would fire on the last block of every file.

**What silence does not mean** is that the arrangement was a good one. The
gate's contract is narrow on purpose — it corrects `scan started`'s `jobs=` and
nothing else — and the arrangement it cannot speak to is `KD22`: a block far
smaller than the `workers × partition_bytes` window it is cut from is found by
reading the whole window and discarding it, which delivers the announced count
while reading 268× a serial scan's bytes at `--jobs 8`. That is a defect in the
leader, not a gap in this line; widening the line to carry an over-read ratio
would cost an accumulator across regions and help nobody it is aimed at
(`docs/design/architecture.md`, "The interior split").

**The source arm is asked about the whole file, not about the region.** A
block-decoding source standing on a region past its last block boundary advises
one partition too — `XzSource::partition_advice` collects the block starts
*strictly inside* the range — so the region question answers "declined" for a
source that is perfectly willing to be split. This is not hypothetical: it
fired on `tests/data/edge_cases.sql` compressed at `--block-size=512`, whose
last `COPY` block starts past the last boundary, reporting `bound_by="source"`
under a 512 MiB budget that afforded the block path outright. So the arm is
gated on `source.partitions(0..size).max_partitions() == Some(1)` — the same
question `stream::compressed_block_path_declined` asks, asked the same way —
and the second `partitions` call is paid only on the path that is about to
report something.

## `would_hold_bytes` is one quantity with two sources

It is **the budget that buys the refused arrangement back**, and in both arms
it is the source's own arithmetic rather than a number this crate derived:

| `bound_by` | the number | why that one |
|---|---|---|
| `source` | `ByteRangeSource::block_decode_bytes()` | what one reader of the container path costs — the line `BlockCache::affordable` actually draws, per-reader charge *plus* the pool floor it leaves |
| `budget` | `Partitioning::worker_memory().at(asked)` | what the readers that were asked for would have held, floor included |

`block_decode_bytes` is the same number
`PlanNoteKind::CompressedBlockPathDeclined` names on a query, so the two
channels cannot disagree about the recourse. It is an `Option<u64>` and
`tracing` omits a `None` field entirely, so a source that states no cost prints
no number rather than a zero.

## What was refused

Both routes the closed "Decisions worth another look" entry priced, and for the
reasons the spec row gives: a `parse`-side decline line in the CLI would print
a decline beside a `jobs=` line that is itself wrong, and would make the CLI
re-derive the source's arithmetic; lifting the plan notes out of `TableStream`
is a public surface change buying nothing the status channel already reaches.
Both are filed as rejected alternatives in `architecture.md`, "Status output".

A third was refused here rather than in the spec: **computing the shortfall in
`map_forward` before the loop**, from `source.partitions(scanned_through..size)`.
It avoids reshaping `scan_region`'s return, and it is a second copy of the
decline rule — the failure the `reader_bytes` collapse was done to prevent.

## What `19.11` inherits

Nothing it has to do, and nothing it has to re-read.

The sweep's command shapes all state `--jobs 1` (`measure.SWEEP_JOBS`), so
`asked <= 1` and `leader::shortfall` returns before it reads anything. The
jobs-axis families state a count, so one of their legs *can* print the line —
a log line, on a channel no figure reads.

**The `reserve` figure's flagless legs cannot print it, and the reason is
structural rather than lucky.** Those legs state neither flag and read their
resolved count back off the run's own `scan started` line
(`measure.SCAN_RESOLVED_RE`, which is anchored on `\bscan started\b` and so
cannot match the new message). The count they read is `Parallelism::fit`'s,
solved against `XzSource::default_worker_memory()` — which *is*
`block_worker_memory()`, the block path's own shape. So a resolved count above
one implies the budget cleared `at(1)`, which is `BlockCache::affordable`
exactly, which is the block path taken; and where it does not clear, `fit`
floors at one and `asked <= 1`. Either way no flagless leg reports a shortfall,
and `charge_model`'s `jobs` stays the count whose resident set it is accounting
for.
