# P31.25 — The bit-string spellings in the `types` fixture: notes

What the slices after this one inherit. The check it gives a fixture to is
31.17's ([`roadmap-P31.17-bit-in-notes.md`](roadmap-P31.17-bit-in-notes.md));
the entry it closes was `KD95`.

## What exists

- **`fixture_schema_types.sql`'s `t_bit`**, at every routine major and in
  every `types` flag set: columns declared `bit`, `bit(3)`, `bit varying`,
  `bit varying(5)` and `"bit"`, each value as long as its column admits
  (a 40-bit `bit varying`, a 10-bit `"bit"`, an empty `bit varying`).
- **What `pg_dump` writes for them, at 13 to 18 and under
  `--quote-all-identifiers` alike**: `bit(1)`, `bit(3)`, `bit varying`,
  `bit varying(5)` and `"bit"` (I8). A bare `bit` is never written for a
  column, the grammar having given it its length before `format_type` reads
  it, so `text_grammar`'s bare-`bit` arm reads hand-written DDL only.
- **Evidence**: `tests/pgtype.rs`'s
  `a_bit_string_s_length_is_read_off_the_spelling_pg_dump_writes` — a strict
  parse of every `types` flag set but the one holding a refused float, the
  spellings asserted, `t_bit` checked in full with nothing unchecked; and a
  field one bit long or short, or holding a digit no bit string does, put
  into `default.sql`'s first row and refused naming its column and declared
  type. Reading `"bit"` as `bit(1)` fails it at the first flag set.
- **No code changed and `CACHE_FORMAT_VERSION` stays 64**; the persisted-index
  digest moved with the fixture and is re-pinned alone. `values.tsv` gains
  only `t_bit.id`'s rows: a bit string maps to text, so the value oracle reads
  none of it.

## Negative results

- **The comparison oracle still asks no bit literal**: a bit string orders
  nothing here and its literal is not checked, so there is no comparison to
  hold to the server.
