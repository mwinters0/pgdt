# P28 — Unrepresentable values

What this phase will do and why; how it lands is its slices'. Progress is
`STATUS.md`'s checklist, never this file. **Its first slice exists to produce
evidence** — the harness that holds every query to one outcome — so slice
numbers after it are allocation order as much as schedule. Grilled 2026-09-29.

## Premise

**A query's outcome never depends on which rows it happened to read.** A typed
column holding a value its Arrow type cannot hold refuses wherever a read
reaches that value (`KD8`), and a DataFusion query need not read every row —
a `LIMIT` one partition meets first, a dynamic filter another partition
tightened, a row a dynamic filter's state rejects before it decodes
(`decisions.md`, "D93") — so the same query answers on one run and refuses on
the next ([`../status/history/2026-09-27.md`](../status/history/2026-09-27.md),
"A `KD8` refusal decided by timing is a defect, and P28's").

## Scope

**How an unrepresentable value is handled is the user's choice, among three
modes, and every map supports all three.** The choice is made at query time;
`parse` records what each mode needs whatever it was asked to gather, since
the mode is chosen after the cache is written.

- **Typed, the default**: the column keeps the type the ADBC floor sets ("D38"), and an
  unrepresentable value is read as NULL. Correct type, value lost; such
  values are rare in real dumps, which is why it is the default.
- **Untyped**: a column holding such a value is read as `Utf8View`, each value
  its text. Correct value, type wider than the floor.
- **Refuse**: today's behaviour, both type and value kept correct at the cost
  of the answer — made deterministic, so it no longer depends on which rows a
  read reached.

**An unrepresentable value is one PostgreSQL accepts for the declared type
and the column's Arrow type cannot hold**, one category the three modes treat
alike: `±infinity` on a `date`, `timestamp`, `timestamptz` or `interval`,
`NaN` on a `numeric(p,s)`, an `interval` time part past Arrow's range, a
`timestamp` or `timestamptz` past `Timestamp(Microsecond)`'s bound — the last
three decades PostgreSQL admits, to `294276-12-31` — and `time` `24:00:00`,
past `Time64`'s, no special case among them. **The definition governs and
this list follows from it**; what Arrow's own spec forbids a type to hold
counts, not only what a decoder refuses.

