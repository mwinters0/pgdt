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
- **Nothing in the workspace holds a `ValueKey`.** The gatherer, the
  summary and `partition_orders` key by the kind they chose a stored set by,
  not through `ValueKey::of`, which D79's Rejected line explains, and none
  holds a nested key. So each holds `internal`'s `OrderKey`, read by
  `field_key` or `order_key` and ordered by `compare_keys_stored`, which
  asserts one kind as `compare_keys` does. The stored order is `internal`'s
  alone, so `ValueKey` has no `stored_order`: `of`, `literal`, `compare` and
  `heap_bytes` are the embedder's.
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
`key/tests.rs` pins the float zeros: equal through `ValueKey::compare`, and
told apart by `compare_keys_stored`.

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

## The summary off the wrapper

`fold` and `column_order` key each stored bound by the kind they chose the
set by (`Accumulator::kind`, `column_order`'s `kind`, both from
`bounds_read_by`), through `internal`'s `order_key`, and order the keys by
`compare_keys_stored`. `fold` reads the kind off its accumulator, so no kind
is cloned a block. `ValueKey::stored_order` leaves the API, and
`order_keys_stored` is private to `key::order`. Both derivations of the kind
agreed wherever a bound was folded (history, 2026-10-16, "36.4's rework
recovers the data-level rise; the summary reads by kind"), so no summary or
declared ordering changes, and the `pgdt-codec` and `pgdt-query` suites pass
unchanged.

## The summary's count: the last of the rise, counted and accepted

`measure.py --count-pair HEAD` on the uncommitted rework, `dbb1a73f` its
parent, whose code is `0152fe56`'s
(`runs/count-pair-36.4-summary-20261016-0635/log`, its `HANDOFF.md`
beside it). It exited 0 and printed 1 of 5 rows resolved, `pgdt sql`
falling by 5,666,664 (−0.026%). It was read in the three steps written
before it ran:

- **The gate holds.** On `pgdt sql` the parent leg,
  21,981,926,581–21,982,426,306, meets the rework pair's tree leg, and its
  median is 242,197 above that leg's, within the spread of 312,202. On the
  data-level `parse` the median is 264 above, within 769,752.
- **The pair refuses nothing.** The `pgdt sql` fall is the 6 M that
  `ValueKey::of` and `stored_order` held, and the other four rows are
  unresolved, flat to within 61,000.
- **Against `a7b6ef0e` the row is resolved above, barely.** The tree leg,
  21,976,387,020–21,976,865,791, has its fewest 16,008 above `a7b6ef0e`'s
  most, and its median is +449,105 (+0.002%) above. So the remainder was
  counted exactly.

The exact count is `runs/attr-36.4-summary-20261016-callgrind/`, the
rework job's apparatus on the `pgdt sql` shape, with the tree binary copied
out (`run.sh` names it). `analyze.py`'s docstring holds the reading,
written before any count, and `reading.txt` is what it printed. The four
builder caches hash alike, and so do the four answers. The totals are not
gated: one binary's two reps differ by 1.4 M, all but 2,179 of it in
identical code and outside pgdt. The class of functions whose machine code differs is
deterministic, at +386,829 and +384,736 in the two rep pairs, so **the
remainder is 36.4's own code**:

- `compare_keys_stored` holds 15.1 `Ir` a call where `a7b6ef0e`'s
  `stored_order` held 12.3, over the 98,272 comparisons of stored bounds:
  +275,847. It reaches `order_keys`' `Option` and unwraps it, where
  `stored_order` called `compare_keys` directly.
- The provider's statistics `OnceLock` initializer, into which the summary's
  `fold` is inlined, changed size and rises by 208,301, about 2 a bound.
- `order_keys` costs one `Ir` a call less than `a7b6ef0e`'s
  `compare_keys`: −98,272. The renamed functions net to zero.

**It is accepted** (spec, "What a hot-path slice owes"). It is about 4 `Ir`
a stored bound, so it grows with the cache's groups, not its rows. One
candidate shape exists: `compare_keys_stored` written over `compare_keys`,
as `stored_order` was. It is not taken, for three reasons:

- Its saving rests on codegen choices, which a rework was already refused
  for resting on (history, 2026-10-16, "36.4's rework recovers the
  data-level rise; the summary reads by kind").
- `compare_keys_stored` is the gatherer's comparison too, inlined into
  `place_keyed` (no symbol of it in the data-level runs), so changing it
  moves the data-level row.
- 36.4.1 reshapes the match both callers reach, and is counted by its own
  pair.

STATUS's "Decisions worth another look" holds the call.

## What the next round inherits

- **36.4.1 is counted against 36.4's commit**, by `measure.py --count-pair
  HEAD~1`. It reshapes `order_keys`, which `compare_keys_stored` unwraps on
  every stored comparison, the gatherer's and the summary's alike (above,
  "The summary's count: the last of the rise, counted and accepted").
- **A count an unrelated commit displaces is taken from a worktree** at the
  slice's commit, as that worktree's own `measure.py --count-pair HEAD~1`
  with `PGDT_MEASURE_OUT_DIR` set to the repo's `runs/`.
- **An embedder holds no kind.** `CompareKind` still reaches an embedder
  through `ComparisonPlan::bounds_kinds` and `bounds_read_by`, the stored
  sets' names (D79). No API path needs one to build or compare a key.
