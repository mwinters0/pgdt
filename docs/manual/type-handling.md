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

You can see exactly what happened to each column: `pgdt info --detail` prints
one line per column, giving the Arrow type it resolved to — or, for a column
that came back as a string, the reason. A column that is a string because
that is simply what it is (`text`, `json`, `inet`) gets no line, since
`Utf8View` is the answer that carries no information. `pgdt info --json`
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

`pgdt query`'s text output is identical whether typing is on or off: every
value is rendered back to the same PostgreSQL text `pg_dump` itself would
have written, so switching `--schema-mode` never changes what shows up on
your terminal or in a pipeline downstream — only whether `pgdt info` (and a
caller reading `RecordBatch` types directly) sees a narrower Arrow type.

A partitioned table dumped through its root — with `--load-via-partition-root`,
or by `pg_dump` on its own for a table hash-partitioned on an enum column — is
written as one `COPY` block per partition, and each block lists the columns in
its own partition's order, which differs from the table's where a partition was
created on its own and then attached. Every row comes back in one column order
whichever block it came from: the table's `CREATE TABLE` order, or the first
block's where the dump holds no DDL for it. A table whose blocks name different
columns is refused rather than padded with NULLs the dump never held.

A table whose every column is dropped or generated — or that has no columns at
all — is written with a `COPY` header listing none, and each of its rows as an
empty line. It comes back as a table with **no columns** and its true row
count, whatever its `CREATE TABLE` declares: a generated column's values are
never in the dump, so they are not in the answer. A non-empty line in such a
block is refused as a row with the wrong number of fields.

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

