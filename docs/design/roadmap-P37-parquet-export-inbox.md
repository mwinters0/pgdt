# P37 inbox — facts filed for its grilling

Evidence P37 (Parquet export as a Parquet-native writer leaves it) will need.
**This is a queue, not a document**: when P37 is grilled, walk every entry,
fold it into the spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

---

## The order oracle is the dump, not the koji replica

**Fact.** `pg_dump` reads with synchronized scans off, so a `COPY` block holds
its source table's heap order, and a restore orders nothing beyond it. The
`koji-pg` replica's heap departs from that order: `public.build` holds the
dump's rows as 11,380 runs of its order under one `xmin`
(`/mnt/ssd/fedora/scratch/pgdump_query/koji-parquet-diag/`). So an order check
reads the dump, not the replica. Against the dump, the five smoke tables keep
its order in every slice, and nothing the parse records loses or wrongly
claims an order (`runs/koji-parquet/dump-oracle-smoke.txt`), and no text
column loses its order, bytewise or under `en_US`
(`runs/koji-parquet/text-orders.txt`). `archive_rpm_components.archive_id` is
unsorted in the dump; `build.promotion_time` is ascending and holds NULLs,
which is `KD146`.

**Why P37 cares.** The sketch's "the orders the parse misses" are found by an
order check; one run against the replica reports orders the dump never had.

**Origin.** The koji Parquet experiment, 2026-10-26. Contingent on the replica
being the restore it is now.

---

## A partial export of koji, and one-partition `COPY` throughput

**Fact.** An export through `pgdt sql` v0.2.0's `COPY` was stopped at the
maintainer's word partway through `buildroot_listing`: 18 tables are complete
under `/mnt/wd12t/fedora/koji/parquet`, beside a `_state` directory to resume
from, with the run's pipeline scripts and logs in `runs/koji-parquet/`. Three
one-partition `COPY` processes read `buildroot_listing` at about the HDD's
rate, each at about one core; whether one alone does is unread. The default
million-row planning sample of `public.archiveinfo` was OOM-killed in a
1.5 GiB container and is unattributed; the retry ran with a 50,000-row one.

**Why P37 cares.** "The writer" is decided by whether `COPY` from one
partition is device-bound on koji; this is the only reading of it, and it ran
three at once.

**Origin.** The koji Parquet experiment, 2026-10-26. Contingent on v0.2.0's
scan; a later release re-reads it.
