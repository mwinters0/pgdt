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

## Scan throughput by input shape

Three 3.00 GiB synthetic dumps on the SSD, `pgdq info --dqcache none` in a
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
  sh -c 'time /pgdq info --source /dump.sql --dqcache none --verbose'
```

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
the other from the commit before it — and alternating:

```sh
cargo build --release --target x86_64-unknown-linux-musl -p pgdump_query-cli
for i in 1 2 3; do for w in baseline census; do
  /usr/bin/time -f "$w run$i %e s maxrss=%MkB" sudo nerdctl run --rm \
    -m 512m --memory-swap 512m \
    -v "$PWD/runs/pgdq-$w:/pgdq:ro" \
    -v "/path/to/copy_control.sql:/dump.sql:ro" \
    postgres:16-alpine /pgdq info --source /dump.sql --dqcache none
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
landed. Nothing plans around keeping one alive — cache-only inspection of koji
(`pgdq info --dqcache`) is available only between a scan and the next bump, and
regaining it costs the full ~54-minute scan.

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
