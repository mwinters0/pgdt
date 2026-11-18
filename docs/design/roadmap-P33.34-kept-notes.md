# P33.34 notes — what the 100 ms purge delay keeps, across reader counts

**What 100 ms keeps at the peak of 33.7's query run is the process's under
the rule fixed before the run, but it is not one constant.** Over the zero
delay's median it keeps the least at one reader and the most at five, and less
again from six readers to eight. It does not grow with the readers, and a
term per reader would mis-bill it. The other finding came from a column the
slice was not reading: **at a zero delay the run passes its allowance at one,
two, six and seven readers**. That excess is the round-robin repartition's
(`../status/upstream.md`, "UF5"), not the delay's. No production code.

## The readings

Taken by hand from `373e1320`'s release-image build, read in place. No code
changed between it and `b5dec340` (`runs/p33.34-delay-readers-20261118/`:
`run.sh`, `probe.sh`, `readings.txt`, `reduce.py`, each rep's `.csv`, `.err`,
`.exit` and `.wall`). The run is 33.7's, `pgdt sql` over the block holding the
longest line in a 4g container. It ran at the binary's default and under
`MIMALLOC_PURGE_DELAY=0`, at nine `SET pgdump.memory` allowances. Eight of
them are the smallest whole MiB whose `EXPLAIN` carves one to eight readers;
the ninth is 33.7's 2 GiB, which carves eight. One MiB under the first is
refused for the longest line (`LineOutgrowsMemory`). There were two sittings,
each its own invocation, of three reps interleaved over every pair, the middle
rep reversed: six reps a leg, 108 runs. The allowances, the count and the
decision rule were fixed in `run.sh` before it ran. Every run exited 0 with
no refusal and with the unlimited run's rows.

- **Kept**, the median 100 ms rep less the median zero one: about 77 MiB at
  one reader, rising to about 273 at five, then about 100 to 145 from six to
  eight (`readings.txt`). The rule's endpoint test reads it as the process's:
  eight less one is within the larger of their legs' spreads. Its test for
  growth fails twice, on those endpoints and on six falling below five by more
  than both their spreads. But five less one is past every spread there, so
  the interior is not flat.
- **The allowance, at one count, does not move it**: 2 GiB and eight's edge
  differ within both spreads.
- **Its shape is the cut's.** What the delay keeps at the peak is what was
  freed in the delay before it. That depends on which lines the pieces reach
  together, and the reader count moves that unevenly. Where the zero delay's
  own peak is the repartition's (below), the two peaks being subtracted fall
  at different moments. Kept at one, two, six and seven readers is confounded
  by that and is unattributed beyond it.
- **At 100 ms every allowance's worst rep passes it**, at four readers by
  5 MiB and at one by about a gigabyte.

## The zero delay passes the allowance where the repartition holds copies

At a zero delay the worst rep passes its allowance at one reader by about a
gigabyte, at six by about 215 MiB, and at two and seven by less. `probe.sh`
re-ran those four allowances at zero with
`datafusion.execution.target_partitions` set to the carved count, which leaves
no `RepartitionExec` above the scan (`plan-<MiB>.txt`). That fixes the cut
and removes only the repartition. Every rep then held within its allowance,
by about 260 to 545 MiB (`readings.txt`). That is UF5's account: a coalescer
holding a decoded copy past the ledger for each output partition, at 24 output
partitions over fewer from the scan. It shows that "the margin absorbs it",
true at eight readers, is false where the cut leaves several long lines'
copies held at once. UF5's workaround says so now.

## What 33.35 inherits

- **The shape is the process's**, as the rule reads it. Its size on this run
  is at most the five-reader median, about 273 MiB, and a single 100 ms rep
  keeps more than its median.
- **Billing the delay alone does not decide 33.7's clause.** At 2 GiB a bill
  carves fewer than eight readers. A bill of about 150 MiB, eight's kept, lands
  on six. At six, the zero delay already passes its allowance by UF5's copies,
  and every 100 ms rep peaks past 2 GiB. A bill of about 273 MiB lands on five,
  where every 100 ms rep here peaks under 2 GiB, though past its own edge's
  allowance. That is an extrapolation: no cut under 2 GiB less a bill was
  read. So which count the bill lands on decides 33.7, through UF5 rather than
  through the delay (STATUS, "Decisions worth another look").
- **A cleaner reading of kept** would subtract the two delays at
  `target_partitions` equal to the carved count, the repartition gone from both
  sides. It was not fixed before this run and was not taken.

## Negative results

- **Fewer readers are not cheaper in resident on this run**: at a zero delay
  one reader peaks highest of all, at about 2.4 GiB under a 1.35 GiB allowance,
  and three to five lowest. The repartition is what does it.
- **Wall clock is no figure**: going by `.wall`, a rep takes about 10 s at one
  reader and 5 s at eight, the 100 ms reps a fraction of a second faster at
  every count, as `purge-delay` prices.