Three kinds of value PostgreSQL accepts do not fit their Arrow types. A
`timestamp` or `timestamptz` from `294247-01-10 04:00:54.775808` UTC to
PostgreSQL's last, `294276-12-31 23:59:59.999999`, is past what Arrow counts
from 1970, and `time` `24:00:00`, PostgreSQL's end of day, is past Arrow's:
each reads as NULL, as a `date` holding `infinity` does (below, "A value its
column cannot hold reads as NULL"). A `date`, `timestamp` or `timestamptz`
after `262142-12-31` is past the calendar Arrow's formatting reads it
through: `pgdt query` prints it, and a DataFusion query reads it as NULL too.
Every one reads back verbatim under `--schema-mode strings`, and `pgdt info
--detail` counts them per column ([dump inspection](dump-inspection.md),
"Values a column's type cannot hold").

### `interval` keeps its three fields, and two kinds of value do not fit

An `interval` column maps to Arrow's `Interval(MonthDayNano)`, which carries
months, days and a time part as three independent fields — exactly what
PostgreSQL stores, so nothing is flattened into a single duration. The text it
is read from is always in PostgreSQL's `postgres` interval style
(`1 year 2 mons 3 days 04:05:06`), because `pg_dump` pins the setting when it
reads the table, and `pgdt query` writes that same text back.

Two kinds of value have no place in the Arrow type, and each reads as NULL
the same way a `date` holding `infinity` does:

- `infinity` and `-infinity`, which PostgreSQL 17 added for this type;
- a time part longer than `2562047:47:16.854775807`. PostgreSQL counts
  *microseconds* where Arrow counts nanoseconds, a thousandth of the range,
  and nothing folds hours into days — so `interval '100000000 hours'` is
  written `100000000:00:00` and is well past it.

Neither is common, and the recourse is the one every such value has: read
the column with `--schema-mode strings`, and the text comes back verbatim —
see "A value its column cannot hold reads as NULL" below, which is the same
shape.

`<`, `<=`, `>` and `>=` on an `interval` column compare **durations**, not the
three fields separately. PostgreSQL treats a month as 30 days and a day as 24 hours when it orders
intervals, so `1 mon`, `30 days` and `720:00:00` are one value, and all three
select the same rows:

```sh
pgdt query --source dump.sql --table public.jobs --filter 'ran_for>1 mon'
pgdt query --source dump.sql --table public.jobs --filter 'ran_for>720:00:00'
```

### `numeric` with no precision is a string, but it still filters as a number

`numeric(10,2)` maps to `Decimal128(10,2)` — precision and scale are declared,
so the mapping is exact. Bare `numeric` is arbitrary-precision and can also hold
`NaN` and `±Infinity`, none of which any Arrow decimal type can represent, so it
comes back as a string. If you need it as a number, cast it downstream where you
can choose what to do with the values that do not fit.

Precision above 76 digits also falls back to a string (`Decimal256`'s limit).

**The filter operators are not fooled by that.** They compare such a column as
a decimal, exactly as PostgreSQL does, over the digits the dump holds — so
`--filter 'v>9'` keeps a row whose `v` is `100.00`, and `1.5` and `1.50` are
one value however the file spelled them, under `=` as much as under `<`. Bare `numeric` also
answers `Infinity`, `-Infinity` and `NaN` in those exact spellings. A
`numeric(p,s)` column cannot hold an infinity at all — PostgreSQL rejects one
under any precision — so a filter naming one there is refused rather than
compared.

**A literal finer than the column's scale is refused too**, rather than being
rounded to fit: `--filter 'price>1.005'` on a `numeric(10,2)` is told the
column is written as a number with at most 2 decimal places. Rounding it would
mean guessing which way you meant it to go; writing `1.00` or `1.01` says so.

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

### A value its column cannot hold reads as NULL

`date`, `timestamp` and `timestamptz` accept `infinity` and `-infinity`;
`numeric` accepts `NaN`, and a bare one accepts `Infinity` and `-Infinity` as
well. Each type is held to its own spelling: `date` writes `infinity` and
`numeric` writes `Infinity`, and neither answers to the other's.

Arrow has nowhere to *put* some of them: `Date32` has no infinity and
`Decimal128` has no `NaN` — a bare `numeric`, read as text, holds all three.
**Such a value reads as NULL, for every purpose**, as though the dump held a
NULL there: the column keeps its type, `--filter 'v_date<2020-01-01'` does
not select the `-infinity` row, `--filter 'v_date is null'` does, and a count
of the column's values leaves it out. So a query answers the same whichever
rows it happens to read. `pgdt query` says on stderr how many such values
each column it prints holds, which is a property of the table, not of the
rows printed:

```sh
pgdt query --source dump.sql --table public.t_date
# warning: public.t_date.v_date holds 2 value(s) its type `date` cannot
# hold, read as NULL
```

**`--unrepresentable refuse` refuses instead**, where the query reads such a
value in a column it prints; one only a filter reads is compared in
PostgreSQL's order, `-infinity` below every finite value, `infinity` above
every one and `NaN` above `infinity`, so `--filter 'v_date<2020-01-01'
--column id` selects the `-infinity` row:

```sh
pgdt query --source dump.sql --table public.t_date --unrepresentable refuse
# Error: public.t_date.v_date at row offset …: `-infinity` is a `date` value
# the column's Arrow type cannot hold, and this query refuses such values —
# read them in the null mode, as NULL, or leave the column unmaterialized
```

Where the query refuses depends on the rows it reads first, which is not
yet fixed. Read the column with `--schema-mode strings` to get the text the
dump holds.

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

A `char(n)` column is read exactly like a `text` one, once its blank padding is
out of the way — see below.

Only `C` and `POSIX` from `pg_catalog` are answered exactly, by name. A
collation of your own that happens to be bytewise — `CREATE COLLATION mycoll
FROM "C"` — is still warned about, even though the dump that declares it says
`locale = 'C'`: the rows are right and the warning is one you can ignore. The
alternative would be pgdt deciding what a collation *does* from something other
than its name, and getting that wrong silently returns the wrong rows.

A filter that orders such a column says so, once per query, on stderr:

```sh
pgdt query --source dump.sql --table public.people --filter 'name<B'
# warning: `name` (text) is compared bytewise: the column declares no COLLATE
# clause, so its collation is the database's, which a plain dump does not
# record — this matches the server only if that collation is C or POSIX
```

**`=` and `!=` on that same column say nothing, and are exact.** A collation
that calls two different strings different — which is every libc collation, and
every ICU one unless it was created otherwise — makes equality a byte
comparison on the server whatever else it orders. The warning is about the
*order*, and it is raised per filter term, not per column — so a query that
asks `name<B` and `name=alpha` warns once.

**The exception is a collation created `deterministic = false`**, which is an
ICU one and which the dump states outright. Such a collation can call two
differently spelled strings *equal* — that is what people create one for, a
case- or accent-insensitive column — so `=` on a column of it returns fewer
rows here than on the server. pgdt reads the `CREATE COLLATION` and says so,
under `=` and `!=` as well as under the ordering operators:

```sh
pgdt query --source dump.sql --table public.people --filter 'name=alpha'
# warning: `name` (text) is compared bytewise: the column declares a collation
# this dump declares non-deterministic, so PostgreSQL neither orders nor
# compares it byte for byte — two values spelled differently can be equal to
# the server
```

The rows come back as an exact-text match, which is a *narrower* answer than
the server's rather than an unrelated one. There is no way to ask for the
server's here.

**A `char(n)` column's blank padding is not part of its value.** A dump writes
every value of such a column padded with spaces to the declared length, and
PostgreSQL strips the trailing blanks off *both* sides before comparing. `<`,
`<=`, `>` and `>=` do the same here, so `--filter 'code>=ab'` and `--filter
'code<=ab'` both select a row whose `code` is `ab` followed by padding, exactly
as the server does — and you may write the padding into the literal or leave it
out, since neither side keeps it.

**`=` and `!=` trim it too**, so `--filter 'code=ab'` selects a row whose
`code` is `ab` followed by padding, exactly as the server does — with or
without the padding written into the literal.

### Six string-shaped types still order the way PostgreSQL orders them

A column that comes back as text is not necessarily *compared* as text.
`time with time zone`, `inet`, `cidr`, `macaddr`, `macaddr8` and `jsonb` all
arrive as strings — no Arrow type fits them — and all six order the way the
server orders them:

- a `time with time zone` by the instant it names, so `00:00:00-05` is five
  hours after `00:00:00+00` and sorts above it;
- an `inet` or `cidr` by address family first (every IPv4 address below every
  IPv6 one), then the network, then the netmask length, then the host part;
- a `macaddr` or `macaddr8` by its octets;
- a `jsonb` by its structure — see below, since it is the one with a caveat.

**Write the value the way the dump writes it.** The filter reads each of these
— and an `interval`, whose Arrow type does not widen what its literal may say —
in PostgreSQL's *output* spelling only, which is what every value in the file
is already in. So `--filter 'ran_for>1 mon'` works and `--filter 'ran_for>1
month'` does not, and `--filter 'host>08:00:2b:01:02:03'` works where
`08-00-2b-01-02-03` does not — the server accepts both, pgdt accepts the one
a dump can contain. A spelling it will not read is refused by name:

```sh
pgdt query --source dump.sql --table public.jobs --filter 'ran_for>1 month'
# error: filter value `1 month` for `ran_for > ...` does not parse as the
# column's declared type `interval`
```

### `jsonb` compares as a document, with one caveat about strings

A `jsonb` filter compares the way PostgreSQL compares two documents, not the
way their text sorts. The rules that surprise people are the server's own:

- **Kind decides first**, and the order is object, then array, then boolean,
  then number, then string, then JSON `null`. So `--filter 'doc>1'` keeps every
  object and array in the column and drops every string.
- **Then size**: a two-key object sorts above a one-key object whatever the
  keys say, and a two-element array above a one-element array.
- **Then the members**, left to right — for an object, key and then value, with
  the keys in the order the server stores them, which is *shortest first* and
  only then alphabetical.
- One genuine oddity, which PostgreSQL's own source calls a mild anomaly and
  has frozen: a bare scalar sorts **above** an empty array. `1 > []` is true,
  and so is `1 < [1]`.

**Write the literal as JSON, not as the file spells it.** This is the one
string-shaped type that reads more than the dump writes: `{"a":1}` and
`{ "a" : 1 }` both work, keys may be in any order, a duplicate key resolves to
the last one, and `1`, `1.0` and `1e2` are one number. What is refused is what
the server refuses — `01`, `+1`, `.5`, `1.`, `NaN`, an unquoted key, a trailing
comma.

**The caveat is collation, and it is the text caveat one level down.**
PostgreSQL orders every string *inside* a `jsonb` document — values and object
keys alike — by the database's collation, which a plain dump does not record.
So pgdt compares those bytewise, exactly as it does a bare `text` column, and
says so once on stderr — for an *ordering* filter. `=` and `!=` on a `jsonb`
column are exact, for the same reason they are on a text one. A document with
no strings in it, or one whose comparison is settled before a string is
reached, is unaffected either way.

**`json` is compared as text**, and warned about the way a text column is,
because PostgreSQL defines no comparison for `json` at all — no `=`, no `<`,
nothing. There is no server answer to agree with.

### `=` and `!=` compare values, not spellings

Every filter operator but `IS NULL`/`IS NOT NULL` reads your value with the
column's own decoder, so `=` asks the question you meant rather than the one
your keyboard typed:

```sh
--filter 'price=1.5'      # matches a numeric(10,2) column written 1.50
--filter 'code=ab'        # matches a char(10) column written "ab" + padding
--filter 'span=30 days'   # matches an interval written 1 mon
--filter 'addr=10.0.0.1/32'   # matches an inet written 10.0.0.1
```

On a `text` or `varchar` column nothing changes — the value you type is already
the value the file holds.

**A value that is not of the column's type is refused by name**, before any row
is read, instead of quietly matching nothing — and the refusal says what the
column *does* read:

```
$ pgdt query --source dump.sql --table public.t --filter 'v_flag=true'
Error: filter value `true` for `v_flag = ...` does not parse as the column's declared type `boolean`, which is written `t` or `f`
```

Write it the way the dump writes it — a `timestamp` carries a time part
(`2020-01-01 00:00:00`), an `interval` uses the spellings `interval` prints, a
`macaddr` is colon-separated. Every value in the file is already in that form,
so the only thing this rules out is a spelling you would have had to guess at
anyway.

**One thing `=` does not do is search.** It is exact equality against one
column; there is no `LIKE`, no pattern and no case folding.

**A column whose type we cannot type at all still answers `=`, as text, and
says so.** For most such types the dump's text *is* the value, so the answer is
the server's; for the geometric types it is not — PostgreSQL's `box` equality
compares *areas*, so it calls two differently-placed rectangles of one size
equal and a text comparison does not. That is what the warning is for, and it
is why the ordering operators are refused on those columns outright rather than
answered.

### Writing a filter term

A term is `<column><operator><value>`, and it can be written either way round:

```sh
pgdt query --source dump.sql --table public.widgets --filter 'name=alpha'
pgdt query --source dump.sql --table public.widgets --filter 'name = "alpha"'
```

Spaces around the operator are not part of the value — `name = alpha` asks for
`alpha`. **Quote the value when you mean the spaces**, or when you mean quote
marks:

```sh
--filter 'code = " x"'           # a value with a leading space
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

`column IS DISTINCT FROM value` and `column IS NOT DISTINCT FROM value` are
`!=` and `=` with NULL counted as a value rather than as unknown, which is the
one thing plain negation cannot say:

```sh
--filter 'is_active IS DISTINCT FROM t'      # also keeps the rows where it is NULL
--filter 'is_active != t'                    # drops them, as SQL does
```

`column IN (value, …)` keeps a row whose value is any one of the listed ones —
exactly the rows `column=value OR column=value …` keeps, each value read as
`=` reads it, however long the list:

```sh
--filter "status IN (active, 'on hold', pending)"
--where 'not id in (3, 5, 8)'       # NOT IN: also drops the rows where id is NULL
```

Each value is trimmed, or quoted to keep its spaces, a comma or a paren —
`--filter "v in ('(1,a)', '(2,b)')"`. There is no NULL literal: `null` in the
list is the text `null`, as `=null` is. An empty list, an empty value and
anything after the closing `)` are refused.

In `IS [NOT] DISTINCT FROM` and `IN`, any run of whitespace separates the
words and the case is free. Whichever
operator comes first in the term wins, so `--filter 'note=a is distinct from
b'` is the equality it reads as, and a column whose name really is
`is distinct from` is still asked for as `--filter 'is distinct from=x'`.
`IN` needs whitespace before it and a `(` after it, so `--filter "note='x in
(y)'"` is an equality against `x in (y)` — quoted, since a paren in a value is
otherwise refused (below).

**`--column` and `--table` take their names exactly as given** — there is no
quoting to strip there, because the shell has already delimited the argument.
`--column '"name"'` looks for a column whose name really does begin and end
with a quote mark, and says so when it does not find one.

### Combining terms: `--where`

A repeated `--filter` is an `AND`. For anything else — `OR`, negation,
grouping — there is `--where`, which takes one expression over exactly the
terms above:

```sh
pgdt query --source dump.sql --table public.widgets \
  --where 'name=alpha or (name=beta and is_active=t)'
pgdt query --source dump.sql --table public.widgets --where 'not name=alpha'
```

`NOT` binds tighter than `AND`, which binds tighter than `OR`; parens
override that. The keywords are case-insensitive, and they are only keywords
as whole words outside quotes — `--where 'tag=and'` is still an equality
against `and`, the `=` before it joining it to the term, and so is
`--where 'tag="and"'`. On the value side a quote only quotes where it opens
the value, exactly as in a `--filter` term, so an apostrophe inside an
unquoted value is just a character:
`--where "note=don't and x=1"` is `note=don't` and `x=1`.

An `IN` list's parens are the term's own, not a group, so a keyword or a
paren inside the list is part of it: `--where "tag in (and, 'a)b') or x=1"` is
two terms.

**A value that holds a paren must be quoted**, because a bare `(` groups:

```sh
--where "v='(1,a)'"     # a composite literal, quoted
--where 'v=(1,a)'       # refused: the ( opens a group
```

Given both flags, the expression and every `--filter` term must all hold.

**A `--filter` term is never read as an expression — and may not hold one
either.** A term carrying an unquoted `AND`, `OR` or `NOT` as a word, or a paren outside an
`IN` list, is refused
rather than taken literally, so no string can mean one thing under `--filter`
and something else under `--where`:

```sh
--filter 'note=a or b'      # refused: OR is a reserved spelling
--filter "note='a or b'"    # the equality against `a or b`
--filter "note=don't or b"  # refused: a quote inside the value does not quote
--where  'note=a or b'      # `note=a` and then a leaf `b`, which is not a term
--filter "span='[1,10)'"    # a range literal: its `)` is a paren, so quote it
--filter "v='(1,a)'"        # and so is a composite literal's
```

