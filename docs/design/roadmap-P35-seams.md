# P35 — Crate seams and file limits

The library becomes six crates whose dependency graph Cargo enforces, each
crate's module tree is redrawn as a design in which no file is past a limit,
and a check and a skill keep both true afterwards. No behaviour changes:
every test that passes before a slice passes after it, unmoved or moved whole.

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
longer re-reads the others, and a test binary moved down links less. **No
reading of it is taken**: no outcome would undo the split, and a before and
after on two commits prices everything between them. A build-time figure
earns its apparatus when the loop becomes the complaint.

**Runtime is a cost the split can carry unseen.** One crate lets the compiler
inline across every module; six stop it at each boundary for a function that
is neither generic nor `#[inline]`, and the per-field and per-row calls —
query into codec and core — are where that lands. The release build states no
profile, so nothing restores it, and nothing has measured it. The phase owes a
reading of what the split costs at run time. A regression's remedy is in the
code, never the profile alone: an embedder builds without our profile, so the
default leg is held within 3% of the shipped one ([`roadmap.md`](roadmap.md),
"A downstream build is within 3% of ours"), and the shipped profile is its own
decision with its own `D<k>` and figure, never a slice's quiet edit.

The reading is a `release-profile` figure built as `allocator` is: legs at
Cargo's default, thin LTO, and fat LTO with one codegen unit, each built from
the tree measured, so no sitting subtracts one commit from another. It sits
once on the one-crate tree and once on the split one; **the split cost
nothing recoverable if the default leg's gap to fat LTO is no wider after the
carve than before, within the figure's noise**. The shipped build's profile
is decided from the two sittings whatever they show, the first pricing
today's binary as much as the split, and a small trade between shapes by
[`roadmap.md`](roadmap.md), "Where a setting trades one workload for another,
the typed DataFusion query wins". It times the `allocator` figure's three
shapes — `parse`, and `query` untyped and typed, warm over the control file —
which hold the per-row split and the per-field decode the split crosses. Each
leg is a named Cargo profile inheriting `release`, built into its own target
dir, and `pgdt --version` names it, a binary that cannot say which leg it is
refused as an allocator leg is; `profiling` inherits whatever `release`
becomes, so a sampling profile is always of the shipped build.

## The crate graph

Six library crates, each depending only on those above it in this list; the
layers of D68 become crates, with one new one below the parsers.

- **`pgdt-core`** — the error type, the `introspect` hooks, the SQL lexer and
  the COPY text format: what every other crate reads and nothing else owns.
  **The error type names no dependency's error**: it is the library's
  vocabulary of failure, and a dependency's error — Arrow's, xz-seek's,
  bincode's, tokio's — is a cause it carries opaquely, reached by downcast,
  never a variant that would compile that dependency into every crate.
- **`pgdt-model`** — the dump's vocabulary: spans, blocks, the schema the
  preamble declares, block statistics, diagnostics. **This is the cache's
  format**, `CACHE_FORMAT_VERSION` beside it, and `bump-cache-format` points
  here. Data, and the contracts over a
  block's rows that the parsers call and a layer above implements
  (`FieldCount`, `BlockObserver`) — a contract over the data is model wherever
  its implementor sits; no I/O, no Arrow.
- **`pgdt-io`** — byte sources (local, `.xz`, fetched, remote), the memory
  limit and its discovery, worker sizing. The `http` feature lives here.
- **`pgdt-dump`** — the parsers and the cache: the scanner, the preamble
  grammar, the map builder, the interior split, persisting and reloading the
  model. Depends on core, model, io.
- **`pgdt-codec`** — PostgreSQL values: every decoder, the datetime grammar,
  nested literals, the type register, the comparison plans and their ordering
  keys, column resolution. Depends on core and model; names `arrow-schema`,
  and `arrow-buffer` for `i256` alone, and is synchronous. The ordering keys move here from the query layer,
  where they sit today in `predicate.rs`: they are value semantics, not
  filtering.
- **`pgdt-query`** — Arrow batches, the streams, the filter expression,
  statistics gathering and pruning, summaries. Everything above it is an
  embedder: `datafusion-pgdump`, `datafusion-cli-pgdump`, `pgdt`.