**"Cannot hold" is decided by layer, each limit where it is owned.** The
library and `pgdt` hold a value to **Arrow's format spec** — the list above —
the one line neutral across consumers they cannot know. **The DataFusion
provider adds its engine's**: `arrow-cast` formats a `date` or timestamp
through `chrono`, whose calendar ends at `262142-12-31` (`RT21`), so there a
`date`, `timestamp` or `timestamptz` past it is one as well, each mode acting
on it as on the rest; `pgdt query` prints it. Refused: `chrono`'s bound in
every layer, which binds every library consumer to one engine's limit while
Python's `datetime` stops at 9999 and `java.time` far past it; and the format
spec alone, which leaves DataFusion's display refusing the value wherever a
read reached it, the outcome the Premise removes. **A nested value holding one is
one**: an array element, a range bound or a composite's field makes the whole
value unrepresentable, and each mode acts on the whole — the typed mode's NULL
being the array, range or composite, since a NULL range bound would mean
unbounded. **Text that does not parse as its type is not one**:
it is outside the input contract (`roadmap.md`, "The input contract is valid
PostgreSQL, not `pg_dump`'s output") and refuses in every mode.

**Under the typed mode the value is NULL for every purpose**, as though the
dump held one: a pushed filter evaluates it in three-valued logic and
`IS NULL` matches it, statistics' NULL counts include it and their bounds
exclude it, `COUNT(<column>)` does not count it — so a pushed filter answered
`Exact` ("D88") and DataFusion's own evaluation keep the same rows. Under the
other two a filter keeps PostgreSQL's order, as today ("D56").

**The refuse mode refuses at planning, by column**: a query materializing a
column — projecting it, or handing DataFusion an expression over it — refuses
wherever the map says the table holds such a value in it, in every block,
before a row is read. A column read only by a filter the library answers is
not materialized, its text compared in PostgreSQL's order. Over the groups a
static filter keeps is refused: which survive depends on statistics, and
statistics never change an answer. So `pgdt query` refuses a query whose
filter excludes every such row, which it answers today. The refusal names the
column and its count, and the other two modes.

**Each block counts, per column, the values no Arrow type of the column's
could hold, beside the array-shape census** ("D35"): a lexical test on the
still-escaped bytes of each leaf — exactly `infinity`, `-infinity`, `NaN` or
`24:00:00`, an hour part past the `interval` bound, or a year part past the
`timestamp` one, only a year of exactly 294247 taking arithmetic, a
`timestamptz`'s offset moving the bound within it — and never otherwise a
decode. **It is two tiers per column**: past Arrow's format spec, which every
layer reads, and within it but past `chrono`'s calendar — a `date` or
timestamp year past 262142, only 262142 and 262143 taking a `timestamptz`'s
offset — which only the provider adds. **The cache records the calendar bound
it counted under**, and a cache counted under another is refused, naming
`parse`, never read as current nor replaced ("D20").
**The leaves are the column's declared type's**: a nested value is
walked by its declared PostgreSQL type, which the dump's DDL states, and a
leaf is tested against its own type's spellings, so a `text` field reading
`infinity` beside a `date` one is never counted. The cache so stays a
function of the dump, not of the mapping, which is what the census's
type-blindness protects; the census itself stays type-blind. The count is
consulted only where the resolved type cannot hold the value, so it is not
billed as a statistic, and a declined block ("D85") and `KD33`'s tail carry
it.

**`pgdt` serves two uses, named the *metadata* level and the *data*
level**: information about a dump, and queries over it. The metadata level
records a block's location and row count and nothing drawn from its data —
**no census, no unrepresentable count, no statistics** — so it decodes and tests no field; a
user choosing it has opted out of the query affordances, the correctness a
census buys among them. Today's census under `none` ("D35", "every mapping
pass censuses") is changed here, keeping the code the count joins coherent.
**A table at the data level is censused and counted in every column**, the two
being query affordances rather than statistics: a column selection governs
only what is billed as one.

**One option states the level, `--statistics-level`, replacing
`--statistics all|none|<list>`**: a default for every table, first, then
overrides — `data`, the default; `metadata`; `metadata,public.foo=data`,
every table at the metadata level but `public.foo`; `metadata,public.foo.bar=data`,
`public.foo` at the data level with statistics on `bar` alone. A target is
spelled as today's selection spells one (`schema.table`, a bare `table`,
`schema.table.column`). A data-level table without statistics is not offered,
`all` and `none` going as a separate axis. Column opt-in is kept because it
exists and serves wide tables, to be reconsidered before 1.0 against the
simplicity of "data means every column"; table-level opt-in is expected to be
the more used. The library's `StatisticsSelection` becomes the same shape, a
default and overrides, "D77"'s reasoning intact. Re-parsing under a wider
level re-reads only the blocks lacking what is now asked, as `--statistics`
does, and never replaces anything.

**The most specific entry wins, and every column has a level**: its own
entry's, else its table's, else the default — a bare `table` less specific than
`schema.table`, the order after the default immaterial. A table is censused and
counted where any of its columns is at the data level, and a column gathers
statistics where it is. So `data,public.big=metadata` opts a table out,
`data,public.foo.blob=metadata` keeps `foo` at the data level with statistics
on every column but `blob`, and `metadata,public.foo=data,public.foo.bar=metadata`
says the same the other way. Two entries of one specificity naming the same
column are refused; an entry restating what it would inherit is not.

