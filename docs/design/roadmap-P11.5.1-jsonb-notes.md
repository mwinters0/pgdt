# P11.5.1 — `jsonb`

What the next slices inherit. How the register *works* is
[`architecture.md`](architecture.md), "Ordering operators compare typed"; the
external facts are [`postgres-invariants.md`](postgres-invariants.md)'s I41.
This doc is the part that is neither — the calls that are not obvious from the
diff, and what 11.6 and 11.10 pick up.

## What landed

- **`CompareKind::Jsonb`**, and `jsonb` off `ComparisonPlan::AS_TEXT`, which
  now has exactly one member: `json`.
- **`OrderingDivergence::JsonbStringCollation`**, a fifth variant, and a
  sentence that is not the `bytewise(...)` one — the structure *is* compared
  PostgreSQL's way and only a string leaf is left.
- **`Jsonb`, `JsonCursor` and `storage_order` in `predicate.rs`**, plus
  `OrderKey::Jsonb`. A recursive-descent `jsonb_in` and a recursion standing in
  for `compareJsonbContainers`' lockstep token walk.
- **`NumericKey::from_parts`**, factored out of `NumericKey::parse` so the JSON
  number path and the bare-`numeric` path normalize through one function.
- **I41 added.** The manual's `json`/`jsonb` paragraph becomes a section of its
  own, and its "six string-shaped types" section becomes seven.

## Calls worth knowing about

**The comparison is written as a recursion and the source is a lockstep walk
over two token streams; they agree, and the argument is worth keeping.** Both
stop at the first position where the documents differ, and up to that position
the two streams are identical — a kind mismatch anywhere is the type-defined
order, and a container's size is settled at its opening token before any member
is looked at. So the recursion's first difference *is* the walk's. What the
recursion buys is that the object case can be written as "keys then values,
pair by pair" instead of as a `WJB_KEY`/`WJB_VALUE` interleaving whose
desynchronisation has to be reasoned about separately.

**The raw-scalar wrapper is modelled, not special-cased**, and that is what
produces `'1'::jsonb > '[]'::jsonb`. `compareJsonbContainers` reads the
`rawScalar` flag *and then lets the element count overwrite what it concluded*,
so a top-level scalar sorts below `[1]` and above `[]`. `jsonb_key` wraps a
scalar in `Jsonb::Array { raw_scalar: true, items: vec![scalar] }` and the arm
is written in the source's own order, overwrite included. A "scalars sort below
every array" rule would be simpler and wrong on one input. v18 added a comment
to that arm calling it *a mild anomaly* and saying the sort order cannot be
changed now, which is upstream freezing it (I41).

**An object carries two different orders and both are load-bearing.** Pairs are
*stored* by `lengthCompareJsonbString` — key length first, then `memcmp` — and
*compared* by `varstr_cmp`. `storage_order` does the first; `Jsonb::cmp` does
the second. A model that sorted the keys alphabetically would answer
`{"z":1,"aa":2}` against `{"y":3,"zz":4}` backwards, which is the assertion
`jsonb_object_pairs_are_walked_in_storage_order` exists for.

**Last duplicate wins, and it comes out of a `reverse` before a stable sort.**
`lengthCompareJsonbPair` breaks a key tie on *descending* insertion order and
`uniqueifyJsonbObject` keeps the first of each run; reversing before a stable
sort by key and then `dedup_by` reproduces exactly that without a second
comparator.

**The literal grammar is `jsonb_in`'s whole grammar, which is the opposite call
from 11.5's four types.** Those read the type's `*_out` spelling and nothing
wider, on the grounds that every value in a dump is already in that form and a
user's remedy is to copy it. That argument fails for `jsonb`: nobody types
`{"a": 1, "b": [1, 2, 3]}` with the server's spacing, and `{"a":1}` is what a
person writes. The cost is a parser rather than a per-spelling table, and it is
paid once. The field side goes through the same parser — sorting an
already-sorted pair list is cheap, and one parser cannot disagree with itself.

**Numbers normalize through `NumericKey`, so the exponent is applied rather
than carried.** A `jsonb` number is a stored `numeric` compared by
`numeric_cmp`, which is what `NumericKey` already is; `1e2`, `100` and `100.00`
therefore meet. The exponent is bounded at ±100000 — ours, not PostgreSQL's,
which reaches further. It costs nothing real: `jsonb_out` prints through
`numeric_out`, which writes no exponent at all, so no *field* can reach the
bound and only a literal can.

