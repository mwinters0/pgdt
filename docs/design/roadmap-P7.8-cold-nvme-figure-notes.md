# P7.8 — the cold-NVMe figure

What `7.8.1` inherits: **a device on which the parse is visible, and the number
that bounds all three of its levers to 8.9%**. The figure itself is filed by
subject — [`measurements.md`](measurements.md), "Scan throughput by input
shape", the third of that section's tables — and this doc holds the apparatus,
why the regime is a regime rather than a flag, and the two readings that come
out of it.

No library code: `pgdump_query/src/` and `pgdump_query-cli/src/` are
byte-unchanged, and no Rust file is touched at all. This slice is one harness
regime and one measurement.

## Module map

| File | What it is |
|---|---|
| `scripts/measure.py`, `Config.nvme_dir` | `PGDQ_MEASURE_NVME_DIR`, the third staging area — a second *device*, not a second cache |
| `scripts/measure.py`, `Stager.nvme_path` | copy from the SSD cache, stamped with the generator's own stamp, and **kept** |
| `scripts/measure.py`, `Stager._nvme_preflight` | the area is usable and holds what still has to be copied there, checked in the first second |
| `scripts/measure.py`, `Figure.nvme_inputs` | a third input list, so a figure may want a file on two devices |
| `scripts/measure.py`, `CONTENTION_LIMITS["cold-nvme"]` | the gate's row for the regime, written out rather than inherited |
| `scripts/measure.py`, `run_scan_throughput_nvme` | the figure: `_throughput_figure` at regime `cold-nvme`, five reps |
| `scripts/measure.py`, `main`'s `--stage` | selection splits on `+` rather than matching as a substring, so `cold` does not reach `cold-nvme` |
| `scripts/test_measure.py`, `ColdNvme` | nine: the figure reads the NVMe and not the SSD, the rows match the cold table's, the three regimes resolve to three directories, a stale copy is replaced and a matching one left alone, every regime is gated, a full area is refused before any run, a figure with no NVMe inputs checks nothing and creates no directory, and a `cold` stage selection does not reach another device |

## The regime is a regime, not a flag on `cold`

Three decisions, and each of them fails the same way — a plausible table of the
wrong thing:

**Its own directory.** `cold_inputs` are read *in place* out of `cache_dir`,
which is the SSD. A figure that declared its inputs there and asked for an NVMe
reading would measure the SSD and emit a table that formats correctly.
`nvme_inputs` is a third list for that reason, and a test holds that the figure
declares no `cold_inputs` at all.

**Its own contention row.** `contention_verdict` gates nothing for a regime the
limits table does not carry, so a new regime added without a row would take
every reading it was handed however busy the machine was. `cold-nvme`'s row is
written out with the same three limits as `cold` rather than falling back to
it — the fallback is the failure, not the duplication.

**Its own `--stage` token.** `--stage` matched its argument as a *substring* of
the figure's stage, which is how `cold+warm` is selected by both `cold` and
`warm`. Left alone, `--stage cold` would have pulled the NVMe figure into a
cold-SSD sitting, and its absolutes belong to no such sitting. Selection now
splits on `+`, which is the same rule stated exactly instead of approximately.

*Rejected: generating the inputs a second time onto the NVMe.* A `--seed`ed
generator is byte-for-byte reproducible, so a copy is the same file and a 3 GiB
copy is seconds where the generator is minutes — the argument the tmpfs staging
already makes. What the copy has to carry with it is the **stamp**, or a
checkout with an existing copy benchmarks pre-change bytes forever after the
generator moves, silently, in exactly the population comparing a new number to
an old one.

*Rejected: evicting the NVMe copies at cleanup, as tmpfs is evicted.* That area
is disk. There is no budget to reclaim, nothing else contends for it, and
re-copying 9 GiB before every sitting buys nothing. It costs ~9 GiB of a volume
with 200 GiB free, and deleting it by hand is safe — the next sitting copies
what it needs.

