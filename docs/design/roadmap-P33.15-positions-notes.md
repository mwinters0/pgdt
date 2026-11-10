# P33.15 notes — a normalized type's key is positions

**A `jsonb` and a multirange term hold positions, not a key of the field**
([`decisions.md`](decisions.md), "D152"). No reading was taken; what proves
it is `crates/pgdt-query/tests/decoded_copies.rs`'s
`a_normalized_term_holds_its_positions` — a `jsonb` object of many pairs
whose keys carry an escape, written out of storage order and escaped by
`COPY`, and an `int4multirange` of many members written out of order, each
of eight MiB, ordered and compared by a query projecting neither — which
holds each under its key factor and fails on the parent, each leg holding
the field's key tree, over a hundred MiB.

- **`jsonb_field_order`** (`pgdt-codec`'s `value/json/order.rs`), reached
  as `key::order::jsonb_order`: the field walked whole by `JsonWalk` first,
  so it is refused as `parse_jsonb` refuses it, then compared as `Jsonb::cmp`
  compares two, each object's keys held as `u32` positions while it is on
  the comparison's path, an array's elements counted by a skip and read
  from where each starts. Held to `field_key` and `compare_keys` over
  seeds damaged a byte at a time, in place and escaped
  (`json/order/tests.rs`).
- **`order_multirange`** (`nested/key/multirange.rs`): two passes of
  `walk_multirange` — every member keyed and counted, then each non-empty
  one's opening held — sorted and merged in place as
  `canonical_multirange` does, a union holding the openings of the members
  whose bounds it keeps. Held to `nested_key` by `key/field/tests.rs`, its
  multirange seeds widened to unsorted, overlapping, adjacent and empty
  members over `int4`, `numeric` and `date`, and `jsonb[]` added.
- **`KeyFactor`** (`value/factor.rs`, where a family states its own
  without reading `crate::key`), and **`key_factor`, `nested_key_factor`**
  (`key/factor.rs`): `JSONB_KEY_FACTOR` and `MULTIRANGE_KEY_FACTOR` beside
  their keys, a plan's the largest of its positions'.
- **`FieldText::offset` and `seek`**, over `Unescaped::offset` and
  `Unescaped::resumed` (`pgdt-core`): a place in a field as `COPY` wrote
  it, read again from there. `walk_range`'s bracketed part is
  `walk_bracketed`, a multirange's member.
- **The term** (`predicate/eval.rs`): `ResolvedTerm::jsonb` leaves an
  escaped `jsonb` field as `COPY` wrote it under an ordering term and `=`
  alike; `eval/tests.rs` holds it to `eval_value` over the unescaped text.

## What 33.16 inherits

- **The key factors are positions alone.** Beside them a multirange keys up
  to four members' ranges at once as it sorts and merges, each bound a key
  of its subtype — a few words for every built-in multirange, a bound's
  text for a range over `text` — and a `jsonb` comparison collects one
  number's text at a time. Folding a query's factors is 33.16's.

## What 33.4 inherits

- **An `IN` list over `jsonb` or a multirange** (`Lookup::Decoded`,
  `Lookup::Nested`) still unescapes each field and builds its key, the
  list's binary search reading it whole.

## Negative results

- **No count pair was taken**: the slice changes neither what `field_key`
  reads with nor what a batch decodes with, the two the spec's "How it is
  shown" prices. The positions cost time instead of memory: a `jsonb` field
  is read twice and a sort re-reads two keys a comparison, and a multirange
  keys each member once a comparison it takes part in, none of it timed.
