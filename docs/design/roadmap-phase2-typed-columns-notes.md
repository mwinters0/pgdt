# Phase 2 (typed columns) — Implementation Notes

Phase 2 is complete: every functional item in
[`roadmap-phase2-typed-columns.md`](roadmap-phase2-typed-columns.md) is
implemented. That doc remains the specification — *what* typed columns do and
why. This one records *how* it landed: where each piece lives, and the
implementation-level facts later phases inherit. Not a changelog and not a
status doc — for what is and isn't built now, see
[`../status/STATUS.md`](../status/STATUS.md).

## Module map

| Concern | Where |
|---|---|
| DDL preamble parsing → `DumpMetadata` (`CREATE TABLE`/`TYPE`/`DOMAIN`/`EXTENSION`, `\connect` segmentation) | `pgdump_query/src/preamble.rs` (L1) |
| Declared-type string → Arrow `DataType`; domain/enum/range/multirange resolution | `pgdump_query/src/pgtype.rs` (L2) |
| `ResolvedSchema`/`ColumnResolution`/`Diagnostic` — joins a `COPY` header against `DumpMetadata` | `pgdump_query/src/resolve.rs` (L2) |
| Per-type field decode + render-back | `pgdump_query/src/decode.rs` (L2) |
| COPY TEXT field unescaping (`decode_field`, existing) and escaping (`encode_field`, new) | `pgdump_query/src/copy.rs` (L1, extended) |
| Typed Arrow batch assembly (`ColumnBuilder` per `DataType`), per-`CopyBlock` database field | `pgdump_query/src/batch.rs`, `index.rs` (L3/L1, extended) |
| Per-block database attribution during live scans, one-target-per-query enforcement | `pgdump_query/src/stream.rs` (L4, extended) |
| CLI: `--preamble-only`, `--schema-mode`, `--database`, `IS [NOT] NULL` filters | `pgdump_query-cli/src/main.rs` (extended) |
| Decoder + whole-file regression benchmarks | `pgdump_query/benches/{decoders,whole_file}.rs` |
| Synthetic throughput dataset generator | `scripts/generate_perf_data.py` |

Each module's layer assignment constrains what it may depend on and know:
[`layering.md`](layering.md). `preamble.rs`, `pgtype.rs`, `resolve.rs`, and
`decode.rs` didn't exist before this phase; the L1/L2 split between `copy.rs`
(byte-level COPY escaping, both directions) and `decode.rs` (decoded text ↔
Arrow value, never touching escaped bytes) is this phase's own addition to
that boundary, not inherited from Phase 1.

Integration tests mirror the module split: `tests/preamble.rs`, `tests/pgtype.rs`
(both new), plus typed-mode coverage added to the existing `tests/{batch,stream,scan}.rs`
and a new `tests/decode.rs`.

## Fixture tree

`fixtures/<version>/<schema>/<flag-set>.sql`, two schemas: `edge_cases`
(scanner/preamble robustness shapes) and `types` (one table per Arrow-mappable
PostgreSQL type family, including the boundary values in the phase doc's
"Boundary values worth building in" table). Uniform across all six routine
versions (13–18) and every flag set each schema defines — including `create`
(the only flag that makes `pg_dump` emit `\connect`, needed for multi-database
coverage), `no-comments`, and `dumpall` (a real `pg_dumpall` run, not a
`pg_dump` flag set — `generate_fixtures.py`'s `SCHEMAS` dict uses `None` as
the sentinel meaning "swap the binary, don't append flags"). "Absent" in this
tree always means "deliberately absent," never "not yet generated."

Multi-database test coverage does not need a second real fixture beyond
`create.sql`: `tests/{preamble,pgtype,stream}.rs` each build a two-database
shape at test time by reading `create.sql`, writing a byte-substituted copy
with the database name changed, and concatenating — the parser only cares
about the `\connect`-delimited shape, not which process produced it. A real
`pg_dumpall` fixture (`dumpall.sql`) exists separately because `pg_dumpall`
is *not* just several `pg_dump --create` outputs concatenated: it passes
`--create` for ordinary databases but writes `template1`/`postgres`'s
`\connect` itself, ahead of their version headers rather than after (I9) —
a shape the hand-built concatenation cannot produce and that `PreambleBuilder`
has to handle regardless.

`scripts/generate_fixtures.py [--version N] [--schema types|edge_cases]`
regenerates; both schemas across all six versions is one run
(~2 min, throwaway 512MB containers, never a host-run Postgres).

## Preamble parsing (`preamble.rs`)

