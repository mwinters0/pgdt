# P38.3.1 — A partitioned table read alone answers empty: notes

What 38.3.1 leaves the slices after it. The spec is
[`roadmap-P38-descendants.md`](roadmap-P38-descendants.md), its load-via-root
decision as amended on 2026-10-31; the rule is I93, the schema's shape D132,
and what still refuses `KD158`. The oracle check is unchanged in shape:
`crates/pgdt-query/tests/descendants.rs` and `crates/pgdt/tests/descendants.rs`
now expect, under `only`, the leaves alone refused.

## What the next slices inherit

- **A read alone refuses only by its own rows**: `Refusals::of` takes no
  fallback to the descendants' refusal, so 38.5's compressed-chunk refusal,
  which refuses under both modes, still goes in both fields, and one that
  refuses only a read through goes in `descendants` alone.
- **`Table::read` under `only` reads a table's own blocks without
  `partition_root`**: a block naming a table as the root its partitions' rows
  were written through is never that table's own (I93).
- **A table with no block of its own or a descendant's** — a partitioned
  table below a load-via-root root — keeps the blocks written through its
  root on `Table::through`, which its schema read alone settles from and a
  cold pass censuses under a typed query. Nothing else reads them.
- **A block no TOC entry names (`KD1`) no longer refuses a partitioned table
  read alone**, its rows being certainly no partitioned table's; it still
  refuses each leaf below its root.

## Negative results

- **No hypertable meets this path**: TimescaleDB never makes a hypertable
  partitioned ([`roadmap-P38-descendants.md`](roadmap-P38-descendants.md),
  "TimescaleDB in a dump"), so `has_partitions` is false for every hypertable
  root and 38.5's refusals join `Refused` unaffected.
