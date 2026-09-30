# P28.3 — The unrepresentable count: notes

What the slices after this one inherit. The spec is
[`roadmap-P28-unrepresentable-values.md`](roadmap-P28-unrepresentable-values.md),
"Scope"; the decision is `decisions.md`, "D96".

## What exists

- **`CopyBlock::unrepresentable`**: one `Unrepresentable { format, engine }`
  per header column, `None` exactly where `array_shapes` is, both set by
  `CopyBlock::set_census` from a `BlockCensus` (`CACHE_FORMAT_VERSION` 31).
  `census_row` folds a row into both, so every path that censuses counts: the
  serial pass, the leader's pieces (`leader::CensusPlan`, which replaced the
  bare width) and `reread_rows`, which the back-fill and a query's census of
  a metadata-level table share. Nothing sums a table's blocks yet; a
  consumer wanting one writes the name-keyed union `union_census` is.
- **The count is `unrepresentable::Counter`** (a new L2 module), built by
  `counter_for` from the block's DDL resolved typed with no census — the
  resolution a statistics observer gathers against — and handed down through
  `map::FieldCount`, an L1 trait, as `BlockObserver` is.
- **So a count can outlive its column's type.** A column the table-wide
  census retypes to `Utf8View` (`VaryingArrayShape`) keeps the counts its
  DDL's `List` gave it; the spec's "consulted only where the resolved type
  cannot hold the value" is the reader's to apply. `pgdt info --detail`
  applies it, printing `unrepresentable:` only under a column not resolved
  to `Utf8View`.
- **The engine tier's bound is `calendar_end()`** (L1, `index.rs`), read off
  `chrono::NaiveDate::MAX` through a direct `chrono` dependency at `arrow`'s
  own version. The cache records it; `read_cache_file` refuses a cache
  counted under another as `Unusable::CalendarChanged`, overwritable, and only
  where some block holds a count — a metadata-level cache counted nothing.
- **`build_index` and `build_map` moved to `stream.rs`**, public at the crate
  root as before: counting resolves declared types, which L1 cannot, so the
  eager pass states each database's DDL at its first `COPY` block as
  `map_forward` does (`eager_pass`). `a_cold_map_file_matches_build_index` and
  the serial-against-parallel tests now hold the counts too.
- **`decode_time64_micros` refuses `24:00:00`**; a filter still orders it,
  through `decode::time_of_day_micros`. The extremes record's `Why` is now the
  tier — `Format`, the decoder refusing; `Engine`, a value `arrow-cast`
  cannot display — and `v_time` id 2 moved to `Format`.
  `every_extreme_is_held_by_arrow_or_recorded` holds each major's counts to the
  record by tier.

## What the next slices inherit

- **Every row of a data-level table with a counted column is now split by
  the mapping pass**, where before only a row holding `{` or `[` was; a
  table of nothing but held types still costs one `memchr2` a row. A block
  that also gathers statistics splits each row twice, once here and once in
  its observer. Neither is priced: the census figures are retired, and a
  profile of a data-level `parse` attributes the census's price, the count
  inside it (28.9).
- **A type mapping that changes what a column holds changes the counts** in
  every cache: it bumps `CACHE_FORMAT_VERSION` ("D96").

## Negative results

- **"Does not parse" is the decoder's grammar, not PostgreSQL's**:
  `decode_date32` takes `300000-13-45` by arithmetic, so the count would call
  that year past the calendar. No `pg_dump` writes it.
- **`chrono` cannot name a day past its own calendar**, so a refusal naming a
  later calendar end states it as days from 1970.
