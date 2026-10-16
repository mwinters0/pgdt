# P36.4.1 notes — keys compare only where an order relates them

`OrderKey`'s variant is its order. `Int` holds the integer widths alone, and
`oid`, an enum's position, `date`, `time`, `timestamp` and `timestamptz` each
have their own variant. `Bytes` is split into `MacAddr`, `Uuid` and `Bytea`.
`Decimal` carries its scale, and `-infinity` and `infinity` carry their
`Domain`. `order_keys` matches two keys of one variant first. Any other pair
goes to `order_across`, which is out of line and marked `#[cold]`: a special
value against a key of its own domain, two `Decimal` scales, a `Decimal`
against a `Numeric`, a `date` against a `timestamp`, and a `macaddr`
against a `macaddr8`. Every other pair answers `None`. `compare_keys_stored`
is now written over `compare_keys`, and `order_keys_stored` is gone. D56
carries the decision and what it rejected.

## What landed beside the row's words

- **Which pairs an order relates.** Two kinds of pair relate: those that
  PostgreSQL orders through one comparison (the integer widths, two
  `numeric` typmods, `inet` against `cidr`), and those it orders through a
  cast that needs no setting. The second kind is a `date` at midnight
  against a `timestamp` (`date2timestamp`), a `macaddr` widened with
  `FF:FE` against a `macaddr8`, and a `character(n)`'s trimmed text against
  a `text`. Pairs the server orders only under its `TimeZone` answer
  `None`: `timestamp` against `timestamptz`, `date` against `timestamptz`,
  and `time` against `timetz`. So do pairs it relates only through an
  implicit cast across families: an integer against a `numeric` or a float,
  a float against a `numeric`, and an integer against an `oid`, where
  PostgreSQL wraps a negative integer. The review replaced that boundary
  with the server's operator resolution, which it contradicts on both
  sides: the server finds `macaddr < macaddr8` ambiguous and orders the
  cross-family casts (history, 2026-10-16, "36.4.1's calls reviewed: the
  order's identity moves to `ValueKey`"). 36.4.4 lands it.
- **Two enum types still compare by position.** An enum's key carries no
  type, and `KD118` is 36.4.4's to close.
- **A special value's domain** is `DateTime` (covering `date`, `timestamp`
  and `timestamptz`), `Interval`, `IntervalFields` or `Numeric`. Within one
  domain, two infinities are equal across types: `date 'infinity'` equals
  `timestamptz 'infinity'`, as the server's casts make them. `NaN` is
  always `Numeric`'s, so a `numeric` `NaN` against a float's answers
  `None`.
- **A cross-scale `numeric` comparison goes through `NumericKey`.** It
  allocates two digit strings, which only `order_across` ever pays. Within
  one column, two `Decimal` keys always share a scale and stay on the
  match's first arm.
- **`daterange`'s successor** (`make_range`) now matches `Date` beside
  `Int`, and still skips an infinite bound.
- `POSTGRES_EPOCH_UNIX_MICROS` is `pub(crate)`, for `date_micros`.

## Tests

`key/tests.rs` covers two sets of pairs. In
`keys_no_order_relates_answer_none`, each pair answers `None` both ways.
In `keys_an_order_relates_compare_across_plans`, each pair the order
relates answers its order, reversed when the sides are swapped. The oracle's
total-order test and the golden-order digest pass unchanged, so no ordering
within a column moved. The `pgdt-codec`, `pgdt-mapping` and `pgdt-query`
suites pass.

## The count: two rises, both the slice's own code

`measure.py --count-pair HEAD` on the uncommitted tree, `9ab5860f` its
parent, whose code is 36.4's `a9718ed4`
(`runs/count-pair-36.4.1-20261016/log`, its `HANDOFF.md` beside it). It
exited 0 and printed 2 of 5 rows resolved, both rises. It was read in the
two steps written before it ran:

