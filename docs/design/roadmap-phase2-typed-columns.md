# Phase 2 (Typed columns) — Specification

Recover column types from the dump's own DDL and return properly typed Arrow
arrays instead of all-`Utf8View` ones. Supersedes the Phase 2 sketch in
`docs/design/roadmap.md`, which is kept there for the reasoning behind the
metadata companion.

**Status: specified, not built.** Nothing here is implemented yet.

Invariants of `pg_dump`'s output that this design treats as guaranteed are
recorded, with their source evidence, in `docs/design/postgres-invariants.md`
(referenced below as I1–I8). Pre-1.0 there are no compatibility obligations —
see "Pre-1.0" in `docs/design/roadmap.md`.

## Output model

**Typed is the default, with per-column string fallback.** `BatchOptions` gains
a `SchemaMode`:

- `Typed` (default) — each column gets the narrowest Arrow type this build can
  decode from its declared PostgreSQL type. A column whose type has no mapping
  stays `Utf8View`, exactly as Phase 1 produced it.
- `Strings` — every column is `Utf8View`. Reproduces Phase 1 byte-for-byte, and
  is the escape hatch for a caller who distrusts the mapping or needs a stable
  schema.

An Arrow `RecordBatch` carries a `DataType` per column, so a partially-typed
schema (`id: Int32, extra: Utf8View`) is an ordinary mixed schema, not a
special case. This is deliberately a **progressive enhancement** model: each
release narrows more columns, and a column's type is expected to change between
releases as coverage grows. `SchemaMode::Strings` is the crude way to pin a
schema against that; the Future item "caller-supplied type mapping" in
`docs/design/roadmap.md` is the real one.

Typing is a per-column decode function chosen once per block, not a second
batcher — the assembly path in `batch.rs` stays single, and the zero-copy
`Utf8View` machinery it already has is what unmapped columns keep using.

## Schema resolution

**The `COPY` header is authoritative for which columns exist and in what order;
the DDL is a by-name type lookup and nothing more.** Every way the two can
disagree then degrades in one direction — an unexplained column stays
`Utf8View` with a diagnostic — and no dump shape can produce a *wrong* column.
The disagreements are real, not hypothetical (I5):

- Generated and dropped columns are in `CREATE TABLE` and never in `COPY`.
- `--binary-upgrade` re-creates dropped columns in the DDL as
  `INTEGER /* dummy */`.
- A typed table (`CREATE TABLE x OF t`) has no column list in its DDL at all.
- `--data-only` has no DDL whatsoever — the uniform case where every column is
  unexplained, which is exactly Phase 1's behaviour plus one diagnostic saying
  why.
- A table with no undropped columns emits `COPY x  FROM stdin;` with no column
  list at all, which is where Phase 1's placeholder names come from.

**One schema per stream.** The schema is resolved once, up front, and held for
the whole stream. This is close to a tautology within a single-database dump,
because a table produces at most one `COPY` block (I2) — but it is the contract
Phase 5's `TableProvider` needs, and it forecloses any design where a column's
Arrow type depends on how far a scan happened to get.

## Multi-database dumps

`pg_dumpall` output, and concatenated dump files, contain several databases'
dumps in sequence, each introduced by `\connect`. This is the only way one
table name can own more than one `COPY` block (I2).

- The preamble search **re-arms at each `\connect`** — the pre-data ordering
  invariant is per database, not per file (I1).
- A query **never spans databases**. A table query that matches blocks in more
  than one database is an error naming the candidates, not a silent union of
  two different tables under one schema.
- Single-database dumps — every fixture, and koji — never trigger the check.

Declining to support this shape would not have protected anyone: the file still
parses, blocks still match by name, and the wrong answer still comes out.

## The preamble pass

One pass over the pre-data region recovers everything Phase 2 needs:
`CREATE TABLE` column types, `CREATE TYPE` / `CREATE DOMAIN` definitions,
`CREATE EXTENSION` lines, and the two version header lines. They are all
pre-data, all ahead of the first `COPY` block (I1), and all wanted at once.

