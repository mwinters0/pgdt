# P4 — Composite value decoding: notes

The phase's residue after the wrap audit. Every mechanism it built is described
by subject in [`architecture.md`](architecture.md) — the recursive mapping, the
twelve hardcoded range/multirange subtypes, both array refusals and the six
declaration spellings under "Type resolution"; the quoted-token scanner under
"The nested literal codec"; the `List`/`Struct` builders and the plan that
picks their codecs under "Nested columns: `NestedPlan` travels beside the
`DataType`"; the recording and the transform under "The array shape census" and
"What the census decides, and who may believe it"; the resolved Arrow type on
`pgdq info --verbose` under "Coverage is stated once, at the top"; the tables that hold the
shapes under "Fixtures" — so what is left here is what subject-filing has no
home for: what the phase tried and abandoned, how its own numbering misleads,
and where its facts went.

## What the phase got wrong on the way, and what that is worth

**"The recursion handles it" is not coverage, and it is the phase's central
mistake.** The spec's mapping table is recursive by design, and 4.4 delivered
it: `resolve_declared_type` calls itself, nesting composes in both orders, and
every fixture round-tripped on six majors. It also typed one declared shape it
could not fill — a column of `d[]` over `CREATE DOMAIN d AS integer[]`, which
resolves `List<List<T>>` while its literal is one brace deep (I26), so no value
can fill the type. Six majors of green tests said nothing about it because no
fixture held the shape. The transferable half is not "add a fixture": it is
that a shape *observed to work* during a sweep is not covered until a fixture
holds it, which is why 4.4.2 landed
`every_resolution_outcome_is_produced_by_a_real_fixture_column` and why
`t_nested_array` carries five multi-hop shapes that merely worked. That test
mechanizes half of the fixture rule; the other half — a shape resolving to an
outcome already covered — stays a judgement call, and naming that limit is
better than a check implying coverage it does not have.

**Reading a declaration more literally than the server does is the same defect
pointed the other way.** 4.4.2's refusal was written against the *spelling*, so
`integer[][]` was refused as an array of arrays. PostgreSQL collapses every
array-bounds spelling to one array level (I21, I28), so that column's element
type is `integer` and the refusal stated something false about it rather than
declining to answer. Both defects live in the same twelve lines and both were
found by writing down what `pg_dump` actually emits; the pair is the argument
for the phase's own fixture-first ordering, which the two follow-ups kept.

**Two predicates walking the same domain chain is what made "where does
normalization live" a question.** 4.4.3 had to normalize the array-bounds
production at two call sites and flagged the second as an unattended call.
Grilling established the choice was free — both placements produce identical
outcomes on every input — and that the real finding was the shape:
`element_is_opaque` and `element_is_array` each walked the chain, each carrying
part of the argument for the whole. 4.4.4 folded them into `resolve_array` over
one walk, and the question stopped existing rather than being answered. The
refusal order survives as documentation of what must win *if* the two ever
overlap; as written no input reaches both, and both answer `Utf8View`, so the
order can only ever pick a label.

**A gate on work that looks expensive bought the pre-filter and cost a state no
user could repair.** 4.5 censused only under `ScanExtent::Full`, on the premise
that a cold query should not pay per-row work for evidence `is_complete` would
discard. The cold query already receives every row it would need, so the saving
was the pre-filter alone — 1.2% on a cold read — while a dump mapped by a cold
query and *then* by a full one came out `is_complete` with its early blocks
permanently uncensused, because `map_forward` splices onto a prefix it does not
re-read. 4.5.1 reversed it, and the `Option` the gate forced went with it. The
shape to recognize again: a saving measured against the work, and a cost paid
in a state that has no second visit at which to be repaired.

**A guard is worth less than removing the case it guards.** 4.5.1 shipped
`retype_from_census` refusing to retype an already-nested `Array` plan, because
such a plan was then reachable from the DDL and rebuilding to the census's
depth would have collapsed the column and failed on every row. 4.4.2 — landed
*after* it — refused the shape at resolution instead, and the guard went with
it. What survives for a reader of the transform is not a check but a property:
a nested `Array` plan can only ever be one the transform itself built.

**One of the spec's verification bullets could not survive the phase's own
reversal.** "The mixed-dimension fixture column resolves to `Utf8View` **with**
a census and raises `FieldDecode` **without** one" was written when censusing
was conditional. After 4.5.1 there is no without: every mapping pass censuses,
so a top-level array column cannot reach the refusal, and
`a_multidimensional_or_decorated_array_is_refused_on_the_optimistic_path` was
deleted rather than repaired. The refusal's one remaining cause — an array
inside a composite — has no fixture, because no fixture schema puts a 2-D array
in a composite field, so it is pinned by a hand-built dump paired with a
control table holding a 1-D array in the same field: the control round-trips
through the typed path, so a failure on the other table is the nested array's
and not a misquoted composite's.

**A slice row that commits to a measurement without naming its instrument
leaves the instrument to whoever implements it.** 4.6 was amended to owe three
figures; its text named both instruments for the array ratio and none for the
composite's, so the composite shipped as a micro and the row read as satisfied.
4.6.1 was earned for the remainder, and the finding is now a standing rule in
[`roadmap.md`](roadmap.md). What 4.6.1 then found is the more useful half: the
composite's end-to-end share **cannot be separated by differencing whole
generated files**. Two files differing only in RNG seed read +0.22 µs/row apart
where the true difference is zero, so anything under ~±0.5 µs/row out of that
instrument is apparatus — and the composite's share sits inside it, reading
negative twice. More repetitions do not fix this; a different instrument would
(two files with byte-identical data sections, one declaring `v_comp` as
`text`), and it is named, costed and not built. The bound is what the phase
delivers, and it answers the question that was actually asked: of the three
nested columns, the arrays carry essentially all the cost.