- **The gate holds.** The parent leg's median on `pgdt sql`,
  21,976,535,359, is inside the tree leg of 36.4's summary pair,
  21,976,387,020–21,976,865,791. On the data-level `parse` its median,
  38,444,244,646, is inside that pair's 38,444,183,208–38,444,245,872. So
  the two sittings count alike.
- **The pair refuses the slice.** The expected fall did not come:

| Row | parent, fewest–most | tree, fewest–most | median change |
|---|---|---|---|
| `parse`, data level | 38,444,242,872–38,444,246,067 | 38,827,100,630–38,827,163,080 | +382,914,200 (+0.996%) |
| `pgdt sql` | 21,976,447,318–21,976,570,431 | 21,978,384,485–21,978,873,142 | +2,211,930 (+0.010%) |

`parse` at the metadata level and both `query` rows are unresolved, flat to
within 84,000.

## Both rows counted exactly

The job is `runs/attr-36.4.1-20261016-callgrind/`, 36.4's apparatus: the
same image, the rework job's data-level run and the summary job's
`pgdt sql` run, unchanged. The pair's two legs are copied out to
`/mnt/ssd/fedora/scratch/pgdump_query/attr-36.4.1/`, where `run.sh` names
them. `analyze.py`'s docstring holds each shape's reading, written before
any count, and `reading.txt` is what it printed. The eight runs took five
and a half minutes, so the job was waited on.

- **The data-level rise is the slice's own code.** The rep pairs differ by
  +382,861,148 and +382,947,446 `Ir`, reproducing the native rise, and
  each run's statistics account is 36.4.2's. All of it is in functions whose
  machine code differs. Over 9,593,971 keyed fields, three comparisons
  each, it sums to +382.8 M:
  - `observe_row`, +182.9 M. 163.1 M of it is 17 more instructions a field,
    which copy the 64-byte key twice more between `field_key`'s result and
    the gatherer's placing it. The gatherer's source did not change; the
    key's layout did.
  - `order_keys`, +167.4 M over 28.7 M comparisons, 47.8 M of it
    `pg_float_cmp`, now inlined into it: +4.2 `Ir` a comparison net. 36.4's
    tail-called `NetworkKey`'s and `NumericKey`'s comparisons and saved one
    register. This one inlines both and saves six on every call, which
    costs more than the two rank lookups it no longer makes.
  - `field_key`, +92.7 M, 8 instructions more on the path every key takes:
    its shared return writes the key in a split around `Decimal`'s scale,
    with more spills. `special_order_key` falls by 26.4 M, and the key's
    drop glue rises by 13.4 M.
- **The `pgdt sql` rise is the slice's own code, in the summary.** The
  class of changed functions is +1,949,721 and +1,948,870 in the two rep
  pairs, and the builder caches and answers hash alike. Over the 98,304
  stored bounds the summary keys, `field_key` rises by 952,320 and
  `order_keys` by 509,783, `special_order_key` and `pg_float_cmp` fall by
  374,744, and the provider's statistics `OnceLock` initializer, into which
  the summary's `fold` is inlined, rises by 863,116. That is about 20 `Ir`
  a stored bound.

## The rise is 36.4.4's

None of it is a shape the source chose: the copies are in the gatherer's
unchanged code, and the rest is which registers `order_keys` saves and how
`field_key` writes its result. They follow from a key of one variant per
order with a scale beside `Decimal`'s integer, and no shape inside that
split recovers them. The split buys nothing within a column — the
golden-order digest did not move — and serves only `ValueKey::compare`
across plans, which nothing in the workspace calls. So the review moved the
order's identity onto `ValueKey`, and 36.4.4 returns the hot path's key to
36.4's shape (history, 2026-10-16, "36.4.1's calls reviewed: the order's
identity moves to `ValueKey`").

## What the next round inherits

- **36.5's pair prices neither this slice nor 36.4.4**: `pgdt-dump` does
  not depend on `pgdt-codec`.
- **36.4.4 starts from the kept legs.** Its rework is counted by its own
  `--count-pair`; the callgrind job counts it exactly with the reworked
  binary named in `run.sh` and `analyze.py` in place of the tree leg.
