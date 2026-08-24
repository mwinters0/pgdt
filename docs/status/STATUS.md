# Status

Snapshot of implementation state. Rewritten in place as state changes — this
doc describes what *is*, not how it got there. For dated notes on what a future
session should pick up, or on discoveries that changed the plan, see `history/`
(one file per day, `YYYY-MM-DD.md`) — not a changelog, only entries worth
keeping.

Phase 1 (MVP) is complete: every functional item in
`docs/design/roadmap-phase1-mvp.md` is implemented; what remains under "Not
started" is groundwork that phase specified but never required. How
it landed — module map, and the implementation facts later phases inherit — is
in `docs/design/roadmap-phase1-mvp-notes.md`.

Phase 2 (typed columns) is complete: every functional item in
`docs/design/roadmap-phase2-typed-columns.md` is implemented, across five
slices plus the `<N>.<M>` follow-ups each earned by changing an
already-landed slice's contract (2.2.1, 2.3.1, 2.3.2, 2.3.3). How it landed —
module map, and the implementation facts later phases inherit — is in
`docs/design/roadmap-phase2-typed-columns-notes.md`.

Phase 3 (full DDL object inventory) has landed every slice in
`docs/design/roadmap-phase3-object-inventory.md`'s checklist; the phase is
**not yet wrapped** — its per-slice notes docs (3.1 through 3.6) still need
consolidating into one `roadmap-phase3-object-inventory-notes.md` before this
section can be rewritten the way Phase 1/2's are above.

Last updated: 2026-08-24 (3.5 complete — CLI surface and the dump-inspection
manual page, see the 3.5 checklist entry. All of Phase 3's slices are now
landed; phase wrap is still open, see "Not started").

## Phase 3 progress

- [x] **3.1** The `objects` fixture schema — every TOC `Type:` kind neither
      existing schema produces, plus large objects, under a
      `{default, verbose}` flag set for all 6 routine versions. No library
      code. Notes: `docs/design/roadmap-phase3.1-objects-fixture-notes.md`
- [x] **3.2** `map.rs` as a standalone module: `Span`/`SpanBody`, a
      statement-driven boundary/classification pass, `check_tiling` and its
      test over all 96 fixtures, cache identity checking (format v1→v2), the
      hardened statement accumulator. Notes:
      `docs/design/roadmap-phase3.2-span-model-notes.md`
- [x] **3.2.1** The map becomes `DumpIndex`'s primary structure — `spans`
      primary with `blocks()`/`blocks_for` derived, `Span::Data` holding
      `CopyBlock` inline, spans persisted (cache format bump), and
      `build_index` building spans in its existing pass instead of
      `build_map` being a second one. `Unscanned` is now produced for real by
      `preamble_only`. (Its `stream.rs` half — a `Recorder` appending each
      live-discovered block as a bare `Span::Data` — was superseded by
      3.2.1.2.1 and no longer exists.) Earned by 3.2's mis-sizing, not by a
      wrong contract. Notes:
      `docs/design/roadmap-phase3.2.1-span-wiring-notes.md`.
- [x] **3.2.1.1** `DumpMetadata` as a memoized derived view over `spans`
      (`dump_metadata_from_spans`), replacing the separate `PreambleBuilder`
      pass it used to run alongside the span builder — `SpanBody` grows
      `Connect`, `VersionHeader` and `AlterTypeAddValue` to carry what
      `Framing`/`Unparsed` couldn't. Verified against the old pass's output
      across every fixture before `PreambleBuilder` was deleted. Earned by
      3.2.1's mis-sizing, not by a wrong contract. Notes:
      `docs/design/roadmap-phase3.2.1.1-metadata-derived-view-notes.md`.
- [x] **3.2.1.2** `map::Builder::snapshot` — a non-consuming "spans so far"
      read, sound at any boundary where nothing is mid-classification (e.g.
      right after `on_copy_end`). `push_span` now fixes up the previous
      span's `end` at push time rather than deferring it to a single
      end-of-scan pass. No caller yet. Notes:
      [`roadmap-phase3.2.1.2-builder-snapshot-notes.md`](../design/roadmap-phase3.2.1.2-builder-snapshot-notes.md).
