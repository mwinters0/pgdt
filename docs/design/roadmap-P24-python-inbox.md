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
answers a cancelled mapping pass with `Error::ScanCancelled`. P6's provider
sets no flag at all — it reads a complete cache and is cancelled by dropping
its stream ([`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md),
"Cancellation").

**Why P24 cares.** A Python caller expects Ctrl-C to raise `KeyboardInterrupt`
promptly, which needs the binding to release the GIL during a scan and to wire
Python's signal check to something the scan reads. Whether that is
`ScanOptions::cancel`, a dropped stream as in P6, or both — and what a binding
that maps (rather than requiring a cache) does with the uncancellable preamble
prepass — is this phase's to decide.

**Origin.** Re-filed from P6's inbox when P6 narrowed to DataFusion,
2026-09-21; the original entry dates from 2026-08-27.
