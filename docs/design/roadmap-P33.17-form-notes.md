# P33.17 notes — which form a resumable reader takes

**The input walks resume as hand-written push machines.** That is
`resumable-reader-micro`'s verdict, taken at `0fd33bd3`
([`measurements.md`](measurements.md), "Which form a resumable reader takes"):
the `async` walk is past its noise of today's pull on every one-run row but
the composite's, and the push machine cheaper than the `async` walk past
theirs on each of those rows. The criterion was in the renderer before the
reading (`measure.resumable_verdict`), settled with the maintainer
([`../status/history/2026-11-03.md`](../status/history/2026-11-03.md),
"`resumable-reader-micro`'s noise and verdict, settled with the maintainer"),
so the verdict follows from it with no call made. The figure is that
decision's evidence; 33.18, which builds the form, files the `D<k>` entry
for it with the code it explains.

- **One reader, three forms** (`crates/pgdt-codec/benches/resumable/`):
  `pull.rs` restates today's walk — `Source`, `Bytes`, `Part`, `Lexer`,
  `walk_array` and `walk_record` as `value/source.rs` and
  `value/nested/input.rs` write them, both forks kept; `walk.rs` is the same
  walks as `async fn`s over a polled `dyn` source; `push.rs` is hand-written
  machines, a frame a container. Every form reads its leaves through one
  `IntState` and one text count, so what differs between them is how a byte
  reaches a reader, not the reader. `push.rs` is the shape 33.18 starts from.
- **The bench refuses to time a form disagreeing with another**
  (`agreement`), every timed literal over each feed and hand-written refusals
  two plans deep among them, so the three forms read one grammar.

## What 33.18 and its successors inherit

- **A run boundary costs the push machine almost nothing.** Fed a run a byte,
  it is still cheaper than the `async` walk fed one run, on every literal. So
  the case these notes left open for a push machine — a field fed a span per
  escape, meeting an escape-dense field as many runs — needs no fixed scratch
  buffer in front of it on cost grounds: a span per escape is affordable
  within this grammar.
- **On a short scalar fed one run the push machine costs more than today's
  pull**, unattributed (the figure's section names the candidate). Every
  array and record row is cheaper pushed than pulled. 33.13.3 meets this: it
  moves every scalar reader onto a byte source and resumes them on 33.18's,
  and a scalar is read whole off one run on nearly every line.
- **The layout leg moves the arrays' gaps more than the sitting's scatter
  does**: on these readers code placement is the larger noise, so a bench
  without a layout leg reads them narrower than they are.

## Negative results

- **Not `COPY`'s unescaping**: every literal is unescaped text and no form
  reads through `Unescaped`, a cost the three would share.
- **Not every grammar**: no `[m:n]=` decoration, radix prefix, range or
  multirange, and a byte at a time throughout — no form scans a run in a
  tight loop, which only the push machine could do without a source between.
- **Not the crate's own walk**: `walk_array` is crate-private over a
  crate-private `Source`, and its leaves collect a `String`
  (`grammar/check.rs`, `leaf_refused`), a cost 33.13.3 removes and no form
  of the resumable reader would pay. The pull leg is a restatement frozen at
  `0fd33bd3`; the figure declares the bench's files alone, so a change to
  `value/source.rs` or `value/nested/input.rs` moves no reading.
- **Unattributed: the pull leg's per-element cost is several times
  `nested-decode-micro`'s**, which predates the input grammars becoming
  walks; the figure's own section says what nothing here separates,
  and why it moves no verdict.
