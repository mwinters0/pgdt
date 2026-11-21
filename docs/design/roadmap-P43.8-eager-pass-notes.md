# P43.8 — The eager pass in the fixtures: notes

Slice 43.8 of [`roadmap-P43-block-model.md`](roadmap-P43-block-model.md). No
format change. The open's shape is D170, which now names the eager pass.

## What the next slices inherit

- **`build_index`, `build_map` and `backfill_block` are `pgdt_fixtures`'**,
  in `crates/pgdt-fixtures/src/eager.rs` behind `mapping`. Their signatures
  and contracts are what `pgdt_mapping`'s were. Every test names them as
  `pgdt_fixtures::…`.
- **The eager pass opens each block through `pgdt_mapping::internal::open`
  under `Policy::Eager`.** That third arm censuses every block, attaches no
  observer and records the TimescaleDB catalog's rows, as `Pass::Query`
  does, but is never offered to the interior. The eager pass reads with
  `pgdt_dump::scan`, which streams no row, so it reads neither `streams`
  nor `offers_interior`.
- **`open`, `Policy` and `OpenBlock` are exported through
  `pgdt_mapping::internal`**, `OpenBlock`'s fields public.
- **A block back-filled outside a pass goes through
  `pgdt_mapping::internal::backfill_one`.** It takes a `ReadContext`, the
  statistics allowance and the five arguments the old `backfill_block`
  took after its arrangement. The fixture builds the `ReadContext` from
  the arrangement, as a pass does, with a token nothing fires. `false` now
  also covers that token stopping the read, which the old function asserted
  could not happen. The read-loop announcement stays after the check for
  a block owing nothing, so a call that reads nothing still says nothing.
- **`pgdt-fixtures` depends on `pgdt-core` behind `mapping`**, for the
  `Result` and `Error` the three functions answer. `scripts/layering.py`'s
  `GRAPH` row says so. `pgdt-core` is the root, so the graph stays one-way.
- **`pgdt-mapping`'s dev-dependency on `pgdt-fixtures` states `mapping`**,
  for its integration tests. A unit test there calling the eager pass would
  meet a second copy of the crate, as the fixtures' manifest says, and none
  does.

## Negative results

- **The eager pass names `attach_text` and `derived_diagnostics` through
  `pgdt_dump`, not `pgdt_mapping::internal`** as the spec words it.
  `pgdt-fixtures` already depends on `pgdt-dump` behind `mapping`, so a
  re-export from the mapping pass would be a second path to the same item.
- **No `--count-pair`.** The row path is unchanged. The forward pass and a
  re-read open as before, plus one `matches!` per block. The eager pass is
  called by tests alone and is never timed.
