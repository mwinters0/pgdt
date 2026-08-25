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

Phase 3 (full DDL object inventory) is complete: every functional item in
`docs/design/roadmap-phase3-object-inventory.md` is implemented, and both
measurements its "Verification" section gates the phase on are done. It landed
in fifteen slices — the six its spec originally listed, plus nine earned: six
by mid-slice re-sizing (3.2.1, 3.2.1.1, 3.2.1.2, 3.2.1.2.1, 3.2.2) or a wrong
contract (3.2.3), and three by the end-of-phase grilling (3.1.1, 3.3.1,
3.7). How it landed — module map, the span model, the mapping/streaming
split, and the implementation facts later phases inherit — is in
`docs/design/roadmap-phase3-object-inventory-notes.md`.

Last updated: 2026-08-25 (Phase 3 wrapped: per-slice notes consolidated;
out-of-band item M3 measured, with a result that argues for work Phase 3
declined to do — see "Decisions worth another look").

## Not started

- **Phase 4's grilling and spec** — `docs/process.md`'s step 6. The roadmap is
  re-grilled before Phase 4 is specified, since Phase 3's evidence outdates
  guesses made before it. Phases 4-8 are sketched only to
  corner-avoidance depth in `docs/design/roadmap.md`.
- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; the resulting changes land as
  out-of-band items.
- **A like-for-like koji throughput re-measurement** — deferred past Phase 3;
  the HDD is still contended. See "Decisions worth another look".

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
  detection. Rows are never a union either way, and ambiguity is raised
  *before* any row is emitted rather than partway through one candidate's,
  which is what the previous form of this gap cost. Accepted, not
  scheduled — closing it means abandoning early stopping, which is the
  phase's cost argument. Filed into
  [`roadmap-phase6-inbox.md`](../design/roadmap-phase6-inbox.md) so the
  embedded API's promises get decided against it deliberately.
- `map::parse_toc_header_line` does not recognize `TOC_PREFIX_STATS`
  (`"Statistics for "`, a v18+ `--statistics` component — not
  `--with-statistics`, which does not exist in any version). A deliberate
  deferral rather than a gap: fixture evidence exists
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
- An `INSERT` run is folded into one `Data` span, but every line in it is
  still decoded into `Event::Line` and pushed through the statement
  accumulator — unlike the large-object region, which is skipped unread at
  the scanner level. Measured (out-of-band item M3): **~218MB/s against
  ~1.0GB/s for a `COPY` dump of the same size on the same disk**, i.e. about
  5× the per-byte cost, and CPU-bound rather than I/O-bound. What this costs
  is the phase's cost claim for `--inserts` input: a koji-scale 1TB
  `--inserts` dump maps in ~75 minutes rather than the ~15 the `COPY` rate
  implies. Correctness is unaffected — the map, the tiling and the row counts
  are the same either way. Not scheduled: the fix is a scanner-level
  `INSERT` path, which changes a decision and so needs a slice, filed into
  [`roadmap-phase7-inbox.md`](../design/roadmap-phase7-inbox.md) and flagged
  below.

## Decisions worth another look

Calls made without the maintainer present that are worth weighing in on —
cautionary and informational, not blocking. An entry leaves this section once
it has been looked at: settled into the design docs, or reversed.

- **Slice 3.6's koji regression check was accepted on a confounded
  throughput number, reasoned around rather than re-measured cleanly.** The
  re-scan's raw throughput (~110MB/s wall-clock, ~120-130MB/s by
  `node_exporter`'s disk-read counter) came in well under the 243MB/s
  baseline. Investigation found a concurrent Postgres restore writing
  ~30MB/s to the same physical HDD throughout the scan — confirmed by
  `koji-pg`'s own checkpoint-frequency logs and `node_exporter`'s
  `node_disk_written_bytes_total`, and directly by the maintainer — fully
  accounting for the gap on a disk that was ~93-95% busy either way. The
  regression check was still called a pass, on the strength of the scanned
  output being byte-for-byte identical to the pre-3.6 baseline (same 74
  blocks, same row counts, same every offset) plus the unchanged
  control-flow argument, rather than on a clean throughput number. A
  maintainer who wants a like-for-like figure can re-run
  `runs/koji-3.6-scan.log`'s command once the HDD is uncontended. **The
  maintainer has deferred the re-measurement past Phase 3** — the HDD is
  still contended — so this entry stays until that run happens.

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
  default/`--map` fallback — checks `scanned_through` against the live
  source's size itself, which it already has to stat regardless. Reasoning:
  [`roadmap-phase3-object-inventory-notes.md`](../design/roadmap-phase3-object-inventory-notes.md),
  "Cache". Made without the maintainer present; if reconsidered, the fix is
  mechanical — fold `Incomplete` into `None` in `CacheMode::load` and accept
  that `table_stream`/`preamble_only` lose incremental cache reuse, or add a
  second entry point for them that bypasses the fold.

- **Out-of-band item M3 measured what slice 3.6 assumed, and contradicted
  it — the response needs a decision this session could not make.** Three
  3.00 GiB synthetic dumps, same disk, same session, three runs each: a
  `COPY` block scans in ~2.6-3.3s, a large-object region in ~3.6-5.0s, an
  `INSERT` run in **~14.6s**, against a ~3.7-3.9s `cat`-to-`/dev/null` floor
  for the same files. The first two are at the I/O floor; the `INSERT` scan
  is four times above it, ~11s of CPU per 3 GiB. So `Event::Line` decode plus
  the statement accumulator *does* dominate, which is exactly the outcome
  3.6's notes named as earning `INSERT` runs a scanner-level fast path.
  Building that path changes a decision, so it is not out-of-band work and
  was not started here; it wants grilling and a slice number, with the
  maintainer looking at this number first. Filed as a Phase 7 inbox entry
  (scan performance is where a second scanner-level fast path gets decided)
  and as a known gap above. Evidence:
  [`history/2026-08-25.md`](history/2026-08-25.md).

- **3.6's recorded ~1.9GB/s large-object figure is page-cache-warm, not a
  disk throughput.** Cold, the same 3GB bench measures ~560MB/s; warm again,
  ~890MB/s — which is the `cat` floor for that file, i.e. the scan is
  I/O-bound either way. 1.9GB/s is more than twice what this disk gives
  `cat`, so it can only have been served from cache. 3.6's conclusion is
  unaffected (the skip is still several times cheaper per byte than walking
  the same bytes, and M3 above now measures what walking costs), but the
  figure should be read as a ratio against a same-cache-state number, never
  as throughput. Both figures and the floor are recorded in the phase notes.