**Structure discovery uses the TOC comment as a segmenter; a strict grammar
parses the statement.** `-- Name: <name>; Type: <desc>; Schema: <s>; Owner: <o>`
precedes every object in plain-format output and is not suppressed by
`--no-comments` (I3). The parser therefore knows what it is looking at before
it looks, and can skip whole objects it does not care about (`FUNCTION`,
`INDEX`, `ACL`) without parsing them. A statement that does not match the
grammar its comment announced produces a diagnostic, never a guess.

TOC comments are a segmentation *hint*, never load-bearing for correctness: a
`--data-only` dump has neither comments nor DDL and must still work, and a
dollar-quoted function body could contain a line that looks like a header.

**When it runs.** On a cache miss only. A cold `Typed` query reads from the
file start to the first `COPY` header and persists the result in
`DumpMetadata`; a warm query reads neither. koji's preamble is 2,475 lines
(~120 KB) against a 784 GB file, so the cold cost is not measurable. The
metadata carries its own **coverage watermark** — which databases' preambles
have actually been read — so a query for a table whose DDL was never scanned
can say so rather than silently returning untyped columns.

The rejected alternative is lazy resolution (type a column once its DDL happens
to have been seen), which makes the Arrow schema depend on scan progress.

**A full metadata scan is `pgdq parse`, not a new flag.** `parse`'s job grows
to include metadata; it already means "scan the whole file eagerly and persist
what you find". An incremental `Typed` query reads preamble as it goes and
records coverage; if it resolves a table whose database's preamble it never
covered, it errors with `metadata for database <db> was not scanned — run
'pgdq parse <file>' first, or use --schema-mode strings`. Detection and remedy
in one message, on the eager/incremental axis Phase 1 already built. A `--full`
flag on `query` would be a third way to say the same thing.

**`DumpIndex::scanned_through` is what makes "not present" distinguishable from
"not yet scanned".** The field already exists and already advances as
incremental scans progress. Compared against the file size it becomes a
completeness proof: `scanned_through == size` means the whole file has been
seen, so a database or table absent from the index is **definitively absent**
and the query can say "no such table" rather than "coverage unknown". Below the
file size, the only honest answer for a miss is that we have not looked
everywhere yet. This is also the natural home for the metadata coverage
watermark — per-database preamble coverage is a refinement of the same idea,
not a second mechanism.

**`--cache-path none` disables persistence, not typing.** The preamble is
re-read on every invocation instead of being served from cache; output is
identical. Disabling the cache is a statement about writing files, and letting
it change the *schema* a query returns would reintroduce exactly the
accident-dependent typing that "resolve once, up front" rules out. `pgdq parse`
always populates metadata — ~120 KB against a 784 GB file is never worth a flag
— and continues to reject `--cache-path none` outright.

**Dollar-quote tracking closes the Phase 1 known gap.** The scanner gains
`$tag$ ... $tag$` state tracking across the file. In Phase 1 the gap was one
rare misparse; in Phase 2 the preamble is exactly where function bodies live,
so an unguarded body can produce a fake `CREATE TABLE`, a fake TOC comment, or
the original fake `COPY` header — and the failure mode becomes wrong types on a
real table. The outside-block path is already line-oriented over a few MB, so
the cost is not measurable, and
`docs/design/roadmap-phase6-scan-performance.md` explicitly preserved the
option.

## What the cache stores

`DumpIndex::metadata` (reserved and empty since Phase 1) is populated with
**what the dump said, never what we concluded**: declared PostgreSQL type
strings per column, the extension list, the version headers, `CREATE TYPE` /
`CREATE DOMAIN` definitions, and the coverage watermark.

The Arrow mapping is a *versioned opinion* that changes as coverage grows.
Caching resolved Arrow types would recreate exactly the staleness hazard the
roadmap flags for Phase 4 statistics — a cache written under an older mapping
silently reused under a newer one. Declared type strings are durable facts
about the file that cannot go stale while the file does not change.
Re-resolving on load is a few thousand string comparisons.

### `DumpMetadata` shape

Keyed by database, with database identity explicit:

