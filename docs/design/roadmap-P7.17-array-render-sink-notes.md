# P7.17 — the array arm's render, through the sink

What the rest of P7 inherits: **an array column is written where it will be
read.** `render_field_into`'s `Array` arm walks the Arrow list and appends each
element straight into the caller's buffer, so the four allocations an
array-bearing row used to pay per value — a `String` per element, the `Vec`
collecting them, an un-presized whole-array `String`, and the copy of it into
the caller's buffer — are all gone. A typed `pgdq query` over the
`--arrays --composite` file falls **104.094 G → 83.925 G user instructions**
(−19.38%) and the array column projected alone **67.535 G → 49.440 G**
(−26.79%), which is 24.0% below where 7.16 found that shape rather than the
+3.8% it left it at. The mechanism is filed by subject:
[`architecture.md`](architecture.md), "Decoders and render-back" for the arm and
its scratch, and "The nested literal codec" for the obligation this discharges.
This doc holds the apparatus, the one behavioural difference the walk carries,
what the other four nested arms did *not* inherit, and what it hands 7.12.

## Module map

| File | What it is |
|---|---|
| `pgdump_query/src/batch.rs`, `render_array_into` | the arm: the empty-array collapse, and the scratch and `dims` the walk needs |
| `pgdump_query/src/batch.rs`, `render_list_level` | one `List` level — braces, separators, and either a recursion or the leaf elements |
| `pgdump_query/src/nested.rs`, `quote_array_element` | quotes an element already sitting at `out[mark..]`, moving it aside only if the grammar wants it quoted |
| `pgdump_query/src/nested.rs`, `push_array_null` | the bare `NULL` an array spells a SQL NULL element as |
| `pgdump_query/src/batch.rs`, `prior_shape::{collect_array, render_array}` | the `ArrayLiteral`-building render, verbatim, as the oracle |
| `pgdump_query/src/batch.rs`, `differential::*` | four differential tests: one, two and three dimensions, and an array of composites |
| `pgdump_query/tests/render_allocations.rs`, `array_row` | the allocation budget of an array value at five elements and at fifty |

`nested::render_array` and `ArrayLiteral` are **untouched and still exported**:
`benches/decoders.rs`, `tests/nested.rs` and the round-trip property all read
them, and they remain what the *decode* direction renders back through. What
changed is only that `batch.rs` no longer builds one.

## The element is rendered where it lands, and only a quotable one moves

The shape that makes this worth a row rather than a tidy-up: `render_list_level`
records `out.len()` as a mark, calls `render_field_into` on the element so the
value is appended in place, and then hands the mark to
`nested::quote_array_element`. For an element that needs no quoting — every
element of an `integer[]`, and most of a `text[]` — that call is one
`needs_quote` walk and returns, leaving the text exactly where the caller wanted
it. **Nothing is copied and nothing is allocated.**

The scratch buffer the spec called unavoidable is entered only on the other
branch. The quoting decision is made from the *finished* element text and
escaping expands it, so there is nowhere in `out` to write the escaped form over
the raw one, and `String` offers no safe way to shift bytes within itself. So a
quotable element is moved into `scratch` — cleared per element, reused for the
whole value — and re-emitted through `push_token`.

*Rejected: escaping in place by reserving the expansion and shifting the bytes
right.* It is the only shape that would remove the last copy, and it needs
`String::as_mut_vec`, which is `unsafe`. The measured alternative costs one
allocation per array *value* (not per element) and only for a column whose
elements are quotable at all, which is what `tests/render_allocations.rs` pins:
an `integer[]` costs **1** allocation at five elements and **1** at fifty, a
quotable `text[]` **2** and **2**. The one that remains in both is
`ListArray::value`'s slice — an `Arc` per list level, which the shape this
replaced paid too.

*Rejected: keeping `ArrayLiteral` and giving it a borrowed element.* This is the
closure 7.16's notes named and it is aimed at the wrong structure;
`collect_array` produced `Cow::Owned` unconditionally, so the lifetime does no
work on the render path and reshaping it would have put
[`measurements.md`](measurements.md)'s "Nested decode costs what it copies" at
risk to buy nothing. The reasoning is filed at
[`architecture.md`](architecture.md), "The nested literal codec" and in
[2026-09-05](../status/history/2026-09-05.md).

## The one behavioural difference, and it is on a shape nothing produces

The rectangularity guard moved from a **flattened product** to a **per-list
length**. `collect_array` recorded the first list seen at each depth and
`nested::render_array` then asserted that the dimensions' product equalled the
element count; `render_list_level` records the same first length and asserts
each later list at that depth against it directly.

The new check is **strictly stronger**: any value it accepts has every list at a
given depth the same length, which is rectangular, which the product check also
accepts — and any value the product check rejected is non-rectangular, so this
one rejects it too. The shapes it newly catches are the ones whose raggedness
happened to preserve the total: a `{{1,2},{3,4,5,6}}` with `dims [2,3]` used to
be *silently reshaped* to `{{1,2,3},{4,5,6}}`. Neither shape is reachable —
`append_typed` rejects a value whose shape does not fit, and `decode_array`
rejects a ragged nesting — so this is a guard on a caller-assembled Arrow array
either way.

Everything else is asserted identical rather than argued so:
`prior_shape::render_array` keeps the previous implementation verbatim and the
`differential` tests put a generated corpus through both. The corpus is built as
Arrow values **directly** rather than through `append_typed`, because the
quoting rule is what is under test and `decode_array` only ever hands back text
`array_out` would have written — so a corpus routed through it could not reach
an element holding a brace, a bare `NULL` or a lone backslash. The alphabet is
the force-quote set, all six `array_isspace` characters, a multi-byte character
and the letters of `NULL`; the shapes are one, two and three dimensions and an
array of composites. Checked killing four mutations: never quoting, the
empty-array collapse removed, a NULL element written as nothing, and a dropped
separator.

