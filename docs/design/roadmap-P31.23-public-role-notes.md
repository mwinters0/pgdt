# P31.23 — A role named `PUBLIC` kept: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"What the register finds, and when the phase ends"; the entry it closes was
`KD91`.

## What exists

- **`insert_role` drops exactly `public`, and `insert_tablespace` exactly
  `pg_default`**: the pseudo-role reaches it only as the bare keyword, which
  `parse_ident` folds, and no role can be named `public` in either spelling
  (I85), so a role `"PUBLIC"` and a tablespace `"PG_DEFAULT"` are kept from
  every source — a TOC header's raw `Owner:` and `Tablespace:` fields, `OWNER
  TO`, a grantee and `SET default_tablespace`.
- **`CACHE_FORMAT_VERSION` was bumped**, both pins re-pinned beside it with
  their digests unchanged: no fixture holds such a name, but a cache of a dump
  that does persisted an inventory without it.
- **Evidence**: `tests/map.rs`'s
  `a_role_named_public_and_a_tablespace_named_pg_default_are_kept`, over
  `pg_dump` 16.15's output of a server holding both names, and the unit test
  `a_quoted_public_role_and_pg_default_tablespace_are_kept`, which also holds
  a quoted `"public"` dropped.

## Findings

- **The fixtures were not extended**: the names change no span, no type and
  no column, so the one dump the integration test holds is the evidence, and
  regenerating six majors' fixtures for it would buy nothing a test reads.
- **A tablespace `pg_default` that is not the built-in one** needs
  `allow_system_table_mods` and a rename of the built-in away; I85's scope
  limit names it, and it is not filed.
