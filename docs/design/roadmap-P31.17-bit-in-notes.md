# P31.17 — `bit_in`'s and `varbit_in`'s grammar: notes

What the slices after this one inherit. The check it extends is 31.14's
([`roadmap-P31.14-strict-notes.md`](roadmap-P31.14-strict-notes.md)), and the
shape it follows is 31.16's
([`roadmap-P31.16-json-in-notes.md`](roadmap-P31.16-json-in-notes.md)).

## What exists

- **`decode::bit_in`**, a whole port of `bit_in` and `varbit_in` (I73, one
  `pg-refuses` marker), the typmod's length included. It answers read or
  refused, nothing between.
- **`pgtype::TextGrammar`**, the input grammar of a built-in held as its text
  and ordered by nothing, and **`pgtype::text_grammar`**, which reads it off a
  declared type through any chain of domains: `bit` alone is `bit(1)`,
  `"bit"` has no length, and `varbit` is now a catalog name for `bit varying`
  (`CATALOG_NAMES`). Its one variant is `Bit`.
- **Where the grammar is read.** A column's own type ordered by nothing is
  `ComparisonPlan::Refused`, which carries nothing, so `gather::field_checks`
  reads the grammar off the column's declared type beside the plan
  (`FieldCheck::grammar`) and checks a field by `predicate::grammar_refuses`.
  A position beneath a container carries it on
  `NestedCompare::Uncomparable::grammar`, set by `nested_position`, so
  `checked_key` reads an array's elements and a composite's fields by it.
  The register's comparisons are unchanged: `bit` still orders nothing here.
- **`CACHE_FORMAT_VERSION` is 54**: a block's `checked_in_full` now covers its
  `bit` and `bit varying` fields. No persisted byte moved and neither pinned
  digest did.
- **Evidence**: `decode.rs`'s
  `a_bit_string_is_refused_only_where_bit_in_refuses_it` (every case put to
  `pg_input_is_valid` under its type on the koji replica, PG16);
  `pgtype.rs`'s `a_bit_string_s_grammar_is_read_off_every_spelling`;
  `predicate.rs`'s `a_strict_check_finds_a_refusal_anywhere_in_a_field`
  (top level, a domain, an array, a composite's fields); `tests/statistics.rs`'s
  `a_strict_parse_refuses_the_fields_a_default_one_leaves_to_a_query`, whose
  dump gained a `bit(3)` column.

## Negative results and limits

- **No fixture holds a `bit` or `bit varying` column**, at any major, and the
  comparison oracle asks no bit literal; I8 had claimed `t_type_spelling`
  carries `"bit"`, which it never has. The gap is `KD95`, for 31.25, and
  I73's observations stand in for the fixture until then.
- **A bit string has no order here**, so a filter on one is refused past
  `=`/`!=` as before, and its literal is not checked; a `default` parse and a
  query read the column as its text.

## What the slices after this inherit

- **31.18** reads the geometric types as `TextGrammar` variants, an array of
  `box` included
  ([`roadmap-P31.18-geometric-in-notes.md`](roadmap-P31.18-geometric-in-notes.md)).
- **31.21** names what a `strict` parse left unchecked; a column with a
  `TextGrammar` is checked, so it is not among them.
