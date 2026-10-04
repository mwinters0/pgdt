# P31.21.1 — The strict listing's reasons split by what decides the input: notes

What the slices after this one inherit. The listing is 31.21's
([`roadmap-P31.21-strict-unchecked-notes.md`](roadmap-P31.21-strict-unchecked-notes.md));
why the split is
[`../status/history/2026-10-04.md`](../status/history/2026-10-04.md), "A
built-in type with no reader is outside the strict promise by scope".

## What exists

- **`pgtype::Unchecked` names what decides the input**: the restoring server
  — `Xml`, `Money`, `BaseType`, `RangeCanonical`, and now `Catalog` (the
  `reg*` types and `aclitem`) and `UndeclaredType` (a qualified name no
  `CREATE TYPE` declares, as an extension's, and an unqualified one
  `pg_catalog` holds no type under); scope — `NoReader`, now exactly the
  eight built-ins `tsvector`, `tsquery`, `pg_lsn`, `jsonpath`, `tid`,
  `oidvector`, `pg_snapshot`, `txid_snapshot`, every other `pg_catalog`
  base type at v13.23 and v18.6 having a reader or one of the grammars
  below; and pgdt's reading — `Unparsed` (a composite's fields or a range's
  subtype the preamble did not read, a multirange companion of the latter, a
  domain chain that never ends, a built-in with a grammar here written with a
  typmod no server holds) and `ArrayShape` (an array of arrays, `KD3`). A
  shell type is `BaseType`, being completed only as one.
- **`TextGrammar::RefusesAll`**: `pg_node_tree`, `pg_ndistinct`,
  `pg_dependencies`, `pg_mcv_list`, `pg_brin_bloom_summary`,
  `pg_brin_minmax_multi_summary` and `gtsvector` (I78), and an enum whose
  labels are exactly none (I70) — every non-NULL field refused, at the column
  or beneath a container. **`TextGrammar::RefusesNothing`**: `"char"`,
  `refcursor`, `xid`, `xid8`, `cid` (I79) — read, so the container around one
  is checked, and refused by nothing. Both come from `text_grammar`, so the
  check and the listing agree as before.
- **I78 and I79 are new**, I79 taking the v16 narrowing out of
  [`postgres-major-differences.md`](postgres-major-differences.md), and the roadmap's
  exceptions to "the newest major's `*_in`" and the manual's "Where
  PostgreSQL majors differ" name `xid`, `xid8` and `cid` beside `oid` and
  `line`.
- **`CACHE_FORMAT_VERSION` is 59**: a block's `checked_in_full` now covers
  these fields. No persisted byte moved; both pinned digests re-pinned to
  the version alone.
- **Evidence**: `pgtype.rs`'s
  `a_type_refusing_every_value_or_none_has_a_grammar_saying_so` and
  `what_a_strict_parse_leaves_unchecked_is_named_where_it_lies` (every
  reason); `predicate.rs`'s `a_strict_check_finds_a_refusal_anywhere_in_a_field`;
  `tests/statistics.rs`'s
  `a_strict_parse_refuses_the_fields_a_default_one_leaves_to_a_query`, whose
  dump gained a `pg_node_tree` column, refused at a value, and an `xid` one
  spelled as only v13 to v15 read it.

## Negative results and limits

- **No fixture holds a column of any type this slice reads**: a
  `pg_node_tree` column can only be made from a catalog, and the rest would
  each be a fixture column whose only evidence is that nothing is refused.
  The source reading is the evidence (I78, I79).
- **An unqualified name `pg_catalog` holds no type under is
  `UndeclaredType`**, where 31.21 called it `NoReader`: a hand-written dump
  may name a type its search path finds, and pgdt cannot tell which.

## What the slices after this inherit

- **A built-in gaining a reader leaves `NoReader`'s list in `unread`** as well
  as gaining its arm; `NoReader` is the scope boundary the roadmap names.