**A query over a table the map holds at the metadata level has a cold query's
semantics**: `pgdt query` and the library map that table's blocks at the data
level as a cold query does — census and count, no statistics — hold them for
that query alone and write nothing, the cache keeping the level `parse` gave
it; the cost is a second read of the table. Every mapping pass a query makes
censuses and counts, so opting out at `parse` costs time, never correctness.
The provider refuses, as it refuses anything but a complete map, naming the
`parse` that puts the table at the data level. Refused: a typed query refusing
at planning; degrading, which leaves arrays and the modes to timing. Without a
census an array column keeps its DDL's one-level `List`
(`resolve::retype_from_census`) and a deeper value refuses where a read reaches
it, the defect this phase removes. `--schema-mode strings` needs neither, and
`info` reports each table's level.

**The mode is one option, stated where a dump is opened**, the provider
fixing a table's schema when it builds it: `QueryOptions::unrepresentable`
(`Null`, the default, `Text`, `Refuse`), `pgdt --unrepresentable
null|text|refuse`, `PgDumpOptions`, and in the shell a `pgdump.unrepresentable`
table option beside `pgdump.schema_mode` and a `--dump` suffix beside
`:strings`. It is apart from `--schema-mode`, which is about ignoring the DDL,
and moot under `strings`. The library's refusal names the modes, not any front
end's flag.

**The typed mode warns of a property of the table, never of the run**: once
per materialized column holding such a value, at planning, a `Finding`
through the library's `DiagnosticSink` as a collation note is — "`<table>.<col>`
holds N values `<type>` cannot hold, read as NULL", naming the predicate term
below. The count is the map's; what a run skipped depends on a `LIMIT` or a
dynamic filter, so it is never stated. `pgdt` prints it to stderr, the shell
through its sink, and `pgdt info --detail` shows the count per column.

**A column the untyped mode widens is a `ColumnResolution` of its own**, and
compares in each front end's semantics: in Arrow's (the DataFusion
translators) by its bytes, the order DataFusion evaluates on a `Utf8View`, so
a pushed filter stays `Exact` and statistics, stored in the declared type's
order, prune nothing on it; in PostgreSQL's (`pgdt --where`) in the declared
type's order, special values ranked ("D56"), `pgdt` having no second engine to
disagree with. Refused: text everywhere, `pgdt --where` refusing its order as
it does an unmapped column's (`predicate.rs`, `NOT_MAPPED`). `t_date` and
`t_numeric` are fixture columns reaching the variant.

**"D38" binds the typed mode.** The untyped mode, like `--schema-mode
strings`, is the user asking for a type wider than the floor, and widens only
a column the map says holds such a value, in the tiers its front end reads —
so the provider widens a column holding only the engine tier's where `pgdt`
keeps it typed; the entry gains that clause, and `floor_mapping.py` checks the
typed mode alone.

**A statistics group keeps two views where they differ**: its bounds over
representable values, its count of unrepresentable values, and — only where
that count is not zero — its bounds in PostgreSQL's order over every value.
The typed mode reads the first pair and adds the count to the group's NULL
count, so exact bounds, `IS NULL` truths, an `Exact` `COUNT(<column>)`
("D89") and pruning survive it; the refuse mode and PostgreSQL's semantics
read the second pair where it exists. Refused: PostgreSQL-order bounds and the
count alone, the typed mode giving up a group's bounds wherever such a value
occurs. Both are statistics, absent at the metadata level. **The engine tier
adds a view of its own**: where a group's count of it is not zero, its bounds
over values at or before the calendar end, which the provider's typed mode
reads, adding that count to the NULL count as well. Refused: dropping a bound
past the calendar end in the provider, which gives up pruning and an `Exact`
`MIN`/`MAX` on the group to save a field.

**A dynamic filter's per-row evaluation ("D93") follows the static filter's**:
under the typed mode it evaluates the value as NULL, so a row its state
rejects before decoding hides no refusal, none being raised; under the refuse
mode a materialized column holding one refused at planning. `pgdt query
--statistics none`'s help, "`none` is also how to find one in data left
unread", is superseded by the predicate term.

