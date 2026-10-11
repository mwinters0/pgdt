# P35.26 notes — `pgdt-fixtures`, the tests' vocabulary

The crate the spec's "The crate graph" names beside the libraries
(`../status/history/2026-10-11.md`, "The test vocabulary, a crate of its
own"). No library code moved and no test changed what it asserts; nothing
persisted changed, so no format bump.

## Where the vocabulary landed

- **`crates/pgdt-fixtures/src/lib.rs` is pgdt-mapping's `tests/common/mod.rs`,
  moved whole**, `refused.rs` beside it as a private module whose items the
  root re-exports, so each has one path. `edge_cases.sql` sits beside the
  manifest. `all_fixtures` now sorts, as `pgdt`'s and `datafusion-pgdump`'s
  copies did.
- **`sweep_request` and `past_refused` sit behind `mapping`**, the only
  dependency. pgdt-query, datafusion-pgdump turn it on; `pgdt`'s tests read
  `refused_field` alone, and pgdt-codec's and pgdt-mapping's tests neither.
- **Each crate's tests reach it by dev-dependency**: pgdt-codec, pgdt-mapping,
  pgdt-query, datafusion-pgdump and `pgdt`. The tests of pgdt-core,
  pgdt-model, pgdt-io and pgdt-dump read no fixture, so take none. pgdt-mapping's integration tests
  import it directly, their `mod common` gone; pgdt-query's and `pgdt`'s
  `tests/common/mod.rs` re-export it beside what reading a batch back, or
  running the binary, adds.
- **Every copy replaced**: `pgdt`'s `fixture`, `all_fixtures`, `sandboxed`,
  `sandboxed_multidb` and its three `edge_cases_sql` bodies;
  datafusion-pgdump's `fixtures_root` and enumerators in four targets, the
  filtered ones now filters over `all_fixtures`; the unit tests' fixture roots
  in pgdt-codec's `key/tests.rs`, pgdt-query's `predicate/tests.rs` and
  pgdt-mapping's `interior/tests.rs`; `MAJORS`, now `VERSIONS`, in the first
  two and pgdt-query's `tests/nested.rs`; and the inline `../../fixtures`
  joins in pgdt-query's and pgdt-mapping's tests. `pgdt`'s
  call sites address a fixture by `(major, schema, flag set)` where they wrote
  a relative path; a `const` holding one is joined to `fixtures_root()`.

## The checks

- **`check.py` follows `mod` lines only**, and **a library only
  dev-dependencies reach reads for every target depending on it**
  (`Package.support`): without that, a fixture change would have reached only
  the crate whose source holds the literal. A fixture change runs every
  target of the five crates above.
- **`layering.py` refuses `#[path]` in every member, tests included**, which
  is what lets `check.py` stop following it. `GRAPH` lists `pgdt-fixtures`
  with its one edge, to pgdt-mapping, and no row lists it, so a normal
  dependency on it is refused.
- **`persisted_index_is_pinned_to_the_format_version` keeps the label
  `"tests/data/edge_cases.sql"`**, which its digest hashes; the path it reads
  moved.

## What later cuts inherit

- **A unit test in pgdt-codec or pgdt-mapping links a second copy of its own
  crate** when the workspace is built together, feature unification giving
  `pgdt-fixtures` its `mapping` edge. Nothing passes a type across, and a
  test that wanted `sweep_request` there would find the two copies' types
  unequal.
