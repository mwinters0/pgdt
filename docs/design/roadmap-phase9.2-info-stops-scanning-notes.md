# 9.2 — `info` stops scanning

The verb split, landed. `parse` is the only command that reads a dump for its
structure; `info` reports what the cache holds and errors when it holds
nothing.

## `CacheStatus` split four ways

`Absent` is gone, replaced by `Missing`, `Unreadable`, `UnsupportedVersion` and
`SourceChanged { cached_size, live_size }`. `Valid` also gained `total_size`,
so both usable variants carry it and a reporting caller never has to ask which
one it holds.

`CacheMode::load` still folds all four unusable outcomes into `Ok(None)` — that
is where the collapse belongs, for callers whose only response is to scan. What
changed is that the collapse is no longer the *only* thing on offer.

**`read_cache_file` is the new shared helper**: it does the read plus the
envelope check and returns `Result<Result<CacheFile, CacheStatus>>`, so `load`
and `load_offline` share three of the four unusable outcomes and `load` adds
the size check on top. `load_offline` cannot reach `SourceChanged` at all —
there is no live source — which is exactly what its `CacheOffline` diagnostic
already warns about.

## Diagnostics are now recomputed on load

`status_from_file` re-derives the tiling check and the TOC-coverage figure from
the spans it just read, on every successful load. This is not a new idea —
`diagnostic.rs` has always said they are recomputed rather than persisted — but
before this slice nothing recomputed them on the *load* path, because every
consumer had just scanned. `info` no longer scans, so the TOC-coverage figure
would simply have vanished from its output.

Two consequences worth knowing:

- `cache::save` → `cache::load` now round-trips `DumpIndex` **including**
  `diagnostics`, because `build_index` derives its own from the same spans by
  the same pure function. Three tests in `tests/cache.rs` used to clear
  `diagnostics` before saving to make an equality assertion work; they no
  longer need to, and the equality is a stronger statement now.
- `stream::map_file` filters the loaded diagnostics down to `CacheMtimeChanged`
  before recomputing (see 9.1's notes), or it would double them.

## `pgdq info`, as it now behaves

- Loads via `cache::load`/`CacheMode::load_offline` and matches on the status
  itself, rather than going through `CacheMode::load`.
- `unusable_cache_message` / `unusable_offline_cache_message` own the four
  sentences. Both take the whole `CacheStatus` and match exhaustively with
  `unreachable!` arms for the usable ones, so a new variant is a compile error
  rather than a plausible-looking wrong message.
- `--dqcache none` is rejected through the existing
  `CacheMode::require_enabled`, the same mechanism `parse` already used.
- Cache-only mode reports an `Incomplete` cache instead of refusing it. That
  refusal was the whole reason `--preamble-only` needed a special case in
  `info_offline`; both are gone.
- The mtime warning surfaces by pushing `Diagnostic::cache_mtime_changed()`
  onto the loaded index, which is why that constructor is now `pub` while its
  three siblings stay `pub(crate)`. A caller that matches on `CacheStatus`
  itself still needs the library's answer to "what severity is this".

## `--preamble-only` moved to `parse`

`index::preamble_only` is unchanged; only the verb moved. `parse
--preamble-only` prints the metadata block and writes the partial cache, and
`info` reads that cache like any other — which is what let `info_offline` drop
its `preamble_complete` special case.

`MetadataJson`/`print_metadata_json` were deleted with it. `parse` has no
`--json`, and `info --json` over a preamble-only cache is a superset of what
that shape carried.

## The CLI now has integration tests

`pgdump_query-cli/tests/partial_reporting.rs`, driving the real binary through
`env!("CARGO_BIN_EXE_pgdq")` — no new dependency beyond `tempfile` and `tokio`
as dev-dependencies. It is the only place that can assert an exit status or an
error message, which is most of what this slice changed.

Its partial caches are **built by hand** from a complete index (see 9.1's notes
on greedy spans and the clamp that is required). The spec sanctions this —
"asserted against a cache truncated at a known block boundary" — and it is what
makes the assertions deterministic; `pgdump_query/tests/map_file.rs` separately
covers that a real interruption leaves this shape.