**A user can ask which rows of a column held an unrepresentable value**, so
that a NULL the dump holds is told apart from one the typed mode produced:
**two operators, `IS UNREPRESENTABLE` and `IS NOT UNREPRESENTABLE`**,
two-valued as `IS NULL` is and taking no value, reopening "D53"'s closed set.
The library evaluates them on the text against the column's *declared* type —
`pgdt --where '<column> is unrepresentable'`, and in DataFusion a scalar UDF,
`pgdump_unrepresentable(<column>)`, `NOT` above it the negation, always pushed
`Exact` and refusing at planning wherever DataFusion would have to evaluate it
itself, a NULL no longer carrying its origin. Each tests the tiers its front
end reads, `pgdt` the format spec's and the UDF both, so each tells apart
exactly the NULLs its own typed mode made. It answers in the typed, untyped
and refuse modes — in the last where the column is not materialized — and
refuses under `--schema-mode strings`, which reads no declared type and makes
no NULL of one; `IsNull`'s "matches only a NULL field (`\N`)" is amended for
the typed mode. Refused: a table function listing each
occurrence, which no row identity joins back to its row; a companion column
per affected column, which DataFusion 55.1's lack of hidden columns puts in
every `SELECT *`.


## Evidence

**The first slice is the harness, and today's tree fails it**, each case
recorded as failing where it lands and turned green by the slice that fixes
it. `M176`'s
node running a scan's partitions in a stated order
(`datafusion-pgdump/tests/in_order/`) runs each query shape in every order —
a `LIMIT` one partition meets first, an ungrouped `MIN`/`MAX`, a TopK, a
join, and `pgdump.dynamic_filter_rows`' row drop — under each mode, with
statistics used and not, dynamic filters on and off, over majors 13, 16 and
18, and asserts one outcome per query, the refuse mode's being one refusal.
`statistics_never_change_an_answer` returns to DataFusion's own schedule,
`M176` having pinned it to file order only until this phase.

**The category's list is held complete by a test, not by review**: the
`types` fixture holds PostgreSQL's extremes of every type the floor maps —
least and greatest, the infinities, `NaN`, `24:00:00`, `interval`'s included
— and each must decode to a value its Arrow type's spec admits or be counted
unrepresentable, Arrow's validity rather than a decoder's refusal being the
test. **Arrow's validity is DataFusion's own path**: the column built, each
value formatted by `arrow-cast`'s display and cast to `Utf8`, an error in
either being unrepresentable — no bound written per type, which would be the
list again. The record states each value's tier, format spec or engine, and
**28.3's count is held to it tier by tier over the same rows**, so the
fixture's rows either side of `chrono`'s calendar end fail `mise run check`
the moment a `chrono` or `arrow-cast` upgrade moves it. **A base type a later PostgreSQL major introduces is held to the
same test**, by a reconciliation beside `floor_mapping.py`'s: every
`builtin_scalar` arm to a type other than `Utf8View` has an extremes row and
every row an arm, so a new major's `floor.tsv` forces a mapping decision
("D38") and a typed mapping forces its extremes. The values are written by
hand, PostgreSQL keeping no catalog of a type's least and greatest.
**`interval`'s extremes**, at every major: the time parts
`±2562047788:00:54.775807`, PostgreSQL's longest; `2562047:47:16.854776`, a
microsecond past Arrow's; `2562047:47:16.854775`, inside it; and an
`interval[]` holding one — and from 17, gated as the fixture gates a
multirange, `infinity` and `-infinity`.

