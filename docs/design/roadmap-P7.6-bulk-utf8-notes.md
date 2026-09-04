# P7.6 — Bulk `simdutf8` over the chunk, and the unchecked borrow path

What the rest of P7 inherits: **a row's bytes are validated as UTF-8 once per
chunk, not once per field**, and the vocabulary a row travels in has changed —
`copy::RawRow` carries the bytes plus, when the loop validated them, the same
bytes as `str`, and `copy::field_ranges` is the splitter both consumers walk.
The mechanism and its rejected alternatives are filed by subject:
[`architecture.md`](architecture.md), "A row's bytes are validated once, in
bulk". This doc holds the apparatus, the reading, and the one finding that was
not this slice's subject at all.

## Module map

| File | What it is |
|---|---|
| `pgdump_query/src/copy.rs`, `validated_prefix` | the bulk pass: the largest newline-terminated prefix of a span, through `simdutf8::basic::from_utf8`, empty when it does not validate |
| `pgdump_query/src/copy.rs`, `RawRow` | a row's bytes, and the `str` when there is one; `decode(range)` picks the path |
| `pgdump_query/src/copy.rs`, `field_ranges` / `FieldRanges` | the splitter, as ranges; `split_fields` is it, resolved |
| `pgdump_query/src/copy.rs`, `unescape_field` | the escaped path, shared by both decodes, still ending in `String::from_utf8` |
| `pgdump_query/src/stream.rs`, `row_text` | where a row sits inside the validated prefix — `str::get`, so a wrong range costs the fast path and nothing else |
| `pgdump_query/src/stream.rs`, the replay loop | takes the pass, lazily, on the first row that will decode something |
| `pgdump_query/src/batch.rs`, `RowBatcher::decodes_fields` and `predicate::ResolvedExpr::reads_fields` | the gate that keeps `--no-columns` paying nothing |
| `pgdump_query/src/copy.rs`, `mod tests` | five: the two splitters agree, the prefix's cut, a prefix that fails, the two decode paths agree, and the boundary fallback |
| `pgdump_query/tests/stream.rs` | two: a query answers identically at every chunk size from 1 byte, and a non-UTF-8 field is refused only where it is read |

`CopyScanner`, `ChunkCarry`, `io.rs`, `map.rs` and the cache are untouched. The
scanner still hands out `Event::Row(&[u8])`: **the validation is the read
loop's, not the scanner's**, because only the loop knows whether anything
downstream will decode a field.

## The two decisions worth not re-deriving

**No `unsafe`, deliberately.** The obvious shape is
`str::from_utf8_unchecked` behind a `bool` the caller promises, which makes a
safe `pub(crate)` function able to cause undefined behaviour if an offset
computation is wrong three modules away. Slicing the validated `&str` with
`str::get` instead is O(1) — one bounds check and one `is_char_boundary` — and
a wrong range costs that row its fast path. The fallback is total: `RawRow`
with no `str` behaves exactly as `decode_field` always did.

**The pass is taken lazily, on the first row that will decode.** A query that
decodes nothing at all — `--no-columns` with no filter — would otherwise pay a
megabyte of validation per chunk for bytes it never reads, and that shape is a
published figure's row. `decodes_fields` is cached on the batcher;
`reads_fields` walks the resolved tree, which is one node for the empty
conjunction a filterless query carries.

## What it was worth

Deterministic instrument, warm 3.00 GiB control on tmpfs, `release` builds of
`060551d` with and without the change, five reps each, interleaved by rep.
**Every after rep is below every before rep in all four shapes.**

| user instructions | before | after | |
|---|---|---|---|
| `query --schema-mode strings` | 26.288 G | 24.682 G | **−6.11%** |
| `query --schema-mode typed` | 96.115 G | 93.785 G | **−2.42%** |
| `query --no-columns` | 4.2532 G | 4.2093 G | −1.03% |
| `parse` | 1.40379 G | 1.40461 G | +0.06% |

Spreads over five reps: 0.14% / 2.63% / 0.0004% / 0.0009% before, 0.02% /
1.05% / 0.0004% / 0.0003% after — the typed leg is the loose one and its two
ranges are still disjoint (94.484–97.015 G against 93.080–94.065 G).

The scripts and their output are `runs/measure-7.6.sh` / `.tsv`,
`runs/measure-7.6-arrays.sh` / `.tsv` and `runs/measure-7.6-budget.sh` /
`.tsv`, with the ten profiles as `runs/profile-7.6-*` and the two binaries as
`runs/pgdq-7.6-{before,after}`. They are `runs/` artifacts, not harness
figures: they hardcode this machine's paths and a binary built from a working
tree.

The profile agrees within one sitting: `core::str::converts::from_utf8` is
**7.81%** of a `strings` profile and **2.60%** of a typed one before, and is
absent from both after, against **2.62%** and **0.85%** for the bulk pass and
the `memrchr` that finds its cut. `push_row` (children) 49.92% → 46.97% on
`strings`.

**`parse` moves by +0.06% and that is code layout, not work.** A `parse`
decodes no field, so nothing this slice added is on its path; the reading is
0.8 M instructions on 1.4 G and the phase already has a standing rule for
differences of that shape ([`measurements.md`](measurements.md), "Two builds of
one source can differ by layout").

**The library's per-row budget is re-derived from its own sitting** rather than
left to be inferred — [`architecture.md`](architecture.md), "The library's own
per-row budget". The row that shrank is the field split inside `push_row`,
because the per-field validation sat there and not in the decode's bucket.

## The finding that was not this slice's subject

**The census's field split was the byte loop, not the census.** `split_fields`
was `line.split(|&b| b == DELIMITER)` — a closure per byte — and it is now
`memchr`, because that is what `field_ranges` had to be for a row to be sliced
out of a `str`. `map::Builder::on_row` is the only hot caller, and on the
`--arrays --composite` file a whole-file `parse` falls **17.781 G → 2.919 G**
user instructions, a factor of **6.1**, with the cache it writes byte-identical
and the row counts unchanged. `on_row`'s self share falls 87.9–90.2% →
5.96–6.25% over three profiles each.

Two consequences the next slices inherit:

- **7.7's stake is smaller than its row says.** "the census re-splits every
  brace-bearing row" is still true, and a whole `parse` of the file where that
  costs most now runs on a sixth of the instructions, so what is left to win by
  *sharing* one split is the walk, not the split.
- **`measurements.md`'s "The census on array-bearing rows" is moved by more
  than any figure this phase has touched**, and it was already stale on
  `copy.rs`. No sweep is owed here (7.12's pair re-takes every table); what is
  owed is that nobody quotes the published cell in the meantime, which is what
  [`../status/STATUS.md`](../status/STATUS.md) now says.

Reasoning: [`../status/history/2026-09-04.md`](../status/history/2026-09-04.md),
"The census's field split was the byte loop, not the census".

## Deliberately not done

- **No sharing of the split across consumers.** That is 7.7, and it is a rework
  of an already-tested core path in three modules; this slice only made the
  splitter itself SIMD and gave it a range-yielding form.
- **No change to `Event::Row`.** Putting the `str` on the scanner's event would
  make L1 decide something only L4 knows — whether anything will decode — and
  would oblige every one of the three read loops to validate.
- **No sweep and no figure re-take.** Every figure that times a `pgdq` run was
  already stale on `copy.rs`; 7.12's pair is what re-takes them.
- **No second look at `unescape_field`**, which is now 22.45% of a `strings`
  profile and the largest single library bucket left. It is an escaping
  question and belongs to whichever row takes it, not to this one.