```rust
struct DumpMetadata { databases: Vec<DatabaseMetadata> }

struct DatabaseMetadata {
    /// `None` for a plain `pg_dump` output, which has no `\connect` and so
    /// names no database. Never guessed.
    name: Option<String>,
    /// Whether this database's preamble was read to completion.
    preamble_complete: bool,
    server_version: Option<String>,
    pg_dump_version: Option<String>,
    extensions: Vec<Extension>,
    types: Vec<TypeDef>,
    /// Qualified table name -> (column, declared type) in DDL order.
    tables: BTreeMap<String, Vec<(String, String)>>,
}
```

Declared types are stored **as strings, exactly as the dump wrote them** —
`character varying(16)`, not a parsed `(base, typmod)` pair. Typmod parsing
happens at resolution time, so a change to how we interpret a type can never
invalidate a cache. This is "store what the dump said, never what we concluded"
made concrete.

`preamble_complete` is per-database because that is the granularity the
coverage question is asked at, and it composes with `DumpIndex::scanned_through`
rather than duplicating it: the index watermark says how far the file has been
walked, this says whether a given database's DDL was fully understood.

### Nullability

**Every Arrow field is nullable in Phase 2**, including columns the DDL
declares `NOT NULL`.

`Field::nullable` is a contract, not a hint: a non-nullable field whose array
contains a null is a corrupt `RecordBatch`, and DataFusion optimizes on the
strength of it. Trading a modest optimizer hint for the chance of propagating a
malformed batch into an engine that trusts the flag is the wrong side of that
bargain. It is also asymmetric with `--data-only`, where the identical table
would come back nullable purely because of how it was dumped — the same
accident-dependent schema ruled out under "Schema resolution".

Revisit in Phase 4 alongside statistics, where `null_count` gives a *verified*
basis for the claim rather than a declared one.

## Type mapping

The bar is **"the dump alone determines the value."** A type is mapped only if
its COPY TEXT rendering round-trips without consulting anything outside the
file. Everything else stays `Utf8View` with a diagnostic naming the *kind* of
unknown. This is the same asymmetry Phase 1 chose for the `COPY` grammar: not
recognizing something is recoverable and inspectable; misreading it is not.

The koji census says this bar is not restrictive in practice — `integer` (246
columns), `text` (108), `boolean` (56), `timestamp with time zone` (38),
`character varying(n)` (17), `bigint` (4), `double precision` (3) account for
almost every column in a real 75-table schema.

| Declared type | Arrow type | Notes |
|---|---|---|
| `smallint`, `integer`, `bigint` | `Int16`, `Int32`, `Int64` | |
| `boolean` | `Boolean` | Rendered `t` / `f` |
| `real`, `double precision` | `Float32`, `Float64` | `extra_float_digits = 3` guarantees exact round-trip (I4). `NaN`/`Infinity`/`-Infinity` parsed explicitly — Rust accepts `inf`/`NaN`, not PostgreSQL's spellings |
| `numeric(p,s)` | `Decimal128` (p ≤ 38), `Decimal256` (p ≤ 76) | `NaN` bypasses PostgreSQL's precision/scale check and is reachable through *any* numeric column, typed or not (confirmed: `fixtures/*/types/*.sql`, `public.t_numeric.v_small numeric(10,2)`) — hits the "non-finite in a decimal" `FieldDecode` case under "Failure and diagnostics" below, same as the untyped column |
| `numeric` (no typmod) | `Utf8View` | Arbitrary precision, plus `NaN`/`Infinity`, have no Arrow decimal representation |
| `text`, `character varying(n)`, `character(n)`, `name` | `Utf8View` | Already correct in Phase 1 |
| `date` | `Date32` | |
| `timestamp without time zone` | `Timestamp(Microsecond, None)` | Wall-clock reading, no offset in the data |
| `timestamp with time zone` | `Timestamp(Microsecond, Some("UTC"))` | Offset is explicit in the data and normalized to UTC (I4) |
| `time without time zone` | `Time64(Microsecond)` | |
| `time with time zone` | `Utf8View` | Offset semantics map to no Arrow type |
| `interval` | `Utf8View` | `IntervalStyle` is never recorded (I4) — the file does not determine the value |
| `uuid` | `FixedSizeBinary(16)` | Canonical 36-char form |
| `bytea` | `Binary` | `\x48656c6c6f` after COPY unescaping |
| `json`, `jsonb` | `Utf8View` | Arrow has no JSON type |
| `inet`, `cidr`, `macaddr`, `macaddr8` | `Utf8View` | |
| enum (`CREATE TYPE ... AS ENUM`) | `Dictionary(Int32, Utf8)` | Only when the label set is non-empty |
| domain (`CREATE DOMAIN`) | base type's mapping | Resolved transitively |
| array, composite, range | `Utf8View` | Phase 3 — they share one nested-quoting decoder |

