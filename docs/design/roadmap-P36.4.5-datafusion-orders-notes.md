# P36.4.5 notes — DataFusion's answer across two orders

`datafusion_pgdump::compare_value_keys(a, b)` answers every pair of keys.
It asks `ValueKey::compare` first, which answers a pair of one order in
either semantics and every pair PostgreSQL's semantics relates. Otherwise it
takes each key's `ValueKey::emitted()`, which is the column's Arrow type and
the value in that type's units. It coerces the two types with
`BinaryTypeCoercer`, the coercer DataFusion's analyzer calls for `<`. It
casts each one-row column with `ColumnarValue::cast_to`, whose default is
the query's `safe: false`, and compares with `apply_cmp`, the kernel a
query compares by, which makes `-0` equal to `0`. A planning or cast
refusal answers `None`. D56 and D40 carry the decision and what it
rejected; `KD119` is struck.

## The shape

- **A key is built from the resolved column.** `ValueKey::of` and
  `ValueKey::literal` now take `(&ResolvedSchema, column, semantics,
  text)`. In DataFusion's semantics the key carries the field's
  `DataType`. A column the untyped mode reads as text is already
  `Utf8View` with a refused plan in that schema, so the type and the order
  cannot disagree. 36.9 makes `ResolvedSchema` one record per column; the
  two constructors then take that record.
- **The emitted value is withheld where the type cannot hold it.** This
  is decided at construction by `unrepresentable::holds_value`, the D96
  tier test on the key's text, against both tiers, because the provider
  reads both as NULL. A special value has no emitted value either, and
  neither does a timestamp past `i64` microseconds from 1970 or an interval
  time part past `i64` nanoseconds. A literal's emitted type is its
  column's. A `smallint` literal of `70000` therefore keys as an `i64` and
  `compare_value_keys` cannot rebuild it, so it answers `None` against
  another order.
- **`EmittedValue` is the codec's API, not `internal`'s.**
  `datafusion-pgdump` is an embedder (`scripts/layering.py`), and an
  embedder doing the same with another engine needs the same thing. It
  carries `arrow_buffer::i256`, which the codec already names (D74).

## What the oracle found

`tests/key_order.rs` registers PostgreSQL 16's `types` fixture and
compares 33 scalar columns pair by pair, `a < b` and `a = b` over a cross
join. When a query refuses, it is asked again one pair of ids at a time,
because one row's refused cast fails the whole query. Several hundred
cross-order pairs are answered and several thousand refused.
- **DataFusion relates more than `KD119` listed.** It relates a `Date32`
  against an integer, as days. It relates a `Utf8View` text against
  `Binary`, so `bytea` compares with a `text`, a `character(n)`, a bare
  `numeric`, a `timetz` and an enum's dictionary. It relates a `uuid`'s
  `FixedSizeBinary(16)` against `Binary`. The function answers each of
  these as the query does, with nothing listed per pair.
- **Where the scan reads a value as NULL, only the keys' own order
  relates it.** A `date` of `infinity` against another `date` still ranks
  (D56), and against another order answers `None`.

## What the next round inherits

- Not a hot-path slice, so it was not counted: nothing in the workspace
  holds a `ValueKey`, and `OrderKey` and the gatherer are untouched.
- 36.5 counts against the commit that carries this slice. `pgdt-dump`
  depends on neither crate changed here.
