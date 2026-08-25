# Phase 6 inbox — facts filed for its grilling

Evidence found in earlier phases that Phase 6 (embeddable engine story) will
need. **This is a queue, not a document**: when Phase 6 is grilled, walk every
entry, fold it into the spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

Each entry says what the fact is, why *this* phase cares, and where it came
from — go re-check the origin rather than trusting an entry that has aged.

---

## There are two diagnostic channels, and the sink is where they unify

**Fact.** A `DumpIndex` carries `diagnostics: Vec<diagnostic::Diagnostic>`
(file-level: tiling failures, cache mtime mismatches, and whatever 3.3's
TOC-coverage figure becomes). A `ResolvedSchema` separately carries `notes:
Vec<resolve::ColumnNote>`, one per column. They are **not** one type, and
cannot be: `DumpIndex` is L1 while `ColumnResolution` is an L2 conclusion
about PostgreSQL type semantics, so a single enum would have L1 name an L2
type. What they share is the `Severity` scale — `diagnostic::Severity`, which
derives `Ord` (`Info < Warning < Error`) precisely so a consumer can filter
both channels against one threshold.

**Why Phase 6 cares.** The phase-3 spec defers the caller-supplied sink (the
`tracing-subscriber` shape) to Phase 6 and says "a sink can drain this list,
so nothing here forecloses it". That is now load-bearing in a specific way:
**the sink is the designated unification point.** A `Diagnostic` trait that
both types implement is the shape that was left open — deliberately, because
unifying at the storage type is what the layering forbids and unifying at the
drain point is what it permits. A Phase 6 design that gives the sink only one
of the two channels re-opens a question that was already settled the other
way.

**Origin.** Slice 3.2.2 and its follow-up, 2026-08-24. See
[`architecture.md`](architecture.md),
"Diagnostics: a file-level channel on `DumpIndex`", and
[`../status/history/2026-08-24.md`](../status/history/2026-08-24.md), "The
diagnostic vocabulary is a scale, not a type".

**Contingent on.** `Severity` staying derivable for `ColumnNote` (it is a
method, not a field, today) and on no third diagnostic producer appearing with
a shape neither channel fits.

---

## A default query cannot detect every ambiguous table, and the API says nothing about it

**Fact.** A query stops mapping once its target is settled
(`stream::target_settled`, the `ScanExtent::UntilTargetSettled` default), so a
second `COPY` block for the same qualified name **past** the stopping point is
never seen and `Error::AmbiguousTable` is not raised for it — the query
returns the candidate it found, silently. The stop rule catches the two shapes
that announce themselves in the prefix: a matching block carrying the
partition-root marker (I2), and a file containing any `\connect` at all
(`pg_dumpall`, concatenation, `--create`). What stays undetectable is a file
whose *first* segment is an ordinary dump with something concatenated after
it — nothing before the stopping point says so. `ScanExtent::Full`, or any
query against an already-`pgdq parse`d file, detects it exactly. Rows are
never a union either way.

**Why Phase 6 cares.** This is the one place where the default query path
returns a *possibly wrong answer with no signal*, rather than an error or a
degraded one — and Phase 6 is where what the embedded API promises a caller
gets decided. Three shapes are open and only Phase 6 can pick: leave the
default as-is and document it; make `ScanExtent::Full` the default and pay a
full scan per cold query; or emit a `Diagnostic` on every early stop saying
the answer is conditional on no concatenation, which costs nothing and turns
silence into a filterable signal. The third interacts directly with the sink
entry above, which is why both are filed here.

**Origin.** Slice 3.2.1.2.1, 2026-08-24 (the decision), carried through
Phase 3's end-of-phase grilling as an accepted gap. See `STATUS.md`'s "Known
gaps" and
[`architecture.md`](architecture.md),
"Mapping and streaming are separate passes".

**Contingent on.** Early stopping surviving as the default, and on no cheaper
concatenation detector turning up — a prefix-visible marker would collapse the
question entirely.