Microsecond precision throughout, because that is PostgreSQL's storage
resolution. Nullability is not refined by `NOT NULL` — see "Nullability" above.

**Looking up a declared type is a two-way split on the qualified name** (I8).
`pg_dump` empties `search_path`, so built-ins are written bare (`integer`,
`character varying(16)`) and user-defined types schema-qualified
(`public.mood`) — matching the form their own `CREATE TYPE` uses. A `.` in the
declared type is therefore a cheap first discriminator between "look this up in
the built-in table" and "look this up in the metadata's type list".

**Enums and the `--binary-upgrade` trap.** Labels are recoverable in every
mode: a `--binary-upgrade` dump emits an empty `AS ENUM ()` body and follows it
with one `ALTER TYPE x ADD VALUE '<label>';` per label, still in the preamble,
still in the same TOC entry (I6). The preamble parser reads both forms. The
empty-label fallback to `Utf8View` remains for genuinely empty enums.

This makes the metadata pass a **hard prerequisite** for typing, not just an
`info` nicety.

## Failure and diagnostics

Two channels, deliberately separate.

**Decode failures are hard errors.** A value that does not parse as its mapped
type — `12x` in an `integer` column, a non-finite in a decimal, an out-of-range
or `infinity`/`-infinity` `date` or timestamp — raises
`Error::FieldDecode { table, column, row_offset, declared_type, value }`. A
dump is machine-generated, so such a value means either the file is corrupt or
*our mapping is wrong*, and both deserve to surface at a byte offset. Nulling
the value would launder our bug into the caller's silent data loss; the escape
hatch is `SchemaMode::Strings`, which is explicit.

`infinity`/`-infinity` are real, reachable values — PostgreSQL's own pseudo-values
for "unbounded", not corruption — and neither `Date32` nor `Timestamp` has a
sentinel for them, so they hit this path by construction rather than by
accident. `fixture_schema_types.sql`'s `t_date`/`t_timestamp` tables carry
them specifically so this is exercised rather than assumed.

This matches Phase 1's calls for `InvalidUtf8` and `UnknownPredicateColumn`.

**Resolution outcomes are diagnostics, never errors.** A
`ResolvedSchema { schema: SchemaRef, columns: Vec<ColumnResolution>, notes:
Vec<Diagnostic> }` is retrievable from the `TableStream` before or during
consumption. Per-column outcomes carry *why*:

- `Mapped`
- `UnknownType { declared }` — declared, but this build has no mapping
- `NotDeclared` — no DDL explained this column (`--data-only`, typed table)
- `Deferred { kind: Array | Composite | Range }` — decodable, waiting on Phase 3
- `OpaqueBaseType` — a C-level base type; the dump says how the *server* parses
  it, which tells us nothing
- `EmptyEnum`

The distinction is the point: "opaque C type" and "composite, not yet decoded"
are different messages to a user deciding whether to wait for a release.
Diagnostics never affect control flow — the CLI renders them, an embedder
ignores them. Decode failures do **not** come through here.

## Predicates

Unchanged from Phase 1 apart from one addition. Typed predicates and ordering
operators (`<`, `>`) are **Phase 4**, where pushdown lands — a typed post-parse
predicate is a half-measure pushdown rewrites immediately, and it drags
collation, NULL ordering and numeric coercion into a phase whose job is the
type mapping. Phase 2 makes them possible; Phase 4 does them once.

The addition rounds out Phase 1's NULL semantics, where a NULL field matches
neither `=` nor `!=` and there was no way to ask for one:

- `PredicateOp` gains `IsNull` / `IsNotNull`; `Predicate::value` becomes
  `Option<String>`.