**Two bounds exist because a Rust overflow aborts where a refusal is
readable.** `JSONB_MAX_DEPTH` is 1000 nested containers and `JSONB_MAX_EXPONENT`
is the one above. PostgreSQL's parser is recursive too and bounded by
`check_stack_depth()`; ours is a fixed number because there is no equivalent to
catch.

**A `jsonb` column's divergence is unconditional, and deliberately.** A
document with no string anywhere in it compares exactly, but the plan is
decided per column before a row is read, so the note is raised for every
`jsonb` column under an ordering operator. That is the same conservatism
`UnknownCollation` has, and the alternative — deciding a column's verdict from
the values a query happened to scan — is the property the array shape census
exists to prevent, one type over.

## The evidence, and how it was taken

**A throwaway cross-check ran the register against every committed `jsonb`
oracle cell** — `order_key` and `compare_keys` over
`fixtures/<13–18>/oracle/comparisons.tsv`, **972 cells across the six majors,
zero mismatches**, the refused `{` literal included. It was deleted before this
slice landed and then rebuilt as a permanent test over every type, which is
`M35` (`../status/history/2026-09-01.md`, "`M35`: the register is asserted
against the oracle's answers, cell for cell").

**The `jsonb` case list this slice checked itself against reached four kinds
and no container structure** — a number, two spellings of one object, an array
and JSON `null` — so the boolean and string kinds, an object's pair count,
storage order against alphabetical order, and the raw-scalar anomaly rested on
a probe against `postgres:13.23-trixie`, `16.15-trixie` and `18.6-trixie`,
written into I41's **Observed** paragraph with a re-runnable recipe.

**Not regenerating the fixtures was this slice's call, and it was made on a
cost that does not exist.** The claim was that adding `jsonb` cases to
`comparison_oracle.py` would rewrite all 109 files under `fixtures/` — the
`\restrict` token is fresh per dump — and so could not share a review with a
library change, the rule that earned 11.2.2 and 11.11.1. It would not:
`generate_fixtures.py --skip-dumps` rewrites the 18 oracle TSVs and touches no
`.sql` file at all. `M35` took the cases on that basis, and the six facts above
are committed bytes now rather than a probe.

## What later slices inherit

**`AS_TEXT` has one member left, and 11.6 has to split its sentence or delete
it.** `OrderingDivergence::AsText`'s message — "PostgreSQL orders this type by
its own operator, not bytewise" — is *false* for `json`, which the server does
not order at all, and `json` is now the only type that reaches it. 11.5's notes
flagged this while `jsonb` still shared the arm; it is now unambiguous, and the
right fix is a `json`-specific sentence saying there is no server order to
disagree with.

**11.6 gains a third decode-per-row exemption, and it is `jsonb`.** The spec
names bare `numeric` and `interval` as the two types whose equality cannot use
the canonicalize-the-literal-once path. `jsonb` is a third and for a different
reason: the literal *can* be canonicalized once — parse it, sort it, render it
back through `jsonb_out`'s spelling — but that means writing a renderer this
slice did not, since ordering never needs one. The cheaper route is to compare
the parsed documents, which is a per-row parse. Whichever 11.6 picks, the
current code gives it the parse for free and not the renderer.

**11.10 inherits a container comparison it must not reuse.** `Jsonb::cmp` is
structural and so is the array/composite/range comparison 11.10 specifies, and
they share nothing: `array_cmp` orders by dimensions and lower bounds before
elements and puts NULL *above* not-NULL, while `compareJsonbContainers` orders
by kind before size and has no NULL at all — JSON `null` is a value with a rank.
The two are different orders that look alike, and writing one against the
other's intuitions is the mistake available here.

**`OrderingNote` still names a column and not a position.** The spec's
"A divergence names its position, not just its column" is 11.10's, and `jsonb`
would have been its first customer — a document diverges *at its strings* —
but the note is per column and says so in prose instead. Nothing here blocks
the path; the sentence is written to be replaceable.

**No figure was re-taken and none turned newly stale.** `pgtype.rs` and
`predicate.rs` are declared by no figure, and nothing else in the library
moved. `uv run measure.py --stale` names the same seven figures it named
before.