## The apparatus

Five instruments, warm on `/dev/shm`, `--dqcache none`, `--table public.perf`,
`release` binaries of both revisions kept in `runs/pgdq-{before,after}-717` and
driven from `runs/measure-7.17.sh`.

- **`perf stat -e instructions:u`**, five reps a leg, legs interleaved. Typed
  `arrays` **104.094 G → 83.925 G** (−19.38%), every after rep below every
  before rep by 20 G. Typed `control` — the same query over a file with no array
  column — **34.734 G → 34.782 G**, **+0.14%**, with the two legs' ranges
  overlapping (before 34.624–34.769, after 34.667–34.782). Reported rather than
  called zero: a control-file row does re-enter `render_field_into`, it just
  never takes the branch that changed, so what can move is the enclosing
  function's layout and inlining and not its work — which is
  [`measurements.md`](measurements.md)'s "Two builds of one source can differ by
  layout", at a tenth of the size that rule was written for.
- **`parse` is the row's required control**, on the same `arrays` file, three
  reps a leg with the cache removed between them: **2.919233 G → 2.919234 G**,
  flat to 0.00003%. It renders nothing at all, which is what a render-path row
  needs and what a `strings` query is not (7.16).
- **The array column projected alone**, `--column v_int_array_long`, three reps
  a leg: **67.535 G → 49.440 G**, −26.79%. This is the number 7.16 left at
  **+3.8%** (65.018 → 67.517 G): the residual is closed and the arm is now 24.0%
  below the shape 7.16 inherited.
- **Wall and user**, three reps: typed `arrays` **12.43 → 10.19 s** wall,
  **11.21 → 9.11 s** user, system flat at ~1.0, peak RSS unchanged at ~117 MB.
- **Whole-file byte identity, three files.** `pgdq query --dqcache none` over
  the 3.00 GiB `arrays`, `composite` and `control` inputs, before and after,
  compared by `sha256sum`: identical in all three, over 699,962 / 803,995 /
  814,362 rows. That is the row's review question — does the render path emit
  identical text — answered on real data rather than only on a corpus.

**One profile**, `runs/profile-query-typed-arrays-after-717.{data,txt}`, from
the `profiling` build with libc symbols resolved from the installed package. On
the `arrays` file `poll_next` is **63.05%** inclusive against `print_batch`'s
**36.42%** — the same side of the line 7.16 put the control on. The array render
is now about **9%** of the run in self time (`render_field_into` 4.20%,
`quote_array_element` 2.48%, `render_list_level` 1.63%, `push_token` 0.74%),
with `push_integer` at **5.99%** for the elements' own digits; ahead of all of
them sit `nested::scan_token` 9.12%, `batch::append_typed` 8.72% and
`copy::unescape_field` 7.13%, which are the decode direction.

## The other four nested arms did not fall out of it

The spec allowed them to follow "where they fall out of it at no extra risk".
They do not, and none of them changed. `Multirange` still collects a
`Vec<String>` and joins it; `Record`, `Range` and `Int2Vector` still build their
literal and push the rendered text. The array's walk is over an Arrow `List`,
whose structure `batch.rs` already knows; the other four would each need their
own container grammar — `render_record`'s parentheses, `render_range`'s
per-value brackets, `int2vectorout`'s spaces — pushed from `nested.rs` into a
sink form, which is three new public shapes rather than a consequence of this
one. And the prize is small by measurement rather than by guess:
`projection-widths` prices the whole composite column at **+0.98 µs a row**
against the two array columns' +13.21, and the other three render one value a
row against the array's fifty.

## What this hands 7.12

- **Nothing is ordered any more.** `7.17` was the last row ahead of `7.12`, and
  the spec's standing rule stays as written: any row admitted after spec time
  against a typed-query profile lands ahead of the sweep.
- **`nested-end-to-end` and `projection-widths` are the sweep tables this
  certainly moves**, both by shape rather than by inference — the first is the
  `--arrays --composite` typed query itself, the second prices the array columns
  by subtraction and is the table that named the +13.21 µs this row is aimed at.
  `cross-file-floor` and `allocator` run a typed query on a file with no array
  column and are within their own spread. `parse` and the map figures do not
  move at all.
- **`nested-decode-micro` is red on `nested.rs` and unmoved in fact.** It times
  `decode_array`/`render_array`/`decode_record`/`render_record` in isolation and
  none of those functions changed; this row only added two new ones beside them,
  reachable from `batch.rs` alone. The only path from here to that figure is
  code layout, which is a standing rule rather than a re-take.
- **`allocator` is not worth another re-read on this row.** 7.15 and 7.16 each
  earned one by removing allocations from the CLI side; this one removes them
  only from a column shape the `allocator` figure's control input does not have.
- **The library's per-row budget is untouched.** `render_field_into` is outside
  `poll_next`, which is what that budget splits; the `parse` control says so.

## What did not change

- **No behaviour on any reachable value.** Three whole-file outputs
  byte-identical; a generated corpus checked against the previous
  implementation at three dimensionalities and over a composite element. The one
  difference is the rectangularity guard above, on a shape `append_typed` and
  `decode_array` both reject.
- **No `unsafe`**, and no new allocation on any path.
- **`render_field` and `render_field_into`'s signatures**, and
  `nested::render_array`'s. `push_array_null` and `quote_array_element` are
  `pub(crate)`, so the array grammar's constants stay private to `nested.rs`.
- **No figure re-take and no `measure.py` register edit.** `NESTED` and `DECODE`
  already declare `batch.rs` and `nested.rs`, and every figure they can move was
  already red against the `ba2fc12` stamp.