- CLI: `--filter 'col IS NULL'` / `--filter 'col IS NOT NULL'`, matched
  case-insensitively after the column name.
- `col=NULL` keeps meaning the literal four-character string `NULL`, which is
  what a COPY TEXT field containing `NULL` actually is (a SQL NULL is `\N`).

## CLI

`pgdq info` gains dump-level metadata:

- **Always**: a short header — source server version, `pg_dump` version,
  extension count (extension *versions* are absent from regular dumps by
  design, so they are omitted rather than guessed), user-defined type count.
  Two or three lines, and the first thing anyone wants when handed an
  unfamiliar dump.
- **Always**: column types beside the column names already listed. `info`'s job
  is "tell me what is in here", and a column listing without types is the odd
  output.
- **`--verbose` only**: per-column resolution diagnostics, alongside the file
  offsets it already shows. A per-column diagnostic for every unmapped column
  would bury a wide schema for a user who did not ask.
- **Default summary line**: `3 of 47 columns unmapped — run with --verbose for
  details`, keeping it discoverable without the wall of text.
- **`--preamble-only`** (new flag, opt-in): answer from the preamble alone —
  the dump-level header above, no per-block listing or row counts — instead
  of `info`'s usual full structural scan. Backed by
  `crate::index::scan_preamble` (already landed in Phase 2.2.1, ahead of this
  slice, for `table_stream`'s incremental cache-warming prepass —
  `docs/design/roadmap-phase2.2.1-incremental-preamble-notes.md`): bounded to
  the file's
  first `COPY` block (I1, `docs/design/postgres-invariants.md`), so cost is
  independent of dump size regardless of how many gigabytes of `COPY` data
  follow. Reuses a cache's already-known metadata when present (every
  cache-enabled `table_stream`/`pgdq query` run leaves the first database's
  preamble captured this way, whether or not the flag was ever passed) and
  falls back to running `scan_preamble` fresh otherwise — never a full
  `build_index` scan. Two buckets, two operations: this is the "preamble
  only" side of the split described in `docs/design/roadmap.md`'s
  role-discovery Future item, which is the "full scan" side (role/tablespace
  references aren't preamble-confined, so that one can't take this
  shortcut). Default stays a full scan — the block listing and row counts are
  also genuine "what is in here" answers this flag trades away.

`pgdq query` **renders typed values back to their PostgreSQL text form**, so
its output is byte-identical whether typing is on or off. The CLI's job is
showing what is in the dump, and having `--schema-mode` silently change the
bytes on stdout would make the two modes non-comparable — which is exactly what
you want to do when checking whether a mapping is right. It also keeps
`pgdq query` pipeable into anything that expects COPY TEXT.

Arrow's own display formatting is the tempting default and is wrong here: it
renders timestamps in its own format, and `Decimal128` and `FixedSizeBinary` in
forms that do not round-trip back into PostgreSQL at all.

## API shape changes

- `BatchOptions` gains `SchemaMode` (see "Output model").
- `table_stream` exposes `ResolvedSchema` as a method on `TableStream`.
- `read_table` returns `(ResolvedSchema, Option<ResumeToken>)` rather than
  `Option<ResumeToken>`. The schema is known before the first batch, so
  returning it at the end is late but never wrong, and every batch already
  carries `RecordBatch::schema()` for a caller who needs types *during* the
  callback. An out-parameter or a second diagnostics callback would burden the
  common path to serve the rare one; a caller who needs diagnostics before
  consuming should use pull mode, which is what it is for.
- `PredicateOp` gains `IsNull`/`IsNotNull`; `Predicate::value` becomes
  `Option<String>`.
- `Error` gains `FieldDecode`.

## Fixtures

The fixture schemas split by purpose, because they want opposite things:

- `scripts/fixture_schema_edge_cases.sql` (was `fixture_schema.sql`) exercises
  the **scanner** — escapes, `COPY`-like substrings in data, empty tables,
  multiple schemas. Its `insta` snapshots are stable precisely because it is
  small and hand-curated.
