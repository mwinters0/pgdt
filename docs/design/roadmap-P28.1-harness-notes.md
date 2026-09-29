# P28.1 — The harness: notes

What the slices after this one inherit. The spec is
[`roadmap-P28-unrepresentable-values.md`](roadmap-P28-unrepresentable-values.md),
"Evidence". No product code changed.

## What exists

- **`datafusion-pgdump/tests/unrepresentable.rs`** runs 19 queries over
  `fixtures/{13,16,18}/types/default.sql`'s `t_numeric`, `t_date` and
  `t_timestamp`, each in twelve configurations — the map's statistics gathered
  at 64-byte groups and absent; the three producer flags off, on, and on with
  `pgdump.dynamic_filter_rows`; the in-order node's two orders — under each
  mode, and holds each (query, mode) to one outcome. Two partitions a scan,
  asserted, so the two orders are every order; batches two rows long.
- **The oracle is the dump's text and the library's one decoder**
  (`decode_field`): each table read under `SchemaMode::Strings`, each field
  decoded to its declared type, a field that does not decode being
  unrepresentable. The typed mode's promise is DataFusion's answer over a
  `MemTable` with those fields NULL; the untyped mode's, over one with each
  such column `Utf8View`. The refuse mode's is a refusal *at planning* naming
  the column, or, for a column only a library-answered filter reads, the
  answer in PostgreSQL's order, written into the case (`Refuse::Answers`).
- **No mode exists, so `options(mode)` opens today's provider for all three**;
  the slice adding `unrepresentable` states it there, and nothing else in the
  harness changes for it.
- **Each case records the modes it fails in** (`Case::failing`), held exact
  both ways: a recorded mode that passes everywhere fails the test, so each
  mode's slice strikes it from every case in the change that turns it green.
  Today every case fails in the typed mode; in the untyped mode every case
  but `IS NULL` and `IS NOT NULL`, whose text answers as today's does; and in
  the refuse mode every case but the three filters the library answers
  without refusing, whose PostgreSQL-order answer is already that mode's.

## What the harness showed

- **Timing decides today's outcome through every axis it varies**: a bare
  `LIMIT` over `t_numeric` or `t_date` answers in reverse order and refuses in
  file order; `MIN(v_small) WHERE id > 0` answers only in reverse order with
  the aggregate's dynamic filter on, and not in all of those; the join ruling out `t_numeric`'s `NaN` row answers only with
  `pgdump.dynamic_filter_rows`; and `COUNT(v_small)` answers `3`, counting
  the `NaN`, from the map and refuses without it.
- **Every refusal today is at execution**, including the ones every
  configuration agrees on, so no refuse-mode case naming a column passes.
- **PostgreSQL's greatest timestamp is unrepresentable too**:
  `294276-12-31 23:59:59.999999`, `t_timestamp`'s `id` 7, does not decode,
  `Timestamp(Microsecond)` counting from 1970
  (`decode.rs`, `postgresqls_own_max_timestamp_overflows_the_unix_epoch_i64_range`),
  so the oracle NULLs it and widens both timestamp columns for it. A pushed
  `v_tstz < '2000-01-01 00:00:00+00'` under `COUNT(*)`, which projects
  nothing, refuses on it at execution: the filter ranks the infinities
  (`predicate.rs`, `special_order_key`) and decodes every other value. STATUS's "Decisions
  worth another look" carries what that asks of the spec.
- **A `Utf8View` column against a number is DataFusion casting each value to
  the number's type**: `v_small > -5` over the widened column errors on the
  first value not an integer. The untyped mode's cases compare against text
  literals or none, and a user of that mode will meet the same.
- **A `timestamptz` literal the filter pushes is DataFusion's instant**
  (`predicate=v_tstz < 946684800000000`), so the refuse mode's
  PostgreSQL-order filter over it compares the instant, not the literal's
  text.

## Not done, and why

- **No fixture holds an unrepresentable `interval`** — neither an infinity,
  which PostgreSQL 17 admits, nor a time part past Arrow's nanoseconds — so
  the harness covers `date`, `timestamp`, `timestamptz` and `numeric(p,s)`
  alone. Adding one regenerates every major's `types` fixture and every test
  reading it, a change of its own.
- **`statistics.rs`'s `statistics_never_change_an_answer` still runs in file
  order**; the spec returns it to DataFusion's schedule once the harness is
  green.
- **The predicate term has no case**: `pgdump_unrepresentable` does not exist
  to plan, and the spec's "Evidence" names query shapes, not it. Its slice
  adds its own.
