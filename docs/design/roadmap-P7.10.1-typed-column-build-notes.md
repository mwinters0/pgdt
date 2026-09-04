# P7.10.1 — The typed column build

What the rest of P7 inherits: **the builder-append half of the typed row is
52 ns a row, and 7.11's gate is not met.** This slice lands **no library
code** — it is one of the phase's measured refusals ("The phase follows the
evidence", *a lever row is satisfied by a rejection*). The decomposition it
produced is filed by subject:
[`architecture.md`](architecture.md), "The library's own per-row budget". This
doc holds the apparatus, the input the gate reading needed and does not have
in the tree, and what 7.11 and 7.12 inherit.

## What was measured

Four profiles of the shipped shapes, plus release-binary user times taken
without `perf` on them, medians of five. A profile is a `runs/` artifact and
none of this is a figure
([`measurements.md`](measurements.md), the preamble; the phase spec's "How
this phase measures").

`append_typed`'s inclusive share is split by walking each sample's stack
through the inline frames and asking whether an `arrow-*` frame, a builder
method or an `append_value`/`append_null`/`append_view` sits between the leaf
and `append_typed`. That is the whole instrument: everything else under
`append_typed` is a decoder or the codec.

| Run | Rows | user | `append_typed` | Arrow appends under it | µs/row of Arrow appends |
|---|---|---|---|---|---|
| control, typed, all 16 columns | 814,362 | 9.16 s | 7.04% | 0.46% | **0.052** |
| `--arrays --composite`, typed, all 19 | 699,962 | 14.97 s | 25.11% | 0.89% | **0.190** |
| `text[]` variant, typed, all 18 | 291,573 | 6.60 s | 29.15% | 2.43% | **0.550** |
| `text[]` variant, `--column v_text_array_long` | 291,573 | 3.23 s | 40.12% | 3.46% | **0.383** |

The first row is the lever the spec's table names. The last is 7.11's gate.

## The lever: nothing to take

`append_typed`'s 0.79 µs a row on the control splits **0.68 µs of decoders,
0.06 µs of its own dispatch, 0.052 µs of Arrow appends**. The dispatch is a
`match` over `ColumnBuilder`'s twenty variants, which compiles to a jump
table; the appends are `arrow-rs`'s `push_mut`, `append_non_null` and a
`memmove`. The largest arrow leaf on the control is
`__memmove_avx_unaligned_erms` at 0.089% of the run.

**The lever table's 1.66 µs for `append_typed` was never the builder.** Most
of it was the three `malloc`/`free` pairs a row that 7.10 removed from the
decoders it calls; the budget re-read put the bucket at 0.84 µs, and this
slice says 94% of what is left is still the decoders. So the row is
discharged by a reading, not by a diff.

**Pre-sizing the builders was the one obvious candidate and it is refused.**
`new_column_builder` builds every builder with `::new()` rather than
`with_capacity(max_rows)`, so each grows by doubling — about thirteen
reallocations per column per 8192-row batch. The profile prices the whole of
it: `reserve` and `capacity` under `append_typed` are 0.018% and 0.034% of the
control run, together under 6 ns a row, which is below every instrument this
campaign owns. Landing it would also make a one-row batch allocate 8192 slots
in every column, so it is a memory regression bought with nothing.

## 7.11's gate: 0.383 µs/row against 1 µs/row

**The gate names a file that cannot answer it.** `7.11`'s row reads the
`List<Utf8View>` build "on the arrays file", and the registered `arrays` input's
two array columns are `integer[]` — deliberately, so that element count is the
only variable (`scripts/generate_perf_data.py`, `ARRAY_COLUMNS`). No committed
input carries a `List<Utf8View>` at all, so the gate was read on a purpose-built
one instead.

**That input is specified here and lives only in `runs/`.** It is the
`--arrays` shape with the two array columns changed to `text[]`: the same
sixteen scalars from `generate_perf_data.py`'s `COLUMNS` and the same 3–5 and
50 element counts, seed 42, every element the literal token
`lorem_ipsum_dolor_sit`. Three properties are load-bearing and all three are
chosen to be **favourable to 7.11**, so that a refusal read off it is safe:

- **21 bytes an element**, above `arrow`'s 12-byte view-inlining threshold
  (`make_view`). At or below it a view and a copy are the same instruction
  sequence and 7.11's prize is exactly zero, so a shorter element would have
  refused the lever by construction rather than on evidence.
- **No whitespace in an element**, so `array_out` quotes none of them and the
  literal's grammar matches the `integer[]` one. The difference between the
  two files is the child builder and nothing else.
- **Fifty elements in every row**, never NULL and never ragged — the widest
  the registered apparatus uses, so the per-element cost is multiplied by as
  much as any published input multiplies it.

Isolated with `--column v_text_array_long`, the `List<Utf8View>` build is
**0.383 µs a row — 7.7 ns an element — against a gate of 1 µs a row**. What
7.11 would actually win is smaller again: it replaces the copy with a view
write, which `nested-decode-micro` floors at 2.96 ns
([`measurements.md`](measurements.md), "Nested decode costs what it copies"),
so the difference is at most ~5 ns an element, **0.24 µs a row on this
shape** — 2% of that query's 11.08 µs row and 4% of the library's own 5.46 µs
— before any of the recursive chunk-retention and block-invalidation machinery
the change needs at every level of `List` and `Struct` nesting.

**The build is not where a nested row's time goes.** On the same isolated run
`nested::decode_array` is 3.88 µs a row against the build's 0.383 — ten to
one — and inside the codec `memchr_naive` alone is **15.3%** of the run. That
is `Syntax::force_quote`'s linear `contains`, which is 7.14's row; this file
puts it higher than any registered input does, and it is the reason the
nested path is expensive rather than the builder.

## What the next slices inherit

- **7.11 is a refusal, not a build.** Its gate has a measured reading and the
  reading is 38% of the threshold on a shape built to flatter it. The slice's
  delivery is a notes doc saying so; it lands no viewing builder.
- **7.12's sweep is unaffected by this slice.** No code changed, so no figure
  moved and no figure's staleness changed. The per-row budget's typed
  `poll_next` read 3.25 µs this sitting against the published 3.10, which is
  inside the ±8% a warm absolute resolves to across sessions
  ([`measurements.md`](measurements.md), "A move smaller than the apparatus
  resolves is not a finding"), so the published budget stands and gains the
  split above rather than being re-based.
- **7.14's stake is corroborated on a shape no registered input has.** A
  `text[]` column of 50 elements puts `needs_quote`'s byte walk at 15.3% of a
  run against the 14.1% the `--arrays --composite` file gives. Nothing about
  7.14's row changes; it is now known not to be a property of the composite.

## What did not change

- **No library code, no test, no fixture.** The only artifacts are `runs/`
  profiles and the generator above.
- **No registered figure was re-taken**, and none became stale: all seventeen
  were already red against the `ba2fc12` stamp and nothing this slice touched
  is a declared path.
- **The phase spec.** The lever table records the stake the phase committed
  to and progress does not go in the spec. The one thing worth a decision —
  reading 7.11's gate on an input the gate does not name — is filed under
  `STATUS.md`'s "Decisions worth another look", which is the route an
  unattended session has ([`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md),
  "The levers, and what each is worth before it is touched").
