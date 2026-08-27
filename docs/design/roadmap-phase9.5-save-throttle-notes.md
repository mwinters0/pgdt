# 9.5 — The save throttle and the interrupt guard

What 9.5.1 and the phase wrap inherit from the slice that decided *when* a
mapping scan writes its cache, and *how* it stops.

## The shape

Three pieces, one seam between them.

`ScanOptions::cancel: Option<Arc<AtomicBool>>` (L1, `scan.rs`) is the flag.
`None` by default, so no existing caller changed. **Only
`stream::map_forward` reads it** — `scan::scan` and the eager producers built
on it ignore it, and the field's doc comment says why: stopping there would be
indistinguishable from reaching EOF, so a truncated index would come back
looking complete.

`stream::SaveThrottle` (L4, private) is the rule: skip a completed block's save
unless at least `K = 20` times the last save's own *measured duration* has
elapsed since it. `due_after(elapsed, last_cost)` is the predicate, taken apart
from the clock so the unit test does not need one. It starts due (`last_cost`
is `Duration::ZERO`), which is what keeps the first block of a segment banked.

`stream::MapRun { index, resumed_from, interrupted }` replaces `map_file`'s
`(DumpIndex, u64)`. The third field is the whole point: an interrupted run may
not state the three whole-file facts (`metadata` recomputed over every span,
diagnostics, the final save), so it returns before them.

## Four calls the next slice should know about

**The guard is checked at *two* granularities, and the spec said one.** Chunk
granularity is what gets a scan out of a block whose `CopyEnd` is an hour away;
it is useless on a 4000-block 2 MB dump, whose entire 23-second scan is two
chunks. Measured, then fixed: the first end-to-end `SIGTERM` test was swallowed
for twenty seconds. The `CopyEnd` arm now reads the flag too, the spec's
sentence was amended in place, and `STATUS.md` carries the entry. Anything that
changes where the loop spends its time should re-ask this question.

**Every exit saves unconditionally; only mid-scan saves are throttled.** EOF, a
settled target and an interrupt all call `throttle.save` regardless of `due()`.
Skipping the save at the last watermark before a query's early stop would throw
that query's scan away, which is the sharp edge the one-rule-two-callers design
has. In the `CopyEnd` arm this reads as `if settled || cancelled ||
throttle.due()`.

**A cancelled *query* is `Error::ScanCancelled`, not a short stream.**
`map_forward` has two callers and only one of them can report partiality.
`pgdq query` never sets the flag, so this is reachable only by an embedder —
and an embedder that cancels wants an error rather than a silently truncated
prefix of a table's rows. Phase 6 inherits the question of what its API surface
does with it; the entry is in `roadmap-phase6-inbox.md`.

**The CLI's signal handling is Unix-only and deliberately outside the library.**
`install_interrupt_guard` spawns one task per `SignalKind`; each sets the flag
and records the signal number, and a *second* signal of either kind calls
`std::process::exit(128 + n)` so a wedged save cannot hold the process. The
`tokio::select!` form over `ctrl_c` is the one that looks obvious and destroys
the map — `map_file` owns the `DumpIndex` for the whole scan, so cancelling
that future drops it. An interrupted `parse` prints both lines to **stderr**
and no listing: the user asked the scan to stop, not for a report, and `pgdq
info` is the command that reports.

## What the measurement found, and what it did not fix

The throttle hits its design target exactly — at 4000 blocks, 4003 saves become
195 (`n/20`) and ~21 s of saving becomes ~1.2 s of a 23.5 s scan — and the
block-count series **still** quadruples per doubling, because a second cost has
the same shape: every `CopyEnd` rebuilds `DumpIndex::spans` whole
(`map::Builder::snapshot` clones the builder's spans, `stream::splice` clones
the prefix). A scan that never saves at all (`query --dqcache none`) costs
19.7 s at 4000 blocks. Both series and their commands are in
[`measurements.md`](measurements.md); the map half is filed into
[`roadmap-phase7-inbox.md`](roadmap-phase7-inbox.md), beside the entry that
already flags `splice` — they are the same code and want one decision.

**Nothing about koji changed**, which is what the throttle was designed for:
blocks ~45 s apart and saves well under a second clear the `20x` bar every
time, so the +1.5% figure stands as measured.

## Testing it without a signal

`tests/map_file.rs` has `CancelsPast`, a `ByteRangeSource` that trips the flag
once a read asks for a byte at or past a chosen offset — the interrupt
delivered at a *file offset* instead of a wall-clock moment, which is what
makes the test deterministic. It is the sibling of `FailsPast` (9.1's
simulated death), and the difference matters: `FailsPast` returns an `Err`,
`CancelsPast` lets every read succeed, so the scan is stopped by the flag
alone.

The signal path itself has no automated test: a fixture parses in
milliseconds, so any test that races a signal against it is a coin flip. It was
verified by hand against the 4000-block bench file, whose parse takes ~23 s —
`SIGTERM` → exit 143, `SIGINT` → exit 130, both leaving a cache `pgdq info`
reports at 57%, and resuming produced a cache **byte-identical** to a
straight-through parse's. The real-scale version (a signal inside a
hundred-gigabyte block, against a cache holding dozens) still rides on Phase
4's wrap koji run, as the spec says.

## The defect this slice's verification found

An interrupted `parse` leaves a cache with no `DumpMetadata` at all, because
`map_file` runs no preamble prepass, and `resolve_columns` turns "no metadata"
into `NotDeclared` for every column — the *final* answer, where the truth is
`MetadataNotScanned`, "finish the parse and ask again". That is the exact
confusion 9.4 introduced its seventh variant to prevent. It is slice **9.5.1**,
not part of this one, because the fix changes what every cold `parse` does
before mapping. Detail: [`../status/history/2026-08-27.md`](../status/history/2026-08-27.md).
