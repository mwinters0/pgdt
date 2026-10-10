# P35 — Crate seams and file limits

The library becomes six crates whose dependency graph Cargo enforces, every
file over its size limit is cut at its own seams, and a check and a skill keep
both true afterwards. No behaviour changes: every test that passes before a
slice passes after it, unmoved or moved whole.

What this phase will do and why; how it lands is its slices'. Progress is
`STATUS.md`'s checklist, never this file. Grilled 2026-10-10.

## Why now

The code grew from one file to twenty-six by accretion, and P31 added two to
four thousand lines to each of five of them. Navigation is what suffers: the
module a reader wants is a region of a file whose name no longer says what it
holds, and the layer model that keeps the dependencies one-way
([`decisions.md`](decisions.md), "D68") is enforced by a test that reads source
text. The keystone that struck P31 said the shape had stopped being in
question; that is the moment crate boundaries can be real rather than drawn.

Compile time is the second motive and the weaker one: the edit-to-check loop
is already short (`runs/rebuild-baseline-20261009.txt`), and a round's cost is
mostly the suite running. What a split buys is that an edit in one crate no
longer re-reads the others, and a test binary moved down links less; a
`--timings` run on the split tree against the baseline is the figure, taken by
the slice that carves the crates, not promised here.

## The crate graph

Six library crates, each depending only on those above it in this list; the
layers of D68 become crates, with one new one below the parsers.

- **`pgdt-core`** — the error type, the `introspect` hooks, the SQL lexer and
  the COPY text format: what every other crate reads and nothing else owns.
- **`pgdt-model`** — the dump's vocabulary: spans, blocks, the schema the
  preamble declares, block statistics, diagnostics. **This is the cache's
  format**; `bump-cache-format` points here. Pure data, no I/O, no Arrow.
- **`pgdt-io`** — byte sources (local, `.xz`, fetched, remote), the memory
  limit and its discovery, worker sizing. The `http` feature lives here.
- **`pgdt-dump`** — the parsers and the cache: the scanner, the preamble
  grammar, the map builder, the interior split, persisting and reloading the
  model. Depends on core, model, io.
- **`pgdt-codec`** — PostgreSQL values: every decoder, the datetime grammar,
  nested literals, the type register, the comparison plans and their ordering
  keys, column resolution. Depends on core and model; names `arrow-schema`
  only and is synchronous. The ordering keys move here from the query layer,
  where they sit today in `predicate.rs`: they are value semantics, not
  filtering.
- **`pgdt-query`** — Arrow batches, the streams, the filter expression,
  statistics gathering and pruning, summaries. Everything above it is an
  embedder: `datafusion-pgdump`, `datafusion-cli-pgdump`, `pgdt`.

What the graph settles: D74's two rules — the parsing crates compile no Arrow,
the codec reaches `arrow::datatypes` and no further and does no I/O — become
dependency lists rather than greps. Why `pgdt-model` exists: without it the
codec sits above the preamble grammar, so a change to how `CREATE TABLE` is
read recompiles every decoder; with it the codec and the parsers are siblings.
Why the cache is not its own crate: it persists the model and is read by the
index loader that then scans, so it sits beside the parsers, not below them.

**Statistics gathering is query-side.** It decodes values, so it sits above
the codec, as D68 has it; only the statistics *types* are model.

**The graph is the current answer, not a fixed one.** The `seams` skill asks,
each time it runs, whether a crate has become two or two have become one, by
the same evidence that drew this graph: the edges between modules, and which
files a change touches together. A crate split is a decision and goes through
the register; the skill names when one is due.

## Naming and layout

Library crates are `pgdt-<name>`. The two DataFusion crates keep their
`datafusion-*` names: releases are being cut, and that prefix is how the
DataFusion ecosystem finds a provider. `namespace-init` and `peak-rss` are
apparatus, not library, and keep their names. Every member moves under
`crates/`; `vendor/xz-seek` stays where the vendoring script puts it.

During the carve, `pgdump_query` stays as a facade re-exporting what the new
crates define, so the test binaries, `pgdt` and the DataFusion crates keep
building slice by slice; the last carving slice retires it. Pre-1.0 means the
facade owes nobody its survival ([`roadmap.md`](roadmap.md), "Pre-1.0").

## The file limits and the skill

A file is measured as production code lines, production functions and total
lines, its inline tests counted only in the last; the numbers are stated once,
in `scripts/file_limits.py`, and the rule — soft limits that signal, hard ones
that oblige, an exemption table carrying a reason per file — is
[`../process.md`](../process.md), "Seams". The check is a meter in `mise run
check`, never a failure: a slice is sized by its review, and what it leaves
long is the next seams round's.

`.claude/skills/seams/SKILL.md` is that round. Its first cut in any file is
the inline tests to a file beside the module; its cuts are at the file's own
seams — a type cluster, a section banner, a trait and its implementors — never
at a line count; and it carries every marker, citation and script path with
the code it moves.

The first exemption is `datetime_in.rs`: a transliteration of upstream's
`datetime.c`, sectioned as the original is, where a cut would un-mirror the
port. P33 grows it, and the exemption is reconsidered there.

## What this phase does not do

- **Change behaviour.** A move is whole; a signature changes only where a
  crate boundary forces a type public.
- **Publish a `pgtypes` crate.** The codec crate makes one possible — its
  Arrow mapping is one module and its catalog types come from the model — but
  a published crate needs its own catalog types and Arrow behind a feature,
  and D104 says no crate is published. That is a product decision for its own
  phase.
- **Split `datetime_in.rs`**, or any file an exemption names.
- **Retune the limits.** They are a first pass; tightening is a later edit to
  the script and its tests, by the maintainer.

## Slices, in the order their evidence demands

Evidence first: the limits script and its baseline before anything moves.
Tests out of the big files before they are split, so the split reviews
production code alone. The cycles inside the one crate broken, and the
layering test rewritten to the crate assignment, before any `Cargo.toml`
exists — the test passing is the proof the graph is acyclic. Then the carve,
bottom up, behind the facade; then the facade retired and the record
repointed; then the seams round over what is still red.
