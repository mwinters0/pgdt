# P12 inbox — the ADBC type floor

Facts found before this phase had a spec, filed here so its grilling meets them
rather than re-deriving them. **Drain this file when P12 is grilled**: fold each
entry into the spec or discard it as stale, then delete the file
([`../process.md`](../process.md), "Inboxes: facts filed by destination").

Every entry below shares one origin unless it says otherwise: an ad-hoc survey
of the Arrow ADBC PostgreSQL driver against `pgtype.rs`, 2026-08-31, reading the
worktree at `be5f50f08` (`CLAUDE.local.md` holds the path). The reasoning that
produced the phase is
[`../status/history/2026-08-31.md`](../status/history/2026-08-31.md), "The ADBC
driver's type mapping is a floor worth declaring".

## The floor is defined only where ADBC yields a real Arrow type

**Fact.** ADBC's bottom is not a string: anything its `SetSchema` switch does
not model becomes `Binary` carrying the raw *binary* COPY bytes, plus an
`ADBC:postgresql:typname` metadata key. `copy/reader.h`'s `NANOARROW_TYPE_BINARY`
arm says so in as many words — "we can return the bytes of any Postgres type as
binary". That bucket is large: `bit`, `varbit`, `inet`, `cidr`, `macaddr`,
`macaddr8`, `xml`, `tsvector`, `pg_lsn`, `timetz`, the geometric family, **and
every range and multirange**.

**Why this phase cares.** The rule cannot be stated over all types. A `Binary`
of wire bytes and a `Utf8View` of the file's own text are incomparable, and ours
is the more useful bottom — nobody can read ADBC's `inet`, and anybody can read
`192.168.1.0/24`. Scope the rule to ADBC's *non-opaque* answers, or it reads as
a demand to emit bytes a text dump does not contain.

## The delta, as of `be5f50f08`: five types, and four of them are not cheap

**Fact.** Comparing `SetSchema` against `builtin_scalar`, we already meet or
beat the floor everywhere except these:

| Type | ADBC | Us | What it costs |
|---|---|---|---|
| `oid` | `Int32` | `Utf8View` | Nothing — see below; not this phase's |
| `regproc` | `Int32` | `Utf8View` | Not a floor row at all — see below |
| `money` | `Int64` | `Utf8View` | Blocked by the bar — see below |
| `interval` | `Interval(MonthDayNano)` | `Utf8View` | An invariant, and a collision with P11.5 |
| `int2vector` | `List<Int16>` | `Utf8View` | A space-delimited nested codec, for a catalog type |

And we are already **narrower** than the floor for `numeric(p,s)` (decimal vs.
their string), enums (dictionary vs. their string), and ranges and multiranges
(struct and list-of-struct vs. their opaque bytes).

**Why this phase cares.** It sizes the phase honestly: the mapping table barely
moves, so the work is the rule, the oracle that checks it, and two types with
real evidence behind them. A phase scoped as "close five gaps" would be
mis-sized in both directions.

## `regproc` is not a floor row: the two encodings carry different data

**Fact.** `regprocout` (`src/backend/utils/adt/regproc.c`) returns the
function's *name* — schema-qualified when the bare name would not resolve
uniquely, and `-` for `InvalidOid`. The binary encoding ADBC reads is the OID.
So the dump's text and the wire's binary do not encode the same value, and
"narrower than `Int32`" is not a question that can be asked of the text.

**Why this phase cares.** It looks like a one-line gap and is not one. The
`reg*` family generally needs an explicit carve-out in the rule, stated as
*the floor is undefined where the text and binary encodings differ*, rather
than a per-type exception list.

## `money` is refused by our own bar, not by effort

**Fact.** `cash_out` reads the monetary locale — `frac_digits`,
`mon_decimal_point`, `mon_thousands_sep`, `currency_symbol`, `mon_grouping`
(`src/backend/utils/adt/cash.c`) — and `pg_dump` sets neither `lc_monetary` nor
`IntervalStyle` anywhere (`grep -rn 'lc_monetary\|IntervalStyle' src/bin/pg_dump/`
is empty at v16.15). So `1.234,56` and `1,234.56` cannot be told apart from the
file, and the number of fractional digits is not in it either. ADBC escapes this
only because binary hands it the raw `int64`, and it still documents its own
answer as lossy — "PostgreSQL appears to send an int64, without decimal point
information".

