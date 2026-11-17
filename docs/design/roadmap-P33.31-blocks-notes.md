# P33.31 notes — the block tracker

**The instrument, then its readings from its commit, which refute the account
they were taken against.** Nothing the `introspect` build reported named a
buffer (33.7's notes, "The hand runs"), so 33.31 built the reading before
taking it, a probe being taken from a commit, never from a tree carrying its
own apparatus (`measurements.md`, "A figure may be published outside the
sweep"). No production code: every hook compiles to nothing without
`introspect`.

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

## The readings

**Taken from `05ac83ae`**, this slice's commit, on the instrument with line
tables (`release.py build --features introspect --profile profiling`), over
33.7's query run (`runs/p33.7-query-hand-run-20261117-0320/run.sh`, its `q 4g`
line and `SET pgdump.memory = 2147483648`), armed at 1 MiB, twice at
mimalloc's shipped purge delay and twice at a zero one:
`runs/p33.31-blocks-20261117/` (`run.sh`, `build.log`, each run's report,
`.err` and `.csv`, the plan in `explain.txt`, and `readings.txt`, which
`table.py` reads off the reports by site). Every run exited 0 with 33.7's rows
and peaked where 33.7's did — 2,430,020 and 2,450,880 KiB at the shipped
delay, 1,947,396 and 1,930,144 KiB at zero — so the tracker moved neither.

**The account tested**: what the run holds past its `--memory` is the escaped
`msg` value's decoded copy, `unescape_owned`'s `String`, a block of its own in
the batch, freed with it and kept resident by mimalloc beside the next line's;
refuted if, at the shipped delay, the largest freed-resident sum any event
reads is under 480 MiB, the least a zero delay took off 33.7's peak; if more
than half of it is named by another site; or if at a zero delay any event
reads more than a few MiB freed and resident.

**The account is refuted**, on the second and third counts:

- At the shipped delay the largest freed-resident sum is 886 and 889 MiB,
  past 480 MiB; but the decoded copy (`unescape_owned`) names 292 and 328 MiB
  of it, a third. The view builder's own blocks — values under
  `OWN_BLOCK_PAST` that `push_utf8view_field` copies by `append_value` — name
  313 and 310, and the compressed source's decoded-block buffers
  (`BufferPool::pick`) 197 and 180.
- At a zero delay an event reads up to 90 and 91 MiB freed and resident, the
  cache load's freed tables among it seconds after their free: ranges a
  smaller block took, or pages left in a segment still in use. A freed sum is
  an upper bound by that much; what survives is a site's shipped sum less its
  zero-delay one.

**At the resident peak the decoded copy holds almost none of the excess.** The
peak is each run's last event, the longest line's draw returned: 2,374 and
2,343 MiB resident at the shipped delay, 1,903 and 1,886 at zero, about
464 MiB apart. Shipped less zero, by site: the view builder's blocks 186 MiB,
the decoded-block buffers 157, DataFusion's coalescer's buffers 20, xz's
decoder 10, the decoded copy 7, the held line's growth 7, 386 MiB freed. Live
blocks read 53 MiB more resident, capacity not yet written that the shipped
delay hands out on retained pages; the last 24 MiB is under the tracked size
or outside Rust's heap, unattributed. The freed ones are the
bytes of rows streaming through, freed batch by batch and slot by slot at the
run's rate and kept to the purge, which no draw charges.

**A decoded copy outlives its draw.** At the peak 860 MiB of the live heap is
two decoded copies of 440 MiB lines, at either delay, while the ledger holds
11.5 MiB: the copy of the line at 914,895,096,363, whose draw was returned
500 ms before, is live while the longest line is admitted and decoded. D159
returns a draw as the batch holding its row is yielded; the plan's
`RepartitionExec` (`RoundRobinBatch(24)`, `explain.txt`) then pushes that
batch into an output partition's coalescer, which keeps a dense view array's
buffers by reference until its output batch fills (arrow-select 59.2.0,
`coalesce/byte_view.rs`, `append_views_and_update_buffer_index`; DataFusion
55.1.0, `repartition/mod.rs`, `SharedCoalescer`). The holder is read from the
code: the tracker names a block's allocation, not what holds it. At a zero
delay the run holds that copy and stays under its `--memory`.

What this leaves 33.32 is under STATUS's "Decisions worth another look".

## Negative results

- **The tracked heap is not the whole peak**: at a zero delay the live heap
  and the freed sum leave about 555 MiB of the peak outside the tracker — the
  held line's own mapping, C's heap and the binary its candidates, none read.
