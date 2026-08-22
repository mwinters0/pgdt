# Phase 2.3 notes — type resolution, `ResolvedSchema`, diagnostics

## What landed

- **`pgtype.rs`** (new, L2): `resolve_declared_type(declared: &str, types:
  &[TypeDef]) -> TypeOutcome` — the built-in half of "Type mapping"'s table
  keyed on the base type name (typmod split off first; only `numeric` cares
  about its contents, since every other typed mapping is
  `Microsecond`-precision or otherwise typmod-independent by design), plus
  the user-defined half (`.`-qualified — I8) resolved against the querying
  database's own `CREATE TYPE`/`CREATE DOMAIN` list. A domain recurses on its
  base type with no cycle guard needed: PostgreSQL cannot create a domain
  over a type that does not exist yet. `TypeOutcome` is `Mapped(DataType)`,
  `Unknown`, `Deferred(DeferredKind)` (`Array`/`Composite`/`Range`),
  `OpaqueBaseType`, or `EmptyEnum`.
- **`resolve.rs`** (new, L2): `resolve_columns(qualified_table, columns,
  metadata, mode) -> ResolvedSchema`. `ResolvedSchema { schema: SchemaRef,
  columns: Vec<ColumnResolution>, notes: Vec<Diagnostic> }` joins a `COPY`
  header's column list against `DumpMetadata` by name — `columns` is
  positional (parallel to `schema.fields()`); `notes` carries the same
  outcomes named, with the raw declared-type string attached, one entry per
  column (not just the unmapped ones), since `pgdq info`'s display needs both
  the mapped and unmapped columns' declared strings in one place.
  `SchemaMode::Strings` never looks anything up — every column comes back
  `NotDeclared`/`Utf8View`, matching Phase 1 byte-for-byte at zero cost.
- **`BatchOptions` gains `schema_mode: SchemaMode`** (default `Typed`).
  `TableStream::resolved_schema()` and `read_table`'s new `(ResolvedSchema,
  Option<ResumeToken>)` return expose the result. **The actual
  `RecordBatch`es stay all-`Utf8View` regardless** — `RowBatcher` only ever
  builds `StringViewArray`s, and `RecordBatch::try_new` would reject a
  schema/array type mismatch if `schema_for`'s output changed without a real
  decoder to back it (Phase 2.4). So `ResolvedSchema` is a *preview*: the
  target typing this build already understands, computed from the same
  column names `schema_for` derives, entirely decoupled from what actually
  gets built into a batch this slice. `resolve.rs`'s and `stream.rs`'s module
  docs both state this explicitly, since it is the one property of this slice
  most likely to surprise a reader — a caller can see `Int32` in
  `resolved_schema()` and `Utf8View` on the very `RecordBatch` it came with,
  and both are correct.
- **`--cache-path none` disables persistence, not typing**, closing a gap
  the phase doc's own spec named: `table_stream`'s Phase 2.2.1 preamble
  prepass previously skipped entirely under `CacheMode::Disabled` ("pure
  streaming with no side effects"). It now always runs when the first
  database's preamble isn't already known; `cache.save` staying a no-op for
  `Disabled` is what keeps persistence itself opted out. `base_index.metadata`
  is cloned into a local before `base_index` moves into `Recorder` (the move
  is unconditional at that call site regardless of `cache`'s value, since a
  closure that constructs `Recorder { ..., index: base_index }` captures
  `base_index` by move at closure-*construction* time, not at
  `Option::then`'s call time) — every call site that resolves a matching
  block's schema needs that clone.
- **`pgdq info` / `pgtype.rs`'s "the CLI's own database-selection shortcut"**
  is now `resolve.rs::database_for`: first database in `metadata.databases`
  order whose DDL mentions the table, same simplification `pgdq info` used
  before this module existed (see "Decisions worth a second look" below),
  just centralized instead of duplicated between the CLI and (now) the query
  path.
- **CLI**: `pgdq info` prints the dump-level header (unchanged from 2.2)
  plus, per block, each column's declared type string (unchanged display,
  now sourced from `resolve_columns` instead of the old ad hoc
  `declared_type` helper) and, with `--verbose`, one line per non-`Mapped`
  column naming *why* (`resolution_label`). A trailing `"N of M columns
  unmapped — run with --verbose for details"` summary line appears whenever
  `N > 0`. `--preamble-only` (new flag) answers from
  `crate::index::preamble_only` — the first database's metadata alone,
  reusing a cache's already-known copy or running `scan_preamble` fresh and
  persisting it, never a full structural scan — bounded to the first `COPY`
  block regardless of dump size (I1).
- **`Predicate` gains `IsNull`/`IsNotNull`**; `Predicate::value` becomes
  `Option<String>` (`None` for the two null-checking ops, always `Some` for
  `Eq`/`Ne`). CLI: `--filter 'col IS NULL'` / `'col IS NOT NULL'`, matched
  case-insensitively as a suffix after trimming, checked before the `=`/`!=`
  split.
