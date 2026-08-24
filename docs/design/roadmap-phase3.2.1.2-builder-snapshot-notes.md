# Phase 3.2.1.2 — `map::Builder::snapshot`: how it landed

Companion to [`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md)'s
slice table. That doc states the design; this one records how slice 3.2.1.2
landed in code and the decisions made while doing so. Per `CLAUDE.md`, this
file is consolidated into a single phase-level notes doc (and removed) once
all of Phase 3 lands.

## What landed

- **`Builder::push_span` now fixes up the previous span's `end` at push
  time.** The tiling invariant makes a span's true end exactly the next
  span's start, so the moment a new span is pushed, the one before it can be
  closed immediately rather than waiting for a single end-of-scan fixup pass.
  `finish` is simplified to match: it only ever has to close the one span
  still open when the scan ends, not walk the whole list.
- **`Builder::snapshot(&self, end: u64) -> Vec<Span>`.** Non-consuming: it
  clones `self.spans` and stamps the last one's `end` with the caller-given
  watermark, exactly what `finish` does except without taking ownership. Only
  sound to call where `mode` is `Idle` — the doc comment says so — because
  otherwise the last-pushed span isn't the one actually open at `end`.
  `on_copy_end` always leaves `mode` `Idle` (`on_copy_start` holds it there
  for the block's whole duration), so right after a `CopyEnd` is exactly such
  a boundary — the one `stream.rs`'s live segment needs, since `Recorder`
  persists after every completed block, not just once at the true end of a
  scan.
- **Tests drive `Builder` directly** through `feed_line`/`on_copy_start`/
  `on_copy_end` (not just `feed_line`, which the existing `spans_of` helper
  was limited to), calling `snapshot` after each closing event and asserting
  `check_tiling` holds for the prefix scanned so far, then again after
  `finish`. A second test confirms `snapshot` and `finish` agree bit-for-bit
  when called at the same watermark with nothing left to feed.
- **`#[allow(dead_code)]` on `snapshot`, with a doc-comment pointer to
  3.2.1.2.1.** No production caller yet — see "What this slice does not do".

## What this slice does not do, and which slice does it

`stream.rs` is untouched here; `snapshot` gets its first caller in
**3.2.1.2.1**, which separates map-building from row emission outright
(`roadmap-phase3-object-inventory.md`, "Mapping and streaming are separate
passes"). What that slice needs from this module beyond `snapshot` is a
*seeded* constructor: a start offset, so a segment beginning partway through
the file opens its first span at its own first byte rather than at the first
non-blank line after it, and an in-scope database, since `Builder::new()`'s
`database: None` is wrong for a segment starting inside an already-`\connect`ed
database.

`snapshot` as landed is unconditionally correct at any `Idle` boundary and
needs no change for that caller.
