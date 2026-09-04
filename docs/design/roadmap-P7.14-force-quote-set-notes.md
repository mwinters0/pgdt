# P7.14 — `Syntax::force_quote` as a 256-bit set

What the rest of P7 inherits: **the nested codec's innermost predicate is one
indexed bit, not a search.** `needs_quote` asks "does this byte force quoting"
once per byte of every token in both directions, and holding the answer set as
a `&'static [u8]` made every such ask a `memchr` over four to six bytes. The
mechanism is filed by subject: [`architecture.md`](architecture.md), "The nested
literal codec", fifth load-bearing property, with the fusion this slice refuses
as the rejected alternative beside it. This doc holds the apparatus, the
apparatus *fault* it turned up, and what it hands 7.12 and 7.15.

## Module map

| File | What it is |
|---|---|
| `pgdump_query/src/nested.rs`, `ByteSet` | the whole mechanism: `[u64; 4]`, `const fn new` from a byte string, `contains` a shift and a mask |
| `pgdump_query/src/nested.rs`, `Syntax::force_quote` | typed `ByteSet`; the three `const` `Syntax` values build theirs at compile time, so nothing is initialized at run time |
| `pgdump_query/src/nested.rs`, `the_force_quote_set_holds_exactly_the_bytes_each_syntax_names` | the guard: each set against the byte string its `Syntax` was written as, over all 256 bytes |

Nothing else changed. `needs_quote` lost one `&` at the call site, `is_space`
is untouched, and no caller of `nested.rs` was edited.

## Why `is_space` stayed a separate predicate

Folding the six whitespace bytes into the same set would make `needs_quote` one
bit test instead of two, and it is safe — `force_quote` has exactly one reader.
It is not done because it changes what the field *means*: `Syntax::force_quote`
is the force-quote set of one `*_out` function, `is_space` is `array_isspace`,
and the two are separately checkable against upstream. The profile also says the
prize is small: of `needs_quote`'s 14.63%, **10.39% was the `memchr`** and the
rest is the iterator advance plus the whitespace test together.

## The apparatus

Four instruments. The third is the one that carries the slice and the fourth is
the control.

- **The registered figure**, `nested-decode-micro`, re-taken alone with
  `--figure` ([`measurements.md`](measurements.md), "Nested decode costs what
  it copies"). Decode: 218 → **170 ns** at four elements, 2.41 → **1.93 µs** at
  fifty, 114 → **84 ns** for the composite. Render: 237 → **161 ns**, 1.48 µs →
  **841 ns**, 175 → **167 ns**. The per-element slope is **48 → 38 ns**
  decoding and **27 → 15 ns** rendering.
- **A criterion before/after in one sitting**, `--save-baseline` at `39940fe`
  and `--baseline` on the working tree: −27.5%, −25.4%, −18.6% on the three
  decode rows and −26.3%, −46.1%, −3.4% on the three render rows, all
  `p = 0.00`. Read the second paragraph below before trusting how this was
  taken.
- **`perf stat -e instructions:u` over the whole `--arrays --composite` typed
  query**, three reps a leg, `release` binaries of both revisions kept in
  `runs/pgdq-{before,after}-7.14`: **162.807 G → 142.500 G** user instructions
  (**−12.47%**) and 14.96 → 13.74 s of user time, every after rep below every
  before rep.
- **The same over the typed *control***, six reps a leg, medians **90.719 G on
  both sides**. The control file has no array, composite or range column, so
  the predicate is never reached — which is the reading the per-row budget
  re-read consumes, and the reason that budget is unchanged.

**The criterion comparison had to be taken twice, and the first one was wrong
in the safe direction.** The old revision was built in a `git worktree` with
`CARGO_TARGET_DIR` pointed back at the main `target/`, so both baselines landed
in one `target/criterion` — the same source at a different path, which is
[`measurements.md`](measurements.md)'s "Two builds of one source can differ by
layout" with nothing announcing itself. It read −1.2%, 0% and −3.6% on the
decode rows against a whole-query instruction count that was already saying
−12.5%. The tell was the **copy control moving +70% on code neither revision
touched**. Re-taken by checking the old file into the same tree, the rows agree
with the instruction count. That is now a rule beside the layout one, because
the failure was conservative and therefore looked like an honest refusal.

`text_copy/array_50_len` still reads 41 ns against the 27 of the same sitting's
baseline, on `String::from`. That control has now read 24, 42, 41, 24, 24, 41
and 41 ns over seven sittings and it is bimodal; the published table's `÷ copy`
column carries the caveat.

## What the profile says now

`nested::needs_quote` was **14.63%** of a typed `--arrays --composite` run as
its own symbol and is **not a symbol at all** afterwards — it inlined into its
two callers, which carry the whole predicate at 5.47% (`scan_token`) and 3.21%
(`push_token`) of a run that is itself 12.5% smaller. Summed with `scan_token`'s
and `push_token`'s own 3.42% and 1.07% before, the three nested-token symbols go
19.12% → 8.68%, which is where the whole-run 12.47% comes from.

**The `memchr` under `needs_quote` is gone and the one under `scan_token` is
not.** Both profiles carry two `memchr_naive` frames. Before: **10.39%** under
`contains<u8>` → `needs_quote`, and **1.17%** under `contains<u8>` →
`{closure#0}` → `scan_token`, which is `stops` testing `terminators`. After: the
first is gone entirely and the second reads 1.41% of the smaller run, i.e.
unchanged in absolute terms. That second frame is the measured size of the
fusion the spec declines, and it is now a number rather than "whatever this row
leaves": ~1.4% of a typed nested query, over a one- to two-byte slice. The
cheaper half of it — giving `terminators` the same `ByteSet` — is written up
beside the mechanism as what to try first if anyone reopens it.

Profiles: `runs/profile-query-typed-arrays-{before,after}-714.txt`.

## What this hands the slices after it

- **7.12's sweep.** Four of the thirteen tables time a typed query, and
  `nested-end-to-end` is the one this reaches; it is stale for this reason as
  well as for `io.rs` and `copy.rs`. The ordering that put 7.14 ahead of 7.12 is
  discharged.
- **7.15.** The two are independent: this one is per byte of every *token* of a
  nested value, 7.15's is per byte of a `bytea` or `uuid` *value* in
  `render_field`, and neither is inside the other. 7.15's stake was taken on the
  control, which this slice leaves at 90.719 G — so that stake is still current
  and does not need re-taking against this change.
- **A register edge nothing declared.** `architecture.md`'s rejected
  viewing-builder paragraph does arithmetic on `nested-decode-micro`'s view
  control (2.96 → **3.03 ns**), and the figure's `quoted_by` named only the
  phase spec. It now names `architecture.md` too. The bound the paragraph states
  is unchanged at ≤4.7 ns an element, so 7.11's refusal is undisturbed.

## What did not change

- **No behaviour.** The truth table is identical by construction and asserted
  over the whole byte domain rather than over the cases the round-trip tests
  reach. `tests/nested.rs`'s conformance pass over every nested column of every
  `types` fixture on all six majors is the end-to-end half; the two legs also
  agree on row count (699,962) over the `--arrays --composite` file.
- **No `unsafe`**, and no run-time initialization: `ByteSet::new` is a
  `const fn` and every `Syntax` is still a `const`.
- **The phase spec's lever row is left alone.** It records the stake the phase
  committed to — 14.1%, of which 10.1% `memchr` — and progress does not go in
  the spec.
