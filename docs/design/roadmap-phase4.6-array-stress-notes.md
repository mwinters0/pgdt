# Phase 4.6 — the array stress data and the three figures

What the phase wrap and Phase 7 inherit from the slice that measured nested
decoding. The figures themselves live in
[`measurements.md`](measurements.md) — three new sections, "The census on
array-bearing rows…", "Nested decode costs what it copies, and an element is
an allocation", and "A typed query over nested columns…". This doc holds what
is not in them. **Two of the three figures were re-taken by `M10` the same
day**, on a generator that declares the types `pg_dump` writes; the two
sections below that record why they could not be trusted are the reason, and
both are closed.

## What landed

- `scripts/generate_perf_data.py` grows `--arrays`: three columns
  (`v_int_array`, `v_int_array_long`, `v_comp`) plus the `CREATE TYPE
  public.perf_comp AS (a integer, b text)` the composite needs — the first
  type definition this generator writes.
- `pgdump_query/benches/decoders.rs` grows a `nested` group: decode and render
  for a 4-element array, a 50-element array and a two-field composite, against
  **two** controls — a `String::from` of the same byte count, and one
  `append_view_unchecked` into a borrowed block.
- The three figures, and one sentence of the array-bearing census cost in
  [`architecture.md`](architecture.md), "The array shape census".
- Two entries in [`roadmap-phase7-inbox.md`](roadmap-phase7-inbox.md) restated
  from "unmeasured" to their figures: the per-row census cost, and the
  copying-vs-viewing baseline.

No library code changed.

## The calls worth knowing about

**The no-census binary is a one-line patch, not an earlier commit.** The
existing brace-free census figure was taken by building the commit before 4.5
and alternating. That method no longer isolates anything: Phase 9 landed the
save throttle, the interrupt guard and the resume path between 4.5 and now, so
a pre-4.5 binary differs from today's in far more than the census.
`measurements.md`'s reproduce block therefore says to add a bare `return;` as
the first statement of `map::Builder::on_row` and rebuild — the pre-filter and
everything after it, and nothing else. **Revert it.** It is not a flag and
must not become one; the census is unconditional by decision
(`architecture.md`, "The array shape census").

**The end-to-end ratio needs two files because `query` has no column
projection.** There is no way to ask what three columns of one table cost, so
the control is a second 3.00 GiB file holding the same sixteen scalar columns
and nothing else. The two `strings` baselines came out within noise of each
other (9.4–9.7 s), which is the check that the zero-copy path is byte-driven
and the two files are comparable at all. A reader who does not know this will
wonder why the control is not simply `--schema-mode strings` on the array
file: that is the *other* axis, and both are used.

**The micro has two controls, and they answer different questions.** The copy
(`String::from`, same byte count) isolates the parse from the copy a nested
value cannot avoid — `batch::append_nested` has no borrowed arm — which is the
variable Phase 7 is choosing over. The view (one `append_view_unchecked` into
a block the builder does not own) is what an unescaped text field actually
costs, so it is the ratio a user experiences. Reporting only the first
understates the gap by roughly 7×, and reporting only the second measures a
path nested values do not have.

**The view control is batched ×1024 and its figure must be divided**, which is
the only bench in the file that is not directly readable. Timing one append
through `iter_batched_ref` gave ~12.6 ns against a harness floor `bool/decode`
puts at ~1.1 ns — three quarters apparatus. It is also a **floor** on the
borrowed arm rather than the arm itself: `push_utf8view_field` additionally
scans the chunk deque with `find_map` and calls `block_for`, so every ratio
taken against it bounds the real one from above. Both caveats are in the bench
header and beside the figure.

**Compare nested shapes per element, never per byte.** The two array lengths
exist to give a slope, and they do: 78 ns per element decoding, 28 ns
rendering, which is one allocation each — `ArrayLiteral::elements` is a
`Vec<Option<String>>`. The composite's 217 ns against the 4-element array's
265 ns says two fields cost about what two-to-four elements cost; it says
nothing about bytes, and the 50-element array's 149× against its byte-matched
control is a number about element count wearing a byte-ratio's clothes.

