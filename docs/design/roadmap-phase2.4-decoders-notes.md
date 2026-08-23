# Phase 2.4 notes — decoders, render-back, round-trip tests

Every non-deferred row of the "Type mapping" table in
`docs/design/roadmap-phase2-typed-columns.md` now has a real decoder and its
exact inverse render-back function, wired into `batch.rs` so a `RecordBatch`
this build produces actually carries the typed `DataType` its
`ResolvedSchema` already promised (Phase 2.3 built the promise; this landed
the code that keeps it). Array/composite/range/multirange stay `Utf8View`
exactly as designed — Phase 4's job.

## Module shape

`decode.rs` (new, L2 — `docs/design/layering.md`): one `decode_*`/`render_*`
pair per Arrow `DataType` [`crate::pgtype::resolve_declared_type`] can
produce, operating on the already-COPY-unescaped `&str`
(`crate::copy::decode_field`'s output) and plain Rust values — never an Arrow
builder, per the layering rule. `batch.rs` (L3) is the only place that picks
a `ColumnBuilder` variant from a `DataType`, calls the matching `decode.rs`
function, and appends the result (or a `\N` null) to the right typed Arrow
builder.

`ColumnBuilder` covers every `DataType` `resolve_columns` can produce:
`Utf8View` (unchanged zero-copy path), `Boolean`, `Int16/32/64`,
`Float32/64`, `Date32`, `Timestamp(Microsecond, tz)`, `Time64(Microsecond)`,
`Decimal128`/`Decimal256`, `FixedSizeBinary(16)` (uuid), `Binary` (bytea),
and `Dictionary(Int32, Utf8)` (enum). Every non-`Utf8View` arm always copies
— a decoded value (an `i32`, a `[u8; 16]`, an unscaled `i128`/`i256`, …) has
its own representation, not a byte range of the original field, so the
zero-copy machinery `roadmap-phase7-scan-performance.md` cares about stays
scoped to exactly the column type it was built for.

`RowBatcher` now carries the qualified table name and each column's declared
type string (from `ResolvedSchema::notes`), which is all `Error::FieldDecode`
needs beyond what it already had (row offset, the failing field's own text).

`stream.rs`'s `resolve_block` used to return two schemas — a
`schema_for`-built all-`Utf8View` one for the actual batch, and a separate
`ResolvedSchema` preview. It now returns just the `ResolvedSchema`, and
`RowBatcher` is built directly from it: `TableStream::resolved_schema` and
the batches it describes are now *the same schema*, not a preview and a
placeholder — see the new `resolved_schema_matches_the_batches_it_describes`
test (renamed from `..._while_batches_stay_utf8view`, which described the
Phase 2.3 state this slice replaced). `schema_for` was renamed
`column_names` and now only produces the column-name half (placeholders for
a headerless block); the schema itself is whatever `resolve_columns` builds.

## `decode.rs` render-back targets decoded text; `copy.rs` owns the escaping leg

`decode.rs` (L2) works entirely in "decoded text vs. Arrow value" terms: every
`render_*` produces the same *already-unescaped* text `copy::decode_field`
returns for a correctly-formed value, never the raw on-disk COPY-escaped
bytes. Converting between on-disk bytes and decoded text is `copy.rs`'s job
(L1) in both directions — `decode_field` (existing) and `encode_field` (added
alongside this decision), its exact inverse for any text `pg_dump`'s `COPY TO`
could actually have produced. This keeps the L1/L2 boundary
`docs/design/layering.md` draws intact: L2 never touches escaped bytes at
all, so it has no need for an encoder of its own.

`encode_field` only implements the seven escapes `COPY TO` ever emits — a
doubled backslash and the six control-character mnemonics `\b \f \n \r \t \v`
(`docs/design/postgres-invariants.md` I15) — not the octal/hex forms
`decode_field` accepts on input, which are a `COPY FROM` reader convenience
`COPY TO` never produces. It is deliberately not a general COPY-text encoder;
nothing in this codebase needs one, since every fixture is still produced by
real `pg_dump`, never hand-encoded.

This resolves what was flagged after this phase first landed as an
interpretive call: whether the round-trip test's "original bytes" meant
decoded text or literal on-disk bytes (the two agree for every mapped type
except `bytea`, whose decoded text contains a backslash). Both legs are now
tested, each at the layer that owns it: `tests/decode.rs`'s round trip
compares decoded text (decode.rs's own scope, unaffected by this decision);
`copy_text_escaping_round_trips_through_postgres` (`tests/scan.rs`) compares
`encode_field(decode_field(raw))` against the literal on-disk bytes of every
row in the `public.escapes` fixture, across all three routine `pg_dump`
versions.

## Round-trip test design: `Strings` mode as the oracle

`tests/decode.rs`'s round trip doesn't hand-transcribe expected values (the
phase doc's own stated reason to avoid that: "a test cannot encode the same
misreading twice"). Instead it queries the same real `pg_dump` fixture twice —
`SchemaMode::Typed` and `SchemaMode::Strings` — and asserts every field
renders identically. `Strings` mode is Phase 1's untouched, byte-for-byte
behavior, already covered by its own extensive test suite, so it's a
trustworthy oracle: if `Typed` mode's decode-then-render round trip is wrong
anywhere, it disagrees with `Strings` mode's plain pass-through, on real
`pg_dump`-produced bytes, across all three routine `pg_dump` versions
(13/16/18, matching the trio `tests/pgtype.rs` already iterates).

Covers every column family that always decodes successfully on the `types`
fixture: `t_int`, `t_float`, `t_text`, `t_uuid`, `t_bytea`, `t_time`, `t_json`,
`t_net`, `t_interval`, `t_enum_domain`. `t_numeric`/`t_date`/`t_timestamp`
each carry a boundary row (`NaN`, `infinity`) that's a genuine `FieldDecode`
by design (see below), so they get their own tests asserting the error names
the right table/column/declared-type/value instead of joining the blanket
round trip — and a check that `SchemaMode::Strings` still shows the raw text
with no error, proving the escape hatch actually works on the exact value
that trips `Typed` mode.

Hand-written boundary-value assertions live in `decode.rs`'s own unit tests,
per the phase doc's list: `NaN`, `±Infinity`, `infinity`/`-infinity`
dates/timestamps, year 0001/9999, 38-vs-39-digit numeric, negative-scale
numeric (the last has no fixture coverage at all — no column in
`fixture_schema_types.sql` uses a negative scale — so it's pinned from the
type's documented semantics rather than observed `pg_dump` output).

## Two findings from testing against real values, not just designing against the spec

PostgreSQL's float-formatting fixed-vs-scientific rule (`postgres-invariants.md`
I14 — source-confirmed against `src/common/f2s.c`/`d2s.c`) and the fact that
PostgreSQL's own documented maximum timestamp overflows this build's
`Timestamp(Microsecond)` mapping were both established by direct testing
rather than assumed from the phase doc's spec — discovery path in
`docs/status/history/2026-08-23.md`. Both are now pinned as tests
(`float_formatting_matches_postgresql_exactly` and
`postgresqls_own_max_timestamp_overflows_the_unix_epoch_i64_range` in
`decode.rs`) rather than left as something a future reader would need to
re-derive.

## Calendar math

`decode.rs`'s date/timestamp decode and render use Howard Hinnant's
public-domain `days_from_civil`/`civil_from_days` (proleptic Gregorian,
correct for negative/BCE years by construction) rather than a calendar
crate, since nothing else in this codebase takes a date/time dependency and
the algorithm is a few lines either direction. BCE dates convert through
PostgreSQL's own astronomical-year convention (`1 - year` for a `" BC"`-suffixed
value) before hitting the day-count math, so `0044-01-01 BC` and
`9999-12-31`/`0001-01-01` all go through one code path with no BCE special
case in the arithmetic itself — only in the text parsing/rendering around it.

`timestamp with time zone`'s decode subtracts whatever offset the text
carries (general, not hardcoded to `+00`) before storing UTC microseconds,
even though I4 says every real `pg_dump` output normalizes to `+00` — the
general form costs nothing extra and is what
`timestamptz_normalizes_and_renders_with_utc_offset`'s non-`+00` case
exercises directly, in case a future PostgreSQL version or an unusual
session `TimeZone` setting ever produces something else.

## Existing test fallout: `SchemaMode::Typed` is real now

Landing real decoders exposed one place Phase 2.3's tests had assumed a
still-Utf8View batch that no longer holds: `escapes_table_round_trips_through_postgres_batched`
(`tests/batch.rs`) queries `public.escapes` from real `pg_dump` output with
`BatchOptions::default()` (`SchemaMode::Typed`), and `codepoint integer` is
now genuinely `Int32`, not `Utf8View` — the test's `rows_of` helper
hard-downcast every column to `StringViewArray` and panicked. Fixed the
right way per the phase doc's own stated plan ("Test strategy for existing
coverage": the two `public.escapes` tests are the one pair that run in both
modes) rather than pinning this test to `Strings` like the others: `rows_of`
across `tests/batch.rs`, `tests/stream.rs`, and `tests/query_cache.rs` now
renders every column via the new public `render_field` instead of a hardcoded
downcast, and the `escapes` test itself now runs the full check under both
`SchemaMode::Typed` and `SchemaMode::Strings`. Every other existing test
using `BatchOptions::default()` needed no change at all: they all query
`tests/data/edge_cases.sql`, a hand-written fixture with no `CREATE TABLE`
DDL, so `resolve_columns` finds nothing to map and every column stays
`Utf8View` regardless of `SchemaMode` — the "pin existing tests to `Strings`"
step the phase doc anticipated turned out to already be a no-op for all of
them.

## What's left

- Array/composite/range/multirange decoding — Phase 4, unchanged.
- `bare numeric` (no typmod) stays `Utf8View` by design (arbitrary precision
  has no Arrow decimal representation) — unaffected by this slice.
- Slice 2.5 (benchmarks, synthetic performance dataset) is the only
  remaining Phase 2 work.
