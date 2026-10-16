# P36.4 notes — a key for every value

`pgdt_codec::key::ValueKey` is read from a column's `ComparisonPlan` in a
`ComparisonSemantics`: `ValueKey::of` for a field, `ValueKey::literal` for a
filter's literal. A nested value has a key, and two keys that do not share a
representation compare as `None`. The dispatch beneath it lives in the
`pub(crate)` child `key::order`, and only `internal` re-exports it, so
`KD116` and `KD117` are struck. Each item's rustdoc says what it answers.
`key/tests.rs` asserts the API. The dispatch's own tests moved with it, to
`key/order/tests.rs`.

## What landed beside the spec's words

- **The kind a plan is keyed by in a semantics is the kind a filter's term
  compares by.** PostgreSQL's semantics uses the register's kind, a
  divergent one included. DataFusion's uses `datafusion_order`, and a plan
  the register gives no order is read as its text. A nested plan is keyed
  only in PostgreSQL's semantics, and only where every position is ordered
  (`ComparisonPlan::orders`). Everything else has no key. This matches what
  `predicate/resolve.rs` builds a term from, so a summary's extremes and a
  filter order alike.
- **A field is read as `PostgresInvalidValues::Default` reads it**: through
  the typmod, and refused where `*_in` refuses. A nested field is read in
  the strict `*_out` grammar, so `{ 1 , 2 }` is a literal and not a field.
- **"Of different kinds" means of different representations.** A key does
  not carry the plan it was read from. Two keys whose plans share a
  representation, such as a `date` and an `integer` (both `Int`), compare as
  that representation orders, and the infinities and `NaN` compare against
  any key by rank. That falls short of the spec, as `KD118`, which 36.4.1
  closes.
- **`stored_order` answers `Option` too**, since it refines `compare`.
- **The gatherer holds no `ValueKey`.** It keys by its sets' kinds, not
  through `ValueKey::of`, which D79's Rejected line explains, and it never
  holds a nested key. So it holds `internal`'s `OrderKey`, read by
  `field_key` and `order_key` and ordered by `compare_keys_stored`, which
  asserts one kind as `compare_keys` does. `ValueKey::stored_order` is the
  same refinement over the API's key (`order_keys_stored`). `pgdt-query`'s
  summary and `partition_orders` key through the plan.
- **A nested key's comparability is checked before it is compared**
  (`value::nested::key::comparable`), so the range algebra and
  `compare_nested` are unchanged, and the predicate still calls
  `compare_nested` with its precondition. The scalar comparison has one
  match, `key::order::order_keys`, which answers `None`. `compare_keys` is
  that match with the precondition asserted.
- **`NestedKey::heap_bytes`** counts each vector's capacity and what its
  slots hold, the same way `OrderKey`'s count does. `ValueKey::heap_bytes`
  adds the box.
- `scripts/measure.py`'s `PREDICATE` mechanism declares `key/order.rs`
  beside `key.rs`.

## The count: a rise on two rows, refusing the slice