- [x] **3.2.1.2.1** Mapping and streaming are separate passes in `stream.rs`:
      `map_forward` classifies DDL and never yields, stopping once the
      queried table is settled; rows come only from replaying already-mapped
      blocks, resume included. `CopyBlock` gains `partition_root` (I2),
      `BatchOptions` gains `ScanExtent`, cache format v3→v4, and a
      `partitions` fixture schema lands. A query-built `DumpIndex` now tiles
      — and matches `build_index` span for span. Notes:
      [`roadmap-phase3.2.1.2.1-mapping-streaming-split-notes.md`](../design/roadmap-phase3.2.1.2.1-mapping-streaming-split-notes.md).
- [x] **3.2.2** `Span::text` sliced from the file by offset (64KB cap +
      `truncated` marker, no text on `Data`/`Unscanned` spans, cache format
      v4→v5), and a new L1 `diagnostic.rs` carrying
      `DumpIndex::diagnostics` — through which the runtime tiling check and
      the cache's mtime warning now report. Notes:
      [`roadmap-phase3.2.2-span-text-diagnostics-notes.md`](../design/roadmap-phase3.2.2-span-text-diagnostics-notes.md).
- [x] **3.2.3** `scan::Event::DollarQuoteEnd`, a position-only event for the
      end of a dollar-quoted region, which `map::Builder` treats as
      completing the statement in flight — so a TOC-comment-less dump
      degrades to one span per object instead of one span for the rest of the
      file. Real `pg_dump` output is unaffected. Earned by a wrong contract,
      not by mis-sizing. Notes:
      [`roadmap-phase3.2.3-dollar-quote-end-notes.md`](../design/roadmap-phase3.2.3-dollar-quote-end-notes.md).
- [x] **3.3** The TOC enrichment layer: `Span::toc` (owner, kind label, the
      `Tablespace:` field), TOC-coverage as an `Info` diagnostic. Also fixed
      two pre-existing boundary bugs its own tests surfaced: a `Data` span's
      TOC comment wasn't actually being absorbed into it (every real `COPY`
      block, not an edge case), and `scan_preamble` needed to retreat its
      stop point rather than guess a still-pending comment's classification.
      Notes:
      [`roadmap-phase3.3-toc-enrichment-notes.md`](../design/roadmap-phase3.3-toc-enrichment-notes.md).
- [x] **3.4** The cross-reference set — referenced roles and tablespaces.
      `DumpIndex::roles`/`tablespaces`, accumulated in `map::Builder` from
      `Span::toc` and (new) `preamble::extract_statement_cross_refs` over
      `OWNER TO`/`GRANT`/`REVOKE`/`ALTER DEFAULT PRIVILEGES FOR ROLE`/`SET
      default_tablespace`; `PUBLIC`/`pg_default` filtered, cache format v6→v7.
      `objects.rs` not split out — `preamble.rs` is still under the ~1500-line
      threshold. Notes:
      [`roadmap-phase3.4-cross-references-notes.md`](../design/roadmap-phase3.4-cross-references-notes.md).
- [x] **3.6** The `Data`-span fast path for `INSERT` runs and the
      large-object region, plus this phase's two gating measurements.
      `SpanBody::Data` now holds `crate::map::DataBlock`
      (`Copy`/`InsertRun`/`LargeObjects`), a new `crate::scan::CopyScanner`
      state skips the large-object region at the scanner level, cache format
      v7→v8. The synthetic large-object measurement: ~1.9GB/s on a 3GB
      region, an order of magnitude above the 243MB/s `COPY`-decode baseline.
      The koji full re-scan: `exit=0`, output byte-for-byte identical to the
      pre-3.6 baseline (74 blocks, same row counts, same offsets); its raw
      throughput figure was confounded by a concurrent Postgres restore on
      the same HDD and is not a like-for-like comparison against the 243MB/s
      baseline — see the notes doc for the full reasoning and evidence, and
      "Decisions worth another look" below. Notes:
      [`roadmap-phase3.6-bulk-region-fast-path-notes.md`](../design/roadmap-phase3.6-bulk-region-fast-path-notes.md).
- [x] **3.5** CLI surface: `pgdq info` gains `roles`/`tablespaces`/`object
      kinds` summaries by default (empty sets print nothing) and a `--map`
      flag listing every span in file order (not just `COPY` blocks),
      mutually exclusive with `--preamble-only`. `docs/manual/dump-inspection.md`
      is the new manual page; `README.md` links it alongside
      `type-handling.md`. Pure presentation over already-tested `DumpIndex`
      data — no library code changed. Notes:
      [`roadmap-phase3.5-cli-surface-notes.md`](../design/roadmap-phase3.5-cli-surface-notes.md).

