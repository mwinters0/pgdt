# Phase 3.2 — the span model: how it landed

Companion to [`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md)'s
slice table. That doc states the design; this one records how slice 3.2
landed in code and the decisions made while doing so. Per `CLAUDE.md`, this
file is consolidated into a single phase-level notes doc (and removed) once
all of Phase 3 lands.

## What landed

- **`crate::map`** (new, L1): [`Span`]/[`SpanBody`], a statement-driven
  boundary/classification pass (`build_map`), and [`check_tiling`] — the
  runtime-checkable tiling invariant, with a test running it over all 96
  real `pg_dump`-generated fixture files (six routine versions × three
  schemas × their flag sets) plus the hand-written adversarial
  `tests/data/edge_cases.sql`. Every one tiles exactly.
- **Cache identity checking** (`crate::cache`): every cache now records the
  source's size and mtime at save time; loading re-observes the live source
  and compares. A size mismatch invalidates the cache (treated the same as
  absent/foreign/wrong-version — the module's existing best-effort
  philosophy, not a new hard-error path); an mtime mismatch is surfaced on
  `CacheStatus::Valid` but does not invalidate. `ByteRangeSource` gained
  `modified()` to support this. Cache format version bumped 1 → 2 (free
  pre-1.0).
- **The hardened statement accumulator** (`crate::preamble::statement_complete`):
  now tracks double-quoted identifiers (`""` doubling) and `--` line
  comments, closing the gap `docs/status/history/2026-08-23.md` flagged —
  an apostrophe inside either used to open a string that never closed. Also
  exposed as `crate::preamble::in_open_quote` (a partial-scan query used by
  `crate::map`'s boundary-reassertion check, see below) and
  `crate::preamble::classify_statement` (the three-shape classifier
  `PreambleBuilder::dispatch` and `crate::map` both now share).

## What this slice does not do, and which slice does it

3.2 was originally specified to carry the integration work as well. It was
mis-sized — a self-contained new module and a rework of the tested core query
path are two review cycles — and split; see
[`../status/history/2026-08-23.md`](../status/history/2026-08-23.md), "Slice
3.2 was mis-sized". What is outside this slice, and where it went:

- **`crate::map` is not wired into `DumpIndex`/`crate::cache`/`crate::stream`
  — slice 3.2.1.** `build_map` is a standalone function that always scans to
  EOF, so today a caller wanting both an index and a map pays **two full
  passes**; `build_index` does not produce spans. `DumpIndex::blocks` is
  still the primary structure rather than a derived filter over `spans`, and
  `SpanBody::Unscanned` is exercised only by `check_tiling`'s own unit tests
  (`an_unscanned_tail_tiles_cleanly`), never by a real incremental scan. The
  work `stream.rs` needs is a segment planner that reads spans instead of
  `blocks`/`blocks_for`.
- **Span text storage, the cache's 64KB-per-span cap, and the file-level
  `Diagnostic` channel — slice 3.2.2.** `Span` carries offsets only. Nothing
  in a production path calls `check_tiling`, because there is no diagnostic
  channel for it to report through yet; the invariant is currently enforced by
  `tests/map.rs` alone.
- **A dedicated `Data`-span fast path for `INSERT` runs and the large-object
  region — slice 3.6.** The generic statement-grammar fallback already tiles
  both shapes correctly, including the embedded-raw-newline case
  (`fixtures/*/edge_cases/inserts.sql`'s `public.escapes` row 10), because
  `statement_complete` re-scans its whole accumulated buffer (quotes and
  parens included) on every appended line regardless of how many physical
  lines a value spans. It just produces one `Unparsed` span per `INSERT`
  statement (or per large-object `lowrite` call) instead of one `Data` span
  per whole run — `every_fixture_tiles_exactly` and
  `inserts_dump_with_an_embedded_newline_value_still_tiles` (`tests/map.rs`)
  are the evidence. So a koji-scale `--inserts` dump is correct under this
  slice, just not walked at the cost the design promises, and the map's shape
  over a bulk region is not yet the one the design specifies.
- **TOC field enrichment** (owner, kind label, `Tablespace:`, TOC-coverage
  reporting, grouping via `Dependencies:`) — slices 3.3/3.4, as originally
  specified. `crate::map` reads a TOC comment block only as a lexical
  *boundary* signal (does it contain a `-- Name: ...; Type: ...` line at
  all), never parsing or storing its fields.

## Span boundaries: TOC-block-anchored, not pure statement completion

The design doc's own wording — "a span opens at the `--` of its TOC
comment... or, where there is no TOC comment, at the first byte of its
first statement" — is what this slice implements literally, and it turns
out to be load-bearing, not just a convenient description: a `CREATE
FUNCTION`/`PROCEDURE` body is dollar-quoted, and `crate::scan::CopyScanner`
never emits an `Event::Line` for a dollar-quoted line, including the one
that closes the tag with the statement's own terminating `;` on it
(`docs/status/history/2026-08-23.md`, "Dollar-quoted lines never reach
`Event::Line`"). A boundary detector built purely on tracking when the
*current* statement's own `;` arrives can never observe that such a
statement closed — nothing in the visible line stream says so, ever.

Recognizing the *next* TOC comment block instead sidesteps this
completely: every real `pg_dump` entry, `CREATE FUNCTION` included, carries
its own `-- Name: ...; Type: ...` header, and that header can never appear
inside a dollar-quoted body (the scanner already filters those lines out
before any event reaches `crate::map`, fake-looking `-- Name:` text
included). So "the next entry's header arrived" is a sound, general "the
previous span has ended" signal regardless of whether the previous
statement's own completion was ever observable.

**This still leaves a gap for content with no TOC comment at all**
(`tests/data/edge_cases.sql`'s two hand-written `CREATE FUNCTION`s have
none) **or where a dollar-quoted statement's gap is followed directly by
another TOC-comment-less span-worthy line** — the statement-grammar
fallback's own buffer just keeps absorbing every line, TOC comments
included, until *something* supplies a `;` outside a string/paren/comment.
Caught empirically while writing `tests/map.rs`: `sample_fn` and
`tagged_fn` in `edge_cases.sql` (adjacent, no TOC comments, separated only
by a plain `--` comment with no `Name:` line) initially merged into one
`Unparsed` span 691 bytes wide instead of two, because the statement
accumulator doesn't distinguish "an unrelated new comment block just
started" from "more of my own statement." Fixed by a boundary-reassertion
check in `Mode::Statement`'s line handler: a `--`-prefixed line always
closes the current statement and reopens a fresh `Comment`/`Framing`
span — *unless* `crate::preamble::in_open_quote` says the accumulated
buffer is currently inside an open string or double-quoted identifier, in
which case the `--`-looking line is genuinely string content (a value
spanning physical lines) and must not be mistaken for a boundary.
`pg_dump` never emits an inline `--` comment inside any of this module's
recognized statement shapes, so the check is unambiguous for every other
case — confirmed by re-running `every_fixture_tiles_exactly` after the fix
with no regressions.

**The reassertion check is not a general fix, and slice 3.2.3 is.** It only
fires on a `--`-prefixed line, so a header-less file whose dollar-quoted body
is followed by an ordinary statement still collapses: measured at one
`Unparsed` span for a whole 206-byte file
([`../status/history/2026-08-23.md`](../status/history/2026-08-23.md), "A
dollar-quoted body with no TOC comment"). 3.2.3 closes it properly, with a
position-only `scan.rs` event marking where a dollar-quoted region ended. No
real `pg_dump` output is affected — every entry carries a TOC header, and
`--no-comments` does not remove them.

## Framing recognition beyond `\connect`/`\restrict`

The design's "Framing spans" section names the `SET`/`set_config` block
`_doSetFixedOutputState()` writes as framing. `crate::map::classify`
recognizes any complete statement starting `SET ` or `SELECT
pg_catalog.set_config(` (case-insensitive) as `SpanBody::Framing`
regardless of where it appears — including a `SET default_tablespace =
...;` that `_selectTablespace()` emits ahead of an individual object's
definition (the `Tablespace:` cross-reference, not read until Phase 3.4).
Since this slice does no grouping, that `SET` still tiles as its own
span adjacent to the object it precedes either way; treating it uniformly
as framing was simpler than trying to distinguish the two producers by
position.

## Verification

`tests/map.rs`'s `every_fixture_tiles_exactly` walks all 96 files under
`fixtures/` (13–18 × `edge_cases`/`objects`/`types` × their flag sets) —
including every degenerate shape the design doc calls out by name:
`data-only`, `schema-only`, `inserts`/`column-inserts` (zero `COPY`
blocks), and `dumpall.sql` (concatenated, multi-`\connect`) — plus targeted
tests for the `objects` fixture's dollar-quoted `FUNCTION`/`AGGREGATE`/
`EVENT TRIGGER` bodies and large objects, classification fidelity for
`CREATE TABLE`/`TYPE`/`EXTENSION`, and the "no grouping" adjacent-span
shape for a trailing `ALTER ... OWNER TO`. `src/map.rs`'s own `#[cfg(test)]`
module exercises the `Builder` state machine directly against small
synthetic inputs (no file I/O), including the dollar-quote-gap case
`docs/status/history/2026-08-23.md` first identified as needing evidence.

No koji-scale performance verification was run this slice — the design's
own "Verification" section belongs to slice 3.6, which lands the `Data`-span
fast path for bulk regions. This slice's cost profile is unchanged from Phase
1/2 (the statement-driven pass walks the same DDL-sized preamble either way;
it doesn't yet walk `INSERT`/large-object bulk regions any differently than
ordinary DDL).
