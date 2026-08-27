# Status

Snapshot of implementation state. Rewritten in place as state changes — this
doc describes what *is*, not how it got there. For dated notes on what a future
session should pick up, or on discoveries that changed the plan, see `history/`
(one file per day, `YYYY-MM-DD.md`) — not a changelog, only entries worth
keeping.

## What exists

Phases 1-3 are complete and were struck at the keystone review, so there is no
per-phase checklist here any more. How the system works is
[`../design/architecture.md`](../design/architecture.md); what is still ahead is
[`../design/roadmap.md`](../design/roadmap.md).

| Capability | State |
|---|---|
| Streaming row extraction from plain-format dumps, push and pull mode, resumable | working |
| Typed Arrow columns from `CREATE TABLE` DDL, with per-column resolution diagnostics; `SchemaMode::Strings` for the untyped path | working |
| Full byte-exact file map — every byte in exactly one span, verified over every fixture | working |
| DDL object inventory: TOC enrichment, referenced roles and tablespaces, object census | working |
| Best-effort structural cache with source-identity checking and cache-only inspection | working |
| CLI `pgdq parse` / `info` / `query`, including `--map`, `--json`, and cache-only `info` | working, text output shape provisional; `--json` carries no shape promise at all. **`parse` is the only scanner** — it resumes from a matching cache and banks its progress at `COPY` block boundaries, throttled to ~5% of scan time and saving unconditionally on Ctrl-C (exit 130/143); `info` reports from the cache and never scans |
| Partial reporting | `info` reports a cache from an unfinished scan for as far as it got, with `Scan completion: N% (M bytes)` stated once at the top; `--json` carries the coverage components and per-`COPY`-block type resolution. An interrupted cache is **typed** for every database segment the scan finished — the mapping pass states each database's DDL at that database's first `COPY` block (I1) |
| Arrays, composites, ranges, multiranges | typed and decoded end to end: `List<T>`, `Struct<…>`, the five-field range struct, `List<` range struct `>`, and `List<List<T>>` for a uniformly multi-dimensional array column. Three shapes stay a string, each with its own resolution outcome: an array whose element type is opaque (`box`, a C base type, a shell type, through any chain of domains), an array whose element type is itself an array (I26), and an array column whose values disagree on shape |
| Array shape census | recorded by every mapping pass (`CopyBlock::array_shapes`) and **consumed**: a query retypes its top-level array columns from the union over the blocks it will replay, before the first batch |
| Predicate and projection pushdown; per-row-group statistics | not started — Phase 5 |
| `object_store` I/O, Python bindings, DataFusion `TableProvider` | not started — Phase 6 |
| Device-bound scan performance campaign, sparse row index | not started — Phase 7 |
| `--inserts` row reading; custom/directory/tar archive formats | not started — Phase 8 (the map already locates and attributes `INSERT` runs) |

Last updated: 2026-08-27 (**"Decisions worth another look" is empty** — all seven measurement-era entries reviewed and closed on 2026-08-27; the work they queued is `M11` under "Not started". Phase 9 is **complete and wrapped**; Phase 4 has **one item left — its wrap**. **4.6.1** has landed, which finishes the phase's measurement work: the nested end-to-end table is three files taken in one interleaved sweep, and the composite column's own share turned out to sit below what differencing two generated files can resolve. A phase boundary: an unattended loop stops here.)

## Phase 4 progress

Specified in
[`../design/roadmap-phase4-composite-decoding.md`](../design/roadmap-phase4-composite-decoding.md).

- [x] **4.1** The fixture value shapes the phase needs — lower-bound
      decoration, a mixed-dimensionality column, array-of-composite and
      composite-containing-array, a text-subtype range, array-of-enum,
      `mybase[]`. Generator plus regenerated fixtures; no library code. Notes:
      [`../design/roadmap-phase4.1-fixture-shapes-notes.md`](../design/roadmap-phase4.1-fixture-shapes-notes.md)
- [x] **4.1.1** Two fixture values found after 4.1 landed: an array over a
      domain whose base is `box` (I22 — a domain inherits its base type's
      array delimiter, so the value is semicolon-separated and escapes the
      opaque-element refusal as 4.1 knew it), and a zero-field composite
      (I23). Generator plus regenerated fixtures; no library code. Notes:
      [`../design/roadmap-phase4.1.1-delimiter-and-empty-composite-notes.md`](../design/roadmap-phase4.1.1-delimiter-and-empty-composite-notes.md)
- [x] **4.2** The nested literal codec (`nested.rs`, L2): parameterized
      quoted-token scanner plus array/record/range instantiations, decode and
      render, round-tripped against 4.1's literals. Notes:
      [`../design/roadmap-phase4.2-nested-codec-notes.md`](../design/roadmap-phase4.2-nested-codec-notes.md)
- [x] **4.3** `ColumnBuilder`'s `List`/`Struct` arms, unit-tested directly;
      nothing resolves to them yet. Notes:
      [`../design/roadmap-phase4.3-nested-builders-notes.md`](../design/roadmap-phase4.3-nested-builders-notes.md)
