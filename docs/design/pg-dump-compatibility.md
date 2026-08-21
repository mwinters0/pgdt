# pg_dump Option Compatibility Matrix

Tracks `pg_dump` options/output variants against this library's support
status. Exists so compatibility scope stays explicit rather than implicit —
raised during MVP design review. Populated/kept current by the fixture
tooling described in `docs/design/mvp.md` (Testing & fixtures) —
`scripts/generate_fixtures.py`, run against `postgres:13-alpine` /
`postgres:16-alpine` / `postgres:18-alpine` containers. Rows evidenced this
way say so; rows not yet exercised by that tooling are asserted from manual
inspection of the `koji` sample dump or from `pg_dump` documentation instead
— that distinction is called out per row.

Status values: **Tested** (fixture or real-dump evidence), **Untested**
(plausibly relevant, no evidence yet), **Unsupported** (explicitly out of
scope), **N/A** (flag doesn't apply to plain format at all).

| Option / variant | Status | Notes |
|---|---|---|
| `--format=plain` (default) | Tested | Confirmed against the koji sample (784GB, `pg_dump 16.14`). This is the library's only target format. |
| `--format=custom` | Unsupported | Non-goal — existing tools (`pgdumplib`, etc.) already cover this well. |
| `--format=directory` | Unsupported | Same rationale as custom. |
| `--format=tar` | Unsupported | Not named explicitly in the design docs, but falls under the same "non-plain format" exclusion as custom/directory. **Doc gap**: should be named explicitly next time `mvp.md`'s hardcoded format scope is revised. |
| `--inserts` | Tested (fixture) | Confirmed against `postgres:13/16/18-alpine`: produces `INSERT INTO` statements, zero `COPY` blocks. Parser support for this shape is still Phase 5 — this row tracks the *fixture data existing*, not library support. |
| `--column-inserts` | Tested (fixture) | Same as above; fixture confirms the variant output shape. Phase 5 for actual support. |
| `--rows-per-insert=N` | Untested | Only relevant once `--inserts` support exists (Phase 5); not yet in the fixture matrix. |
| `--data-only` | Tested (fixture) | Confirmed against all 3 routine versions: `COPY` blocks present with minimal preceding DDL, as expected. |
| `--schema-only` | Tested (fixture) | Confirmed: zero `COPY` blocks in the output across all 3 versions — the degenerate case parses to "no tables found," not an error. |
| `--no-owner`, `--no-privileges` | Tested (fixture) | Confirmed no structural effect on `COPY` blocks across all 3 versions. `--no-comments`/`--no-security-labels` not yet in the fixture matrix but expected to behave the same (same category of DDL-only removal). |
| `--clean` / `--if-exists` | Tested (fixture) | Confirmed: adds `DROP ... IF EXISTS` before `CREATE`s, no structural surprise, across all 3 versions. |
| `--schema=X` (schema filtering) | Untested | The koji sample happens to contain two schemas (`lock_monitor`, `public`) with `COPY` blocks in each, so multi-schema dumps are informally covered — but the filtering *flag* itself hasn't been exercised. |
| Multiple databases (`pg_dumpall`) | Untested / likely out of scope | Design target is `pg_dump` output, not `pg_dumpall`. Needs an explicit scope decision, not yet made. |
| Large objects (BLOBs) | Untested | Plain-format dumps represent large objects via `lo_create`/loader calls, not `COPY` blocks — likely a genuine gap in the current design's model (no notion of non-table, non-COPY binary content). Not yet scoped. |
| `\restrict` / `\unrestrict` / `\connect` (psql meta-commands) | Tested | Confirmed present in the koji sample (`pg_dump 16.14`) bracketing the DB-create preamble and the main content section. Also confirmed present in **all 3** fixture versions — `pg_dump 13.23`, `16.15`, `18.6` — from the 2025 search_path-safety patch. Narrows (doesn't yet pin down exactly) the version threshold: present by `13.23`, so introduced somewhere between `13.0` and `13.23` in that patch series (and the equivalent point release in each other series). |
| Non-UTF8 `client_encoding` | Untested | koji sample is UTF8 only. Matters because it bears on whether `COPY` data can always be treated as valid UTF-8 at the byte level. |
| Dump-level compression (`-Z`/`--compress`) | N/A | Per `pg_dump` docs, compression only applies to custom/directory/tar archive formats, not plain SQL text output. |
| External compression of the `.sql` file (e.g. `.xz`, `.gz`) | Unsupported (by design) | Input is assumed already-decompressed plain SQL text; decompression is a caller-side preprocessing step (see: the koji sample also exists as a separate `.xz`-compressed download, decompressed before use here). |
| CSV-format `COPY` blocks | Unclear / needs clarification | Listed as a future aspiration in the historical doc, but no known `pg_dump` flag actually produces CSV-format `COPY ... FROM stdin` output — plain-format dumps always use `COPY`'s TEXT format. This may refer to a hypothetical alternate input the library could accept rather than real `pg_dump` output; needs clarification before Phase 5 scoping. |
| COPY TEXT wire format stability across PG 13-18 | Tested (partial) | Confirmed structurally consistent (same `COPY`/`\.`/escaping shape) across `pg_dump 13.23`, `16.15`, `18.6` fixtures — the 3 routine versions. The full 8-version worktree sweep (`v13.0`, `v13.23`, `v14.0`, `v15.0`, `v16.0`, `v17.0`, `v18.0`, `v18.6`) as a manual/occasional job is still open, and worktree binaries still need to be built for that (see `mvp.md`). |
