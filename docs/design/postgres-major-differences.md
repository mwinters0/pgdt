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
  <!-- difference-watermark: VD1 -->
- **An entry holds** a **Claim** naming the majors on each side, falsifiable;
  its **Proof**, upstream source first and the commit that made the
  difference where one is found; what was **Observed**, or that nothing was;
  and the **Re-verify** command.
- **When a decision starts depending on one, it moves**: the change that makes
  the decision writes the invariant, and the manual row where pgdt's reading
  changes, and strikes the entry here — the fact is stated once.
- **Walk the file at a new major**, as the invariants are walked: a release can
  add a difference, and a back-patch can remove one.

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

**Observed.** Not observed; source only.

**Re-verify.**

```sh
cd /mnt/wd12t/upstream/postgres
for v in release-v*; do echo "== $v"; sed -n '/^float4_div/,/^}/p;/^float8_div/,/^}/p' "$v/src/include/utils/float.h"; done
```

It prints both functions per release; v13 lacks `!isnan(val1)` in the
zero-divisor test and `!isinf(val2)` in the underflow test.