**The figures move with the metadata level.** `measure.py` times every parse
whose subject is not statistics at `--statistics none` (`NO_STATISTICS`), a
census included today. **Scan figures time the metadata level**, the scan
alone, and are re-taken. **Query figures build a data-level cache untimed**
and query with `--statistics none`, so no pruning enters the reading. **The
census's price, the count now inside it, is an attribution**, read off a
`perf` profile of a data-level parse (`roadmap.md`, "Attribution is
introspective; only the gate is blind"): the `census-*` differencing figures
and the pinned census-off build (`runs/pgdt-nocensus`) retire. Refused: a
census-only level a user can reach, kept only for the harness to difference.

## Slices

**Evidence first, then the map, then each mode**, a rework of a tested core
path never sharing a slice with a new mechanism:

1. **The harness** (Evidence), no product code, its cases recorded failing.
2. **The metadata and data levels**: `StatisticsLevel` and
   `--statistics-level` with its overrides; the census gated on the level, the
   metadata level splitting no field; `info` reporting each table's level; a
   query's cold semantics over a metadata-level table, and the provider's
   refusal of one. "D35" and "D77" amended — a rework of the mapping pass.
3. **The count beside the census**: lexical, per block and column, each
   leaf walked by the declared type, in two tiers, in the cache format with
   the calendar bound it counted under; `info --detail` showing it per
   column; `decode_time64_micros` refusing `24:00:00`; the count and the
   extremes record held to each other by tier.
4. **Statistics' views**: gathered, cached, and read by pruning under
   each semantics, the engine tier's included.
5. **The mode option and the typed mode**: `QueryOptions::unrepresentable`,
   NULL for every purpose — decode, the static and dynamic filters, NULL
   counts, "D89"'s statistics — the warning's `Finding`; `pgdt
   --unrepresentable`, the provider's option, the shell's table option and
   suffix; the refusal naming the modes.
6. **The refuse mode**: by column, at planning, from the map.
7. **The untyped mode**: the widening `ColumnResolution` and its comparison
   in each semantics; "D38"'s clause and `floor_mapping.py`. The harness is
   green here, and `KD8` closes.
8. **The predicate term and its UDF**; "D53" amended.
9. **The figures** (Evidence): scan figures at the metadata level, query
   figures over a data-level cache, the census's price attributed by a `perf`
   profile, the `census-*` figures and the census-off build retired.
10. **The extremes** (Evidence), no product code, admitted after the spec
    and landing after 1 and before 2, its fixture being what 3's count is
    tested against: an extremes table in `fixture_schema_types.sql` — every
    typed arm's least and greatest, the special values, `24:00:00`,
    `interval`'s, nested cases — every major's fixtures regenerated; the
    reconciliation beside `floor_mapping.py`; the DataFusion-path validity
    test, its failing values recorded; the harness extended over the new rows.

## Facts found while grilling

- **The mode is fixed where a dump is opened, and covers every column.**
  `pgdt query --schema-mode`, `QueryOptions::schema_mode`,
  `PgDumpOptions::schema_mode`; in the shell the `--dump <src>:strings` suffix
  or `CREATE EXTERNAL TABLE`'s `pgdump.schema_mode` option — a table option,
  not a setting `SET` reaches (`datafusion-pgdump/src/settings.rs`). The
  provider resolves a table's schema once, at `PgDumpTable::build`, so a mode
  that changes a schema cannot be a per-statement setting there.
- **The census is per block, gathered by every mapping pass** ("D35"),
  `CopyBlock::array_shapes`, merged per column at query time
  (`index::union_census`); statistics are the separate, optional
  `CopyBlock::statistics`. A count the modes need can ride beside it.
- **The library can query cold; the provider cannot.** `table_stream` maps
  before it replays, so it has a map of the blocks it reached; the provider
  refuses without a complete one and names `pgdt parse`
  (`datafusion-pgdump/src/dump.rs`).
- **Which values are reachable**: `±infinity` on `date`, `timestamp`,
  `timestamptz` and (from PostgreSQL 17) `interval`; `NaN` on
  `numeric(p,s)`, a typmod refusing `±Infinity` (`pgtype.rs`); an `interval`
  time part past Arrow's nanoseconds; a `timestamp` or `timestamptz` from
  `294247-01-10 04:00:54.775807` UTC, where `i64` microseconds from 1970 end,
  to PostgreSQL's `294276-12-31 23:59:59.999999`, which counts from 2000
  (`decode.rs`,
  `postgresqls_own_max_timestamp_overflows_the_unix_epoch_i64_range`;
  `t_timestamp`'s `id` 7 at every major); and `time` `24:00:00`. Bare
  `numeric`, and a precision past 76, is already `Utf8View`; every finite
  `date` fits `Date32`, and PostgreSQL's earliest `timestamp` fits Arrow's.
  Each decoder but `time`'s answers `None` (`decode_date32`,
  `decode_timestamp_micros`, `decode_interval`, `decimal_unscaled_digits`), and
  `RowBatcher::push_field` or `ResolvedTerm::eval` raises `Error::FieldDecode`.
- **DataFusion displays a `date` or timestamp only to `262142-12-31`**:
  `arrow-cast` converts through `chrono`, whose `NaiveDate` packs the year
  into an `i32`'s high 19 bits (`RT21`), so `5874897-12-31` and `i64`'s last
  microsecond decode and print `ERROR: Cast error` (`t_extremes` ids 2, 12,
  15). `pgdt query` prints them, its renderer (`decode.rs`,
  `render_date32_into`) holding no `chrono`. The bound lies below `i64`'s, so
  the provider's tier needs no second timestamp bound.
- **`time` `24:00:00` decodes to a value Arrow forbids**:
  `decode_time64_micros` answers `86_400_000_000` by design, where Arrow's
  `Time64` holds `[0, 86400 s)` (`Schema.fbs`, `Time`; `arrow-ipc` 59.2.0's
  `gen/Schema.rs`). DataFusion 55.1 prints the cell as `ERROR: Cast error:
  Failed to convert 86400000000 to temporal for Time64(µs)`, and `MAX` over
  it answers that error, over `fixtures/16/types/default.sql`'s `t_time`
  `id` 1. So "the one decoder refuses it" was never the whole category.
- **A new major's base types already reach the floor mechanically**:
  `scripts/adbc_floor.py` sweeps every declarable `pg_catalog` type into
  `fixtures/<major>/adbc/floor.tsv`, and `floor_mapping.py` fails on a floor
  row the driver types that `builtin_scalar` does not answer, or an arm no
  row backs ("D38"). A type left `Utf8View` holds any text, so only a typed
  arm can hold an unrepresentable value.
- **A nested value carries the same values**: a `date[]` element, a
  `daterange` bound or a composite's field may be `infinity`, decoded by the
  same scalar decoders beneath `append_typed`'s nested arms; the category's
  list names none, and a lexical test on the whole field matches none.
- **A filter orders a special value by rank, not text** ("D56"):
  `predicate::special_order_key`, `-infinity` below every finite value,
  `infinity` above, `NaN` above that.
- **Nothing counts a value that does not decode**; gathering only drops the
  column's sums (`ColumnStatistics::sums`) and NULL counts are `\N`s
  (`gather.rs`, `ColumnGatherer::observe`).
- **No row identity exists** — no row id, no virtual column; DataFusion 55.1's
  `virtual_columns` are file sources' alone and appear in `SELECT *`, and a
  `TableProvider` has no hidden-column mechanism.
- **The refusal's remedy is the library's text** (`error.rs`,
  `Error::FieldDecode` and `Error::MetadataNotScanned`), passed through by
  the provider unchanged, so the shell names `pgdt`'s flag.
- **ADBC's floor does no better** (`fixtures/*/adbc/floor.tsv`, ADBC 1.12.0,
  and upstream `main` at `616acfdfc`): every `numeric` is `arrow.opaque` over a
  string, `NaN` and the infinities spelled `nan`, `inf`, `-inf`; a `date` or
  `timestamp` infinity arrives as a wrong value, the epoch offset added to
  PostgreSQL's sentinel unchecked (`c/driver/postgresql/copy/reader.h`). So
  the typed mode matches the floor's type and does better on its data, and
  only the untyped mode on a `date` or `timestamp` needs "D38"'s clause.
- **The census is "D35"**, not "D43", which is resolution's
  `MetadataNotScanned`.
