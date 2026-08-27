# Measurements

Every performance figure the design relies on, with the command that
reproduces it. A baseline nobody can re-run is a rumour with a decimal point,
so **a figure that loses its regeneration command should be deleted, not
kept**.

All figures are on the hardware `CLAUDE.local.md` describes. Synthetic inputs
are regenerable with `--seed 42` and are **never committed** — they measure
throughput, not correctness, which stays entirely fixture-based.

Two standing rules for reading anything below:

- **Every figure is a ratio, never a disk throughput.** Page-cache state
  dominates. A number taken warm on a freshly generated file can be twice what
  the disk delivers to `cat`, which is exactly how the large-object figure was
  once misread. Always take the `cat`-to-`/dev/null` floor for the same file on
  the same disk in the same session, and compare against that.
- **Long runs are detached.** A koji-scale scan is roughly an hour; see
  `CLAUDE.md`, "Long-running processes", for why waiting on one is expensive
  and what to do instead.
- **A koji figure taken while local work ran is not a figure.** Whether this
  checkout and the sample share a spindle is a machine fact — see
  `CLAUDE.local.md`'s hardware section, which records what the contention costs
  here.

## Scan throughput by input shape

Three 3.00 GiB synthetic dumps on the SSD, a whole-file `pgdq` scan in a
512MB-limited container, three consecutive runs each in one session so
page-cache state is comparable.

| Input | Wall | Rate | Against the floor |
|---|---|---|---|
| `COPY` block | 2.55–3.27 s | ~1.0–1.26 GB/s | at the I/O floor |
| Large-object region | 3.61–4.99 s | ~645–890 MB/s | at the I/O floor |
| `INSERT` run | 14.55–14.79 s | ~218–221 MB/s | **4× above it** |
| `cat` → `/dev/null` | 3.67–3.85 s | ~840–880 MB/s | — |

Max RSS is ~35–42 MB across all three.

**What this says.** The `COPY` and large-object paths are device-bound. The
`INSERT` path is not: it spends roughly 11 s of CPU per 3 GiB that the other
two do not, because every line is still decoded into `Event::Line` and pushed
through the statement accumulator (see
[`architecture.md`](architecture.md), "Bulk regions"). Mapping an `--inserts`
file costs about what *decoding* a `COPY` file costs, not what *scanning* one
costs — so a koji-scale 1 TB `--inserts` dump maps in ~75 minutes rather than
the ~15 the `COPY` rate implies. Correctness, tiling and row counts are
unaffected. The fix is a scanner-level `INSERT` path;
[`roadmap-phase7-inbox.md`](roadmap-phase7-inbox.md) holds it.

Regenerate the three inputs:

```sh
cd scripts
uv run generate_perf_data.py         --size-mb 3072 --seed 42   # COPY control
uv run generate_large_object_bench.py --size-mb 3072 --seed 42
uv run generate_insert_run_bench.py   --size-mb 3072 --seed 42
```

The `INSERT` generator writes one `INSERT INTO public.bench_inserts VALUES
(…);` per line under an ordinary `TABLE DATA` TOC comment — 7,656,060 rows,
with an apostrophe doubled the way `pg_dump` writes one in ~15% of them, so
the accumulator's quote tracker is genuinely exercised. The large-object
generator is `LOBBUFSIZE`-chunked to match real `pg_dump`.

Measure each the same way:

```sh
cargo build --release --target x86_64-unknown-linux-musl -p pgdump_query-cli
sudo nerdctl run --rm -m 512m --memory-swap 512m \
  -v "$PWD/target/x86_64-unknown-linux-musl/release/pgdq:/pgdq:ro" \
  -v "/path/to/bench.sql:/dump.sql:ro" \
  postgres:16-alpine \
  sh -c 'time /pgdq parse --source /dump.sql --dqcache /tmp/x.dqcache'
```

`parse` is the only command that reads the dump
([`architecture.md`](architecture.md), "CLI surface"), and the cache it must
write goes to the container's ephemeral layer — a few hundred KB against 3 GiB
read, which is why these figures are comparable to the ones taken before that
split existed.

## The array-shape census costs nothing on brace-free data

