# P33.14 notes — a scalar filter term holds no copy

**A filter's term reads an escaped field off its bytes where it reads no more
of it than its bound**, and an `IN` list collects it no further than its
longest literal ([`decisions.md`](decisions.md), "D150"). No reading was
taken; what proves it is `crates/pgdt-query/tests/decoded_copies.rs`'s
`a_filter_term_holds_no_copy_of_an_escaped_field` — a `text`, a
`character(n)` and a `bytea` of eight MiB, ordered, compared, listed and
tested for NULL by a query projecting none of them — which fails on the
parent, its first leg holding the field's unescaped text and more.

- **`RawRow::read` and `RawField`** (`pgdt-core`'s `copy.rs`): a field as
  `decode` reads it, an escaped one left as `COPY` wrote it. `decode` is
  untouched, so the batch's path is unchanged.
- **`BytewiseRuns` and `FieldOrder`** (`pgdt-codec`'s `key/runs.rs`): a text
  ordered bytewise against a bound a run at a time, trimmed of its trailing
  blanks for a `character`; and a `text`, `character` or `bytea` field
  ordered against its kind's key, its typmod held by `TextFold`, a `bytea`
  decoded by `ByteaFold` as it is read. `FieldOrder::of` is the same over a
  whole text, so a plain field's ordering term builds no `OrderKey::Text`
  either. Held to `read_key` and `compare_keys` over every kind, bound and
  text cut at every pair of places (`key/runs/tests.rs`).
- **`Spellings::runs`** (`key/equality.rs`): `matches` over runs, for
  `Equality::Canonical` whatever the kind, so an `=` on an escaped field of
  any type compares off its bytes.
- **`TextFold::valid_up_to`**: the error a field's unescape raises where its
  text is not UTF-8, named by the fold, so a term reading off the bytes
  raises it alike.
- **The term and the list** (`predicate/eval.rs`): `ResolvedTerm::runs` and
  `ResolvedMembership::texts` say which read a field's runs; `eval`, the
  list's `eval` and `is_false` take an escaped field that way, a plain one
  as its borrowed text. A refusal quotes the field by `printout_head`.
  `Lookup::Canonical` and `Trimmed` hold a `Texts`, which knows its longest
  literal. `eval/tests.rs` holds the escaped path to `eval_value` over the
  unescaped text, every byte escaped and as `COPY` writes it.

## What 33.14.1 inherits

- **A nested term still unescapes its field and builds its key tree**
  (`nested_key`), and so does every term over a keyed scalar (`read_key`) —
  an integer, a float, `numeric`, a datetime, a network type, `jsonb` — and a
  term over a column whose unrepresentable values are tested or read as NULL.
  33.14.1 removes the nested ones' copy and tree; a keyed scalar's is its
  slice reader's whole text, as the spec's "A pass holds a line only where
  it decodes it" states for a parse, and `jsonb`'s is 33.15's.
- **`decode_*` is still the slice scanners**: `nested_key` reads a field
  with them, a query's batch decodes with them (`batch/builder.rs`) and the
  census's tier counts with them (`unrepresentable.rs`). Their walk is
  33.14.1's; a batch decoding over it changes what it decodes a column
  with, which the spec's "How it is shown" prices.

## What 33.4 inherits

- **A filter adds no copy of a `text`, `character` or `bytea` field to a
  query's charge**; a list adds at most its longest literal. A term over any
  other type still unescapes an escaped field, which no `pg_dump` writes in
  such a column.

## Negative results

- **No count pair was taken**: the slice changes neither what `field_key`
  reads with nor what a batch decodes with, the two the spec's "How it is
  shown" prices. A plain field's text ordering term now compares its text
  with no key built, an allocation fewer per row.
- **Hashing an escaped field run by run** into the list's set was not built:
  it rests on the hasher treating a string's bytes written in pieces as
  written whole, which `Hash for str` does not promise.
