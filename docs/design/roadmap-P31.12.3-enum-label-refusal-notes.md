# P31.12.3 — An enum's undeclared label refused where its labels are exact: notes

What the slices after this one inherit. The mechanism is 31.12's
([`roadmap-P31.12-field-refusal-notes.md`](roadmap-P31.12-field-refusal-notes.md));
why the enum was split out of 31.12.1 is
[`../status/history/2026-10-03.md`](../status/history/2026-10-03.md),
"31.12.1 lands without the enum". The entry it closes was `KD83`.

## What exists

- **`TypeKind::Enum` carries `exact`**, and `CompareKind::Enum` with it:
  `field_key` answers `Refused` for a miss only where it is set (I70), and for
  a text of `NAMEDATALEN` bytes or more whatever it is, no label being so
  long. Every other miss stays `Unparsed`.
- **What clears it**: a `CREATE TYPE … AS ENUM` fragment that is no plain
  string literal (an `E''` one), and `SpanBody::EnumLabelsUnread` — an
  `ALTER TYPE` holding the word `VALUE` that `parse_alter_type_add_value_body`
  does not read, so `RENAME VALUE`, `ADD VALUE IF NOT EXISTS`, a label it
  cannot lex, and an `ADD VALUE` a comment splits. It clears the named enum's;
  one naming no type declared so far, an `ADD VALUE` read included, clears
  every enum declared so far, since the server may resolve the name (an
  unqualified one under a `search_path`) where `find_type` does not. A later
  `CREATE TYPE` of the name is exact again. Nothing `pg_dump` writes clears
  it; `tests/preamble.rs` asserts the fixtures' `public.mood` exact at 13, 16
  and 18, plain and `--binary-upgrade`.
- **`parse_alter_type_add_value_body` reads `ADD` and `VALUE` as keywords**
  after the name, where it searched for `ADD VALUE` anywhere past it.
- **`info --detail` says it**: `enum: … (labels not read exactly)`, or
  `enum: (labels not read)`; `--map` names the unread statement.
- **`CACHE_FORMAT_VERSION` is 49**: the persisted spans carry `exact` and the
  new span kind. `PERSISTED_INDEX` re-pinned; `GOLDEN_ORDER`'s digest did not
  move.
- **Evidence**: `tests/statistics.rs`'s
  `a_parse_fails_at_an_undeclared_label_only_where_the_labels_are_exact`
  (both exact shapes refuse at `COPY` line 31; four inexact ones go on, the
  group unbounded), which fails with every miss refused; `map.rs`'s
  `an_alter_type_changing_labels_unread_leaves_them_inexact`; `predicate.rs`'s
  `a_field_postgresql_refuses_is_told_from_one_this_build_does_not_read`. The
  koji replica (PG16) answered every probe I70 lists.

## Findings

- **Four limits filed, each `(c)`, none a form `pg_dump` writes**: a filter
  on an inexact enum refuses a label the server may hold (`KD86`); code that
  is not an `ALTER TYPE` changing labels is not seen (`KD87`); the typed read
  emits an undeclared label (`KD88`); `ADD VALUE … BEFORE`/`AFTER` is folded
  as appended, misordering the label (`KD89`).
- **An inexact enum with no label read still resolves as an empty enum**, a
  text column, as before; only `info` tells the two apart.

## What the slices after this inherit

- **31.14's `strict`** decodes an enum field through the typed read, which
  refuses nothing (`KD88`); refusing it there needs `field_key`'s arm, labels
  and `exact` with it, and leaves a clean `strict` parse a full check except
  where `exact` is clear or `KD87` applies.
