# P35.4 notes — the six crates carved behind the facade

What the next slices inherit from carving `pgdump_query` into `crates/pgdt-*`.
The graph is [`decisions.md`](decisions.md), "D68", held by
`pgdump_query/tests/layering.rs`; each crate's root says what it holds, and
its `internal` module what a crate above reaches past its API. No body
changed: a file moved whole, its `crate::` paths rewritten to the crate now
holding what they name, and a visibility widened only where a crate above
needs it.

## How the carve fell

- **A crate whose whole content was one module is that module at its root**
  where the module was public (`pgdt-model` is `model.rs`, its submodules
  beside it, re-exported by name rather than by glob so an item made public
  for `internal` stays off the root), and keeps it as a private module where
  it was private (`pgdt-io` holds `io.rs`, its root re-exporting what the
  facade's root did).
- **What a crate above reaches that is not API is `pub` and re-exported from
  `internal`**, and the importer names `internal`. Two gates sit inside it:
  `#[cfg(feature = "introspect")]` on the hooks only that build compiles, and
  `#[cfg(any(test, feature = "test-support"))]` on what only a crate above's
  tests reach — `pgdt-codec`'s `scalar_tier` and `scalar_read`, and
  `pgdt-dump`'s cache encoding; `ValueKey::compare` carries the same gate
  where it is defined. `pgdt-query` turns both crates' `test-support` on in
  its dev-dependencies.
- **A method or a field cannot go in `internal`**: one a crate above calls is
  `pub` where it is defined, and an item made `pub` inside a `pub` module is
  reachable by that module's path too. 35.10 closes both
  ([`roadmap-P35-seams.md`](roadmap-P35-seams.md), "What this phase does not
  do").
- **`timed!` and `row_evaluated!` are `#[macro_export]`ed** and re-exported
  from `pgdt-core::internal`. Their `cfg(feature = "introspect")` is read in
  the expanding crate; every crate compiling such a gate — `pgdt-model`,
  `pgdt-io`, `pgdt-query` — has an `introspect` turning on `pgdt-core`'s, and
  the facade's turns on all four.
- **The codec names `arrow-schema` and `arrow-buffer`**, the second for
  `i256` alone; its tests and bench take `arrow` as a dev-dependency.
- **`leader::scan_region` and `leader::Shortfall` are imported by name** in
  `stream.rs` rather than through a `leader` module path, the module being
  private to `pgdt-dump`.

## Where the tests went

- **Every integration test is `pgdt-query`'s**, with `tests/common/`,
  `tests/data/` and the snapshots: each one maps or reads through `map_file`,
  `build_index` or `table_stream`, query's, and `common` renders through
  `render_field`. `tests/layering.rs` stays in the facade: it exercises no
  crate's API, and 35.5 makes it a script. No test reading DDL is the codec's,
  so `pgdt-codec` dev-depends on nothing in `pgdt-dump`.
- **A test in a lower crate cannot call a higher one**: a dev-dependency on a
  crate that depends on the crate under test links a second copy of it, whose
  types are not the test's. So `io`'s `the_library_defaults_to_serial`, which
  reads both option structs, moved whole to `pgdt-query`'s `batch/tests.rs`,
  and the cache's two save-encoding tests, which build their index with
  `map_file`, moved whole into `pgdt-query`'s `tests/cache.rs` as its
  `encoding` module, reaching the encoding through `pgdt-dump`'s
  `test-support`. `leader.rs`'s tests read the hand-written dump from
  `pgdt-query`'s `tests/data/` by relative path.
- **The benches and the example went to the lowest crate they use**:
  `decoders` to `pgdt-codec`, `whole_file` to `pgdt-query`, `xz_decode` to
  `pgdt-io`, and `measure.py` and `release.py` build them there.

## What 35.9 inherits

- **Every figure's `depends` names the new paths**, so `measure.py --stale`
  reads a moved file as changed against the commit a figure was taken at. The
  release profiles and `pgdt/build.rs` did not move; the legs build as 35.8
  left them.

## What 35.5 inherits

- **The facade re-exports exactly what its root and its public modules did**,
  by name, from the crate now defining each; nothing but the embedders and
  `tests/layering.rs` builds against it.
- **What still names `pgdump_query`**: `pgdt`'s, `datafusion-pgdump`'s and
  `datafusion-cli-pgdump`'s `use`s and manifests; `pgdump_query::…` paths in
  prose across `docs/`, `scripts/` and comments, which resolve through the
  facade or name a module now in a crate; README's and CONTRIBUTING's opening
  lines; `CLAUDE.md`'s commands; the graft index. Every file path moved here
  is already repointed.
- **`pgdt` and `datafusion-pgdump` dev-depend on `pgdt-query`** for
  `tests/common/refused.rs`, which their tests include by path from
  `pgdt-query`'s `tests/common/`.
- **A doc link from a lower crate to a higher one does not resolve**
  (`pgdt-model`'s to `pgdt_dump::cache::Unusable`, say): rustdoc warns, and
  no check runs rustdoc.
- **The check 35.5 writes sees `internal` paths and the two macros**, and
  so holds only once 35.10 has left no other route to the methods and
  `pub`-module items the previous section names.

## Negative results

- **No cache byte moved**: bincode encodes no path, and
  `persisted_index_is_pinned_to_the_format_version` passes at the version it
  had.
- **`arrow::datatypes` is not `arrow-schema`**: it re-exports
  `arrow-buffer`'s `i256` and interval types too, so the codec reached
  `arrow-buffer` before the carve, by D74's reading of that path.