**The composite has no isolated end-to-end figure, and that is 4.6.1.** The
end-to-end run reports the three nested columns as one per-row number, because
`pgdq query` cannot project columns and separating them needs its own
generated file. 4.6 shipped the composite as a micro and read the spec row as
satisfied; review split it. The row is rewritten to what landed, the tick
stands, and the remainder is 4.6.1 — which waits on `M10` for both the
re-measurement and the `--arrays`/`--composite` flag split it needs. The
finding underneath is now a standing rule
([`roadmap.md`](roadmap.md), "A slice row that commits to a measurement names
its instrument"): the same row's array ratio named both its instruments and
was delivered with both.

## Three pre-existing generator facts the figures rest on

All three were found while taking the figures and none was changed, because
changing any of them changes the default output's bytes — which is the
regeneration command of every figure already taken on it, the scan-throughput
table and the brace-free census figure included.

**Three of the sixteen scalar columns do not type — closed by `M10`, which
fixed the generator and re-took the figures.** The generator declared
`time`, `timestamp` and `timestamptz`; `pgtype` maps only `time without time
zone` and the two long `timestamp` spellings, which is what `pg_dump` writes.
So `parse` reports "3 of 16 columns unmapped" on the control and "3 of 19" on
the array file — that count is these three, not an array failure. The scalar
half of the typed/strings ratio is 13 columns, which makes 1.85× a floor; the
nested attribution is untouched, since the three columns are identical in both
files and cancel. Fixing the generator resets `whole_file.rs`'s baseline and
every typed figure taken on this input, which is why it is **`M10`** — its own
out-of-band change, with the re-measurement in it — and not a change here.

**`pgdq query --schema-mode typed` and `strings` do not agree byte for byte on
this input, and that is the generator too — closed by `M10`, which also made a
test assert the agreement.** `v_real` was filled with `repr()`
of a Python float — a float64's 17 significant digits — in a column declared
`real`. `typed` decodes it to `Float32` and re-renders the shortest string
that round-trips an `f32`, so `-510216.29239304754` comes back
`-510216.28`; `strings` passes the field through. `pg_dump` writes what
`float4out` produced, which round-trips by construction, so the CLI's
byte-identity property is untouched by any real dump. It costs the figures
nothing — both modes walk the same bytes — but it rules out using `cmp` on the
two outputs as a smoke test on this file.

`v_bytea` is *not* an instance of this, though it looks like one: the
generator writes `\\x…` with a doubled backslash, and so does `pg_dump`
(`fixtures/16/types/default.sql`, `public.t_bytea`, is `\\xdeadbeef00ff`).
Both modes print `\x…` for it.

**The three temporal columns carry a fraction PostgreSQL would have trimmed,
and that couples them to the spelling fix.** The generator formats with
`%H:%M:%S.%f`, which is always six digits, and its timestamps are whole
seconds — so every value ends `.000000`. `decode.rs`'s `format_hms_frac` trims
trailing zeros and drops an all-zero fraction entirely, matching what
PostgreSQL emits. Today this is invisible, because those are exactly the three
columns whose declared spelling does not resolve, so they never reach a
decoder. **Correcting the spellings alone would therefore turn one silent
problem into three loud ones** — three columns that decode and re-render to a
different string than the file holds. The two have to move together.

## Verification of the gate

`--arrays` off produces byte-identical output to the pre-slice script at
`--seed 42`, checked by diffing a 2 MiB run of each. That is the whole
guarantee the flag exists for: `measurements.md`'s scan-throughput table and
brace-free census figure keep the command that reproduces them.

`M10` is the change that deliberately breaks this, and it is allowed to
because it re-takes those figures in the same change. It also splits the flag
into `--arrays` and `--composite`, so the file these figures were taken on
becomes `--arrays --composite` — one rewrite of the recorded commands, not
two.