The census walks every data row of every block any mapping pass maps — a cold
query's included, since a mapped block always carries one
([`architecture.md`](architecture.md), "The array shape census"), so it is a
change to the scan hot path. Same 3.00 GiB `COPY` control as above, same
container, the pre-census binary and the census binary alternating in one
session so page-cache state is shared:

| Run | Pre-census | With census |
|---|---|---|
| 1 (cold) | 5.89 s | 5.92 s |
| 2 | 4.45 s | 4.31 s |
| 3 (warm) | 3.18 s | 3.26 s |

Max RSS ~41–47 MB either way. The pairs track each other as the cache warms
and the differences (−3% to +2.5%) fall on both sides of zero, so the census
is **free at this measurement's resolution**.

**What this figure does and does not cover.** The control holds no `{` or `[`
in any data row, so every row is rejected by the census's own pre-filter after
one pass over its bytes, and no row is ever split into fields. That is
deliberately the koji shape — koji's six array columns are entirely NULL — and
it is the case worth knowing is free, since it is what a `pgdq parse` over a
real dump mostly does. **The cost on array-bearing rows, where the pre-filter
passes and every field is inspected, is not measured here**; it belongs with
the array stress data (`roadmap-phase4-composite-decoding.md`, slice 4.6).

Reproduce by building both binaries — the census one from the working tree,
the other from the commit before it — and alternating. **The pre-census binary
predates the `parse`/`info` split**, so it takes `info --source /dump.sql
--dqcache none` where the current one takes the line below; both drive the same
whole-file scan.

```sh
cargo build --release --target x86_64-unknown-linux-musl -p pgdump_query-cli
for i in 1 2 3; do for w in baseline census; do
  /usr/bin/time -f "$w run$i %e s maxrss=%MkB" sudo nerdctl run --rm \
    -m 512m --memory-swap 512m \
    -v "$PWD/runs/pgdq-$w:/pgdq:ro" \
    -v "/path/to/copy_control.sql:/dump.sql:ro" \
    postgres:16-alpine /pgdq parse --source /dump.sql --dqcache /tmp/x.dqcache
done; done
```

**The large-object skip is the measurement that justifies it.** Completing a
3GB region inside a 512MB limit, an order of magnitude faster per byte than
the `INSERT` path walks the same kind of bytes, is the "skipped, not walked"
evidence: walking it statement by statement would mean one span *and one
stored text string* per `lowrite` call, hundreds of thousands of them.

## koji full scan — the regression check

The 784GB real sample (`CLAUDE.local.md` has the path). Roughly an hour on the
HDD; run it detached per `CLAUDE.md`.

| | |
|---|---|
| `COPY` blocks | 74 |
| Rows | 19,575,829,920 |
| Bytes accounted for | 784,019,857,152 |
| RSS | flat, ~9 MiB (untyped scan) |
| `UnterminatedCopyBlock` | none |

**The `.dqcache` a run leaves behind dies at the next cache-format bump**, and
those are free and frequent pre-1.0 (`architecture.md`, "The cache"). Treat the
koji cache as a byproduct of a scan run for another reason, never as an asset:
the cache left by the 2026-08-25 run was already unreadable by the time 4.4
landed. Nothing plans around keeping one alive — inspecting koji at all
(`pgdq info`, with or without `--source`) is available only between a scan and
the next bump, and regaining it costs the full ~54-minute scan.

**The regression check is byte-for-byte identity, not throughput**: every
block's header/data/terminator/end offset must match the previous run. That
identity over a change touching only what happens *between* blocks is what the
check is for.

`lock_monitor.activity` — whose row data contains a literal `COPY … TO
stdout;` substring — must parse as one correct block. That is the case that
motivated line-anchored detection.

