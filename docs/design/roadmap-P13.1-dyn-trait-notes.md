# P13.1 — Vendor `xz-seek`, and rework `ByteRangeSource` to dyn-compatible signatures

## What landed

**`ByteRangeSource` is dyn-compatible (D3).** All three required methods now
return `Pin<Box<dyn Future<Output = …> + Send + '_>>` instead of `impl Future`
(RPITIT). `LocalFileSource`'s three methods each wrap their previous `async
fn` body in `Box::pin(async move { … })`; nothing about the body itself
changed.

**Every caller that was generic over `S: ByteRangeSource` now takes `&dyn
ByteRangeSource` (or `&'a dyn ByteRangeSource` where a lifetime was already
named) instead of a type parameter.** No caller needed `Arc<dyn
ByteRangeSource>` — every existing call site already held a plain reference,
never an owned handle crossing a spawn boundary — so D3's `Arc<dyn …>`
mention is unexercised by this slice; it stays available for whichever of
P13's own later slices or P14 first needs to own a source rather than borrow
one.

Converted: `scan` (`scan.rs`), `read_table` (`batch.rs`), `build_index`,
`scan_preamble`, `preamble_only` (`index.rs`), `map_forward`,
`SaveThrottle::save`, `map_file`, `table_stream` (`stream.rs`), `attach_text`,
`build_map` (`map.rs`), and `SourceIdentity::observe`, `cache::load`,
`cache::save`, `CacheMode::load`, `CacheMode::save` (`cache.rs`).
`table_stream`'s signature lost its `<'a, S>` generic entirely — it is now
`table_stream<'a>(source: &'a dyn ByteRangeSource, …)`.

Every call site passing `&source` where `source: LocalFileSource` (the CLI,
the library's own internal calls, every integration test) coerces to `&dyn
ByteRangeSource` for free via unsized coercion — none of them needed an
explicit `as &dyn ByteRangeSource`. The three test-only `ByteRangeSource`
implementors (`FailsPast`, `CancelsPast` in `tests/map_file.rs`,
`CountingSource` in `tests/query_cache.rs`) needed their `impl` blocks
rewritten to the boxed-future shape; two of the three (`CancelsPast`,
`CountingSource`) forward directly to an inner `LocalFileSource`'s
already-boxed future rather than re-boxing, since the inner call's return
type already matches the trait's.

**`xz-seek` is now a path dependency of `pgdump_query` itself** (D9's "This
phase is what wires it in"), `xz-seek = { path = "../vendor/xz-seek" }`,
taking the vendored manifest's own default features (`liblzma`,
`fast-checks`) per D9's "The build takes `liblzma`". Nothing in
`pgdump_query`'s source uses the crate yet — that starts at 13.2/13.3.
`cargo build --workspace` pulls in `liblzma-sys` and links against the
system's `liblzma` without any extra setup on this machine (its `pkg-config`
discovery just worked), which is a fact worth having in hand before 13.3
writes `XzSource` against it: nothing here had to touch `mise.toml` or
document a new build dependency.

## What this changes for measurement

D3's own text already named this: touching `io.rs`, `scan.rs`, `batch.rs`,
`stream.rs`, `map.rs`, `index.rs` and `cache.rs` reddens every
`measure.py`-registered figure that times a scan or a query, with no
mechanical oracle to clear it (`--verify-additive` settles generator changes
only). Those figures were already stale before this slice, from the
buffer-pool work `STATUS.md` already records — this slice adds no newly-stale
figure that wasn't stale already, and no number in `measurements.md` is
touched or contradicted by it. No sweep is owed for landing this slice, per
the standing rule that a stale figure does not oblige one.

## What 13.2 inherits

- The trait shape 13.2's `stored_size()`/`SourceIdentity`-opacity work lands
  on top of is exactly what's here now — `size()` still means "the exact
  addressable length" and nothing about `SourceIdentity` or `total_size`
  changed in this slice.
- The three read-loop call sites that announce `hint_read_size` before a loop
  starts (`scan`, `map_forward`, `table_stream`) are unchanged in behavior;
  only their parameter's type changed.
- `xz-seek`'s crate is now resolvable from `pgdump_query`'s own manifest, so
  13.2/13.3 can `use xz_seek::…` directly without any further Cargo wiring.
