# P36.4.3 notes — 36.4's rise, counted exactly

No code landed. The job is `runs/attr-36.4.3-20261016-callgrind/`.
`run.sh` ran it and `analyze.py` read it; that script's docstring holds the
gate and the reading, written before any count existed. `reading.txt` is
what it printed, and `{parent,tree}-{1,2}.out` are the four callgrind files
(`--dump-instr=yes`, so `callgrind_annotate` can resolve them by
instruction). Each run is 36.4's data-level `--count-pair` shape on the same
`control.sql`, using the two shipped binaries 36.4.2 kept. It ran in the
register's runtime image with valgrind added (`image/Dockerfile`), four
4 GiB containers at once. The whole job took four minutes, so no handoff was
needed.

**The rise is 36.4's own code.** Callgrind reproduces the native rise
exactly, and every instruction of it is in functions whose machine code
36.4 changed:

- **The gate reproduced.** The two rep pairs differ by +493,286,345 and
  +492,625,967 `Ir`, against the native +493,301,876 `instructions:u`. Each
  leg's spread between its reps is under 0.7 M. The parent's total,
  38,515,898,633, sits within 0.03% of its native count.
- **Changed code holds +493,301,636 of the +492,956,156 mean.** Identical
  code called as often holds +2,792, and identical code with a changed call
  count holds −355,355, mostly mimalloc's slow path called less often. So
  36.4.2's +410 samples in identical decode code (`from_utf8`,
  `unescape_field`, …) were sampling error, not layout. `from_utf8` moves +4
  `Ir`.
- **Every run's statistics account matches 36.4.2's** (`held
  bytes=11884774 peak_bytes=12356065`), so gathering made the same
  decisions under callgrind. Each run was told `--memory 2147483648`, the
  limit the native count discovered, because the container was larger to
  hold valgrind.

## Where it sits

The renamed key functions net to nothing, apart from the comparison, which
falls. The rise is in the gatherer's own functions, into which 36.4's key
code is inlined. Self `Ir` change, the reps' mean:

| Function | Δ self `Ir` | Calls | Δ a call |
|---|---|---|---|
| `GroupState::heap_bytes` | +178,224,766 | 26,012,902 | +6.9 |
| `Gatherer::observe_row` | +160,963,300 | 814,362 (a row) | +197.7 |
| `RowOrder::place_keyed` | +87,545,475 | 9,593,971 | +9.1 |
| `ViewOrders::heap_bytes` | +78,071,358 | 26,012,902 | +3.0 |
| drop of `(ValueKey, String)`, now out of line | +16,946,895 | 1,129,793 | +15.0 |
| `compare_keys` → `order_keys` | −28,708,173 | 28,708,173 | −1.0 |

Some pairs cancel exactly: `field_key` against `order::field_key`,
`special_order_key` and `pg_float_cmp` against their moved copies,
`OrderKey`'s clone, and `ValueKey`'s drop against `OrderKey`'s. Their sums
are zero to the instruction.

What the table shows is the accounting. `heap_bytes` is called 32 times a
row, and the two `heap_bytes` rows hold more than half the rise. Each of
those calls now goes through the `Key` wrapper's match (`ValueKey::heap_bytes`
in `key.rs`) before the `OrderKey` dispatch it used to be. `observe_row` and
`place_keyed` carry the wrapping, the `Option` that `stored_order` now
answers, and the moves of the wider value. The table does not split those
three apart; the `.out` files can, by instruction.

## What the next round inherits

- **36.4's refusal now leads to rework or acceptance** (spec, "What a
  hot-path slice owes"). Layout is ruled out, so how `--count-pair` is read
  is not reopened, and 36.5 may be counted with it.
- **The candidate the 2026-10-16 entry names targets exactly these rows.**
  The gatherer would keep its scalar key without the wrapper's match. It
  never holds a nested key, and it already reaches the key code through
  `internal`. Whether that shape is as clean is the rework's call to make.
  The rise is 1.28% of the data-level `parse`.
- **A rework is counted twice.** Once by `--count-pair HEAD~1`, as the spec
  requires, and once with this job's apparatus where an attribution is
  needed. To reuse the apparatus, copy `run.sh` and `analyze.py` with the new
  pair's binaries and native rise. A run of this shape takes about four
  minutes, so it is waited on, not handed off.
- **The `pgdt sql` row's +6.3 M is still unattributed.** This job counted the
  data-level `parse` alone.
