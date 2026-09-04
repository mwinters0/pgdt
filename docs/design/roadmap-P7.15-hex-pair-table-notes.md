# P7.15 — the hex pair table

What the rest of P7 inherits: **the CLI's render-back is no longer twice the
library.** Two functions writing a `bytea` and a `uuid` back out as text were
37.75% of a typed control query, and removing their per-byte `format!` takes the
whole query down **45.31%** of its user instructions. The mechanism is filed by
subject: [`architecture.md`](architecture.md), "Decoders and render-back", with
the `render_field_into` sink this slice refuses as the rejected alternative
beside it, and the profile restatement the row owed is in that file's
`query-profile` section. This doc holds the apparatus, the three candidate
shapes and why the third one won, and what it hands 7.12.

## Module map

| File | What it is |
|---|---|
| `pgdump_query/src/decode.rs`, `HEX_PAIRS_BYTES` | `[u8; 512]`, the 256 lowercase hex pairs end to end, built by a `const` block from the same digit string `HEX_NIBBLE` inverts |
| `pgdump_query/src/decode.rs`, `HEX_PAIRS` | the same table as `&'static str`, converted once at compile time through `const` `str::from_utf8` — so a renderer appends with `push_str` and pays no validation of its own |
| `pgdump_query/src/decode.rs`, `push_hex_pair` | `out.push_str(&HEX_PAIRS[b * 2..][..2])`, the whole per-byte step |
| `pgdump_query/src/decode.rs`, `render_uuid` / `render_bytea` | one pre-sized `String` each, 36 bytes and `2 + 2n` |
| `pgdump_query/src/decode.rs`, `prior_shape::render_{uuid,bytea}` | the two previous implementations, verbatim, as the oracle |
| `pgdump_query/src/decode.rs`, `hex_render_agrees_with_the_shape_it_replaced` | the differential test |

Nothing else changed. No caller was edited, no signature moved, and
`predicate.rs`'s `comparison_form` — which canonicalises a literal through both
functions once per term per query — is untouched and simply cheaper.

## Three shapes were measured, and the fastest is also the one with no panic path

The row says "a hex-pair table … written into one pre-sized `String`", which
three implementations satisfy. All three were built and benched in one criterion
sitting, so the numbers below are comparable:

| Shape | `uuid/render` | `bytea/render`, 128 bytes |
|---|---|---|
| *(before)* `format!("{b:02x}")` per byte | 925.82 ns | 5.3445 µs |
| `[[u8; 2]; 256]`, `String::push` twice per byte | 39.38 ns | 246.11 ns |
| `[[u8; 2]; 256]` into a `Vec<u8>`, then `String::from_utf8` | 37.50 ns | 142.17 ns |
| **`&'static str` table, `push_str` per byte** | **24.40 ns** | **130.95 ns** |

The gap between the first two candidates is the whole point: `String::push`
takes a `char`, so each of the two ASCII bytes pays a `len_utf8` branch and a
separate capacity check, and a two-byte `extend_from_slice` is one 16-bit store
under one check. The third then beats *that* while doing strictly less work —
`push_str` over a `&str` needs no final UTF-8 validation at all, where
`String::from_utf8` walks the finished buffer. The `str::from_utf8` that makes
the table a `&str` is a `const` item, evaluated at compile time, so its
`panic!` arm cannot be reached at run time and there is no fallible conversion
inside either renderer.

*Rejected: `String::from_utf8_unchecked`.* It is the only way to keep the
`Vec<u8>` shape and skip the validation, and it would buy nothing over the
shape that is already faster without `unsafe` — the same answer 7.6 and 7.9
each reached by a different route.

## The apparatus

Four instruments, and the fourth is the control.

- **A criterion before/after in one sitting.** `benches/decoders.rs`'s `uuid`
  and `bytea` groups, `--save-baseline` on the stashed tree and `--baseline` on
  the working one, both built and run from the same path so that
  [`measurements.md`](measurements.md)'s "Two builds of one source can differ by
  layout" does not apply: `uuid/render` **925.82 → 24.399 ns** (−97.36%),
  `bytea/render` **5.3445 µs → 130.95 ns** (−97.55%), both `p = 0.00`. These
  groups feed no registered figure, which is why this row owes no re-take.
- **`perf stat -e instructions:u` over the whole typed control query**, five
  reps a leg, legs interleaved, `release` binaries of both revisions kept in
  `runs/pgdq-{before,after}-715`: **89.725 G → 49.074 G** user instructions
  (**−45.31%**), every after rep below every before rep. Wall 9.40 → 6.03 s and
  user 8.53 → 5.15 s over three reps a leg, system flat at ~0.8 s.