**Throughput, re-measured clean.** A 2026-08-25 re-run on an uncontended disk
(container `pgdq-koji`, `runs/koji-throughput-scan.log`) reproduced the same
74 blocks/19,575,829,920 rows/784,019,857,152 bytes, and measured 54m9.97s
wall-clock — **~241 MB/s**, against the original 2026-08-22 baseline's ~243
MB/s. The two independent runs agree to within ~1%, which is inside the noise
this kind of wall-clock measurement carries; no `cat`-to-`/dev/null` floor was
taken for this file specifically (impractical at 784GB on the HDD for a
one-point confirmation), but the close agreement between two runs that were
each I/O-bound at ~33% of one core is itself the evidence that the 2026-08-24
run's ~110 MB/s figure was the outlier, caused by the concurrent restore
documented below, not a real regression. Command and container recipe are in
`CLAUDE.md`.

**Per-block cache persistence costs ~1.5%, and the throttle leaves it
untouched.** `pgdq parse` serializes the whole cache at a `CopyEnd` watermark,
so a koji scan writes it 74 times where the build before it wrote it once. The
self-tuning throttle below never fires on this shape — koji's blocks are ~45 s
apart and its saves cost well under a second, so the "20x the last save's own
cost" bar is cleared every time — which is the regime it was designed not to
change. Container
`pgdq-koji-9.1`, launched 2026-08-26T04:21:17Z, `runs/koji-9.1-scan.log`,
`exit=0`:

| | |
|---|---|
| Wall | 3300 s |
| Rate | ~238 MB/s |
| Against the no-per-block-save run above (3250 s) | +50 s, +1.5% |
| Final cache | 247,380 bytes |
| Bytes the saves wrote | under 74 x 247,380 = 18.3 MB |
| Amplification against 784 GB read | ~2.3e-5 |

The +1.5% sits inside the run-to-run spread the two baseline scans above
already show (~241 against ~243 MB/s), and the byte bound is an over-estimate
twice over: every save is charged the *final* cache size, which only the last
block's save actually pays. **This figure is why the throttle is not an
interval**: at koji scale there is nothing to save, and any constant chosen to
help the block-rich shape below would have had to be checked against this one.

This run agreed with the one above on the block list, the per-block row counts
and the byte total. It did **not** re-run the byte-for-byte offset identity
check, which needs `--verbose`; that check runs on its own.

```sh
cargo build --release --target x86_64-unknown-linux-musl -p pgdump_query-cli
mkdir -p runs
sudo nerdctl run -d --name pgdq-koji-9.1 -m 512m --memory-swap 512m \
  -v "$PWD/target/x86_64-unknown-linux-musl/release/pgdq:/pgdq:ro" \
  -v "$PWD/runs:/out" \
  -v "/path/to/koji.dump:/dump.sql:ro" \
  postgres:16-alpine \
  sh -c 'L=/out/koji-9.1-scan.log; S=$(date +%s); \
    echo "started=$(date -Iseconds)" > $L; \
    /pgdq parse --source /dump.sql --dqcache /out/koji-9.1.dqcache >> $L 2>&1; \
    echo "exit=$?" >> $L; echo "seconds=$(( $(date +%s) - S ))" >> $L; \
    ls -l /out/koji-9.1.dqcache >> $L'
```

The comparison's other half is the throughput row above: the same recipe on a
build predating the per-block save, so reproducing the *delta* means checking
out one of each.

## The preamble prepass is bounded by the schema, not by the dump

