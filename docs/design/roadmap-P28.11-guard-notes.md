# P28.11 — The unrepresentable guard, the embedder's: notes

What the slices after this one inherit. The spec is
[`roadmap-P28-unrepresentable-values.md`](roadmap-P28-unrepresentable-values.md),
"Scope" and "Slices"; the decision is `decisions.md`, "D101", and the
external behaviour it leans on `runtime-invariants.md`, "RT22".

## What exists

- **`register_unrepresentable(ctx)`** registers `pgdump_unrepresentable` and
  nothing else, through `SessionContext::register_udf`, which rebuilds no
  state. `register_dump` and `register_table_factory` call it; an embedder
  registering a `PgDumpTable` by hand calls it itself.
- **`with_unrepresentable_guard(builder)`** pushes the function onto the
  builder's scalar functions and appends a physical optimizer rule,
  `pgdump_unrepresentable_guard`, which walks every node's
  `apply_expressions` for a `ScalarFunctionExpr` of that name and refuses the
  plan, returning it unchanged otherwise. Its refusal reaches the caller
  wrapped in DataFusion's `Context` naming the rule.
- **Without the guard the function refuses at execution** (`exec_err!`),
  naming `with_unrepresentable_guard`; the guard's refusal does not name it.
- **`datafusion-cli-pgdump` builds its state** as `new_with_config_rt` did,
  with the guard, a `pgdump:`-marked change to upstream's `main.rs`.
- **The harness's function test plans under the guard**; the rest of the
  harness does not install it, holding no use of the function.

## What the next slices inherit

- **Nothing 28.9 times holds the function**, so no figure moves with the
  guard, and the shell's session is otherwise the one `new_with_config_rt`
  built.

## Negative results

- **A plan whose statistics answer the aggregate holds no function to
  refuse**: `SELECT id, (SELECT MAX(x) FROM memory WHERE
  pgdump_unrepresentable(x)) FROM t_date` over an empty `MemTable` plans
  under the guard, the filtered aggregate planned as `NULL` from the table's
  exact zero rows (its `EXPLAIN`: `ProjectionExec: expr=[NULL as
  max(memory.x)]`), and runs, the
  function never evaluated; a walk of the logical plan, which still holds the
  `Filter`, refuses it. The refusal test gives the table a row.
