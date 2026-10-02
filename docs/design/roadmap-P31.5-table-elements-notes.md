# P31.5 — A `CREATE TABLE` list's constraints and brackets: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"Columns declared elsewhere, and what the preamble holds".

## What exists

- **`preamble.rs`'s `parse_table_element`** sorts each top-level fragment of
  a `CREATE TABLE` list into gram.y's three `TableElement` kinds, holding the
  column and dropping the other two; the external fact it rests on is I53.
  `split_top_level_commas` tracks `[`…`]` beside `(`…`)`, for every caller.
- **`emitters.stamped`**, a default of `ARRAY[now(), now()]` followed by a
  column `now`, at every major and flag set of the `emitters` schema, and
  `tests/preamble.rs`'s `emitters_tables_declare_their_columns_and_nothing_else`
  holding the parent, the child and it to exactly the columns `pg_dump` wrote.
  It failed before the fix at 13 on the parent's fourth column, `constraint`.

## Findings

- **18 writes a table-level `NOT NULL <column>`**, and `CONSTRAINT <name> NOT
  NULL <column>`, as list elements among the columns, for an inherited column
  carrying a local not-null constraint (`dumpTableSchema`'s
  `!shouldPrintColumn` arm). The spec's list of constraint openers omits
  `NOT`; it is gram.y's `ConstraintElem` like the rest, so it is classified
  with them. `fixtures/18/emitters/default.sql`'s `emitters.child` held it,
  read as a column `not` of type `NULL label`.
- **A `LIKE` clause's columns are not followed**: P32's inbox carries it.
- **No cache-format bump landed with it**, nor with `M199` or `M200`, though
  each changes what the cache persists: `M202` makes the one bump owed for all
  three (D22).
- **Regenerating `emitters` rewrites every file's `\restrict` key and
  timestamps**; a file whose diff was only those was restored, so the change
  touches only the files holding `stamped` and the OIDs it shifted under
  `--binary-upgrade`.

## What the slices after this inherit

- **31.6's typed-table column written without a type** (`name NOT NULL`
  under `OF`) still parses to no column: `parse_column_fragment` refuses an
  empty type, which is where the walk the spec describes takes over.
- **A new constraint opener at a later major** surfaces as I53's re-verify
  printing an arm it does not name.