`pgdq parse` and every cold query open with `index::scan_preamble`, which reads
from byte 0 to the first `COPY` header. It is the one region that ignores
`ScanOptions::cancel` (see `architecture.md`, "`parse` resumes, and saves as it
goes"), so "bounded by its own length" is the claim that has to hold.

**koji: 63,333 bytes of 784,019,857,152** — 0.00000008 of the file. The whole
uncancellable region is one read.

```sh
LC_ALL=C grep -m1 -b -a -E '^COPY .* FROM stdin;' /path/to/koji.dump | cut -c1-80
```

**The most preamble-heavy shape available: 0.04 s.** A 4000-table dump is 49%
preamble by bytes (the first `COPY` header sits at 980,996 of 1,998,741), and
`--preamble-only` maps all of it in 40 ms — against ~20 s for the same file's
full `parse`, which is dominated by the quadratic below.

```sh
cd scripts && uv run generate_block_count_bench.py --blocks 4000 --out /tmp/r4000.sql
rm -f /tmp/r4000.sql.dqcache
/usr/bin/time -f '%e s' pgdq parse --preamble-only --source /tmp/r4000.sql
```

So the region grows with the *schema* — table count and DDL size — and not with
the data, which is what makes an immediate Ctrl-C during it a non-issue on a
local file. The remote case is not covered by these numbers: 63 KB is still one
ranged GET that can hang, and that is a Phase 6 decision
(`roadmap-phase6-inbox.md`).

## Per-block cache saving is quadratic in block count, and so is the map

The regime koji cannot show: **block-rich and byte-poor** — a schema with
thousands of tables, or one partitioned table with a daily leaf over a decade.
Four columns, three rows per table, the schema section then the data section,
which is how `pg_dump` orders a plain dump:

```sh
cd scripts
for n in 500 1000 2000 4000; do
  uv run generate_block_count_bench.py --blocks $n --out /tmp/r$n.sql
done
```

`parse` on each, cache removed first, one session, warm page cache. "Before" is
the build with a save at every `CopyEnd`; "after" is the same binary with
`SaveThrottle` (`K = 20`):

| blocks | dump | final cache | before | after | saves before → after |
|---|---|---|---|---|---|
| 500 | 248 KB | 322 KB | 0.63 s | 0.32 s | 503 → 21 |
| 1000 | 496 KB | 647 KB | 2.50 s | 1.30 s | 1003 → 42 |
| 2000 | 997 KB | 1.3 MB | 10.44 s | 5.66 s | 2003 → 97 |
| 4000 | 2.0 MB | 2.6 MB | 44.31 s | 23.56 s | 4003 → 195 |

Every run is 99% CPU at every point: the cost is *serializing* the index, not
writing it. The control is the same byte count in **one** `COPY` block —
`uv run generate_perf_data.py --size-mb 2 --seed 42 /tmp/one_block.sql`, which
`parse` finishes in under 10 ms — so at 4000 blocks the overhead is three
orders of magnitude above the scan it protects.

**The throttle does exactly what it was designed to do, and the series still
quadruples per doubling.** Saves fall to `n/20` — the `1/K` bound, visible in
the last column — and the ~21 s of saving at 4000 blocks becomes ~1.2 s of a
23.5 s scan. What is left is a *second* quadratic with the same shape and a
different cause: every `CopyEnd` clones the whole span list
(`map::Builder::snapshot`, then `stream::splice` over the prefix), so the map
is O(blocks²) with the cache **disabled entirely**:

```sh
# maps to EOF (the table never matches) and never saves
./target/release/pgdq query --source /tmp/r$n.sql --table public.nosuchtable --dqcache none
```

| blocks | 1000 | 2000 | 4000 |
|---|---|---|---|
| map only, no saving | 1.02 s | 4.58 s | 19.67 s |

So the cache was roughly half the cost at 4000 blocks and the map is the other
half. Closing the second half means not rebuilding the span list per block;
it is filed in [`roadmap-phase7-inbox.md`](roadmap-phase7-inbox.md), because it
is a change to how the map is assembled rather than to when it is written.

Save counts come from `strace -f -e trace=openat` filtered to the cache path
(`std::fs::write` opens once per save; the first is the load's miss).
Reproducing the "before" column means building the commit that precedes the
throttle — `git worktree add <dir> <commit>` and a release build there, the
same two-binary method the census figure above uses.

## Decoder and whole-file benchmarks

`criterion`, `harness = false`. A regression tripwire for per-byte CPU cost,
not an optimization campaign.

```sh
cargo bench -p pgdump_query
```

- `benches/decoders.rs` — one `decode`/`render` pair per mapped type family.
- `benches/whole_file.rs` — one warm-cache end-to-end `SchemaMode::Typed`
  scan. It warms the page cache before measuring, deliberately: the figure is
  meant to move when decode cost moves, not when the disk is busy.

Its input comes from `generate_perf_data.py` (default `--size-mb 256`, sized
to fit page cache): one wide table, one column per family the decoders cover,
plus `v_long_text` (very long values) and `v_escaped` (high escape density).
Its escaping is a Python reimplementation of `copy::encode_field` (I15), since
the script has no Rust runtime to call into.
