# Phase 4.6 — the array stress data and the three figures

What the phase wrap and Phase 7 inherit from the slice that measured nested
decoding. The figures themselves live in
[`measurements.md`](measurements.md) — three new sections, "The census on
array-bearing rows costs 87% of a warm scan", "Nested decode costs what it
copies, and an element is an allocation", and "A typed query over nested
columns costs 2.9× a string one". This doc holds what is not in them.

## What landed

- `scripts/generate_perf_data.py` grows `--arrays`: three columns
  (`v_int_array`, `v_int_array_long`, `v_comp`) plus the `CREATE TYPE
  public.perf_comp AS (a integer, b text)` the composite needs — the first
  type definition this generator writes.
- `pgdump_query/benches/decoders.rs` grows a `nested` group: decode and render
  for a 4-element array, a 50-element array and a two-field composite, each
  against a `String::from` control of the same byte count.
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

**The micro's control is a copy, not a zero-copy view**, for a reason stated
at length in `decoders.rs`'s header: `batch::append_nested` has no borrowed
arm, so what a nested value can be asked to justify is the parse *on top of*
the copy it cannot avoid. Benching against the `Cow::Borrowed` view path would
have measured the thing Phase 7 might build, not the thing that exists.

**Compare nested shapes per element, never per byte.** The two array lengths
exist to give a slope, and they do: 78 ns per element decoding, 28 ns
rendering, which is one allocation each — `ArrayLiteral::elements` is a
`Vec<Option<String>>`. The composite's 217 ns against the 4-element array's
265 ns says two fields cost about what two-to-four elements cost; it says
nothing about bytes, and the 50-element array's 149× against its byte-matched
control is a number about element count wearing a byte-ratio's clothes.

**The composite has no isolated end-to-end figure.** Isolating it needs a
third generated file (~4.5 minutes to write, ~2 minutes to measure) and the
micro already separates the two by type, so the end-to-end run reports the
three nested columns together. `measurements.md` says so where the figure is.

## Two pre-existing generator facts the figures rest on

Both were found while taking the figures and neither was changed, because
changing either invalidates the regeneration command of a figure that is
already recorded.

**Three of the sixteen scalar columns do not type.** The generator declares
`time`, `timestamp` and `timestamptz`; `pgtype` maps only `time without time
zone` and the two long `timestamp` spellings, which is what `pg_dump` writes.
So `parse` reports "3 of 16 columns unmapped" on the control and "3 of 19" on
the array file — that count is these three, not an array failure. The scalar
half of the typed/strings ratio is 13 columns, which makes 1.85× a floor; the
nested attribution is untouched, since the three columns are identical in both
files and cancel. Fixing the generator would reset `whole_file.rs`'s baseline
and every typed figure taken on this input, which is why it is a STATUS
"Decisions worth another look" entry and not a change here.

**`pgdq query --schema-mode typed` and `strings` do not agree byte for byte on
this input, and that is the generator too.** `v_real` is filled with `repr()`
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

## Verification of the gate

`--arrays` off produces byte-identical output to the pre-slice script at
`--seed 42`, checked by diffing a 2 MiB run of each. That is the whole
guarantee the flag exists for: `measurements.md`'s scan-throughput table and
brace-free census figure keep the command that reproduces them.
