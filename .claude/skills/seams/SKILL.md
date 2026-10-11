---
name: seams
description: Redraw seams where the code has outgrown its files or crates — run `scripts/file_limits.py`, move inline tests beside their module, design the next version of the concept map (`docs/design/concepts.md`) across crates and each crate's tree from it, cut toward it, and say when a crate should split, two merge, or a concept's part move crate. Use when the user invokes /seams, when the `seams` meter is red in `mise run check`, before a phase wrap, or when a slice leaves a file over a limit.
---

The rule is `docs/process.md`, "Seams"; **invoke the `process` skill first**.
This is the procedure. Nothing here changes what the code does: every test
that passes before a cut passes after it, unmoved or moved whole, and a
signature changes only where a module boundary forces an item `pub(crate)`.

## 1. Read the report

Run `cd scripts && uv run file_limits.py`. Keep every `!!` and `..` line and
every `==` exemption. A hard line puts **its module tree** in this round's
scope — the crate it sits in, or the top-level module when the crate is large
— and every soft line inside that scope comes with it. `--all` prints every
file's reading; read it for the whole scope, since the design is of the tree.

Then read `docs/design/concepts.md` for every concept the scope's files hold,
**and every crate each one reaches**: a concept is designed once, across the
crates holding it, so its other crates' modules are in scope too.

## 2. Tests out first

A file over a limit whose tests are inline gets this cut before any other:
each `#[cfg(test)] mod name { … }` becomes `#[cfg(test)] mod name;` and the
body moves whole to `<module>/name.rs` (`src/name.rs` beside a crate root),
indentation left to `cargo fmt`, never dedented by hand — a multi-line string
literal would change. Then hunt what named the test by its old path: a script
locating a test function by file (`rg -n 'fn <name>\(' scripts/`), a doc
citing `<module>.rs`'s unit test, and repoint it in the same change.

## 3. Design the tree, then cut toward it

A red file says the seams around it have stopped fitting (`docs/process.md`,
"Seams"), so the round's first product is **a design, not a cut**: the module
tree the scope's code now wants, written out — each module with a one-line
purpose, nesting where siblings form a group, no file past a limit — and
compared with today's. **What a design is for, above everything else, is that
each concept lives in one place**: everything a reader needs to understand or
change one concept is in one module. That is what people look for in a code
layout, and every other property here serves it. The test
is a reader who knows the domain and not the code: from the names and module
docs alone, can they say where a concept lives, and where a new feature goes?

**The design is the concept map's next version**: each concept's claims as the
design draws them, and no **Not in one place** line it leaves standing. Draw it
across crates first, then each crate's tree. A concept may span crates only
along the layering (`docs/process.md`, "Seams"), each part saying why its crate
holds it; a part with no such reason moves to the crate the layering names, and
a whole module changing crate edits `docs/design/decisions.md`, "D68", so it
goes to the maintainer as §4's answers do.

The evidence for where seams fall, in order of weight:

- **a type cluster** — a struct or enum, its impls and the free functions only
  they call, which `graft callers <type> --direction out` shows closed;
- **a trait and its implementors**, or **one arm of a family** — one source
  kind, one type family's decoders, one subcommand;
- **the phase of a pipeline** — building, observing, closing;
- **a section banner** — a `// ----` rule or a `//!`-style heading the author
  left, whose span is one concern. Weakest: it records where the file grew,
  not where the design divides.

Never at a line count, and never splitting one type's impls across files. A
module that becomes a directory keeps `<module>.rs` holding the `mod` lines,
the module doc, and the `pub use` re-exports that keep every external path
unchanged; a nesting the design calls for may move a path, which pre-1.0
allows. The doc moves with the items it describes, and tests follow their
module.

A design larger than one review lands over several slices, each a cut the
design contains; the notes doc holds the design until the last lands.

### Antipatterns

Each is what a cut leaves when it serves one file rather than the tree. Read
the scope for them before the design and again after the cuts. None is counted
against a threshold: each one found is removed by the design or kept with its
reason (a dispatcher and its feature-gated arm do import each other).

- **One concept, several files** — the antipattern that matters most, every
  other one below being a way of arriving at it or a symptom of it. A type
  family's reader, renderer and refusal check, or one grammar, spread over
  modules — or over crates where the layering does not divide it, a library
  and the binary that uses it among them. The map's **Not in one place**
  lines list the ones known.
- **The banner split.** One file cut at its section banners into a flat list
  of siblings, and the round stops. The banners record where the file grew;
  groups among the siblings go unnested and the tree goes undesigned.
- **The scope of one file.** Only the red file is cut while its neighbours
  share its concepts, or only the hard-red files while soft-red ones in the
  same tree wait.
