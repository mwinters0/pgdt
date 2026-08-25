# Phase 4.2 — The nested literal codec: notes

What 4.3 and 4.4 inherit. `pgdump_query/src/nested.rs` (L2, recorded in
[`layering.md`](layering.md)) decodes and renders the four container literals.
Nothing resolves to it yet — `resolve_declared_type` still answers `Deferred`
for all four families, and no `ColumnBuilder` arm exists.

The mechanism is described in [`architecture.md`](architecture.md), "The
nested literal codec". These notes are what is *not* in the code or that
section.

## The API 4.3/4.4 build on

```rust
decode_array(&str)      -> Option<ArrayLiteral>   // elements, dims, lower_bounds
decode_record(&str)     -> Option<RecordLiteral>  // fields
decode_range(&str)      -> Option<RangeLiteral>   // empty, lower, upper, two flags
decode_multirange(&str) -> Option<Vec<RangeLiteral>>
```

plus a `render_*` for each. `Option` rather than `Result` is deliberate and
matches `decode.rs`: L2 decoders return `None` and the caller — `batch.rs` —
is the one that knows the table, column, declared type and row offset that
`Error::FieldDecode` needs.

Two conveniences 4.4 will want: `ArrayLiteral::ndim()` and
`ArrayLiteral::is_decorated()`. Those two predicates *are* the optimistic
path's error condition — `ndim() != 1 || is_decorated()` is exactly the value
`List<T>` cannot represent — and 4.5's census records nothing else about a
value.

## Non-obvious calls

**Decode is strict where `*_in` is permissive.** An unquoted token that
`needs_quote` would have quoted is rejected, as are whitespace padding, a
ragged nesting, an inner `{}`, and a decorated `{}`. The reason is not
pedantry: it is what makes decode and render actual inverses. A lenient
decoder accepts `{ 1,2}` and renders `{1,2}`, and then `pgdq query` silently
stops reproducing its input — the one failure mode this project does not
accept. Everything rejected here is unreachable from `*_out`, so nothing real
is lost; the round-trip test over six majors is the evidence.

The one deliberate leniency is the opposite direction: a bare `NULL` array
element is matched case-insensitively, as `array_in` does. `array_out`
force-quotes any element whose text is `null` in any casing, so this can never
misread real output, and it means we agree with PostgreSQL on a hand-written
file rather than with nobody.

**`needs_quote` is shared between decode and render**, so the strictness above
and the render-back predicate can never drift apart. If the predicate is wrong
it is wrong in both directions at once, which is what makes the fixture
round-trip test able to catch it at all.

**Arity is not checked.** `decode_record` returns whatever fields the literal
holds; it does not know the composite's declared field list. `()` is one NULL
field — that is what `record_out` writes for a one-field composite whose field
is NULL — and a mismatch against `TypeKind::Composite::fields` is 4.4's error
to raise, with the type name in hand.

**`ArrayLiteral` normalizes `{}` to `dims: []`, `lower_bounds: []`.** So an
empty array has `ndim() == 0`, not 1. 4.3's `List` builder must treat that as
"append an empty list", not as a shape disagreement — an empty array is
representable at every dimensionality, which is precisely why `array_out`
collapses it.

**Byte scanning, not char scanning.** Every structural character is ASCII and
no multi-byte UTF-8 sequence contains an ASCII byte, so the scanners index
bytes and only validate UTF-8 when a token is materialized.

## What 4.3 and 4.4 still have to decide

- **Recursion is the caller's.** `nested.rs` peels exactly one layer: an
  array's elements come back as `Option<String>`, and if an element is itself
  a record literal it is the caller that calls `decode_record` on it. The
  recursive walk lives with the recursive *type* mapping, in 4.4.
- **Nothing here consults `typdelim`.** The separator is hardcoded to `,`,
  which is only sound because 4.4 refuses to resolve an array whose element
  type is `box` or any `TypeKind::Base`. If that refusal is ever relaxed, this
  module needs the delimiter threaded through `Syntax` — the field exists.
- **The multirange member is a `RangeLiteral`, not a nested type.** A member
  is always a bracketed range: PostgreSQL drops empty ranges when it builds a
  multirange, so `{empty}` is rejected rather than accepted as a member.

## Verification

`tests/nested.rs` runs every nested column of `fixtures/*/types/default.sql`
on **all six majors** (not the usual 13/16/18 sample — the codec is exactly
the kind of thing a minor upstream change to a `needquote` predicate would
break, and reading six small fixtures costs nothing) and requires
`render(decode(v)) == v` for each. It also walks the two both-conventions
columns one layer at a time in each nesting order, and reads the census
shapes off `t_array_shape` — `[2, 2]`, `[1, 2]` and a `[0:2]=` prefix — so
4.5 has a pinned statement of what its census must report.

`nested.rs`'s own unit tests cover what a small fixture cannot reach: three
dimensions, a two-dimensional lower-bound prefix, `{{},{}}`, and the
malformed shapes.