## Not started

- **Phase 3's wrap step** — every slice (3.1 through 3.6) is landed, but the
  phase itself isn't wrapped: the per-slice notes docs still need
  consolidating into one `roadmap-phase3-object-inventory-notes.md` (per-slice
  files deleted after) and this file's Phase 3 section rewritten to the
  terse "complete" form Phase 1/2 carry above, per `docs/process.md`'s step
  5. Re-grilling the roadmap before Phase 4 gets a spec (step 6) follows the
  wrap.
- **Phases 4-8** — not designed. See `docs/design/roadmap.md`.

## Known gaps

- A query stops mapping once its target is settled, so a conflicting
  candidate **past** the stopping point is never seen and
  `Error::AmbiguousTable` is not raised for it — the query returns the
  candidate it found, with no signal that another existed. The stop rule
  (`stream::target_settled`) rules out the two shapes that announce
  themselves: a matching block carrying a partition-root marker (I2), and a
  file containing any `\connect` at all (`pg_dumpall`, concatenation,
  `--create`). What is left undetectable is a file whose *first* segment is a
  plain dump with something concatenated after it — nothing in the prefix says
  so. `ScanExtent::Full` (or a query after `pgdq parse`) gives exact
  detection. Rows are never a union either way, and ambiguity is now raised
  *before* any row is emitted rather than partway through one candidate's,
  which is what the previous form of this gap cost.
