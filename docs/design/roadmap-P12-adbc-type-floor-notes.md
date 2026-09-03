# P12 — The ADBC type floor: notes

What the phase left behind that subject-filing has no home for. The spec is
[`roadmap-P12-adbc-type-floor.md`](roadmap-P12-adbc-type-floor.md); every
mechanism this phase built is described by subject in
[`architecture.md`](architecture.md) and is **not** repeated here:

| Mechanism | Section |
|---|---|
| The rule, its stances, `KD13` | "The floor: the ADBC driver's answer bounds ours" |
| The oracle: sweep, columns, refusals, `test_adbc_floor.py` | "The ADBC floor oracle" |
| `interval` → `Interval(MonthDayNano)`, `int2vector` → `List<Int16>` | "Type resolution" |
| `interval_parts`' one walk, render-back's third outcome, `KD8` | "Decoders and render-back" |
| `int2vector`'s literal, the one container with no leaf rule | "The nested literal codec" |
| `NestedCompare::Int2Vector`, `anyarray` polymorphism | "Ordering operators compare typed" |
| The fixture table and its five rows | "Fixtures" |

Artifacts: `scripts/adbc_floor.py`, `scripts/floor_mapping.py`, their two test
modules, `fixtures/<13–18>/adbc/floor.tsv`, the `adbc-driver-postgresql` pin in
`scripts/pyproject.toml`, `decode.rs`'s interval pair, `nested.rs`'s
`int2vector` codec, `Error::FieldRender`, and I47.

## The phase's largest obligation evaporated on evidence, and that is the finding

The inbox recorded *"carry the declared PostgreSQL type name on every column"*
as this phase's work. Against release 24 it is not work at all: no modeled
field carries any metadata, and we stamp `arrow.uuid` where the release hands
back opaque bytes — so the metadata floor is met by construction and a join on
it would check something that cannot fail. What produced that answer was
deciding *which* driver the floor is (D1), not any measurement of ours. The
obligation is real against unreleased main and is filed as a watch item where
the rule lives, not as work anybody deferred.

The same shape held over the whole catalog: 58 of 82 rows a major are placed by
the oracle file's own columns with no hand-written line, so the phase's
coverage was the whole declarable type set while its work was 24 rows.

## What the slicing got right, and the two places it did not

D11's seam — evidence before mechanism, and a new module apart from a rework of
a tested path — held. Two rows were mis-drawn, both in the same direction, and
both because `builtin_scalar` is one exhaustive `match` rather than three:

- **12.4's row was two-thirds delivered before the slice opened.** The
  comparison register's `interval` arm is the *same tuple* as the resolution
  arm, so 12.3 could not change the mapping without landing it. Nothing was
  skipped; the row named work that the arm's shape had already forced.
- **12.6 owned an oracle case D11 never named.** `oracle_register.py` reads one
  arm per declared base name, so adding an `"int2vector"` arm fails the
  register-to-oracle join until a case names the type. Whether that case should
  have been 12.5's was reviewed and answered — an arm's oracle case belongs to
  the slice that writes the arm ([`architecture.md`](architecture.md), "The
  register-to-oracle reconciliation";
  [`../status/history/2026-09-02.md`](../status/history/2026-09-02.md)) — and
  the requirement it carries is a property of the *case*, not of its slice: it
  must contain a pair on which element-wise and lexical order disagree, because
  a plan that fell back to text agrees with the server on everything else.

**The lesson for the next type-closing phase:** a slice that adds a
`builtin_scalar` arm lands the Arrow type, the comparison plan, the oracle case
and — where the type is a container — a `NestedPlan` table entry, whatever the
slice table says. Only the *fixture* and the render-back rework separate
cleanly, which is exactly where 12.3/12.4 and 12.5/12.6 were cut.

## Facts filed forward

- **P6** has an inbox entry: the floor is checkable and stating it is P6's to
  do, with the two qualifications any wording inherits
  ([`roadmap-P6-embeddable-engine-inbox.md`](roadmap-P6-embeddable-engine-inbox.md)).
- **P10's `interval` entry was deleted by 12.3**, its premise reversed: there is
  no longer a per-column type for a census to decide. The census argument it
  carried is now a rejected alternative in "Type resolution".

## Apparatus costs, so nobody re-derives them by running it

Six majors, on this machine: the floor sweep is **30 s** and needs no fixture
DDL, so `--skip-dumps --skip-oracle` is a meaningful invocation; an oracle
re-take is **63 s**; regenerating one schema's dumps is **43 s**. Scope a
regeneration to the schema that moved — a wider one adds a `\restrict` token to
every file it rewrites, and `binary-upgrade` adds OID drift on top, both of
which bury the change being reviewed.

## Measurement

The phase moved no figure and wrote no acknowledgement. All thirteen were
already stale for the reasons [`../status/STATUS.md`](../status/STATUS.md)
gives; the Rust changes add a `Result` discriminant and two match arms to the
per-field decode and output paths, which is neither byte-identical input nor
unreachable code, and "small" is not evidence. `--check` still reconciles
thirteen markers.
