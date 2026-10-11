# P35.21 notes — datafusion-pgdump's filter and its root

The cut toward [`roadmap-P35.11-trees-notes.md`](roadmap-P35.11-trees-notes.md)'s
"datafusion-pgdump" tree, its `moves` block applied whole and then struck from
that doc. Every body moved whole. `module_graph.py` finds no cycle in
datafusion-pgdump, so its `OWED` entry is gone and `pgdt` ↔ `where_expr` is
the last; doc links against the edges fell from 204 to 192.

## Where the cut fell

- **`PgDump`'s impls sit in three files**, as
  [`roadmap-P35.11.1-graph-notes.md`](roadmap-P35.11.1-graph-notes.md) has it:
  `open` and the rest in `dump.rs`, `table` in `table.rs`, `report` in
  `report.rs`.
- **Widened to compile**: `Located` and `Reporting`, with their fields, to
  `pub(crate)`, which `table.rs`'s `report` and `report_plan` build and read.
  `PgDumpTable::reporting`, the accessor `report.rs` reached the field through,
  is gone: both its readers are now the table's own methods.
- **The leaves are private to `filter.rs`**, `pub(crate)` before: a parent's
  private item is visible to its children, the only readers. `filter.rs`
  re-exports `translate`, `DynamicFilters` and `ReplayFilter`, as
  pgdt-query's `predicate.rs` does, so `table.rs` and `exec.rs` name
  `crate::filter`.
- **The paragraph on a literal's text (D88) went with `literal_text`**, from
  `pushdown.rs`'s doc to `filter.rs`'s.
- **A doc link into `filter`'s children from outside it is a code span**: the
  children are private to `filter`, so rustdoc cannot resolve them from a
  sibling. Its warnings in the crate fell from eight to six, both links
  public docs made to the old modules gone. A doc link to an item now defined
  below the root names its module (`crate::catalog::register_dump`), so the
  five that run against the edges are listed by `--docs`.
- **`factory.rs` keeps its own `external`**, over this crate's `Error` rather
  than the library's: no move line names it.

## Tests

- **The suite is unchanged.** `dynamic_filter/tests.rs` moved whole to
  `filter/dynamic/tests.rs`; `report.rs`'s inline tests read
  `BudgetedPlanNote` alone and stayed.
