# Phase 3.3 — the TOC enrichment layer: how it landed

Companion to [`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md)'s
"What a span carries" and "Scanning: one statement-driven pass, TOC comments
as an enrichment layer". Per `CLAUDE.md`, consolidated into the phase-level
notes doc (and removed) once all of Phase 3 lands.

## What landed

`Span` grows `toc: Option<TocHeader>` (`map.rs`), filled whenever the comment
block preceding a span's classification contains a line matching
`_printTocEntry()`'s header grammar (I3/I18): `-- Name: <name>; Type: <kind>;
Schema: <schema>; Owner: <owner>[; Tablespace: <tablespace>]`, or its `--
Data for Name: ...` sibling ahead of a `COPY` block. `parse_toc_header_line`
does the parsing — a fixed sequence of `split_once` calls on the literal
separators, not a general grammar (I18's own scope limit: `sanitize_line`
never escapes a literal `; Type: ` etc. that might occur inside an object's
own name). A field that fails to parse just leaves `toc: None`, the same
graceful-degradation case header-less input already produces.

`Owner`/`Schema` collapse `-` (and, for `Owner`, also empty — I18's two
"no value" shapes) to `None`. `--verbose`'s `-- TOC entry N (class C OID O)`
/ `-- Dependencies: ...` lines, which can precede the Name line, are ordinary
comment lines to this parser — they don't match, contribute nothing, and
don't stop the Name line after them from parsing.

`DumpIndex::diagnostics` gains a `TocCoverage { headers, spans }` entry,
always `Info`, computed by `index::toc_coverage_diagnostic` as
`spans.iter().filter(|s| s.toc.is_some()).count()` against `spans.len()` —
produced by `build_index` and `stream.rs`'s mapping pass, the same two call
sites `tiling_diagnostics` already had. `scan_preamble`/`preamble_only` don't
get one, matching the precedent `tiling_diagnostics` already set (neither
computes it either).

Cache format v5→v6 (`Span::toc` is persisted, like `Span::text`).

## A pre-existing gap this slice's own testing surfaced and fixed

The design doc's "COPY blocks are the one exception" section already claimed
a `Data` span's outer boundary starts at its TOC comment. That claim was
**not actually true before this slice**: `looks_like_toc_name_line` only
matches `-- Name: `, never `-- Data for Name: `, so a `-- Data for Name: ...`
block's `saw_name` was always `false` — and the boundary-close logic treated
*any* non-`--` line following a `saw_name == false` comment (including a
blank one) as the trigger to close it immediately as its own
`Framing`/`Unparsed` span. Since `_printTocEntry()` always writes a blank
line after the closing `--` (`ahprintf(AH, "--\n\n")`) regardless of what
follows, this fired on *every* real `COPY` block, not just an edge case —
the comment closed as its own span before `on_copy_start` ever saw it, and
`on_copy_start`'s `Mode::Comment` absorption arm was consequently dead code
for real fixtures. Invisible until now because nothing compared a `Data`
span's start offset against its comment's: `check_tiling` doesn't care which
span owns which bytes, and Phase 3.2's own tests never asserted more than
"two adjacent spans, no gap."

Fixed in `Builder::step`'s `Mode::Comment` arm: a **blank** line no longer
forces the close-or-transition decision — it's absorbed, leaving the block
open until either a real (non-blank) line disambiguates it as DDL, or
`on_copy_start` fires directly (never routed through `feed_line` at all,
since `CopyScanner` intercepts the header line as `Event::CopyStart`). This
is what makes the new `toc`-on-`Data`-spans tests possible, and it also
merges what used to be a spurious `Framing` span plus the `Data` span into
one `Data` span for *every* commented `COPY` block, real fixtures included.

**This forced a second fix**, found by the query-vs-`build_index` parity
test (`a_query_built_index_tiles_in_every_cache_state`): `scan_preamble`
(`index.rs`) deliberately never feeds the first `COPY` block's `CopyStart`
to its `Builder` — it stops at `header_offset` and calls `finish` there, on
purpose, so it never opens a `Data` span it can't close (see that function's
own comment). Before this slice that was harmless, because the preceding
comment had *already* closed into its own span via the (now-fixed) premature
blank-line close. With the fix, the comment instead stays pending right up
to the stop point — and `finish`'s `flush_pending` would guess it closed as
`Framing`/`Unparsed`, exactly the wrong answer now that a later, unfed-
truncated scan is the one that can classify it correctly as part of the
`Data` span.

Fixed with `Builder::pending_comment_start()` (`Some(start)` iff `mode` is
`Comment`) and `flush_pending`/`finish` taking `end`: `scan_preamble`
retreats its stop point to the comment's own start when one is pending
(`end = spans.pending_comment_start().unwrap_or(start.header_offset)`), and
`flush_pending` now **discards** a pending comment whose `start >= end`
instead of pushing a zero-length guessed span — the comment's bytes are left
for whichever scan resumes from that retreated point to absorb correctly.
Verified by re-running the pre-fix regression
(`preamble_only_persists_a_real_unscanned_tail`, which caught the zero-length
span directly) and the full fixture suite.

Neither fix is TOC-enrichment-specific — both are boundary/state-machine
corrections that this slice's new `toc`-on-`Data` test happened to be the
first thing precise enough to catch. `build_index_spans_match_build_map_exactly`
and `every_fixture_tiles_exactly` (Phase 3.2's own tests, unchanged) still
pass; they just didn't have `Span::toc` (or a span-start assertion) available
to notice the pre-existing split.

## What the next slice inherits

- `Span::toc` is populated the same way regardless of span kind — `Table`,
  `TypeDef`, `Extension`, `Data`, and `Unparsed` can all carry one; `Connect`,
  `VersionHeader`, and generic `Framing` never do (nothing precedes them with
  a TOC comment). Phase 3.4's cross-reference set (roles, tablespaces) reads
  `toc.owner`/`toc.tablespace` off whichever spans carry them, plus
  `OWNER TO`/`GRANT`/`SET default_tablespace` statement text for the sources
  the TOC comment alone doesn't cover (see the design doc's "What a span
  carries").
- **Grouping remains unimplemented and unassigned.** Neither 3.3's nor 3.4's
  spec row lists it (`map.rs`'s own module doc previously said "3.3/3.4",
  corrected during this slice to say it has no assigned slice yet) — an
  object's trailing `ALTER ... OWNER TO` still tiles as its own adjacent
  `Unparsed` span. If a later phase wants it, the TOC's `Dependencies:` field
  (only captured today under `--verbose`, and not parsed by anything) is the
  documented mechanism.
- `TocHeader` doesn't carry the verbose `-- TOC entry N (class C OID O)` /
  `-- Dependencies: ...` lines' fields (dump ID, catalog OID, dependency
  list) — out of scope per the spec's slice row (owner, kind label,
  `Tablespace:`, TOC-coverage only), and no fixture need has surfaced for
  them since. They remain ordinary, unparsed comment lines to this module.
- I18 (`postgres-invariants.md`) is the new register entry for the header
  line's exact field grammar — read it before changing
  `parse_toc_header_line`'s split order, and its scope limit before trusting
  the `Tablespace:`/`TOC_PREFIX_STATS` paths against real data (no fixture
  exercises either).
