# P35.15 notes — pgdt-dump's tree

The cut toward [`roadmap-P35.11-trees-notes.md`](roadmap-P35.11-trees-notes.md)'s
"pgdt-dump" tree. Every body moved whole; the crate's API and `internal`
export the names they did, so nothing above pgdt-dump changed but comments
citing a path. `leader.rs` is untouched, 35.18 moving it.

## Where the cut fell

- **The tree is the design's**, with the placements it left open:
  - `ddl/lex.rs` takes every helper that steps over a quote or matches a
    paren or keyword, the two quote-aware word splitters (`type_tokens`,
    `top_level_words`), `collapse_spacing` and `holds_word` among them;
    `ident_after` went to `ddl/refs.rs`, its one caller's file.
  - The column grammar — a column's type words, `COLLATE`, `NOT NULL` and
    `CHECK`s — is `ddl/table.rs`'s, and `ddl/types.rs` reads it for a
    composite's fields and a domain's tail; nothing reads back.
  - `map/toc.rs` holds the comment-block readers: the TOC header, the
    boundary test, the partition-root marker, the version-header lines and
    `close_comment`. The `INSERT INTO` target stays in `map.rs` with the run
    it opens.
  - `cache/mode.rs` takes `CacheLoad` beside `CacheMode`, being what its
    `load` answers; `advisory_identity_diagnostics` went to
    `cache/identity.rs`, `colocated_path` stayed in `cache.rs`.
- **Visibility.** Items shared inside `ddl/` are `pub(super)`; what `map.rs`
  reads is re-exported `pub(crate)` by `ddl.rs`. `partition_bound` is
  `pub(super)` for its test alone, which sits in `ddl/tests.rs`. `cache.rs`
  declares the identity and mode items by `pub use` from its children, `mode`
  private.
- **Tests beside what they test.** `preamble/tests.rs` split four ways:
  `ddl/tests.rs` keeps every test driving the grammar through
  `classify_statement` and its `ALTER` entry points, unsplit by child since
  they share its `parse_*` helpers; `ddl/metadata/tests.rs` the folds,
  reaching `ddl::tests::parse_type` (`pub(super)`) for the one that parses a
  composite first; `ddl/refs/tests.rs` and `map/statement/tests.rs` the
  rest. `map/toc/tests.rs` takes the six tests reading the comment-block
  readers alone. `cache/tests.rs` tested `write_beside` only, so it is
  `cache/file/tests.rs`. The crate's suite is the same 144 tests.
- **Scripts.** `measure.py`'s `MAP`, `CACHE` and `PREAMBLE` declare the new
  files; `StatementScan` now sits under `MAP`, so the `INSERT`-run figures
  declare it, as they did not while it was in `preamble.rs`.
  `emitter_register.py`'s reader is a module, its file and the production
  files under the directory beside it (`reader_source`): a per-file reader
  would have split each `Clause` from the `Read` it lies within, `INHERITS`
  in `ddl/table.rs` from `CREATE TABLE` in `ddl.rs`.

## What later cuts inherit

- **One edge runs both ways**: `cache/identity.rs`'s `SourceWatch` holds a
  `StrictIdentity`, which the design puts in `cache/mode.rs`, and `mode`
  reads `SourceWatch`, `WeakIdentity` and `OriginMatch` back. Moving
  `StrictIdentity` into `identity.rs` would make it one-way — a placement
  for 35.12's closing read to weigh, as 35.14 left `source/xz.rs` ↔
  `xz/blocks.rs`.
- `ddl/tests.rs` is the crate's longest test file. A split by child would
  share its `parse_*` helpers across the children's test files, as
  `parse_type` already is with `ddl/metadata/tests.rs`.