`measure.py --count-pair HEAD~1` from a detached worktree at `0588c9e8`,
`a7b6ef0e` its parent (`runs/count-pair-36.4-20261015-2048/log`, its
`HANDOFF.md` beside it). It exited 0 and printed **2 of 5 rows resolved,
both rises**, so the slice is refused (spec, "What a hot-path slice
owes"):

| Row | parent, fewest–most | tree, fewest–most | median change |
|---|---|---|---|
| `parse`, data level | 38,505,290,290–38,505,354,688 | 38,998,654,072–38,998,764,578 | +493,303,174 (+1.281%) |
| `pgdt sql` | 21,976,066,601–21,976,371,012 | 21,982,318,314–21,982,413,268 | +6,263,374 (+0.029%) |

`parse` at the metadata level and both `query` rows are unresolved, and
flat to within 60,000. Both rises reproduce outside the container: the
pair's two binaries, run on the host under `perf stat`, differ by
+493.3 M and +6.0 M, each to within 0.5 M across two reps. The data-level
pair's wall clocks are equal.

**The data-level rise is 36.4's own code**, in the gatherer functions
its key code is inlined into, the accounting's `heap_bytes` above all
([36.4.3's notes](roadmap-P36.4.3-callgrind-notes.md), an exact count). A
sampled per-symbol profile cannot place a rise of this size: one taken
here (`runs/profile-36.4-count/`) put most of it in unchanged decode code,
which was sampling error
([36.4.2's notes](roadmap-P36.4.2-attribution-notes.md)). The `pgdt sql`
row's 6 M is 60 samples, below what a sampled profile resolves. An exact
count puts it in the summary, which builds a `ValueKey` for each stored
bound but none for a row (below, "Both rows counted exactly against
`a7b6ef0e`").

## The rework: the gatherer off the wrapper

The rise is 36.4's own code, in the gatherer functions its key is inlined
into ([36.4.3's notes](roadmap-P36.4.3-callgrind-notes.md)). The rework
takes the 2026-10-16 entry's candidate: the gatherer holds the scalar key,
as `pgdt-query`'s predicate already does through `internal`, so its
accounting, its extremes and its row order match no `Key` wrapper and
unwrap no `Option`. Its key path is then `a7b6ef0e`'s with the names
changed. `internal`'s `ValueKeyExt` goes, since nothing else called it. The
golden-order digest now sorts by `compare_keys_stored`, and is unchanged.
`key/tests.rs` pins `ValueKey::stored_order`'s float zeros against it.

## The rework's count: the data-level rise recovered

`measure.py --count-pair HEAD` on the uncommitted rework, `6cbb04e6` its
parent, which carries 36.4's code unchanged
(`runs/count-pair-36.4-rework-20261016-0430/log`, its `HANDOFF.md`
beside it). It exited 0 and printed 2 of 5 rows resolved, both falls: the
data-level `parse` by 554.4 M against 36.4, and `pgdt sql` by 0.4 M. It
was read in the three steps written before it ran:

- **The gate holds.** On the data-level `parse` the parent leg,
  38,998,593,682–38,998,656,484, meets 36.4's tree leg, and its median is
  3,389 below that leg's, whose spread is 110,506. On `pgdt sql` the
  median is 22,493 above. So the two sittings count alike, and
  `a7b6ef0e`'s leg in 36.4's pair reads beside this tree leg.
- **The rise is recovered.** The tree leg is not resolved above
  `a7b6ef0e`'s. Its most is 61.0 M below `a7b6ef0e`'s fewest:

| Row, across the two pairs | `a7b6ef0e` | the rework | median change |
|---|---|---|---|
| `parse`, data level | 38,505,290,290–38,505,354,688 | 38,443,477,512–38,444,247,264 | −61,109,078 (−0.159%) |
| `pgdt sql` | 21,976,066,601–21,976,371,012 | 21,981,887,414–21,982,199,616 | +5,873,572 (+0.027%) |

`parse` at the metadata level and both `query` rows are unresolved in this
pair, flat to within 50,000, as they were in 36.4's.

## Both rows counted exactly against `a7b6ef0e`

The job is `runs/attr-36.4-rework-20261016-callgrind/`. It is 36.4.3's
apparatus: the same image, and the same two `a7b6ef0e` data-level runs.
The rework's shipped binary is copied out of the pair (`run.sh` names it).
`analyze.py`'s docstring holds each shape's gate and reading, written
before any count, and `reading.txt` is what it printed. On the `pgdt sql`
shape the builder runs natively and only the scan runs under callgrind,
each told the 512 MiB the native container gave. The four builder caches
hash alike, and so do the four answers. The six runs took five and a half
minutes, so the job was waited on.

- **The data-level fall is the rework's own code.** The rep pairs differ
  by −61,181,428 and −61,771,434 `Ir`, so the gate is reproduced. All of
  it is in functions whose machine code differs, plus the calls they make.
  Each run's statistics account is 36.4.2's. `observe_row` falls by
  59.4 M (73 a row), and the comparison by 28.7 M (one `Ir` a call, as
  36.4.3 found). `place_keyed` rises by 27.6 M (2.9 a call). The renamed
  functions net to zero. The two `heap_bytes` rows, which held more than
  half of 36.4's rise, move by under 2,000.
- **The `pgdt sql` rise is 36.4's own code, in the summary.** The rep
  pairs differ by +4,807,685 and +5,968,566, so the gate is reproduced.
  The parent's reps are 1.3 M apart, which is scheduling under valgrind.
  Every instruction of the +5.39 M mean is in changed code and the calls
  it makes. `ValueKey::of` holds +5.35 M over 98,304 calls (54 a call).
  That is the summary's `fold` keying each stored bound through the plan:
  one key per bound. 98,304 is two for each of 16 columns in each of the
  table's 3,072 one-MiB groups, so it grows with the cache's groups, not
  with its rows. `ValueKey::stored_order` holds
  +1.0 M (10 a call, the wrapper's match), and the provider's statistics
  `OnceLock` initializer, whose code changed, falls by 0.4 M.

## What the next round inherits

- **The `pgdt sql` row's rise is neither reworked nor accepted.** The
  shape that would recover it is the gatherer's. The summary already
  holds the kind a term reads its bounds by (`Accumulator::kind`, from
  `bounds_read_by`), and it never reads a nested bound (D79). So it could
  key through `internal`'s `field_key`, as the gatherer now does. Whether
  that shape is as clean is the open question STATUS's "Decisions worth
  another look" puts on the gatherer. If the gatherer's rework is affirmed,
  the summary is reworked the same way and counted by `--count-pair
  HEAD~1`. If it is reverted, the gatherer's rise comes back and both
  rises are accepted. An unattended session does not rework the summary
  while that question is open.
- **A count an unrelated commit displaces is taken from a worktree** at the
  slice's commit, as that worktree's own `measure.py --count-pair HEAD~1`
  with `PGDT_MEASURE_OUT_DIR` set to the repo's `runs/`.
- **An embedder holds no kind.** `CompareKind` still reaches an embedder
  through `ComparisonPlan::bounds_kinds` and `bounds_read_by`, the stored
  sets' names (D79). No API path needs one to build or compare a key.
