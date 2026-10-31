# P38.5 — Each root's own rows in the answers: notes

What 38.5 leaves 38.5.1. The spec is
[`roadmap-P38-descendants.md`](roadmap-P38-descendants.md), "What is shown,
claimed and checked"; the reading is D130's, its TimescaleDB fact TS8. The
split from the spec's slice 5 is the 2026-10-31 history entry's.

## What 38.5.1 reads

- **`Answers::held`** (`pgdt-fixtures`' `answers.rs`): per hypertable the
  restore holds, its root read `ONLY` under `timescaledb.restoring = on`. It
  is what a hypertable's root holds where the server's own `only` answers
  nothing (2.28 on) and its read through leaves the root out. A hypertable read
  through is held to its `held` rows and its present chunks' `descendants`
  answers; its root read alone to `held`.
- **The internal compressed hypertables have `held` lines too** (0 rows),
  so an internal one the server refuses ("no dimension slices", 2.21–2.28) is
  checked the same way: at those pins its compressed relations inherit
  nothing, so it reads as its own block.
- **Where a `held` line is**: each hypertable the restore's catalog lists, so
  none in `schema-public` (no catalog restored), and at `exclude-chunks` only
  the two user hypertables, the materialization ones being excluded with
  `_timescaledb_internal`. 18's `exclude-chunks` holds `metrics`' stray row
  under `held` where its server's reads answer 0.
- **Each `held` line is its root's own block's rows** at every pin and flag
  set (`pgdt-mapping`'s `tests/timescaledb.rs`), so 38.5.1's root's own rows
  are the block `Table::read` already reads alone.

## Negative results

- **`--answers-only` reproduced every existing answer line byte for byte**
  over the committed dumps; the diff is the `held` lines alone. A dump and its
  answers come from one run either way: a full regeneration re-dumps, and its
  `creation_time`s and OIDs drift (38.4's notes).
