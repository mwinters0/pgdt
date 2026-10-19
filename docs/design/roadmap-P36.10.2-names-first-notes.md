# P36.10.2 notes — A misspelled column refused before what it asks

Within each block, a filter term naming a column the block lacks is now
`UnknownPredicateColumn` ahead of every refusal about what a term asks. That
covers the strings schema mode's `TestNeedsDeclaredType` and, in either mode,
an order's, a comparison's and a literal's refusal, whichever term carries the
typo. D54 is rewritten to say names come first within a block.

## The shape

- **`check_columns`** (`crates/pgdt-query/src/predicate/resolve.rs`) walks the
  whole tree for the first term, left to right, whose column the block's
  unprojected schema lacks. `resolve_for_query` calls it after the block's
  schema and before `query_tests`, so the order is: schema, names, the mode,
  each term, the projection. The first refusing block in file order is still
  the plan's error.
- **`resolve_expr` keeps its own lookup refusal.** The plan never reaches it,
  but unit tests call `resolve_expr` directly, and a resolver that panicked or
  guessed on a missing column would be worse than one that refuses twice.
- **`Expr::first_term` takes the whole `Predicate`**, not only its operator,
  so the name check and `query_tests` walk the tree with the same function.

## Negative results

- **Not counted.** The added walk runs once per block at planning and touches
  no row path, the same reasoning 36.10.1 applied to its own walk.
- **A box inside an array is the fixture's unordered column.** The test needs
  a column whose order the register refuses. `json` does not serve: it is
  compared bytewise with a note, not refused. `t_delimiter.v_box_domain_array`
  is refused because of the `box` inside it.

## What the next round inherits

- **Nothing for 36.11's shell to spell differently.** The adapter's filters
  come from DataFusion's planner, which refuses an unknown column before a
  filter reaches the provider, so the order settled here is met through
  `pgdt query` and an embedder's own `Expr`.
