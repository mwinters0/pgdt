# P28 inbox — facts filed for its grilling

Evidence that P28 (unrepresentable values) will need. **This is a queue, not a
document**: when P28 is grilled, walk every entry, fold it into the spec or
discard it as stale, and delete this file. See `docs/process.md`, "Inboxes:
facts filed by destination".

---

## Two paths decide a refusal by timing today

**Fact.** At two partitions, with every dynamic filter off,
`SELECT v_small FROM t_numeric LIMIT 3` over `fixtures/16/types/default.sql`
answers on some runs and refuses on others, and `t_date`'s `infinity` does the
same: `CoalescePartitionsExec` takes rows in arrival order and stops at its
fetch, so a clean partition meeting the limit first hides the other's refusal.
With the filters on, an ungrouped `MIN(v_small)` does the same, a partition
skipping the `NaN`'s rows once the other has tightened the aggregate's bound.
`LIMIT 4`, one partition, and `MIN` with the filters off refuse on every run.

**Why P28 cares.** Its evidence is that every order gives one answer, and
neither path shows under a fixed schedule: `M176`'s test-only node, running a
scan's partitions in a stated order (`datafusion-pgdump/tests/in_order/`), is
the instrument, run in every order: over majors 13, 16 and 18, thirty runs
each, the blind `MIN(v_small)` refuses in every file-order run and answers
`-1.50` in every reversed one.

**Origin.** 2026-09-27, `runs/limit-refusal-20260927/` and
`runs/statistics-flake-20260927/`. *Contingent on* DataFusion 55.1's merge.

## A dynamic filter drops a row before its columns decode

**Fact.** A replay evaluates a dynamic filter's state on each row its static
filter keeps, in every block, and drops a row the state rejects before any
column of it decodes (`pgdump_query/src/stream.rs`, `DynamicRead::rejects`),
re-reading the state at each chunk ("D93"). So a projected column's
unrepresentable value in a row the state rejects is never decoded: the
refusal it would raise depends on whether the state had narrowed when the row
was read — inside a row group, where the paths above decide it between
partitions and between groups. A field of the state's own that does not
decode keeps its row, so that refusal is not hidden.

**Why P28 cares.** It is a third timing path, and the only one inside a
group: the typed mode's diagnostic must not count rows a scan decoded.

**Origin.** 27.5, 2026-09-27;
`pgdump_query/tests/dynamic_filter.rs`,
`a_row_the_state_rejects_is_dropped_before_it_decodes`. *Contingent on*
row-level evaluation shipping on, which 27.7's figures decide.

## A third option: the refusal made deterministic

**Fact.** Today's refusal can itself be made deterministic: refuse at planning
wherever the map says a column the query reads holds such a value. It keeps
both the type and the data correct, at the cost of the answer — the typed
mode's diagnostic stated as an error.

**Why P28 cares.** It is a mode for a user who wants neither a silent NULL nor
a wider type, and it needs nothing the two modes' map data does not already
give.

**Origin.** 2026-09-27, found grilling the flaky test.

## What ADBC does with these values

**Fact.** ADBC 1.12.0, the floor (`fixtures/*/adbc/floor.tsv`), and upstream
main at `616acfdfc` read every `numeric` as `arrow.opaque` over a string —
`NaN` and the infinities spelled `nan`, `inf`, `-inf` — whatever values the
column holds, since the driver streams; an unknown OID as opaque binary; and a
`date` or `timestamp` infinity with the epoch offset added to PostgreSQL's
sentinel unchecked (`c/driver/postgresql/copy/reader.h`), so it arrives as a
wrong value. `numeric`'s floor row carries an extension, which takes it out of
"D38"; `date`'s and `timestamp`'s do not.

**Why P28 cares.** The typed default matches the floor's type and does better
on its data; only the untyped mode on a `date` or `timestamp` column needs
"D38"'s exception.

**Origin.** 2026-09-27. *Contingent on* the floor's pinned release.

## The typed mode changes more than a decoder

**Fact.** The library compares `NaN` and the infinities in PostgreSQL's order,
so none is ever `IS NULL`
([`../manual/type-handling.md`](../manual/type-handling.md), "`infinity` and
`NaN` filter correctly even where the column cannot hold them"), where
DataFusion would see a NULL: a pushed filter answered `Exact`
([`decisions.md`](decisions.md), "D88") would then keep rows DataFusion's
evaluation drops. NULL counts, pruning's `IS NULL` truths and an `Exact`
`COUNT(<column>)` ("D89") move with it, which `decode.rs`'s `KD8` marker
states. The diagnostic's number is deterministic only when read from the map,
over the groups the plan keeps, never from the rows a scan decoded, which a
`LIMIT` or a dynamic filter varies.

**Why P28 cares.** Each is a decision the typed mode forces, and 27.5's
per-row evaluation of a dynamic filter is one more place it lands.

**Origin.** 2026-09-27.

## What a map already knows

**Fact.** Gathering tells a value that does not decode as its column's type —
it keeps no sum for that column (`pgdump_query/src/statistics.rs`,
`ColumnStatistics::sums`) — and a type decided from what the parse saw has a
precedent in the array-shape census, recorded by every mapping pass and applied
by `retype_from_census` alone (`decisions.md`, "D43"). Statistics are optional
and may cover a prefix (`KD33`).

**Why P28 cares.** The count both modes need must be recorded by every map,
so it belongs with the census rather than behind `--statistics`.

**Origin.** 2026-09-27.

## The shell's refusal names `pgdt`'s flag

**Fact.** In `datafusion-cli-pgdump` the refusal says to use
`--schema-mode strings`, which is `pgdt`'s flag; the shell's remedy is the
`--dump` suffix `:strings` or `pgdump.schema_mode`.

**Why P28 cares.** It rewrites that refusal, and the untyped mode is the
remedy it will name.

**Origin.** 2026-09-27, `runs/limit-refusal-20260927/`.
