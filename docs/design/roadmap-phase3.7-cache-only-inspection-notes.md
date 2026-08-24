# Phase 3.7 — Cache-only inspection: how it landed

Companion to [`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md)'s
"Cache-only inspection". That doc states the design; this one records what
slice 3.7 produced and what later slices/phases inherit. Per `CLAUDE.md`,
consolidated into the phase-level notes doc (and removed) once all of Phase
3 lands.

## What landed

- **`CacheMode` grows `Offline(PathBuf)`** (`cache.rs`) — never constructed
  by `CacheMode::resolve`; the CLI builds it directly when `pgdq info` gets
  no `--source`. `load`, `save` and `require_enabled` all reject it with a
  new `Error::CacheModeMismatch(&'static str)`, and the new
  `CacheMode::load_offline` rejects `Enabled`/`Disabled` the other way — the
  design's "the library enforces the split, not just the CLI." `read_table`
  (`batch.rs`) rejects it too, explicitly, ahead of `table_stream`'s own
  `cache.load` call (which would also reject it, but a caller should not
  have to trace through a generator to learn that `query` never accepts a
  cache-only mode).
- **`CacheStatus` grows `Incomplete { index, mtime_changed, total_size }`**
  — the former out-of-band item M1. `cache::load` and the new
  `cache::load_offline` share one helper, `status_from_file`, that compares
  `index.scanned_through` against the cache's own recorded
  `SourceIdentity::size` (`total_size`): short of it is `Incomplete`, at or
  past it is `Valid`. `load_offline` has no live source to stat, so it
  always passes `mtime_changed: false` into that helper — the "this is
  unverified" fact cache-only mode carries travels through the diagnostic
  below instead, not this flag.
- **`CacheMode::load_offline`** — the cache-only counterpart to
  `CacheMode::load`, returning the full `CacheStatus` (not the
  `Option<DumpIndex>` `load` collapses to) because a cache-only caller has
  to tell `Valid` apart from `Incomplete` to decide whether it has enough.
  Every successful load — `Valid` or `Incomplete` — gets a new
  `DiagnosticKind::CacheOffline` (`Severity::Warning`) pushed onto it
  unconditionally: there is no live file to compare against, so the result
  is unverified and historical regardless of completeness.
- **`preamble_only`'s return type widened** to
  `(DumpMetadata, Vec<Diagnostic>)` — it was the one library entry point
  answering with `DumpMetadata` alone rather than a whole `DumpIndex`, so
  its diagnostics (a `CacheMtimeChanged` warning, in the live case) had
  nowhere to travel back to the caller. The CLI now prints them the same way
  `print_index` prints `DumpIndex::diagnostics`.
- **`pgdq info` prints `diagnostics:`** — the former out-of-band item M2.
  Folded into `print_index` (default and `--map`) and called explicitly
  after `print_metadata` for both `--preamble-only` paths (live and
  cache-only), since preamble-only mode never builds a `DumpIndex` to hang
  the printing off of. Cache-only mode's "unverified, historical" banner
  rides this path for free, exactly as the design says.
- **CLI flags restructured**, project-wide: `parse`, `info` and `query` all
  drop their `file` positional for `--source <path>`; the cache path becomes
  `--dqcache <path>` (was `--cache-path`) on all three; `query`'s `table`
  positional becomes `--table`. `info`'s `--source` is `Option<PathBuf>`;
  clap's `required_unless_present = "source"` makes `--dqcache` required
  exactly when `--source` is absent, which is the cache-only trigger the
  design calls for — no separate flag, no ambiguity. `parse` and `query`
  keep `source` as a plain required `PathBuf`.
- **`pgdq info`'s live full-scan fallback now checks completeness, not just
  presence**, closing the M1 bug: a loaded cache is trusted for the default
  listing only when `index.scanned_through >= source.size()`; anything
  short of that (a preamble-only cache, or a query-built partial one) falls
  back to a fresh `build_index`, the same as no cache at all. This reads the
  live source's size directly rather than going through `CacheStatus`,
  which is a live source stat `pgdq info` already has to make regardless —
  see "A judgment call: `CacheMode::load` does not fold `Incomplete` into
  `None`" below for why this check isn't `CacheMode::load`'s own job.
- **`info_offline`** (`main.rs`) is the cache-only handler: `Absent` errors
  ("no usable cache found"); `Incomplete` errors for the default/`--map`
  listing (naming `scanned_through`/`total_size`) but succeeds for
  `--preamble-only` when the index's own `preamble_complete` flag is set —
  a preamble-only cache is exactly the shape that flag exists to recognize,
  and is *always* `Incomplete` in the whole-file sense, so gating on it
  separately is load-bearing, not redundant; `Valid` always succeeds.

## A judgment call: `CacheMode::load` does not fold `Incomplete` into `None`

The design's "live mode still treats it like Absent (falls back to a scan)"
reads as a blanket rule for every live consumer of a `CacheStatus`. Taken
literally at `CacheMode::load` — the `Option<DumpIndex>`-returning wrapper
`crate::stream::table_stream` and `crate::index::preamble_only` both call —
it would fold `Incomplete` into `None` there too.

That breaks both of those callers. `table_stream`'s incremental map is
*designed* to leave a partial cache behind (`scanned_through` short of the
file's size is the normal, expected shape of a cold query that stopped once
its target settled — "Mapping and streaming are separate passes"). If
`CacheMode::load` returned `None` for that cache on the next query,
`map_forward` would start over from byte 0 instead of resuming from the
frontier, silently discarding the entire benefit of the structural cache for
every ordinary query — not a fix for the M1 bug, which was specifically
about `pgdq info`'s default listing reporting a partial map as if it were
the whole file.

So `CacheMode::load` treats `Incomplete` exactly like `Valid` (`Some`, not
`None`), and the one caller that actually needs the "is this the *whole*
file" distinction — `pgdq info`'s default/`--map` fallback — makes that
check itself, against the live source's size it already has in hand rather
than through `CacheMode::load`. `CacheStatus::Incomplete` itself is still
real and still exposed (via `cache::load`, and load-bearing for
`load_offline`, which has no live size to fall back on) — this is about
which of `CacheStatus`'s two "found something" cases `CacheMode::load`'s
own `Option` collapsing conflates, not about removing the case.

Filed as a "Decisions worth another look" entry in `STATUS.md` — this is a
deviation from a literal reading of one sentence in the phase spec, made
without the maintainer present, to avoid what would otherwise be a silent
query-performance regression.

## What later slices/phases inherit

**`Span::text` being `None` for every `Data` span (Phase 3.2.2) is why
`query` stays live-only.** Cache-only mode covers `info` alone, by design —
row bytes are never in the cache regardless of this slice, so there is
nothing `query` could ever answer from a cache alone. Any future phase
revisiting that boundary needs to know it isn't this slice's boundary to
begin with.

**The completeness check (`scanned_through` vs. the cache's own recorded
size) is now a real, reusable primitive** (`CacheStatus::Incomplete`,
`cache::load`/`load_offline`'s shared `status_from_file`), not just CLI
plumbing — a future embedder asking "does this cache already cover what I
need" has a typed answer to match on instead of re-deriving it from
`scanned_through` and a live stat by hand.

**`roadmap.md`'s out-of-band ledger's M1/M2 rows are already marked folded
into this slice** (done at spec time, not here) — no further edit needed
there when Phase 3 wraps.
