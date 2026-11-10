# P33.13.1 notes — a strict check holds no tree of a value it reads

**A strict parse checks a nested, `json` or `jsonb` field off its bytes as
`Unescaped` reads them**, a part at a time through its containers' input
grammars, with no copy of the field, no key and no tree
([`decisions.md`](decisions.md), "D143"). No reading was taken; what proves it
is `crates/pgdt-query/tests/decoded_copies.rs`'s
`a_strict_parse_holds_no_tree_of_a_nested_value` — a `smallint[]` of many
elements, a `text[]` and a composite of escaped texts, a `json` and a `jsonb`
of escaped strings, a default and a strict parse each, and a refused element
still refused — which fails on the parent, its first leg's strict parse
holding its field's tree, many times the field.

- **`Source`, `Bytes`, `Lexer`, `Part`** (`pgdt-codec`'s `value/source.rs`):
  a text read forward a byte at a time, with a peek and a fork; a part of a
  literal — an element, a field, a bound, a multirange's member — read off its
  container's source by its grammar's lexer, dynamically dispatched, since a
  type's depth is the plan's.
- **The input grammars are walks** (`value/nested/input.rs`): `walk_array`,
  `walk_record`, `read_range`, `walk_multirange` and `walk_int2vector`, each
  handing its caller every part as it reads it; `parse_*` collect them.
  `input/prior_shape.rs` keeps the slice scanners they replaced, and
  `input/differential.rs` holds the two to the same answer over generated and
  damaged literals.
- **`walk_json`** (`value/json.rs`) is `json_in`'s grammar, and with `jsonb`
  `JsonCursor`'s refusals beside it, each counted only as far as the cursor
  reads — a value nested past `JSONB_MAX_DEPTH` stopping it — so
  `jsonb_reads` answers what `parse_jsonb` does, held to it over a generated
  corpus and nestings either side of the depth (`json/tests.rs`). The
  `numeric` bounds both read are `numeric_stores`.
- **`walks` and `walk_refused`** (`grammar/check.rs`): the plans `field_refused`
  reads a byte at a time, and its answer off a byte iterator; `refused` is
  `checked_key`'s refusals over the walks, held to it over generated nested
  literals and over each `COPY`-escaped
  (`a_walked_check_refuses_what_the_keys_refused`). `checked_key` is left
  building a range bound's key, which `range_in` compares.
- **`check_row`** (`statistics/gather/strict.rs`) walks a field whose plan
  `walks` off `Unescaped`, unescaping it only to name it once refused, and
  asks a refused one's bytes for UTF-8 apart.

## What 33.13.3 inherits

- **A strict check still collects a scalar part's text**: a scalar position
  other than a text or `bytea` (`leaf_refused`), one a `TextGrammar` reads
  (`refused`) and a range's two bounds (`read_range`), each into a `String`
  grown through the global allocator; a scalar column's escaped field is
  unescaped whole as before. Such a part is bounded only by its line: every
  number's `*_in` takes leading zeros and blanks at any length, a date's
  `ParseDateTime` skips blanks outside its work buffer, and `pg_dump` itself
  writes a `polygon`, `path` or `bit varying` element of any length.
- **A `box[]` nested in a composite is collected whole**
  (`TextGrammar::BoxArray`, `refused`), a container D143's walk was owed.

## What 33.14 inherits

- **A column summing, or counting unrepresentable values, still unescapes a
  long escaped field whole** (`ColumnGatherer::observe`, D142): a container's
  tier walks `decode_*`, the output grammar, which is still the slice
  scanners. Its walk is 33.14.1's, the first to consume it, and 33.16's; the
  gatherer reading over it is P40's.
- **`walk_json`'s stack** of open containers holds a bit a level, so a document
  nested as deep as its line is long holds an eighth of it.

## What 33.4 inherits

- **A strict parse holds no copy or tree of a nested, `json` or `jsonb`
  value, and no line of one past its chunk**, but the scalar parts and the
  nested `box[]` 33.13.3 inherits; a parse whose column sums or counts
  unrepresentable values holds its field whole, and an escaped value's copy,
  drawn from the ledger
  ([`roadmap-P33.19-fold-notes.md`](roadmap-P33.19-fold-notes.md)).

## Negative results

- **No count pair was taken**: a strict parse's check, a filter's literal and
  a `parse_*` call now walk a dynamically dispatched source, none of them on
  a default parse's or a query's hot path, and P33's spec adopts no pricing
  of a hot-path change ([`decisions.md`](decisions.md), "D108").
- **A walk generic over its source** was not built: an element's source wraps
  its container's, so a plan nested at runtime depth instantiates without
  bound.
