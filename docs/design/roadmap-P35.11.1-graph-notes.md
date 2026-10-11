# P35.11.1 notes — the design checked against the module graph

35.11's design, stated as moves and projected onto `2ea0056b`'s references by
`scripts/module_graph.py --design`. Each crate's section of
[`roadmap-P35.11-trees-notes.md`](roadmap-P35.11-trees-notes.md) now carries
its cut's ```` ```moves ```` block and the tree redrawn where the projection
refuted it. The doc's blocks projected together leave no cycle but the kept
`origin` ↔ `remote`, and no crate cycle. No code moved.

## What the cuts inherit

- **The index the moves were drawn against** is
  `runs/module-graph-35.11.1/index.scip`, with `cycles.py` and `items.py`
  beside it (its `README.md`): every cycle with its references, and each
  module's items with the references in and out. A cut builds its own index;
  this one reads `2ea0056b`'s paths, which the blocks name.
- **Each `OWED` entry names the cut that removes it**: 35.17 the codec's two,
  35.18 `gather/` and `map` ↔ `backfill`, 35.19 and 35.20 the stream's, 35.21
  datafusion-pgdump's root, 35.22 `pgdt` ↔ `where_expr`, 35.23 pgdt-model's,
  pgdt-dump's `cache/` and `xz` ↔ `blocks`.
- **A `--design` run still reads `OWED` for the cycles a block leaves
  untouched**, and names no entry its moves made stale; a cut reads the `!!`
  lines, and the tree's own run after the cut is what fails a stale entry.
- **35.19's `eval.rs` and `truths.rs` are `impl` blocks of the types
  `resolve.rs` defines**, one move line a method; `impl ResolvedMembership` and
  `impl Lookup` split by method in the cut.
- **35.22 re-projects once `commands` is split** into each subcommand's `run`:
  `commands` is one item today, so the block can only move it whole.
- **35.21 keeps `PgDump`'s impls in three files** — `dump.rs`, `table.rs`'s
  `PgDump::table`, `report.rs`'s `PgDump::report` — as today's code has them,
  split by role across siblings rather than inside `dump`'s tree.

## Settled with the maintainer

- **`dictionary` folds into `column`** in pgdt-mapping's `gather/`: the
  dictionary's state is the column group's texts, and a `GroupTexts` type of
  its own would change a field's type, which a whole move does not.
- **A type's impls may be split by role inside its own tree**, each method
  placed by its own move line; the `seams` skill says so, and
  `module_graph.py` reads `<module>::<Type>::<method> -> <to>`.
- **35.23 is a cut of its own**, ahead of 35.17, for the cycles no other cut
  removes; 35.12 stays the closing round.
- **pgdt-dump's operations go to `cache/store.rs`**, and pgdt-model's
  `BlockObserver` to `statistics.rs` beside what it answers.
- **The codec's `OrderKey` builders stay in `key.rs`**, `first_difference`
  in `value.rs`, and `key` reads `value/datetime/grammar.rs`'s `DateTimeInput`.

## Negative results

- **35.11's design, unread against the edges, cycled in every crate whose
  cut is still owed.** "`compare` imports `TextGrammar` from `grammar`, and nothing
  imports back": `grammar`'s checks read `compare`'s plans throughout, and
  `compare` ↔ `pgtype` stood whichever way one file was cut, until the
  declared type's catalog walk went below both. In pgdt-query, `table_stream`
  in `stream/table.rs` and `replay` read each other; in pgdt-mapping,
  `Clipped` in `bounds` made `order` ↔ `bounds`; in `pgdt`, `--filter`'s entry
  in `term.rs` remade today's `pgdt` ↔ `where_expr` as `term` ↔ `expr`; in
  datafusion-pgdump, `literal_text` left in `pushdown` made `filter` ↔
  `pushdown`; a
  family building an `OrderKey`, which holds the family's key type, cycles
  with `key`; a `numeric/spelling.rs` child reads its parent's typmod helpers
  and is read by it.
- **A parent defining what its children read and calling them is a cycle**:
  pgdt-dump's `cache.rs` held the operations `mode` calls and read `file`,
  which answers `CacheStatus` from it; datafusion-pgdump's `lib.rs` held
  `Error`, which every module reads, and `register_dump`, which reads them.
- **The graph credited a generic `impl`'s methods to no item**: the `impl`
  header's `<…>` hid its self type, and `Believed<'_>`, `Position<T>` and the
  like named no item a move could; each definition is now credited to the
  top-level item holding it.
