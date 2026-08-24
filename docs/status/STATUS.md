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

Phase 3 (full DDL object inventory) has landed every slice its spec originally
listed, plus two more the phase's end-of-phase grilling earned — **3.1.1** and
**3.3.1** — and a third grilling-earned slice, **3.7** (cache-only inspection,
absorbing what were out-of-band items M1 and M2), all now landed, plus one
remaining out-of-band item (M3). The phase is **not yet wrapped**; see "Not
started" for what remains and in what order.

Last updated: 2026-08-24 (slice 3.7 landed: `pgdq info` answers from a
retained `.dqcache` with no `--source`, `CacheMode` gains `Offline`, and
`CacheStatus` gains `Incomplete` — folding in the former out-of-band items M1
and M2).

## Phase 3 progress

- [x] **3.1** The `objects` fixture schema — every TOC `Type:` kind neither
      existing schema produces, plus large objects, under a
      `{default, verbose}` flag set for all 6 routine versions. No library
      code. Notes: `docs/design/roadmap-phase3.1-objects-fixture-notes.md`
- [x] **3.1.1** Fixtures for the four shapes 3.1 never produced, all read out
      of upstream source and never checked against real output before now:
      the TOC comment's `Tablespace:` field and a non-default `SET
      default_tablespace` (`objects.tablespaced_table`, via
      `generate_fixtures.py`'s `prepare_tablespace_dir` `mkdir`/`chown` plus
      a `CREATE TABLESPACE` in the schema SQL), a `REVOKE`
      (`objects.no_public_execute()`'s default-PUBLIC-`EXECUTE` revoke), and
      `TOC_PREFIX_STATS` (`fixtures/18/objects/stats.sql`, a new
      version-conditional `stats` flag set in `SCHEMAS`, v18 only — the real
      flag is `--statistics`, not `--with-statistics`, which does not exist).
      Closed I18 and I19, whose register entries moved from source-reading to
      fixture evidence. Notes:
      [`roadmap-phase3.1.1-toc-shape-fixtures-notes.md`](../design/roadmap-phase3.1.1-toc-shape-fixtures-notes.md).
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
- [x] **3.3.1** TOC inheritance for follow-on statements — a span continuing
      the object before it (`ALTER … OWNER TO`, `ADD MAPPING FOR`, `ALTER
      EVENT TRIGGER … DISABLE`) inherits the governing entry's `TocHeader`
      instead of carrying `None` (`Span::toc_owned` records whether a span
      carried the header text itself), and `toc_coverage_diagnostic` counts
      attributed spans — no code change there, since its numerator was
      already `toc.is_some()`. `Framing`/`Connect`/`VersionHeader` never
      inherit; a mid-file `SET default_tablespace = ...;` (which classifies
      as `Framing` only once `classify` runs) is vetoed after the fact in
      `push_statement_span`. `pgdq info`'s `object kinds:` breakdown moved to
      `toc_owned` spans in the same slice — verified against
      `fixtures/16/objects/default.sql`: `TABLE: 8`, unchanged, not
      double-counted. `docs/manual/dump-inspection.md` needed no change — its
      "object kinds" description already reads as a census. Cache format
      v8→v9. Notes:
      [`roadmap-phase3.3.1-toc-inheritance-notes.md`](../design/roadmap-phase3.3.1-toc-inheritance-notes.md).
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
- [x] **3.7** Cache-only inspection: `pgdq info` (default, `--map`,
      `--preamble-only`) answers from a retained `.dqcache` with its source
      dump gone. All three subcommands move from a `file`
      positional/`--cache-path` to `--source`/`--dqcache`; `query`'s `table`
      positional becomes `--table`. `CacheMode` gains `Offline(PathBuf)`
      with its own `load_offline`, rejected library-side (not just by the
      CLI) by `load`/`save`/`require_enabled`/`read_table` the one way and
      by `load_offline` the other. `CacheStatus` gains `Incomplete` (former
      out-of-band item M1), and `pgdq info` prints `DumpIndex::diagnostics`
      (former out-of-band item M2) — cache-only mode's "unverified,
      historical" banner rides that same path. Notes:
      [`roadmap-phase3.7-cache-only-inspection-notes.md`](../design/roadmap-phase3.7-cache-only-inspection-notes.md).

## Not started

One item remains before Phase 3 wraps:

- **Out-of-band item M3** — the synthetic `INSERT`-run throughput
  measurement; see "Decisions worth another look".

Then the wrap, and only then the step-6 roadmap re-grill.
- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; the resulting changes land as
  out-of-band items too.
- **A like-for-like koji throughput re-measurement** — deferred past Phase 3;
  the HDD is still contended. See "Decisions worth another look".
- **Phase 3's wrap step** — every slice (3.1 through 3.7) is landed, but the
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
  which is what the previous form of this gap cost. Accepted, not
  scheduled — closing it means abandoning early stopping, which is the
  phase's cost argument. Filed into
  [`roadmap-phase6-inbox.md`](../design/roadmap-phase6-inbox.md) so the
  embedded API's promises get decided against it deliberately.
- `map::parse_toc_header_line` does not recognize `TOC_PREFIX_STATS`
  (`"Statistics for "`, a v18+ `--statistics` component — not
  `--with-statistics`, which does not exist in any version). A deliberate
  deferral rather than a gap: fixture evidence now exists
  (`fixtures/18/objects/stats.sql`, slice 3.1.1) and the entry just degrades
  gracefully, tiling as an ordinary `Unparsed` span with `toc: None`, the
  same as any other unhandled TOC comment shape.
- `DumpIndex::roles`/`tablespaces` are complete only once `scanned_through`
  reaches the file's size — the same partiality `metadata`'s
  `preamble_complete` already carries, for the same reason: a query that
  stops at its target (`ScanExtent::UntilTargetSettled`, the default) never
  reaches a reference past the stopping point, which is exactly koji's
  `backup` role (granted only in a post-data `GRANT`). `ScanExtent::Full` (or
  a query after `pgdq parse`) gives the complete set.

## Decisions worth another look

Calls made without the maintainer present that are worth weighing in on —
cautionary and informational, not blocking. An entry leaves this section once
it has been looked at: settled into the design docs, or reversed.

Slice 3.2.2's `Diagnostic` unification — one severity scale and two
types rather than one enum — has been reviewed and settled into
[`roadmap-phase3-object-inventory.md`](../design/roadmap-phase3-object-inventory.md)'s
"Diagnostics: a file-level channel on `DumpIndex`".

Slice 3.6's `DataBlock` shape — an enum (`Copy`/`InsertRun`/`LargeObjects`)
behind the one `SpanBody::Data` variant rather than three sibling `SpanBody`
variants — has been reviewed and settled into
[`roadmap-phase3-object-inventory.md`](../design/roadmap-phase3-object-inventory.md)'s
"Bulk regions", along with the evidence that decided it (two call sites want
the generic *is this bulk row data* predicate).

Slice 3.5's `--map` output format and `object kinds:` bucketing have been
reviewed and accepted **as provisional**: the CLI surface stays as it is until
real users have tried it, and their feedback becomes its own work item. See
"Not started".

- **Slice 3.6 put `INSERT` runs on a `map.rs`-only fast path, and never
  measured one.** The large-object region got a `crate::scan`-level fast path
  (lines skipped unread); `INSERT` runs instead reuse the existing statement
  accumulator and skip pushing a span per statement, which removes the
  per-statement span/text cost but still decodes every line into
  `Event::Line`. Deliberate — the phase's "Verification" section gates only
  large-object-region throughput — but a koji-scale `--inserts` dump's actual
  per-row cost has never been measured, so the phase's cost claim for
  `--inserts` input rests on argument rather than a number. Reasoning:
  [`roadmap-phase3.6-bulk-region-fast-path-notes.md`](../design/roadmap-phase3.6-bulk-region-fast-path-notes.md).
  Scheduled as out-of-band item **M3**, before the phase wraps: a synthetic
  `INSERT` run on the SSD, measured the way 3.6's large-object bench was
  (`scripts/generate_large_object_bench.py` is the pattern). A confirming
  number changes no decision; a bad one — `Event::Line` decode dominating —
  earns `INSERT` runs a scanner-level path, which does change one, and
  escalates M3 to a slice.

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
  back differently. **The maintainer has deferred the re-measurement past
  Phase 3** — the HDD is still contended — so this entry stays until that run
  happens.

- **Slice 3.7's `CacheMode::load` does not fold `CacheStatus::Incomplete`
  into `None`, despite the spec's "live mode still treats it like Absent
  (falls back to a scan)".** Read literally at `CacheMode::load` — the
  `Option<DumpIndex>`-returning method `table_stream` and `preamble_only`
  both call to seed an incremental scan — that sentence would make every
  ordinary query's partial cache invisible to the next query, discarding the
  entire benefit of the structural cache: a cold query's map is *designed*
  to stop short of the file's size once its target settles, so a partial
  cache is the normal shape there, not a defect. `CacheMode::load` treats
  `Incomplete` exactly like `Valid` instead; the one caller that actually
  needs the "is this the *whole* file" distinction — `pgdq info`'s
  default/`--map` fallback, which is what the M1 bug this slice fixes was
  actually about — checks `scanned_through` against the live source's size
  itself, which it already has to stat regardless. Reasoning:
  [`roadmap-phase3.7-cache-only-inspection-notes.md`](../design/roadmap-phase3.7-cache-only-inspection-notes.md).
  Made without the maintainer present; if reconsidered, the fix is
  mechanical — fold `Incomplete` into `None` in `CacheMode::load` and accept
  that `table_stream`/`preamble_only` lose incremental cache reuse, or add a
  second entry point for them that bypasses the fold.