- **A matched pair of `profiling` profiles** of the same query,
  `runs/profile-query-typed-control-{before,after}-715.{data,txt}`.
- **The `strings` control**, the same query at `--schema-mode strings`:
  **24.7935 G on both sides**, medians of three reps a leg, wall 3.75 → 3.79 s.
  `render_field` is not reached in that mode, so this is the reading that says
  the change is confined to the render path and that no `parse` figure and no
  embedder is touched.

Warm, on `/dev/shm`, over the registered 3.00 GiB brace-free control
(`--dqcache none`, `--table public.perf`), which is the shape every published
`query` figure times.

## What the profile says now

| Inclusive share of a typed control run | before | after |
|---|---|---|
| `pgdq::print_batch` | 70.61% | **52.74%** |
| ↳ `batch::render_field` | 58.76% | 34.12% |
| ↳ `alloc::fmt::format::format_inner` | 42.85% | 20.68% |
| ↳ `decode::render_bytea` | 25.66% | **0.86%** |
| ↳ `decode::render_uuid` | 10.73% | **0.48%** |
| `poll_next` — the library's whole batch stream | 28.46% | **45.83%** |

The two renderers are effectively gone: 0.86% and 0.48% of a run that is itself
45% smaller. What did **not** happen is the reversal the arithmetic invited —
`print_batch` is still the larger of the two buckets, so `query-profile`'s
heading holds, but by 53/46 rather than 71/28.

**What is left of `core::fmt` on that path is the date and time renderers.**
`format_inner` is still about a fifth of the profile, and its largest single
contributor is now `render_timestamp_micros` at **11.81%** — a
`format!("{out_year:04}-{m:02}-{d:02} …")` driving `Formatter::pad_integral` for
each zero-padded field, which is the same shape of cost this slice removed and
is unchanged in absolute terms. `render_decimal`, `render_f64`,
`render_time64_micros` and `render_date32` follow, each under 3%. This is not
admitted here: it is a lever the table does not name, and an unattended session
files rather than admits ([`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md),
"The levers"). It is filed under `STATUS.md`'s "Decisions worth another look".

## What this hands 7.12

- **The ordering is discharged.** `7.15` ran before `7.12` because four of the
  sweep's thirteen tables time a typed query and this change moves all four;
  nothing in the phase is ordered any more.
- **How much it moves them.** The four are `nested-end-to-end`,
  `cross-file-floor`, `projection-widths` and `allocator`. On the control the
  whole query falls 45.31%, and the size of the move on any given file is set
  by how much of its row is `bytea` and `uuid` — 3.3% of the control's 3,956
  bytes returning 37.75% of the time before. A file with neither column moves
  by nothing, which is why the `strings` leg of every one of those tables is
  unchanged.
- **`allocator` is worth re-reading rather than re-taking blind.** 7.13 settled
  adoption on a ranking whose largest number was the read path's allocation;
  this slice removes 85 allocations a row from the *other* end. The decision is
  already made and the sweep re-takes the table anyway, but the ratios in it are
  now taken over a binary that allocates far less per row, so a leg moving is
  expected rather than surprising.
- **The library's per-row budget is untouched**, and the `strings` control is
  the evidence: `render_field` is outside `poll_next`, which is what that budget
  splits. The row said so at spec time and the measurement agrees.

## What did not change

- **No behaviour.** Both functions are total on their input domain, so the
  corpus is enumerated rather than fuzzed: every one of the 256 pairs, in a
  single value and alone; every length 0–33 for `bytea`, including the empty
  `\x`; and for `uuid` every byte value at each of the sixteen positions the
  hyphens are interleaved into, plus 20,000 random ones. All checked against
  `prior_shape`, which holds the two previous implementations verbatim. The
  test kills the two mutations that matter and was checked doing so: an
  uppercase digit string in the table, and a hyphen at position 12 instead of
  10.
- **No `unsafe`**, and no fallible conversion at run time.
- **No public signature.** `render_uuid` and `render_bytea` keep their
  arguments and their `String` return, so `batch::render_field`,
  `predicate::comparison_form` and `benches/decoders.rs` all compile unchanged.
- **No figure re-take and no `measure.py` register edit.** `DECODE` — the
  mechanism 7.10 added — already declares `decode.rs`, so the four typed-query
  figures were already red for this file and stay red for one more reason.
