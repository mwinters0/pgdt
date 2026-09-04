# P7.9 — `decode_array`'s element allocations

What the rest of P7 inherits: **an array element is a slice of the field, not
a `String`.** `ArrayLiteral::elements` is `Vec<Option<Cow<'_, str>>>` and one
token scanner serves all three container forms, so an element is copied only
where the literal actually carried a `\` or a doubled `""`. The mechanism is
filed by subject: [`architecture.md`](architecture.md), "The nested literal
codec", fourth load-bearing property. This doc holds the apparatus, what the
change did *not* reach, and the one finding it turns up for 7.10 and 7.11.

## Module map

| File | What it is |
|---|---|
| `pgdump_query/src/nested.rs`, `scan_quoted` | the whole mechanism: a first pass that looks for the closing quote and copies nothing, and a second that rebuilds the token from the bytes already walked once an escape aborts the first |
| `pgdump_query/src/nested.rs`, `scan_token` | returns `Option<Cow<'a, str>>`; the unquoted arm was already a slice and stopped calling `to_string` on it |
| `pgdump_query/src/nested.rs`, `ArrayLiteral<'a>` / `Elements<'a>` | the lifetime, and the alias `read_array_body` and `ArrayScan` share |
| `pgdump_query/src/nested.rs`, `an_element_is_copied_only_where_the_literal_escaped_it` | the regression guard, asserted on the `Cow` *arm* |
| `pgdump_query/src/batch.rs`, `append_array_level` / `collect_array` | the two consumers whose element type moved with it |

`decode_record` and `decode_range` still hand back `String` fields: they take
the same `scan_token` and call `Cow::into_owned` at the call site, which is
where a later slice would push the borrow one level further. `predicate.rs`
needed no edit at all — `&Cow<str>` coerces to `&str` at every site that read
an element.

## Why the record and range literals were left owned

The spec row names `decode_array` and the stake is the array file, so the
borrow stops at the array. It is not that the other two cannot have it: they
share the scanner, so the change is their `RecordLiteral`/`RangeLiteral`
field types and the `predicate.rs`/`batch.rs` sites that read them. What it
would cost is a wider review than the row asks for — `range_key` takes
`&Option<String>` and `batch.rs` builds both literals to render them back.

It is worth less than it looks, and the figure says so: `record_2/decode`
fell **190 ns → 114 ns** on this change *without* its fields being borrowed,
because the win there was `scan_quoted`'s growing `Vec<u8>` rather than the
final `String`. A quoted token now knows its length before it allocates.

## The apparatus

Three instruments, in the order they were read.

- **The registered figure**, `nested-decode-micro`, re-taken alone with
  `--figure` ([`measurements.md`](measurements.md), "Nested decode costs what
  it copies"). Decode: 290 ns → **218 ns** at four elements, 3.85 µs →
  **2.41 µs** at fifty, 190 ns → **114 ns** for the composite. The per-element
  decode slope is **77 ns → 48 ns**.
- **A criterion before/after in one sitting**, `--save-baseline` at `7b456ae`
  and `--baseline` on the working tree, which is what the percentages above
  are: −25.6%, −37.6% and −40.6%, all `p = 0.00`.
- **`perf stat -e instructions:u` over the whole `--arrays --composite` typed
  query**, three reps each leg, `release` binaries of both revisions kept in
  `runs/pgdq-{before,after}-7.9`: **176.38 G → 165.37 G** user instructions
  (−6.2%) and 19.02 → 17.40 s of user time, every after rep below every before
  rep. That is the reading the budget re-read consumes.

**Read the criterion `÷ copy` column with the doc's caveat, not as a
regression.** The 601-byte copy control moved 24 → 41 ns between sittings on
unchanged code, which is most of why that ratio went 226× → 94.8×.

**One row of the table moved that this change did not touch.** `record_2/render`
read −8.3% and −5.6% in the two sittings against a render path with no edit in
it, and `nested/text_view_x1024` read +13.4% in one and +0.5% in the other. That
is [`measurements.md`](measurements.md), "Two builds of one source can differ by
layout", at bench scale; the decode rows are the ones the change is read on, and
they moved by an order of magnitude more than that noise.

## What the profile now says, and what it hands 7.10 and 7.11

`nested::needs_quote` is **the largest single symbol** in a typed
`--arrays --composite` query, at 14.1% of the run, and it is split roughly
evenly between the two directions — `push_token` on the way out and
`scan_token` on the way in. Both are the same predicate: walk the token's
bytes testing each against a five-byte force-quote set and the six whitespace
characters.

Two things follow, and neither is this slice's to take.

- **The decode half is what is left of the per-element slope.** With the
  allocation gone, `scan_token` walks a token once for its terminator and
  `needs_quote` walks it again to reject what `array_out` would have quoted.
  A byte-classification table would fuse them. That is a further cut inside
  `nested.rs` that the lever table does not name, so it needs the spec
  amendment the phase's "a lever the profile finds" paragraph describes rather
  than being folded into a slice quietly.
- **The render half is library code too**, not the CLI's. `push_token` is in
  `nested.rs` and 4.82% of the run is inside it; what the CLI decides is
  *whether* rendering runs, not what a byte costs while it does — which is why
  `7.14` reaches both directions in one change
  ([`../status/history/2026-09-04.md`](../status/history/2026-09-04.md),
  "`needs_quote` is admitted as a lever"). What *is* true is that a whole-query
  number on this shape understates a library-only one: `print_batch` is 59.1%
  of the run.

**7.11's gate is unaffected by this slice.** The viewing builder is measured
against 7.10.1 — the builder-append half, which is its own slice as of 7.10's
split — on the arrays file, and `append_typed`'s
share barely moved (30.7% → 28.8%) — the allocation that went was under
`decode_array`, not under the Arrow build.

## What did not change

- **No behaviour.** Every `decode_*` accepts and rejects exactly what it did:
  the borrowed arm returns the same bytes the copying arm assembled, and the
  strictness rules are untouched. `tests/nested.rs`'s conformance pass over
  every nested column of every `types` fixture on all six majors is what holds
  that.
- **No `unsafe`.** The borrowed arm is `std::str::from_utf8` over a subslice,
  with the same UTF-8 check the copying arm ran on its assembled bytes.
- **`parse_array` is still owned per element.** The `array_in` superset trims,
  unescapes and re-cases, so its tokens are rarely the bytes the user typed —
  and it reads one filter literal per query rather than one per row, so a
  second scanner for it would buy nothing.
- **The phase spec's lever row is left alone.** It records the stake the phase
  committed to (4.14 µs/row, from the pre-change figure), and progress does not
  go in the spec.
