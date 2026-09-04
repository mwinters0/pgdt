# P7.11 — The viewing builder for a nested `Utf8View`

What the rest of P7 inherits: **the viewing builder is refused, and
`batch::append_nested`'s copy is now a decided property rather than a pending
measurement.** This slice lands **no library code** — it is the phase's third
measured refusal ("The phase follows the evidence", *a lever row is satisfied
by a rejection*), and the largest single prize the lever table was admitted
with.

The reading itself was taken by **7.10.1**, whose row exists to price this gate
([`roadmap-P7.10.1-typed-column-build-notes.md`](roadmap-P7.10.1-typed-column-build-notes.md),
"7.11's gate"). This doc is what that reading decided, and the discharge of the
two places in the tree that were still promising the measurement.

## The gate, and the number it refused

| | |
|---|---|
| Gate (spec, as amended) | the build **minus** the view write that replaces it, over **1 µs/row** |
| `List<Utf8View>` build, isolated, 50 elements a row | 0.383 µs/row — 7.7 ns an element |
| view write's own floor (`nested-decode-micro`) | 2.96 ns of a 7.66 ns element build |
| **prize** | **≤ 4.7 ns an element — 0.237 µs/row, 24% of the gate** |

Both bounds are generous to the lever. The 2.96 ns is a *floor* on the write —
the borrowed arm also walks the chunk deque and calls `block_for` — so the real
subtraction is larger and the prize smaller. And the file the build was read on
is not a registered input but one built to flatter the change: 21-byte elements
above `arrow`'s 12-byte inlining threshold, fifty of them a row, none quoted,
none NULL.

**A refusal read off an unregistered input is safe in the direction it is
read.** A shape chosen to maximise the prize still gives a quarter of the gate,
so no committed input can give more. The reverse claim — that this file's
0.237 µs generalises — is not made and is not needed.

## Why only one leg ever needed that file

`batch::append_nested`'s copying arm is **one** arm. It serves a `Struct`'s
fields exactly as it serves a `List`'s elements, so the registered `--arrays
--composite` file bounds the `Struct` leg on committed bytes: its `perf_comp AS
(a integer, b text)` is a `Struct{Int32, Utf8View}` whose text field is a
quoted-but-unescaped token — `scan_quoted`'s borrowed arm, the eligible shape —
and that file spends **0.19 µs a row on every Arrow append across all nineteen
columns**. That is below a fifth of the gate before the composite is separated
from the other eighteen.

So the `text[]` file answered the `List` leg alone, and the phase never lacked
evidence for the other one.

## What this slice discharged

Two live obligations named the measurement rather than its answer, and both
would have read as an open question to the next session that met them:

- [`architecture.md`](architecture.md), "Nested columns: `NestedPlan` travels
  beside the `DataType`" — the paragraph stating that nested values always copy
  said the change was one "P7 owns and measures before it takes". It now states
  the answer and points at the reading.
- `pgdump_query/src/batch.rs`, `append_nested`'s doc comment — same sentence,
  same fix, retargeted from the phase spec to the mechanism's section
  ([`../process.md`](../process.md), "A code comment cites a phase number for
  something already built").

The decision's own paragraph — the rejected alternative, its two legs and the
reopening trigger — is beside the mechanism under
[`architecture.md`](architecture.md), "The library's own per-row budget". It is
where a session about to widen the view path meets it.

## What would reopen it

**Element width first, then count.** At or below the 12-byte inlining threshold
the prize is exactly zero, since `make_view` already stores a short element
inline and a view write and a copy are the same instructions. Above it the copy
grows with the element and the view write does not, so the prize-to-build ratio
rises with width where count scales both together. On the 21-byte shape the
prize reaches the gate at roughly **210 elements a row**; the build reaches it
at 130.

That trigger is necessary and not sufficient. The change carries recursive
chunk-retention and block-invalidation at every level of `List` and `Struct`
nesting — a fixed cost in correctness surface, not a per-row one, so it does not
shrink as the prize grows. A future reading that clears the gate still owes the
review the spec's row asked for: last, and alone.

## What the next slices inherit

- **7.12's sweep is unaffected.** No timed path changed. The only edit to a
  declared path is a doc comment in `batch.rs`, which retires no acknowledgement
  and moves no figure — every table was already red against the `ba2fc12` stamp.
- **The phase's remaining rows are 7.14, 7.15 and then 7.12**, in that order,
  and this slice does not touch the reasons for it: both levers move paths the
  sweep's typed-query tables time.
- **The lever table's largest admitted prize is closed at a quarter of its own
  gate.** What took it apart was the decomposition rather than an attempt: the
  bucket that made it look large was three `malloc`/`free` pairs a row inside
  the decoders `append_typed` calls, which 7.10 removed.

## What did not change

- **No library code, no test, no fixture, no measurement.** The only source
  edit is a doc comment.
- **The phase spec.** The lever row records what the phase committed to
  measuring and what it would take to land it; progress does not go in the spec.
- **No `Decisions worth another look` entry.** The one call this slice could
  have raised — reading the gate on an input the gate does not name — was raised
  by 7.10.1 and closed before this slice began.
