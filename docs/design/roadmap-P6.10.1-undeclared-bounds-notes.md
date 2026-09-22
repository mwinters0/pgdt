# P6.10.1 — A column no DDL declared is bounded: notes

An earned slice of [`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md),
admitted in [`../status/history/2026-09-22.md`](../status/history/2026-09-22.md),
"A column no DDL declared is bounded, its set chosen by what gathering
stored". Gathering now bounds every scalar column, so a column no DDL declared
is bounded as its text. A term no longer names a stored set. It carries the
kind it compares by (`ComparisonPlan::bounds_read_by`). `prune_block`
recomputes the kinds gathering stored each set under
(`gather::stored_bounds_kinds`) and reads the set keyed by that kind
(`pgtype::bounds_set_keyed_by`). The why is [`decisions.md`](decisions.md),
"D79".

## What 6.7 and 6.8 inherit

- **Whether a column has Arrow bounds is a block's answer, not the query's
  schema's.** A query's `ResolvedSchema` names no set. 6.7's `Exact` `MIN`/`MAX`
  holds where, in every block, `bounds_set_keyed_by(stored, kind)` finds a
  set, with `kind` = `bounds_read_by(Arrow)` of the query's plan and `stored`
  gathering's kinds for the block, and every group of that set is bounded.
  Blocks of one table share their DDL, so in practice it is one answer per
  table, but nothing enforces that. `stored_bounds_kinds` is `pub(crate)`;
  6.7 decides how the provider reaches it.
- **Under `SchemaMode::Strings`, Arrow semantics reads a set wherever one is
  keyed `Text` or `macaddr`.** That covers text in any collation, an enum's,
  a bare `numeric`'s, `timetz`'s and `inet`/`cidr`'s Arrow set, `jsonb`, and
  `character(n)`. It reads none for a value-ordered kind such as `integer`,
  `interval` or a typmodded `numeric`. PostgreSQL's semantics reads none
  under `Strings`, every plan being `Refused`.
- **The pins.** `a_term_reads_the_set_gathering_stored_under_the_kind_it_compares_by`
  (`tests/pruning.rs`) covers both roads on six majors: a `--data-only`
  dump's `v_mood` and `id`, and the default dump under `Strings`, where
  `v_mood` reads its enum's Arrow set and `id > '9'` must skip nothing.
  `a_term_reads_the_set_keyed_by_the_kind_it_compares_by` (`pgtype.rs`)
  covers the chooser.

## Negative results

- **No `CACHE_FORMAT_VERSION` bump.** No kind's order or equality moved
  (D78). A cache at 24 holds an undeclared column with `bounds: None`, and
  the back-fill's `missing` guard re-reads that block on the next gathering
  `parse` (`an_undeclared_column_held_without_bounds_is_reread_for_them`,
  `tests/statistics.rs`). Until then a query reads the set as absent. The
  provider never parses, so it prunes nothing there until someone does.
- **`prune_block` resolves the block's columns once more**, as gathering
  resolved them, per block with statistics. Nothing prices it. It is one
  `resolve_columns` beside the one the query already makes per block.
- **No figure moves.** Every figure's input declares its columns.
