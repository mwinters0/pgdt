# P11 — Typed predicates: notes

What the next phase inherits from a phase that ran in twenty-three slices. **This
is an audit, not a transcription** (`../process.md`, "Consolidation at wrap"):
`architecture.md` already holds every mechanism P11 built, filed by subject, so
this doc carries only what subject-filing has no home for.

Where the mechanisms are: "Predicates", "Ordering operators compare typed",
"Equality is typed too", "Nested columns compare structurally", "The nested
literal codec", "A filter term is parsed for two audiences", "`--where` builds
an expression out of those terms", "Fixtures", "The comparison oracle", "The
cross-major differ", "The register-to-oracle reconciliation" and "The register
against the oracle's answers", all in
[`architecture.md`](architecture.md). The external facts are I33–I46 in
[`postgres-invariants.md`](postgres-invariants.md).

## What the wrap moved, and where

Three things the slices learned had not reached `architecture.md` and are there
now, in the same change as this doc:

- The rejected **one-major glibc witness file** — the smaller diff the fixture
  family switch was measured against — in "Fixtures".
- The rejected **exact Kleene evaluation with no `exact` flag**, in
  "Predicates".
- The `json[]` announcement being the one rule asserted by unit test rather
  than against committed bytes, and why neither a fixture nor an oracle case
  can close it, in "Nested columns compare structurally".

Everything else the twenty-three slice notes carried was already filed by subject
or is provenance.

## What the phase leaves open

**`KD7` and `KD10`, both `(c) unowned`, and neither owned by P11** — so the
wrap's re-homing pass had nothing to move. `KD7` survives at one statement, a
column that *states* a collation this build does not implement; the roadmap's
Future item "Collation-aware comparison" is what would promote it, and the
`pgcollate` spike (`CLAUDE.local.md`) has already demonstrated its feasibility.
`KD10` is a column whose declared type this build models no comparison for
answering `=` bytewise, which is wrong for the geometric types; a dump whose
queried columns are geometric promotes it.

**Three of `KD7`'s four original statements closed as properties rather than as
code**, and that is the phase's own correction to its spec: a database
collation no plain dump records (I32), reached through a bare column and one
level down through a `jsonb` string leaf, and `json`, which the server does not
order at all. None has a remedy anybody could write, so none is a deficiency.
The reasoning is beside the mechanism, in "Ordering operators compare typed".

## What the next phase should know

**The per-row field walk is unchanged and now costs more**, since a
disjunction short-circuits on the first term that *succeeds*. It is filed where
it will be read, in
[`roadmap-P7-scan-performance-inbox.md`](roadmap-P7-scan-performance-inbox.md),
along with the two fixes that were considered and not taken here.

**`interval`'s Arrow mapping is unblocked and was deliberately not taken.** I4
was corrected during 11.5 — `pg_dump` *does* pin `IntervalStyle`, at every
supported major — which removes the reason `interval` is a `Utf8View`. That is
filed in
[`roadmap-P12-adbc-type-floor-inbox.md`](roadmap-P12-adbc-type-floor-inbox.md),
whose ADBC floor is where the mapping decision belongs.

**The comparison register is a per-column vector an embedder can read**, and
P10's pruning question — "is a bound over this column sound" — is exactly what
it answers. That is filed in
[`roadmap-P10-row-group-statistics-inbox.md`](roadmap-P10-row-group-statistics-inbox.md).

**Adding a comparison costs a six-major oracle regeneration, and only that.**
`uv run generate_fixtures.py --skip-dumps` rewrites the eighteen oracle TSVs in
about 35 seconds and touches no `.sql` file; the belief that a case addition
rewrites all 109 fixture files is wrong and cost one slice a piece of evidence
it could have had cheaply. A *schema* change is the expensive one, at about 80
seconds and every file in the tree.

## Where the plan was weak

Eight slices earned a third level, and every one of them split at the same seam:
a **mechanism** paired with the **evidence or the parse it needs**. 11.2 was
written to reconcile against a register a later slice creates; 11.5 paired four
scalar comparisons with `jsonb`'s container walk; 11.6 paired a register
correction with a new mechanism over every `CompareKind`, and 11.6.1 in turn
paired that mechanism with a preamble parse and a cache-format bump; 11.10
paired a structural walk over parts the register already had with a
canonicalization that needed the subtype's successor function; and 11.11
carried a library change whose two agreeing cases no fixture could exercise.

The lesson is `process.md`'s "Size a slice by its review, not by its scope",
and what P11 adds to it is the *shape* to look for at spec time: a row that
names a mechanism **and** the artifact that will check it is two slices. The
day-by-day reasoning for each split is in `../status/history/` — 2026-08-31,
2026-09-01 and 2026-09-02.
