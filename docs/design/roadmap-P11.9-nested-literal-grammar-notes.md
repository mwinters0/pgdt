# P11.9 — The nested literal input grammar

What 11.10 inherits. The spec is
[`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md); how the
mechanism works now is [`architecture.md`](architecture.md), "The nested
literal codec", and the external evidence is I44. This doc is the part that is
neither — the calls that are not obvious from the code, and the two facts the
probing turned up.

## What landed

Four `pub fn` in `nested.rs`, beside the four `decode_*` they mirror:

- `parse_array` — `ReadArrayDimensions`/`ReadDimensionInt`/`ReadArrayStr`/
  `ReadArrayToken`, transcribed from v18.
- `parse_record(s, columns)` — `record_in`, arity included.
- `parse_range` — `range_parse`/`range_parse_bound`.
- `parse_multirange` — `multirange_in`'s state machine, members delegated to
  `parse_range`.

Plus I44, ten unit tests in `nested.rs`, and the acceptance walk in
`tests/nested.rs`'s new `oracle` module. **No comparison**: nothing in
`predicate.rs` calls any of them yet, which is the slice's contract, so the
row set of every query is byte-identical to before.

## The four calls worth knowing

**`parse_record` takes the field count and `decode_record` does not**, which
looks like an inconsistency and is the invariant. `record_in` reads exactly the
composite's declared columns and calls anything else "Too few columns" or "Too
many columns" — so `()` is a value at arity 0 and 1 and a fault at every other
arity, and `(1,a,b)` is a fault only because `point2d` has two fields. The
*output* side has no such question to ask: `record_out` wrote whatever it
wrote, and `decode_record`'s doc comment already says arity is the caller's
join. 11.10 has the field list in `NestedPlan::Record`, so the argument is
free there.

**The transcription is v18's, not v16's, and that is the union rule.** v17
rewrote `array_in` from a validate-then-extract pair into a single-pass token
reader, and the rewrite accepts one thing more: a `}` closing an **empty
sub-array**, so `{{},{}}` is `22P02` on 13–16 and `{}` on 17–18. Everything
else is presentational. Implementing the newer grammar means pgdq accepts a
literal a v13 server would have refused, which is the safe direction — the
value cannot exist in a v13 dump, so no comparison can be wrong about it.

**Under-acceptance is chosen where reproducing the server exactly is not worth
it.** A dimension bound goes through `strtol` on the server, so `[1abc:2]`
scans `1` and then fails on the missing `]`; a strict sign-plus-digits parse
gets to the same refusal by a different road, and where the two could differ
(`atoi`-era slop) pgdq refuses. That is the direction the spec permits: taking
a literal the server would refuse is a divergence nothing else here allows,
while refusing one it would take costs an error message.

**One `is_space` for all four grammars**, because `array_isspace` (through
v16), `scanner_isspace` (v17+) and the C locale's `isspace` that `record_in`,
`range_in` and `multirange_in` call are the same six characters. The predicate
was already in the module for the force-quote rule and needed no second
version. What is *not* shared is where it is applied: the array drops
whitespace around an element and the other two keep every byte, which is the
first place the grammars visibly disagree and the reason they are four
functions rather than one parameterized one.

## What 11.10 inherits

**A parsed literal is the parts as the user spelled them.** `parse_array("{ 1
, 2 }")` gives elements `"1"` and `"2"`; `parse_record("( 1 , a )", 2)` gives
`" 1 "` and `" a "`. The server's stored value for the second is `(1," a ")` —
`int4in` threw the first field's blanks away, not `record_in`. So **11.10's
element-wise comparison must canonicalize per element through the element
type's own plan**, not compare the parsed strings; the container grammar has
done all it can.

**Three canonicalizations are missing and all three need the subtype's order**,
which is exactly what 11.10 adds:

- A discrete range's bounds — `int4range '[1,10]'` is `[1,11)` — through the
  subtype's successor function.
- A range whose bounds are out of order is `22000`, not a grammar fault:
  `int4range '[10,1)'` parses here and the server refuses it.
- A multirange's members are sorted, coalesced and empty-dropped
  (`{[5,6),[1,2)}` → `{[1,2),[5,6)}`, `{[1,3),[2,5)}` → `{[1,5)}`, `{[1,1)}` →
  `{}`). `parse_multirange` returns the members as written, dropping only a
  member spelled literally `empty`.

The acceptance walk records all four of the first kind and the one of the
second as **exact sets** (`CANONICALIZED`, `SEMANTIC_REFUSALS`), so 11.10
closing one has to delete its entry, and 11.10 closing none leaves them
standing as the statement of what is still owed.

**The walk classifies by resolution, not by a hand-kept list.**
`parser()` calls `resolve_declared_type` and switches on the `NestedPlan`, so
an oracle case whose type changes shape is re-classified rather than
mis-parsed. The set it produces is asserted against a committed `NESTED`
constant, whose one deliberate absence is `public.intarr[]`: an array whose
element is an array (I26) resolves to `NestedArrayElement` and compares as text
(`KD3`), so the parser will never be handed one — even though it reads them
(`nested.rs` carries the shape as a unit test).

**`public.empty_comp` is the arity check's only zero-field case**, and it is
sharper than it looks: `()` is the value, `( )` is a fault, because `record_in`
skips leading whitespace *before* the `(` and trailing whitespace *after* the
`)` and nowhere in between. The oracle carries `(1)` as its malformed row;
`( )` is a unit test, from the probe.

## Where the expectations came from

Every hand-written expectation in `nested.rs`'s new unit tests was put to a
live server before it was written down — `postgres:16.15-trixie` and
`postgres:18.6-trixie`, one `SELECT textin(<typoutput>($1::<type>))` per
literal inside a PL/pgSQL block returning `'E' || SQLSTATE`, which is the shape
`scripts/comparison_oracle.py` already uses. I44's `Re-verify` carries a
trimmed version of that probe, so the same question is re-askable at the next
major without reconstructing the apparatus.

That probe is also where `{{},{}}` came from. Reading `arrayfuncs.c` at two
majors said the rewrite *should* differ there; the server is what says it does.
