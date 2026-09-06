# P13.4 — Recognition (D8) and the non-seekable diagnostic (D2)

## What landed

**`pgdump_query::open_local(path) -> Result<Arc<dyn ByteRangeSource>>`**, in
`io.rs` (D8). It reads the first six bytes of `path` and compares them against
`.xz`'s magic (`\xfd7zXZ\x00`); a match opens an `XzSource`, anything else
(including a file shorter than six bytes, answered `false` rather than an
error) opens a `LocalFileSource`. Content-sniffed, not name-sniffed — a real
`.xz` file is recognised whatever it's called, and a file merely *named*
`.xz` with plain content opens plain rather than failing.

**Exported at the crate root, not through a public `io` module.** The spec's
own sketch named `pgdump_query::io::open_local`, flagged "to be settled at
implementation." `mod io` was already private with `ByteRangeSource`,
`LocalFileSource` and `XzSource` re-exported through `pub use io::{...}` at
the crate root — `open_local` joins that same list rather than making `io`
public, so the module's privacy boundary is unchanged and the new function is
reached the same way its neighbors already are:
`pgdump_query::open_local(path)`.

**The CLI's three call sites** (`parse`, `info`, `query` in
`pgdump_query-cli/src/main.rs`) now call `open_local(&file)?` instead of
`LocalFileSource::open(&file)?`, and pass `source.as_ref()` wherever a
`&dyn ByteRangeSource` is wanted — `source` is now `Arc<dyn ByteRangeSource>`,
so trait methods (`.size()`, `.seek_table()`, …) resolve straight off it via
`Deref`, and `LocalFileSource` dropped out of the CLI's import list entirely
(it's still used directly by `pgdump_query-cli/tests/partial_reporting.rs` and
`pgdump_query/tests/cache.rs`, which build sources for test setup rather than
going through the CLI's recognition path).

**`DiagnosticKind::NonSeekableCompressedSource { block_count: usize }`** (D2),
in `diagnostic.rs`, `Severity::Warning` — usable as-is, matching
`CacheMtimeChanged`'s and `CacheOffline`'s severity. Its constructor is
`pub(crate)`; nothing outside the crate pushes one.

**One shared decision function, not three separate ones.** `index.rs` gains
`non_seekable_compression_diagnostic(table: Option<&xz_seek::SeekTable>) ->
Option<Diagnostic>` — `None` for no compression layer or an already-seekable
table, `Some` naming `table.block_count()` otherwise (0 or 1, since
`is_seekable()` is exactly `block_count() > 1`). Three call sites read a table
from three different places and hand it to this one function, so "does this
table warrant the warning" is answered identically regardless of which asked:

- `build_index` — `source.seek_table()`, after a full scan (`parse` without
  `--preamble-only`).
- `preamble_only` — the same, after a (possibly cache-short-circuited)
  preamble scan, guarded by `!base_index.diagnostics.contains(&d)` so a
  warning already inherited from a loaded cache (see below) is not pushed a
  second time. This one is idempotent rather than gated on `!known`
  (`known` = the preamble was already complete in the cache): a cache that
  exists but is *not yet* `known`-complete still carries the warning from its
  own prior `status_from_file` load, and gating on `!known` would have missed
  that case and duplicated it.
- `cache.rs`'s `status_from_file` — `file.compression` as
  `Option<&xz_seek::SeekTable>` (via `Option::map` over the
  `CompressionIndex::Xz` variant), recomputed on every load exactly the way
  `tiling_diagnostics`/`toc_coverage_diagnostic` already are, since
  `DumpIndex::diagnostics` is `#[serde(skip)]`.

**`pgdq query` still never surfaces `DumpIndex::diagnostics`** — that was
already true of `TilingBroken`/`CacheMtimeChanged`/`TocCoverage`/`CacheOffline`
before this slice, so a non-seekable warning during a `query` run is silent in
exactly the way every other file-level diagnostic already is. Not a gap this
slice opened or is scoped to close.

**The CLI's message** (`diagnostic_message` in `main.rs`) names both remedies
D2's spec paragraph named: `` `xz -T0` `` or `` `--block-size=<size>` ``.

## What was decided along the way

**`SeekTable::blocks_in(range)` is not used, deliberately, even though it now
exists upstream.** 13.3's notes flagged this as open: the vendored snapshot
(`54c7983`) predates it, and the choice was "re-sync first, if it's landed" or
"name the block count some other way." It has landed upstream — as part of
`xz-seek`'s own P3 (parallel block decode), now visibly in progress there
(`3.1`–`3.3` committed). That is exactly the situation D9's vendoring
timing was chosen to avoid re-syncing into: the vendored copy was frozen
specifically to stay stable *before* P3 started moving the source underneath
it, and P3 has now started. Re-syncing here would pull in-flight parallel-
decode work for the sake of one method whose absence costs nothing this
slice needs — `blocks_in`'s whole value is a bounded warning scoped to a
query's touched byte range, and D2 never asked for that; a file-wide warning
naming `SeekTable::block_count()` — always 0 or 1 once `is_seekable()` is
false — says everything the spec's own wording asks for. So this stays on
`block_count()`, and the vendor re-sync is left for whenever P10 or another
consumer actually needs post-`54c7983` upstream work.

**A latent defect in `tests/cache.rs`'s own `xz_compress` helper surfaced and
was fixed in this slice, not deferred.** Its docstring claimed
`--block-size=65536` "forc[es] several blocks," but `edge_cases.sql` is 2,352
bytes — under any block size at or above that, `xz` never splits, so every
caller of that helper (including the pre-existing differential test,
`xz_source_produces_the_same_index_and_cache_as_the_plain_file`) was silently
exercising the *non-seekable* shape while labeled "the seekable shape." This
was invisible before this slice: with no diagnostic distinguishing the two
shapes, a non-seekable table and a seekable one produced identical
`DumpIndex`es. D2's diagnostic gave the "seekable" claim something to
disagree with, and the pre-existing differential test failed the moment this
slice's `build_index` wiring landed. Fixed by lowering the block size to 512
(confirmed with `xz --list -v` to yield 5 blocks on this exact fixture) and
adding an explicit `is_seekable()` assertion at each of that helper's three
call sites, so a future fixture change that breaks the assumption fails loudly
at the assertion rather than silently testing the wrong shape again.

## What was deliberately left for 13.5

- **No committed `.xz` fixtures.** This slice's own tests (in `io.rs` and
  `tests/cache.rs`) build throwaway `.xz` payloads with `xz` shelled out, the
  same pattern 13.3 used. The phase's two `edge_cases.sql`-derived committed
  fixtures (seekable multi-block, non-seekable single-block) and the
  CLI-level differential parity against them are 13.5's.
