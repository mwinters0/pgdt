# P11.2 — The cross-major differ

What the next slices inherit. How the differ *works* is
[`architecture.md`](architecture.md), "The cross-major differ"; this doc is the
part that is not description — the calls that are not obvious from the code,
and what the slice found out about its own spec row.

## What landed

- `scripts/oracle_differences.py` — the classification, the alignment and
  apparatus guards, the committed-file reader/writer, and a report.
- `fixtures/oracle-differences.tsv` — **509 differences across 13–18, every one
  of them additive**, header row plus one line per moved cell.
- `scripts/test_oracle_differences.py` — 27 tests; the four `CommittedTree`
  cases are the slice's suite assertion.
- `comparison_oracle.escape_copy_text` / `format_tsv` — the encoder beside the
  decoder that was already there.
- `generate_fixtures.py` runs the check at the end of an oracle pass and fails
  on a difference that has not been filed.
- **I35** in [`postgres-invariants.md`](postgres-invariants.md), and that file's
  header now carries the mechanical half of the "walk this file" ritual.

The three transitions the file records are the ones the release notes name:
`numeric`'s infinities and the two multirange types in v14, `interval`'s
infinities in v17. No two majors disagree about an input both accept, which is
the evidence the phase's union rule was asserted on.

## The slice was mis-sized, and `11.2.1` is what came out of it

The spec row asked for a **three-way reconciliation** as well. Two of its three
directions join against the L2 comparison register, which **11.3** builds, so
they could not land here at any effort; the third — every major present in
`fixtures/` has an answer file — is a precondition the differ has to check for
itself anyway, and does (`alignment_problems`). The row is rewritten to what
landed, the remainder is `11.2.1` sitting after 11.3, and the reasoning is in
`history/2026-08-31.md`.

**11.1's notes and `test_comparison_oracle.py`'s docstring had both already
said the reconciliation needs the register first.** The fact was on the page
and the slice table was not changed to match it, which is the failure worth
remembering: a slice's row is the thing to re-read when the previous slice
files a prerequisite.

## What later slices inherit

**The verdict rule is strict in one place on purpose, and `11.3`–`11.5` are
what will test it.** A cell moving between two *rejections* with different
SQLSTATEs is classified non-additive, which would fail the check for something
that moved nothing a dump can hold. It has never fired across 13–18. If it
ever does — most likely when a new major lands and renames an error — the fix
is a third verdict, not a silent widening of `additive`, and
`test_a_changed_sqlstate_is_non_additive` is where the decision is pinned.

**An accepted literal's `output` spelling is in scope, and it is the half
`comparisons.tsv` cannot see.** 11.6's canonicalize-the-literal-once path
renders a literal into the form the *file* holds; a spelling that differed
between majors would be a rendering no single implementation could get right,
so it is classified non-additive rather than ignored. The spec had put output
spelling out of the differ's scope on the grounds that a fixture regeneration
shows it as a file diff; it is in scope now, for free.

**The alignment guard is the differ's own, not inherited.** The answer files
carry no case identifiers, so a row means what it means only by sitting at
`comparison_cases()`'s index. `test_comparison_oracle.py` asserts that too, but
the differ re-checks it before zipping rather than trusting a test that runs at
a different moment: mis-aligned files produce a *silent wrong answer*, not an
error, and that is the one failure mode this artifact cannot have.

**The apparatus check is why a hundred false breaks cannot happen.** Every
major's `meta.tsv` must agree on the pinned session GUCs and on `datcollate`;
an unpinned `DateStyle` in one container would move every timestamp spelling in
that major's file. That is a fault in the *generation*, so it is reported as a
problem rather than as 500 differences, and `apparatus_problems` is where a
future session adds a key when `comparison_oracle.SESSION_SQL` pins one more.

**Adding a major means adding it to `ROUTINE_VERSIONS` and nothing else.**
`majors()` reads the tree and sorts numerically, and the chain of adjacent
pairs follows. The new pair's differences are what the invariants walk reads
first (I35's `Re-verify`).

## Calls worth knowing about

**Two assertions, not one.** "The committed file is what a fresh computation
produces" and "no difference is non-additive" are separate tests, because
filing a real break must not silence the alarm about it. The script's exit code
is 1 for either.

**A header row, unlike the oracle files.** The server writes those and we write
this; it is the artifact a reviewer reads in a diff, and ten positional columns
are too many to hold. `read_committed` requires it, which doubles as a format
check.

**One row per moved cell, and a status change carries its output with it.** A
literal that starts being accepted moves `status` *and* `output` in the same
breath, so it is one difference rather than two — otherwise every additive
transition in the tree would be double-counted.

**The file sits at the top of `fixtures/`, not under a version**, because it
belongs to no single major. Both fixture walks recurse into directories only —
Rust's `all_fixtures()` and `test_comparison_oracle.py`'s version list — so a
file there is invisible to each, which is what makes the location free.

**The differ is Python, beside the oracle it reads.** No library code is
involved: the inputs are committed TSVs and the row alignment is guaranteed by
the Python case table, so a Rust differ would have to re-derive both. It is the
same shape as `deficiencies.py` and `measure.py --check`.
