# P6.3 — One diagnostics sink: notes

The third slice of [`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md),
"Diagnostics: one sink". The library now has `diagnostic::Finding`, the shared
trait, and `diagnostic::DiagnosticSink`, the caller's object, both defined at
L1 in `pgdump_query/src/diagnostic.rs`. `Diagnostic`, `ColumnNote` and
`ComparisonNote` implement `Finding`, and `diagnostic::drain` hands one
channel's findings to a sink. The check is `pgdump_query/tests/diagnostics.rs`:
one closure sink receives all three channels from a real fixture, and each
finding is recovered as its own type.

## What 6.5 and 6.8 inherit

- **A finding carries its sentence, and the library writes it.** `Finding`
  has `severity`, `message` and `as_any`. The file-level sentences moved out of
  `pgdt`'s `diagnostic_message` into `Diagnostic`'s `message`. The per-column
  sentence moved out of `pgdt`'s `resolution_words` into
  `ColumnResolution::describe`. `pgdt` calls both, so its output is unchanged.
  `pgdt` keeps only the `--json` token (`resolution_token`). So a stderr sink
  in `datafusion-pgdump-cli` prints `<severity>: <message>` and writes no
  sentences of its own.
- **A finding does not name its table.** `ColumnNote` names its column only,
  and so does `ComparisonNote`. At registration the provider knows which table
  it is draining, so its sink should wrap the caller's and add the table name.
  The library has no field for it.
- **`message` is a trait method now, not an inherent one.** An embedder
  calling `ComparisonNote::message` has to import `pgdump_query::Finding`.
  `ColumnNote::severity` is the same.
- **Two channels do not implement `Finding`.** `TableStream::plan_notes` and
  `TableStream::early_stops` report what a query raised, and the spec names
  only three channels. `pgdt` decides their severity per kind in
  `announce_plan_notes`, and adds a budget-origin clause that only it knows.
  If 6.8's "after the statement" print should include them, either they gain a
  `Finding` impl with the severity moved into the library, or the CLI prints
  them beside the sink.
- **In Arrow semantics a query's comparison channel is always empty**
  (6.2's notes). So the provider's comparison findings are not per term. The
  spec reports divergence per column, at registration, which
  `predicate::column_divergences` produces (6.3.1).

## Negative results

- **`pgdt` does not adopt the sink**, as the spec allows. It still prints each
  channel where its output already speaks: `info`'s `diagnostics:` block,
  `--detail`'s column lines and `query`'s stderr warnings.
