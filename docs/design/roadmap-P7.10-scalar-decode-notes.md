# P7.10 — Scalar decode

What the rest of P7 inherits: **a `decode_*` allocates only where its return
type is an allocation.** Three of the scalar decoders took a `String` on the
way to a fixed-size value and none does now, so `batch::append_typed`'s bucket
no longer carries three `malloc`/`free` pairs per row that were never the
builder's. The mechanism and the rule are filed by subject:
[`architecture.md`](architecture.md), "Decoders and render-back". This doc
holds the apparatus, the split that earned `7.10.1`, and what the profile hands
the two slices after it.

## Module map

| File | What it is |
|---|---|
| `pgdump_query/src/decode.rs`, `HEX_NIBBLE` | a 256-byte nibble table, `0xFF` outside hex, so a pair is two loads and an `or` and validity is one bit test on the accumulated `or` rather than a branch per byte |
| `pgdump_query/src/decode.rs`, `decode_bytea` | the largest single win: `as_chunks::<2>()` over the field, validity accumulated across the whole value |
| `pgdump_query/src/decode.rs`, `decode_uuid` | the hyphen-stripped `String` gone — a filter over the field's own bytes, 32 nibbles consumed then a check that nothing is left |
| `pgdump_query/src/decode.rs`, `parse_time_of_day` | the pad-to-six-then-parse gone; the fraction is accumulated and multiplied by `POW10[6 - frac.len()]` |
| `pgdump_query/src/decode.rs`, `decimal_unscaled_digits` | three allocations to one: `int_part ++ frac_part` is never materialized, `digit(i)` indexes into whichever half `i` falls in, and the answer is written once at its final length |
| `pgdump_query/src/decode.rs`, `mod prior_shape` / `mod differential` | the four previous implementations kept verbatim, and the corpora the new ones are checked against |
| `scripts/measure.py`, `DECODE` | the mechanism nothing declared, now on the four figures that run a typed query |

`copy::hex_val` stays as it was and `decode.rs` no longer imports it: its
remaining caller is `unescape_field`'s `\xNN` escape, one or two bytes at a
time, where a table buys nothing.

## Why the equivalence is asserted rather than argued

The whole claim of this slice is *no behaviour change*, and three of the four
rewrites are the kind where an argument is cheap and wrong. So `prior_shape`
keeps the previous implementations and `differential` checks the new ones
against them, over two corpora per decoder:

- **Free-form**, from an alphabet of the valid bytes plus the ones just outside
  each accepted range plus one multi-byte character. It reaches the rejecting
  branches and almost nothing else — one string in twenty thousand is a
  well-formed time of day.
- **Shaped**, built to that decoder's own grammar and then damaged at one
  position two times in three. That is what puts an accepting branch beside
  every rejecting one: every fractional width from zero to eight, 31/32/33 hex
  digits with hyphens scattered rather than only at the canonical four
  positions, both parities of `bytea` length including the empty `\x`, and
  numerics with leading zeros, all-zero values and both signs.

Each of the four tests was checked against a mutation of the function it
covers — the fraction's scale dropped, the uuid's trailing-length check
dropped, `bytea`'s odd-length check dropped, the numeric's collapse-to-`"0"`
dropped — and all four mutants died.

**The corpora are generated from a seed rather than committed.** A failure is
reproducible from the seed alone, and a listed corpus is a list of the cases
somebody thought of, which is the half of the input space these rewrites were
least likely to break.

## The apparatus

Two instruments, and the second is the one that carries the slice.

- **`perf stat -e instructions:u` over the whole typed control query**, three
  reps a leg, `release` binaries of both revisions kept in
  `runs/pgdq-{before,after}-7.10`. **95.046 G → 90.719 G** user instructions,
  **−4.55%**, every after rep below every before rep.
- **The same, in `strings` mode, as the control**: medians 24.772455 G and
  24.772451 G, which is **−0.00002%** — five of the six reps inside a 16 k
  instruction band on a 24.8 G run. Not "within the noise": unmoved. That is
  the reading that says the change is confined to the typed path, and it is
  worth more than the typed number on its own, because the two modes share
  every line of the scan, the split and the unescape.

