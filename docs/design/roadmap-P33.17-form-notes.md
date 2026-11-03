# P33.17 notes — which form a resumable reader takes

**The instrument is landed, its layout leg and `push` comparison with it,
and `resumable-reader-micro` is registered untaken** (`measure.UNTAKEN`). The
readings are the next round's: a figure is taken from a commit, never from a
tree carrying its own apparatus ([`measurements.md`](measurements.md), "A
figure may be published outside the sweep"), so they are `cd scripts && uv
run measure.py --figure resumable-reader-micro` at the commit carrying this —
`cargo bench` in the release image twice, 32 criterion benches each at its
default sampling, the layout leg's first a cold build of the bench's tree in
a target dir of its own, minutes rather than the half hour past which a job
is handed off. The verdict it prints is
the slice's deliverable, and the form 33.18 builds in.

- **One reader, three forms** (`crates/pgdt-codec/benches/resumable/`):
  `pull.rs` restates today's walk — `Source`, `Bytes`, `Part`, `Lexer`,
  `walk_array` and `walk_record` as `value/source.rs` and
  `value/nested/input.rs` write them, both forks kept; `walk.rs` is the same
  walks as `async fn`s over a polled `dyn` source, the field's runs `Pending`
  past the run fed; `push.rs` is hand-written machines, a frame a container.
  Every form reads its leaves through one `IntState` and one text count, so
  what differs between them is how a byte reaches a reader, not the reader.
- **Two feeds**: the field as one run, and a run a byte. Today's pull reads
  the second as one iterator chaining the runs, which prices a run boundary
  it never resumes at.
- **The criterion is in the renderer before any reading**
  (`measure.ResumableRow`, `resumable_verdict`): a one-run row's `async`
  median is within its noise of the pull's where it exceeds the mean of the
  pull's two timings — `pull` first in the group and `pull_again` last — by
  no more than the larger of what separates those two and the intervals'
  half-widths summed, widened by how far `async − pull` moves in the layout
  leg. Every one-run row within takes the `async` form; past it on some, the
  push machine only if it beats the `async` walk past a noise formed alike on
  each of those, else the `async` walk
  ([`../status/history/2026-11-03.md`](../status/history/2026-11-03.md),
  "`resumable-reader-micro`'s noise and verdict, settled with the
  maintainer").
- **The layout leg** (`RESUMABLE_ALIGNED_BENCH`) is the same bench under
  `-C llvm-args=-align-all-functions=6`, run after the shipped one in its own
  target dir, `image/resumable-aligned` under the allocator legs' root
  (`image_target_dir`); `release.Bench` carries the flags as `Build` does,
  and refuses them in the release state's target dir. Every gap is read off
  the shipped build; the leg only widens a noise. The flag is accepted by the
  pinned rustc; the leg has not yet been built.
- **The bench refuses to time a form disagreeing with another**
  (`agreement`): every timed literal over each feed, and hand-written cases
  of each refusal the walks make, nested plans two deep among them. Run once
  in criterion's `--test` mode on the debug build, contained; it bit when an
  expected answer was falsified by hand.

## What the readings round inherits

- **The pull leg is a restatement, frozen at this commit**: the figure
  declares the bench's files alone, so a change to `value/source.rs` or
  `value/nested/input.rs` moves no reading. It prices today's shape against
  two others, which is all 33.17 asks of it.
- **The `async` walk is handed its plan, not an `each` callback**, an `async`
  closure being no trait object; the pull leg's per-part `dyn FnMut` call is
  today's and stays in it. A nested container is a boxed future in the
  `async` form and a boxed frame in the push form, built per value; a leaf is
  read in place in both. The pull form's `NULL` test forks, a box per
  unquoted element, as today's does.
- **Read every column** ([`.claude/skills/evidence/SKILL.md`](../../.claude/skills/evidence/SKILL.md),
  rule 4): the push column beside the two being decided, the run-boundary
  lines under the table, and the two pull timings' gap, which is the
  sitting's own drift.
- **For 33.18, should `push` win**: only one-run rows decide because a field
  crosses a run boundary once a chunk, and `Unescaped` yields bytes, so an
  escape makes no boundary for a pulled or `async` source. A `push` machine
  fed a span per escape would meet an escape-dense field as many runs, which
  the byte-a-run rows price; an unescaper filling a fixed scratch buffer meets
  it as few.

## Negative results

- **Not `COPY`'s unescaping**: every literal is unescaped text and no form
  reads through `Unescaped`, a cost the three would share.
- **Not every grammar**: no `[m:n]=` decoration, radix prefix, range or
  multirange, and a byte at a time throughout — no form scans a run in a
  tight loop, which only the push machine could do without a source between.
- **Not the crate's own walk**: `walk_array` is crate-private over a
  crate-private `Source`, and its leaves collect a `String`
  (`grammar/check.rs`, `leaf_refused`), a cost 33.13.3 removes and no form
  of the resumable reader would pay.
