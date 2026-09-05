# P7.16 — the date/time renderers and the `render_field` sink

What the rest of P7 inherits: **the CLI's render-back is no longer the largest
bucket of a typed query.** A row is now built in one buffer and the four
date/time columns write their digits into it, which takes a typed control query
down **29.5%** of its user instructions and, for the first time since
`architecture.md`'s `query-profile` section was written, puts `poll_next` above
`print_batch` — 62.51% against 36.42%. The mechanism is filed by subject:
[`architecture.md`](architecture.md), "Decoders and render-back" for the sink,
`DEC_PAIRS` and the two rejected shapes, and its `query-profile` section for the
profile that changed sides. This doc holds the apparatus, the two regressions
the obvious spellings caused, what the row's own re-derivation obligation turned
up, and what it hands 7.12.

## Module map

| File | What it is |
|---|---|
| `pgdump_query/src/decode.rs`, `DEC_PAIRS_BYTES` / `DEC_PAIRS` | the 100 two-digit decimal pairs end to end, `&'static str` at compile time — `HEX_PAIRS`'s decimal counterpart |
| `pgdump_query/src/decode.rs`, `DEC_DIGITS` | `"0123456789"`, so a lone leading digit is a `&str` slice like every other piece |
| `pgdump_query/src/decode.rs`, `push_padded` | `{:0width$}` for an `i64`: pairs, forward, whole length reserved once |
| `pgdump_query/src/decode.rs`, `push_integer` | `push_padded(.., 0)` — `i64::to_string` without the `String` |
| `pgdump_query/src/decode.rs`, `push_two` / `push_year` / `push_fraction` / `push_civil_date` | the calendar and clock fields, and the `YYYY-MM-DD` head a `date` and a `timestamp` share |
| `pgdump_query/src/decode.rs`, `render_{date32,time64_micros,timestamp_micros}_into` | the sink forms; the owned ones are wrappers over them |
| `pgdump_query/src/decode.rs`, `prior_shape::render_{date32,time64_micros,timestamp_micros}` | the three previous implementations and their `format_hms_frac`, verbatim, as the oracle |
| `pgdump_query/src/batch.rs`, `render_field_into` | the form that does the work: appends and answers `bool`, `false` being SQL NULL |
| `pgdump_query/src/batch.rs`, `render_field` | a wrapper over it, pre-sized at 16 bytes |
| `pgdump_query-cli/src/main.rs`, `print_batch` | one `String` per *batch*, cleared per row |
| `pgdump_query/tests/render_allocations.rs` | a counting global allocator, and the per-column allocation budget of one control row |
| `pgdump_query/src/decode.rs`, `differential::{date_and_time_render,integer_render}_agrees_*` | the two differential tests |

`predicate.rs`'s `comparison_form` calls these renderers once per term per query
and is untouched and simply cheaper, exactly as it was for 7.15.

## The two obvious spellings are both slower, and each cost a build to find

Neither is a subtlety of this codebase; both are properties of `String` and
`core::fmt` that a reviewer would have to take on trust otherwise.

**`write!(out, "{value}")` is not `to_string` without the allocation.** It was
the first shape of the integer arms, and it reaches the same `Display` impl —
through `core::fmt::write`, where `i32::to_string` is specialised away from
`core::fmt` entirely. On the `--arrays --composite` file, whose 53 array
elements a row all go through that arm, it cost **287 instructions per
element**: a whole-file typed query went **113.116 G → 112.975 G**, which is a
0.12% win where the same change on the array-free control was 28.75%. The array
column alone, projected on its own, was **+15.5%**. `push_integer` exists
because of that reading.

**A `String` that starts empty is grown twice by a ten-digit value.** With the
pair loop in place the array column was still +8.9%; `finish_grow`,
`do_reserve_and_handle` and `realloc` were 5.2% of the profile between them and
had no counterpart in the before. Two fixes, and both are in the code as
comments because neither is visible from the source: `push_padded` reserves its
whole length once, and `render_field`'s wrapper starts at
`String::with_capacity(16)` — which is under glibc's smallest chunk, so it costs
nothing over an exact fit. That took the array column to **+3.8%** and the
whole-file query to **−8.0%**.

**What is left is the wrapper split itself**, and it is not closed. An array
element goes through `render_field` *and* `render_field_into` where it used to
go through one function, and `#[inline]` on `push_padded`/`push_integer` bought
0.5% of it. The residual is **65.018 G → 67.517 G, +3.8%** on an array column
projected alone, against −29.5% on the sixteen scalar columns of the same row —
so every registered file improves and the array-bearing one improves least. Closing it means
`collect_array` not needing an owned `String` per element, which is a rework of
`nested::ArrayLiteral`'s shape and belongs to whoever takes that.

*Rejected: `str::from_utf8` over a stack digit buffer.* It is the shape
`i64::to_string` uses internally (with `from_utf8_unchecked`) and the natural
one here, and the safe version puts a per-field validation call back — 8.5% of
the array profile as `push_padded` plus 0.85% more in `from_utf8` itself.
Emitting each piece as a slice of a `&'static str` is why `DEC_DIGITS` exists
beside `DEC_PAIRS`.

## The apparatus

Six instruments, and the fifth is the control — which is **not** the one 7.15
used.

- **Whole-file byte identity, three files.** `pgdq query --dqcache none` over
  the 3.00 GiB `control`, `composite` and `arrays` inputs, before and after,
  compared by `sha256sum`: identical in all three, over 814,362 / 804,022 /
  699,962 rows. That is the row's review question — does the render path emit
  identical text — answered on real data rather than only on a corpus.