- **No end-to-end `pgdq parse`/`query`/`info --source foo.dump.xz` test.**
  This slice's tests exercise `open_local`, the diagnostic's three producers,
  and the CLI wiring compiles and passes the existing (non-xz) CLI test
  suites unchanged — but nothing here runs the actual `pgdq` binary against a
  `.xz` file end to end. That is explicitly 13.5's differential parity work.
- **`cache::save`'s persisted `compression` field is still write-then-
  recompute, never read to skip a footer re-walk.** `XzSource::open` always
  walks the file's stream footers; nothing constructs one from a cached
  `SeekTable` via `xz_seek::Builder::open_with_table`. `open_local` decides
  *which* source to build, not how cheaply — that optimization was already
  named as 13.4/P14's territory in 13.3's notes and stays unclaimed here too.

## Testing

**`io.rs`'s own test module** gains `open_local`'s three shapes: xz content
under a non-`.xz` name (recognised as `XzSource` via `seek_table().is_some()`,
since dot-call downcasting isn't needed to tell the two apart), a `.xz`-named
file holding plain content (opened as `LocalFileSource`, content wins over
name), and a file shorter than the six-byte magic (opened as plain, no
error).

**`tests/cache.rs`** gains three integration tests wiring the diagnostic
through `build_index`, a cache round trip, and `preamble_only`'s two call
shapes (fresh scan, and a second call that finds the preamble already
complete) — the idempotence claim above is what the second half of that last
test pins.

`cargo test --workspace`, `cargo clippy --workspace --all-targets` and
`cargo fmt --check` all pass with no new warnings.

## What this changes for measurement

Touches `io.rs`, `index.rs`, `cache.rs`, `diagnostic.rs` and the CLI's
`main.rs` — all already declared paths for every figure `measure.py --stale`
reports red since 13.1. No newly-stale figure, and no number in
`measurements.md` is touched or contradicted. No sweep is owed, per the
standing rule that a stale figure does not oblige one.

## What 13.5 inherits

- `open_local` is a working, tested recognition convenience over both
  sources — 13.5's end-to-end tests can call it directly against the two
  committed fixtures rather than constructing `XzSource`/`LocalFileSource` by
  hand.
- `DiagnosticKind::NonSeekableCompressedSource` exists and is wired through
  every path that can produce a `DumpIndex` from a `.xz` source
  (`build_index`, `preamble_only`, a cache load) — 13.5's non-seekable
  fixture is what first exercises this through the actual `pgdq` CLI rather
  than through library-level calls, and its differential-parity tests can
  assert the diagnostic's text appears in `pgdq parse --preamble-only` and
  `pgdq info` output for that fixture.
- `tests/cache.rs`'s `xz_compress` helper is now confirmed (by assertion, not
  just by comment) to actually produce a multi-block file for
  `edge_cases.sql` — 13.5's own fixture generation should size its
  `--block-size` the same deliberate way rather than picking a round number
  and trusting it.
