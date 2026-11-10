# P33.23 notes — the scalar readers back on slices

**The workspace's `crates/` are `d33fa096`'s again**, 33.13.3's tree, byte for
byte: nothing touched them between `f799618c` and the re-target, so the revert
is exact and no hand merge was made. `field_key`, a query's batch and a strict
check read an integer, `oid`, `boolean`, enum, `uuid` and `numeric` with the
slice readers 33.13.4 and 33.13.5 replaced (`int_in`, `oid_in`, `decode_bool`,
`decode_uuid`, `numeric_in_stores` and its spellings), and a strict check
collects such a part's text whole (`grammar/walk.rs`, `Check::Scalar`) and
unescapes such a column's field whole, as the spec's
"A pass holds a line only where it decodes it" states. D147 and D148 are struck;
D144 names the streamed readers and rejects a machine per keyed scalar; D143's
holding clause and the invariants' "Relied on by" lines are `d33fa096`'s.

Gone with the machines: `key/order/differential.rs`, `key/order/prior_shape.rs`,
`value/numeric/input.rs`, `value/numeric/prior_shape.rs`, and
`tests/decoded_copies.rs`'s `a_strict_check_holds_no_text_of_a_scalar_it_keys`
and `a_strict_check_holds_no_text_of_a_numeric`. No count pair was taken, by the
maintainer's call ([`../status/history/2026-11-10.md`](../status/history/2026-11-10.md)):
33.22 placed both rises in the machines this removes
([`roadmap-P33.22-priced-notes.md`](roadmap-P33.22-priced-notes.md)).

## What 33.19 and 33.4 inherit

- **A keyed scalar is the text a parse collects**: a strict check holds an
  integer, `oid`, `boolean`, enum, `uuid` or `numeric` part, or such a column's
  unescaped field, whole until its key is read, as it holds a float, datetime,
  `interval` or network address and a range's bounds. 33.19's fold collects it
  and 33.4's ledger draws for it past its chunk term.
- **`walks`** (`grammar/check.rs`) again names a column by its comparison plan
  alone — a nested one, `jsonb`, `bytea`, a text, one compared as text — beside
  a `TextGrammar`'s, so a strict parse unescapes every other column's field.
