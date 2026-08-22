# History

Dated notes, for two purposes only:

- What a future session should pick up mid-work (state that isn't obvious
  from `../STATUS.md` alone — e.g. a half-finished approach, a known-bad
  path already tried).
- A discovery that changed the plan. The resulting decision belongs in the
  relevant design doc (`docs/design/*.md`); the entry here is where the
  reasoning/evidence behind it lives, referenced by a pointer from that
  doc — not inlined into it.

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
