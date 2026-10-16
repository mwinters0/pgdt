# P36.4.4 notes — the order a key was read in, on `ValueKey`

`OrderKey`, `order_keys` and `compare_nested` are 36.4's again (`a7b6ef0e`),
and `compare_keys_stored` is still written over `compare_keys`, as 36.4.1
left it. `ValueKey` holds its semantics and, beside the key, its order:
`key/relate.rs`'s `ScalarOrder`, the plan's kind with what moves no key
folded away, or `NestedOrder`, the tree with each leaf's type and order.
`compare` checks the semantics, then the orders, and only then reads values.
In PostgreSQL's semantics, two keys of different orders go to
`relate::postgres`. `nested/key.rs`'s `comparable` is gone, because the
orders now decide what it decided. `CompareKind::Enum` carries the type's
qualified name. D56 carries the decision and what it rejected.

## The split: DataFusion's cross-order answers are 36.4.5's

In DataFusion's semantics, keys of two different orders answer `None`
(`KD119`, owned by 36.4.5). The spec asks for DataFusion's own answer
wherever its comparison coercion relates the two emitted types. In 55.1.0
that coercion (`datafusion/expr-common/src/type_coercion/binary.rs`,
`comparison_coercion`) casts in cases the codec cannot reproduce:
- it parses a string as the number or time it is compared with
  (`string_numeric_coercion`, `string_temporal_coercion`);
- it casts a float compared with a decimal to `Decimal128(30, 15)`
  (`coerce_numeric_type_to_decimal128`), and caps a widened decimal's
  precision.

The answer in those cases is arrow-cast's parse and cast. `pgdt-codec`
links only `arrow-buffer` and `arrow-schema` (its `Cargo.toml`, D74), so
which way 36.4.5 goes is under STATUS's "Decisions worth another look".

## What landed beside the row's words

- **Every cross-order answer was probed on the koji replica** (PostgreSQL
  16.15): `EXPLAIN VERBOSE` over a materialized CTE, to see which operator
  and casts were resolved, plus the value. The results:
  - An integer is cast to `oid`. `int2` and `int4` wrap a negative value;
    `i8tooid` refuses anything outside `0..=4294967295`.
  - An integer is cast to `double precision`. So is a `numeric`, through
    `numeric_out` and `float8in`, which refuses an overflow or a nonzero
    value that underflows to zero. A `real` is widened.
  - An integer against a `numeric` is compared exactly.
  - `date` against `timestamp` uses `date_cmp_timestamp`, and a date past
    the timestamp range is still ordered.
  - A `time` is cast to `interval`.
  - A `character(n)` is cast to `text` against `text` or `name`, but a
    `character varying` is cast to `bpchar`.
  - `oid` against `numeric` or `double precision`, and `boolean`, `uuid`,
    `bytea` or `jsonb` against anything else: no operator.
  - `macaddr` against `macaddr8` is ambiguous.
  - `time` against `timetz` is cast under the session's `TimeZone`.
- **A nested order names each leaf's type** (the declared spelling with
  its typmod cut out), as well as its order. In a rolled-back transaction,
  the server refused `text[] < varchar[]`, `posint[] < integer[]` (a
  domain), `myr < int4range` and `int2vector < smallint[]`. On records,
  `record_cmp` raised "cannot compare dissimilar column types" for
  `(integer, text)` against `(integer, varchar)`, and for `integer` against
  a domain over it. It compared `varchar(3)[]` with `varchar(5)[]`, and two
  composite types with the same column types. A range carries its
  canonical function, which tells `int4range` from a user range over
  `integer`. This is a judgement call, made under STATUS's "Decisions worth
  another look".
- **What a key's order cannot tell apart is `KD120`.** A `text` key that
  ends in a blank, against a `character(n)`, answers `None`: `text` and
  `character varying` are one `CompareKind::Text`, and the server casts
  them in opposite directions. Two `numeric[]` at different scales answer
  `None`, although the server compares them. Two range types over one
  subtype compare, although the server refuses.