`DumpMetadata` / `DatabaseMetadata` / `Extension` / `TypeDef` / `TypeKind`
recover server/`pg_dump` versions, extensions, user-defined types, and each
column's declared type **as the literal string `pg_dump` wrote** — never a
parsed pair — per the layering rule that L1 stores what the dump said, not
what a later layer concludes. `crate::scan::Event::Line` feeds every
outside-block line to a `PreambleBuilder`, incrementally.

**Grammar approach**: dispatch directly off five fixed line-start keywords
(`CREATE TABLE`, `CREATE TYPE`, `CREATE DOMAIN`, `CREATE EXTENSION`, `ALTER
TYPE`) rather than modeling `pg_dump`'s `-- Name: ...; Type: ...` TOC-comment
grammar as a separate segmentation pass. An unrecognized line — a TOC
comment, a `SELECT pg_catalog.binary_upgrade_*` call under `--binary-upgrade`
(I6), `CREATE FUNCTION`, `ALTER ... OWNER TO`, blank lines — is simply never
matched to a trigger keyword and ignored, which gives the same soundness
property ("never guess, only act on a fully-recognized shape") the
TOC-comment design was after. Once a trigger line is seen, subsequent lines
are appended (rejoined with `\n`) until parens balance (respecting
`'...'`/`''`-escaped literals) and the statement ends in `;` — sufficient
because `pg_dump` never puts a semicolon inside a nested expression for any
of these five shapes. A declared type string is recovered by
whitespace-tokenizing the tail after a column/field name and stopping at the
first column-constraint keyword (`NOT`, `DEFAULT`, `COLLATE`, `GENERATED`,
`PRIMARY`, `REFERENCES`, `CHECK`, `UNIQUE`, `CONSTRAINT`), since `pg_dump`
never puts a space inside a type's own parenthesized modifier
(`numeric(38,10)`); a `--binary-upgrade` dummy column's type comment
(`INTEGER /* dummy */`, I5) is stripped first.

**Multi-database (`\connect`) segmentation.** A `--create` dump's
pre-`\connect` segment is never a real database entry (`name: None` is
reserved for a genuine non-`--create`, no-`\connect` dump) — but its version
headers, which print once ahead of the first `\connect`, are carried forward
onto the database that first `\connect` switches into (I9). Every subsequent
database segment (a real multi-database `pg_dumpall`-style file) carries its
own independent version-header pair, staged via a `pending_headers` field
whenever they arrive while `feed_line`'s early-return gate
(`current.preamble_complete`) would otherwise have dropped them, and
consumed by `on_connect` on *every* transition, not just the first.

