# P36.6 notes — `CacheMode::open`

The open protocol is `CacheMode::open`, in
`crates/pgdt-dump/src/cache/open.rs`. It reads the cache once, opens the
source on the compression layer the cache settles, takes the `SourceWatch`,
and judges the cache against the live source. It answers an `Opened`, which
holds the source, the watch and a `Held`. `pgdt info` and `PgDump::open`
open through it. `pgdt parse` and `pgdt query` do not yet: the slice split.
`pgdt parse` moves in 36.6.1, `pgdt query` in 36.8.

## The shape

- **`Held` has four cases: `Loaded`, `Missing`, `Disabled` and
  `Replacing(Unusable)`.** An unusable cache is the open's refusal, built by
  `CacheMode::refusal`, so it is word for word a scan's. A mode that may
  overwrite the cache answers `Replacing` instead, with its source opened
  claiming nothing. `Disabled` stays apart from `Missing`, for D22's
  reason.
- **`CacheStatus` is `Missing`, `Unusable` or `Loaded(LoadedCache)`.**
  Completeness is `LoadedCache::is_complete`. `CacheLoad` is unchanged.
- **Judging a load is one method, `CacheMode::judged`**: the strict-identity
  refusal, or the advisory diagnostics. `CacheMode::load` and the open both
  call it. `strict_identity_refusal` was folded into it, and
  `advisory_identity_diagnostics` left the API. The three cache-warning
  constructors it calls in `pgdt-model` moved to `DiagnosticExt`, because
  `pgdt info` was what kept them public.
- **`settled_by` and `against_source` in `cache/store.rs`** are what
  `claim`, `load` and the open all use. They are the pre-source stored-size
  check against the origin's probe, and the live check. So the early verdict
  and the late one are still one rule.
- **A file that moves between the probe and the open** reaches
  `against_source` as `Unusable`. A mode that may replace the cache then
  opens the source and the watch again, claiming nothing: the first source
  was built on a seek table that belonged to another file.
- **`OfflineCache`** is the cache read with no source, and its own type.
  `CacheMode::Offline` and `Error::CacheModeMismatch` are gone, along with
  `read_table`'s runtime check for it and two tests that exercised the
  misuse.
- **`CacheMode::resolve` takes every path as given.** `pgdt`'s `none` is
  `args::cache_mode`'s to parse. The provider no longer special-cases it:
  `cache_path: Some("none")` is now a cache at the path `none`. The only
  way to reach that is from Rust, since no SQL option sets it.
- **`CacheMode::path`** answers the path, or `None` for a disabled mode. It
  replaced three hand matches.
- **The free `load`, `save` and `load_offline` are `internal`'s**, behind
  `test-support`, for tests that write a cache by hand. Crates above
  `pgdt-dump` that cannot name `internal` save through `CacheMode::save`
  with a watch: `pgdt-fixtures`, and `pgdt`'s and `datafusion-pgdump`'s
  tests. They read back through `CacheMode::load` or `OfflineCache`.

## `info` opens with its in-flight check off

`StrictIdentity::between_runs` is the selection with its in-flight term
turned off. `info` opens under it because it reads nothing of the dump past
the open. Under the plain selection, `SourceWatch::open` would refuse a
remote dump whose server offers neither validator. `info` has never refused
one (it opened no watch), and `crates/pgdt/tests/remote.rs` now pins that.
36.6.3 replaces it: the refusal moves to where a read begins, and
`between_runs` goes (history, 2026-10-17, "36.6's calls reviewed").

## Negative results

- **`pgdt parse` and `pgdt query` opening through `CacheMode::open` in this
  slice** was refused. A scan entry point loads the cache itself, so it
  would be decoded twice. The index the open decoded would also have to be
  dropped inside a statistics scope (as `claim` does), or the instrument's
  statistics reading would never come down. The entry points would also
  open a second watch, and the strict-identity refusal would move ahead of
  the arrangement's announcement on stderr. So the two commands keep
  `claim` until their entry points take the open: 36.6.1 for `pgdt parse`,
  36.8 for `pgdt query`.
- **`Held` as an `Option<LoadedCache>`** was refused. It folds three
  reasons for having no index into one, which is D22's rejected collapse.

## What the next round inherits

- **36.6.1: the mapping pass takes the open.** `map_file` and
  `preamble_only` read the cache the open decoded, and `pgdt parse` opens
  through it. Tests that build their own sources need a way to open over a
  source already in hand.
- **36.8 moves the rest.** `pgdt-query`'s stream entry points load the
  cache and open their own watch (`stream/mapping.rs`, `stream.rs`,
  `stream/partitions.rs`); 36.8 replaces them with the `Dump`, so `pgdt
  query` opens through `CacheMode::open` there, and `claim`,
  `CacheMode::load`, `CacheLoad` and `pgdt/src/source.rs`'s `open_for_scan`
  go once it does.
- **36.6.2: `scan` and `preamble_only` take only the fields they read.**
  `scan` announces `ScanOptions::cancel` to the source, and `map_file`'s
  preamble prepass relies on that announcement to drop a read in flight
  (D26). So narrowing `scan` moves the announcement to the prepass's
  callers. `ChunkedScanner::new` takes the narrowed options too.