**Why this phase cares.** It is the phase's first *deliberate* below-floor row,
so it is the one that proves the rule needs a stance field rather than a
verdict. It likely earns a `KD<k>` under stance (a) — a consequence of a
deliberate tradeoff, never to be worked — with its paragraph beside "The bar" in
[`architecture.md`](architecture.md).

## `interval` collides with P11.5, and needs an invariant before it can be mapped

**Fact.** Two things stand between `interval` and `Interval(MonthDayNano)`.
I4 records that `pg_dump` sets `DATESTYLE = ISO` and `extra_float_digits = 3`
but **never** `IntervalStyle`, so the value's rendering depends on the dumping
session's setting. The four styles' outputs *appear* mutually unambiguous —
`P1Y2M` starts with `P`, `sql_standard` writes `1-2`, `postgres` and
`postgres_verbose` use unit words — but that is an observation, not evidence.
Separately, P11.5 closes the comparison register's `interval` row as an ordering
over the *text*; mapping the type re-keys that arm and needs its own oracle case
group.

**Why this phase cares.** It is the only below-floor type with real value in a
user's schema, and it is the single largest piece of evidence work in the phase:
a new `postgres-invariants.md` entry proving the four styles are mutually
distinguishable, with a re-verification command, before a line of mapping is
written. Getting that wrong misreads a `sql_standard` dump silently, which is
the failure mode the bar exists to prevent. Sequencing note: this lands *after*
P11.5, never beside it.

## The array census is where the floor and an existing design conflict

**Fact.** ADBC names `List<T>` from the type alone, before it has seen a value,
and its array reader reads `n_dim` and rejects only `n_dim < 0` — a
two-dimensional array is flattened into a one-dimensional list rather than
refused. We resolve `List<T>` optimistically and let the census demote a column
of mixed dimensionality or `[lb:ub]` decoration to `Utf8View`
(`ColumnResolution::VaryingArrayShape`, deficiency `KD3`). Strictly read, we are
*wider* than the floor there.

**Why this phase cares.** This decides whether P12 pulls a Future item in with
it. Two resolutions, and the second is the better one: scope the rule to
fidelity, so a narrower type bought by losing data does not count; or adopt the
Future item *"the shape-general array representation"*
(`Struct{dims, lbounds, elements}`) as the demotion target instead of
`Utf8View`, which is narrower than a string and makes the floor hold honestly
while closing `KD3`.

## Metadata is a second dimension the rule has to rule on

**Fact.** ADBC attaches `ARROW:extension:name` — `arrow.json` for `json` and
`jsonb`, `arrow.uuid` for `uuid` — and `ADBC:postgresql:typname` on every
opaque field. We attach **no field metadata at all**: our `uuid` is the same
`FixedSizeBinary(16)` without the extension name, and our `json` the same string
without it.

**Why this phase cares.** The phase cannot say the floor is met without deciding
whether metadata counts, and if it does, three columns are below it today on
metadata alone. The `typname` half is independently worth having for our own
opaque columns — a consumer receiving `Utf8View` currently has no in-band way to
learn the column was `inet`.

## Take the floor mechanically; it moves

**Fact.** The floor is one C++ `switch` in an actively developed upstream —
`be5f50f08` is 2026-08-30, and nothing announces a change to it.
`generate_fixtures.py` already stands a Postgres container per major and takes
the comparison oracle with `--skip-dumps`; the same apparatus can create one
column per type, query it through `adbc_driver_postgresql`, and commit the
resulting `ArrowSchema`.

**Why this phase cares.** A transcribed table decays silently and a generated
one does not, which is the standing rule "verify objectively wherever possible"
applied to somebody else's code. The oracle must also **record the ADBC revision
it was taken at**, for the same reason the comparison oracle records its
platform triple. **Contingent on:** that the Python driver installs cleanly
under `uv` here — unverified, and the first thing to check when this phase
becomes current, because the whole shape above depends on it.

## Two name-resolution gaps are the same shape as the array one

**Fact.** ADBC resolves every type by OID against the catalog. We resolve by
declared name against the dump's own `CREATE TYPE` list, which misses a type
name that needs quoting (`KD4`) and a composite whose body the grammar could not
read — both landing on `Utf8View` where ADBC would produce a real type.

**Why this phase cares.** The rule has to say whether a *lookup* failure counts
as being below the floor, or only a mapping choice does. If it counts, the
Future item *"a real type-name tokenizer"* becomes this phase's, and `KD4`
acquires an owner; if it does not, say so, because the check will otherwise
report it every run.
