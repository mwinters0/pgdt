# P31.7 — Three preamble point fixes: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"What the register finds, and when the phase ends"; the three entries it
closes were `KD65`, `KD66` and `KD68`.

## What exists

- **`classify_statement` reads `CREATE UNLOGGED TABLE` and `CREATE FOREIGN
  TABLE` as tables** (I54), through the same `parse_create_table`; a foreign
  table's `SERVER` and `OPTIONS` follow its list and `INHERITS`, so nothing
  reads them.
- **`SpanBody::AlterTypeDropAttribute`**, beside `AlterTypeAddValue`, is
  `--binary-upgrade`'s `ALTER TYPE … DROP ATTRIBUTE`, and
  `fold_alter_type_drop_attribute` removes the placeholder field from its
  composite. I5 now carries the composite half.
- **`parse_connect` reads `\connect -reuse-previous=on "dbname='…'"`** (I55),
  undoing the identifier's doubled `"` and the connection string's `\`
  escapes. A connection string holding any key but `dbname` stays no boundary.
- **`CACHE_FORMAT_VERSION` was bumped**, both pins re-pinned beside it.
- **Evidence**: `tests/preamble.rs`'s
  `unlogged_and_foreign_tables_declare_their_columns`,
  `a_composite_s_dropped_attribute_is_dropped_under_binary_upgrade` and
  `a_connection_string_connect_opens_its_own_database`, at every major, and
  unit tests in `preamble.rs` and `map.rs`. The four `known_failures.rs` rows
  went, and with `KD68`'s its only case, `DatabaseListed`.

## Findings

- **A foreign table's `--binary-upgrade` list holds its dropped column's
  placeholder** (`emitters.external`), as a plain table's does. Its `ALTER
  FOREIGN TABLE ONLY … DROP COLUMN` is not folded, nor is a plain table's:
  the `COPY` header names the columns read (I5), so the placeholder is never
  looked up.

## What the slices after this inherit

- **31.11 (`KD73`) now reaches one more database**: `pgdt-emitters`, once
  merged into `template1`, is `\connect`ed twice in
  `emitters/dumpall-binary-upgrade` at every major, as `pgdt_fixture` is.