What the graph settles: D74's two rules — the parsing crates compile no Arrow,
the codec reaches `arrow::datatypes` and no further and does no I/O — become
dependency lists rather than greps. Cargo refuses a cycle but not an edge the
graph does not draw, so once the facade is gone a script reads the graph
from `cargo metadata` and holds every member's dependencies to the list
above, the codec synchronous and naming no `arrow_buffer` item but `i256`,
no embedder naming an `internal` path or
`timed!` or `row_evaluated!`, which Rust roots at their crate whatever module
declares them, no library module loaded by `#[path]`, and no namespaced API
item re-exported at its crate's root too. Why `pgdt-model` exists: without it the
codec sits above the preamble grammar, so a change to how `CREATE TABLE` is
read recompiles every decoder; with it the codec and the parsers are siblings.
Why `pgdt-core` is not folded into it: the model is exactly what the cache
persists, so an edit there is the edit that asks whether the format bumps, and
that test holds only while the lexer and the error type live elsewhere.
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
apparatus, not library, and keep their names. A dependency's version is
stated once, in `[workspace.dependencies]`, every member naming it by
`workspace = true`. Every member moves under
`crates/`; `vendor/xz-seek` stays where the vendoring script puts it.

During the carve, `pgdump_query` stays as a facade re-exporting what the new
crates define, so the test binaries, `pgdt` and the DataFusion crates keep
building slice by slice; the last carving slice retires it. Pre-1.0 means the
facade owes nobody its survival ([`roadmap.md`](roadmap.md), "Pre-1.0"), and
none is kept as a public face: each crate's `internal` module already marks
its API, a crate re-exporting the rest would spell every public name twice,
and nothing is published (D104). P24's bindings make their own entry point.

## The file limits and the skill

A file is measured as production code lines, production functions and total
lines, its inline tests counted only in the last; the numbers are stated once,
in `scripts/file_limits.py`, and the rule — soft limits that signal, hard ones
that oblige, an exemption table carrying a reason per file — is
[`../process.md`](../process.md), "Seams". The check is a meter in `mise run
check`, never a failure: a slice is sized by its review, and what it leaves
long is the next seams round's.

`.claude/skills/seams/SKILL.md` is that round. Its first cut in any file is
the inline tests to a file beside the module. **A red file indicts the tree
around it, not just itself**: the round designs the module tree its crate now
wants — each concept in one place above all, each file one concern, nested
where siblings form a group, no file past a limit, tests beside what they
test — and cuts toward that design, at a type cluster, a trait and its
implementors, one arm of a family, never at a line count. Cutting the one file at its banners and stopping leaves a layout
nobody designed; the skill's "Antipatterns" names what that looks like. Every
marker, citation and script path moves with the code.

The first exemption is `datetime_in.rs`: a transliteration of upstream's
`datetime.c`, sectioned as the original is, where a cut would un-mirror the
port. P33 grows it, and the exemption is reconsidered there.

## What this phase does not do

- **Change behaviour.** A move is whole; a signature changes only where a
  crate boundary forces a type public. **What a boundary forces public that is
  not API goes in that crate's `internal` module**, so a caller reaching past
  the API names the word, and `datafusion-pgdump`, `datafusion-cli-pgdump`
  and `pgdt` never do — a check, not a convention. So no other path reaches
  it: every library module is private but an API namespace an embedder
  would name (`cache`, `decode`, `instrument`, …), which is its file's own
  module made public, **declaring** the API — its doc and the `pub use` of
  each API item from the private child that defines it, a child holding one
  whole concern, API and not alike — so no file loads by `#[path]` under a
  second name and no API item has a second public path; and a method a crate above calls that is not API is a method
  of an extension trait `internal` exports, its body unmoved. **A crate's API
  is a designed surface**: what an embedder of that crate would call, given
  what its `lib.rs` says the crate is for, whether or not one does today —
  the `COPY` text format is `pgdt-core`'s, the nested literal codec
  `pgdt-codec`'s beside `decode`. Neither what was `pub` before the carve,
  which the one crate's integration tests forced, nor what an embedder names
  today decides it. **An API item has one path**: a namespaced item is not
  also at its crate's root. `internal` is
  per crate: a crate above may adopt a lower crate's `internal` item into its
  own API by re-exporting it, where the item must be defined lower but means
  something only above (`OVERWRITE_WAYS_OUT`, the cache's remedy text).
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
exists — the test passing is the proof the graph is acyclic. The profile
figure's first sitting; then the carve, bottom up, behind the facade; then the
figure's second sitting and the profile decided; then every forced-public item
put where only `internal` reaches it, so the check is written against a tree
it can hold; then the facade retired and the record repointed; then the seams
round over what is still red. That round cut only the files past a hard limit,
at their own banners and by visibility, which left most crates' trees
undesigned; so the trees are designed next, every crate at once and with no
code moved, the taste calls settled with the maintainer, because the trees are
one design — a test or helper may belong a crate away from where it sits — and
a design per slice would redraw what the last one drew. Then the cuts, bottom
up, a crate or a half of one a slice, and a closing round that finds nothing
past any limit.
