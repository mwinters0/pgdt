# P14.9 — A dropped read is the interrupt by another door

What landed: `stream::cancelled_read`, the one predicate that says a failed
read *is* the cancellation, and four call sites that route it onto the outcome
the polled check point beside each of them already produces —
`map_forward`'s chunk read, its trailing `attach_text`, `map_file`'s preamble
prepass, and `observe_rows`, which is the back-fill's read loop.
`stream::interrupted_run` is the one construction of an interrupted `MapRun`.
`main.rs`'s translating arm is gone: `parse` no longer classifies an error as
an interrupt, and a remote stop prints the same two lines a local one prints.
No `D<k>` was added — [`decisions.md`](decisions.md), "D26" already carries the
rule ("the shape follows the run, not the provider"), and this slice is the
code catching up to it.

## What the rest of the phase inherits

- **The predicate is `ScanCancelled` *and* the flag.** A source reporting a
  cancellation nobody asked for is misreporting itself and stays an error, so
  the translation cannot swallow a fault. 14.7's `XzSource` over a remote
  source inherits it unchanged: every fetch a footer walk or a block decode
  makes goes through `RemoteSource::read_range`, so a Ctrl-C under one arrives
  as the same error at the same four places.
- **`map_for_query` is deliberately untouched.** It turns `MapStop::Interrupted`
  back into `Error::ScanCancelled` ([`decisions.md`](decisions.md), "D48"), so
  a cancelled query errors on both providers — which is why "every read loop"
  was the wrong scope for this and the mapping pass was the right one.
- **The trailing `attach_text` now runs before the index advances.** The span
  list is built and texted aside, and only a whole one is assigned. Both
  `map_forward`'s tail and the prepass do it that way, in the same shape, and
  the ordering is load-bearing rather than tidiness: a run banked at
  `scanned_through == size` is never read again, so texting in place would have
  let a dropped read persist a map whose spans lost their text with nothing
  that would re-attach it.
- **The back-fill keeps its own counts.** The translation there sits in
  `observe_rows` rather than at `backfill_statistics`'s call site, so a dropped
  read leaves `lacking_statistics` and `backfilled` saying what a polled stop
  says. That is what keeps `pgdq parse`'s two interrupt sentences apart —
  "the map is short" against "the re-read is short".

## Negative results

- **Nothing was added to `ByteRangeSource`.** A trait member saying "this
  source cancels by failing" was the alternative; the error variant and the
  flag already say it between them, and a source that answers a cancellation
  any other way is served by the same predicate.
- **A cancelled preamble prepass banks nothing but still saves.**
  [`decisions.md`](decisions.md), "D26" refuses a half-read preamble, so the
  run returns the map it started from — and saves it, so that `parse`'s "the
  cache at … holds the scan so far" is true of a run that had no cache before.
  The alternative, returning without saving, makes that sentence a lie on the
  one path that can reach it with nothing on disk.
- **The evidence is a pair, not a single test.**
  `map_file::a_dropped_read_is_the_same_interrupt_as_the_flag` trips at the
  same offset as `a_cancelled_map_file_reports_it_and_banks_what_it_scanned`
  and asserts the same watermark, the same incomplete cache and the same
  resumed index; reading one without the other would not say the two providers
  converge. Both were run against this slice's `stream.rs` reverted and both
  failed on the `Err` they now refuse, so what they measure is the translation
  and not the fixture. `DropsPast` is `CancelsPast` with the bytes withheld, so
  the pair differs in exactly the thing under test.
- **The oracle proves the whole path once**, in
  `remote::a_cancelled_remote_read_is_an_interrupted_run_rather_than_an_error`:
  a real stalled request, a real dropped future, `map_file` answering `Ok`.
  The library pair is where the watermark and the resume are asserted, because
  a stall knob cannot say *which* read was dropped.
