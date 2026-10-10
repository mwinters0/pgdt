# P35.10 notes — every forced-public item behind `internal`

What 35.5 inherits from closing the two routes 35.4 left past each crate's
`internal` module ([`roadmap-P35.4-carve-notes.md`](roadmap-P35.4-carve-notes.md),
"How the carve fell"): a `pub` item in a `pub` module, and a `pub` method a
crate above calls. The rule is [`roadmap-P35-seams.md`](roadmap-P35-seams.md),
"What this phase does not do"; each crate's `lib.rs` is where it lands. No
body changed and no test moved.

## How the boundary fell

- **API is what was `pub` before the carve; `internal` is what the carve
  widened.** The 150 items `baee2c41` turned from `pub(crate)` to `pub`, and
  the cache encoding it made public for tests, are the forced-public set;
  every item public before it is its crate's API. So a module
  made private took its API to the crate root where no namespace holds it:
  `copy`'s to `pgdt-core`'s, `nested`'s to `pgdt-codec`'s, and `scan`'s
  `DollarQuoteEnd` to `pgdt-dump`'s, which an `Event` variant carries.
  35.10.1 re-sorted the line by purpose
  ([`roadmap-P35.10.1-api-sort-notes.md`](roadmap-P35.10.1-api-sort-notes.md)).
- **Four namespaces, the ones an embedder names**: `pgdt_dump::cache`,
  `pgdt_codec::decode`, `pgdt_codec::pgtype` and `pgdt_core::instrument`.
  `resolve` was named once, by `pgdt`, for items all at the codec's root, so
  `pgdt` names the root instead and the module is private. How each
  namespace's file is laid out is 35.10.2's
  ([`roadmap-P35.10.2-namespaces-notes.md`](roadmap-P35.10.2-namespaces-notes.md)).
- **One extension trait per type, `<Type>Ext`**, the methods' bodies in an
  `impl` beside the type's inherent one: ten in `pgdt-model` — `ArrayShape`,
  `Unrepresentable`, `BlockCensus`, `DatabaseMetadata`, `TableDef`,
  `Diagnostic`, `FieldRefusal`, `IgnoredRefusals`, `BlockStatistics`,
  `ColumnStatistics` — and `Discrete`'s in `pgdt-codec`. A caller in the
  defining crate imports the trait as one above does.
- **A method on a type only `internal` exports stays inherent**: `Cursor`,
  `Lexer`, `DecodeCounter`, `StatisticsScope`, `Builder`, the leader types,
  `ColumnTier`, `UnrepresentableRead`. Nothing reaches one but
  through `internal`, as with the leader types' fields; no API signature or
  public field names any of them.
- **Comment paths moved with the items**: a `pgdt_*::<module>::<item>` path
  in a comment names the item's API or `internal` path, and one naming a
  private item keeps its module path as a location.

## What 35.5 inherits

- **The facade re-exports only `cache`, `decode`, `pgtype`, `instrument` and
  `model` as modules**, beside its root; no embedder names another module.
- **Every `internal` item is reachable by no other path** but `timed!` and
  `row_evaluated!` at `pgdt_core`'s root, as `#[macro_export]` puts them, and
  `OVERWRITE_WAYS_OUT`, which `pgdt_dump::cache` adopts. No embedder names
  `internal` or either macro, so the check holds of today's tree.
- **Three items in prose no longer resolve through the facade and were
  repointed** to their crates: `pgdt_core::encode_field` in
  `postgres-invariants.md` and `generate_perf_data.py`, and
  `pgdt_query::ROW_GROUP_DEFAULT_{SIZE_BYTES,MIN_ROWS}` in `measure.py` and
  `row_density.py`.

## Negative results

- **Nothing at a crate's root can take a private module's name** — a
  `pub use` (E0255), an inline `pub mod` (E0428) — so a namespace and its
  file's module cannot share one without the file moving.
- **A trait method taking `Option<Self>` needs `Self: Sized`**, which an
  inherent one has implicitly: `IgnoredRefusalsExt` declares it.
- **No cache byte moved**: nothing persisted changed, and
  `CACHE_FORMAT_VERSION` stands.
