# P42 inbox — facts filed for its grilling

Evidence found in earlier phases that P42 (`--format=directory` and
`--format=tar`) will need. **This is a queue, not a document**: when P42 is
grilled, walk every entry, fold it into the spec or discard it as stale, and
delete this file. See `docs/process.md`, "Inboxes: facts filed by
destination".

Each entry says what the fact is, why *this* phase cares, and where it came
from — go re-check the origin rather than trusting an entry that has aged.

---

## Every source sizes itself as the run's only source

**Fact.** `DecodeBudget` apportions the whole stated budget to its own pools
(`source/compressed/blocks.rs`), and a gzip first pass spawns up to `jobs − 1`
tasks (`deflate/first_pass.rs`). Each source answers for its own defaults
(`roadmap.md`, "A default runs as fast as the allocation permits"), as though
it were the only one.

**Why P42 cares.** A directory archive opened as N whole-file sources would
make N claims on `--jobs` and `--memory`. The source layer presenting many
files, which this phase owns, decides who apportions the run's allocation
among them. The aggregate behaviour is inferred from the per-source sites, not
observed.

**Origin.** An architecture review at `ec20c8fd`, 2026-11-20. *Contingent on*
`crates/pgdt-io/src/source/compressed/blocks.rs`.
