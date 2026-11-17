# P33.31 notes — the block tracker

**The instrument landed; its readings did not.** Nothing the `introspect`
build reported named a buffer (33.7's notes, "The hand runs"), so 33.31 builds
the reading before taking it, and a figure or probe is taken from a commit,
never from a tree carrying its own apparatus (`measurements.md`, "A figure may
be published outside the sweep"). No production code: every hook compiles to
nothing without `introspect`.

- **The block tracker**, `crates/pgdt/src/introspect/blocks.rs`: armed by
  `PGDT_INTROSPECT_BLOCKS_FROM=<bytes>`, the counting allocator captures a
  stack for each Rust heap block of at least that size, and at each moment a
  held line's draw is first admitted or returned reads, by `mincore`, how many
  of each live block's and each freed range's pages are resident. The report
  carries `blocks_*` readings, then every event and every site as prose. Its
  choices and what it is blind to are its module's rustdoc;
  `measurements.md`, "What an instrument can see", lists it.
- **The moments** are pgdt-io's ledger's (`Holder::draw`, `Holder::release`),
  told through `pgdt_core::instrument`'s `lines.rs` to the one observer a
  binary registers.
- **`KD109` rewritten** to what is still true: the introspect build now names
  the shipped heap's large blocks by stack, and no tool records the whole of
  it.

## The readings, for the next round

The 33.7 query hand run's command (`runs/p33.7-query-hand-run-20261117-0320/run.sh`,
its `q 4g` line and `SET pgdump.memory = 2147483648`), on the instrument built
from this slice's commit with line tables, so an inlined allocation names its
function:

```sh
(cd scripts && uv run release.py build --features introspect --profile profiling \
  --target-dir /mnt/ssd/fedora/scratch/pgdump_query/alloc-builds/image/introspect-profiling)
```

run under `contained.py -e PGDT_INTROSPECT_OUT=<file> -e
PGDT_INTROSPECT_BLOCKS_FROM=1048576`, once at mimalloc's shipped purge delay
and once with `-e MIMALLOC_PURGE_DELAY=0`, each answering 33.7's rows.

**The account it tests** (the history entry of 2026-11-17): what the stated
run holds past its `--memory` is the escaped `msg` value's decoded copy —
`unescape_owned`'s `String`, a block of its own in the batch
(`push_utf8view_field`) — freed with its batch and kept resident by mimalloc
beside the next line's. **It is refuted** if, at the shipped delay, the
largest freed-resident sum any event reads is under 480 MiB, the least a zero
delay took off the peak (33.7's notes); or if more than half of it is named by
a site other than the decoded copy's; or if at a zero delay any event reads
more than a few MiB freed and resident, which would be ranges a smaller block
took, counted as retention.
