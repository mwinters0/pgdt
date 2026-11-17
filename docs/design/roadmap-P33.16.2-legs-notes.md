# P33.16.2 notes — the term legs and their factors

**`long-line-resident` was taken at `93428ef7`, the commit carrying the
instrument, and folded in** ([`measurements.md`](measurements.md), "What a held
line holds resident"), which reads it against the falsifiers
`run_long_line_resident` states and the spec's account. The launch's log is
`runs/long-line-resident-20261111-1243/`, the sitting's tables, readings and
instrument reports `runs/measure-20261111T125321/`. Every kind's factor was
read, no leg was refused or killed, and every answer was the input's rows.

- **The `introspect` build reports the factor its query planned**:
  `planned_factor=<held>/<per>`, or `none`, the largest any replay of the
  process planned, `plan_replay` handing each plan's to
  `pgdt_core::instrument`'s `factor.rs`; absent where nothing was planned.
  Held by `plan/tests.rs`'s
  `a_planned_replay_hands_its_factor_to_the_instrument` and `introspect.rs`'s
  `a_planned_factor_is_one_reading`.
- **Three term kinds** (`generate_long_lines.py`, `LONG_LINE_TERM_KINDS`):
  `int8array`, a `bigint[]` of single digits, and `textarray`, a `text[]` of
  one-letter elements, each read by the provider's query; `jsonb`, an object
  of empty keys, read by `pgdt query`'s ordering filter `v>{}` projecting
  `id`. Each reads at one partition alone, and the bound beside its ladder,
  in a second sweep with its 512 MiB anchor as every other kind does
  (`textarray` since 33.7).
- **Each kind's factor is read before any timed leg**: one instrument leg a
  kind with a query or filter pass (`LONG_FACTOR_FAMILY`), over its 32 MiB
  input on the introspection build, refused where its reports name no factor
  or it answers short (`long_line_factors`).
- **The allowance is the sitting's** (`long_line_allowance`): a timed leg's
  shape states `ALLOWANCE_SLOT`, its spec the number (`RunSpec.allowance`,
  part of its key), sized from each kind's line plus its read factor, a
  parse's from its line alone. `LONG_LINE_WHOLE_VALUE_CHARGE` is gone.

## What the readings found

- **A factor holds wherever no builder doubles**: text, escaped text and
  `bytea` queries hold their line and one decoded copy, the `jsonb` filter
  its line and its key's planned factor, at every length to the bound.
- **`int8[]` and `text[]` fire four falsifiers**, so no factor is read off
  their ladder: at 512 MiB and the bound each holds its line, its values and
  every buffer their doubling outgrew, between one plus the planned factor and one plus
  twice it, the term 33.24 removes.
- **A one-worker parse is flat to about a seventieth of a byte a line byte**,
  unattributed.

## What 33.24 inherits

- **A second values-sized buffer, unnamed.** At 32 MiB the instrument legs'
  reports hold two of a line's value buffers live in Rust's heap over
  `int8[]` and `text[]`, and at 128 MiB both cells hold about one more than
  their doubling can; at 288 MiB the account sums without it, and at 512 MiB
  and the bound it cannot be told from the doubling's retention. Builders
  reserved to the bound remove the doubling, not necessarily this, so it is
  attributed before the reservation is designed against the figure: the
  `introspect` build's live and committed peaks over the 128, 288 and 512 MiB
  inputs, and a run at `MIMALLOC_PURGE_DELAY=0` (the figure's prose names
  both).
- **The allowance every leg states is outgrown** by `int8[]` at the bound and
  `text[]` at 512 MiB. It is inert today; once 33.24 holds a nested value to
  what it reserves, whether those cells fit the allowance sized from the
  plans' factors is 33.7's re-take's to read.
- **The introspection build is `alloc-builds/image/introspect`**, which the
  figure rebuilds from the commit it runs at (`ensure_instrument_binary`).

## Negative results

- **The provider cannot hold a `jsonb` key**: a filter is pushed `Exact` only
  where it answers as DataFusion does (D88), and DataFusion compares a `jsonb`
  column as its text, so no key factor is planned. `pgdt query` compares as
  PostgreSQL does, and its printing renders `id` alone.
- **The instrument legs sweep in their kind's ladder sweep**, not one of
  their own: an input staged in a second sweep must be paired there by a
  subtraction (`test_measure.py`'s `test_a_split_figure_covers_its_warm_set`).
- **A term's kind reads no four-partition column, and `text[]` no bound**:
  the cut's count is every other kind's column, and `text[]` decoded at
  `161/16` with its builders' doubling outgrows `LONG_LINE_MEMORY` at either
  (the spec's "How it is shown").
