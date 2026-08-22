# Phase 1 (MVP) — Full Specification

Supersedes `docs/design/historical/initial.md` for Phase 1. Problem
statement, MVP use cases, and design philosophy (Hardcoded /
Configurable-MVP / Configurable-future buckets) carry over unchanged from
that doc and aren't repeated here.

## Crate layout

Cargo workspace, two members:
- `pgdump_query` — the core library.
- `pgdump_query-cli` — thin binary crate, produces binary `pgdq`.

Edition 2024. No pinned MSRV for MVP — float to a recent stable toolchain;
there are no downstream consumers yet to accommodate.

## Core execution model

**Hardcoded:** async core, built on `tokio`. The library defines its own
minimal internal trait for byte-range reads:

```rust
trait ByteRangeSource {
    async fn read_range(&self, offset: u64, len: usize) -> Result<Bytes>;
    async fn size(&self) -> Result<u64>;
}
```

shaped to match `object_store`'s `get_range`/`head` semantics on purpose, so
a real `object_store`-backed implementation is a drop-in addition later
(Phase 4), not a redesign. MVP ships exactly one implementation: a local-file
backend (`std::fs::File::read_at` wrapped in `spawn_blocking`, since `tokio`
has no native async positioned-read). No dependency on the `object_store`
crate itself in MVP — it would pull in a cloud-SDK dependency tree that buys
nothing for "read one local file."

**Configurable (future):** a Cargo feature flag (default off) adding an
`object_store`-backed `ByteRangeSource` implementation, for S3/GCS/Azure/etc.
Not built in MVP; the trait shape above is what keeps this additive.

The parsing/scanning logic (finding `COPY` boundaries, splitting rows,
unescaping) is synchronous CPU-bound code operating on bytes already read —
only the I/O layer is async.

## Row / batch representation

