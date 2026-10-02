# P31.8 — `byteaout`'s escape form: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"The session-setting axis"; the entry it closes was `KD67`.

## What exists

- **`decode_bytea` reads both of `byteaout`'s forms**, told apart by the `\x`
  prefix no `escape` text opens with (I56). `decode_bytea_escape` reads exactly
  what `byteaout` writes, a printable byte in octal (`\101`) refused as D55
  refuses every `*_in`-only spelling, and keeps only a leading `limit` bytes
  while checking the whole text. `render_bytea_escape` is its inverse, used
  only to spell a literal.
- **An equality literal is rendered in both forms** (`predicate.rs`,
  `Spellings`, D56), so `=`, `!=`, `IN` and a group's dictionary match a value
  in whichever form the file holds, still one byte comparison a form per row.
- **Gathering puts an `escape` value's head in `hex`** (`gather::Canonical::of`),
  `BYTEA_ESCAPE_HEAD_BYTES` of it, enough that a cut value reads as no whole
  value exactly as the value itself does, so an `escape` column stores the
  bounds and row order its `hex` twin does without copying a long value whole
  (D76).
- **Typed output renders `hex`** whatever the dump held (D66), under
  "Decisions worth another look".
- **`CACHE_FORMAT_VERSION` was bumped**, both pins re-pinned beside it: the
  `escape` fixture's `bytea` groups now store bounds.
- **Evidence**: `tests/decode.rs`'s
  `an_escape_output_dump_reads_and_filters_as_its_hex_twin` at every major;
  `value_oracle.rs` and `known_failures.rs` lost their `KD67` rows, and with
  the second its only case, `TypedRead`; unit tests in `decode.rs`,
  `predicate.rs` (`a_bytea_literal_matches_its_value_in_either_output_form`)
  and `gather.rs` (`a_bytea_column_gathers_alike_in_either_output_form`).

## Findings

- **`KD67` understated its defect.** Equality never decodes, so before this
  slice an `=` or `IN` on a `bytea` of an `escape` dump answered no row
  rather than refusing, and a dictionary holding the column's `escape` text
  answered the same way for a whole group; only a decode — the column
  materialized, or an ordering term — refused. The register's entry named the
  refusal alone.
- **The empty `bytea` is the empty text in `escape` form**, so an empty field
  now reads as the empty value in a `hex` dump too, where it never occurs.

## What the slices after this inherit

- **A setting variant's reading needs every comparison path, not only the
  decoder**: a `*_out` with two spellings reaches `equality_comparison`'s
  canonical arm and `gather::Canonical` as well. 31.10's lossy float
  spelling (`KD72`) is a decoded kind and meets neither.
