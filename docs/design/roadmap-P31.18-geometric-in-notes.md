# P31.18 — The seven geometric types' input grammars: notes

What the slices after this one inherit. The mechanism it extends is 31.17's
([`roadmap-P31.17-bit-in-notes.md`](roadmap-P31.17-bit-in-notes.md)), the
check 31.14's
([`roadmap-P31.14-strict-notes.md`](roadmap-P31.14-strict-notes.md)).

## What exists

- **`decode::point_in`, `line_in`, `lseg_in`, `box_in`, `path_in`,
  `poly_in` and `circle_in`**, whole ports over shared `pair_decode`,
  `path_decode` and `pair_count` (I74, one `pg-refuses` marker each). Each
  answers read or refused, nothing between. Their numbers are read by
  `decode::strtod`, glibc's `strtod` in the C locale, hexadecimal and
  `nan(…)` included, and refused out of range as `float8in_internal` refuses.
- **`pgtype::TextGrammar::Geometric(Geometric)`**, read off the declared type
  through domains by `text_grammar`; a geometric type written with a typmod
  has none, no server holding one.
- **`TextGrammar::BoxArray`**: a `box[]` column resolves to text with its
  plan refused (I22), so `text_grammar` gives it a grammar of its own, read
  by `checked_key` as an array of `box` positions. A `box` position beneath a
  container carries `box_in`'s grammar from `array_comparison`'s opaque arm,
  and `predicate::array_delimiter` splits its array at `;` through
  `nested::parse_array_delimited` — so a composite's `box[]` field is now read
  at `;`, where it was read at `,` and a hand-written quoted element was
  refused that PostgreSQL reads.
- **`line_in` refuses only where every supported major refuses**: v13 builds
  a two-point line by other arithmetic (I74), and the port runs both, reading
  where either does. The roadmap and the manual's "Where PostgreSQL majors
  differ" say so beside `oidin`'s.
- **`CACHE_FORMAT_VERSION` is 55**: a block's `checked_in_full` now covers its
  geometric fields. No persisted byte moved and neither pinned digest did.
- **Evidence**: `decode.rs`'s
  `a_geometric_field_is_refused_only_where_every_major_refuses_it` (144 cases
  observed on 13.23, 16.15 and 18.6) and
  `a_geometric_number_is_read_as_strtod_reads_it`; `pgtype.rs`'s
  `a_geometric_type_s_grammar_is_read_off_every_spelling`; `predicate.rs`'s
  `a_strict_check_finds_a_refusal_anywhere_in_a_field` (each type, a domain,
  `point[]`, `box[]` as a column and as a composite's field);
  `tests/statistics.rs`'s
  `a_strict_parse_refuses_the_fields_a_default_one_leaves_to_a_query`, whose
  dump gained a `box[]` column.

## Negative results and limits

- **A hexadecimal number is read in a geometric field and not in a `real` or
  `double precision` one**, where `float_in` leaves it `Unparsed`, a D55
  shortfall: here the grammar has to know where each number ends, so
  `strtod`'s whole prefix is ported. `decode::strtod` agreed with glibc's over
  595,823 generated spellings; the harness was not kept.
- **No geometric value has an order or a decoder here**: a filter on one is
  as before (`KD10`), and only a `strict` parse reads its grammar.
- **An array of a user base type in a container's check is 31.24's**
  (`KD94`): found while threading `box`'s delimiter.

## What the slices after this inherit

- **31.21** names what a `strict` parse left unchecked; every built-in held
  as its text but `xml` and `money` is now checked.
