# Type handling

How `pgdump_query` decides what Arrow type a column gets, and where a
PostgreSQL type does not survive the trip into a dump file intact.

## The short version

Every column gets the narrowest Arrow type we can decode from the type the dump
declares for it. A type we cannot decode **falls back to a string column**
rather than failing — so a dump always reads, and coverage improves release to
release. Pass `SchemaMode::Strings` to get every column as a string, which is what you
want if you would rather do your own parsing.

Array columns get one extra step: an array's dimensionality is a property of
each **value**, not of the declared type, so we take it from the values
themselves as the file is read rather than guessing from the DDL. That happens
on any query — see "Arrays, composites, ranges, and multiranges" below.

You can see exactly what happened to each column: `pgdq info --verbose` prints
one line per column, giving the Arrow type it resolved to — or, for a column
that came back as a string, the reason. A column that is a string because
that is simply what it is (`text`, `json`, `interval`) gets no line, since
`Utf8View` is the answer that carries no information. `pgdq info --json`
carries the same per-column outcomes in machine-readable form (see
[dump inspection](dump-inspection.md), "Scripting against the output"), and the
library exposes them on the resolved schema
(`TableStream::resolved_schema`, or `read_table`'s returned `ResolvedSchema`).

```
public.t_composite (3 rows)
    columns: id integer, v_point public.point2d, v_points public.point2d[]
    id: Int32
    v_point: Struct("x": Int32, "y": Utf8View)
    v_points: List(Struct("x": Int32, "y": Utf8View))
```

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

### `oid` is unsigned

PostgreSQL's `oid` is a 32-bit *unsigned* integer, and it comes back as
`UInt32` — so an OID at or above 2147483648 reads as the large positive number
it is, not as a negative one.

The server accepts a filter literal with a minus sign and silently wraps it
(`-1` means 4294967295 to PostgreSQL). We do not: `--filter 'v_oid<-1'` is
refused with a message naming the value, rather than compared as −1, which no
OID could ever equal. Write the value you mean.

### Floating point round-trips exactly

`pg_dump` sets `extra_float_digits = 3`, which is enough for `float4`/`float8`
to round-trip without loss. `NaN`, `Infinity` and `-Infinity` are written in
those exact spellings and are parsed as such.

### `infinity` and `NaN` filter correctly even where the column cannot hold them

`date`, `timestamp` and `timestamptz` accept `infinity` and `-infinity`;
`numeric` accepts `NaN`. PostgreSQL gives all of these a place in the order —
`-infinity` below every finite value, `infinity` above every one, `NaN` above
`infinity` — and `<`, `<=`, `>` and `>=` answer them exactly, in the spelling
each type writes.

Arrow has nowhere to *put* them: `Date32` has no infinity and `Decimal128` has
no `NaN`. So the two halves of a query have different powers, and it shows up
like this:

```sh
# selects the -infinity row, prints it, and succeeds — v_date is not built
pgdq query --source dump.sql --table public.t_date \
  --filter 'v_date<2020-01-01' --column id

# selects the same row and then fails building the Date32 column for it
pgdq query --source dump.sql --table public.t_date --filter 'v_date<2020-01-01'
# Error: public.t_date.v_date at row offset …: value `-infinity` does not
# parse as its mapped type `date` — use --schema-mode strings to read this
# column verbatim
```

The filter is right in both. What fails in the second is materializing a value
the output type cannot represent, and the message is the ordinary decode error
above. Project the column away, or read it with `--schema-mode strings`, and
the value comes back as the text the dump holds.

### Text ordering is bytewise, and your server's may not be

`<`, `<=`, `>` and `>=` on a text column compare **bytes**. PostgreSQL compares
by the column's *collation* — so the two agree exactly where that collation is
`C` or `POSIX`, and can differ anywhere else.

On an `en_US.utf8` database — the usual default on a glibc server — `A` sorts
*after* `a`, `a` sorts before `B`, and `é` sorts between `e` and `f`. Bytewise,
`A` sorts before `a`, `B` before `a`, and `é` after every unaccented letter.

**Which collation a column has is read out of the dump where the dump says.**
`pg_dump` writes a `COLLATE` clause on any column whose collation differs from
its type's own default, so:

- a column declared `COLLATE "C"` or `COLLATE "POSIX"` is answered **exactly**,
  and says nothing;
- a `name` column with no clause is answered exactly too — `name`'s own default
  collation is `C`;
- a column declared with any other collation is warned about, and so is a
  `text`, `varchar` or `char` column with **no** clause, whose collation is the
  database's and is the one thing a plain dump never records.

A filter that orders such a column says so, once per query, on stderr:

```sh
pgdq query --source dump.sql --table public.people --filter 'name<B'
# warning: `name` (text) is compared bytewise: the column declares no COLLATE
# clause, so its collation is the database's, which a plain dump does not
# record — this matches the server only if that collation is C or POSIX
```

**`char(n)` is warned about whatever its collation**, for a different reason: a
dump writes every value of it blank-padded to the declared length, and
PostgreSQL strips trailing blanks before comparing. So `--filter 'code>ab'`
selects a row whose `code` is exactly `ab`, where the server would not.

**`=` and `!=` are unaffected by collation.** Every libc collation calls two
different strings different, so equality is bytewise on the server too.

### Writing a filter term

A term is `<column><operator><value>`, and it can be written either way round:

```sh
pgdq query --source dump.sql --table public.widgets --filter 'name=alpha'
pgdq query --source dump.sql --table public.widgets --filter 'name = "alpha"'
```

Spaces around the operator are not part of the value — `name = alpha` asks for
`alpha`. **Quote the value when you mean the spaces**, or when you mean quote
marks:

```sh
--filter 'code = " x"'           # a space-padded char(n) value
--filter "note = 'it''s'"        # an interior quote is doubled, as in SQL
--filter 'note = "it'"'"'s"'     # or written in the other quote character
--filter 'tag = """hello"""'     # the seven characters "hello", quotes and all
```

(The third line's contortion is the shell's doing, not this grammar's: `it's`
cannot be written inside shell single quotes at all.)

Both `'` and `"` open a quoted value; the pair must match, and a value that
opens with one has to close with it at the very end. An unbalanced quote is
refused before any row is read rather than searched for literally.

Quotes work on the column side too, which is how a column whose name holds a
space or an operator character is named:

```sh
--filter '"my column" = alpha'
--filter '"a=b" = alpha'        # a column named a=b
--filter '"is null" = alpha'    # a column named is null
```

`column IS NULL` and `column IS NOT NULL` are matched only on a term with no
operator in it, so `--filter 'note=this is null'` is an equality against the
value `this is null`.

**`--column` and `--table` take their names exactly as given** — there is no
quoting to strip there, because the shell has already delimited the argument.
`--column '"name"'` looks for a column whose name really does begin and end
with a quote mark, and says so when it does not find one.

### Arrays, composites, ranges, and multiranges

All four map to real Arrow types, and they nest in any combination:

| Declared | Arrow | A value in the dump |
|---|---|---|
| `text[]` | `List(Utf8View)` | `{a,b,"c,d"}` — three elements, the third containing a comma |
| a composite `(x integer, y text)` | `Struct("x": Int32, "y": Utf8View)` | `(1,"a,b""c")` |
| `int4range` | `Range<Int32>` (see below) | `[1,10)` |
| `int4multirange` | `List(Range<Int32>)` | `{[1,10),[20,30)}` |

A range type's auto-created multirange companion is recognized too, even
though the dump never declares it. Nesting composes without special cases:
a composite array is `List(Struct(…))`, a composite with a `text[]` field is
`Struct("label": Utf8View, "tags": List(Utf8View))`, and an array of ranges is
`List(Range<…>)` — the same Arrow type a multirange gets, since they are the
same shape; the declared PostgreSQL type on the same `pgdq info` line is what
tells them apart.

However an array column was declared, it is the same type: PostgreSQL accepts
`integer[]`, `integer[3]`, `integer[][]`, `integer[3][4]`, `integer ARRAY` and
`integer ARRAY[4]`, discards the bounds and the dimension count, and keeps
"array of `integer`". pgdq reads all six that way. `pg_dump` only ever writes
the first, so this matters for SQL written by hand or by another tool; what
shape the *values* have is a separate question, answered under "Its arrays do
not all have the same shape" below.

A part that has no mapping of its own becomes a string **in that position**
only: a composite field of type `interval` is a `Utf8View` field inside an
otherwise typed `Struct`, exactly as an `interval` column would be at top
level.

**A range is five fields**, and `pgdq info` prints them as `Range<T>` because
they are the same five for every range column in every dump:

| Field | Type | Meaning |
|---|---|---|
| `lower`, `upper` | the bound type (`T`) | null means unbounded |
| `lower_inclusive`, `upper_inclusive` | `Boolean`, never null | `[` / `(` and `]` / `)` |
| `empty` | `Boolean`, never null | the `empty` range |

A range bound is never SQL NULL, which is what lets a null `lower` mean
"unbounded" without ambiguity — and `empty` is not redundant with the two
flags, since `empty` and `(,)` are different ranges and neither has bounds.

#### Four ways one of these columns is still a string

- **The array's element type is opaque.** `box[]`, an array of a C-level base
  or shell type, or an array of a domain over any of those. PostgreSQL lets an
  element type choose the separator its arrays are written with — `box` uses
  `;`, not `,` — and for exactly these types the dump does not say which:
  `box` is built in and has no `CREATE TYPE` in the file at all, and a domain
  inherits its base type's separator while recording nothing about it. Splitting
  such a literal on `,` would invent element boundaries that are not there, and
  the elements it recovered would be opaque text anyway, so the whole value
  stays one string. `pgdq info --verbose` reports this as `opaque element
  type`.
- **The array's element type is itself an array.** `CREATE DOMAIN intarr AS
  integer[]` and a column of `intarr[]` is legal, and PostgreSQL writes such a
  value one brace deep — `{"{1,2}","{3}"}`, each element an array literal in
  its own right, quoted — rather than as a two-dimensional array. So the
  literal's shape and the column's declared depth say different things, and we
  decline the column rather than guess which. It comes back as text, and `pgdq
  info --verbose` reports `nested array element`. Unlike an opaque element
  type, nothing about this one is unknowable: it is a shape we have not chosen
  to represent, and the lossless array representation planned in the next
  bullet would cover it too.
- **Its arrays do not all have the same shape.** PostgreSQL does not record an
  array's dimensionality in its type — `integer[]`, `integer[3]`,
  `integer[][]`, `integer[3][4]`, `integer ARRAY` and `integer ARRAY[4]` are
  six spellings of one type, and `pg_dump` writes every one of them back as
  `integer[]` — so we read the column's actual values while mapping the file
  and give it the shape they have. A column
  holding only 2-D values becomes `List(List(Int32))`. A column holding
  `{1,2}` in one row and `{{1,2},{3,4}}` in the next has no honest Arrow list
  type, and neither does one holding a value with an explicit lower bound
  (`[0:2]={7,8,9}`) — Arrow lists start at 0 and have nowhere to record an
  index origin. Both come back as text, and `pgdq info --verbose` reports
  `varying array shape`.

  A structured representation that is lossless for *every* array — dimensions,
  lower bounds and elements in one value — is planned as a selectable
  alternative for these columns. Today the answer is text.
- **You asked for strings.** `--schema-mode strings` (`SchemaMode::Strings`)
  returns every column, nested ones included, as the literal text `pg_dump`
  wrote.

#### An array inside a composite is the one shape still decided optimistically

The shapes above are read per **column**, which is the level at which a query
can act on them before it hands back its first batch. An array that is a
*field* of a composite — or the element type of another array — has no such
record, so we assume one dimension and find out at the value:

```
public.t_shipments.v at row offset 9311: value `(a,"{{1,2},{3,4}}")`
does not parse as its mapped type `public.boxed` — use --schema-mode strings
to read this column verbatim
```

Reading more of the file will not change this one, and there are **two** ways
through it. The message names one: `--schema-mode strings`
(`SchemaMode::Strings`) hands the literal back verbatim, braces and all — for
every column in the table, which is a heavy price for one of them.

The other is to leave the column out of the query. **A column you do not
project is never decoded**, so `--column`/`QueryOptions::projection` is a
per-column escape from this error where `--schema-mode strings` is a
whole-table one:

```sh
# fails on v
pgdq query --source dump.sql --table public.t_shipments

# succeeds: v is never decoded
pgdq query --source dump.sql --table public.t_shipments --column id
```

The error message does not mention this second remedy. Both work; which one
you want depends on whether you need that column's *values* or only its
absence.

#### A predicate still matches the literal text

A filter on one of these columns compares the value's **PostgreSQL text**, as
it appears in the dump, not its decoded elements. `--filter 'tags={a,b}'`
works exactly as it did when the column was a string. The consequence worth
knowing: a spelling PostgreSQL would accept but never write — `{a, b}`, with a
space — matches nothing, because the dump holds the canonical form and that is
what is being compared.

### `json` and `jsonb` are strings, and say so in the schema

Arrow has no JSON type. `jsonb` is normalized JSON text by the time it reaches
the dump; `json` is whatever was inserted. Both come back as strings, parse-ready.

The field carries Arrow's canonical `arrow.json` extension name, so a consumer
that understands extension types can tell a JSON column from any other string
without knowing what the dump declared. A `uuid` column carries `arrow.uuid`
the same way, over the `FixedSizeBinary(16)` it already was. Neither name
changes a value, a type or a byte — it is a label on the column. Nested
positions do not carry one: a `uuid` inside a composite or an array is
`FixedSizeBinary(16)` with nothing said about it.

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
