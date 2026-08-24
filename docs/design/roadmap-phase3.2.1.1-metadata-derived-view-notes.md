# Phase 3.2.1.1 — `DumpMetadata` as a derived view over spans: how it landed

Companion to [`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md)'s
slice table. That doc states the design; this one records how slice 3.2.1.1
landed in code and the decisions made while doing so. Per `CLAUDE.md`, this
file is consolidated into a single phase-level notes doc (and removed) once
all of Phase 3 lands.

## What landed

- **Three new `SpanBody` variants** (`crate::map`): `Connect { database:
  String }`, `VersionHeader { server_version: Option<String>, pg_dump_version:
  Option<String> }`, `AlterTypeAddValue { type_name: String, label: String }`.
  Each replaces what used to be a generic `Framing`/`Unparsed` span for that
  content — a `\connect` line, the two-line `-- Dumped from database
  version .../-- Dumped by pg_dump version ...` block (I9), and a
  `--binary-upgrade` dump's `ALTER TYPE ... ADD VALUE ...;` (I6)
  respectively — because `dump_metadata_from_spans` (below) needs the actual
  payload, not just "this was framing."
- **`crate::preamble::dump_metadata_from_spans(&[Span]) -> DumpMetadata`**: a
  pure function mirroring the deleted `PreambleBuilder`'s state machine
  exactly — multi-database segmenting on `Connect`, version-header staging
  across that boundary (a later database's headers arrive while `current` is
  still the previous, already-`preamble_complete` segment), and
  `--binary-upgrade` enum-label folding via a new `fold_alter_type_add_value`
  helper — but keyed on span kinds instead of line prefixes.
- **`crate::map::classify`** recognizes `ALTER TYPE ... ADD VALUE ...;` via a
  new `crate::preamble::parse_alter_type_add_value_body` (the parsing half of
  the old `apply_alter_type_add_value`, split from its folding half since
  `classify` has no open `TypeDef` to fold into — that's
  `dump_metadata_from_spans`'s job now).
- **`build_index` and `scan_preamble` no longer run a separate `PreambleBuilder`
  pass.** Both now build spans only (via `map::Builder`) and derive
  `DumpMetadata` from the result with `dump_metadata_from_spans`. This is the
  cutover the design's "The span is the container" section describes: parsed
  content lives in the span, once.
- **`PreambleBuilder` and `PendingStmt` are deleted.** Nothing called them
  once the two functions above stopped needing them. `strip_kw` was widened
  to `pub(crate)` (needed by `crate::map::classify`'s own `ALTER TYPE`
  check), and `PreambleBuilder::finalize` survives as a free function
  (`finalize`) since `dump_metadata_from_spans` still needs it.
- **Test coverage**: `tests/map.rs::metadata_from_spans_matches_preamble_builder_exactly`
  ran `dump_metadata_from_spans(&index.spans)` against the *old*
  `PreambleBuilder`-derived `index.metadata` across every fixture (96 files)
  plus `edge_cases.sql` — including the concatenated multi-`\connect` dump,
  every `--binary-upgrade` flag set, and the version-header staging case —
  and passed unmodified before the cutover, which is what made deleting
  `PreambleBuilder` safe rather than a leap of faith. `src/preamble.rs`'s own
  unit tests were ported from feeding raw lines through `PreambleBuilder` to
  either calling `classify_statement` directly (the DDL-grammar-only cases,
  which never depended on the builder's state machine) or constructing a
  small hand-written `Vec<Span>` and calling `dump_metadata_from_spans` (the
  state-machine cases: multi-database segmenting, header staging, enum-label
  folding). One test (`binary_upgrade_oid_noise_between_comment_and_statement_is_skipped`)
  was dropped rather than ported: it tested `PreambleBuilder::feed_line`'s
  line-triggered noise-skipping specifically, a concern that doesn't exist at
  the span level — noise between two objects is already its own span by the
  time `dump_metadata_from_spans` sees it, which every other span-based test
  already demonstrates.

## What this slice does not do, and which slice does it

Split out as **3.2.1.2** (earned mid-slice — see
[`../status/history/2026-08-24.md`](../status/history/2026-08-24.md) for why):
`crate::stream::table_stream`'s live segment still only tracks
`CopyStart`/`CopyEnd`/`\connect`, not full DDL classification, so a
query-built `DumpIndex`'s spans don't tile the way `build_index`'s do, and
its `metadata` still comes from `scan_preamble`'s own (discarded) spans
rather than a merge into the persisted index. Scoping this surfaced a real
blocker beyond "wire it up": `map::Builder::finish` consumes `self` and can
only be called once, but `Recorder` needs to persist progress after *every*
completed block, not just at the true end of the live segment — the two are
incompatible without a new, non-consuming "spans so far" capability on
`Builder` that doesn't exist yet.
