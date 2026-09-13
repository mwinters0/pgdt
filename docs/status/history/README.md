# History

Dated notes, for two purposes only:

- What a future session should pick up mid-work (state that isn't obvious
  from `../STATUS.md` alone — e.g. a half-finished approach, a known-bad
  path already tried).
- A discovery that changed the plan. The decision goes in the doc that holds
  the decision; its evidence is a figure in `measurements.md`, a test, an
  invariant, or a `runs/` artifact, cited from there. The entry here points at
  those. It is not where reasoning lives.

**A day's entry is short** — pointers and settled facts, about a hundred lines
at most. Reasoning that must outlive the day goes beside its mechanism.

**Entries are deleted at each keystone** once nothing outside this directory
cites them; git holds them. A live document that needs a fact from an entry
takes the fact, not the pointer.

Not a changelog. Routine progress already reflected in `../STATUS.md`
doesn't get an entry here.

One file per day: `YYYY-MM-DD.md`. Multiple entries on the same day share
the file as separate `##` sections.

## Write the end state, not the path to it

An entry exists to be read tomorrow, not to record today. Write the settled
facts as they stand when the day closes, in one pass, as if looking back:

- **No supposition → correction chains.** If an assumption was overturned,
  state only what is true now. The discarded assumption earns a line only where
  a future session would otherwise re-derive it — "the obvious approach is X;
  it fails because Y."
- **No "resolved:" / "original note:" pairs, and no in-progress status that has
  since resolved.** Rewrite the section in place when the question is answered
  and delete the question.
- **No framing relative to what someone believed earlier** — "confirmed",
  "corrected", "turns out", "more actionable than expected". State the fact.
- **Rejected alternatives stay only when they carry forward**: the cost of the
  road not taken, or what closing a known gap would require.
- **Supersede rather than append.** A later day's entry states what is true
  then; earlier entries are not rewritten to match. But an earlier entry that
  has become actively misleading gets corrected or deleted, not left as a trap.

This is CLAUDE.md's "document what is, not what was" applied inside a file whose
name is a date. The date records *when* something was learned; it is not licence
to narrate *how*.

## An entry carries yesterday's truth

An entry says what was true on its date and is not maintained afterwards.
Comparing it against today is what `git log` is for, and that is the property
that makes writing one affordable: it is written in a single pass, as the day's
settled facts, without classifying each sentence against a future that has not
happened.

**So a citation in a dated entry is a statement about the day it was written,
not a live pointer.** Its target may since have been renamed or deleted — a
keystone deletes phase docs and is forbidden to rewrite history, so it leaves
such citations dangling by rule rather than by oversight — and that is not a
defect and is not repaired.

The exception is a citation that **never** resolved: a relative path written at
the wrong depth, a section named by a title its target has never carried. That
was not true yesterday either, so nothing protects it, and it is fixed like any
other typo. `git log` is what tells the two apart.

This is what "actively misleading" above does and does not reach. An entry
whose *claims* have become traps is corrected; a pointer that has merely
outlived its target is not one, given this understanding.