**Bounded preamble-only reads.** `crate::index::scan_preamble` scans from
byte 0 to the first `COPY` block header (or EOF), which — per I1 — always
closes out the *first* database's preamble regardless of how many
`\connect`-ed databases precede it, so its cost is independent of dump size.
`table_stream` runs it once, up front, whenever caching is enabled and the
first database's `preamble_complete` isn't already known, persisting
immediately (not deferred to a later segment's save) so even a caller that
polls once and drops the stream leaves a cache with that metadata. Skipped
under `CacheMode::Disabled` (pure streaming, no side effects). This is also
what makes `pgdq info --preamble-only` cheap: a bounded scan, not a full
structural one. A **later** `\connect`-ed database's preamble is populated
only by `build_index`'s full scan — there is no incremental equivalent for
anything past the first database.

**Per-`CopyBlock` database attribution.** `CopyBlock` carries
`database: Option<String>`, populated from `PreambleBuilder`'s current
database at `CopyEnd` (safe there since a `\connect` cannot occur mid-block).
`table_stream`'s live scan tracks this independently via
`preamble::parse_connect` (applied to every `Event::Line` it already
receives) rather than running a full `PreambleBuilder` — yielding a database
*name* only, never a column type, so the resolved schema still cannot change
mid-stream. Seeding this tracker for a fresh, non-resumed scan needs two
cases: if cached blocks already exist, the one with the greatest
`end_offset` names the database in scope at the watermark; if none do (a
cold start, or `CacheMode::Disabled`), the watermark sits exactly at the
first `COPY` block (the preamble prepass's own boundary), so the truth is
`metadata.databases`'s *first* entry, not `None` (I1: nothing precedes any
database's first block). `ResumeToken`/`InCopyResume` both carry their own
`database: Option<String>` so a resumed stream doesn't lose this state.

## Type resolution (`pgtype.rs`, `resolve.rs`)

`resolve_declared_type(declared, types) -> TypeOutcome` maps the built-in
half of the type-mapping table keyed on the base type name (typmod split off
first — only `numeric` inspects it, every other mapping being
`Microsecond`-precision or otherwise typmod-independent), and the
user-defined half (`.`-qualified per I8) against the querying database's own
`CREATE TYPE`/`CREATE DOMAIN` list, recursing through domains with no cycle
guard needed (PostgreSQL cannot create a domain over a type that doesn't
exist yet). `TypeOutcome` is `Mapped(DataType)`, `Unknown`,
`Deferred(DeferredKind::{Array,Composite,Range})`, `OpaqueBaseType`, or
`EmptyEnum`.

PostgreSQL's six built-in range types (`int4range` … `daterange`) and six
built-in multirange types (`int4multirange` … `datemultirange`) are
special-cased as bare names in `map_builtin`, since — unlike composites —
they appear unqualified and never reach the `.`-qualified user-type path at
all. A range's auto-created multirange companion is never itself the subject
of a `CREATE TYPE` (I10); resolving its name falls back to scanning `types`
for the `Range` whose `TypeKind::Range::multirange_type_name` matches.
`ColumnResolution::Deferred`'s `kind` does not distinguish a built-in range
from a user-defined one — nothing downstream needs the distinction yet;
Phase 4's range decoder will need the *subtype*, held in
`TypeDef::Range::subtype` for a user-defined range and absent entirely for a
built-in one (PostgreSQL encodes it in the catalog, not in DDL text), so this
is left for whichever slice of Phase 4 builds that decoder.

`resolve_columns(qualified_table, columns, metadata, mode, database) ->
ResolvedSchema` joins a `COPY` header's column list against `DumpMetadata` by
name, requiring the caller's `database` — resolved from the matched block's
own attribution, never guessed — to pick the right `DatabaseMetadata`.
`ResolvedSchema { schema, columns, notes }`: `columns` is positional
(parallel to `schema.fields()`); `notes` carries one `Diagnostic` per column
(mapped and unmapped alike, with the raw declared-type string attached),
since `pgdq info`'s display needs both. `SchemaMode::Strings` (the default's
alternative) never looks anything up — every column is `NotDeclared`/
`Utf8View`, byte-for-byte Phase 1 behavior, at zero lookup cost. `resolve.rs`
checks, in `SchemaMode::Typed` only, that the block's attributed database is
both present in `metadata.databases` and `preamble_complete`; otherwise it
raises `Error::MetadataNotScanned { database }` — reachable only through
`table_stream`'s incremental path, since a full `build_index`/`pgdq parse`
scan always leaves every database it found `preamble_complete`. Its own
remedy is real: `pgdq query --schema-mode strings` bypasses the check
entirely, same as `--schema-mode typed` (default) requests it.

## One target per query

A `COPY` header match by bare or qualified name alone is not enough once a
resolved schema and real row data both depend on *which* database and table
a match came from — the same collision can happen across `\connect`ed
databases or, via `CopyHeader::matches`'s schema-agnostic bare-name rule,
within one database across schemas. `table_stream` narrows every match to at
most one `(database, qualified_name)` candidate: for known (cached) blocks,
filtered by `BatchOptions::database` first if given, then checked for a
second distinct candidate before any streaming starts; for live discovery,
the same check runs incrementally as each new matching block is found, since
a cold scan cannot know the full candidate set until it reaches one. A second,
differing candidate raises `Error::AmbiguousTable { name, candidates }`
immediately — but a cold, uncached query can still have already streamed
some of the first candidate's rows to the caller before the live scan
discovers the conflict (those rows are genuinely correct, never a union; the
caller must still treat any output preceding a stream error as incomplete,
same as any other mid-stream error). A warm cache, or a query run after
`pgdq parse`, catches the ambiguity before any streaming starts. `--database`
(CLI) / `BatchOptions::database` (library) resolves the ambiguity by name,
matched against `DatabaseMetadata::name`.

`pgdq info` groups its block listing by `database: <name>` whenever more
than one is present (silent for the common single-database case) — this is
what makes an `Error::AmbiguousTable`'s candidate names actionable, since
they're names that listing already showed.

## Decoders and render-back (`decode.rs`)

One `decode_*`/`render_*` pair per Arrow `DataType`
`resolve_declared_type` can produce: `bool`, `Int16/32/64` (via `str::parse`
directly in `batch.rs`, no dedicated function), `Float32/64`, `numeric`/
`decimal` (`decimal_unscaled_digits`/`render_decimal`, producing/consuming an
unscaled integer digit string at a fixed scale — `i128::from_str`/
`i256::from_str` accept it directly), `date`, `time`, `timestamp`/
`timestamptz`, `uuid`, `bytea`. Every `decode_*` takes the already-COPY-
unescaped `&str` `copy::decode_field` returns and yields a plain Rust value;
every `render_*` is its exact inverse, producing that same decoded-text
shape — never an Arrow builder (that's `batch.rs`, L3) and never raw
on-disk COPY-escaped bytes (that's `copy.rs`, L1). `ColumnBuilder` (`batch.rs`)
covers every producible `DataType`, including `Dictionary(Int32, Utf8)` for
enum (an unparsed dictionary-key append — enum text is never itself
decoded/rendered) and `Utf8View` (the unchanged zero-copy path). Every
non-`Utf8View` builder arm copies unconditionally, since a decoded value
(an `i32`, a `[u8; 16]`, an unscaled `i128`/`i256`, …) has its own
representation rather than a byte range of the original field — the
zero-copy machinery `roadmap-phase7-scan-performance.md` cares about stays
scoped to exactly `Utf8View`. `RowBatcher` carries the qualified table name
and each column's declared-type string (from `ResolvedSchema::notes`), which
is everything `Error::FieldDecode` needs beyond what it already had.

**Escaping (`copy.rs`) and decoding (`decode.rs`) are separate concerns in
separate layers.** `copy::encode_field` is `decode_field`'s exact inverse —
re-applying COPY TEXT escaping to already-unescaped text — implementing only
the seven escapes `pg_dump`'s `COPY TO` ever actually emits: a doubled
backslash and the six control-character mnemonics `\b \f \n \r \t \v` (I15).
The octal/hex forms `decode_field` accepts are a `COPY FROM` reader
convenience only; `COPY TO` never produces them, so `encode_field` doesn't
need to reproduce them. It is deliberately not a general COPY-text encoder —
nothing in this codebase needs one, since every correctness fixture is still
produced by real `pg_dump`, never hand-encoded; the one place it's needed is
proving `decode_field`/`encode_field` round-trip against real on-disk bytes
(see "Testing" below).

Non-obvious calendar/formatting facts, pinned as tests rather than left for a
future reader to re-derive:

- Date/timestamp decode and render use Howard Hinnant's public-domain
  `days_from_civil`/`civil_from_days` (proleptic Gregorian, correct for
  negative/BCE years by construction) rather than a calendar crate. BCE
  values convert through PostgreSQL's astronomical-year convention
  (`1 - year` for a `" BC"`-suffixed value) before the day-count math, so
  `0044-01-01 BC` and `9999-12-31`/`0001-01-01` share one code path with no
  BCE special case in the arithmetic itself.
- `timestamp with time zone` decode subtracts whatever UTC offset the text
  carries (general, not hardcoded `+00`), even though I4 guarantees every
  real `pg_dump` output normalizes to `+00` — costs nothing extra and covers
  a future PostgreSQL version or unusual session `TimeZone`.
- PostgreSQL's float formatting switches to scientific notation on a fixed
  exponent threshold (`FLT_DIG`/`DBL_DIG`, 6/15), not by significant-digit
  count — I14, source-confirmed against `src/common/f2s.c`/`d2s.c`.
- PostgreSQL's own documented maximum timestamp (`294276-12-31
  23:59:59.999999`, relative to its 2000-01-01 epoch) genuinely overflows
  this build's `Timestamp(Microsecond)` mapping (`i64` micros since the
  1970-01-01 Unix epoch, 30 years earlier) — a real, expected
  `Error::FieldDecode`, not a bug.
- `numeric` with no typmod stays `Utf8View` by design — arbitrary precision
  has no Arrow decimal representation.

See `docs/manual/type-handling.md` for the user-facing statement of what
recovers exactly, what stays a string, and what a mismatch produces — this
doc covers only the facts a later phase needs, not the user-facing summary.

## Testing philosophy

**The primary correctness tool is a round trip with no hand-transcribed
expected values**, since (per the phase doc) "a test cannot encode the same
misreading twice." Two independent round trips, each owned by the layer it
tests:

- `tests/decode.rs`: queries the same real `pg_dump` fixture in both
  `SchemaMode::Typed` and `SchemaMode::Strings` and asserts every field
  renders identically. `Strings` mode is Phase 1's untouched, byte-for-byte
  behavior, already covered by its own test suite, so it's a trustworthy
  oracle for "did `decode_*`/`render_*` stay exact inverses" — entirely in
  decoded-text terms, never touching on-disk escaped bytes.
- `tests/scan.rs`'s `copy_text_escaping_round_trips_through_postgres`:
  `encode_field(decode_field(raw))` must equal the literal on-disk bytes of
  every field of `public.escapes` (one row per codepoint), across all three
  routine `pg_dump` versions — the escaping/unescaping leg `decode.rs`'s
  round trip deliberately never exercises.

Boundary values a fixture round trip cannot reach on its own (`NaN`,
`±Infinity`, `infinity`/`-infinity` dates/timestamps, year 0001/9999,
38-vs-39-digit numeric, negative-scale numeric) are hand-written unit tests
in `decode.rs` instead, pinned from the type's documented semantics where no
fixture column happens to exercise them (negative-scale numeric has none).
`t_numeric`/`t_date`/`t_timestamp`'s boundary rows (`NaN`, `infinity`) are
genuine `FieldDecode`s by design, so they get their own tests asserting the
error names the right table/column/declared-type/value, plus a check that
`SchemaMode::Strings` still shows the raw text with no error.

**Existing Phase 1 tests stay pinned to `SchemaMode::Strings`** (they prove
scanner/batch-assembly properties unrelated to types); new typed coverage is
added alongside rather than by retyping their expectations. The two
`public.escapes` tests are the deliberate exception, run in **both** modes
since `text` maps to `Utf8View` either way and the results must match. Every
column-rendering test helper (`rows_of` in `tests/{batch,stream,query_cache}.rs`)
goes through the public `render_field` rather than a hardcoded
`StringViewArray` downcast, since `SchemaMode::Typed` now genuinely produces
non-`Utf8View` columns.

## Benchmarks and the synthetic performance dataset

`criterion` (`harness = false`, `[[bench]]` in `pgdump_query/Cargo.toml`) is
scoped as a regression tripwire for this phase's own new per-byte CPU cost,
not the start of Phase 7's optimization campaign
(`roadmap-phase7-scan-performance.md`, "Measurement discipline"):

- `benches/decoders.rs` — one `decode`/`render` benchmark pair per mapped
  type family (`bool`, `int`, `float`, `numeric`, `date`, `time`,
  `timestamp`, `timestamptz`, `uuid`, `bytea`). `text`/`varchar`/`char`
  (zero-copy, no decode step) and `enum` (unparsed dictionary-key append,
  also no decode step) have nothing to benchmark.
- `benches/whole_file.rs` — one warm-cache end-to-end `SchemaMode::Typed`
  scan, since `Strings` mode wouldn't exercise `decode.rs` at all.

`scripts/generate_perf_data.py` generates the whole-file benchmark's input
directly (never via real `pg_dump`, unlike every correctness fixture): a
single wide table, one column per family `decode.rs` covers, plus
`v_long_text` (very long values) and `v_escaped` (high escape density) —
the stress shapes the phase doc calls for. Parameterized by `--size-mb`
(default 256, sized to fit page cache; Phase 7 turns this to koji-scale,
~100 GB, on the SSD or root NVMe volume). Its escaping matches `encode_field`
(I15), reimplemented in Python since the script has no Rust runtime to call
into. Generated into the gitignored `runs/`, never committed — two runs
producing different bytes is fine, since this measures throughput, not
correctness, which stays entirely fixture-based.

## Evidence carried forward for later phases

- **Array dimensionality isn't in the catalog.** `integer[][]` in DDL comes
  back from `pg_dump` as plain `integer[]`, identical to a one-dimensional
  column — PostgreSQL arrays carry no fixed dimensionality in the type
  system. Phase 4's array decoder will need to infer nesting from the
  literal's own brace structure (`{{1,2},{3,4}}`), not the declared type.
- **`jsonb` reformats whitespace on storage** (`{"a":1}` in, `{"a": 1}` out)
  while `json` preserves the source text verbatim — doesn't affect the
  mapping (both stay `Utf8View`) or round-trip testing (which compares
  against what the dump emits, not the original `INSERT`).
- **`--binary-upgrade` interleaves OID-preservation noise**
  (`SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid(...)`-style calls,
  one to three) between a TOC comment and the statement it announces, for
  every object type — not just the enum-label case I6 documents. The
  preamble parser tolerates this because none of the noise lines start with
  one of its five trigger keywords, so they're never absorbed into a
  pending statement; nothing needed to change to handle it.
- **`CREATE TYPE ... AS RANGE` and base/shell-type grammars** are now backed
  by real fixture evidence (`public.myrange`, `public.mybase`,
  `public.shellonly` in `fixture_schema_types.sql`; I10, I11), not just
  source reading.
