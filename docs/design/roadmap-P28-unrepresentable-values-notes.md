# P28 — Unrepresentable values: notes

What the phase leaves later work, consolidated at its wrap: the negative
results nothing else records, and facts aimed past the phase. The spec is
[`roadmap-P28-unrepresentable-values.md`](roadmap-P28-unrepresentable-values.md);
its decisions are [`decisions.md`](decisions.md), "D35", "D38", "D53", "D77"
and "D96" to "D103", leaning on `runtime-invariants.md`'s `RT21` and `RT22`
and `postgres-invariants.md`'s `I49`; its figures are
[`measurements.md`](measurements.md)'s, "What the data level costs a parse"
first.

## Negative results

- **The null mode's NULL count cannot answer `IS [NOT] UNREPRESENTABLE`**, in
  either direction: under that view a group of NULLs may hold nothing but such
  values, so a group answers off its own count (`prune.rs`, `unrepresentable`).
- **A physical optimizer rule sees no function a plan's statistics
  answered**: `SELECT id, (SELECT MAX(x) FROM memory WHERE
  pgdump_unrepresentable(x)) FROM t_date` over an empty `MemTable` plans under
  the guard as `NULL`, from the table's exact zero rows, and runs, the function
  never evaluated; a walk of the logical plan, which still holds the `Filter`,
  refuses it. The guard's refusal test gives its table a row.
- **"Does not parse" is the decoder's grammar, not PostgreSQL's**:
  `decode_date32` takes `300000-13-45` by arithmetic, so the count calls that
  year past the calendar. No `pg_dump` writes it.
- **`&StatisticsRequest::METADATA` is not a promotable constant**: its
  selection holds a `Vec`, so a borrow of it where the future outlives the
  statement is a temporary dropped too early. Bind it first.

## Facts for later

- **A figure's container sizes the cache its query decodes.** Every
  cached-query figure's builder (`measure.DATA_LEVEL_BUILDER`) discovers its
  statistics allowance from the container (D85), and the timed query decodes
  that cache whole, statistics included.
- **`raw.json`'s `reported` resolution is the untimed builder's on a
  cached-query shape**: `parse_resolution` reads the first `scan started` line,
  which a cached `query` never prints, so every provider leg of
  `parallel-scan-throughput` reports the builder's one reader. Only `reserve`,
  whose legs are `parse`s, consumes the pair, so no table is wrong; what is
  lost on those legs is the check `parse_resolution`'s docstring names.
- **`measure.py`'s `section` for `nested-end-to-end` quotes a number the
  document's heading no longer does**, so a generated table heads that figure
  with it; the marker, not the heading, addresses a figure.
- **About one cached-`query` rep in thirty is a slow outlier**, across the
  nested, cross-file, allocator and parallel figures, where the `da05a72`
  sitting's uncached shapes read one; medians absorb it. It is unattributed,
  and not a mapping pass: a probe of the same shape found none in a slow rep's
  `stderr`.
