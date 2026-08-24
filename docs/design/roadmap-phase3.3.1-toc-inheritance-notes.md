# Phase 3.3.1 — TOC inheritance for follow-on statements: how it landed

Companion to [`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md)'s
"Span boundaries: statement-anchored, object-attributed, greedy" and "TOC
coverage is recorded per file". That doc states the design; this one records
what slice 3.3.1 produced and what later slices inherit from it. Per
`CLAUDE.md`, consolidated into the phase-level notes doc (and removed) once
all of Phase 3 lands.

## What landed

- **`Span` grows `toc_owned: bool`** (`map.rs`) — `true` iff this span's own
  preceding comment carried the TOC header text `toc` came from, `false` if
  `toc` was inherited from an earlier entry's span (always `false` when `toc`
  is `None`). This is the "separate record of whether it carried the header
  text" the design calls for. Cache format v8→v9.
- **`Builder` grows `governing_toc: Option<TocHeader>`** — the entry a
  follow-on statement with no comment of its own now inherits, instead of
  carrying `None`. Updated by every `push_span` call, keyed on the pushed
  span's `body`: a plain statement or `Data` span becomes the new governing
  entry (whether its own `toc` was freshly parsed or itself inherited —
  either way it's what the *next* follow-on should carry); `Framing`,
  `Connect` and `VersionHeader` clear it to `None`, per the design's "is
  cleared by any span that is not a plain statement."
- **The one place inheritance has to be vetoed *after* it was seeded**:
  `push_statement_span` classifies `buf` before deciding the final span, and
  a mid-file `SET default_tablespace = ...;`/`SET ...;` statement
  (`looks_like_framing_statement`) classifies as `Framing` — which must never
  carry an inherited `toc`, even though the `Mode::Idle`→`Statement`
  transition that opened it had no way to know that yet. `push_statement_span`
  overrides `(toc, toc_owned)` to `(None, false)` whenever `classify(buf)`
  comes back `Framing`. Nothing else needed this override: every other
  `push_span` call site either never inherits (Connect, backslash-`Framing`,
  a non-entry `Mode::Comment` close) or passes `toc.is_some()` directly
  (`Data` spans, whose `toc` always comes from their own preceding comment,
  never from `governing_toc`).
- **`toc_coverage_diagnostic` needed no code change at all.** Its numerator
  was already `spans.iter().filter(|s| s.toc.is_some()).count()` — once
  `Span::toc` reflects attribution (inheritance included), that expression
  *is* "attributed spans," which is exactly what the design asked the
  numerator to become. Renamed `headers`→`attributed` in
  `DiagnosticKind::TocCoverage` and `Diagnostic::toc_coverage` for clarity
  only; the computation is unchanged.
- **`pgdq info`'s `object kinds:` breakdown switched to `toc_owned` spans**,
  in the same slice, per the design's explicit requirement — counting every
  attributed span would double-count any object with a follow-on statement.
  Verified manually against `fixtures/16/objects/default.sql`: `TABLE: 8`
  (unchanged from before this slice), not 8-plus-however-many trailing
  `OWNER TO` statements.
- **Five new unit tests** in `map.rs`'s own test module cover the mechanism
  directly (a follow-on inherits; several consecutive follow-ons all inherit
  the same header; a mid-file framing statement neither inherits nor lets
  inheritance cross it; `\connect` clears it; an uncommented `COPY` block
  resets it too). One integration test
  (`tests/map.rs::alter_owner_to_is_its_own_adjacent_unparsed_span`) and two
  existing TOC-coverage tests were extended rather than replaced, since the
  behavior they exercise is still correct — just more of it now.

## What later slices inherit

**`docs/manual/dump-inspection.md` needed no change.** Its `object kinds`
description ("a count of every kind of object the dump defines... read
straight from `pg_dump`'s own per-object comments") already describes an
object census, which is exactly what `toc_owned`-filtered counting still
produces — the manual was never wrong, only the *implementation* (before this
slice, inheritance didn't exist, so `toc.is_some()` and `toc_owned` were the
same set of spans by construction). Checked, not assumed: read in full before
concluding this.

**`Span::toc` now means "the TOC entry this span belongs to," not "the TOC
comment this span starts with."** Any future code reading `Span::toc` to mean
the latter (e.g., "does this span's own preceding bytes contain a `--
Name:` line") must check `toc_owned` instead, or it will get a false
positive for every inherited follow-on. `crate::preamble::dump_metadata_from_spans`
and `crate::index::build_index`/`scan_preamble` don't read `Span::toc` for
anything but the coverage figure and the cross-reference extraction already
covered above, so nothing else needed auditing.

**A live segment's `Builder` does not carry `governing_toc` across instances**
— `crate::stream::table_stream`'s mapping pass always resumes exactly at a
`Data` span's own boundary (the frontier only advances at `CopyEnd`), and
real `pg_dump` output never follows a `TABLE DATA`/large-object entry with a
comment-less statement, so nothing observable depends on this. Noted in
`Builder::governing_toc`'s own doc comment rather than engineered around, the
same way `Builder::with_database` already doesn't carry other transient
per-segment state forward.

**I2's `-- load via partition root <root>` marker line still isn't itself a
TOC entry** and doesn't interact with `governing_toc` — it's recognized in
`feed_line` ahead of `step`'s own dispatch, unaffected by this slice.
