# Differences between PostgreSQL majors

Behaviour that differs between PostgreSQL majors which **no decision here
depends on yet**, kept because reading upstream keeps turning them up and any
one may become actionable. A difference a decision does depend on is not here:
it is an invariant in [`postgres-invariants.md`](postgres-invariants.md), and
where it changes what pgdt reads, a row of the manual's
[`../manual/type-handling.md`](../manual/type-handling.md), "Where PostgreSQL
majors differ" — together the index of the differences acted on, not repeated
here.

- **`VD<n>` is allocated on discovery, never renumbered and never reused.**
  <!-- difference-watermark: VD2 -->
- **An entry holds** a **Claim** naming the majors on each side, falsifiable;
  its **Proof**, upstream source first and the commit that made the
  difference where one is found; what was **Observed**, or that nothing was;
  and the **Re-verify** command.
- **When a decision starts depending on one, it moves**: the change that makes
  the decision writes the invariant, and the manual row where pgdt's reading
  changes, and strikes the entry here — the fact is stated once.
- **Walk the file at a new major**, as the invariants are walked: a release can
  add a difference, and a back-patch can remove one.
- `scripts/major_differences.py` holds these, and fails a `VD<n>` cited
  anywhere but here and a dated entry: a citation is a dependence.

---

## VD1 — v13 refuses a float division by an infinity, and `NaN` divided by zero

**Claim.** On v13, `float4_div` and `float8_div` — the `/` of `real` and
`double precision`, mixed widths included — raise `value out of range:
underflow` for a nonzero finite dividend over an infinite divisor
(`1 / 'infinity'::float8`) and `division by zero` for `NaN` over zero. v14 to
v18 return a zero of the quotient's sign and `NaN`. The overflow tests differ
in text and not in effect, a finite value over an infinity never being
infinite. The geometric types' arithmetic shares the function; where that
reaches a type's input it is I74.

**Proof.** `src/include/utils/float.h`, `float4_div` and `float8_div`, compared
at v13.23 and v14.24, and identical from v14.24 to v18.6. The two changes are
upstream `4fb6aeb4f6e` ("Make floating-point "NaN / 0" return NaN instead of
raising an error", 2020-07-20) and `fac83dbd6fe` ("Remove underflow error in
float division with infinite divisor", 2020-11-04), neither back-patched to 13.

**Observed.** On the pinned `-trixie` images, 13.23 refuses
`1 / 'infinity'::float8`, `-1 / 'infinity'::float8` and the `real` one with
`value out of range: underflow`, and `'NaN'::float8 / 0` and the `real` one
with `division by zero`; 14.24 and 18.6 return `0`, `-0` and `NaN`.

**Re-verify.**

```sh
cd /mnt/wd12t/upstream/postgres
for v in release-v*; do echo "== $v"; sed -n '/^float4_div/,/^}/p;/^float8_div/,/^}/p' "$v/src/include/utils/float.h"; done
```

It prints both functions per release; v13 lacks `!isnan(val1)` in the
zero-divisor test and `!isinf(val2)` in the underflow test.

---

## VD2 — v16 refuses an `xid`, `xid8` or `cid` v13 to v15 read as anything

**Claim.** On v13 to v15, `xidin`, `xid8in` and `cidin` take `strtoul` or
`strtou64` of the text with no end pointer and no error check, so they refuse
nothing: `abc` is `0`, and a value past the width wraps or saturates. v16 to
v18 read them through `uint32in_subr` and `uint64in_subr`, as `oidin` (I66),
refusing trailing garbage and a value past the width.

**Proof.** `src/backend/utils/adt/xid.c`, `xidin`, `xid8in` and `cidin`,
compared at v15.19 and v16.15. The change is upstream `eb8312a22a8` ("Detect
bad input for types xid, xid8, and cid", 2022-12-27), absent from
release-v13.23 to v15.19 and present from v16.15.

**Observed.** On the koji replica (16), `'abc'::xid` is refused as invalid
input syntax and `'4294967296'::cid` as out of range; `' 0x10 '::cid` reads
`16`. No v13 to v15 server was asked.

**Re-verify.**

```sh
cd /mnt/wd12t/upstream/postgres
for v in release-v*; do echo "== $v"; sed -n '/^xidin/,/^}/p;/^xid8in/,/^}/p;/^cidin/,/^}/p' "$v/src/backend/utils/adt/xid.c"; done
```

It prints the three functions per release; v13 to v15 pass `NULL` as the end
pointer and check nothing.