The refusal is exactly as narrow as the expression grammar's own reading of a
string, so everything that was one term stays one: a keyword needs whitespace
or a paren beside it, which leaves `--filter 'tag=and'`, `--filter
'note=a b'`, `--filter 'v=not a'` and both `IS` forms untouched. A column whose
name really is a keyword is asked for the way the grammar already teaches —
`--filter '"and" is null'`.

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
same shape; the declared PostgreSQL type — on the table's `columns:` line in
`pgdt info`, above the Arrow types `--detail` lists, and beside the Arrow type
in `--json` — is what tells them apart.

However an array column was declared, it is the same type: PostgreSQL accepts
`integer[]`, `integer[3]`, `integer[][]`, `integer[3][4]`, `integer ARRAY` and
`integer ARRAY[4]`, discards the bounds and the dimension count, and keeps
"array of `integer`". pgdt reads all six that way. `pg_dump` only ever writes
the first, so this matters for SQL written by hand or by another tool; what
shape the *values* have is a separate question, answered under "Its arrays do
not all have the same shape" below.

A part that has no mapping of its own becomes a string **in that position**
only: a composite field of type `inet` is a `Utf8View` field inside an
otherwise typed `Struct`, exactly as an `inet` column would be at top level.

**`int2vector` is a fifth shape, and it is written differently.** It is a
`pg_catalog` type — you will only meet it in a dump of the system catalogs, or
in a schema that borrowed it — and it maps to `List(Int16)`, the same Arrow
type `smallint[]` gets. What differs is the text: PostgreSQL writes it as plain
numbers separated by spaces, with no braces, no quoting and no NULL element,
and an **empty** vector as an empty field. So a filter on such a column is
written the same way:

