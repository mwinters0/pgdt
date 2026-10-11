# P35.5 notes — the facade retired, the graph a script

What 35.6 and the wrap inherit from retiring `pgdump_query`, moving every
member under `crates/` and turning the crate check into
`scripts/layering.py`. No body changed and no test moved but with its crate;
`CACHE_FORMAT_VERSION` stands.

## How it fell

- **The workspace's members are `crates/*`**, and nothing else: `pgdt`,
  `datafusion-pgdump`, `datafusion-cli-pgdump`, `namespace-init` and
  `peak-rss` moved whole beside the six libraries, `vendor/xz-seek` staying
  where `vendor_xz_seek.py` puts it.
- **Each embedder depends on the libraries it names**, by crate: `pgdt` and
  `datafusion-pgdump` on all six, `datafusion-cli-pgdump` on all but
  `pgdt-io`. A `pgdump_query::<name>` path became the defining crate's API
  path; `pgdt-query` left the two embedders' dev-dependencies for their
  dependencies. `introspect` forwards to `pgdt-query/introspect`, which turns
  on the core's, the model's and io's as the facade's did, and `http` to
  `pgdt-io/http`.
- **A path out of a moved crate gained one `../`**: fixtures, scripts,
  `CONTRIBUTING.md`, and the `#[path]` by which `pgdt`'s and
  `datafusion-pgdump`'s tests include `pgdt-query`'s `tests/common/refused.rs`.
  The help-text snapshots' `source:` headers name `crates/pgdt/tests/`.
- **`layering.py` reads `cargo metadata --no-deps --offline`** and the
  members' sources through `file_limits.masked`; its rules are its module
  docstring. Its `ThisRepo` test runs the real check, so every `mise run
  check`, `--affected` included, runs it through the scripts' `unittest`
  rather than a cargo target. Its two tables are D68's:
  - `GRAPH` holds every member's dependencies, as the spec's "The crate
    graph" says, the apparatus with rows of their own. Beyond what Cargo's
    cycle refusal already gives, that refuses an unlisted member and any
    edge to `namespace-init` but `pgdt`'s and `datafusion-cli-pgdump`'s.
  - `NAMESPACES` lists each library's API namespaces, and any other `pub
    mod` but `internal`, or a `pub mod` inside a namespace, is refused —
    the spec's "every library module private", without which one `pub mod`
    line would reach past `internal` unrefused.
  - Dev-dependencies are not read, as the Rust test did not read them.
- **`repoint.py`'s meter reads a stamp's own member roots** (`members_at`):
  `CODE_ROOTS` is `crates` alone, and a file git does not pair with its old
  path is a deletion at the stamp's root, which `crates` alone would leave
  uncounted.
- **The graft index was rebuilt**: `graft build`, the cards for the old
  top-level directories gone. It is local and ignored.

## What 35.6 inherits

- **`file_limits.py` names four files past a hard limit**, all libraries':
  `stream.rs` and `gather.rs` in `pgdt-query`, `decode.rs` in `pgdt-codec`
  and `io.rs` in `pgdt-io`. A cut inside a crate leaves `layering.py` alone;
  a new namespace, or a namespace's child made public, does not.
- **A comment-only change to a library runs no test under `--affected`**:
  the layering binary read `crates/` whole, and no test target does now
  (`test_check`'s `RealWorkspace`).

## Negative results

- **`vendor/xz-seek/VENDORED_FROM` still names `pgdump_query`**: the copy is
  never patched in place, and `vendor_xz_seek.py`'s template already says
  "the workspace", so the next sync rewrites it.
- **No cache byte moved**: nothing persisted changed.
