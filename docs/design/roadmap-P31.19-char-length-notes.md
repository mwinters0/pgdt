# P31.19 — A `varchar(n)` or `char(n)` field past its length refused: notes

What the slices after this one inherit. Why the length binds `default` too is
[`../status/history/2026-10-03.md`](../status/history/2026-10-03.md), "A
clean `strict` parse promises what the field and its declaration decide"; the
shape it follows is 31.15's
([`roadmap-P31.15-numeric-typmod-notes.md`](roadmap-P31.15-numeric-typmod-notes.md)).

## What exists

- **`decode::char_typmod_refuses`**, `varchar_input`'s and `bpchar_input`'s
  test (I75, one `pg-refuses` marker): past `n` characters, counted in UTF-8,
  anything but `0x20` refuses. A field no longer in bytes than `n` is not
  counted.
- **The length travels in `CompareKind::Text { length }` and
  `CompareKind::PaddedText { length }`**, read off a `character varying(n)`'s
  or `character(n)`'s typmod by `pgtype::char_length`, and in
  `NestedPlan::Text { length }` beside the Arrow type, as a decimal's
  precision does (D39). `datafusion_order` keeps it, `PaddedText` becoming
  `Text` of the same length. `name`, `text`, `bpchar` and a bare
  `character varying` carry none.
- **Bare `character` is `character(1)`**: `builtin_name` gives the SQL word
  with no length a typmod of 1, as `gram.y` does, and `bpchar` none (I8).
- **Where it refuses**: `predicate::field_key` (an ordering term's field, a
  nested leaf, the strict check's `checked_key`), `field_refused` (a scalar
  under `strict`), `gather::BoundsGatherer::observe` through
  `Canonical::Text`/`PaddedText` (a data-level parse keying it, `default`
  included), and the typed read (`batch::text_refused`, the top-level
  zero-copy path and a nested leaf), each but the read failing as 31.12's
  refusals do and the read with `FieldDecode`. **A literal is never held to
  it**: `literal_key` and `equality_comparison` read one of any length, the
  server coercing it with no typmod.
- **`ignore`** reads such a field as written, in the query and the parse,
  the parse keeping no statistic of its group (D103).
- **`bounds_set_keyed_by` matches text kinds whatever their length**, the
  length moving no key: a `SchemaMode::Strings` term still reads a
  `varchar(n)` column's `Text` set (D79).
- **`CACHE_FORMAT_VERSION` is 56**: a block's `checked_in_full` now covers
  the length, and a block holding a field past it fails a data-level parse.
  Neither pinned digest moved.
- **Evidence**: `decode.rs`'s
  `a_character_field_is_refused_only_past_its_length_but_for_blanks` (every
  case put to `pg_input_is_valid` under `varchar(3)` and `char(3)` on the koji
  replica, PG16); `predicate.rs`'s
  `a_strict_check_finds_a_refusal_anywhere_in_a_field` (a domain, an array);
  `pgtype.rs`'s `text_like_types_map_to_utf8view_deliberately` and
  `every_spelling_of_a_built_in_is_that_built_in` (`char` and `character` as
  `character(1)`); `tests/decode.rs`'s
  `a_character_field_past_its_length_is_refused_wherever_it_is_read` (the
  parse, the read, an ordering filter, `ignore`, a strict parse beneath an
  array and a composite, and a literal past the length).

## Negative results and limits

- **A field longer than `n` only by blanks is read with them** (`KD96`),
  where the server cuts it to `n`: cutting it would give a `varchar(n)` a key
  no `SchemaMode::Strings` term shares; `KD96`'s marker says what the fix is.
- **`=`, `!=` and `IN` never decode the field**, so they do not refuse one
  past its length, as they do not a `numeric(p,s)` past its precision.
- **A `char(n)` field shorter than `n` is read unpadded**, as before
  (`KD96`); its comparison trims both sides, so only the emitted text
  differs.

## What the slices after this inherit

- **31.20**'s `NOT NULL` is the other declaration a field is held to under
  `default`; the column's comparison and plan are where this slice put the
  length, and the same resolution reaches a domain's.
- **31.21** names what a `strict` parse left unchecked; a `varchar(n)` or
  `char(n)` column is checked, so it is not among them.
