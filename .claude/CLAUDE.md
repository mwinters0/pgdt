## Finding code: `rg` and `LSP`

- **Locate** by name with `rg`, or with the `LSP` tool's `workspaceSymbol`. Get
  a file's outline with `LSP` `documentSymbol`, then `Read` only the span you
  need.
- **Callers, references and implementations** come from `LSP`
  (`incomingCalls`, `findReferences`, `goToImplementation`). These are
  rust-analyzer's, so they are exact for the configuration it loads.
  - Every request needs an existing `.rs` file, and a position on the
    symbol's name: a position on its doc comment or its first character can
    return nothing.
  - For an impl of a trait method, ask about the trait method. An impl's own
    references omit calls made through the trait.
- **What `LSP` does not see**, so confirm with `rg -w <name>` before
  concluding "no callers" or "these are all":
  - code behind a `cfg` that rust-analyzer has off: a non-default feature, or
    another target;
  - impls declared inside a function body, which `goToImplementation` omits.

  `mise run check` lints a build with each declared feature on, except those
  `scripts/check.py` exempts in `UNLINTED_FEATURES`, so it settles what a
  gated caller needs.

There is no code-graph index here. CodeGraph and graft were measured on this
workspace against rust-analyzer's index and against agents doing real
lookups. Their name-matched edges missed a third to a half of callers and
added wrong ones, and an agent trusting them answered wrong or used more
tokens than `rg`.
