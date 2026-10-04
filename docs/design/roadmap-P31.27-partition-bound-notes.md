# P31.27 — A partition's bound and its root's routing named per block: notes

What the slices after this one inherit. The listing is 31.21's
([`roadmap-P31.21-strict-unchecked-notes.md`](roadmap-P31.21-strict-unchecked-notes.md));
why a bound is outside the strict promise is
[`../status/history/2026-10-04.md`](../status/history/2026-10-04.md), "A
partition's bound is outside the strict promise"; the external facts are I86.

## What exists

- **`TableDef::partition_of`**, a `PartitionOf` — the parent, qualified as a
  table key is, and the bound as text (`FOR VALUES IN ('a')`, `… FROM (…) TO
  (…)`, `… WITH (…)`, `DEFAULT`), block comments dropped and spacing
  collapsed outside a literal, `None` where the statement names a parent and
  the bound is none of `PartitionBoundSpec`'s forms. Read from `ALTER TABLE
  [ONLY] <parent> ATTACH PARTITION <partition> <bound>;` as a new
  `TableReference::PartitionOf`, folded into the partition it names, and from
  a hand-written `CREATE [FOREIGN] TABLE … PARTITION OF <parent> [(…)]
  <bound>`, whose list is read as a typed table's. `--map` labels the
  `ATTACH` span with parent, partition and bound.
- **`DatabaseMetadata::partition_bounds`** walks a partition's bound up
  through every ancestor that is a partition, and **`has_partitions`** says a
  table is some partition's parent.
- **`strict_unchecked` takes the `CopyBlock`**, not its header and database,
  because the routing reads the block's `load via partition root` marker.
  `StrictUnchecked` gains `bounds` (`UncheckedBound`: partition, parent,
  bound) and `routed_through` — the block's table where it carries the marker
  or the preamble declares a partition of it, a `--data-only` dump's marked
  block included. `is_empty` counts both.
- **The listing's lines**: `<partition> PARTITION OF <parent> <bound>: a
  partition bound, compared under its key's operator classes` per bound, then
  `routed through <table>: a row no partition's bound admits is refused`.
  `--json`'s `unchecked` carries both fields.
- **`CACHE_FORMAT_VERSION` is 66**: the persisted metadata and spans gained
  the field and the variant. `PERSISTED_INDEX` re-pinned; `GOLDEN_ORDER`'s
  digest did not move.
- **Evidence**: `preamble.rs`'s `a_partition_bound_is_read_in_each_of_its_forms`,
  `a_partition_s_parent_and_bound_are_read_from_either_statement` and
  `a_partition_s_bounds_run_up_its_ancestors`; `tests/preamble.rs`'s
  `a_partition_s_bound_and_its_root_s_routing_are_named_per_block` (the
  `partitions` schema, every major, both flag sets) and
  `a_hand_written_partition_hierarchy_is_named_per_block` (two levels, a
  `DEFAULT` declared `PARTITION OF`, a `--data-only` marked block); `pgdt`'s
  `a_strict_parse_lists_a_partition_s_bound_and_its_root_s_routing`.

## Negative results and limits

- **A partition declared `PARTITION OF` resolves no column** (`KD102`): it
  records its parent and bound, but no lookup walks to the parent's columns.
  Walking it is right only for a table declaring none of its own — an
  attached partition's list is whole and in its own order, which differs
  from the parent's in the `shuffle` fixture — so it was left apart from this
  slice rather than added to a tested walk.
- **No fixture holds a range bound, a `DEFAULT` partition or a partition of a
  partition**: the `partitions` schema has list and hash partitions of one
  level. Those forms are pinned by unit test and a hand-written dump.
- **`DETACH PARTITION` is not followed**, as `NO INHERIT` is not: no
  `pg_dump` writes either before the data.
- **The emitter register's function list lacks `dumpTableAttach`**, whose
  output this slice now reads from v14; v13's is `dumpTableSchema`'s, already
  registered. STATUS's "Decisions worth another look" holds the call.
