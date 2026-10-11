---
name: seams
description: Redraw seams where the code has outgrown its files or crates — run `scripts/file_limits.py`, move inline tests beside their module, design the module tree a red file's crate now wants and cut toward it, and say when a crate should split or two merge. Use when the user invokes /seams, when the `seams` meter is red in `mise run check`, before a phase wrap, or when a slice leaves a file over a limit.
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
compared with today's. Splitting the red file at its banners and stopping is
the failure; a flat list of a dozen children cut from one file is its
commonest shape. The test of a design is a reader who knows the domain and
not the code: from the names and module docs alone, can they say where a new
feature goes and where a given item lives?

The evidence for where seams fall, in order of weight:

- **a type cluster** — a struct or enum, its impls and the free functions only
  they call, which `graft callers <type> --direction out` shows closed;
- **a trait and its implementors**, or **one arm of a family** — one source
  kind, one type family's decoders, one subcommand;
- **the phase of a pipeline** — building, observing, closing;
- **a section banner** — a `// ----` rule or a `//!`-style heading the author
  left, whose span is one concern. Weakest: it records where the file grew,
  not where the design divides.

**The marks of an undesigned tree** are evidence, read for the scope before
the design and again after the cuts: two siblings importing each other
(`rg -n 'use (super|crate)::' <module>/`) — one concern cut in two, or a third
they share; a child's `use super::` reaching its parent's private items; a
file's `pub(super)` count (`rg -c 'pub\(super\)' <module>/`); one concept's
items in several files, or a module named for its visibility (`refusal`,
`internal`-only) or its caller rather than what it holds. None is always
wrong — a dispatcher and its feature-gated arm import each other — so each is
answered in the design, never counted against a threshold.

Never at a line count, and never splitting one type's impls across files. A
module that becomes a directory keeps `<module>.rs` holding the `mod` lines,
the module doc, and the `pub use` re-exports that keep every external path
unchanged; a nesting the design calls for may move a path, which pre-1.0
allows. The doc moves with the items it describes; a file whose doc is left
describing what moved out is a defect, and so is an item made `pub(super)`
only because a cut ran through its cluster. Tests follow their module: a
production file cut in five leaves no one tests file holding all five.

A design larger than one review lands over several slices, each a cut the
design contains; the notes doc holds the design until the last lands.

**What moves with the code.** Every marker on it — `deficiency: KD<k>`,
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

Either answer is a **decision**, not a cut: it reverses or extends the entry
that drew the graph, so it goes to the maintainer with the evidence, or under
`STATUS.md`'s "Decisions worth another look" unattended. The skill names when
one is due; it does not carve the crate.

## 5. Exempt, with the reason

A file a cut would make worse — a port mirroring an upstream file's sections,
a grammar whose cases are one table, a seam that would be arbitrary — goes in
`file_limits.py`'s `EXEMPT` with that reason, and the reason names what would
lift it. An exemption is a debt the check reads as live: one for a file that
has come under the limit fails the check and is struck.

## 6. Finish

Run `cargo fmt`, then `mise run check` — whole, never `--affected`, since a
move touches paths no test reads — and `uv run citations.py`. One commit a
cut, subject `seams: <module> → <what it became>` or `seams: <module>'s tests
beside it`, body listing each new file and what it holds. The notes doc of
the slice that ran the round, or the day's history entry out of band, holds
the design, what of it has landed, and what is still red and why it waited.

## Driven unattended

A cut is behaviour-preserving by construction and may be landed; the design
it cuts toward is a judgement, so it goes under "Decisions worth another
look" with the tree it replaced, and a crate question may not be answered at
all. List that with both pieces of evidence, and stop at the cut before it.
