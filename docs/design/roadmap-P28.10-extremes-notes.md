# P28.10 — The extremes: notes

What the slices after this one inherit. The spec is
[`roadmap-P28-unrepresentable-values.md`](roadmap-P28-unrepresentable-values.md),
"Evidence". No product code changed.

## What exists

- **`public.t_extremes`** in `scripts/fixture_schema_types.sql`, at every
  major: one column per `builtin_scalar` arm that maps to an Arrow type other
  than `Utf8View` — `numeric` twice, at `(38,0)` and `(76,0)`, one per decimal
  width — and one row per kind: least, greatest, the negative and positive
  specials, `NaN`, then each edge walked from both sides (`interval`'s time
  part, `timestamp`'s `i64`, the calendar `arrow-cast` displays a date
  through). **`public.t_extremes_nested`** holds a `date[]`, a `daterange`, a
  composite with a `date` field and an `interval[]`, row 1 each holding one
  such value, row 2 none — its composite's `text` field reading `infinity`.
- **`floor_mapping.py` holds the table to the arms**, both ways, per major
  (`extremes_problems`): a typed arm no column declares, a column no typed arm
  answers, a major whose `types` dump lacks the table. A typed arm is one whose
  rendering is not the fallback's, so `numeric`, which `map_numeric` answers
  per typmod, is one.
- **`every_extreme_is_held_by_arrow_or_recorded`**
  (`datafusion-pgdump/tests/unrepresentable.rs`) decodes each extreme with
  `decode_field` and holds the value to `arrow_holds` — `arrow-cast`'s display,
  and a strict cast to `Utf8` — over all six majors, against
  `UNREPRESENTABLE`, which records each value outside its type and whether the
  decoder refused it or Arrow did. The record is exact both ways.
- **The harness reads the extremes too**: its oracle NULLs, or widens, a value
  `arrow_holds` refuses as well as one the decoder refuses, and thirteen cases
  over the two tables are recorded failing as the first nineteen were.

## What the extremes showed

- **A `date` or timestamp past `262142-12-31` is unrepresentable to
  DataFusion**, the provider's tier (spec, "Scope"):
  `arrow-cast` formats `Date32` and `Timestamp` through `chrono`, whose
  calendar ends there (`NaiveDate::MAX`, `chrono` 0.4.45), long before
  `Date32`'s `i32` or the timestamp's `i64` does. So PostgreSQL's greatest
  `date`, `5874897-12-31`, decodes to a value DataFusion displays as `ERROR:
  Cast error`, and so does `i64`'s last microsecond, `294247-01-10
  04:00:54.775807`; `262142-12-31` and its last microsecond display. The
  record's `Why` is that tier, `Engine`, or the format spec's, `Format`, and
  the library's count is held to it tier by tier
  ([`roadmap-P28.3-count-notes.md`](roadmap-P28.3-count-notes.md)).
- **`interval`'s longest time part is not the same at every major.** From 15
  it is `±2562047788:00:54.775807`; at 13 and 14 `interval2tm` holds the hours
  in an `int` and `interval_out` refuses past one, so a table can hold a value
  `pg_dump` cannot write, and the longest a dump holds is
  `±2147483647:59:59.999999`. The fixture gates the two rows on the major;
  both are past Arrow's nanoseconds either way. From 17 a literal with all
  three fields at their maximum reads as `infinity`.
- **The decoder and Arrow agree everywhere else**: every integer's, `oid`'s,
  both floats' (their specials included), both decimal widths', `uuid`'s,
  `bytea`'s, `boolean`'s and `int2vector`'s least and greatest are held; the
  interval's months and days at their `i32` bounds are held; its time part is
  held to `±2562047:47:16.854775` and refused a microsecond past. PostgreSQL's
  least `date` and timestamps, Julian day 0, are held.
- **A value the decoder accepts and Arrow cannot hold is answered, never
  refused**: `MAX(v_time)` answers the `24:00:00` DataFusion cannot display in
  every configuration of the harness, where a value the decoder refuses gives
  28.1's timing-dependent refusal.

## Negative results

- **A cast to `Utf8` is not a validity test for a binary type**: `Binary`'s
  cast reads the bytes as UTF-8, so it refuses `bytea`'s `\xff`, a value
  `Binary` holds and displays as hex. `arrow_holds` holds a binary type to its
  display alone.
</content>
</invoke>