- **The cut by visibility.** What is private, or what `internal` re-exports,
  moved out as a unit instead of a concern, so each family ends up half in
  the parent and half in a child.
- **The half-split.** Part of a concern moved out and the rest left, so the new
  module calls back into the one it left (`crate::` into a binary's root).
- **Sibling cycles.** Two children importing each other (`rg -n 'use
  (super|crate)::' <module>/`): one concern cut in two, or a third they share
  that neither should own.
- **The child reaching up.** A child's `use super::` naming its parent's
  private items: the parent still holds part of the child's concern.
- **Visibility widened to compile.** `pub(super)` or `pub(crate)` added only
  because a cut ran through a cluster (`rg -c 'pub\(super\)' <module>/`); the
  right seam leaves most items as private as they were.
- **Stranded helpers.** A helper left where its banner fell, used only by a
  sibling; a contract between a producer and its readers kept inside the
  producer, so the readers import from its internals.
- **The grab bag.** A child whose doc lists unrelated things joined by "and",
  or one defined as "everything but X".
- **A name that is not the content.** A module named for its visibility
  (`refusal`), its caller (`leader`) or a verb it does not do (a `load` that
  loads nothing); one word naming two concepts in one crate.
- **The stale doc.** A module doc still describing what moved out of it, or
  scoped narrower than what the module now holds.
- **Monolithic tests.** Production cut in five, one tests file holding all
  five, reaching every child's private items and forcing them wider.
- **Carve residue.** Nesting that served an older layout — a crate whose
  whole body is one module named for the crate, two re-export lists for one
  API.
- **A size justified by a rule.** A file left large because a layout rule seems
  to require it ("what is left is its API"). A rule is read for its goal;
  where it blocks the design, it goes to the maintainer, not the file's size.

**What moves with the code.** Its claim in `docs/design/concepts.md`, and the
**Not in one place** line the cut settles, struck (`uv run concepts.py`).
Every marker on it — `deficiency: KD<k>`,
`upstream: UF<k>`, `pg-refuses: I<n>` — and the register line naming the
file: run `uv run deficiencies.py`, `uv run upstream.py`, `uv run
pg_refuses.py`. Every script that reads the file by path (`rg -n
'<module>\.rs' scripts/`): `emitter_register.py`, `oracle_register.py`,
`floor_mapping.py` each name readers by file. Every citation by filename in
`docs/` and `README.md` (`rg -n '`<module>\.rs`' docs/ README.md`), repointed
to the file that now holds what the sentence names. A module's crate is the
directory it lives under, and `scripts/layering.py` holds each crate's
dependencies to D68's graph: a cut inside a crate leaves both alone.

## 4. A crate is a seam too

The workspace graph is the current answer, not a fixed one. Before closing,
ask it two questions, by evidence rather than taste:

- **Should a crate split?** Its modules fall into two groups with few edges
  between them (`graft callers <module> --depth all` from each side), one
  group changing in most commits that leave the other alone (`git log
  --stat`), and a dependency the smaller group would not need.
- **Should two merge?** Most commits touching one touch the other, or the
  boundary forces a type `pub` that nothing outside the pair reads.
- **Is a concept's part in the wrong crate?** The layering gives no reason for
  it to be there, most of its callers sit in another crate, or an older layout
  is the only reason it is where it is. Items moving between modules D68
  already assigns to the right crate are a cut; a module changing crate is
  this decision.

Either answer is a **decision**, not a cut: it reverses or extends the entry
that drew the graph, so it goes to the maintainer with the evidence, or under
`STATUS.md`'s "Decisions worth another look" unattended. The skill names when
one is due; it does not carve the crate.

## 5. Exempt, with the reason

A file whose length overstates its breadth (`docs/process.md`, "Seams") — one
narrow concept a cut would only scatter, as a port mirroring an upstream
file's sections, or a grammar whose cases are one table — goes in
`file_limits.py`'s `EXEMPT` with that reason, and the reason names what would
lift it. An exemption is a debt the check reads as live: one for a file that
has come under the limit fails the check and is struck.

## 6. Finish

Run `cargo fmt`, then `mise run check` — whole, never `--affected`, since a
move touches paths no test reads — and `uv run citations.py` and `uv run
concepts.py`. One commit a
cut, subject `seams: <module> → <what it became>` or `seams: <module>'s tests
beside it`, body listing each new file and what it holds. The notes doc of
the slice that ran the round, or the day's history entry out of band, holds
the design, what of it has landed, and what is still red and why it waited.

## Driven unattended

A cut is behaviour-preserving by construction and may be landed; the design
it cuts toward is a judgement, so it goes under "Decisions worth another
look" with the tree it replaced, and a crate question may not be answered at
all. List that with both pieces of evidence, and stop at the cut before it.