- **`ValueKey::heap_bytes` counts a nested key's order tree**, but not an
  enum's labels or name, which are `Arc`s shared with the plan.
- `NumericKey::to_f64` is `numeric_float8`'s reading. `pg_float_cmp`,
  `OrderKey::rank` and `FINITE` are `pub(crate)` for `relate`.

## Tests

`key/tests.rs` has two tests. `keys_no_order_relates_answer_none` checks
32 pairs, each in both directions. `keys_an_order_relates_compare_across_plans`
checks 36 pairs, each against the server's answer and its reverse. The rest
of the `pgdt-codec` suite passes unchanged, including the oracle's
total-order test and the golden-order digest.

## The count: two falls, and what stays above 36.4

`measure.py --count-pair HEAD` on the uncommitted tree, whose parent is
`99f2f817` (36.4.1's code, `195a464a`), ran detached
(`runs/count-pair-36.4.4-20261016/log`, its `HANDOFF.md` beside it). It
exited 0 after 25.5 minutes and printed 2 of 5 rows resolved. It was read
in the two steps written before it ran:

1. **The gate holds.** The parent leg reproduces 36.4.1's tree leg
   (`runs/count-pair-36.4.1-20261016/log`), and no row resolves between
   them. On the data-level `parse` the two ranges meet and the parent's
   median is 4,109 above, within either leg's spread. On `pgdt sql` the
   parent leg lies inside that leg's range, its median 235,629 below.
2. **No row rises; both refused rows fall.** The data-level `parse` falls
   by 358,314,385 (−0.923%) and `pgdt sql` by 2,802,205 (−0.013%). The
   other three rows are unresolved.

**Against 36.4's code, the two rows part.** 36.4.1's parent leg is 36.4's
code, `a9718ed4`, and the gate ties the two sittings. On `pgdt sql` the
tree leg lies wholly below that leg, its median 825,904 under; nothing
counted that exactly. On the data-level `parse` it lies wholly above, its
median +24,603,924 (+0.064%): 93.6% of 36.4.1's rise is recovered.

**That remainder is counted exactly, and it is `CompareKind`'s width.**
`runs/attr-36.4.4-20261016-callgrind/` runs 36.4.1's data-level run
unchanged (`in-data.sh`), on 36.4's binary and this pair's tree leg. Both legs are copied
to `/mnt/ssd/fedora/scratch/pgdump_query/attr-36.4.4/`, where its `run.sh`
names them. `analyze.py`'s rules were written before the count, and
`reading.txt` is what it printed. `perinstr.py` aligns the two builds'
`observe_row` instruction by instruction, and `instr.txt` is its output.
- **The rep pairs differ by +24,678,147 and +24,557,349 `Ir`**,
  reproducing the native remainder. Every run holds 36.4.2's statistics
  bytes. The tree's peak is 512 bytes higher, which 32 kinds 16 bytes
  wider would make it; nothing counted where those kinds are held.
- **All of it is in the gatherer's `observe_row`**, at +24,602,511 over
  814,362 calls in both builds. Its source did not change, but the
  structs it reads are 16 or 32 bytes wider. `CompareKind` is 16 bytes
  wider, because `Enum`, its largest variant, gained the type's name, an
  `Arc<str>`.
- **One instruction is the whole remainder.** One array index runs
  25,572,940 times. Its element grew from 272 to 288 bytes, and the
  compiler now multiplies with `lea` and `shl` instead of one `imul`:
  +25,572,940. A register reassignment nearby nets −970,429.

Accepting it is under STATUS's "Decisions worth another look".

## What the next round inherits

- **36.5 is counted against the commit that carries this slice.**
  `pgdt-dump` does not depend on `pgdt-codec`, so 36.5's own pair does not
  price this slice.
- **The kept legs price any rework of `CompareKind::Enum`.** It is counted
  by its own `--count-pair`. The callgrind job counts it against 36.4 with
  the reworked binary named in `run.sh` and `analyze.py` in place of the
  tree leg. `perinstr.py`'s addresses are the two kept binaries'.
