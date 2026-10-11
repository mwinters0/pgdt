# P35.25 notes — `grammar`, an API namespace

The cut toward [`roadmap-P35.11-trees-notes.md`](roadmap-P35.11-trees-notes.md)'s
"pgdt-codec" tree for `grammar`, as amended
(`../status/history/2026-10-11.md`, "The strict parse's API types, declared by
`grammar`"). Every body moved whole; nothing persisted changed, so no format
bump. `module_graph.py` finds no new cycle: `grammar/input.rs` reads only
`declared`, so `compare`'s import of `TextGrammar` runs one way, as the
design drew it, while `grammar/check.rs` still reads `compare`'s plans.

## Where the cut fell

- **`grammar.rs` declares and defines nothing**: its doc and the `pub use`
  of `TextGrammar` and `Geometric` from `grammar/input.rs` and of
  `Unchecked` from `grammar/check.rs`. `pgdt_codec::grammar::<type>` is
  each one's only public path; `compare` imports `TextGrammar` privately for
  its field.
- **`text_grammar` stays `pub(crate)` in `grammar/input.rs`**, named by its
  full path (`grammar::input::text_grammar`) from `compare/walk.rs`,
  `grammar/check.rs` and `pgtype/tests.rs`, as `check`'s functions are by
  theirs; the namespace re-exports API only. The invariants naming it
  (I8, I70, I73, I74, I78, I79) were repointed to that path.
- **`grammar/tests.rs` is `grammar/input/tests.rs`**, renamed whole: its
  three tests all drive `text_grammar`.
- **Above the codec only paths moved**: `pgdt-mapping`'s
  `statistics/gather/strict.rs` and `pgdt-query`'s `tests/preamble.rs` name
  `grammar::` where they named `compare::`. `layering.py`'s `NAMESPACES` and
  its tests' fixture tree list `grammar`; `measure.py`'s `PGTYPE` declares
  `grammar/input.rs`.

## What later cuts inherit

- **pgdt-codec's API namespaces are now six**, the last the design names:
  35.12's read for antipatterns sees the codec's final surface.
