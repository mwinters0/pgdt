# Phase 2.1 notes — dollar-quote tracking in the scanner

Closes the Phase 1 known gap: a line inside a dollar-quoted function body that
both starts at column 0 and matches the full `COPY ... FROM stdin;` grammar no
longer gets mistaken for a real block.

## What landed

- `pgdump_query::copy::scan_dollar_quotes(line, tag) -> (tag, touched)` — a
  pure, per-line function that advances `$tag$ ... $tag$` tracking state.
  `tag` is the currently-open delimiter (`None` outside any dollar-quoted
  string); `touched` reports whether the line held any delimiter activity at
  all, which the caller uses to skip structural checks for that line
  regardless of whether the quote opened, closed, or stayed open across it.
  Handles same-line open+close, a tagged delimiter with an untagged pair
  inside it (and vice versa — only the *matching* tag closes), and rejects
  tag syntax PostgreSQL itself would reject (`$1$`, a positional-parameter
  reference, isn't a valid tag since identifiers can't start with a digit).
- `CopyScanner` gained a `dollar_tag: Option<Vec<u8>>` field, sibling to
  `state` rather than nested inside it (avoids a partial-move-through-`&mut
  self` fight with a non-`Copy` `Vec<u8>` payload). Consulted only in
  `State::Outside` — a dollar-quoted body can never appear inside COPY data
  (`postgres-invariants.md` I1: `CREATE TYPE`/`CREATE EXTENSION`, and by the
  same dependency-graph argument, function bodies, are always pre-data or
  otherwise outside a `COPY` block). `resume()` always starts it `None`: a
  resume point is a cached block's `data_offset`/`end_offset`, never inside a
  dollar-quoted string.
- Two adversarial functions added to both fixture sources:
  `pgdump_query/tests/data/edge_cases.sql` (hand-written, feeds the existing
  `edge_case_dump_event_stream` insta snapshot) and
  `scripts/fixture_schema_edge_cases.sql` (real `pg_dump`, feeds
  `fixtures/*/edge_cases/*.sql`) — `public.sample_fn()`, whose `$$`-quoted
  body contains a byte-for-byte valid `COPY public.widgets (id, name) FROM
  stdin;` header followed by rows and a bare `\.`, and `public.tagged_fn()`,
  whose body's own `$$` forces `pg_dump` to pick a tagged delimiter (`$_$`,
  confirmed empirically — see below) wrapping a body that itself contains an
  untagged `$$` pair.
- `scan_dollar_quotes` unit tests in `copy.rs` cover each transition directly
  (open/close across lines, tag mismatch not closing, same-line round trip,
  the `$1$` non-tag case) without needing a live scan.

## What generation confirmed

`appendStringLiteralDQ()` (`src/fe_utils/string_utils.c`) picks `$$` unless
the body text already contains it, in which case it appends `_`, `X`, `1`, …
suffixes until the candidate delimiter no longer collides — read from source,
then confirmed empirically: `pg_dump` on `sample_fn` (no `$$` in its body)
emits plain `$$`, and on `tagged_fn` (body contains `$$`) emits `$_$`, both on
`postgres:16-alpine`. `generated_fixtures_have_the_expected_structure` and
`data_only_dumps_carry_every_block` in `tests/scan.rs` already assert the
exact 6-block structure of `fixtures/*/edge_cases/default.sql` byte-for-byte
across all 3 versions — since neither function produces a `COPY` block, an
unfixed scanner would have shown up there as a 7th phantom block (or a
misaligned `public.widgets` block), and both tests already ran clean against
the real, regenerated fixtures with the fix in place, so no new fixture-level
assertion was needed beyond what already existed.

**Self-inflicted trap worth remembering**: an early draft of the adversarial
fixture put descriptive prose containing literal `$func$`/`$$` substrings in
a `--` comment *above* the functions. The scanner (correctly, per its own
heuristic) read those as real delimiter tokens and swallowed everything after
them, including the real data blocks — not a scanner bug, but a reminder that
this heuristic can't distinguish a comment from code and test fixtures need
to avoid dollar-sign pairs in their own prose.

## Not touched

`docs/design/pg-dump-compatibility.md`'s "`COPY ... FROM stdin;` at line
start inside a dollar-quoted body" row updated from "known gap" to closed.
No `postgres-invariants.md` entry needed — this is a scanner-robustness fix,
not a new dependency on `pg_dump` behavior; the existing I1/I3 entries already
cover the invariants it relies on (pre-data ordering, TOC comments as a
segmentation hint only).
