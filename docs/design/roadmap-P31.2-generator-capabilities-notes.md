# P31.2 — Generator capabilities: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"Version-conditioned schemas", "The session-setting axis", "The emitter
register" and "A known failure is asserted to fail". No product code changed
but two `deficiency:` markers.

## What exists

- **Version sidecars**: `generate_fixtures.schema_files` loads
  `fixture_schema_<schema>.<major>.sql` after the base file at that major and
  above, and refuses a sidecar name whose middle is not a major. The one
  sidecar is `fixture_schema_types.18.sql`, `t_v18_columns`, read by
  `tests/preamble.rs`'s `v18_s_column_shapes_keep_their_type_and_their_displaced_collation`.
- **Session-setting variants** are a third flag-set shape, `Setting(name,
  value)`: `ALTER DATABASE … SET` before one plain dump, `RESET` after it.
  `types/bytea-output-escape` and `types/timezone-monrovia`.
- **The six option flag sets** the spec names: `types/quote-all-identifiers`,
  `types/extra-float-digits-0`, `edge_cases/rows-per-insert` (`=2`),
  `edge_cases/on-conflict-do-nothing` (with `--inserts`, which it needs),
  `edge_cases/disable-triggers` (with `--data-only`, I31), and
  `objects/include-foreign-data`, over a `file_fdw` table `objects.imported`
  the schema adds. It is in `objects` rather than `edge_cases` because
  `objects` already holds the FDW objects and is the DDL inventory schema; its
  data file is written by the schema's own `COPY … TO '/tmp/…'`, inside the
  throwaway container.
- **The value-form half**: `emitter_register.VALUE_FORMS`, six spellings —
  bytea's octal and backslash escapes, an offset with seconds and the era it
  moves a year across, `float4out`'s and `float8out`'s `DBL_DIG`-digit forms —
  each the file's bytes after `COPY`'s escaping and naming the flag set that
  reaches it; a selector naming no flag set is a problem. All six are covered
  at every major. **Typmod-selected spellings are this half's too and not
  listed**: 31.4's.
- **The strict known-failure table** is `pgdump_query/tests/known_failures.rs`:
  a row is a `KD<k>`, a fixture, a **case** (what a correct reading shows) and
  a **control**, a fixture the case passes on, so a case failing for an
  unrelated reason reads as a broken row. A sweep the same defect trips keeps
  a strict exclusion of its own naming the entry, asserting the fixture still
  fails there, and loses it with the entry's last row (`value_oracle.rs`'s
  `EXCLUSIONS` is one).
- **Regenerated**: `types`, `objects` and `edge_cases` at all six majors;
  every other diff in their files is D69's four movers. The oracles, the
  floor and `partitions`/`statistics` were not re-taken.

## Findings

- **`KD71`, filed**: under `--quote-all-identifiers` every column resolves as
  in `default.sql` but `t_delimiter.v_box_domain_array`, whose domain's base
  is written `"box"` and so is split at `,` (seven elements, the first `(1`).
  Slice 31.9.
- **`KD72`, filed**: `--extra-float-digits=0` writes `DBL_MAX` as
  `1.79769313486232e+308`, read as an infinity; `statistics_never_change_an_answer`
  found it as `COUNT(DISTINCT v_double)` reading 5 with statistics and 3 without.
  `real`'s `3.40282e+38` stays under `FLT_MAX`. Slice 31.10.
- **Read correctly, no entry**: every Monrovia offset, the era-crossing year
  included, reads as its instant; the three v18 shapes keep type and
  collation; `--rows-per-insert` and `--on-conflict-do-nothing` tile and map
  to one `InsertRun` per table. `KD1`, `KD65` and `KD67` reproduce as filed.
- **Literals the new fixtures reach**: `\nSERVER `, `\nOPTIONS (\n    ` and
  (13–16) `ALTER FOREIGN TABLE ` from the foreign table; ` NO INHERIT` and
  `COMMENT ON CONSTRAINT ` at 18. Still uncovered from 31.1's KD rows: `ALTER
  FOREIGN TABLE ONLY ` (a per-column foreign option) and v18's
  `contype = 'n'`, `conkey IN (` and `conname IN (`, which need an inherited
  not-null under `--binary-upgrade`.

## What 31.3 inherits

- The value oracle reads the `types` database, which now loads the sidecar at
  18, so `t_v18_columns` is one of its tables there.
- Each `types` variant's typed read is to be held to the same `values.tsv`.
  `bytea-output-escape` refuses until 31.8 and `extra-float-digits-0` reads
  `±DBL_MAX` as infinities until 31.10: both are known failures, and the
  check against them is the known-failure table's until their slices land.
