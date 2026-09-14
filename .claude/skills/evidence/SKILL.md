---
name: evidence
description: How to draw a conclusion from a measurement or a test — account before fitting, check a fit inside its own data, and what makes two readings independent rather than merely agreeing. Use before concluding anything from a benchmark, profile, resident-set reading or timing; before fitting a model to measurements; before planning a slice whose deliverable is a reading; when a measured quantity is partly unexplained; when building or reading a figure that subtracts one build from another; and when writing a code comment that asserts a property the code does not test.
---

`docs/design/measurements.md` and `scripts/measure.py` govern **how a figure is
taken** — the apparatus, the staleness edges, what may be published. They say
nothing about **what may be concluded from it**, and that is the half that has
actually gone wrong here. This skill is that half.

It is short on purpose. Seven rules, each with the failure it prevents.

## 1. Account before you fit

An *account* names the terms and sums them. A *fit* produces a slope and an
intercept. They are not the same deliverable and the second is not evidence for
the first.

Where a quantity is unexplained, the first move is **arithmetic from the
source**, not another sitting: enumerate every live allocation (or every
operation, for a timing), say for each whether it scales with the input, the
worker count, the unit or nothing, write the expression, and evaluate it at the
cells already measured. A term table that does not sum to the reading is a
finding on its own, and it is free.

Reach for a new measurement only when the model is built and two of its terms
are indistinguishable in it. **A measurement can rank candidates; only the code
can name them.**

**A gate is a fit too.** An acceptance criterion amended one term a round,
against an account nobody has completed, is this failure one level up: enumerate
the whole account before the next amendment, not after the fourth.

> When this was skipped, three sessions measured the consequences of a defect
> and attributed them to a pool floor that the pool's own slot arithmetic ruled
> out. The refutation, when it finally came, needed no run at all.

## 2. A fit is a claim about its window, not about the mechanism

Before publishing or reasoning from `y = a + b·x`, **evaluate it at a point
inside its own data — the smallest arrangement especially** — and state the
residual there. An intercept is only a physical quantity if the fit still holds
where the mechanism is simplest.

Two specific traps:

- **A concave or stepped curve fitted over a window that excludes the origin
  puts the excluded curvature into the intercept.** The intercept then looks
  like a constant of the process and is a property of where you started
  measuring.
- **A quantity that is fixed at one end of an axis and per-unit at the other**
  is the signature of a clamped term. Check for a `clamp`/`min`/`max` in the
  code before believing either shape.

> The worked instance: `resident = 403 MiB + 31.2 MiB a reader`, fitted over
> 3–24 readers, against **62.9 MiB measured at one reader**. The dataset
> falsified the model, and nobody evaluated the model on the dataset.

## 3. Agreement is not confirmation unless the apparatus differs where the error lives

Two sittings that agree are evidence only if they differ in the dimension that
could carry the error. Before treating a second reading as independent, say out
loud **which assumption the two share**: the fitting window, the input
generator, the mirror of a library constant, the wrapper that reads the
counter, the machine.

Shared methodology produces shared bias, and the agreement then reads as
strength exactly when it is weakest.

> `19.15` and `19.18` agreed to within 1% on different harnesses, different
> containers and different command shapes. They shared the fitting window,
> which is where the error was.

## 4. Read the columns you were not investigating

A probe prints more than the number it was written for. Before closing a
reading, scan **every** column it emitted — wall clock beside resident, resolved
counts beside budgets, exit codes, spreads — and say whether each is consistent
with the story. A regression in the column nobody was studying is the cheapest
finding available and the one most reliably missed.

> The throughput collapse that identified the defect was printed in the same
> log lines, in the adjacent column, for three sessions.

## 5. An invariant asserted in prose is untested

This project has registers for *external* assumptions
(`postgres-invariants.md`, `runtime-invariants.md`) with re-verify steps, and
standing constraints with greps. It has nothing for the properties the code
asserts about **itself**.

So: when a doc comment or a design section states a property the code relies on
— *this read spans one block*, *exactly one buffer is live here*, *this is
zero-copy*, *this list is bounded by N* — either write the test in the same
change or say in the comment that nothing checks it. Prefer the test; these are
usually a unit test over an existing fixture, and they fail loudly the moment
something upstream changes.

**Watch for a value with two consumers.** The defect behind all of this was one
number serving as both a memory charge and a cut size: it was changed for the
first and the second followed silently. When a constant or a computed value is
read from two places for two purposes, name that in its doc comment and test
the consumer that is not the one you are changing.

## 6. A residual owes a name or an explicit "no name"

Never let a remainder be carried as a bare number across sessions. Either it is
attributed to a named term, or the record says **in those words** that it is
unattributed and what would attribute it. A number with no owner acquires a
false one — it gets a label, then a deficiency entry, then a slice planned to
price it.

And: a residual's *shape* is evidence. Whether it rises, falls or is flat along
each axis usually eliminates more candidates than another sitting would.

## 7. A comparison between two builds decays unless something expires it

A figure that subtracts one build from another names the change it means to
price — the census, the throttle. What it *measures* is everything that differs
between the two commits, and that set is not fixed: it grows every time the
tree moves and the pinned side does not. So the subtraction's honesty is a
condition rather than a property, and a condition nobody checks is one that has
already quietly failed.

**State the expiry when the comparison is built, and make it refuse rather than
drift.** Say out loud what would have to remain true for this difference to
still be the thing you named, then encode it — an ancestry check, a
declared-path check, a rebuild-and-restamp — so the figure is *refused* when it
stops holding instead of returning a plausible-looking number for something
else. A comparison whose caveat lives only in prose is one that will be read
without the caveat.

A pinned historical build is the common case and the worst-behaved. It cannot
be re-taken, so it accumulates every unrelated change by construction, and
nothing about running it looks wrong. Where the thing being priced is a
*settled* historical fact, prefer recording the number beside its mechanism
over re-measuring it every sitting: re-measurement buys apparatus consistency,
and once the gap is wide enough that is no longer what dominates the reading.

> The census-off binary carries a `.stamp` and is refused unless its commit is
> an ancestor of the one being measured with **no declared path changed in
> between**. The pre-throttle binary, one figure over, had no such guard. By
> the time anything ran it, "before the throttle" was 453 commits back and the
> column priced 453 commits of unrelated work under the throttle's name — the
> doc conceding it was "a whole-commit comparison" while the gap grew by two
> orders of magnitude. It surfaced as a crash only because a later apparatus
> rule handed it a flag it predated; a figure that merely *ran* would have
> published the wrong number (2026-09-13).

## Before planning a slice whose deliverable is a reading

Four questions. If the first two have answers, the slice may not be needed.

1. **Can the code answer this?** Arithmetic, a `grep` for a value's consumers,
   a slot count computed by hand.
2. **Can a direct instrument answer it?** A counter that prints the live bytes
   beats any subtraction of medians between two binaries, and it is usually
   minutes rather than an hour.
3. **What result would falsify the hypothesis?** A probe that cannot come back
   "no" is not an experiment. Write the falsification criterion into the probe's
   docstring before running it.
4. **Is the code under measurement believed correct?** Readings taken over a
   suspected defect price the defect. Repair first, then measure — even when a
   spec's binding order says otherwise, because that order was written before
   the defect was known.

## What this does not cover

How a figure is taken, what invalidates it, what may be published, the borrow
graph, the apparatus rules — all of that is
[`docs/design/measurements.md`](../../../docs/design/measurements.md) and
`scripts/measure.py --list`. Read those for the mechanics; read this before
believing what the mechanics produced.