```sh
pgdt query --source dump.sql --table pg_index --filter 'indkey=1 2 3'
```

It compares element by element, exactly as an array does — so `2` is *less
than* `10`, where the two strings sort the other way round.

**A range is five fields**, and `pgdt info --detail` prints them as `Range<T>` because
they are the same five for every range column in every dump:

| Field | Type | Meaning |
|---|---|---|
| `lower`, `upper` | the bound type (`T`) | null means unbounded |
| `lower_inclusive`, `upper_inclusive` | `Boolean`, never null | `[` / `(` and `]` / `)` |
| `empty` | `Boolean`, never null | the `empty` range |

A range bound is never SQL NULL, which is what lets a null `lower` mean
"unbounded" without ambiguity — and `empty` is not redundant with the two
flags, since `empty` and `(,)` are different ranges and neither has bounds.

#### Filtering one of these columns

**A nested column is filtered structurally**, the way PostgreSQL compares two
of them — not the way their text sorts. All six operators work on an array, a
composite, a range and a multirange alike, and the rules are the server's:

- **Elements and fields decide first**, left to right, each by its *own* type's
  comparison: an `integer[]` orders its elements numerically, a `mood[]` by the
  enum's declaration order, a composite's `integer` field numerically and its
  `text` field as text.
- **A NULL element or field is above every value**, and two NULLs are equal.
  This holds under `=` as well as `<`, so `--filter 'tags={NULL}'` matches a
  row whose array is a single SQL NULL — there is no three-valued surprise
  inside a container. Only the whole column being NULL makes a row unknown.
