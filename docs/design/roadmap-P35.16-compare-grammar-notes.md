# P35.16 notes — the comparison register and the strict parse's grammar

The cut toward [`roadmap-P35.11-trees-notes.md`](roadmap-P35.11-trees-notes.md)'s
"pgdt-codec" tree for its `compare` and `grammar` modules. Every body moved
whole; nothing persisted changed, so no format bump.

## Where the cut fell

- **`compare.rs` is the register**, a public namespace: `CompareKind` to
  `ComparisonPlan`, `ComparisonSemantics`, `UnanswerableReason`,
  `bounds_set_keyed_by`, the collation helpers and the walk
  (`comparison_for` to `comparison_user_type`). `pgtype.rs` keeps the
  mapping, `Position`, the typmod types and the walk helpers both walks
  share, now `pub(crate)`.
- **`builtin_scalar` and `builtin_range_subtype` stay in `pgtype.rs`**, the
  design's "builtins": one table answers the Arrow type and the comparison
  (D40), and `floor_mapping.py` reads it there. `oracle_register.py` reads
  its arms out of `compare.rs` and `pgtype.rs` as one text, `REGISTER`
  naming both.
- **`discrete` and `divergence` are folded into `compare.rs` as inline
  `pub(crate) mod`s**, each beside its kin — `DiscreteExt` after `Discrete`,
  `datafusion_position_divergences` after `CompareKind::datafusion_divergence`.
  Defined at `compare`'s top level they would have had a public path beside
  `internal`'s, which the spec forbids a non-API item ("What this phase does
  not do"); a child file is the split by visibility the fold removes.
  `internal`'s paths are `compare::discrete` and `compare::divergence`.
- **`grammar.rs` is private**, as designed: `TextGrammar`, `Geometric`,
  `text_grammar` and `Unchecked` from `pgtype.rs`, `column_grammar`,
  `unchecked_positions` and their helpers from `pgtype/strict.rs`, and
  `field_refused`, `grammar_refuses`, `accepted_form` and
  `nested_accepted_form`, with the enum and numeric clauses, from
  `key/order.rs`. That last section is the one of `key/order.rs`'s six the
  design sends to `grammar.rs`, so it moved with the module rather than in
  35.17; `field_key` and `checked_key` stay in `key/order.rs`, `pub(crate)`
  for it. **`compare` declares the three API types** (`pub use`), the
  register's plans carrying the first two and a strict parse's listing the
  third; that call is under STATUS's "Decisions worth another look".
- **Tests beside what they test.** `pgtype/tests.rs` kept every test driving
  `resolve_declared_type`, those pinning a type and its comparison together
  (`int2vector`, a quoted name, a `NOT NULL` domain's positions, the
  bounded walk) included; `compare/tests.rs` took the register's section
  and the quoted-collation test; `grammar/tests.rs` took the four
  `text_grammar` and `unchecked_positions` tests and, from
  `key/order/tests.rs`, the `field_refused`, `grammar_refuses` and
  `accepted_form` ones. The crate's suite is the same 172 tests.
- **Above the codec only paths moved**: `pgtype::<register item>` became
  `compare::<item>` in every crate, `layering.py`'s `NAMESPACES` lists
  `compare`, `measure.py`'s `PGTYPE` declares `compare.rs` and `grammar.rs`
  in place of the three child files, and the `KD7`, `KD10`, `KD40` and
  `KD48` markers moved to `compare.rs` with their index lines.

## What later cuts inherit

- **Three edges run both ways** where the design drew one way.
  `compare` ↔ `pgtype`: the built-in table builds plans and the register
  walks with `pgtype`'s helpers. `compare` ↔ `grammar`: the design has
  `compare` import `TextGrammar` "and nothing imports back", but
  `column_grammar`, `unchecked_positions` and `field_refused` read a
  `ComparisonPlan`. `grammar` ↔ `key::order`: `checked_key` refuses an
  uncomparable position by `grammar_refuses`, which reads a `box` array
  through `checked_key`. 35.17's families may dissolve the third; the first
  two are placements for 35.12's closing read to weigh.