- `scripts/fixture_schema_types.sql` (new) exercises the **type mapping** — one
  column per mappable type, plus the boundary values that break them: `NaN`,
  `±Infinity`, `infinity`/`-infinity` timestamps, year 0001 and 9999,
  `numeric` at 38 and 39 digits, empty vs NULL `bytea`, an enum, a domain over
  a domain. Dropping this into the edge-case schema would swamp its snapshots.

Each schema gets its **own flag list**, and output nests by schema:
`fixtures/<version>/<schema>/<flag-set>.sql`.

| Schema | Flag sets |
|---|---|
| `edge_cases` | the existing seven, plus `binary-upgrade` |
| `types` | `default`, `data-only`, `binary-upgrade` |

The types schema needs only three: `--no-owner` and `--clean` cannot change a
column's declared type, while `data-only` is the no-DDL degradation path and
`binary-upgrade` is where I5's `INTEGER /* dummy */` dropped columns and I6's
relocated enum labels appear. `binary-upgrade` joins the edge-case matrix too —
those dummy columns are a scanner-visible shape with zero coverage today.
Nesting by schema costs a one-time move of the existing
`fixtures/<version>/*.sql` and keeps `generate_fixtures.py` a loop over
`(schema, flag_set)` rather than a special case.

### Generating the types fixture is a validation step, not just test data

The mapping table above is derived from the `pg_dump` source and from Arrow's
type list — it is **not** derived from observing what `pg_dump` actually emits
for most of those types, because koji contains none of them. That makes it the
weakest part of this spec.

So generating `fixture_schema_types.sql` comes **first**, before any code, and
its output is *read* before slice 2.4 commits a decoder to anything:

1. Spin up the routine-version containers `generate_fixtures.py` already
   drives.
2. Populate a table per type family, several rows each, covering the boundary
   values below.
3. `pg_dump` it out of the container into `fixtures/<version>/types/`.
4. **Read the emitted COPY TEXT** and check each mapping-table row against what
   is actually there.
5. Where prediction and reality disagree, the spec is what changes — and if the
   disagreement is about `pg_dump` behaviour rather than our reading of it, it
   earns an entry in `postgres-invariants.md`.

Boundary values worth building in, chosen because each one has a plausible way
to go wrong:

