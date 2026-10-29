# P24 inbox — facts filed for its grilling

Evidence found in earlier phases that P24 (Python bindings) will need. **This
is a queue, not a document**: when P24 is grilled, walk every entry, fold it
into the spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

---

## A Python caller expects `KeyboardInterrupt` to reach a scan, and the library's cancellation is a flag

**Fact.** `map_file`'s required `tokio_util` `CancellationToken` stops a
mapping scan cooperatively, polled once per chunk and at every completed `COPY`
block, and handed to a source that races an in-flight read against it
([`decisions.md`](decisions.md), "D26", "D63"). `scan::scan`, and so the
preamble prepass, polls none on purpose. A query takes no token: dropping its
stream is its cancellation, as it is the DataFusion provider's, which reads a
complete cache ([`decisions.md`](decisions.md), "D90").

**Why P24 cares.** A Python caller expects Ctrl-C to raise `KeyboardInterrupt`
promptly, which needs the binding to release the GIL during a scan and to wire
Python's signal check to something the scan reads. Whether that is
`map_file`'s token, a dropped stream as a query takes, or both — and what a binding
that maps (rather than requiring a cache) does with the uncancellable preamble
prepass — is this phase's to decide.

**Origin.** Filed 2026-08-27, for the embeddable-engine phase that Python
bindings were carved out of.

---

## A front end other than DataFusion takes semantics of its own

**Fact.** `QueryOptions::semantics` names the front end, not an abstract
order: `ComparisonSemantics::DataFusion` is DataFusion's comparison (a
float's `-0` made `0`, `compare.rs`), and under the typed mode it also nulls
what DataFusion cannot print, `arrow-cast`'s calendar ending at `262142-12-31`
([`decisions.md`](decisions.md), "D98"; `RT21`).

**Why P24 cares.** A Python caller handing batches to pyarrow, polars or
pandas compares and prints with that library, not DataFusion. Borrowing
DataFusion's variant would null dates that front end prints, and compare a
float's `-0` as it does not; the binding decides whether it needs a variant
of its own, whose comparison and display each set the tiers the typed mode
nulls.

**Origin.** Filed 2026-09-30, closing a call on D98. Contingent on
`arrow-cast` still printing through `chrono` (`../status/upstream.md`, "UF2").


## The library P24 binds is seven crates with no facade

**Fact.** The library is `pgdt-core`, `pgdt-model`, `pgdt-io`, `pgdt-dump`,
`pgdt-codec`, `pgdt-mapping` and `pgdt-query`, with no facade and no public
face; what a boundary forces public that is not API sits in each crate's
`internal` module, and no embedder may name one
([`decisions.md`](decisions.md), "D68", "D106").

**Why P24 cares.** The binding is an embedder like `datafusion-pgdump`, and
which crates it depends on, and whether it wants one entry crate of its own,
is its to decide — D106 leaves a binding to make its own entry point.
Whatever it decides, `scripts/layering.py` refuses a member its `GRAPH` does
not list, and reads an embedder's sources for `internal` only once
`EMBEDDERS` names it, so the binding's crate takes a row in both.

**Origin.** Filed 2026-10-10, grilling the crate split; contingent on D68's
graph and D106.

---

## An embedder tells one compressed failure from another only by naming its codec's crate

**Fact.** `pgdt_core::Error::Codec` carries `xz_seek`'s error boxed for an
`.xz` failure and `gz_seek`'s for a gzip one, naming the codec, and
`Error::codec_cause::<E>()` downcasts it, so a caller distinguishing
`InvalidTable` from `BlockCheckFailed`, or `UnitCheck` from `IndexMismatch`,
depends on that crate and names its type. The seek table is opaque (`pgdt_io::SeekTable`); this path is
unweighed, and no embedder in the workspace reads it.

**Why P24 cares.** A Python exception hierarchy either maps those causes to
classes of its own, which needs the binding to name `xz_seek`'s error, or
reports every codec's failures as one class; the library offering the
distinction in its own terms is the alternative, and a spec change.

**Origin.** Filed 2026-10-19, wrapping the embedder API's reshape, and
widened to gzip 2026-10-29; contingent on `Error::codec_cause` (`crates/pgdt-core/src/error.rs`).

---

## Every table stream is `'static`, and its type still carries a lifetime

**Fact.** `TableStream<'a>` and `BlockingTableIter<'a>` keep their lifetime
parameter though every stream a plan hands back owns its source, watch and
blocks (`Table` and `TablePartitions` hold them by `Arc`), so each is
`TableStream<'static>` in practice. The parameter stays because dropping it
touches the DataFusion adapter's signatures.

**Why P24 cares.** A `pyo3` class cannot hold a borrowing type; the binding
names `TableStream<'static>`, or the parameter goes first.

**Origin.** Filed 2026-10-19, wrapping the embedder API's reshape;
contingent on `crates/pgdt-query/src/stream/table.rs`.