**Hardcoded (MVP):** each row's columns are returned as `Utf8View` Arrow
arrays, nullable (SQL `NULL` distinct from empty string). Chosen over
`Utf8`/`LargeUtf8` because DataFusion (Phase 4's primary embedding target) is
converging on `Utf8View` as its preferred string representation — starting
there avoids a representation migration later, at the cost of being the less
battle-tested of the three options in the wider Arrow ecosystem.

Typed (non-string) Arrow columns from parsed `CREATE TABLE` DDL are Phase 2,
not MVP.

**Configurable (MVP):** batch size limits — row count and/or in-memory byte
size, whichever is hit first. Default 8192 rows (matches common Arrow/
DataFusion batch-size convention), no default byte cap. Both remain
user-settable.

**Hardcoded (MVP):** MVP always returns all columns of the queried table — no
projection. Column projection is Phase 3.

## Streaming API

**Hardcoded (MVP):**
- **Pull mode**: an async `Stream<Item = Result<RecordBatch>>`, natural for
  async engine embedding. A blocking `Iterator` wrapper over the same stream
  is provided for sync contexts (the CLI).
- **Push mode**: a **sync** callback, `FnMut(RecordBatch) -> ControlFlow<()>`,
  driven internally as the library drains the async stream. Chosen over an
  async callback to avoid `async fn`-in-traits ergonomics and per-call boxed-
  future allocation; an async caller who needs non-blocking callback behavior
  can use pull mode directly instead.
- **Resume/position tracking**: an opaque `ResumeToken` (file offset +
  in-table row index + a cache-generation stamp), with no public field
  access in MVP. Sufficient for a caller to stop consuming partway through a
  stream and resume later within the same process. `resume` takes priority
  over cache replay: it always starts live from the token's offset — see
  "Index / structure cache" below for what that costs in the rare case of
  resuming from inside a would-be replay.
- **Cache consulting**: both entry points take a `CacheMode` — see "Index /
  structure cache" below.

**Configurable (future):** persisting a `ResumeToken` across process
restarts (independent of the index cache below).

## Predicate filtering (MVP)

**Configurable (MVP):** a single-column predicate, operator `=` or `!=`
only, compared against the raw (unparsed) string value. Evaluated **after**
a row is fully parsed/unescaped — this is correctness-level filtering, not
pushdown (pushdown, which skips parsing non-matching rows entirely, is Phase
3). Ordering operators (`<`, `>`, etc.) are deliberately excluded: on
unparsed strings they'd be actively misleading for numeric/date columns
(`"9" < "10"` is false lexicographically) and are deferred until Phase 2
typed columns can do them correctly.

Implemented as `pgdump_query::predicate::Predicate { column, op, value }`,
threaded through `table_stream`/`read_table` as `Option<Predicate>`. `column`
is resolved against the matching `COPY` block's own schema — the header's
column list, or its `column1`, `column2`, ... placeholders when it has none
— once per block, since schemas can differ block-to-block. Referencing a
column absent from a block's schema is `Error::UnknownPredicateColumn`, not a
silent non-match: a query-shaping mistake should surface immediately rather
than quietly returning zero (or every) row. **A NULL field never matches
either operator** — this MVP has no `IS [NOT] NULL` predicate, so both `=`
and `!=` collapse SQL's three-valued NULL comparison to "excluded" rather
than guessing which reading a caller wants; see
`docs/status/history/2026-08-22.md` for the reasoning.

`pgdq query <file> <table> [--cache-path PATH|none] [--filter
column=value|column!=value]` is the CLI surface: streams the table's rows
tab-separated (`\N` for NULL, matching COPY TEXT's own marker), one line per
row, a header line of column names first.

## Index / structure cache

**Configurable (MVP): whether to build/consult a cache at all.** Pure
streaming with no side effects is supported; so is cache-backed
acceleration. `table_stream`/`read_table` take a `CacheMode` directly:
`Disabled` is pure streaming; `Enabled(path)` replays every already-cached
block matching the query table (at zero I/O cost for every non-matching
cached block in between), then scans live from the cache's watermark,
persisting each newly-discovered block — matching or not — as it completes.
This is the "incremental" half of the axis below; `pgdq parse` (eager, full
scan up front) is the other.

**Known gap:** resuming a `ResumeToken` taken from partway through a
cache-replay falls back to treating the rest of the file as unscanned live
territory (re-reading, and possibly re-recording, bytes the cache already
covered) rather than resuming the replay — `ResumeToken` doesn't carry which
blocks were already known when it was taken, and `mvp.md` requires it stay
field-free/opaque. Correctness is unaffected; this only gives back some of
the I/O saving for that one combination.

**Configurable (MVP): cache location.** Exactly two options, no others:
1. Colocated with the dump file: `<dump-path>.dqcache`.
2. An explicit path supplied by the caller.

No XDG-style or other fallback location.

**Hardcoded (MVP): a cache write failure is a hard error.** If the resolved
path can't be written (read-only mount, permissions, disk full), the library
returns an error rather than silently proceeding without a cache. This is a
different axis from the "best-effort" rule below: *reading* a missing,
foreign, or stale cache falls back to scanning without complaint, because
that's the routine first-run/cold-cache case. A *write* failure means
something is actually wrong with the resolved location, and swallowing it
would silently degrade every future run of the same command back to a full
scan with no indication why.

**Configurable (MVP): explicitly disabling the cache.** A caller can opt out
of the cache entirely for one operation: any file already at the resolved
location is ignored (never read), and nothing is written when the operation
finishes. The CLI spells this `--cache-path none`. Disabling the cache is
incompatible with an operation whose entire purpose is to populate it
(`pgdq parse`) — combining the two is a hard error at both the library level
(`pgdump_query::cache::CacheMode::require_enabled`) and the CLI level, not a
silent no-op.

**Format:** custom binary encoding via `serde` + `bincode`. Not
human-readable by design — the CLI `info` command is the intended way to
inspect it.

**Contents** (per the historical doc, unchanged): byte offsets of each
discovered `COPY <table> (...) FROM stdin;` header and its terminating `\.`
line; a watermark of how much of the file has been scanned so far.

Three optional fields are reserved in the serialized form from the first
release, though none is populated in Phase 1 — adding any of them after the
format ships would be a break, and all are cheap to leave room for now: a
**sparse row index** (`docs/design/scan-performance.md`); a **dump-level
metadata block** (server version, `pg_dump` version, extension list,
user-defined type definitions — `docs/design/roadmap.md`, Phase 2 companion);
and **per-row-group column statistics**, keyed to the sparse index's checkpoints
(`docs/design/roadmap.md`, Phase 3 companion).

**Configurable (MVP): eager vs. incremental indexing.** Incremental
(default) discovers structure only as far as needed to answer the current
query, persisting whatever it discovered along the way (so a query for table
X that scans past A, B, C leaves the cache useful for those too). Eager mode
(`pgdq parse`) scans the whole file up front before answering any query.

**Hardcoded (MVP):** reading the cache is best-effort, never required for
correctness. Missing, stale, or non-covering cache → fall back to scanning
(from the furthest covered point, or from the start). This best-effort
contract covers *reads* only — see "a cache write failure is a hard error"
above.

**Configurable (future):** cache invalidation strategy if the underlying
dump file changes (size/mtime check vs. trusting the client). Note that this
stops being optional once the statistics field above is populated: a stale
structural index only costs a rescan, but a stale statistic prunes real rows and
yields a wrong answer. See the correctness asymmetry in `docs/design/roadmap.md`,
Phase 3 companion. Exporting the cache to a common/portable format (raised
during design review as a plausible later ask — not scoped further yet).

## Parser robustness requirements (hardcoded)

Derived from sanity-checking the design against the real `koji` sample dump
(784GB, `pg_dump 16.14`, plain format) — see PR/session notes for the source
data. These are correctness requirements with one right answer, not
configurable behavior:

- The scanner must tolerate arbitrary leading-backslash **psql
  meta-commands** (`\restrict`, `\unrestrict`, `\connect`, and generically
  any line starting with `\`) as skippable non-SQL lines. `\restrict`/
  `\unrestrict` are emitted by pg_dump versions carrying the 2025
  search_path-safety patch (confirmed present in `pg_dump 16.14`) and
  weren't part of the original format description.
- `COPY` block detection must be **line-anchored** (`^COPY `). Row *data*
  can legitimately contain the literal substring `COPY ... TO stdout;`
  mid-line (observed in a `lock_monitor` logging table in the koji dump) —
  a non-anchored scan would misfire on this.
- Row/column splitting must honor standard COPY TEXT escaping (`\N` for
  NULL, `\n`/`\t`/`\\` escapes) rather than naive byte-level line splitting;
  confirmed present and load-bearing in the koji sample (long text fields
  with embedded literal `\n` sequences).
- Input is assumed to already be decompressed plain SQL text — the library
  does not handle compressed dumps (`.gz`/`.xz`/etc.) itself; that's a
  caller-side preprocessing step. This is a rule about *plain-format input*
  specifically, not a global policy: archive formats (Phase 6, Track B)
  compress per entry, internally, and will need streaming decompression
  inside the container layer.

## CLI (`pgdq`)

- `pgdq parse <file> [--cache-path PATH|none]` — eager full-file scan; builds
  and writes the cache (colocated by default, or at `PATH`). `--cache-path
  none` is rejected: `parse`'s whole purpose is to persist a cache, so
  disabling it is a contradiction.
- `pgdq info <file> [--cache-path PATH|none] [--verbose]` — pretty-print what
  the cache knows: table names, column *names* (from the `COPY` header list —
  types aren't available until Phase 2), row counts. `--verbose` additionally
  shows file offsets for each `COPY` block and other cache internals.
  `--cache-path none` ignores any existing cache and performs a fresh scan
  without persisting it.
- `pgdq query <file> <table> [--cache-path PATH|none] [--filter
  column=value|column!=value]` — stream a table's rows (bare or qualified
  `table` name), one tab-separated line per row (`\N` for NULL), a header
  line of column names first. `--filter` applies the post-parse predicate
  above; omitted, every row is streamed. Consults/populates the cache the
  same way `info` does.

## Testing & fixtures

- **Unit/snapshot tests**: `insta`, run against small synthetic fixture
  dumps — not koji-derived, deliberately designed to cover the edge-case
  breadth this doc calls out (NULLs, escaped whitespace/backslashes/
  newlines, empty tables, psql meta-commands, a data value containing a
  `COPY ...;`-like substring mid-line, multiple schemas).
- **Fixture generation & the pg_dump compatibility matrix**: Python scripts
  under `scripts/` (`scripts/generate_fixtures.py`), using `uv` (already
  provisioned in `mise.toml`). The Postgres worktrees at
  `/mnt/wd12t/upstream/postgres/` turned out to be source checkouts only, not
  built binaries — building 3+ Postgres versions from source was too heavy a
  cost for routine fixture generation. Instead, the tooling drives
  memory-limited, throwaway containers (`docker`, aliased to `nerdctl` in
  this environment, via passwordless `sudo`) running the official
  `postgres:13-alpine` / `postgres:16-alpine` / `postgres:18-alpine` images —
  oldest supported major, version matching the real koji sample, newest
  available. Loads `scripts/fixture_schema.sql` (a small synthetic schema
  deliberately covering the edge cases this doc calls out, not derived from
  koji) and runs `pg_dump` across a flag matrix, writing output to
  `fixtures/<major-version>/<flag-set>.sql`. The worktrees remain useful for
  other needs (e.g. building an exact patch level unavailable as an image)
  but aren't the routine mechanism. The full historical worktree-version
  sweep (`v13.0`, `v13.23`, `v14.0`, `v15.0`, `v16.0`, `v17.0`, `v18.0`,
  `v18.6`) as a manual/occasional job is unaffected by this — it's still
  open, just not yet built. See `docs/design/pg-dump-compatibility.md` for
  the tracked option matrix this tooling exists to populate.
  Cross-checking against `pgdumplib` (Python) is in scope for this tooling
  where useful, not yet implemented.
- **Benchmarks**: `criterion`.
- **The real koji dump** (784GB, HDD): manual, opt-in performance/smoke
  validation only. Never part of CI or the routine test suite, given its
  size and the HDD's speed.

## Error handling

`thiserror` for library errors (`pgdump_query`), `anyhow` in the CLI binary
(`pgdump_query-cli`) — standard split, not further specified here.

## Decisions that keep later phases open

Phase 1 is plain-format-only and single-threaded by design. These four
choices are what make Phases 5 and 6 additive rather than a rewrite, and they
are cheap to hold to now — so hold to them, even where Phase 1 alone wouldn't
require them.

- **The COPY TEXT decoder stays independent of where its bytes came from.**
  `copy.rs` operates on a caller-owned slice and never assumes "a file at
  offset N". Every dump format stores table data as this same COPY TEXT
  payload, so this decoder is the one component all of Phase 6 reuses
  verbatim.
- **Structure discovery is a separate concern from row decoding.** `scan.rs`
  finds `COPY` boundaries in a plain file; an archive reads them from a TOC.
  Keeping the boundary between "what entries exist and where are their bytes"
  and "decode these bytes into rows" clean is what lets a container layer slot
  in later (roadmap Phase 6, Track B).
- **The cache format is versioned and records what produced it.** Serialized
  `DumpIndex` carries a format-version field and a container-kind tag from the
  first release, so archive-derived indexes and entry-relative offsets are a
  later variant rather than a breaking change. A cache whose version or kind
  isn't recognised is treated as absent — the "never required for correctness"
  rule above makes that safe.
- **`ResumeToken` exposes no fields, ever.** Phase 1's contents are a file
  offset plus a row index, but a raw file offset is meaningless inside a
  compressed archive entry. Opaque now means the representation can change
  without an API break.

Phase 5 (scan performance) adds a fifth, which bears on work in flight right
now rather than later: the batch layer should build `Utf8View` arrays over the
scanner's existing chunk buffer instead of copying field bytes out of it. See
`docs/design/scan-performance.md`.

## Non-goals (Phase 1)

- Writing or modifying dump files — permanently out of scope.
- A full SQL/DDL parser — only enough recognition to locate `COPY` blocks
  (and, in Phase 2, extract columns/types from `CREATE TABLE` for the
  table(s) being queried).
- Typed columns, predicate pushdown, column projection pushdown, and
  multi-language/engine bindings — Phases 2-4.
- Custom/directory/tar archive formats, and `--inserts`/`--column-inserts`
  input variants — Phase 6. Not Phase 1 work, but no longer permanently out
  of scope; see `docs/design/roadmap.md`.