## The slice numbers are not the order things landed

Read top to bottom the checklist implies 4.4.2 → 4.4.3 → 4.4.4 preceded 4.5;
they did not. The census landed first, and all three follow-ups were **earned
after 4.5.1 was in** — which is why 4.4.2 deletes a guard 4.5.1 shipped, and
why the two slices after it rework an array arm the census had already been
built against. A third level is earned rather than planned
([`../process.md`](../process.md), "Slice numbering"), so it necessarily lands
out of numeric order; the alternative was renumbering a tail and erasing the
record that anything was earned at all. `git log` is the order; the numbers are
the lineage.

The spec's table had six rows and thirteen slices landed: one split at
grilling (4.4.1), one split mid-slice (4.5.1), and five earned after the fact
(4.1.1, 4.4.2, 4.4.3, 4.4.4, 4.6.1). That ratio is itself the phase's summary —
the spec was right about the shape of the work and consistently wrong about how
much of it one review could hold.

## Where the phase's facts went

- **Mechanism** — [`architecture.md`](architecture.md), by subject, in the
  sections named at the top. The wrap moved in what the slice notes held and it
  did not say: why `pgdq query` is a pull-mode caller and what that leaves
  `read_table` with; the census vector's sizing and the header-less block's
  short one; that a reported schema and a streamed one may legitimately
  disagree about one table; how the `types` schema's array tables are divided;
  and the one fixture column the round-trip suite deliberately excludes.
- **External behaviour** — ten entries in
  [`postgres-invariants.md`](postgres-invariants.md), I20 through I29. Two came
  out of the phase grilling and the other eight out of the work itself, most of
  them from writing a fixture and reading what `pg_dump` wrote back — which is
  the fixture-first ordering paying for itself.
- **Figures** — [`measurements.md`](measurements.md): "The census on
  array-bearing rows costs 81% of a warm scan", "Nested decode costs what it
  copies, and an element is an allocation", "A typed query over nested columns
  costs 15 µs a row more than a string one" and its subsection on the
  cross-file subtraction's floor. Two of that doc's standing rules were
  written out of 4.6.1's re-take — a parsing-CPU figure belongs on tmpfs, and
  a comparison table is re-taken whole in one interleaved sweep.
- **User-facing** — `docs/manual/type-handling.md`: what each family becomes,
  the range struct's layout (which is what makes `--verbose`'s `Range<T>`
  elision lossless), the four ways one of these columns is still a string, the
  one shape still decided optimistically, and that a predicate on a nested
  column still matches literal text.
- **Accepted deficiencies** — [`../status/STATUS.md`](../status/STATUS.md),
  "Known gaps": the array inside a composite, the two refusals that leave a
  fully-understood array as text, and the quoted type name that costs a weaker
  type.
- **Facts for phases with no spec** —
  [`roadmap-P5-pushdown-inbox.md`](roadmap-P5-pushdown-inbox.md) (nested columns keep an
  untyped text-shaped predicate; per-block per-column recording exists and
  avoided the L1/L2 injection it looked like it needed),
  [`roadmap-P6-embeddable-engine-inbox.md`](roadmap-P6-embeddable-engine-inbox.md) (push mode has no
  non-test consumer left; a partial index's blocks are each fully censused
  while the *table's* set may not be; resolution keyed by `COPY` block leaves
  block-to-table to that phase) and
  [`roadmap-P7-scan-performance-inbox.md`](roadmap-P7-scan-performance-inbox.md) (nested values always
  copy and a large share of them are viewable; every mapping pass now does
  per-row work; the cross-file attribution floor).
- **Wanted but unscheduled** — [`roadmap.md`](roadmap.md), "Future": the
  lossless array representation as a selectable knob, keying the census by path
  rather than by column, and a real type-name tokenizer. All three are
  strictly additive — each only ever touches columns this phase left as
  `Utf8View`.

## What the next phase inherits directly

**The type set is complete, which is the reason this phase preceded pushdown.**
`TypeOutcome::Deferred` and `DeferredKind` are gone; every declared type now
resolves to a real Arrow type or to `Utf8View` with one of eight named reasons.
P5 designs against that table rather than against a partial one, and
P6 freezes those reasons as an outward surface.

**The pair is the unit.** `resolve_declared_type` is the one producer of
`(DataType, NestedPlan)` and `retype_from_census` the one transform, and
neither half is ever rewritten alone. Two trees that must agree are kept
agreeing by that rule and not by a structure enforcing it, which is affordable
exactly while those two sites are the only writers — the single-tree fallback
is in `architecture.md`'s "Nested columns" section, unbuilt and costed, for
whenever a third appears.

**The koji identity check passed as part of the P9 wrap run.** The spec
puts it once at phase wrap; the 2026-08-27 run (`measurements.md`, "koji full
scan") reproduced 74 blocks / 19,575,829,920 rows / 784,019,857,152 bytes on a
tree carrying every library change of this phase, so no separate P4 run
was owed. That the phase's whole subject is absent from koji — no composites,
six array columns all `NULL` — is what makes identity the right check there:
a difference would mean a slice touched the scanner by accident.
