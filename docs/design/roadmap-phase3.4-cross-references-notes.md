# Phase 3.4 — the cross-reference set: how it landed

Companion to
[`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md)'s
"What a span carries". Per `CLAUDE.md`, consolidated into the phase-level
notes doc (and removed) once all of Phase 3 lands.

## What landed

`DumpIndex` grows `roles: BTreeSet<String>` and `tablespaces: BTreeSet<String>`
(`index.rs`) — flat, per-file, persisted (cache format v6→v7). Populated by
`map::Builder`, which now accumulates the same two sets as it classifies
spans:

- `Builder::push_span` reads `toc.owner`/`toc.tablespace` off every span's
  parsed [`TocHeader`], regardless of the span's kind — the source I16/I18
  already establish.
- `Builder::push_statement_span` — the new entry point every `classify(buf)`
  call site now goes through instead of calling `classify` directly — first
  runs `preamble::extract_statement_cross_refs(buf, ...)` over the statement's
  own text, then classifies it as before. This is what reaches the three
  statement shapes no TOC field covers: `ALTER ... OWNER TO`, `GRANT`/
  `REVOKE`/`ALTER DEFAULT PRIVILEGES FOR ROLE`, and `SET default_tablespace`
  (I19, the new invariants-register entry this slice added).

`extract_statement_cross_refs` (`preamble.rs`) matches by marker substring —
`find_ci` for the keyword, `Cursor::parse_ident` (already used for qualified
names) for the identifier that follows — the same tolerance level
`parse_toc_header_line` already documents for the TOC grammar: this only ever
feeds enrichment, never a span boundary, so it doesn't try to be a full SQL
parser. `insert_role`/`insert_tablespace` (`preamble.rs`, `pub(crate)`) are
the shared filters — `PUBLIC`/`pg_default` are never recorded — used by both
`push_span`'s TOC-sourced path and the statement-sourced path, so the
filtering logic exists exactly once.

**The `PUBLIC` filter must be case-insensitive.** `Cursor::parse_ident`
lowercases every *unquoted* identifier it parses (matching how PostgreSQL
folds one) — including the literal keyword `PUBLIC` a `GRANT`/`REVOKE`'s empty
grantee list writes, which arrives at the filter as `public`. Found by the
first version of `a_grant_records_its_grantee_but_not_public`, which compared
against literal `"PUBLIC"` and failed. Fixed by comparing case-insensitively
in both filters (`pg_default` too, for consistency, though its source-form
case never actually varies). This is also an irreducible ambiguity in the
dump text itself, not just in this parser: `GRANT ... TO public;` (unquoted)
means the pseudo-role to PostgreSQL's own grammar too, never a same-named real
role — a role genuinely named `public` can only be granted to when quoted,
which `fmtId` would do, and which `Cursor::parse_ident`'s quoted branch
preserves case for.

`stream.rs`'s `map_forward` (the live segment) and `index.rs`'s
`scan_preamble` (the `--preamble-only` prepass) both merge the builder's
`roles()`/`tablespaces()` into the caller's `DumpIndex` at every point they
already persist spans — `map_forward` does it per-`CopyEnd` snapshot (not just
once at `finish`), so a resumed/interrupted scan's partial cross-reference set
is exactly the persisted spans' prefix, the same partiality
`DatabaseMetadata::preamble_complete` already signals for its own data.

## A design-doc ambiguity resolved while implementing

The design doc's "What a span carries" section reads, in one sentence, as if
the cross-reference set were part of *what an individual span carries*. It
isn't: it's a **file-level accumulator**, built the same way `DumpMetadata`
is (`dump_metadata_from_spans`) but *not* stored as a derived view over
`spans` — unlike `DumpMetadata`, recovering a role/tablespace reference from
`Span::text` alone means re-parsing raw statement text, not filtering an
already-typed `SpanBody` variant, so recomputing it on every load would mean
paying that parse on every `pgdq info` rather than once. That's why it's
accumulated *during* the scan (in `Builder`, where the statement buffer is
already in hand and the parse is free) and persisted directly, rather than
derived from `spans` after the fact the way `metadata`/`diagnostics` are. The
design doc's "Both sets are stored flat and per-file" sentence is the one to
trust; a future edit should make the "What a span carries" section's phrasing
match it.

## What the next slice inherits

- `DumpIndex::roles`/`tablespaces` are complete only once `scanned_through`
  reaches the file's size — the same caveat `metadata`'s
  `preamble_complete` already carries. koji's `backup` role (this phase's
  motivating case) is reachable only through a post-data `GRANT`, so a query
  that stops at its target (`ScanExtent::UntilTargetSettled`, the default) can
  see a real but incomplete set; `ScanExtent::Full` (or a query after `pgdq
  parse`) is the way to the complete one. Covered directly by
  `tests/query_cache.rs`'s `a_cold_query_misses_post_data_grants_but_scan_extent_full_finds_them`.
- **Per-database filtering is not implemented** — the design doc explicitly
  scopes it out ("a per-database view is a filter over each span's `database`
  attribution, not a second stored structure"), and no caller needs it yet.
  A future caller filters `spans` by `Span::database` directly rather than
  DumpIndex growing a third structure.
- I19 (`postgres-invariants.md`) is the new register entry for the four
  statement shapes `extract_statement_cross_refs` matches — read it before
  changing that function's marker strings, and its scope limit before
  trusting the `REVOKE`/non-empty-`SET default_tablespace` paths against real
  data (no fixture exercises either, the same gap I18 already names for the
  TOC's own `Tablespace:` field).
- `preamble.rs` is 1337 lines; the `objects.rs` split the phase spec
  conditions on "~1500 lines" is still not triggered.
