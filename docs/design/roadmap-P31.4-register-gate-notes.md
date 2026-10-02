# P31.4 — Schema content for every row, and the register a gate: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"The emitter register" and "What the register finds, and when the phase
ends". No product code changed but one `deficiency:` marker.

## What exists

- **The `emitters` schema**, `scripts/fixture_schema_emitters.sql` with a v14
  sidecar (`SET COMPRESSION`), each object named beside the literal it
  reaches: base types over `LANGUAGE internal` I/O functions, a canonical,
  a `subtype_diff` and an opclass range, a domain `CHECK`, reloptions, a
  check-option view, forced row security, both replica identities, a
  statistics target, a populated materialized view, a missing value, two
  dropped columns, a dropped foreign column and a foreign column option, and
  the filed defects' shapes: an unlogged table, an inheritance child, a typed
  table, a composite with a dropped attribute. Its flag sets are `default`,
  `binary-upgrade`, `clean`, `data-only` and fourteen `pg_dumpall` runs.
- **Cluster globals** are `generate_fixtures.CLUSTER_SCRIPTS`: a script run
  in `postgres` before the schema loads, here
  `fixture_schema_emitters_cluster.sql` — a commented tablespace with
  options, and a database named `pgdt-emitters` — and what
  `drop_fixture_db` removes after.
- **A `pg_dumpall` flag set is `Dumpall(flags)`**, replacing the `None`
  sentinel, so `pg_dumpall` takes options of its own.
- **`objects` carries a set for each option that selects or omits objects**,
  combining options that do not conflict and gating one a major lacks by its
  minimum, so a set passes an option without isolating its effect. Its
  pre-existing fixture files were not rewritten: a regeneration moves only
  D69's four movers in them, checked on 17's `default.sql`, and each was
  restored. Its `--filter` file is written by the schema script.
- **Dispositions are `emitter_register.EXEMPTIONS`**, four reasons, rules in
  the module docstring. **The gate** is `test_emitter_register`'s
  `test_every_row_is_held_by_a_fixture_or_exempt`. At every major every
  literal is held by a fixture but one, I52's; 29 or 30 options are exempt;
  thirteen value forms, the seven typmod-selected ones held by
  `types/default`.
- **A `NoOutput` needle is resolved by the extraction**, the choice 31.1's
  notes left here: `--extract` finds it in the program's source at each major
  holding the option and writes an `evidence` row into `emitters.tsv`, and
  the join refuses an exemption whose row is missing or holds another needle.
  The other choice, evidence pointing at something committed, had nothing
  committed to point at.
- **`known_failures.rs`** gained two cases, `NoTableSpan` and
  `DatabaseListed`, and a row per filed defect the new fixtures expose.
- **I52**, new: the `conkey IN (` fix-up is written only against an older
  server, so no fixture holds it.

## Findings

- **`KD73`, filed**: a `--create` dump reconnects after `DATABASE
  PROPERTIES`, and the second `\connect` opens a second segment of the same
  name, so `emitters/dumpall-binary-upgrade` resolves 37 of 40 columns
  `NotDeclared` at every major. `dumpall-clean`'s `template1` shows the same
  repeat with nothing in it. Slice 31.11.
- **Reproduced as filed**: `KD1` (both `--disable-triggers` sets), `KD61`,
  `KD64`, `KD65` (unlogged), `KD66`, `KD68`, `KD70`.
- **Read correctly, no entry**: every base-type column (opaque text, I11), the canonical range (`[1,5]` dumped `[1,6)`), the other two
  ranges, the domain, the materialized view, the missing value and the
  dropped columns, and, under `--binary-upgrade`, the inheritance child and
  the typed table, whose full column lists resolve. Every new fixture tiles
  and the whole suite passes over them.

## What the slices after this inherit

- **Each fix slice deletes its rows** in `known_failures.rs`, its fixtures
  already in the tree; `emitters/binary-upgrade` is the control for `KD64`
  and `KD70`, `emitters/dumpall` for `KD73`.
- **No exemption names a `KD<k>`**, so the spec's end criterion, the
  register green with none naming a P31 entry, holds already; what is still
  open is the known-failure table's P31 rows and the checklist's.
- **`--disable-triggers`, `--inserts` and its siblings now reach `pg_dumpall`
  output** (`emitters/dumpall-data-only`) and `--attribute-inserts` an
  `objects` set (`omit`): `INSERT` shapes P8's row reader meets.
- **The suite's slowest tests walk every fixture**:
  `every_fixture_prunes_to_the_rows_it_returns_unpruned` took 197 s in a
  205 s run with the 31.4 tree (`runs/31.4/nextest-1.log`).
- **Regenerating** is `uv run generate_fixtures.py --schema emitters
  --skip-oracle --skip-floor --skip-values`, ten seconds a major; `--schema
  objects` rewrites the old files too, for their movers alone.
