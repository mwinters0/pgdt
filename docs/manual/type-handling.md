# Type handling

How `pgdump_query` decides what Arrow type a column gets, and where a
PostgreSQL type does not survive the trip into a dump file intact.

## The short version

Every column gets the narrowest Arrow type we can decode from the type the dump
declares for it. A type we cannot decode **falls back to a string column**
rather than failing — so a dump always reads, and coverage improves release to
release. Pass `SchemaMode::Strings` to get every column as a string, which is what you
want if you would rather do your own parsing.

You can see exactly what happened to each column: `pgdq info --verbose` lists
per-column resolutions, and the library exposes the same thing as diagnostics
on the resolved schema (`TableStream::resolved_schema`, or `read_table`'s
returned `ResolvedSchema`).

`pgdq query`'s text output is identical whether typing is on or off: every
value is rendered back to the same PostgreSQL text `pg_dump` itself would
have written, so switching `--schema-mode` never changes what shows up on
your terminal or in a pipeline downstream — only whether `pgdq info` (and a
caller reading `RecordBatch` types directly) sees a narrower Arrow type.

## What we can and cannot recover from a dump

A dump is not a database. Some things PostgreSQL knows about a value are simply
not written into the file, and no amount of parsing recovers them. These are
the cases worth knowing about before you trust a column.

### Timestamps are exact; the dump's timezone is not recorded

`pg_dump` sets `DATESTYLE = ISO` on its connection, so timestamps are written
in ISO format, and `timestamp with time zone` values carry an explicit UTC
offset:

```
2016-04-13 16:52:35.456696+00
```

That makes the **instant** unambiguous, and we map it to
`Timestamp(Microsecond, Some("UTC"))`, normalizing the offset. What is *not*
recorded anywhere in the file is the session `TimeZone` the dump ran under —
`pg_dump` never writes one. So the offset you see is whatever that session
happened to have. If you need to know the original wall-clock rendering rather
than the instant, the dump cannot tell you.

`timestamp without time zone` has no offset at all; it is a wall-clock reading
and maps to `Timestamp(Microsecond, None)`.

Fractional seconds are trailing-trimmed, so the same column can hold
`…35.456696+00` and `…10.41925+00` and `…52+00`. That is normal.

### `interval` is a string, and will stay one

PostgreSQL renders an interval according to `IntervalStyle`, and `pg_dump` never
sets or records it. The same interval is `1 day 02:03:04` under one setting and
`P1DT2H3M4S` under another, and **the file does not say which**. Since we cannot
determine the value from the dump alone, `interval` columns come back as
strings. This is not a coverage gap we intend to close; it is a property of the
format.

### `numeric` with no precision is a string

`numeric(10,2)` maps to `Decimal128(10,2)` — precision and scale are declared,
so the mapping is exact. Bare `numeric` is arbitrary-precision and can also hold
`NaN` and `±Infinity`, none of which any Arrow decimal type can represent, so it
comes back as a string. If you need it as a number, cast it downstream where you
can choose what to do with the values that do not fit.

Precision above 76 digits also falls back to a string (`Decimal256`'s limit).

### Floating point round-trips exactly

`pg_dump` sets `extra_float_digits = 3`, which is enough for `float4`/`float8`
to round-trip without loss. `NaN`, `Infinity` and `-Infinity` are written in
those exact spellings and are parsed as such.

### Arrays, composites, ranges, and multiranges are strings for now

`text[]` arrives as `{a,b,"c,d"}` — a value with its own quoting rules nested
inside the escaping COPY TEXT already applies. Decoding it correctly is real
work, shared with composite types, ranges, and multiranges (including a
range type's own auto-created multirange companion), and it is scheduled. Until then these come back as strings **with their outer COPY
escaping already removed**, so what you get is the literal array text
PostgreSQL would print.

### `json` and `jsonb` are strings

Arrow has no JSON type. `jsonb` is normalized JSON text by the time it reaches
the dump; `json` is whatever was inserted. Both come back as strings, parse-ready.

### Enums work, with one exception

An enum's labels are written into the dump, so an enum column maps to
`Dictionary(Int32, Utf8)`. The exception is a dump taken with
`--binary-upgrade`, which writes the labels differently — we read both forms, so
this should be invisible to you. A genuinely empty enum falls back to a string.

### Domains resolve to their base type

`CREATE DOMAIN email AS text NOT NULL` gives you a `Utf8View` column that is
known non-nullable. Domains over domains resolve transitively. Constraints
beyond `NOT NULL` are not enforced — we are reading a dump, not validating it.

## When a value does not match its type

If a column is typed `Int32` and a value in it does not parse as an integer,
that is an **error**, not a null. A dump is machine-generated, so a value that
contradicts its declared type means either the file is damaged or our mapping is
wrong — and both are things you want to hear about, with the byte offset,
rather than discover later as missing data.

The error names the table, column, row offset, declared type, and the offending
value. If you would rather not have your read fail on it, `SchemaMode::Strings`
gives you every column unparsed.

## Columns we cannot type at all

Some columns have no type information available:

- A `--data-only` dump has no DDL in it at all, so **every** column is
  untyped. This is not an error; you get string columns and one diagnostic
  explaining why.
- A table with no undropped columns emits a `COPY` header with no column list,
  and its columns are named `column1`, `column2`, …
- A C-level base type (`CREATE TYPE x (INPUT = …, OUTPUT = …)`) tells us how the
  *server* parses the value, which tells us nothing about the value itself.

In each case you get a string column and a diagnostic that names which of these
happened — they are distinguished on purpose, because "we have not implemented
this yet" and "the dump does not contain the information" are different answers
to "will this improve later?"
