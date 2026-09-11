# P19.17.1 — a killed leg is a reading, and it bars publication

What `19.18` inherits. The standing rule is beside the apparatus it belongs to
([`measurements.md`](measurements.md), "The apparatus"); this doc holds the
premise re-test the row was held open for, what the harness actually does now,
and the two things a re-taken `reserve` sitting has to know.

## The premise re-test, which the row itself asked for

`19.19`'s notes left `19.17.1` with an open question — *the acceptance run
killed nothing at any registered limit, so its premise is worth re-testing
before it is built*. It was re-tested and **holds**, on three legs, none of
which is a prediction that something will die:

- **The contradiction is in the tree, not in a forecast.** `RESERVE_LIMITS`'
  own registration called an OOM kill "a reading rather than an apparatus
  failure" and sat beside a `time_run` that raised on any non-zero exit,
  losing the figure. A document that says the opposite of the code it
  registers is wrong whether or not the case arises.
- **The harness could not tell an OOM kill from a parse error at all**, which
  is a defect in what it *reports*, not only in what it survives. Verified
  directly, in the apparatus rather than from the source: the same wrapper
  around a 200 MiB allocation in a 64 MiB container and around `false` both
  exit **1**, and the only thing separating them is `memory.events`'
  `oom_kill`, 1 against 0.
- **`19.11`'s gate is not otherwise answerable.** Its precondition is *a
  `--figure reserve --alone` sitting completes with no killed leg*. Under
  fail-fast a sitting with a killed leg does not complete at all, so "completed
  with no kill" and "completed" were the same sentence and the gate checked
  nothing. It is now a question the run answers — in the banner, in `raw.json`,
  and in the exit code.

The headroom the shipped arrangement leaves is the fourth leg and the weakest,
so it is not leaned on: after `19.19` the thin cells are 1 GiB at **11.6%** and
1.5 GiB at **10.3%** over three reps
([`roadmap-P19.19-per-file-term-notes.md`](roadmap-P19.19-per-file-term-notes.md)),
against a criterion of 20%. That is close, not doomed, and a slice built on "it
will probably die" would have been built on a guess.

## What the harness does now

**The oracle is the container's own `memory.events`**, read inside it after the
timed command. `measure.OOM_ORACLE` is appended in `time_run`, **not** in
`_script`: what a figure records as its shape stays exactly what was measured,
and every shape is covered by construction rather than by thirty branches
remembering to carry it. It saves `$?` first and re-exits with it, so a
container does not come back green because `cat` succeeded, and it writes to
stderr, where `parse_reported` — which reads a binary's *stdout* — cannot mistake
`oom_kill 0` for an instrument's `key=value` line.

`parse_oom_kills` answers `None` where the counter was unreadable, and that is
**not** zero. "Nothing was killed" and "the harness cannot tell" are different
answers and a caller that conflated them would re-create the defect one level
up; an unreadable counter leaves the old fatal behaviour in place, with the
reason said.

**The licence is `measure.KILL_TOLERANT`, and only the reserve figure's
flagless family is in it.** That family's subject is a rule that aims resident
*at* the allocation, so a leg against its own ceiling is the arrangement under
test. Everywhere else a kill is the apparatus failing and still ends the
figure — `parallel-peak-rss` once measured 3067 MiB in a 3072 MiB container,
and a harness-wide licence would have turned that into a footnote. The
mechanism legs fall inside the licence without being named: they *are* the
flagless shape with one mechanism changed, so the prefix covers them.

**A censored cell is not a number.** The killed rep enters neither `readings`
nor `rss`; it is counted in `Session.killed` and recorded in `raw.json` with
everything that could still be read off it under names that cannot be mistaken
for readings — `maxrss_bound_kib` and `seconds_to_kill`, both lower bounds on a
run that stopped. The renderer prints `**OOM-killed**, n rep(s)` with the
arrangement the run reported before it died, and `head —` in place of a
headroom.

**A partly-killed leg leaves the fit too**, which is the subtle half. Its
surviving reps are exactly the ones that stayed *under* the ceiling, so their
median understates and their worst is not the worst; a line through them reads
low, which is the number that makes a too-small reserve look adequate. The cell
shows them with the kill beside it and the fit names the leg it dropped.

**A killed leg bars the figure**, in the run header and in a note above the
table — a section is what gets pasted, so a banner at the top of `tables.md` is
not carried by the one table somebody copies out. The sitting exits non-zero:
"the sitting finished" and "the figure may be published" stopped being the same
claim.

## One defect found on the way, fixed here

**`--render` could not re-render a `reserve` sitting at all.** It looked its
figures up in `FIGURES`, which is the set the *document* carries, so a
diagnostic sitting of an **untaken** instrument came back `unknown figure(s)`.
That is backwards: an untaken instrument is taken repeatedly while its renderer
is still being written, which is exactly what `--render` is for, and `reserve`
is the only figure that can carry a censored cell. It now resolves through
`SELECTABLE_BY_ID` — a sitting is re-renderable exactly when `--figure` could
have taken it. Not filed out-of-band: without it this slice's own claim that a
re-render reproduces the censored cells was unreachable for the one figure that
can have them.

## What `19.18` inherits

**Two things it must not re-derive.**

- **The three mechanism legs are still dead**, arithmetically, and this slice
  changed nothing about that. `19.17.1` makes a sitting survivable; it does not
  re-aim one. Re-aiming or dropping those legs is `19.18`'s first act, before
  the hour is spent.
- **A censored sitting is still a sitting.** If a leg dies, the run now comes
  back with every other family measured and the kill named, so the finding is
  *readable* — but the figure may not be published from it, and the reserve
  constant `19.16` picks may not be read off a table with a censored cell in
  it. What a kill licenses is stating the kill as the finding, which is what
  the fourth amendment's acceptance gate asks for in the first place.

**No library code moved**, so no figure was made stale by this slice beyond
`session-drift`, which has been red on `scripts/measure.py` since the harness
took the derived direction of the borrow graph.
