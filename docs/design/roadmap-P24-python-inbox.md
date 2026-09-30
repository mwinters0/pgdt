# P24 inbox — facts filed for its grilling

Evidence found in earlier phases that P24 (Python bindings) will need. **This
is a queue, not a document**: when P24 is grilled, walk every entry, fold it
into the spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

---

## A Python caller expects `KeyboardInterrupt` to reach a scan, and the library's cancellation is a flag

**Fact.** `ScanOptions::cancel: Option<Arc<Cancellation>>` stops a mapping scan
cooperatively, read once per chunk and at every completed `COPY` block, and
carries an awaitable signal a source races an in-flight read against
([`decisions.md`](decisions.md), "D26", "D63"). `scan::scan`, and so the
preamble prepass, ignores the polled flag on purpose. `stream::table_stream`
answers a cancelled mapping pass with `Error::ScanCancelled`. The DataFusion
provider sets no flag at all — it reads a complete cache and is cancelled by
dropping its stream ([`decisions.md`](decisions.md), "D90").

**Why P24 cares.** A Python caller expects Ctrl-C to raise `KeyboardInterrupt`
promptly, which needs the binding to release the GIL during a scan and to wire
Python's signal check to something the scan reads. Whether that is
`ScanOptions::cancel`, a dropped stream as in the DataFusion provider, or both — and what a binding
that maps (rather than requiring a cache) does with the uncancellable preamble
prepass — is this phase's to decide.

**Origin.** Filed 2026-08-27, for the embeddable-engine phase that Python
bindings were carved out of.

---

## A front end other than DataFusion takes semantics of its own

**Fact.** `QueryOptions::semantics` names the front end, not an abstract
order: `ComparisonSemantics::Arrow` is DataFusion's comparison (a float's `-0`
made `0`, `pgtype.rs`), and under the typed mode it also nulls what
DataFusion cannot print, `arrow-cast`'s calendar ending at `262142-12-31`
([`decisions.md`](decisions.md), "D98"; `RT21`). Its rename to `DataFusion`
is M182.

**Why P24 cares.** A Python caller handing batches to pyarrow, polars or
pandas compares and prints with that library, not DataFusion. Borrowing
DataFusion's variant would null dates that front end prints, and compare a
float's `-0` as it does not; the binding decides whether it needs a variant
of its own, whose comparison and display each set the tiers the typed mode
nulls.

**Origin.** Filed 2026-09-30, closing 28.5's call on D98. Contingent on
`arrow-cast` still printing through `chrono` (`../status/upstream.md`, "UF2").

