# P31.26 — A NULL beneath a `NOT NULL` domain refused: notes

What the slices after this one inherit. The column-level half is 31.20's
([`roadmap-P31.20-not-null-notes.md`](roadmap-P31.20-not-null-notes.md)); the
forms and their evidence are I76, which now also records `record_in`'s call
and `range_in`'s bound tests, with the replica's observations of each.

## What exists

- **`pgtype::Position<T>`** — `plan` and `not_null` — is a position a NULL
  can sit at: `NestedPlan::Array` and `Record` and `NestedCompare::Array` and
  `Record` hold one per element or field. `not_null` is
  `domain_not_null` of the position's declared type, set where
  `resolve_user_type`, `resolve_array`, `array_comparison` and
  `comparison_user_type` build the node. `NestedPlan::array` and `record`
  build nullable positions, and are what tests and the census's outer levels
  use.
- **A range's bound is no `Position`**: `range_in` hands an infinite bound to
  no input function, so a range over a `NOT NULL` domain may be unbounded.
- **Where a NULL there is refused**, but under `ignore`: the typed read
  (`batch::append_nested`, via `ListParts::not_null` and
  `StructParts::not_null`), failing with `FieldDecode` on the whole literal as
  31.19's nested refusals do; a comparison reading the field
  (`predicate::nested_key`, `Side::Field`), which is any operator on a nested
  column; a filter literal holding one, always (`PredicateValueDecode`, its
  accepted form saying so); and a strict parse (`checked_key`), as
  `FieldRefused`. `TextGrammar::BoxArray` carries its element's `not_null`,
  the one array a strict parse reads by a grammar rather than a plan.
- **A default data-level parse keys no nested column**, so it refuses none of
  these, as it refuses no nested leaf past its length.
- **`Unchecked::DomainNotNullBeneath` is gone**, and with it the strict
  listing's `NOT NULL`-beneath line, which also named a range's bound
  wrongly. A position read by nothing — a base type's array whose delimiter
  the preamble could not read — is still listed under its own reason.
- **`CACHE_FORMAT_VERSION` is 65**: a block's `checked_in_full` now covers
  these NULLs. Neither pinned digest moved.
- **`pgdt info --json`'s `plan`** writes an `Array` or `Record` position as
  `{"plan": …, "not_null": …}`.
- **Evidence**: `pgtype.rs`'s
  `a_not_null_domain_beneath_a_container_marks_its_position`; `batch.rs`'s
  `a_null_at_a_not_null_position_is_refused_but_under_ignore` (the census's
  two-dimensional plan, `()`, a range's infinite bound); `predicate.rs`'s
  `a_strict_check_finds_a_refusal_anywhere_in_a_field`;
  `tests/decode.rs`'s
  `a_null_beneath_a_not_null_domain_is_refused_wherever_it_is_read` (a
  domain over one, a composite's field, an array of composites, a domain
  over a composite; every surface, `ignore`, the strings mode, a literal, and
  four that refuse nothing a query reads).

## Negative results and limits

- **No fixture holds such a domain beneath a container**: `pg_dump` writes no
  NULL a restore would refuse, so the evidence is hand-written dumps and the
  replica's observations in I76.
- **A typed read names the whole literal**, not the position, as every nested
  refusal does.