- [x] **4.4** Flip type resolution: recursive mapping, built-in range
      subtypes, opaque-element refusal, the `ColumnResolution` surgery
      (`OpaqueElementType` in, `Deferred`/`DeferredKind` out), the
      `(DataType, NestedPlan)` pair threaded through `ResolvedSchema` and
      `RowBatcher::new`, and `render_field`'s deletion in favour of the
      plan-taking one. Nested columns decode end-to-end on the optimistic
      path. Notes:
      [`../design/roadmap-phase4.4-resolution-flip-notes.md`](../design/roadmap-phase4.4-resolution-flip-notes.md)
- [x] **4.4.1** The presentation half: the resolved Arrow type on `pgdq info
      --verbose`'s per-column line for every column that is not `Utf8View`,
      rendered with arrow's `Display` except the range struct, which collapses
      to `Range<T>`; and the `type-handling.md` rewrite. The manual documented
      **two** ways a column of these types is still a string, not the spec's
      three; 4.5.1 added the third. Notes:
      [`../design/roadmap-phase4.4.1-presentation-notes.md`](../design/roadmap-phase4.4.1-presentation-notes.md)
- [x] **4.4.2** The array-of-array-typed-element refusal, earned from a defect
      4.4 shipped (I26): the shape resolves `Utf8View` with
      `ColumnResolution::NestedArrayElement`, `retype_from_census`'s guard is
      gone, and the resolution-outcome coverage test now enforces
      `roadmap.md`'s fixture rule. `types/default.sql` gained
      `t_nested_array` (both faces of the refusal plus the five
      working-but-unpinned shapes) and `t_enum_domain.v_empty_enum`, which the
      coverage test needed and which produced I27. Notes:
      [`../design/roadmap-phase4.4.2-nested-array-refusal-notes.md`](../design/roadmap-phase4.4.2-nested-array-refusal-notes.md)
- [x] **4.4.3** The array-declaration spellings, earned from 4.4.2:
      `pgtype::array_element` normalizes the whole `Typename` array-bounds
      production (`[]`, `[n]`, repeated, `ARRAY`, `ARRAY[n]` — I28) to the
      element type plus one array level, so `integer[][]` resolves as
      `integer[]` does and the census decides its depth; a declaration
      PostgreSQL rejects stays `Unknown`. The I26 refusal is unchanged and is
      now the only shape that reaches it. `t_array_spelling` (four spellings,
      all dumping as `integer[]` on all six majors) is the fixture half.
      Notes:
      [`../design/roadmap-phase4.4.3-array-spellings-notes.md`](../design/roadmap-phase4.4.3-array-spellings-notes.md)
- [x] **4.4.4** The array arm folded into one function, earned from 4.4.3's
      unattended call: `resolve_array(element, types) -> TypeOutcome` replaces
      `element_is_opaque`/`element_is_array` and holds the whole array
      decision over one domain walk, so which function normalizes the
      array-bounds production stops being a question. No test was edited or
      added, and `domain_terminal` has one caller again. The "Not quote-aware"
      paragraph is corrected in `array_element` **and** in
      [`../design/architecture.md`](../design/architecture.md), "Type
      resolution", which carried the same false claim (I29). Notes:
      [`../design/roadmap-phase4.4.4-array-arm-notes.md`](../design/roadmap-phase4.4.4-array-arm-notes.md)
- [x] **4.5** The shape census, **recording half**: `ArrayShape` on every
      `CopyBlock`, recorded per column, and the cache format bump that
      persists it; `DumpIndex::is_complete`. Nothing consumed it — that is
      4.5.1's half. The slice was specified as one row and split mid-slice;
      the spec's table carries the earned `4.5.1`. Notes:
      [`../design/roadmap-phase4.5-census-recording-notes.md`](../design/roadmap-phase4.5-census-recording-notes.md)
- [x] **4.5.1** The shape census, **consuming half**: the census is
      unconditional (`Builder::censusing` gone, `CopyBlock::array_shapes` a
      plain `Vec<ArrayShape>`, cache format bumped, `tests/query_cache.rs`
      comparing spans for span again); `resolve_columns` takes the census and
      retypes the `(DataType, NestedPlan)` pair from it;
      `ColumnResolution::VaryingArrayShape` and its `resolution_label` arm;
      `Error::FieldDecode` names `--schema-mode strings`; the manual's
      statement of what an array column becomes, the one case still decided
      optimistically, and the planned representation knob. Notes:
      [`../design/roadmap-phase4.5.1-census-consumption-notes.md`](../design/roadmap-phase4.5.1-census-consumption-notes.md)
- [x] **4.6** The array stress section in `generate_perf_data.py`, **gated
      behind `--arrays`** (default output verified byte-identical; `M10` has
      since split that flag from `--composite`), the `nested` group in
      `benches/decoders.rs`, and all three `measurements.md` figures: the
      census on array-bearing rows, the per-element decode micro (78
      ns/element; array and composite against a same-bytes copy control), and
      `pgdq query --schema-mode typed` against `strings`. `M10` re-took the
      first and third on a corrected generator — see
      [`../design/measurements.md`](../design/measurements.md) for the
      figures that stand. No library code. Notes:
      [`../design/roadmap-phase4.6-array-stress-notes.md`](../design/roadmap-phase4.6-array-stress-notes.md)
