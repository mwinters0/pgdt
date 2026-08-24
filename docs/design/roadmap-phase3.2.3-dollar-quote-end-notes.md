# Phase 3.2.3 — the dollar-quote-end event: how it landed

Companion to [`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md)'s
"Span boundaries: object-anchored and greedy". Per `CLAUDE.md`, consolidated
into the phase-level notes doc (and removed) once all of Phase 3 lands.

## What landed

`scan::Event::DollarQuoteEnd { offset }` — one `u64`, nothing else. Emitted
where a dollar-quoted region closes; `map::Builder::on_dollar_quote_end`
completes whatever statement is in flight when it arrives.

**The detection is "touched and now closed", not a tag transition.**
`copy::scan_dollar_quotes` already returns `(new_tag, touched)`, so the
condition is `touched && tag.is_none()`. Testing "had a tag, now doesn't"
would miss a region that opens *and* closes on one line (`AS $$ SELECT 1 $$;`),
which is a shape `pg_dump` emits — there is a scan test for exactly that.

**The offset is just past the closing line**, i.e. where a following span
would start. That matters because `Builder` never computes a span's own end;
it only decides where the next one begins, so the natural watermark is the one
the fixup already uses. `on_dollar_quote_end` in fact ignores its argument
today — the span's end comes from the next `push_span` — but the event carries
it because a caller that wants to slice or seek needs the position, and adding
it later would be a signature change for no reason.

**Exhaustive matching found every driver.** Adding the variant broke
compilation in four places (`scan::scan`'s callers in `map`, `index` twice,
`stream` twice), which is the intended property of not having a `_` arm.
`stream.rs`'s *replay* segment ignores it explicitly, paired with
`Event::Line`: a replay covers exactly one `COPY` block, and nothing outside a
block can fall inside one.

## What it changes, and what it doesn't

**Real `pg_dump` output is unaffected** — every entry has a `--` header, which
already reasserted a boundary. Every fixture's map is byte-identical before
and after, and `build_index_spans_match_build_map_exactly` still holds.

**Header-less input stops collapsing.** The shape measured in
[`../status/history/2026-08-23.md`](../status/history/2026-08-23.md) — two
dollar-quoted functions followed by a table, no TOC comments, previously
**one** `Unparsed` span covering all of it — now tiles as one span per object,
with the trailing `CREATE TABLE` recognized as a `Table` again rather than
swallowed. That is `a_header_less_dump_degrades_to_one_span_per_object` in
`tests/map.rs`, and it is the claim the phase's "Scanning" decision rests on:
rejecting TOC-driven segmentation is only defensible if the statement fallback
degrades to *coarser*, not to useless.

**A producer that puts the `;` on a later line** than the closing tag leaves
that line as its own small span. Coarser, still tiling — the same trade the
rest of the fallback makes, and noted on `on_dollar_quote_end`.

## What the next slice inherits

- The standing rejection of "an event variant surfacing dollar-quoted lines"
  is **unchanged**: this event carries a position and no text, so L1's event
  contract still says nothing about DDL. Span text comes from
  `map::attach_text` slicing the file by offset (3.2.2), which is what makes
  that rejection cost nothing.
- `Builder` now has three ways a statement can close — `statement_complete`, a
  `--` line reasserting a boundary, and this event. Anything adding a fourth
  should check `on_dollar_quote_end`'s `Mode::Statement`-only guard: it is a
  no-op in `Idle` and in `Comment`, deliberately, since a dollar quote cannot
  legally occur in either.
