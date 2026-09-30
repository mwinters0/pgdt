# P28.7 — The untyped mode: notes

What the slices after this one inherit. The spec is
[`roadmap-P28-unrepresentable-values.md`](roadmap-P28-unrepresentable-values.md),
"Scope"; the decision is `decisions.md`, "D100", with "D38"'s clause.

## What exists

- **`UnrepresentableMode::Text`**, accepted as `text` by `pgdt query
  --unrepresentable`, `pgdump.unrepresentable` and the shell's
  `:unrepresentable=` suffix; the refusal (`Error::Unrepresentable`) names it
  as "the text mode".
- **`ColumnResolution::UnrepresentableValues`**, made only by a query's
  resolution: `stream::resolve_block` calls `resolve::read_as_text` after
  `resolve_columns`, with the columns `TableColumns::settle` marks — each whose
  blocks count a value in the query's tiers over every block, the refuse
  mode's count (`stream::unrepresentable_values`). `resolve_columns` itself is
  unchanged, so `pgdt info`, gathering and the count never see the variant.
- **The comparison is settled at resolution, per the query's semantics**: the
  declared plan under PostgreSQL's, `ComparisonPlan::Refused` under
  DataFusion's. So every DataFusion-side reader — the summary, the declared
  orderings, `pushdown::compared_as_text`, the dynamic filter's translation —
  treats the column as any text fallback without a line of its own, and only
  `predicate.rs` learned the variant (`compares_as_declared`).
- **`statistics_view` is `Every` under the text mode**: a widened column
  compares every value in PostgreSQL's order there, and every column left
  typed holds no value in the tiers read. `unrepresentable_reads` hands every
  column `None`, as under the refuse mode.
- **The harness is green in all three modes**, bar `KD56`: its `failing`
  records are gone, replaced by `Case::meets_kd56` and
  `Config::meets_kd56` (`Dynamic::OnWithRows`), held to still failing there,
  marked for `UF1`. The untyped mode meets `KD56` with statistics gathered as
  well, a text column having no bound to answer a `MIN` from.
- **`statistics_never_change_an_answer` runs on DataFusion's schedule**, the
  spec's "Evidence" returning it there, with the aggregate dynamic filter off
  in `sessions()` for `KD56`, marked for `UF1`, and gains an untyped pass. The
  generated pruning check draws the untyped mode in both semantics.

## What the next slices inherit

- **28.8's predicate term tests the declared type**, and a widened column
  still carries it: `ResolvedSchema::notes[i].declared`, and under PostgreSQL's
  semantics its declared comparison. Under DataFusion's the widened column's
  plan is `Refused`, so the UDF must read the declared type from the note, not
  from `comparisons`.
- **`pgdt query` announces nothing about a widened column**, as it announces
  no other column's resolution; the provider's registration reports its
  `Warning` note.

## Negative results

- **Keeping the declared plan in both semantics** needs every reader of
  `comparisons` under DataFusion's to check the resolution first; the summary
  and `partition_orders` read `bounds_read_by` directly and would read a
  declared-order bound for a text column ("D100").