- **`Error::FieldDecode`** added to the enum per the phase doc's API-shape
  list, unconstructed until Phase 2.4 builds the decoders that would raise
  it.
- **Built-in range types recognized alongside user-defined ones.** The phase
  doc's mapping table lists `array, composite, range` as the deferred trio
  without distinguishing built-in from user-defined — but unlike composites,
  PostgreSQL ships six built-in range types (`int4range`, `int8range`,
  `numrange`, `tsrange`, `tstzrange`, `daterange`) that appear bare, never
  schema-qualified, so they don't reach `resolve_user_type` through the `.`
  discriminator at all. `fixtures/*/types/default.sql`'s
  `t_range.v_range int4range` column would otherwise silently resolve
  `Unknown` instead of `Deferred(Range)`. `map_builtin` special-cases the six
  names directly; see "Decisions worth a second look" for why this shape
  covers the case without a mapping-table edit.

## Testing

- `pgtype.rs` unit tests cover every mapping-table row from hand-built
  `TypeDef`s, including the numeric precision boundary (38/39/76/77 digits),
  negative scale, case-insensitive matching (the `--binary-upgrade` dummy
  column's literal uppercase `INTEGER`), transitive domain resolution, and
  the six built-in range names.
- `resolve.rs` unit tests cover `Mapped`/`NotDeclared`/`Unknown`/
  `Deferred`/`OpaqueBaseType`/`EmptyEnum` outcomes, `SchemaMode::Strings`,
  no-metadata-at-all, and that `Diagnostic::declared` is `None` exactly when
  no DDL explained the column.
- **`tests/pgtype.rs`** (new, integration-level): resolves every column of
  every `fixture_schema_types.sql` table against the real preamble `pg_dump`
  16.14 emitted, across all three routine PG majors — the same evidence
  `tests/preamble.rs` already pins the DDL grammar against, now extended
  through the type-mapping layer. This is what caught the built-in-range
  gap: `t_range`'s `int4range` column resolved `UnknownType` until
  `map_builtin` was extended, exactly the kind of mapping-table gap 2.0's
  "read what `pg_dump` actually emits" step exists to surface before 2.4
  commits a decoder to anything.
- **`tests/stream.rs`** gained three tests: `resolved_schema` matches the
  DDL while the real batch stays `Utf8View`; `SchemaMode::Strings` never
  resolves; and `CacheMode::Disabled` still resolves types (closing the gap
  above).
- `tests/batch.rs`/`tests/predicate.rs` gained `IsNull`/`IsNotNull` coverage
  alongside the existing `Eq`/`Ne`/null-exclusion tests.

## Decisions worth a second look

- **Multi-database disambiguation is still first-match, not per-`CopyBlock`
  attribution.** `resolve.rs::database_for` picks the first database (in
  `metadata.databases` order) whose DDL mentions the queried table — the
  same simplification the CLI used ad hoc before this module existed, now
  just centralized. The phase doc's "Multi-database dumps" section specifies
  real disambiguation (query errors naming the candidates when a table
  matches DDL in more than one database) and real attribution (which
  database a *found* `CopyBlock` belongs to, so a query can tell which
  database's data it's reading, not just which database's DDL to type
  against) — neither is implemented. koji is still single-database, so this
  path has no *real-dump* exercise, but Phase 2.3.1 added fixture coverage
  (two concatenated `--create` dumps —
  `docs/design/roadmap-phase2.3.1-multidb-fixtures-notes.md`) that proves
  the gap end to end rather than only against hand-written input: a table
  name genuinely defined in two databases resolves types against the first
  one silently (`tests/pgtype.rs`) and `stream::table_stream` returns both
  databases' rows unioned (`tests/stream.rs`) — both now pinned by tests, not
  just described here. **Scheduled as slice 2.3.3**, generalized to cover
  bare-name cross-*schema* ambiguity within a single database as well — see
  `roadmap-phase2-typed-columns.md`, "One target per query".
- **`ColumnResolution::Deferred`'s `kind` doesn't distinguish a built-in
  range from a user-defined one**, even though `pgtype.rs` now recognizes
  both. Nothing downstream needs the distinction yet — Phase 4's decoder
  will need the *subtype* either way, held in `TypeDef::Range::subtype` for
  the user-defined case and nowhere at all for the built-in ones (PostgreSQL
  encodes it in the catalog, not in DDL text) — so this is deferred to
  whichever slice of Phase 4 builds the range decoder, not decided now.
  Multiranges (2.3.3) join the same bucket for the same reason.
- **`map_builtin` covers ranges but not multiranges**, and
  `resolve_user_type`'s `types.iter().find()` assumes one `TypeDef` per name
  — which a completed base type violates, since `pg_dump` emits it as a
  `SHELL TYPE` entry plus a `TYPE` entry under the same name (I11). Harmless
  while `Base` and `Shell` share the `OpaqueBaseType` outcome. Both addressed
  in 2.3.3.