| Type | Cases |
|---|---|
| `numeric` | `NaN`; 38 vs 39 digits of precision; negative scale; trailing zeros; with and without a typmod |
| `real`, `double precision` | `NaN`, `Infinity`, `-Infinity`, `-0.0`, a subnormal, and a value that only round-trips at `extra_float_digits = 3` |
| `date` | `infinity`, `-infinity`, `0001-01-01`, `9999-12-31`, a BC date |
| `timestamp`, `timestamptz` | `infinity`/`-infinity`; 0 and 6 fractional digits; a BC timestamp; a year past 9999; a non-UTC offset if one can be provoked |
| `time`, `timetz` | `24:00:00`; microsecond precision |
| `interval` | several, to see which `IntervalStyle` the container defaults to |
| `bytea` | empty, NULL, high bytes, an embedded backslash |
| `uuid` | all-zeros; mixed-case input (to confirm it renders lowercase) |
| `text`, `varchar`, `char(n)` | empty string vs NULL; `char(n)` padding |
| enum | labels containing a space, a quote, and a comma |
| domain | a domain over a domain; `NOT NULL` |
| array | `{}` vs `{NULL}` vs a NULL array; elements containing `,` `{` `}` `"` `\`; a multidimensional array |
| composite, range | nested quoting; an empty range; an unbounded end |

The array row matters most: `{}`, `{NULL}`, and a NULL array are three distinct
values that all look similar, and getting them confused is the classic way an
array decoder goes wrong. They are Phase 3's problem, but the fixture should
capture them now while the schema is being written.

koji is not evidence for a third of the mapping table — it contains **no**
`numeric`, `date`, `uuid`, `bytea`, or `interval` columns at all, and no column
using a user-defined type — so `fixture_schema_types.sql` is the only coverage
those rows will have.

## Benchmarks

Phase 2 wires up the `criterion` benchmarks specified since Phase 1 and never
built. It is the first phase to add real per-byte CPU work — every typed field
goes through a parse Phase 1 skipped entirely — and the koji baseline (243 MB/s
at ~33% of one core) **cannot detect a typed-decode regression at all**,
because it is I/O-bound on an HDD. The cost only becomes visible on fast media
or a warm page cache, which is exactly the trap
`docs/design/roadmap-phase6-scan-performance.md` names under "Measurement
discipline".

Scope is narrow on purpose — a regression tripwire for Phase 2's own work, not
the start of Phase 6's optimization campaign:

- A decoder microbenchmark per mapped type family.
- One warm-cache whole-file run.

### Synthetic performance dataset

Real measurement on fast media needs a file large enough to be interesting and
small enough to live on one, which koji is not. `scripts/generate_perf_data.py`
(new) generates one: synthetic `Lorem Ipsum`-style content, parameterized by
size, written wherever the caller points it.

- **Generated, never committed.** The generator is in the repo; its output is
  not. Runs go on the SSD (`/mnt/ssd/fedora/...`) or the root NVMe volume —
  see `CLAUDE.local.md` for what each volume is.
- **Not reproducible, and that's fine.** This measures throughput, not
  correctness, so two different random datasets of the same shape are
  interchangeable. Correctness lives in `fixtures/`, which *is* committed.
- **Deliberately includes stress sections** targeting the code paths whose cost
  is expected to move: high-escape-density fields, very long text values,
  wide rows, and — for Phase 3 — large runs of array-valued columns, so array
  decoding has something that surfaces a regression rather than hiding it in
  the average.

Phase 2 uses a size that fits page cache; later phases turn the dial up (~100 GB
is the target for Phase 6's device-bound runs, which is also where the
long-running-process rules in `CLAUDE.md` start applying). The generator is
shared, the size and stress mix are per-phase.

## Implementation slices

Phase 2 is substantially larger than any Phase 1 increment, so it lands as
five numbered subphases, plus 2.2.1 — a small follow-up patch to 2.2's own
contract, numbered because it changed `table_stream`'s behavior and needed
its own record, not because it was planned as a sixth slice. The ordering is
deliberate: **each slice makes the next one's mistakes visible.**

| Slice | Content |
|---|---|
| **2.0** | Generate the types fixture and validate the mapping table against it — no code, see "Fixtures" |
| **2.1** | Dollar-quote tracking in the scanner |
| **2.2** | Preamble parsing, `DumpMetadata`, cache persistence, `pgdq info` display |
| **2.2.1** | Incremental (`table_stream`) scans guarantee the first database's preamble is captured too, not just `build_index`'s full scan — see `docs/design/roadmap-phase2.2.1-incremental-preamble-notes.md` |
| **2.3** | Type resolution, `ResolvedSchema`, diagnostics — still emitting `Utf8View` for every column; also `pgdq info --preamble-only`, a fast path skipping the full structural scan (see "CLI") |
| **2.4** | Decoders + render-back + round-trip tests, one type family at a time |
| **2.5** | Benchmarks and the synthetic performance dataset |

2.0 comes first because it needs no code and de-risks the least-evidenced part
of this spec. 2.1 closes the Phase 1 known gap, ships value with zero new API,
and protects the preamble everything downstream reads. 2.2 makes `pgdq info` immediately
better and proves the metadata pass on its own, with no typing involved. 2.2.1
closes a gap 2.2 left in that proof: a cache file's presence didn't imply any
metadata unless it came from a full scan, which every incremental `pgdq
query` run is not. 2.3 is the important one: the resolved schema and its
diagnostics become inspectable for koji and every fixture *before* a single
decoder exists — which is when the mapping table is cheapest to argue about.
Only then does 2.4 start narrowing column types. 2.3 also gives `pgdq info`
its `--preamble-only` fast path: `crate::index::scan_preamble` (2.2.1)
already proves a preamble read never needs the full structural scan (it's
what `table_stream` uses internally), so exposing that shortcut at the CLI is
wiring, not new design.

### Where each slice's notes live

Each slice accumulates its own implementation notes as it lands, in
`docs/design/roadmap-phase2.<N>-<slug>-notes.md` (or `2.<N>.<K>` for a small
follow-up patch to an already-landed slice, like 2.2.1). At the end of Phase 2
these are consolidated into a single
`docs/design/roadmap-phase2-typed-columns-notes.md` — matching the Phase 1
`-notes` convention — and the per-slice files are removed. The split exists
so that a slice's detail has somewhere to go while it is fresh, without
waiting on the whole phase to finish.

## Module layout

Four new modules, flat, matching the four concerns:

| Module | Concern |
|---|---|
| `preamble.rs` | TOC segmentation and the `CREATE TABLE`/`TYPE`/`DOMAIN`/`EXTENSION` grammars, producing `DumpMetadata` |
| `pgtype.rs` | Declared-type string → Arrow `DataType`; domain/enum resolution; the `.`-qualified split (I8) |
| `decode.rs` | Per-type field decoders **and** their render-back counterparts |
| `resolve.rs` | `ResolvedSchema`, `ColumnResolution`, `Diagnostic` — the join of a `COPY` header against `DumpMetadata` |

Keeping decode and render-back in one module is the non-obvious call: they are
inverse functions, and separating them invites them to drift apart in exactly
the way the round-trip test below exists to catch.

## Testing the mapping's correctness

Throughput is the benchmarks' job; correctness is this one's. A test asserting
`id` came back as `Int32` proves the plumbing, not the parse.

**The primary test is a round-trip through the render-back path**: for every row
of every fixture, decode the COPY TEXT field to a typed value, render it back to
PostgreSQL text, and assert it equals the original bytes. This is self-checking
in the way `public.escapes` already is — no expected values are written by hand,
so a test cannot encode the same misreading twice — and it catches the failures
that matter: a timestamp parsed to the wrong instant, a decimal losing a digit,
a float not round-tripping at `extra_float_digits = 3`.

It also means the `pgdq query` render-back decision earns its keep twice: the
CLI's output format and the test oracle are the same code path.

Hand-written assertions remain only for boundary values a round-trip cannot
reach on its own — `NaN`, `±Infinity`, `infinity`/`-infinity` timestamps, year
0001 and 9999, `numeric` at 38 and 39 digits — which is a short list rather than
a whole table.

## Test strategy for existing coverage

**Phase 1's existing tests pin to `SchemaMode::Strings`;** new typed tests are
written alongside them. Those tests exist to prove *scanner and batch-assembly*
properties — chunk-size independence, straddling fields, resume-without-gap,
batch splitting — none of which is about types. Retyping their expectations
would couple a scanner regression test to the type mapping, so a mapping change
would start breaking tests that have nothing to say about it. It also keeps
`SchemaMode::Strings` under continuous real coverage instead of being an escape
hatch nobody exercises.

The two `public.escapes` round-trip tests are the exception and run in **both**
modes: `text` maps to `Utf8View` either way, so the results must be identical —
which is exactly the assertion worth making.

**Dollar-quote tracking (2.1) gets its coverage from the fixture schema**, which
today contains no functions at all. Two go in:

- A function whose dollar-quoted body contains, at column 0, a syntactically
  perfect `COPY public.widgets (id, name) FROM stdin;` followed by more lines
  and a `\.`. This is the exact adversarial shape, and it makes the fixture
  self-demonstrating: an unfixed scanner mis-parses it and swallows every
  following table, so the test fails loudly rather than subtly.
- A function using a *tagged* delimiter (`$func$`) with an inner `$$` that must
  not terminate it — tag matching is where a naive implementation breaks.

Both go through the generator, so all three PG versions confirm `pg_dump` emits
bodies verbatim. The existing `insta` event-stream snapshot is the regression
guard: it shows the correct block sequence.

**The cache `format_version` is not bumped.** Dollar-quote tracking changes what
a scan *finds*, which would normally warrant it — a pre-fix cache deserializes
cleanly and returns wrong block boundaries, which is a *wrong* cache rather than
a stale one, and rescanning from the watermark cannot repair it. It is moot
here: there are no users and no `.dqcache` files in existence (verified). Apply
the reasoning, not the precedent, to any later slice that changes what a scan
discovers; 2.2 adding metadata does **not** qualify, since a cache without
metadata is legitimately just an older, less complete cache.
