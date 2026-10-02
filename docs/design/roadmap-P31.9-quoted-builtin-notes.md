# P31.9 — A quoted built-in read as its bare name: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"What the register finds, and when the phase ends"; the entry it closes was
`KD71`.

## What exists

- **`pgtype.rs`'s `is_box`** is the `box` test `resolve_array` and
  `array_comparison` make on a domain walk's terminal, reading it through
  `builtin_name` as every other built-in spelling is read: unqualified, bare in
  any case or quoted in the catalog's (I8). `"box"[]` and an array over a
  domain `AS "box"` are refused for their element as `box[]` is.
- **`CACHE_FORMAT_VERSION` was bumped**, both pins re-pinned beside it:
  `types/quote-all-identifiers`'s `v_box_domain_array` is now gathered as the
  text column it reads as, so the persisted index moved.
- **Evidence**: `tests/pgtype.rs`'s
  `every_column_resolves_alike_with_every_identifier_quoted`, every table of
  the `types` schema at every major, its Arrow field, outcome, literal form and
  ordering verdict held to `default.sql`'s; unit tests in `pgtype.rs`.
  `known_failures.rs` and `value_oracle.rs` lost their `KD71` rows.

## Findings

- **Those two tests were the only built-in spellings compared outside
  `builtin_name`.** Every other reader of a declared built-in — `map_builtin`,
  `comparison_walk`, `extension_for` — already went through it, which is why
  every other column of the flag set resolved alike before this slice.
- **A quoted spelling in another case is not `box`**: `"BOX"` names no
  `pg_catalog` type, and its array resolves as an unknown element's does.
