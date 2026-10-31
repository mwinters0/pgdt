# P38.5.1 — Hypertables resolved: notes

What 38.5.1 leaves the slices after it. The spec is
[`roadmap-P38-descendants.md`](roadmap-P38-descendants.md), "TimescaleDB";
the shape is D134, the refusals `KD159` and `KD160`. The pins are checked by
`pgdt-query`'s `tests/timescaledb.rs` against 38.5's answers, every flag set,
complete map and cold pass; `pgdt`'s `tests/descendants.rs` holds `pgdt query`
and `pgdt sql` to the refusal at 18. The split that left the findings to
38.5.2 is the 2026-10-31 history entry's.

## What 38.5.2 and 38.6 read

- **`Verdict`** (`dump/hypertables.rs`) is the catalog's per-read verdict,
  built over `TimescaleDb::of` each time `table_read` runs. Absent chunks,
  each root's own rows and an unreadable catalog's database are not stated
  anywhere yet: that is 38.5.2's, and so is the channel. Of the existing four
  (`pgdt_model::diagnostic`'s module doc), these are properties of the file,
  `DiagnosticKind`'s kind, derived where `derived_diagnostics` is; a finding
  per read would be a fifth.
- **An absent chunk** is a catalog chunk not `dropped` whose table the DDL
  does not declare; it is no descendant, so nothing reads or refuses it.
  **A root's own rows** are its own block's `row_count`, at every level
  (38.5's notes).
- **`Resolution::Unreadable::hypertables`** names what the catalog's
  `hypertable` rows say by `schema_name` and `table_name` alone, `None` where
  those do not read; `pgdt info --json` carries it.
- **A refusal's precedence** under one mode is written through a root
  (`KD158`), then compressed, then an unreadable catalog; no fixture meets
  two.

## Negative results

- **A named hypertable refuses read through under an unreadable catalog
  whether or not the file holds a chunk of it**, as the spec's "every
  hypertable of that database refuses" reads; only the fallback, where no
  name reads, keys on a descendant.
- **The internal compressed hypertables need nothing of their own**: at 2.13
  their compressed relations inherit them and read as their batches; from
  2.21 nothing inherits them and they read as their own empty block, where
  the server refuses them ("no dimension slices").
- **2.21's hypercore chunk (`public.columnar`'s) is marked compressed**, so it
  refuses like any compressed chunk, though its server answers its rows
  `ONLY` too.
- **Every answer matched in both schema modes**, a compressed relation's
  `compressed_data` columns as text included.
