# P31.6 — Columns declared elsewhere: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"Columns declared elsewhere, and what the preamble holds".

## What exists

- **`preamble::TableDef`** is what `DatabaseMetadata::tables` holds per table:
  its own columns, its `INHERITS` parents and its `OF` type, and
  `SpanBody::Table` carries one. `SpanBody::AlterTableReference` is the
  `--binary-upgrade` `ALTER TABLE ONLY … INHERIT …` / `… OF …`, folded into
  its table as `AlterTypeAddValue` is into its enum.
- **`DatabaseMetadata::declared_column`** is the walk, and
  **`declared_columns`** the same reach in the server's column order. Every
  reader of a column's declaration goes through them: `resolve_columns`,
  statistics gathering and the `believed` checks in `prune.rs` and
  `summary.rs` (`gather::declared_column`), and the merged schema's column
  order (`stream.rs`'s `TableColumns`). The walk is in L1, being a lookup of
  what the file declares rather than a conclusion about a type (D74). D36
  carries its refusal.
- **`CACHE_FORMAT_VERSION` was bumped**, both pins re-pinned beside it.
- **Evidence**: `tests/preamble.rs`'s
  `inherited_and_typed_columns_are_declared_through_their_references` over
  `emitters/default` and `emitters/binary-upgrade` at every major, and
  `preamble.rs`'s unit tests for the grammar, the fold and the walk. The two
  `known_failures.rs` rows went.

## Rules the spec did not state, and why

- **A typed table's list declares no column of its own.** The spec has a
  typed table's type-less column "defer to the walk"; gram.y's
  `TypedTableElement` is a column's options or a table constraint and never
  holds a type, so the list is read for nothing and every column is the
  type's field. A `WITH OPTIONS` spelling, which only a hand-written file
  holds, therefore cannot read as a type either.
- **A column the table and a reference both declare is the table's own**,
  at the place the server gives it: the first parent's. The server refuses a
  merge differing in type or collation, so the choice moves nothing `pg_dump`
  writes.
- **Only an added reference is read.** `NO INHERIT`, `NOT OF` and an `ALTER
  TABLE` listing several subcommands stay `Unparsed`; `pg_dump` writes none of
  them for these.
- **An `INHERITS` parent the identifier grammar refuses is dropped**, its
  columns reaching nothing, rather than costing the table its own.

## What the slices after this inherit

- **31.7's foreign table** is already folded: `ALTER FOREIGN TABLE ONLY …
  INHERIT …` parses, and reaches its table once `CREATE FOREIGN TABLE` is
  classified.
- **`KD64`'s ordering cost closed with it**: an inherited column no longer
  sorts after the declared ones in a merged schema.