Wall and user time moved with it — a typed query's user time reads 9.10 s
against the budget's previous sitting at 10.06 — but that is a cross-sitting
absolute and resolves to about ±8%
([`measurements.md`](measurements.md), "A move smaller than the apparatus
resolves is not a finding"), so the instruction count is what the slice is read
on.

**No registered figure was re-taken.** All seventeen were already stale, and
`decode.rs` was declared by none of them, which is the third instance of the
blindness `READ` and `PREDICATE` record. `DECODE` now exists and is declared by
`nested-end-to-end`, `cross-file-floor`, `projection-widths` and `allocator` —
the four that run a typed query. `scan-throughput-*`, the census pair,
`chunk-size` and the map figures are `parse` and `dd` only; `predicate-terms`
is `--schema-mode strings` throughout, so no column of it resolves typed; and
`nested-decode-micro` runs `cargo bench -- nested`, which filters the scalar
groups of `benches/decoders.rs` out even though the file imports them.

## Why the slice split, and what `7.10.1` is

The spec row named its own seam — "a `decode.rs` half and a builder-append
half" — and the two halves are not one review. This half is pure functions with
an oracle to check against, and the evidence above is exact. The other half
edits `batch.rs`, which 7.6, 7.7.1 and 7.9 have each already reworked, and its
prize is now an open reading rather than a number: `append_typed` was sized at
1.66 µs a row and the budget re-read puts it at **0.84**, because most of what
that bucket held was the allocations this slice removed rather than the Arrow
appends.

**So `7.11`'s gate re-targets, and that is the split's one consequence outside
this file.** The viewing builder lands only if the builder-append reading puts
the `List<Utf8View>` build above 1 µs/row on the arrays file; the reading is
`7.10.1`'s, and the spec's row now says so.

## What the profile says next

Read against a typed control query, after this slice:

- **`decode::render_bytea` is 27.34% of the run and `render_uuid` 10.41%**,
  under a `render_field` at 60.01%, all of it inlined into `print_batch`. The
  first is a `format!("{b:02x}")` per byte, one `String` allocation each; the
  second a `format!` per byte through five `collect::<String>()`s. Both are
  *library* functions on `render_field`'s path, so this is not the CLI's cost to
  own the way `core::fmt`'s integer formatting is — which is the reading 7.14's
  admission already settled for `push_token`
  ([`../status/history/2026-09-04.md`](../status/history/2026-09-04.md),
  "`needs_quote` is admitted as a lever"). No lever-table row names *these two*,
  so it is filed under `STATUS.md`'s "Decisions worth another look" rather than
  admitted here — the route the phase spec's "a lever the profile finds"
  paragraph prescribes for an unattended session.
- **`copy::unescape_field` is the largest library bucket in both modes now**,
  7.88% of a typed profile and 21.97% of a `strings` one. It is
  `decode_field`'s remainder and an escaping question, not a decoding one.
- **What is left in `decode.rs` is small and evenly spread**: `decode_bytea`
  0.67%, `parse_time_of_day` 0.64%, `parse_ymd` 0.35%,
  `decimal_unscaled_digits` 0.19% plus its `digit` closure at 0.29%,
  `decode_timestamp_micros` 0.27%, `decode_date32` 0.13%, and `decode_uuid`
  below the profile's floor entirely. There is no second `decode_bytea` in
  there.

## What did not change

- **No behaviour**, asserted as above and additionally by the 52,338-cell
  register-against-oracle test, `tests/decode.rs`'s round-trip against
  `SchemaMode::Strings`, and the fixture conformance pass over six majors —
  all of which read these decoders on every typed column they touch.
- **No `unsafe`**, and no public signature. `decimal_unscaled_digits` still
  returns the digit string both `i128` and `i256` parse, which is what keeps
  the two widths agreeing on what a `numeric` field means.
- **The render direction.** Every `render_*` is byte-for-byte the function it
  was, which is why `nested-decode-micro`'s render rows and the round-trip
  tests are untouched evidence rather than re-taken evidence.
- **The phase spec's lever table.** It records the stake the phase committed to
  (2.61 µs/row, `append_typed` 1.66 and `decode_field` 0.95), and progress does
  not go in the spec; the slice table is amended because the *split* is a
  decision change, which is the one thing that does.
