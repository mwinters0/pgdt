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
[`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md),
"Diagnostics: a file-level channel on `DumpIndex`", and
[`../status/history/2026-08-24.md`](../status/history/2026-08-24.md), "The
diagnostic vocabulary is a scale, not a type".

**Contingent on.** `Severity` staying derivable for `ColumnNote` (it is a
method, not a field, today) and on no third diagnostic producer appearing with
a shape neither channel fits.
