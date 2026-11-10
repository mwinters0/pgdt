# P33.14.1 notes — a nested term holds no tree

**An array, record, range or `int2vector` term orders its field as the
output grammars' walk reads it** ([`decisions.md`](decisions.md), "D151"). No
reading was taken; what proves it is
`crates/pgdt-query/tests/decoded_copies.rs`'s `a_nested_term_holds_no_tree` —
a `bigint[]` of short elements, and a `text[]` and a `public.pair[]` of
escaped elements, each of eight MiB, ordered and compared by a query
projecting none of them — which fails on the parent, its first leg holding
the field's key tree, over half a GiB.

- **`FieldText` and the walks** (`pgdt-codec`'s `nested/output/walk.rs`): a
  field's text read a byte at a time with one looked ahead, in place or as
  `COPY` wrote it, unescaped as read; `walk_array`, `walk_record`,
  `walk_range` and `walk_int2vector` hand each part over as read and return
  what the value states beside its parts. A token is read by `scan_token`
  itself, an escaped field's collected as its container wrote it first, so
  only the structure around a token is a second reading of `decode_*`'s
  grammar.
- **`nested_field_order`** (`nested/key/field.rs`): `nested_key` then
  `compare_nested`, each part keyed, compared with the bound's slot and
  dropped, a bytewise leaf ordered by `FieldOrder` with no key built. A
  leaf, a multirange or a position with no order at the top is read whole,
  as before. `key/field/tests.rs` holds it to `nested_key` and
  `compare_nested` over every form's seeds damaged a byte at a time, in
  place and escaped, under both readings of an invalid value.
- **The term** (`predicate/eval.rs`): `ResolvedTerm::walks` leaves an
  escaped nested field as `COPY` wrote it, checked UTF-8 first so the row's
  own error is raised where it was; `eval_value` walks a whole text, so a
  statistic's dictionary entry is answered by the same code.

## What 33.15 inherits

- **A multirange term reads its field whole**, `nested_field_order`'s
  fallback building its key as `nested_key` does; its members' bounds are
  33.15's, as `jsonb`'s positions are.

## What 33.16 inherits

- **`decode_*` are still the slice scanners** a batch and the census's tier
  decode with; the walk sits beside them, held to them through `nested_key`
  by `key/field/tests.rs`. Making them the walk's collectors, a builder
  taking each element as it is handed over, changes what a batch decodes
  with and is priced then (D108).

## What 33.4 inherits

- **A nested term holds one part's text at a time where it carries an
  escape**, at each level of the path the walk is on: a part an escaped
  field holds, or one quoted with an escape of its container's. A long
  escaped field of one part — a composite's long `text` — is held as its
  part's text. A range's two bound keys are held; a multirange's whole key.
- **An `IN` list over a nested column** (`Lookup::Nested`) still unescapes
  each field and builds its key, the list's binary search reading it whole.

## Negative results

- **No count pair was taken**: the slice changes neither what `field_key`
  reads with nor what a batch decodes with, the two the spec's "How it is
  shown" prices. A plain nested field's term now builds no key of it, an
  allocation per element fewer.
