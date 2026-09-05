# P7.12 — the sweep pair, the koji regression run, and the fold-in

What the next slice — and the phase wrap — inherits from publishing seventeen
figures at once. The numbers themselves are
[`measurements.md`](measurements.md); this doc is what a session cannot re-derive
from them.

## The pair took two attempts, and the first one is the useful half

The 02:11Z pair finished with `legA=1 legB=0`. Leg A, pre-registered as the
publishable leg, met a ~60% `cpu_busy_pct` burst and lost seven figures to the
contention gate; leg B, starting thirteen minutes later, ran clean.

**Leg B was not promoted, and that is the durable decision.** It was a complete,
uncontended sweep sitting on disk, and promoting it would have cost nothing
visible. The roles were fixed in the handoff before either leg ran precisely so
that this choice could not be made on how the legs read — which is the same
argument the session stamp rests on. Spending the pre-registration once teaches
the next session that it can be spent, and the cost of not spending it was one
hour of a quiet machine.

The burst's cause was never identified. It matches no systemd timer on this
machine and leg B ran clean immediately after. That is worth knowing rather than
solving: the gate caught it, which is the property being relied on, and a
mechanism that only works when the cause is known is not the mechanism that was
built.

## `git_head()` is read once per invocation, and a pair is two invocations

**Nothing may be committed between the legs of a pair.** `emit()` calls
`git_head()` once at its top, so leg A and leg B stamp whatever HEAD is when
each starts. A commit landing between them gives the two legs different commits
— and `session-drift` is *defined* as the drift between two sittings on the same
commit, so the drift table would silently become a measurement of whatever was
committed.

This was not written down for the first attempt, and it is not visible from the
orchestration script. It is now in the second attempt's handoff and in
`STATUS.md`. Uncommitted files are fine and always were: `git_head()`'s dirty
flag counts only changes under a figure's declared paths, which no document is.

The corollary a later fold-in needs: **the acknowledgement register lives in
`scripts/acknowledged.py` on purpose**, so deleting spent entries during a
fold-in touches no path a figure declares and does not re-stale the stamp it was
just given. Editing `measure.py` in the same commit would have.

## What the fold-in actually costs is the prose, not the tables

Sixteen tables paste in mechanically. What took the work was that every figure's
surrounding argument quotes its own numbers, and four arguments changed shape
rather than magnitude:

- **`census-arrays` moved by a factor of five** (1.49 µs → 287 ns a row), which
  is the largest correction the register has carried. The knock-on is that the
  census's two tiers are now within a factor of four of each other — the
  pre-filter is 25% of an inspected row against 4% before — so
  `census-brace-free`'s "one tier, not two" conclusion is gone, replaced by a
  two-tier statement. A number moving does not usually invalidate a sentence;
  this one did.
- **`census-attribution`'s gap fell from +0.874 s to +0.033 s**, inside the
  drift figure. The finding survives — the untyped baseline is file-dependent —
  but the table's purpose inverts: it used to attribute an obvious gap, and it
  now supplies a gap nobody could otherwise see.
- **`predicate-terms` measures a different mechanism** than the previous stamp
  did, 7.7.1 having landed in between. Its term-count axis is flat now (+0.05 µs
  from one term to five, against +0.37), which is a cleaner result than the
  figure was built to produce.
- **The I/O-defaults ceiling fell 8.9% → 5.8%**, because 7.13/7.13.1 took the
  parse below the device by more than they took the device. Every lever priced
  against it was already declined; they are declined harder. This number is
  quoted in `architecture.md` twice and in the compatibility matrix, and it is
  the one most likely to be re-quoted stale.

**Two headings had to be rewritten** because they stated findings that moved:
`census-arrays` ("more than triples" → "adds half again") and `nested-end-to-end`
("13.5 µs" → "6.5 µs"). Both keep their leading clause, because `map.rs` and
7.6's notes cite them by truncated prefix. `census-brace-free`'s heading was made
number-free, which its own prose had already argued for.

**`measure.py`'s `section=` strings were deliberately left stale.** Updating them
would edit a `session-drift`-declared path in the fold-in commit and re-stale the
stamp immediately; the doc already records that the harness's heading string is
free to lag. A later harness change can carry them.

## The P7 spec was not touched, and that is a rule rather than an oversight

`roadmap-P7-scan-performance.md`'s lever table still reads `16.5×` for the
`INSERT` fast path and `0.98 µs` for the field split. Those record the stake the
phase *committed* to, which is the only baseline the finished phase can be
measured against (`../process.md`, "Progress lives in STATUS, never in the
spec"). `--check` names the spec as a consumer of five figures; the exemption is
deliberate and is stated at the `KD9` paragraph in `measurements.md`.

## What the wrap inherits

- **Every figure is current in fact**, for the first time in the phase, and the
  document has no cross-sitting absolute in it. A wrap that re-takes anything is
  re-taking a figure that was measured hours earlier.
- **`mimalloc` is ahead on all three shapes** — 0.97×/0.97×/0.99×, every spread
  overlapping the reference's. The adoption decision stands and is not reopened
  here, but the sentence supporting it has weakened, and it is filed under
  STATUS's "Decisions worth another look" for the maintainer rather than settled
  by the session that noticed it.
- **`KD9`'s residual grew in relative terms** — 4.3× → 4.9× warm — with nothing
  about the `INSERT` path changing. The `COPY` path got faster and the `INSERT`
  path did not, because what 7.13/7.13.1 removed is per byte of file and both
  shapes read the same 3.00 GiB. An entry priced as a ratio can move when its
  denominator does, which is a reason to re-read the entry at a wrap rather than
  to re-price it.
- **The per-block-quadratic control row's "before" leg is not to be read.** Its
  two reps are 0.058 s and 0.004 s — a first-run effect on a 2 MB file — so its
  median is an artifact of having two reps. Nothing depends on it; the row exists
  to hold the byte count fixed.