## The reading

Five reps, taken alone on 2026-09-04 at `e889634`, `drop_caches` before every
run including before the floor:

| | Wall | × the floor |
|---|---|---|
| `dd` → `/dev/null` | 1.281 s (~2515 MB/s) | — |
| `COPY` block | 1.406 s | **1.10×** |
| Large-object region | 1.532 s | 1.20× |
| `INSERT` run | 3.40 s | **2.66×** |

**The device is 4.5× the SATA SSD, and that is the whole point.** The cold-SSD
table reads all three shapes at 1.00–1.02× its floor, which was never a
statement about the algorithms: at 557 MB/s that disk hides everything. Here two
of the three are still inside the device and one is not.

**Five reps rather than three.** Each reading is a third of a cold SSD one, and
what the table is read for is a ratio near 1 where a few percent decides three
levers. Nothing compares this table's absolutes against another's, so the
differing rep count costs nothing.

## What 7.8.1 inherits, stated as arithmetic

**No scheme that overlaps I/O with parsing can put a scan below the time the
device takes to deliver the bytes.** So the ceiling on readahead,
`posix_fadvise` and a re-chosen chunk size, *together*, is the amount by which
a cold scan exceeds its own floor — **0.125 s of 1.406 s, 8.9%** — and less
than that in practice, since no scheme overlaps perfectly. The kernel's own
readahead has already taken the rest, which is the same thing the cold-SSD
table said at 1% and the spec suspected at "the kernel's own readahead is
evidently already doing it".

Nothing about a slower device improves that: the same subtraction is ~1% on the
SATA SSD and the HDD scan is device-bound by a factor of several. Nothing about
a *faster* device is available to us to measure.

**The spec's estimate was close on the total and wrong on both legs.** It put
the NVMe at ~0.9 s of device against 0.55 s of parse CPU per 3 GiB, serial
1.45 s. The device is 1.281 s, not 0.9 — this is a 970 EVO Plus read through a
container mount, not the drive's spec sheet — and yet the scan is 1.406 s,
which is nowhere near what device-then-parse in series would cost at any parse
CPU the doc has ever published for this shape. **Most of the CPU is already
hidden behind the read**, and that is a direction rather than a subtraction:
putting a number on it means differencing this table against a warm leg from
another sitting on older code, which this doc's own rules forbid. The
estimate's conclusion survives both its legs being wrong, which is the reason
the phase measures before it lands.

**What this does not settle.** The chunk-size constant is not priced by this
table: `ScanOptions::chunk_size` is not reachable from the CLI, so a sweep over
chunk sizes needs either a flag or a binary per size. That is 7.8.1's to decide
and it is why the two halves are two review cycles — deciding it is a library
change, and this slice deliberately makes none.

## The `INSERT` row is `KD9`'s answer, and it is the opposite of a retirement

[`measurements.md`](measurements.md) had written this figure into `KD9` as the
reading that would *promote the entry to owned work or retire it to a
property*. It promotes it: **2.66× the device's own time against a `COPY`
scan's 1.10×**, so ~2.1 s of a 3.40 s `INSERT` scan is CPU the disk does not
hide, on the top of the hardware range [`roadmap.md`](roadmap.md)'s
device-bound goal names. Which of the three published multiples a user meets is
decided by their storage and not by their dump.

The entry is rewritten to that and stays **(c) unowned**: what it was short of
was evidence, and what it is short of now is an owner. Allocating a slice for
the two named cuts is a change to what P7 committed to, so it is flagged under
`STATUS.md`'s "Decisions worth another look" rather than taken.

*Rejected: taking a warm `INSERT` reading in this sitting to split the 2.66×
into device and CPU.* It would make the sitting two figures rather than one and
put a second warm absolute in the doc for a reading the warm table already
publishes. The 2.66× is the whole claim — that the disk is idle for most of the
scan — and it needs no second leg.
