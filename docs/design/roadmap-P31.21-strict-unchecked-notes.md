# P31.21 — What a `strict` parse leaves unchecked, named per dump: notes

What the slices after this one inherit. Why a strict parse names it is
[`../status/history/2026-10-03.md`](../status/history/2026-10-03.md), "A
clean `strict` parse promises what the field and its declaration decide"; the
check it reports on is 31.14's
([`roadmap-P31.14-strict-notes.md`](roadmap-P31.14-strict-notes.md)). The
external facts are I77 (a `CHECK`) and I76 (a domain's `NOT NULL`).

## What exists

- **`pgdump_query::strict_unchecked(header, metadata, database)`**, in
  `gather.rs` beside the check: a `StrictUnchecked` per `COPY` block —
  `declared` (whether the dump declares the table), `columns` (each
  `UncheckedColumn`: column, path, declared type, `pgtype::Unchecked`) and
  `checks` (each `UncheckedCheck`: name, the ancestor declaring it). It
  resolves as `field_checks` does and reads each column's grammar through the
  one helper both call, `pgtype::column_grammar`, so the report and the check
  are one reading of one plan.
- **`pgtype::unchecked_positions`** names, for one column, every position a
  field is read at by no grammar — a `Refused` plan with no grammar, a nested
  plan's `Uncomparable` position that is neither `json` nor carries one, an
  inexact enum, an `Unanswerable` canonical range — and every domain at or
  beneath it declaring a `CHECK`, and every `NOT NULL` domain beneath a
  container (`KD97`). Paths are `NestedCompare::walk`'s.
- **`Unchecked`'s reasons**: `Undeclared`, `Xml`, `Money`, `BaseType`,
  `RangeCanonical`, `NoReader`, `LabelsInexact`, `DomainCheck`,
  `DomainNotNullBeneath`, each worded by `describe`.
- **The preamble reads every `CHECK` before the data** (`check_constraints`,
  off `top_level_words`, which now yields a quoted identifier as its text so
  a constraint's name survives): `TableDef::checks` (a table constraint, a
  column's, a typed table's column option; `CheckConstraint`'s name and `NO
  INHERIT`), `TableReference::Check` for `ALTER TABLE … ADD [CONSTRAINT <n>]
  CHECK …;` alone, and `TypeKind::Domain`'s `check`.
  **`DatabaseMetadata::table_checks`** walks parents as `column_not_null`
  does, skipping a parent's `NO INHERIT` one and merging a name met twice,
  which is how `--binary-upgrade`'s child re-declares its parent's.
- **`pgdt parse` and `pgdt info` list it under every block**, whichever mode
  built the cache, as `unchecked by a strict parse:` lines, a closing line
  counting the blocks holding any; `--json` carries each block's
  `unchecked` in `resolution`; `info --detail` marks a domain `CHECK`;
  `--map` labels the `ADD CONSTRAINT … CHECK` span. The parse's long help
  says so.
- **`CACHE_FORMAT_VERSION` is 58**: the persisted metadata and spans changed
  shape. `PERSISTED_INDEX` re-pinned; `GOLDEN_ORDER`'s digest did not move.
- **Evidence**: `preamble.rs`'s `every_form_of_check_is_read_and_nothing_else`
  and `a_check_is_found_on_the_table_and_through_its_ancestors`; `pgtype.rs`'s
  `what_a_strict_parse_leaves_unchecked_is_named_where_it_lies`;
  `tests/preamble.rs`'s
  `a_strict_parse_names_what_it_leaves_unchecked_in_the_emitters_schema`
  (every major, default, `--binary-upgrade` and `--data-only`); `pgdt`'s
  `a_strict_parse_names_what_it_leaves_unchecked_and_info_the_same`.

## Negative results and limits

- **A post-data constraint is not named per table**: a `CHECK` held apart
  (I77), a unique, primary or foreign key. The manual names the class.
- **A partition's bound is not named**, though a `COPY` into the partition,
  or routed through its root, is checked against it; STATUS's "Decisions
  worth another look" carries the call.
- **A spelling a reader here cannot read at all is not named per column**
  (`KD90`): it would name every typed column.
- **A built-in type with no reader here** — `tsvector`, `pg_lsn`, `jsonpath`,
  `xid` — is named as `NoReader`, beside `xml` and `money`, rather than filed
  as a gap in the promise; STATUS's "Decisions worth another look" carries
  the call.
- **A column whose plan `resolve_columns` refuses whole is named at the
  column**, where `comparison_for` alone would name the position: an array of
  an opaque element (I22) is `""`, the reason its element's.

## What the slices after this inherit

- **31.22** changes no position here: `KD90` is per value.
- **31.26** closes `KD97` by carrying the flag into `NestedPlan`; it then
  drops `Unchecked::DomainNotNullBeneath` and `domains_beneath`'s arm naming
  it, or the listing goes on naming a checked position.
- **A new reader of a type held as its text** removes it from the listing by
  adding a `TextGrammar`, `column_grammar` being the one place both read.