- [x] **4.6.1** The composite's end-to-end share, **earned** from 4.6's spec
      row naming a deliverable without naming its instrument. The third
      generated file (`--composite` without `--arrays`), the same
      `typed`-against-`strings` pair run on it, and the rewritten
      [`../design/measurements.md`](../design/measurements.md) section — now
      three rows from one interleaved sweep. **The answer is a bound, not a
      point figure**: the composite column's share reads *below zero*, and a
      control pair differing only in RNG seed reads +0.22 µs/row, so this
      instrument resolves nothing under ~±0.5 µs/row. What stands is the
      attribution that bound buys — the two array columns carry essentially
      all of the 15.1 µs/row the three nested columns cost, and the composite
      is under 4% of it. Notes:
      [`../design/roadmap-phase4.6.1-composite-share-notes.md`](../design/roadmap-phase4.6.1-composite-share-notes.md)

## Phase 9 — complete and wrapped

Specified in
[`../design/roadmap-phase9-partial-reporting.md`](../design/roadmap-phase9-partial-reporting.md);
wrapped 2026-08-27, with the per-slice notes consolidated into
[`../design/roadmap-phase9-partial-reporting-notes.md`](../design/roadmap-phase9-partial-reporting-notes.md).
Every slice landed — 9.1 (`parse` resumes and banks per block), 9.2 (`info`
stops scanning), 9.3 (the coverage line), 9.4 (per-block resolution in
`--json`), 9.5 (the save throttle and the interrupt guard, earned from 9.1's
measurement) and 9.5.1 (metadata stated at every legal boundary, earned from
9.5's verification). How the result works is
[`../design/architecture.md`](../design/architecture.md), "CLI surface" and
"The cache"; the wrap moved what the slices learned into it by subject, so the
notes doc holds only the phase's negative results and where its facts were
filed.

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; the resulting changes land as
  out-of-band items. Nothing is pooled here at present.

- **M12, M11, M13 — three out-of-band items in that order**, queued
  2026-08-27 out of the review of the measurement-era backlog. They are three
  rows rather than one because four unrelated edits under one ledger row is
  unreadable later; the order is forced, since each lands on the one before.

  **`M12` — generator hygiene**, both in `scripts/generate_perf_data.py`'s
  orbit. `FRACTIONS` switches to a uniform microsecond draw: the current list
  is chosen for the *rendered* shapes it produces, which is a correctness goal
  `fixtures/` already meets, and it is unrepresentative as a benchmark input —
  37.5% of its values render with no fractional part where 300,000 rows of
  koji's `task.create_time` hold none at all. And
  `pgdump_query-cli/tests/perf_generator_fidelity.rs`'s `have_uv()` skip
  becomes an assertion naming `mise install`, per
  [`../design/roadmap.md`](../design/roadmap.md)'s standing rule "A test may
  assume the tools `mise` pins". First, because `FRACTIONS` changes the
  generated bytes and everything downstream is measured on them.

  **`M11` — the census pre-filter on `memchr2`.** `map::Builder::on_row`'s
  pre-filter (`map.rs:1269`) is a hand-rolled scalar `raw.iter().any(…)`
  measured at ~3.8 GB/s; `memchr` is already a direct dependency
  (`pgdump_query/Cargo.toml:12`), so `memchr2(b'{', b'[', raw).is_some()` is a
  one-line SIMD swap, expected to take the brace-free census figure from +39%
  warm to roughly +7%. `on_row` also gains a doc comment naming the two
  [`../design/measurements.md`](../design/measurements.md) figures that are
  regenerated by patching that function, so the recipe is discoverable from
  the code it depends on. This is what makes the "should the pre-filter be
  skippable" question moot; it reopens only if the swap does not deliver.

  **`M13` — the warm set re-taken on tmpfs, in one session.** Five figures
  are page-cache-warm SSD reads taken before that standing rule existed: both
  census figures, the nested end-to-end table, the per-block quadratic table,
  and the `COPY` path's warm CPU that the scan-throughput table cites. Scope,
  exclusions and the staging hazard — generate the four 3.00 GiB inputs on
  tmpfs from the host, never from inside the 512 MB container, whose cgroup
  would be charged for the pages — are in
  [`../design/measurements.md`](../design/measurements.md), "The warm set, and
  what `M13` re-takes". Taken as a bundle rather than lazily, because a figure
  re-taken alone leaves the doc's warm figures disagreeing about regime, which
  is the failure both new standing rules were written from. Last, so it lands
  on the final generator and the final pre-filter. **Detached**: five reps
  across three files and two modes, plus both census pairs in both orders,
  runs past ten minutes, so `CLAUDE.md`'s "Long-running processes" rule
  applies — launch it writing to `runs/`, and let a later session read the log.

- **Order from here**, re-settled 2026-08-27 after `4.6.1` landed: the
  **Phase 4 wrap**, and nothing else. Every slice of Phase 4 has landed, as
  has everything that was queued ahead of it — M5-M10, 4.4.4, the koji wrap
  run (whose durable halves are in
  [`../design/measurements.md`](../design/measurements.md), "koji full scan",
  and [`../design/architecture.md`](../design/architecture.md), "CLI
  surface"), and Phase 9's own wrap.

  **The Phase 4 wrap** consolidates **thirteen** slice notes docs (4.1 through
  4.6.1) into one `roadmap-phase4-composite-decoding-notes.md` and deletes them
  (`process.md`, step 5). Like Phase 9's, it is an **audit of
  `architecture.md`, not a transcription** (`process.md`, "A wrap after a
  keystone is an audit"): check that doc for what the slices learned and it
  does not yet say, move that in, and leave the notes doc holding the
  residue — negative results, and facts for the next phase not already filed
  as inbox entries. 4.6 and 4.6.1 already filed four of their own findings
  that way — the array-bearing census cost into `architecture.md`, and three
  [`roadmap-phase7-inbox.md`](../design/roadmap-phase7-inbox.md) entries (two
  restated from "unmeasured" to their figures, one new: what a cross-file
  cost attribution can resolve). A short notes doc is a correct outcome; an
  absent one is not. A wrap is **not** a keystone, which is a separate,
  maintainer-triggered judgement that happens once or twice in a project's
  life; both wraps leave a spec and one notes doc behind.

  **What comes after the Phase 4 wrap is a separate conversation**, claimed by
  the maintainer on 2026-08-27. `process.md` step 6 re-grills the roadmap
  before the next phase is specified, and four are unspecified (5, 6, 7, 8);
  numeric order is not plan order, since 9 was taken ahead of 5. An unattended
  session does not pick one.

  The `--disable-triggers` fix is **not** in this order — it is unscheduled, in
  `roadmap.md`'s "Future". Neither are **M12/M11/M13** above, which are
  out-of-band and fit either side of the wrap. Phase 9 is wrapped and Phase 4 has only its
  wrap left, so this is still the phase boundary an unattended loop stops at,
  whatever remains queued behind it.

## Known gaps

- **A `--disable-triggers` dump loses TOC attribution on every data span**,
  `COPY` and `INSERT` runs alike. I31: `pg_dump` writes `ALTER TABLE … DISABLE
  TRIGGER ALL;` — and `SET SESSION AUTHORIZATION DEFAULT;` ahead of the first
  entry — between each `-- Data for Name:` block and the data, so the comment
  is no longer adjacent to what it heads and closes as its own `Framing` span,
  which clears `governing_toc`. Measured against 16.15, two tables: TOC
  coverage 2/24, the two being the comment blocks. **Accepted, not a
  correctness hazard**: tiling stays byte-exact, the table name comes from the
  `COPY` header or the `INSERT INTO` line, the object census still reads
  `TABLE DATA: 2`, and roles still come off the entry's `Owner:`. What is lost
  is the coverage diagnostic and `Span::toc` on data spans. Opt-in and gated on
  `--data-only`/`--section=data`. Not fixed opportunistically because a fix is
  three coordinated changes, one of them to a decision `architecture.md`
  states — so by `roadmap.md`'s admission rule it is a graded slice, not a
  drive-by. Wanted but unscheduled, with the three changes named:
  [`../design/roadmap.md`](../design/roadmap.md), "Future — wanted,
  unscheduled".

- **An array nested inside a composite** is still decided optimistically, so a
  multi-dimensional or `[lb:ub]=`-decorated value there is a hard
  `Error::FieldDecode` naming the column. Deliberate and permanent as things
  stand: the census is keyed by column and has nowhere to record a shape at
  that depth, so scanning more of the file cannot help. `--schema-mode
  strings`, which the message now names, returns the literal verbatim. A
  *top-level* array column no longer reaches this — 4.5.1 retypes it from the
  census on any query, cold or full — and an array whose *element type* is an
  array no longer reaches it either, since 4.4.2 refuses that shape outright
  (I26). Keying the census by path is a roadmap "Future" item and would be
  purely additive.
- **An array type this build declines to represent comes back as text with no
  way to ask for more.** Two shapes are in that state: an array whose element
  type is itself an array (`ColumnResolution::NestedArrayElement`) and an
  array column whose values disagree on shape (`VaryingArrayShape`). Neither
  is opaque — both are fully understood, and a representation that is lossless
  for every array (dimensions, lower bounds and elements in one value) would
  cover both. Accepted, not scheduled: it is a roadmap "Future" item, and
  adding it later only ever touches columns these two refusals leave as
  `Utf8View`, so it strictly widens coverage.
- **A type name that needs quoting comes back `Unknown`.** A type or domain
  whose name contains a space, a bracket, or the `ARRAY` keyword is legal, and
  `pg_dump` writes it quoted in both its `CREATE` statement and every column
  declaration using it (I29). `parse_ident` dequotes it into `TypeDef.name`
  while the declaration keeps its quotes, so the lookup never matches: the
  column resolves `Unknown` and stays `Utf8View`, and an array *of* such a type
  resolves `List<Utf8View>` — the array-ness is still read correctly, since the
  `[]` falls outside the quotes. **No spelling is misread**: a column of a type
  named `"x ARRAY"` or `"d[3]"` is not mistaken for an array, because
  `array_element`'s strip helpers bail on the trailing `"`. So the cost is a
  weaker type, never a wrong one, and every value still decodes as the text the
  file holds. Unreachable from any dump whose type names are ordinary
  identifiers, which is every fixture and the koji sample. The fix is the
  roadmap "Future" item "A real type-name tokenizer", and it is strictly
  additive.

- **Mapping is O(blocks²), and the save throttle only halved it.** Every
  `CopyEnd` rebuilds `DumpIndex::spans` whole — `map::Builder::snapshot` clones
  the builder's spans, `stream::splice` clones the prefix — so a block-rich,
  byte-poor dump pays quadratic CPU with the cache disabled entirely: 19.7 s
  for 4000 blocks under `query --dqcache none`, against under 10 ms for the
  same bytes in one block. 9.5's throttle removed the other half (44.3 s → 23.6
  s for a 4000-block `parse`). Accepted for now, not scheduled: the fix is to
  stop rebuilding the span list per block, which is the same code Phase 7's
  parallel-scan plans would rework and which
  [`roadmap-phase7-inbox.md`](../design/roadmap-phase7-inbox.md) already flags
  for assuming coverage is a contiguous prefix — so the two belong in one
  decision. A **cheap** version exists and was weighed: for `parse` nothing
  reads `index.spans` between saves, so gating the splice on the throttle the
  same way the save is gated would cost `n/20` splices instead of `n` (~19.7 s
  → ~1 s at 4000 blocks). It is not taken, because it would make an interrupt
  bank the last *saved* watermark rather than the last *completed block* —
  reversing a guarantee 9.5 established — and `Builder::snapshot` asserts
  `Idle`, so the chunk-top interrupt check cannot re-derive the spans mid-block
  to compensate. The analysis is in the phase-7 inbox so it is not re-derived. Nothing koji-shaped is affected: 74 blocks over 784 GB pay this
  74 times, and the figure there is +1.5%. Figures and commands:
  [`../design/measurements.md`](../design/measurements.md), "Per-block cache
  saving is quadratic in block count, and so is the map".

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
  scheduled — closing it means abandoning early stopping, which is what makes a
  cold query on a large dump affordable. Filed into
  [`roadmap-phase6-inbox.md`](../design/roadmap-phase6-inbox.md) so the
  embedded API's promises get decided against it deliberately.
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
  the scanner level. Measured at **~209MB/s cold against ~1.10GB/s for a `COPY`
  dump of the same size on the same disk read page-cache warm**, i.e. about 5×
  the per-byte CPU, and CPU-bound rather than I/O-bound; figures and re-run
  commands in [`../design/measurements.md`](../design/measurements.md). A
  koji-scale 1TB `--inserts` dump therefore spends ~45 minutes of CPU that a
  `COPY` dump of the same size does not. Correctness is unaffected — the map, the tiling and the row counts
  are the same either way. Not scheduled: the fix is a scanner-level
  `INSERT` path, which changes a decision and so needs a slice, filed into
  [`roadmap-phase7-inbox.md`](../design/roadmap-phase7-inbox.md).

## Decisions worth another look

Calls made without the maintainer present that are worth weighing in on —
cautionary and informational, not blocking. An entry leaves this section once
it has been looked at: settled into the design docs, or reversed. **Nothing is
open** — the measurement-era backlog was reviewed on 2026-08-27; the notes
below say how each went.

*4.6.1's re-taken table and the ratios that moved were reviewed on
2026-08-27, and the swing is **neutralized by construction rather than
investigated**.* Reconstructing both sessions' raw legs settled it: the shift
is common-mode within a file — `M10`'s `strings`/`typed` read 8.80/19.89 s
(control) and 8.04/27.74 s (nested) against this sweep's 9.74/20.55 and
9.56/29.45 — so the ratio moved because the drifting baseline sits in its
denominator, while the per-row differences the design consumes did not. Three
things changed instead of a diagnosis. The per-row difference is now the
headline of [`../design/measurements.md`](../design/measurements.md)'s nested
section and the ratio a derived column; two standing rules were added there —
a parsing-CPU figure is taken with its input on **tmpfs**, never page-cache
warm off a filesystem (device time and background I/O swamp the difference,
worst on the HDD, and page-cache residency is an assumption), and a comparison
table is re-taken **whole in one interleaved sweep**, never differenced across
sessions or run a file at a time. And `M10`'s explanation of its own 9%
between-file baseline gap — "the untyped path is partly per-row" — is
**retracted**: the interleaved sweep puts the same files 2% apart with the
row-count spread unchanged, so the gap was a file-at-a-time sweep mapping the
session's drift onto file identity. Both rules are also in
[`../design/roadmap-phase7-inbox.md`](../design/roadmap-phase7-inbox.md),
since that campaign is where they bite.

*The composite column's unmeasured end-to-end share was reviewed on
2026-08-27, and the tick **stands** — the promise is corrected, not the
measurement.* Nothing consumes a figure that sharp, and the bound already
answers the question the phase asked: which of the three nested columns is the
cost, and it is the arrays by an order of magnitude. The sharper instrument
stays unbuilt, with its real cost now named rather than left as "a scope
decision": a generator knob that declares `v_comp` as `text` makes the
generator write a declaration `pg_dump` would not — the opposite of what `M10`
corrected it to do — so it needs an explicit exemption from
`perf_generator_fidelity.rs`, not just a flag. What is fixed is the wording
that promised a share: 4.6.1's spec row in
[`../design/roadmap-phase4-composite-decoding.md`](../design/roadmap-phase4-composite-decoding.md)
now says the deliverable is whatever that instrument resolves, and the phase-7
inbox entry carries the exemption. **Generalized into
[`../design/roadmap.md`](../design/roadmap.md)'s standing rule** "A slice row
that commits to a measurement names its instrument", which now states that a
named instrument is satisfied by whatever it resolves, a bound included, and
that the instrument's floor is part of the deliverable — the row is unsatisfied
only if the instrument was not built or not run. That closes the opposite
failure to the one the rule was written for: an unattended session building an
unbudgeted second instrument to rescue a null reading.

*The census's 39% cost on brace-free rows was reviewed on 2026-08-27, and the
answer is to **make the pre-filter cheap, not to make it skippable**.* A knob
would re-introduce exactly what 4.5.1 deleted (`Builder::censusing`) and would
have to be plumbed to a caller who cannot know whether a later query will want
to retype an array column. But the 0.84 s per 3.00 GiB is a hand-rolled scalar
two-comparison byte loop at `map.rs:1269` running at ~3.8 GB/s, and `memchr`
is already a direct dependency (`pgdump_query/Cargo.toml:12`): `memchr2(b'{',
b'[', raw).is_some()` is a one-line, SIMD swap. Queued as out-of-band **M11**
below. The knob question is *deferred, not closed* — if the re-take does not
show the improvement, it reopens with a real number behind it instead of a
figure taken against a scalar loop. Nothing is reversed meanwhile: the census
stays unconditional, and its justification never rested on the pre-filter
being free.

*`M10`'s date/time fractions were reviewed on 2026-08-27: the choice is
**wrong in principle, and the correction rides the next re-take** rather than
buying one.* The list was chosen for the rendered shapes it produces, which is
a **correctness** goal met elsewhere — `fixtures/` already carries all five
(`00:00:00`, `.000001`, `.123456`, `.85312` for the trim, `.999999`). Against
real data the list is badly unrepresentative: 300,000 rows of koji's
`task.create_time` hold **zero** values with no fractional part and 9.9%
ending in a zero digit, against the list's 37.5% and 50%. But the stake is
below the instrument: three of sixteen columns, ~7 bytes shorter per
no-fraction render, is 0.2% of a 3,943-byte row, and the decoder costs about
the same either way — while changing it re-takes five figures. So
`FRACTIONS` in `scripts/generate_perf_data.py` switches to a uniform
microsecond draw as **`M12`**, landing ahead of `M13`'s re-take so the figures
are taken on the final bytes; the comment that currently justifies the list
records at that point that shape coverage belongs to the fixtures.

*The census figures' source-edit recipe was reviewed on 2026-08-27 and
**stands**, with the recipe made discoverable from the code.* Regenerating
either figure means putting a bare `return;` at the top of
`map::Builder::on_row` and rebuilding, which isolates exactly the census but
breaks silently if that function is renamed, split, or loses the pre-filter —
and nothing in the source points back at the doc. `on_row` gains a doc comment
naming the two [`../design/measurements.md`](../design/measurements.md)
sections taken by patching it, the way
[`../design/architecture.md`](../design/architecture.md)'s "`Event` is the
scanner's contract" records a constraint at the site that would break it. It
lands with `M11`, which edits that function anyway. Still not a cargo feature:
the existing *Rejected:* paragraph holds, and a feature is a shipped knob for
a measurement.

*The scan-throughput table's whole re-take was reviewed on 2026-08-27 and
**needed no ruling*** — re-taking two rows the maintainer had not queued was
forced by the floor row being page-cache-contaminated, and correct
re-measurement is not a decision to weigh in on. The practice it exemplifies
is now two standing rules in
[`../design/measurements.md`](../design/measurements.md). Its one residual —
that the section cites the `COPY` path's warm CPU (2.92 s), a
page-cache-warm filesystem read the new tmpfs rule now excludes — is covered
by **`M13`**, which re-takes the whole warm set on tmpfs in one session.

*The fidelity guard's `uv` skip was reviewed on 2026-08-27 and **reversed**.*
The skip guards an environment that does not exist: `mise.toml` pins exactly
one tool, `uv = "latest"`, and there is no CI, so the suite runs in one place
where `uv` is present — while covering the file's only test, so when it does
fire the suite goes green with its only generator guard entirely absent. Its
recorded reason, that such a checkout "should not report a failure it cannot
act on", is false here: the remedy is `mise install`, which is the point of
pinning tools with `mise` at all — a CI run is one step from having everything
the suite needs. `have_uv()`'s skip becomes an assertion naming that remedy,
riding with `M12`. Generalized into
[`../design/roadmap.md`](../design/roadmap.md)'s standing rules as "A test may
assume the tools `mise` pins", which also states the exception that gates the
other way: a machine-local resource `mise` cannot pin, per `CLAUDE.local.md`'s
koji-replica rule.


*4.6's generator-fidelity entry was reviewed on 2026-08-27, settled as
scheduled work with its scope corrected — the finding was three infidelities,
not one — and **landed the same day as `M10`**.*
`scripts/generate_perf_data.py` declared `time`/`timestamp`/`timestamptz`
where `pg_dump` writes the long spellings, never trimmed fractional seconds
where PostgreSQL does, and filled a `real` column with a float64 `repr()`.
The first two were **coupled** — correcting the spellings alone would have
sent three columns that never decoded through a decoder that re-renders them
differently from the file — and the third was what made `typed` and `strings`
disagree on this input (`v_bytea`, which looked like the culprit, is faithful;
`pg_dump` writes the doubled backslash too). The consequence was a validity
problem rather than a fidelity complaint: a benchmark for the typed path
measured 13 of 16 columns while its table said 16. The fix was not 4.6's — it
changes the default output's bytes and so re-takes the figures taken on them.
The findings and the coupling are in
[`history/2026-08-27.md`](history/2026-08-27.md), "The perf
generator is not the pg_dump shape it claims"; what landed is `M10` in
[`../design/roadmap.md`](../design/roadmap.md)'s out-of-band ledger.

*4.4.4's refusal order was reviewed on 2026-08-27 and **stands**, with its
recorded reason replaced.* It was queued as "kept as a counterfactual" — the
order is dead, since the opaque test matches a bare type name where the array
test matches that name with bounds appended, so no input reaches both. What
settles it is that **both refusals answer `Utf8View`**: the order can only ever
pick a *label*, never a type, so keeping it costs nothing and the hypothetical
defence it was written with ("what must win if the two ever overlap") is not
the argument. `resolve_array` and
[`../design/architecture.md`](../design/architecture.md), "Type resolution",
now say that instead, and both name the alternative — testing opaqueness
recursively through the element's own array levels, which would make an array
over a domain whose base is `box[]` answer `OpaqueElementType`. That is a
behaviour change buying a better diagnostic on a shape `pg_dump` cannot write
(I21), so it is not scheduled.

*M7's two entries were reviewed on 2026-08-27.* The **unconditional
absorption** **stands**, with its reason upgraded from "it would put an
unexplainable asymmetry between the `COPY` and `--inserts` paths" to a claim
about the only producer that can reach the shape: `pg_dump` cannot emit a
header-less comment block immediately before an `INSERT INTO`, but a
hand-written dump can, and there absorbing gives one `Data` span where gating
would give a `Framing` span plus a `Data` span — the same trade
`on_copy_start`'s `Mode::Comment` arm already makes. The symmetry is a
consequence of that call, not the argument for it; the durable half is in
[`../design/architecture.md`](../design/architecture.md), "Bulk regions: one
span kind, three payloads". The **corrected verification figure** needed no
ruling: 30/47, 30/47, 31/48 and 6/21 all reproduce on today's tree, so the
entry is simply closed. What the grilling turned up on the way is the
`--disable-triggers` gap below, and a **third** wrong reason recorded for M6's
asymmetry. Reasoning: [`history/2026-08-27.md`](history/2026-08-27.md).

*M6's boundary-signal asymmetry was reviewed on 2026-08-27 and **stands**, with
its recorded reason replaced.* The queued description of the
`TOC_PREFIX_STATS` drive-by was "one prefix plus the object-census kind". It
turned out to be two functions with **different** answers:
`parse_toc_header_line` reads all three of `_printTocEntry()`'s prefixes, while
the boundary signal `looks_like_toc_name_line` accepts `"Statistics for "` and
still refuses `"Data for "`. Grilled on 2026-08-27, and the first
reason recorded for it was wrong: it said a data entry heads a `COPY` block
that arrives as its own event, which is false under `--inserts`. The real
constraint is `Builder::on_copy_start` — it reads the pending `TocHeader` out
of its `Mode::Comment` arm, and its `Mode::Statement` arm pushes a separate
span and passes `None`, so accepting `"Data for "` here would make **every
`COPY` block in the file lose its TOC entry**. The asymmetry is forced, not
chosen. Pinned by `a_statistics_entry_is_one_attributed_span` and
`a_statistics_entry_parses_and_opens_a_span` in `map.rs`; the durable half is
in [`../design/architecture.md`](../design/architecture.md), "TOC enrichment".
What the grilling turned up on the way is the `--inserts` attribution gap,
closed the same day as **M7**. *M5's placement was reviewed the same day and
**stands**, also with a replaced reason*: not "the library error is shared"
(`require_enabled` has two call sites, both in `main.rs`) but that the remedy
interpolates the user's own `--source` path, which `Error::CacheDisabled` does
not have and should not take a `PathBuf` to get — unlike `Error::FieldDecode`,
which names a static flag string. Reasoning:
[`history/2026-08-27.md`](history/2026-08-27.md).

*9.5.1's three entries were reviewed on 2026-08-27.* The **prepass running
even when the cancel flag is already set** **stands**, now with numbers behind
"bounded by its own length": koji's preamble is 63,333 bytes of 784 GB, and the
most preamble-heavy shape available (4000 tables, 49% preamble by bytes) maps
in 0.04 s — both in [`../design/measurements.md`](../design/measurements.md),
"The preamble prepass is bounded by the schema". The **two
`MetadataNotScanned`s** are **both kept, and the record corrected**: grilling
found the entry's own claim false, since `stream::resolve_block` is private and
all three call sites read `metadata` after the mapping pass, so a carried
`ResumeToken` cannot reach it either — `Error::MetadataNotScanned` is
unreachable through every public entry point, while
`ColumnResolution::MetadataNotScanned` stays reachable because `resolve_columns`
is public. The guard is kept as what stands between a future reordering and a
wrongly-typed row, and is now pinned by a unit test in `stream.rs` rather than
left as untested defence. The **`pg_dumpall` fixture** entry is **reversed**:
`edge_cases/dumpall.sql` gains a second data-carrying database rather than
leaving the recurring boundary verified only against a hand-concatenated file
`pg_dump` never wrote — queued under "Not started". Reasoning:
[`history/2026-08-27.md`](history/2026-08-27.md).

*9.5's two-granularity guard entry was reviewed on 2026-08-27 and **stands**,
restated as a principle.* The amendment is right — chunk granularity alone
leaves a block-rich dump unresponsive for its whole scan — and the finding
underneath it was that a spec *enumerating* check points is what let the case
through. Both the spec and `architecture.md` now say the rule is "read the flag
at every point the loop can cheaply reach", with the two current sites as its
instances, and both record the two limits it does not remove: the flag is read
before `read_range`, not during it, and `scan::scan`/`scan_preamble` ignore it
on purpose, because a stop there could not be told from reaching the first
`COPY` header and would cache a truncated preamble as complete. The remote-I/O
half of that is filed into
[`../design/roadmap-phase6-inbox.md`](../design/roadmap-phase6-inbox.md).
Reasoning: [`history/2026-08-27.md`](history/2026-08-27.md).

*Three Phase 9 entries were reviewed on 2026-08-26.* The **save-throttle**
entry is **reversed**: the quadratic regime was measured, it costs 44s on a
4000-block dump, and closing it is slice **9.5** rather than a Phase 7
question. The **`parse` types every `\connect`ed database** entry is
**reversed in the direction of agreement**: the recomputation moves into
`map_forward` so a cold query and a warm one answer alike — an out-of-band
change, since the divergence it removes was never a decision anyone took. The
**coverage-line** entry **stands**: the text line prints unconditionally, since
its absence would leave a user unsure rather than reassured, and `--json`
carries the components without a rendered line, because a caller can divide.
Reasoning: [`history/2026-08-26.md`](history/2026-08-26.md).

*The three remaining Phase 9 entries were reviewed on 2026-08-26 and all three
**stand**.* The diagnostics recompute is 3 ms for koji's 833-span cache — a
full `info --dqcache` run, load and render included — so the O(spans) cost is
below process startup; the figure is in
[`../design/architecture.md`](../design/architecture.md), "The cache".
`info --dqcache none` stays an error, with the message to name `pgdq parse
--dqcache <path>` as its remedy the way `Error::FieldDecode` names
`--schema-mode strings` (out-of-band, riding with 9.5). And `v_empty_enum`
needed no ruling at all: fixture expansion is a standing rule, so additions
under it stop being logged here — flagging each one dilutes a section meant for
decisions.

*4.4.3's normalization-placement entry was reviewed on 2026-08-26 and is
**settled by refactor**.* Grilling established that both placements produce
identical outcomes on every input — normalization can only add an array level,
and no normalized terminal is ever the literal name `box` or a `TypeDef.name` —
so the choice was free and the real finding was that two predicates walking the
domain chain separately is what made placement a question. Slice **4.4.4** folds
the array arm into one function; the reasoning is in
[`history/2026-08-26.md`](history/2026-08-26.md), and the policy it was decided
under is `roadmap.md`, "Refactor when the shape stops fitting".

*4.4.2's `integer[][]` entry was reversed by 4.4.3* — the spelling resolves as
`integer[]` does again, and the refusal it was flagging now has exactly one
DDL shape behind it (I26).

*4.5.1's two entries were reviewed on 2026-08-26.* The
`MAX_ARRAY_DIMS` verdict **stands** — a run past `MAXDIM` is not evidence, so
the column keeps its optimistic type and the row surfaces as a `FieldDecode`;
the durable half is in [`../design/architecture.md`](../design/architecture.md),
"The array shape census", and the scan-time diagnostic it does *not* raise is
now a roadmap "Future" item. The spec/manual divergence is **resolved by
amending the spec**: its "What the manual must say" section now describes one
path, matching the census section the same reversal rewrote. Reasoning:
[`history/2026-08-26.md`](history/2026-08-26.md).