- `pgdq info <file>` (no `--preamble-only`) trusts whatever cache
  `CacheMode::load` finds without checking it actually covers the whole file:
  running `pgdq info <file> --preamble-only` and then `pgdq info <file>`
  prints the preamble-only cache's metadata but reports zero `COPY` blocks
  instead of running a full scan. Predates Phase 3.2.1 (reproduced against
  `main` before that slice's changes); not fixed there since it's unrelated
  to span wiring — see
  `docs/design/roadmap-phase3.2.1-span-wiring-notes.md`. Reachable more often
  since 3.2.1.2.1, because an ordinary query now leaves a *partial* cache by
  design rather than a whole-file one. Likely fix: compare `scanned_through`
  against the source's size before trusting a loaded cache as complete.
- `Span::toc`'s `Tablespace:` field and `TOC_PREFIX_STATS` ("Statistics for
  Name: ...", a v18+ `--with-statistics` component) are parsed from source
  reading alone (I18), with no fixture exercising either — creating a
  non-default tablespace needs filesystem access the fixture generator
  doesn't have. `parse_toc_header_line` degrades gracefully if either shape
  is wrong (the field, or the whole header, just parses to `None`), but
  neither claim has been checked against real `pg_dump` output.
- `DumpIndex::roles`/`tablespaces` are complete only once `scanned_through`
  reaches the file's size — the same partiality `metadata`'s
  `preamble_complete` already carries, for the same reason: a query that
  stops at its target (`ScanExtent::UntilTargetSettled`, the default) never
  reaches a reference past the stopping point, which is exactly koji's
  `backup` role (granted only in a post-data `GRANT`). `ScanExtent::Full` (or
  a query after `pgdq parse`) gives the complete set. `REVOKE` and a
  non-default `SET default_tablespace` value are also unexercised by any
  fixture (I19), the same gap I18 already names for the TOC's own
  `Tablespace:` field.
- `DumpIndex::diagnostics` (a tiling failure, the cache mtime warning, the
  TOC-coverage figure) is fully populated by every scan but never printed by
  the CLI — `pgdq info` has no code path that reads `index.diagnostics` at
  all, in any mode. Found incidentally while building slice 3.5's output
  (`docs/design/roadmap-phase3.5-cli-surface-notes.md`); not fixed there
  since a proper drain point is Phase 6's caller-supplied-sink work
  (`roadmap-phase3-object-inventory.md`, "Diagnostics: a file-level channel
  on `DumpIndex`") and this slice's spec row didn't ask for it. A plain
  `pgdq info` run today gives no visible signal if, say, the tiling check
  ever fails on real input.

## Decisions worth another look

Calls made without the maintainer present that are worth weighing in on —
cautionary and informational, not blocking. An entry leaves this section once
it has been looked at: settled into the design docs, or reversed.

Slice 3.2.2's `Diagnostic` unification — one severity scale and two
types rather than one enum — has been reviewed and settled into
[`roadmap-phase3-object-inventory.md`](../design/roadmap-phase3-object-inventory.md)'s
"Diagnostics: a file-level channel on `DumpIndex`".

- **Slice 3.6 gave `SpanBody::Data`'s payload a shape the spec didn't pin
  down, and made two implementation calls the spec's prose left open.** The
  design's "Bulk regions" section says one `Data` span kind covers `COPY`
  blocks, `INSERT` runs and the large-object region, but a `CopyBlock`'s
  shape (header offsets, a row terminator) genuinely doesn't fit the other
  two — there was no single struct to hold all three. Landed as
  `crate::map::DataBlock`, an enum (`Copy(CopyBlock)`/`InsertRun(_)`/
  `LargeObjects(_)`) behind the one `SpanBody::Data` variant, keeping
  `DumpIndex::blocks()`/`blocks_for`'s signature (`&CopyBlock`) unchanged by
  filtering on `DataBlock::Copy`. This is the natural reading of "one span
  *kind*, not one span *shape*", but it's an architectural call a reviewer
  should confirm before Phase 8's `INSERT`-run reader or Phase 3.5's `--map`
  listing build on it. Second: `INSERT` runs got a `map.rs`-only fast path
  (reuse the existing statement accumulator, skip pushing a span per
  statement) rather than a `crate::scan`-level one like the large-object
  region got — deliberately, since the phase's "Verification" section only
  gates large-object-region throughput, not `INSERT`-run throughput, but a
  koji-scale `--inserts` dump's actual per-row cost was never measured this
  slice. Both calls, and the reasoning, are in
  [`roadmap-phase3.6-bulk-region-fast-path-notes.md`](../design/roadmap-phase3.6-bulk-region-fast-path-notes.md).
  What would change this: if Phase 8's row reader turns out to want a
  different `DataBlock` shape than what's landed, or if `INSERT`-run
  throughput needs its own measurement before this phase wraps.

- **Slice 3.6's koji regression check was accepted on a confounded
  throughput number, reasoned around rather than re-measured cleanly.** The
  re-scan's raw throughput (~110MB/s wall-clock, ~120-130MB/s by
  `node_exporter`'s disk-read counter) came in well under the 243MB/s
  baseline. Investigation (this session) found a concurrent Postgres restore
  writing ~30MB/s to the same physical HDD throughout the scan — confirmed by
  `koji-pg`'s own checkpoint-frequency logs and `node_exporter`'s
  `node_disk_written_bytes_total`, and directly by the maintainer mid-session
  — fully accounting for the gap on a disk that was ~93-95% busy either way.
  The regression check was still called a pass, on the strength of the
  scanned output being byte-for-byte identical to the pre-3.6 baseline (same
  74 blocks, same row counts, same every offset) plus the unchanged
  control-flow argument, rather than on a clean throughput number. A
  maintainer who wants a like-for-like throughput figure can re-run
  `runs/koji-3.6-scan.log`'s command once the HDD is uncontended; nothing
  about this call blocks that, and nothing found here suggests it would come
  back differently.

- **Slice 3.5's `--map` output format and `object kinds:` bucketing are new
  surface with no spec-level detail to check against.** The spec row says
  only "role, tablespace and object-kind summaries" and "a `--map` span
  listing" — every concrete choice (the `[start, end) label` line shape, the
  per-`SpanBody`-variant label text, bucketing `object kinds:` by
  `Span::toc.kind` rather than some other classification, printing
  `roles`/`tablespaces`/`object kinds` unconditionally rather than behind
  `--verbose`, rejecting `--map --preamble-only` outright rather than having
  one flag win) was made this slice with no maintainer present to weigh in
  on the CLI ergonomics. None of it is load-bearing for other code — it's
  formatting over already-tested data, easy to change without touching
  `pgdump_query` itself — but it's the first real user-facing surface this
  phase produced, so it's worth a look. Full reasoning:
  [`roadmap-phase3.5-cli-surface-notes.md`](../design/roadmap-phase3.5-cli-surface-notes.md).
  What would change this: any maintainer preference on output shape, once
  seen.
