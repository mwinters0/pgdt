# P31.13 — `--postgres-invalid-values=default|ignore`: notes

What the slices after this one inherit. The rule is
[`roadmap.md`](roadmap.md), "A literal is guaranteed in `*_out`'s form and
never read past `*_in`'s"; the refusal it opts out of is 31.12's
([`roadmap-P31.12-field-refusal-notes.md`](roadmap-P31.12-field-refusal-notes.md));
where the mode is stated and why no cache records it is D103.

## What exists

- **`PostgresInvalidValues`, `Default` and `Ignore`**, in `scan.rs` (L1, below
  `decode`, so `ScanOptions` can hold it), on two option
  structs: `ScanOptions::postgres_invalid_values`, read by gathering alone
  (`map_file`, `gather_block_statistics`), and
  `QueryOptions::postgres_invalid_values`, read by the typed read and every
  filter leaf. The front ends: `pgdt parse` and `pgdt query`
  `--postgres-invalid-values`, `PgDumpOptions`, `pgdump.postgres_invalid_values`
  and the shell's `:postgres-invalid-values=`. `parse` is a surface D103's four
  did not name, the refusal being a parse's.
- **An ignoring parse keeps no statistic of a refused field's group**:
  `ColumnGatherer::observe` loses every bounds set's group and the group's
  dictionary, and the column's sum, so neither a query refusing the field nor
  one reading it otherwise can be contradicted by what a statistic answers.
- **An ignoring query reads only a float**: `decode::float_field` reads one
  `float_in` refuses as the parse rounds it, `±DBL_MAX` (`±FLT_MAX`) past the
  range and a signed zero below it; `predicate::read_key` keys a refused
  `Float32`/`Float64` field the same way, for a term, a membership and a nested
  leaf (`Side::Field`), static and dynamic filters alike
  (`ResolvedExpr::invalid_values`). Every other refused field fails the read as
  under `Default`. A literal is read as before.
- **Nothing persisted moved**: `CACHE_FORMAT_VERSION` stays 49.
- **Evidence**: `tests/statistics.rs`'s
  `a_parse_ignoring_refused_fields_keeps_no_statistic_of_one` (serial, 2, 4 and
  8 jobs, and a back-fill); `tests/decode.rs`'s
  `a_float_past_its_range_is_read_as_the_largest_only_when_told_to_ignore_it`
  (every major, cached and not); `decode.rs`'s
  `a_float_field_past_its_range_is_read_only_when_ignored`; `provider.rs`'s
  `a_dump_opened_ignoring_refused_values_reads_a_float_past_its_range`;
  `pgdt/tests/statistics.rs`'s
  `parse_and_query_each_take_postgres_invalid_values`; the shell's and the
  factory's option tests.

## Findings

- **A query never raised 31.12's refusal**: it gathers nothing, so a refused
  field reaches it as `FieldDecode`. Under `Ignore` that stays so for every
  kind but the float, whose reader has a value to give.

## What the slices after this inherit

- **31.14's `strict`** is a third variant on the same two structs. A resumed
  parse keeps the blocks a cache already holds, so a cache an ignoring parse
  left is not re-checked by a later `default` one; a `strict` parse promising a
  full check has to read those blocks again or refuse such a cache, and no
  cache today says which mode gathered it (D103).
