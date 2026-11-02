# P33.9 notes — one decoded copy

**A query's value read from a held line is decoded once, where its column keeps
it, and a strict parse's `bytea` check holds none of it**
([`decisions.md`](decisions.md), "D141"). No reading was taken; what proves it
is an allocation count, `crates/pgdt-query/tests/decoded_copies.rs`, whose
allocator tracks the bytes live, the line past its chunk being outside it
(D139). Each of its three tests fails on the parent.

- **`Unescaped`** (`pgdt-core`'s `copy.rs`, `internal`): a field's unescaped
  bytes, read one at a time, or the rest as a slice where nothing in it is
  escaped (`rest_if_plain`). It is COPY's escapes stated once:
  `unescape_field`'s owned path collects it.
- **`decode_bytea_into` and `bytea_reads`** (`pgdt-codec`'s `value/bytea.rs`,
  `internal`): `decode_bytea`'s grammar over those bytes. The hex form
  `pg_dump` writes is read as a slice; any other spelling a byte at a time,
  counted first so its column grows once. `decode_bytea_escape` reads through
  the same byte-at-a-time grammar, so the escape form is stated once.
- **`ByteaBuilder`** (`pgdt-query`'s `batch/bytea.rs`) replaces Arrow's
  `BinaryBuilder` for a `bytea` column: a value is written into its buffer and
  dropped from it where it reads as none. `RowBatcher::push_field` decodes a
  top-level `bytea` there before unescaping it; a NULL, a column under an
  unrepresentable test and a field that reads as no value go the old way, which
  words the error as before. A nested `bytea` still decodes, then copies.
- **An escaped text past `OWN_BLOCK_PAST`** (`batch/chunks.rs`), Arrow's
  largest view block, is handed to its column as a block of its own, the
  decode's `String` and its capacity — the field's escaped length.
- **A strict check** (`pgdt-mapping`'s `statistics/gather/strict.rs`) answers a
  `bytea` that reads off its field and unescapes only one that does not, to
  say whether it is refused.

## What 33.4 inherits

- **The batch's copy is the one decoded copy**: about the line for escaped
  text, half the field for hex `bytea`, as the spec's "What a held line costs"
  states. A `bytea` batch's values are one buffer, so a row after a long value
  in the same batch grows it, copying it; a line past the span cap ends its
  batch first, and the growth is the span's (D84). `decoded_copies.rs` reads
  under a span cap below its line for that reason.
- **A data-level parse holds one copy of an escaped field beside its line**,
  strict or not: `ColumnGatherer::observe` unescapes a whole field before its
  statistics clip a head (`statistics/gather/column.rs`). The strict test's
  peak is that copy. Its removal is 33.13.
- **A filter term's key is a copy beside the batch's**, made and dropped per
  term: an escaped field unescaped, then keyed — a `bytea` decoded beside its
  unescaped text, a text copied for an ordering term. Its removal is 33.14.

## Negative results

- **No count pair was taken**: `unescape_owned` now collects the iterator
  where it pushed in a loop of its own, and P33's spec adopts no pricing of a
  hot-path change ([`decisions.md`](decisions.md), "D108").
- **`std::io::Write` into Arrow's `BinaryBuilder`** grows its `Vec` by
  doubling as the decode writes, copying the value at each step; it was not
  used.
