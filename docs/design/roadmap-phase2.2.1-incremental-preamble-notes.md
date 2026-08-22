# Phase 2.2.1 notes — incremental scans capture the first database's preamble

A small follow-up to 2.2, landed once real usage exposed a gap in it: a cache
file's mere presence didn't mean `DumpIndex::metadata` was populated, even
after an incremental (`pgdq query`/`table_stream`) scan ran all the way to
EOF. 2.2 wired preamble capture into `build_index` (the full, eager scan
behind `pgdq parse`/`pgdq info`) only — `table_stream`'s own live-scan loop
fed every `Event::Line` into a no-op arm regardless of how far it walked. Not
a numbered gap in the original phase doc; numbered here because it changes
`table_stream`'s behavior and cache contents, and needs its own record for
the same reason any slice does.

## What landed

- **`crate::index::scan_preamble`** (new, `pub(crate)`): scans from byte 0
  until the file's first `COPY` block header, or EOF if none exists, feeding
  every `Line` to a `PreambleBuilder` exactly as `build_index` does but
  stopping there instead of continuing. Per I1
  (`docs/design/postgres-invariants.md`), that one offset always closes out
  the *first* database's preamble — regardless of how many `\connect`-ed
  databases precede it — so a single bounded scan is enough, and its cost is
  independent of how many gigabytes of `COPY` data follow. Returns the
  recovered `DumpMetadata` plus that offset, a safe watermark for whatever
  scans the file next.
- **`table_stream` runs it once, up front, whenever caching is enabled and
  the first database's `preamble_complete` isn't already known** — before
  building its `Segment` list, so the segment that would otherwise start at
  `scanned_through == 0` now starts at the preamble's end instead, with no
  change to the segment-building logic itself. Persisted immediately via
  `cache.save`, not deferred to whatever a later segment saves, so the
  guarantee holds even for a caller that polls the stream once and drops it
  — see `interrupted_scan_still_captures_the_first_database_preamble` in
  `tests/query_cache.rs`. Skipped entirely under `CacheMode::Disabled`: that
  mode promises pure streaming with no side effects, and metadata with
  nowhere to persist would just be wasted I/O.
- **Scope is deliberately narrow: the *first* database only.** A later
  `\connect`-ed database's preamble in a multi-database dump is still only
  ever populated by `build_index`'s full scan — closing that gap wasn't
  needed to fix the actual problem (every fixture and koji are
  single-database) and is left for Phase 2.3 if it turns out to matter.
  `DatabaseMetadata::preamble_complete`'s doc comment (`preamble.rs`) says so
  directly: a caller walking `DumpIndex::metadata` still needs to check it
  per-database rather than assume a cache file implies the whole list is
  complete.
- **`preamble_complete`'s own doc comment updated** to stop promising it's
  "always `true` for a full-scan entry ... once an incremental query can
  also produce metadata (Phase 2.3)" — that composition now exists, ahead of
  2.3, for the first database specifically.
- No cache format change. `DumpIndex::metadata` and `preamble_complete` were
  both already part of the format since 2.2; this only changes which code
  paths populate them and when.

## Why not fold this into 2.3

2.3 is type *resolution* — `ResolvedSchema`, diagnostics, the Arrow mapping
consuming `DumpMetadata` for the first time. This fix doesn't touch any of
that; it only makes sure the metadata 2.2 already defined actually shows up
in a cache after the CLI's other everyday command (`pgdq query`), not just
after `pgdq parse`/`pgdq info`. Waiting on 2.3 to fix a correctness gap in
2.2's own contract ("what does a cache file guarantee") would have left that
gap open for an arbitrary amount of time for no real reason.

## Relationship to 2.3's `pgdq info --preamble-only`

`scan_preamble` is also what makes that planned flag (see the phase doc's
"CLI" section) cheap to build when 2.3 lands: the bounded-scan primitive and
the proof that it's independent of dump size already exist and are already
exercised in production code (`table_stream`), not just designed.