- **`perf stat -e instructions:u`**, five reps a leg, legs interleaved,
  `release` binaries of both revisions kept in `runs/pgdq-{before,after}-716`
  and built from the same path (`runs/measure-7.16.sh`). Typed control
  **49.074 G → 34.610 G** (−29.47%); `strings` control **24.793 G → 19.610 G**
  (−20.91%); `composite` **53.360 G → 39.261 G** (−26.42%); `arrays`
  **113.116 G → 104.052 G** (−8.01%). Every after rep below every before rep in
  all four.
- **Wall and user**, same binaries, three reps: typed **6.07 → 4.71 s**, user
  5.17 → 3.85, system flat at ~0.85; `strings` **3.33 s** after.
- **A criterion before/after in one sitting**, `benches/decoders.rs`, the old
  revision checked into the same tree rather than built in a worktree
  ([2026-09-04](../status/history/2026-09-04.md), "A criterion baseline taken
  from a worktree measured the wrong thing"): `date/render` **108.59 → 21.58 ns**
  (−80.1%), `time/render` **152.31 → 19.92 ns** (−86.9%), `timestamp/render`
  **330.54 → 38.39 ns** (−88.4%), `timestamptz/render` **331.07 → 39.15 ns**
  (−88.2%), all `p = 0.00`. Its controls are the groups whose functions did not
  change: `uuid/render` +1.1%, `bytea/render` flat, `numeric/render` +3.2%,
  `float/render` −4.1% — the last two are code-layout noise on untouched
  functions, which is [`measurements.md`](measurements.md)'s standing rule and
  the reason those four rows are read as a set rather than one at a time.
- **`parse` is the control**, at 1.404590 G → 1.404585 G, flat to 0.0004%.
  **7.15's control does not work for this row**: it used the `strings` query on
  the reasoning that `render_field` is not reached in that mode, and it is —
  `print_batch` renders sixteen `Utf8View` columns through it, which is why
  `strings` moves 20.9% here. A control for the render path has to render
  nothing at all.
- **A matched profile in each mode**,
  `runs/profile-query-{typed,strings}-control-after-716.{data,txt}`, from the
  `profiling` build with libc symbols resolved from the installed package.
  `strings` reads `poll_next` 73.07% against `print_batch` 25.75%, which is what
  keeps [`measurements.md`](measurements.md)'s "a mode difference is a CLI
  number" rule true with a number rather than by assertion: 0.76 s of the
  1.35 s gap is still `print_batch`.

Warm, on `/dev/shm`, `--dqcache none`, `--table public.perf`.

## The re-derivation the row owed, and what it points at

The lever table quoted **16 `String`s a row** for the rest of `render_field`
and the row required that re-derived. It is now a test rather than a paragraph:
`tests/render_allocations.rs` installs a counting global allocator and asserts
the allocations per column of the control's sixteen, in a debug build, so what
it pins is what the *source* asks for.

| Column | Allocations, after |
|---|---|
| `id`, `v_smallint`, `v_bigint` | 0, 0, 0 |
| `v_real`, `v_double` | **5**, **7** |
| `v_numeric` | **6** |
| `v_date`, `v_time`, `v_timestamp`, `v_timestamptz` | 0, 0, 0, 0 |
| `v_uuid`, `v_bytea` | 1, 1 |
| `v_bool`, `v_text`, `v_long_text`, `v_escaped` | 0, 0, 0, 0 |

**Twenty a row, and eighteen of them are the three renderers this row put out
of scope.** The 16 was low, as the row suspected, and low in the way it
guessed — it counted returned `String`s and missed the internal temporaries —
but the far larger miss is on the other side: `render_f64` and `render_decimal`
are under 3% of a profile each and own **13 of the 20 remaining allocations a
row**. The profile share and the allocation count rank them differently, and
that is the fact a later row about them starts from, not the 3%.

## What this hands 7.12

- **Nothing is ordered any more.** `7.16` was the last row ahead of `7.12` and
  the spec's standing rule stays as written: any row admitted after spec time
  against a typed-query profile lands ahead of the sweep.
- **The four typed-query tables move by more than 7.15 moved them**, and the
  `strings` legs move too, which no earlier render-path slice did. `parse` and
  the map figures do not move at all.
- **`allocator` is worth re-reading again rather than re-taking blind**, for
  7.15's reason with a bigger number behind it: this slice removes nine
  allocations a row from the CLI side and the ranking 7.13 settled adoption on
  was an allocation ranking.
- **The library's per-row budget is untouched.** `render_field_into` is outside
  `poll_next`, which is what that budget splits; the `parse` control says so
  and no library path changed.
- **`architecture.md`'s `query-profile` heading has flipped**, so a session
  quoting "the CLI's render-back is the largest bucket" from memory is quoting
  a heading that no longer exists. Cite it by its `<!-- section: query-profile -->`
  id, which is what the marker is for.

## What did not change

- **No behaviour.** Three whole-file outputs byte-identical; every one of the
  three replaced renderers checked against `prior_shape` over a swept corpus,
  and `push_integer` against `i64::to_string`. The date/time test kills the
  mutations that matter and was checked doing so: a swapped year pair, a
  fraction's last pair taken mod 10, an off-by-one in the two-digit fast path's
  range, and a sign dropped from the padding width.
- **No `unsafe`**, and no fallible conversion at run time.
- **`render_field`'s signature.** It still returns `Result<Option<String>>`, so
  `tests/`, `benches/` and `predicate.rs` compile unchanged;
  `render_field_into` is added beside it and exported.
- **No figure re-take and no `measure.py` register edit.** `DECODE`, `NESTED`
  and `QUERY_CLI` already declare the three files this touches, and every
  figure they can move was already red.