- **An array falls back to its shape only when the elements agree**: first the
  element count, then the dimension count, then the dimensions, then the lower
  bounds. So `{}` is below `{1,2}`, and `{1,2}` is *above* `[0:1]={1,2}` — the
  elements are equal and the lower bound settles it.
- **A range sorts `empty` below everything**, then by lower bound and then by
  upper. An absent bound is the extreme of its own end, so `(,5)` is below
  `[1,10)` and `[1,)` is above it; and at the same value an exclusive *lower*
  bound is above an inclusive one while an exclusive *upper* is below.
- **A multirange compares member by member**, with a shorter multirange below a
  longer one whose members agree.

**Write the literal the way you would write it in SQL.** The container grammar
is PostgreSQL's input grammar, not its output one, so whitespace around an
element and optional quoting are both fine:

```sh
--filter 'tags={a, b}'          # matches a column written {a,b}
--filter 'tags={ a , "b" }'     # the same
--where  "p='(1,a)'"            # a composite; the parens need quoting
```

**Write each part the way the dump writes it.** The leniency stops at the
element: an element or a field is read in its own type's grammar, which
accepts the form the file holds and little else. So `{ 1 , 2 }` is accepted and
`( 1 , a )` is refused — an array drops the blanks around an element before the
element is read (that half is PostgreSQL's rule), a composite keeps every byte
between its parens, and the blanks are then not part of how an `integer` is
written. The same rule refuses a padded scalar: `--filter 'n=1'` is fine and
`--filter 'n=" 1 "'` is not.

**A column diverges where its parts diverge, and the warning says where.** A
`text[]` column is on the database's collation exactly as a `text` column is,
one level down:

```sh
pgdt query --source dump.sql --table public.t --filter 'tags<{b}'
# warning: `tags[]` (text) is compared bytewise: the column declares no COLLATE
# clause, so its collation is the database's, which a plain dump does not
# record — this matches the server only if that collation is C or POSIX
```

A composite with two such parts warns twice, once per position — `label` and
`tags[]` in a `(label text, tags text[])`.

**A part with no order at all refuses the whole column**, naming it, because
the server refuses it too — `json` has no comparison in PostgreSQL, so a
`json[]` column has none either:

```
$ pgdt query --source dump.sql --table public.t --filter 'docs<{}'
Error: `<` on column `docs` in the COPY block at offset 1234: the column is
nested and `[]` inside it is `json`, which has no order here — PostgreSQL
refuses the same comparison, since a container is ordered by its element type's
own comparison and this type has none; use `=` or `!=` for a text comparison
```

`=` and `!=` do still work there — they compare the whole value's text — and
they warn for the same reason, naming the same part. PostgreSQL has no equality
for a `json[]` either, so a text comparison is an answer the server does not
have rather than a weaker one:

```sh
pgdt query --source dump.sql --table public.t --filter 'docs={}'
# warning: `docs[]` (json) is compared bytewise: PostgreSQL defines no
# comparison for this type at all — no equality, no ordering, no operator class
# — so this comparison is one the server does not have
```

**A range literal is rewritten before it is compared, the way the server
rewrites it.** PostgreSQL does not store a range as you write it: for
`int4range`, `int8range` and `daterange` it shifts a bound to the next value so
that the range is half-open, and it collapses a range holding nothing to
`empty`. pgdt does the same, so every spelling of one value matches:

```sh
--filter "span='[1,11)'"   # all four match the same rows —
--filter "span='[1,10]'"   #   the server stores every one of them
--filter "span='(0,11)'"   #   as [1,11)
--filter "span='(0,10]'"
--filter "span='(1,2)'"    # matches rows holding `empty`: no integer is between
```

A literal whose bound has no next value — `'[1,2147483647]'` for an
`int4range`, `'(5874897-12-31,)'` for a `daterange` — is refused before any
row is read, as PostgreSQL refuses it with "integer out of range" or "date out
of range".

`numrange`, `tsrange`, `tstzrange` and any range type you defined yourself
without a `canonical` parameter do **not** get that shift — PostgreSQL only
rewrites a range whose type declares a canonical function — so there `[1,10)`
and `[1,10]` are two different values, and both are askable. A multirange is
normalized in the same spirit: its
members are sorted, empty ones dropped, and any two that overlap or touch
merged, so `{[5,10),[1,5)}` and `{[1,10)}` are one value.

A range whose lower bound is above its upper is not a value at all, and pgdt
refuses the literal rather than matching nothing:

```
$ pgdt query --source dump.sql --table public.t --filter "span='[10,1)'"
Error: `=` on column `span`: `[10,1)` is not a value of type `int4range` — it
is read as a range literal — `[a,b)`, `empty`, a bound left empty for
unbounded — whose lower bound is not above its upper …
```

**A range type of your own with a `canonical` parameter is refused, not
guessed at.** That parameter names a function on your server that rewrites
every value of the type before storing or comparing it, and no reader of a dump
can run it — so `[1,10]` and `[1,11)` might be one value there or two, and pgdt
will not pretend to know which. A filter on such a column is refused under
**every** operator, `=` and `!=` included, naming the type and the function:

```
$ pgdt query --source dump.sql --table public.t --filter "span='[1,10]'"
Error: `=` on column `span` in the COPY block at offset 1234: the range type
`public.canonrange` declares a canonical function
(`public.canonrange_canonical`), which PostgreSQL applies to every value of it
before storing or comparing one — arbitrary server-side code this build cannot
run, so two spellings the server calls one value would be two values here; no
operator can be answered for this column, `=` and `!=` included
```

The refusal reaches anything holding such a range — an array of one, a
composite with one as a field, its multirange companion, a domain over it —
and `IS NULL`/`IS NOT NULL` still work, since they read no value. The column
itself still comes back: only comparing it is refused. This is rare: a
canonical function has to be written in C or in one of the server's internal
languages, so in practice it comes from an extension or a hand-loaded module.
`pgdt info --detail` names the parameter under the type, so you can see
whether a dump has one before you write a filter.

#### Four ways one of these columns is still a string

- **The array's element type is opaque.** `box[]`, an array of a C-level base
  or shell type, or an array of a domain over any of those. PostgreSQL lets an
  element type choose the separator its arrays are written with — `box` uses
  `;`, not `,` — and for exactly these types the dump does not say which:
  `box` is built in and has no `CREATE TYPE` in the file at all, and a domain
  inherits its base type's separator while recording nothing about it. Splitting
  such a literal on `,` would invent element boundaries that are not there, and
  the elements it recovered would be opaque text anyway, so the whole value
  stays one string. `pgdt info --detail` reports this as `opaque element
  type`.
- **The array's element type is itself an array.** `CREATE DOMAIN intarr AS
  integer[]` and a column of `intarr[]` is legal, and PostgreSQL writes such a
  value one brace deep — `{"{1,2}","{3}"}`, each element an array literal in
  its own right, quoted — rather than as a two-dimensional array. So the
  literal's shape and the column's declared depth say different things, and we
  decline the column rather than guess which. It comes back as text, and `pgdt
  info --detail` reports `nested array element`. Unlike an opaque element
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
  index origin. Both come back as text, and `pgdt info --detail` reports
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
project, and no filter term names, is never decoded**, so
`--column`/`QueryOptions::projection` is a per-column escape from this error
where `--schema-mode strings` is a whole-table one:

```sh
# fails on v
pgdt query --source dump.sql --table public.t_shipments

# succeeds: v is never decoded
pgdt query --source dump.sql --table public.t_shipments --column id
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
the dump; `json` is whatever was inserted. Both come back as strings,
parse-ready. A `jsonb` column is still *compared* as a document under `<` and
friends — see "`jsonb` compares as a document" above.

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

Because the labels come with their declaration order, `<`, `<=`, `>` and `>=`
on an enum column order by that, which is what PostgreSQL does: on a type
declared `('low','medium','high')`, `--filter 'level>low'` keeps the `medium`
and `high` rows, not the alphabetical ones. A value that is not one of the
declared labels is reported as a decode error rather than being compared, on
either side of the operator.

That last one is the way an enum filter usually goes wrong — a label mistyped,
or in the wrong case — so **the refusal lists the labels back to you**, quoted
the way you would write them:

```
Error: filter value `furious` for `v_mood = ...` does not parse as the column's declared type `public.mood`, which is written as one of the type's declared labels: 'sad', 'ok', 'happy'
```

A type declaring more than a dozen labels gets the first twelve and a count of
the rest. `pgdt info --detail` lists an enum column's declared labels beneath
it, in full, so you can read the spelling off the dump instead of guessing at
it — and lists every enum type's labels once, up in the header,
which is where to look for the ones no column of yours happens to use; see
[inspecting a dump](dump-inspection.md#info-reporting-what-is-known).

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
- A C-level base type (`CREATE TYPE x (INPUT = …, OUTPUT = …)`) tells us how the
  *server* parses the value, which tells us nothing about the value itself.

In each case you get a string column and a diagnostic that names which of these
happened — they are distinguished on purpose, because "we have not implemented
this yet" and "the dump does not contain the information" are different answers
to "will this improve later?"

## Appendix: value ranges, from PostgreSQL to pandas

The least and greatest value of every PostgreSQL type that maps to a typed
Arrow column, and how far each consumer downstream of us can carry it. The
infinities and `NaN` are left out. Years are written BC/AD; Arrow's and
`chrono`'s own year numbering is astronomical (year 0 is 1 BC), so their
negative years read one less than the BC year.

Where each column comes from:

- **PostgreSQL**: values PostgreSQL 13 to 18 accept, held in our `types` test
  fixture.
- **Arrow**: the storage range of the Arrow type the column maps to, per
  Arrow's format specification.
- **DataFusion**: what DataFusion 55.1 can display or cast to a string
  (`arrow-cast` 59.2.0, `chrono` 0.4.45). Outside this range it holds, compares
  and writes the value, but printing it gives `ERROR: Cast error`, so the
  DataFusion provider reads such a value as NULL, or refuses it.
- **Python**: the standard library, as `pyarrow`'s `as_py()` hands values to
  it (Python 3.13.7, `pyarrow` 25.0.1).
- **pandas**: `pyarrow`'s `to_pandas()` with its default options (pandas
  3.0.6, numpy 2.5.3), options that change a cell noted in it.

### Dates and times

| Type | PostgreSQL | Arrow | DataFusion | Python | pandas |
|---|---|---|---|---|---|
| `date` → `Date32` | 4714-11-24 BC … 5874897-12-31 | 5877642-06-23 BC … 5881580-07-11 (`i32` days) | 262144-01-01 BC … 262142-12-31 | 0001-01-01 … 9999-12-31 | 0001-01-01 … 9999-12-31, as `datetime.date` objects; with `date_as_object=False`, `datetime64[ms]`, all of Arrow's |
| `timestamp` → `Timestamp(µs)` | 4714-11-24 00:00:00 BC … 294276-12-31 23:59:59.999999 | 290309-12-21 19:59:05.224192 BC … 294247-01-10 04:00:54.775807 (`i64` µs) | 262144-01-01 BC … 262142-12-31 23:59:59.999999 | 0001-01-01 00:00:00 … 9999-12-31 23:59:59.999999 | `datetime64[us]`, all of Arrow's but its least value; with `coerce_temporal_nanoseconds=True`, `datetime64[ns]`, 1677-09-21 00:12:43.145224193 … 2262-04-11 23:47:16.854775807 |
| `timestamptz` → `Timestamp(µs, "UTC")` | as `timestamp`, in UTC | as `timestamp` | as `timestamp` | as `timestamp` | as `timestamp`, `datetime64[us, UTC]` |
| `time` → `Time64(µs)` | 00:00:00 … 24:00:00 | 00:00:00 … 23:59:59.999999 | 00:00:00 … 23:59:59.999999 | 00:00:00 … 23:59:59.999999 | 00:00:00 … 23:59:59.999999, as `datetime.time` objects |

### `interval` → `Interval(MonthDayNano)`

| Field | PostgreSQL | Arrow | DataFusion | Python | pandas |
|---|---|---|---|---|---|
| months | −178956970 years −8 months … 178956970 years 7 months (`i32`) | `i32` | all of Arrow's | all of Arrow's, as `pyarrow`'s `MonthDayNano` | all of Arrow's, as `DateOffset` objects |
| days | ±2147483647 days (`i32`) | `i32` | all of Arrow's | all of Arrow's | all of Arrow's |
| time part | ±2562047788:00:54.775807 from PostgreSQL 15; ±2147483647:59:59.999999 in a 13 or 14 dump | ±2562047:47:16.854775807 (`i64` ns), ±2562047:47:16.854775 at PostgreSQL's microseconds | all of Arrow's | all of Arrow's | all of Arrow's |

### Numbers

| Type | PostgreSQL | Arrow | DataFusion | Python | pandas |
|---|---|---|---|---|---|
| `smallint` → `Int16` | −32768 … 32767 | same | same | unbounded `int` | `int16`; `float64` if the column holds a NULL |
| `integer` → `Int32` | −2147483648 … 2147483647 | same | same | unbounded `int` | `int32`; `float64` if the column holds a NULL |
| `bigint` → `Int64` | −9223372036854775808 … 9223372036854775807 | same | same | unbounded `int` | `int64`; `float64` if the column holds a NULL, exact only to ±2⁵³ |
| `oid` → `UInt32` | 0 … 4294967295 | same | same | unbounded `int` | `uint32`; `float64` if the column holds a NULL |
| `real` → `Float32` | ±3.4028235e+38 | same | same | widened exactly to `float` | `float32` |
| `double precision` → `Float64` | ±1.7976931348623157e+308 | same | same | same | `float64` |
| `numeric(p≤38, s)` → `Decimal128(p, s)` | ±(10³⁸ − 1) at `(38,0)` | ±(10³⁸ − 1) at precision 38 | same | `Decimal`, exact | `Decimal` objects, exact |
| `numeric(39–76, s)` → `Decimal256(p, s)` | ±(10⁷⁶ − 1) at `(76,0)` | ±(10⁷⁶ − 1) at precision 76 | same | `Decimal`, exact | `Decimal` objects, exact |

`uuid` (`FixedSizeBinary(16)`), `bytea` (`Binary`), `boolean` and
`int2vector` (`List<Int16>`) have no range to compare; every consumer above
carries their least and greatest values.

### Where the consumers part company

- **DataFusion's calendar ends at `262142-12-31`**, long before Arrow's
  integers do: it formats dates and timestamps through `chrono`, whose calendar
  ends there. A later value is still a valid Arrow value, and DataFusion
  compares, sorts and writes it; it cannot print it, so the provider reads it
  as NULL, as it does a value Arrow cannot hold.
- **Python's calendar is the narrowest**: 0001 to 9999. `as_py()` raises
  `OverflowError` for any `Date32` or `Timestamp` outside it.
- **`time` `24:00:00`** is past Arrow's day, so it reads as NULL, or is
  refused, rather than handed to a consumer that would misread it: DataFusion
  raises `Cast error` on such a value, `to_pandas()` raises `ValueError` for
  the whole column, and `as_py()` silently returns `00:00:00` — as does
  reading it out of a `pd.ArrowDtype` column. Read such a column with
  `SchemaMode::Strings` to keep it.
- **pandas fails whole, not per value.** A `date` column holding one value past
  9999, or before year 1, makes `to_pandas()` raise for the entire table; pass
  `date_as_object=False` to get `datetime64[ms]` instead, which holds every
  `Date32`. Timestamps come across as `datetime64[us]`, which holds everything
  Arrow does; only asking for nanoseconds shrinks the range to 1677–2262.
- **pandas turns an integer column with a NULL into `float64`** by default,
  which loses `bigint` values past ±2⁵³. `types_mapper=pd.ArrowDtype` keeps
  every column in its Arrow type, NULLs included; values then cross into
  Python objects only as they are read, under Python's limits above.
- **An `interval` never becomes a `timedelta`**, which has no months field:
  `pyarrow` returns its own `MonthDayNano`, and pandas a `DateOffset`.
