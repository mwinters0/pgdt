# P31.22.2 — An `interval` read under its column's field qualifier: notes

What the slices after this one inherit. The grammar is 31.22.1's
([`roadmap-P31.22.1-datetime-refusals-notes.md`](roadmap-P31.22.1-datetime-refusals-notes.md));
carrying a typmod to the comparison kind is 31.15's and 31.19's shape. The
entry it closes was `KD98`.

## What exists

- **`pgtype::IntervalQualifier`**, the thirteen of `opt_interval`, public and
  re-exported, and `range()`, its `INTERVAL_MASK` bits. `builtin_name` returns
  it beside the name and typmod: unquoted, the words after `interval` once the
  typmod group is cut out, folded and single-spaced; quoted, the typmod's
  first integer as a mask, as `intervaltypmodin` reads `"interval"(4)`. Words
  `opt_interval` does not read (`interval year to second`) now name no
  built-in and resolve `Unknown`, where they were `interval`.
- **`CompareKind::Interval { qualifier }` and `IntervalFields { qualifier }`**,
  `None` for a column with none. `datafusion_order` keeps it. Only
  `comparison_for`'s walk passes it to `builtin_scalar`; `map_builtin` passes
  `None`, the Arrow type and the `NestedPlan` being the same for every
  qualifier. An array and a domain carry it, as `array_in` and `domain_in`
  hand the element its typmod.
- **`datetime_in::interval_reads(text, qualifier)`** reads under that
  qualifier's range alone, a column with none under the full range; the
  majors and both `IntervalStyle`s are still all tried. `INTERVAL_RANGES` is
  gone. So a plain `interval` column now refuses what only a qualifier reads:
  `1 1` (an `interval hour`'s), `59:60` (`minute to second`'s).
- **A literal is read under no qualifier** (`literal_key`), the server
  coercing one with no typmod; the readers' keys are the same either way, the
  qualifier deciding only a text they do not read.
- **`CACHE_FORMAT_VERSION` moved past 61**: a block a strict parse checked under 61
  may hold a field this build refuses. Neither pinned digest moved.
- **Evidence**: `datetime_in`'s
  `an_interval_is_read_exactly_where_some_major_reads_it_under_its_qualifier`,
  up to three cases of each of the 19 patterns of verdicts across the
  qualifiers that a differential run found; `pgtype.rs`'s
  `an_interval_s_field_qualifier_reaches_its_comparison_kind`; `predicate.rs`'s
  `a_date_or_time_field_is_refused_only_where_no_major_reads_it`;
  `tests/decode.rs`'s
  `an_interval_field_is_refused_under_its_column_s_field_qualifier`. The run
  is `runs/interval-qualifier-oracle/`, machine-local: `gen.py` reuses
  `runs/datetime-oracle/gen_cases.py`'s vocabulary and writes 3,000 texts put
  to each qualifier, to none, and to `interval(0)` and `interval second(0)`,
  each alone; `run_oracle.sh q` takes the six servers' verdicts, and
  `compare.py q` sets this build's beside them, read from an `#[ignore]` test
  printing `interval_reads` per row of `PGDT_IQ_CASES` to `PGDT_IQ_OUT` — not
  committed, so re-added to run. All 48,000 agreed.

## Negative results and limits

- **The precision refuses nothing every major refuses**: v17 refuses rounding
  past `int64`, 13 to 16 wrap (I84), and the run found no text a precision
  decided. So `interval(3)` reads as `interval`.
- **A field is still read as written**, not truncated to its qualifier or
  rounded to its precision as the server stores it: `KD99`, filed here with
  `time(p)` and `timestamp(p)`'s rounding beside it, **(c) unowned**, no
  `pg_dump` writing such a field.
- **`bounds_set_keyed_by` still matches an interval kind by equality**: a
  term and the stored set come from one declared type, so they share the
  qualifier; a `SchemaMode::Strings` term reads `Text`.
