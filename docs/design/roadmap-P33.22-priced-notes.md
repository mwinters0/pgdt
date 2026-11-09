# P33.22 notes — 33.13.4's and 33.13.5's readers priced

**Both count pairs are read** (`runs/count-pair-33.22-20261109/`, its
`HANDOFF.md` and one log a pair), each from a clean detached checkout of its
commit, as P33's spec's "How it is shown" asks
([`decisions.md`](decisions.md), "D108").

- **33.13.5, `f799618c` against `89cf0a30`, resolved no row**
  (`log-33.13.5`): `numeric`'s reader on `NumericIn`, in `field_key` and in
  the batch's `Decimal128`, costs nothing the pair can tell from its spread,
  and owes nothing.
- **33.13.4, `89cf0a30` against `d33fa096`, resolved a rise on three rows**
  (`log-33.13.4`): the data-level parse, `query --schema-mode typed` and
  `pgdt sql -c`. The metadata-level parse and `query --schema-mode strings`,
  which read none of its readers, resolved nothing.

**callgrind reproduces each rise on both rep pairs and places all of it in
33.13.4's own code** (`callgrind-33.13.4/reading.txt`, each row's
`*-place-<rep>.txt` beside it; the two rep pairs agree function for
function):

- **The data-level parse's is `field_key`'s integer and `uuid` keys**
  (`statistics/gather/bounds.rs`). `KeyRead::run` calls `KeyRead::byte` once
  a byte, out of line, matching the reader's kind afresh each time, and it
  calls `IntIn::byte`, out of line too; the two replace `int_in`'s and
  `decode_uuid`'s slice reads. `KeyRead::byte`'s own count holds the
  machines it inlined, `UuidIn::byte` among them, so how much of it is the
  dispatch is unattributed; the per-instruction counts (`--dump-instr`) on
  the kept legs would attribute it. `KeyRead::finish` and `field_key` carry
  what `int_in` inlined.
- **The typed query's and `pgdt sql`'s is the batch's `uuid`**:
  `decode_uuid` feeding `UuidIn::byte` a byte at a time, where the slice
  reader it replaced was inlined into `append_typed`. `pgdt sql` adds a
  sliver of `field_key`'s integer keys, `PgDumpTable::scan` keying the
  cache's bounds.

## What remains of 33.22

**The rise is reworked, then what remains accepted, then the box is ticked**
(D108); the rework is settled as these three, and nothing beyond them:

- **`KeyRead::run` matches once a run**, feeding each byte to the machine
  that match selected, where it matches once a byte today.
- **`IntIn::byte` and `UuidIn::byte` cost less a byte**, each still a push
  machine of fixed state, since D147 rejects a slice reader beside it:
  `IntIn` a saturated `u64` and a flag where it saturates an `i128`, its
  digit read by a table where by `char::to_digit`; `UuidIn` neither swapping
  `started` nor testing `refused` apart on every byte. The differential test
  holds each to its predecessor unchanged.
- **No `#[inline]` is the rework**: the batch's `decode_uuid` rose because
  the slice reader it replaced was inlined into `append_typed`, and an
  attribute restoring that across the crate boundary rests on codegen, which
  D108 rejects.

**One pair prices all three**: `measure.py --count-pair HEAD` on the
reworked, uncommitted tree, which `count_pair_refusal` takes as the change
measured, launched as a long job, and callgrind places what it leaves. **That
remainder is accepted**, the batch's among it, on D147's "one grammar per
type", its reason filed with D147; no second rework follows.

## Negative results

- **`--callgrind-recipe` had never run as printed**, and three faults each
  stopped or emptied it: the linked input's target was not mounted, a `pgdt`
  valgrind started was its PID namespace's init, whose handler on every
  ending signal valgrind refuses for the real-time one it reserves, and the
  placement's `awk` read no count `callgrind_annotate` right-aligns, every
  one but the widest. The recipe now mounts the input cache, starts each run
  under `sh`, and reads a count after blanks, each held by
  `test_measure.py`'s `CallgrindRecipe`.
- **A row's four runs need not run one after another**: an `Ir` count does
  not move with what else the machine runs, and twelve runs five at a time,
  each with its own cache, took minutes (`callgrind-33.13.4/run.sh`).
