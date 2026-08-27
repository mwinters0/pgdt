# Measurements

Every performance figure the design relies on, with the command that
reproduces it. A baseline nobody can re-run is a rumour with a decimal point,
so **a figure that loses its regeneration command should be deleted, not
kept**.

All figures are on the hardware `CLAUDE.local.md` describes. Synthetic inputs
are regenerable with `--seed 42` and are **never committed** — they measure
throughput, not correctness, which stays entirely fixture-based.

Nine standing rules for reading anything below:

- **Every figure is a ratio, never a disk throughput.** Page-cache state
  dominates. A number taken warm on a freshly generated file can be twice what
  the disk delivers to `cat`, which is exactly how the large-object figure was
  once misread. Always take the `cat`-to-`/dev/null` floor for the same file on
  the same disk in the same session, and compare against that.
- **Say which regime, and stay in it for the whole figure.** "In one session"
  is not enough: a run sequence that starts cold and warms up puts each run in
  a different regime, and a difference smaller than the warming trend
  disappears into it. Either `drop_caches` before *every* run or `cat` the file
  first and take every run warm — and when the figure is a difference between
  two binaries, run the pair in both orders, because within a pair the second
  run is the warmer one. `M10` re-took the census figures for exactly this
  reason ([`../status/history/2026-08-27.md`](../status/history/2026-08-27.md)).
- **A figure about parsing CPU must not be taken against a filesystem.** Put
  the input on tmpfs, or otherwise guarantee it is served from memory for every
  run. Device time and background I/O swamp the difference being measured —
  badly on the HDD, where minor unrelated activity moves the reading further
  than any code change under review will — and page-cache residency is an
  assumption, not a guarantee: a multi-gigabyte input can be partly evicted
  between runs by anything else the machine does. This does not apply to a
  figure whose *subject* is the device ("Scan throughput by input shape"
  below), which is taken cold on purpose. **Every warm figure below is on
  tmpfs**, and they were re-taken together in one session rather than drifting
  onto the new footing one at a time — see "The warm set" below for the
  apparatus they share.
- **Re-take a comparison table whole, in one interleaved sweep.** Never
  difference one row against a figure from another session, and never run a
  multi-file comparison a file at a time. Session-to-session level shifts of
  ~10% happen here on identical binaries and identical inputs, and a
  file-at-a-time sweep maps a session's own drift onto file identity,
  manufacturing a between-file difference that is apparatus. Each rep runs
  every file-and-mode combination in turn; report medians. `M10`'s nested table
  and `4.6.1`'s disagreed by 3.45× against 3.08× for exactly this reason, while
  the per-row differences the design actually consumes barely moved
  ([`../status/history/2026-08-27.md`](../status/history/2026-08-27.md)).
- **The timer goes inside the container, never around it.** A figure must
  not carry the harness that produced it. `sudo nerdctl run` costs **0.74–0.76
  s** before the binary starts — three runs of a trivial command, opening the
  warm-set sweep below — against which a 3.00 GiB warm `parse` of the
  brace-free control is **0.57 s** timed by the container's own shell. The
  wrapper is *larger than the figure*, and more than twice the smallest row of
  the quadratic table. So the timed command is `bash -c 'time /pgdq …'`, whose
  timer resolves to 1 ms. This does not license running a figure outside the
  container to avoid the cost — the cgroup limit is part of the apparatus, and
  a difference of binaries is not measurable across two different ones.
- **A performance figure is taken with the default `glibc` build, in a glibc
  image.** The allocator is part of what is being measured, the two libcs do
  not agree, and the gap is far larger than the first reading of it suggested.
  On the block-serialization workload it is ~25%: the same
  500-block `parse` runs **0.29 s** glibc against **0.41 s** static musl. On
  the two workloads that move real bytes it is a factor: the same 3.00 GiB
  warm `parse` is **0.57 s** glibc against **1.35 s** musl, and the same
  `--schema-mode strings` query over it **4.43 s** against **7.98 s**. So
  the figures here are `cargo build --release` (no `--target`) run under
  `postgres:16` (Debian bookworm), whose **glibc 2.36 malloc is part of the
  apparatus** and should be named when a figure moves. The static musl build
  stays what `CLAUDE.md`'s container recipes use for *portability*, and a
  figure taken with it is not comparable to one here. Debian's `/bin/sh` is
  dash, with no `time`, so the in-container timer is `bash -c 'time …'`.
  Whether a different allocator should be the shipped default is a P7
  question, filed in
  [`roadmap-P7-scan-performance-inbox.md`](roadmap-P7-scan-performance-inbox.md).
- **Never quote a standard error or a *t* from one sweep — give the median and
  the observed spread.** Within-sweep dispersion measures the *reps*, not the
  measurement: the allocator, the stage's position in the session and the
  tmpfs/page state are all held constant inside a sweep, so they contribute
  nothing to an SE and nearly everything to the answer. The demonstration is
  the composite column's per-row share, five interleaved reps on each of two
  builds of the same source: **+0.61 µs/row (t = +4.34) on glibc and
  −1.16 (t = −4.81) on musl** — both "significant", 1.77 µs/row apart, opposite
  signs. A figure the design **depends on** — a ratio the roadmap cites, a
  bound an inbox entry consumes — is therefore taken on **two apparatuses** and
  quoted as a range across them; the second is cheap here, being the same sweep
  script against a musl build. A tripwire or an orientation figure may have one,
  and says so. Reasoning:
  [`../status/history/2026-08-27.md`](../status/history/2026-08-27.md).
- **Long runs are detached.** A koji-scale scan is roughly an hour; see
  `CLAUDE.md`, "Long-running processes", for why waiting on one is expensive
  and what to do instead.
- **A koji figure taken while local work ran is not a figure.** Whether this
  checkout and the sample share a spindle is a machine fact — see
  `CLAUDE.local.md`'s hardware section, which records what the contention costs
  here.

## The warm set

Four figures below share one apparatus and were taken in one sweep, on
2026-08-27: both census figures, the nested end-to-end table (with its
cross-file floor), and the per-block quadratic table. `S3`'s census-on column
is also the `COPY` path's warm CPU that the scan-throughput table cites, so
that fifth figure is the same measurement rather than another one.

**The apparatus is one line: 3.00 GiB inputs on `/dev/shm`, read by a
`glibc` binary in a 512 MB `postgres:16` container, timed by that container's
own `bash`.** They were re-taken together because a figure re-taken alone
would leave the doc's warm figures disagreeing about regime, which is the
failure the standing rules were written from — and every one of them moved,
some by a factor, when the tmpfs, in-container-timer and glibc rules landed
together.

The sweep that produced them was `runs/m13-warm-set.sh` → `.log`, a `runs/`
artifact and therefore gitignored: **each section's recipe below is the
durable record**, and the log was read once. It ran on the tree carrying `M12`
(the generated bytes are final) and `M11` (the pre-filter is `memchr2`), so
every figure here is post-both.

**Out of scope, deliberately.** `benches/decoders.rs`'s micros read literals
written in the bench and never touch a filesystem. The scan-throughput table's
own rows are cold on purpose — the device is their subject. koji is 784 GB on
the HDD and cannot be staged in memory at all; it is a device figure and a
regression check, not a parsing-CPU one.

**Generate the inputs on tmpfs from the host, before starting any container.**
The union of inputs is four 3.00 GiB files — `--seed 42` control, a `--seed 43`
control, `--composite`, `--arrays --composite` — which is 12 GiB against
`/dev/shm`'s 16 G and this machine's ~21 GB available. Writing them from inside
the 512 MB-limited container would charge those tmpfs pages to its cgroup and
kill it; the pages must already be resident and charged to the host when the
container opens the file read-only.

## Scan throughput by input shape

Three 3.00 GiB synthetic dumps on the SSD, a whole-file `pgdq` scan in a
512MB-limited container. **Every run is cold** — `drop_caches` before each one,
including before each `cat` — because that is the only regime in which the
floor row means anything: this file fits page cache twice over, so a second
read of it measures RAM.

| Input | Wall | Rate | Against the floor |
|---|---|---|---|
| `COPY` block | 6.68–6.70 s | ~481 MB/s | 1.2× the floor's time |
| Large-object region | 6.61–6.65 s | ~484 MB/s | 1.2× the floor's time |
| `INSERT` run | 15.13–15.42 s | ~209 MB/s | **2.7× the floor's time** |
| `cat` → `/dev/null` | 5.73–5.74 s | ~562 MB/s | — |

Every run completes inside the 512 MB cgroup, which is the memory claim this
apparatus can actually make. **It carries no max-RSS figure**: `/usr/bin/time
-f %M` around `nerdctl run` reports the *nerdctl client's* peak, not pgdq's —
it read the same ~40–45 MB for a 2 MB input as for a 3.00 GiB one, four times
what koji's row below records for a 784 GB scan. pgdq's own resident set is the
koji figure, ~9 MiB.

**What this says.** The `COPY` and large-object paths are device-bound: they
spend about a fifth more wall-clock than reading the same bytes and doing
nothing, and their CPU (0.57 s for the same 3.00 GiB with the bytes in memory,
below) is an order of magnitude under the 5.73 s the read takes. The `INSERT`
path is not — it takes 8.6 s per 3 GiB *longer* than the
`COPY` path on the same device, because every line is still decoded into
`Event::Line` and pushed through the statement accumulator (see
[`architecture.md`](architecture.md), "Bulk regions"). Mapping an `--inserts`
file costs about what *decoding* a `COPY` file costs, not what *scanning* one
costs: at 1 TB that is ~45 minutes of CPU no `COPY` dump pays, against the
~15 minutes the `COPY` path spends on 1 TB in total. **The ~5× is not a CPU
ratio and never was.** It divides this *cold* `INSERT` rate — which contains
the device — by the `COPY` path's *warm* CPU, so it compares two regimes; and
its `COPY` side has since fallen from 2.92 s (~1.10 GB/s, of which 0.77 s was
wrapper, on a page-cache-warm SSD read with the pre-`M11` scalar pre-filter) to
**0.57 s**. **No warm `INSERT` figure has ever been taken**, so the per-byte
CPU ratio is unmeasured. Bounding it from this table alone: a 15.2 s cold
`INSERT` scan against a 5.73 s device floor puts its CPU between ~9.5 s (fully
serialized with the read) and ~15.2 s (fully overlapped), against 0.57 s for
`COPY` — i.e. somewhere around **16–26×**, with ~5× a floor rather than an
estimate. The measurement that settles it is a warm `INSERT` `parse` on tmpfs,
queued as `M17` (`../status/STATUS.md`, "The out-of-band queue"). The direction is safe meanwhile: every
correction makes the `INSERT` path look worse, never better. Correctness, tiling and
row counts are unaffected. The fix is a scanner-level `INSERT` path;
[`roadmap-P7-scan-performance-inbox.md`](roadmap-P7-scan-performance-inbox.md) holds it.

**The CPU ceiling under the `COPY` row is 0.57 s** — the same scan with the
same bytes served from tmpfs, ~5.7 GB/s, of which 0.33 s is the kernel's read
(the `dd` floor for the same file in the same container). That is the number
the two census figures below are differences against, and the reason they are
stated warm: at 481 MB/s the device hides everything the CPU does.

**This whole table is superseded by `M14`** (`docs/status/STATUS.md`, "The
out-of-band queue"), and by more than a rounding: every `pgdq` row was timed
by `/usr/bin/time` around `nerdctl run` and so carries the 0.77 s the seventh
standing rule now excludes, while the floor row is a **host** `cat` with no
container at all — two apparatuses in one comparison, which no subtraction
fixes. Corrected by that constant the rows read 5.91 / 5.84 / 14.36 s, and
"1.2× the floor's time" becomes **1.03×** for `COPY` and 1.02× for large
objects, with the `INSERT` row at 2.5×. The device-bound conclusion is
unchanged and in fact sharper — the `COPY` path spends 3% more wall-clock than
reading the bytes and doing nothing, not a fifth more — but `M14` re-takes the
rows and the floor under one apparatus rather than leaving the doc quoting
arithmetic.

Regenerate the three inputs:

```sh
cd scripts
uv run generate_perf_data.py         --size-mb 3072 --seed 42   # COPY control
uv run generate_large_object_bench.py --size-mb 3072 --seed 42
uv run generate_insert_run_bench.py   --size-mb 3072 --seed 42
```

The `COPY` control is 817,024 rows of 16 columns, 3,943 bytes each, and holds
no `{` or `[` in any data row — see "The census on brace-free rows" below for
why that is a contract rather than an accident. The `INSERT` generator writes
one `INSERT INTO public.bench_inserts VALUES (…);` per line under an ordinary
`TABLE DATA` TOC comment — 7,656,060 rows, with an apostrophe doubled the way
`pg_dump` writes one in ~15% of them, so the accumulator's quote tracker is
genuinely exercised. The large-object generator is `LOBBUFSIZE`-chunked to
match real `pg_dump`.

Measure each the same way:

```sh
cargo build --release --target x86_64-unknown-linux-musl -p pgdump_query-cli
for i in 1 2 3; do
  sudo sh -c 'sync; echo 3 > /proc/sys/vm/drop_caches'
  echo "run$i"; sudo nerdctl run --rm \
    -m 512m --memory-swap 512m \
    -v "$PWD/target/x86_64-unknown-linux-musl/release/pgdq:/pgdq:ro" \
    -v "/path/to/bench.sql:/dump.sql:ro" \
    postgres:16-alpine \
    sh -c 'time /pgdq parse --source /dump.sql --dqcache /tmp/x.dqcache >/dev/null'
done
# and the floor, in the same container so the comparison is one apparatus:
sudo sh -c 'sync; echo 3 > /proc/sys/vm/drop_caches'
sudo nerdctl run --rm -m 512m --memory-swap 512m \
  -v "/path/to/bench.sql:/dump.sql:ro" postgres:16-alpine \
  sh -c 'time dd if=/dump.sql of=/dev/null bs=4M'
```

`parse` is the only command that reads the dump
([`architecture.md`](architecture.md), "CLI surface"), and the cache it must
write goes to the container's ephemeral layer — a few hundred KB against 3 GiB
read, which is why these figures are comparable to the ones taken before that
split existed.

## The census on brace-free rows costs 7% of a warm scan

The census walks every data row of every block any mapping pass maps — a cold
query's included, since a mapped block always carries one
([`architecture.md`](architecture.md), "The array shape census"), so it is a
change to the scan hot path. The 3.00 GiB `COPY` control the generator writes
by default — 814,362 rows of 16 columns, 3,956 bytes each on `--seed 42`: no
`{` or `[` in any data row, so every row is rejected by the census's own pre-filter
after one pass over its bytes and no row is ever split into fields. That is
deliberately the koji shape — koji's six array columns are entirely NULL — and
it is the case worth knowing the price of, since it is what a `pgdq parse`
over a real dump mostly does.

Census on is the working tree; census off is the same tree with one line
added, so nothing but the census differs between the two binaries (below).

Six reps each, the pair run in both orders; medians, with the full spread
beside them.

| | Census off | Census on | Δ |
|---|---|---|---|
| warm, on tmpfs | **0.531 s** (0.515–0.562) | **0.568 s** (0.560–0.593) | **+0.037 s, +7%** |

Both binaries complete inside the 512 MB cgroup; no max-RSS figure is quoted,
for the reason under the scan-throughput table. `dd` → `/dev/null` on the same
file in the same container: **0.326 s**, so the census-off scan is already
within 1.6× of what the kernel charges to hand over the bytes.

**What this says.** The pre-filter is very nearly free on the shape a real
dump mostly has: **0.037 s per 3.00 GiB of brace-free rows**, 45 ns per
16-column row of 3,956 bytes. That implies ~87 GB/s, which is well above what
this machine's DRAM will give one core — so the reading is not a
memory-bandwidth figure at all: the pre-filter re-walks bytes the scanner has
just walked, out of cache, and `memchr2` is fast enough that what is left is
the loop, not the bytes.

**It was not always.** The scalar `raw.iter().any(…)` loop `M11` replaced ran
at ~3.8 GB/s and cost 1.03 µs per row — +39% as recorded and **+63%
reconstructed**, once the 0.77 s container wrapper that sat in both legs is
taken out ([`../status/history/2026-08-27.md`](../status/history/2026-08-27.md)).
That figure is what argued for the swap, and it is why the deferred question
of a *skippable* census is now closed rather than open: 45 ns per row is not a
cost worth a knob.

So the census's cost is effectively **one tier, not two**: it is paid by the
rows that pass the pre-filter (next section), and the pre-filter itself is
2.5% of what those rows cost. It is unconditional either way
([`architecture.md`](architecture.md), "The array shape census") — the
alternative is a query that cannot retype its array columns without a second
pass. What the figure does *not* license is calling it exactly zero: the two
spreads do not overlap, and the earlier reading that said zero came from
taking the pair while the page cache was still filling.

**The cold row is `M14`'s.** The only cold reading of this comparison is
**+1.2%**, taken off this SSD against a 5.73 s device floor — and it is the
*pre-`M11` scalar* pre-filter, on the superseded apparatus, so it is a bound
rather than this table's other half. A pre-filter 23× cheaper is hidden a
fortiori, but the claim and its measurement should sit in one apparatus:
`M14` already stages this control cold on the SSD with `drop_caches` before
every run and a `dd` floor in the same container, so it takes the census-off
binary through two more cold runs and this row comes back.

**The control's brace-freeness is a contract, not an accident.** The same
generator writes array columns behind `--arrays` and a composite behind
`--composite`, and those flags exist precisely so its *default* output stays
what this figure and the scan-throughput table above were taken on. Anything
that puts a `{` or `[` into the default rows invalidates both.

Both binaries, then the alternating runs. Census off is `pub(crate) fn
on_row`'s body in `map.rs` preceded by a bare `return;` — the pre-filter and
everything after it, and nothing else. **Run the pair in both orders**: within
a pair the second run is warmer, which is exactly the artifact that hid this
figure before.

```sh
# generate from the HOST: 3 GiB written from inside the 512 MB container is
# charged to its cgroup and kills it.
cd scripts && uv run generate_perf_data.py --size-mb 3072 --seed 42 \
  /dev/shm/pgdq/control.sql
cargo build --release -p pgdump_query-cli          # default target: glibc
cp target/release/pgdq runs/pgdq-census
# add `return;` as the first statement of map::Builder::on_row, rebuild,
# copy to runs/pgdq-nocensus, then revert.
for i in 1 2 3; do for w in nocensus census; do
  echo "### $w run$i"; sudo nerdctl run --rm \
    -m 512m --memory-swap 512m \
    -v "$PWD/runs/pgdq-$w:/pgdq:ro" \
    -v "/dev/shm/pgdq/control.sql:/dump.sql:ro" \
    postgres:16 \
    bash -c 'time /pgdq parse --source /dump.sql --dqcache /tmp/x.dqcache >/dev/null'
done; done
# reps 4-6 swap the inner order to `census nocensus`.
# the floor, same container, same file:
sudo nerdctl run --rm -m 512m --memory-swap 512m \
  -v "/dev/shm/pgdq/control.sql:/dump.sql:ro" postgres:16 \
  bash -c 'time dd if=/dump.sql of=/dev/null bs=4M'
```

**Never redirect stderr inside a timed command.** Some shells route `time`'s
own report through the timed command's redirection, so a `2>/dev/null` meant
to hide the binary's chatter deletes the figure and leaves a labelled run with
no number under it. Let the binary's stderr reach the log.

**The large-object skip is the measurement that justifies it.** Completing a
3GB region inside a 512MB limit, at the same rate the `COPY` path walks the
same kind of bytes and less than half what the `INSERT` path costs, is the
"skipped, not walked" evidence: walking it statement by statement would mean
one span *and one stored text string* per `lowrite` call, hundreds of
thousands of them.

## The census on array-bearing rows nearly quadruples a warm scan

The other side of the figure above: a 3.00 GiB dump where **every** row holds
an array, so the census's pre-filter passes on all of them and every field of
every row is split out and inspected. Generated by the same script with
`--arrays --composite`, so the file differs from the control in exactly the
three stress columns — `v_int_array` (3–5 elements), `v_int_array_long` (50)
and `v_comp` (a two-field composite). 699,962 rows, 4,602 bytes each, 19
columns.

Census on is the working tree; census off is the same tree with one line
added, so nothing but the census differs between the two binaries (below).
Six reps each, the pair run in both orders; medians, with the full spread.

| | Census off | Census on | Δ |
|---|---|---|---|
| warm, on tmpfs | **0.468 s** (0.465–0.478) | **1.729 s** (1.699–1.740) | **+1.26 s, +270%** |

Both binaries complete inside the 512 MB cgroup; no max-RSS figure is quoted,
for the reason under the scan-throughput table. The `dd` floor for a 3.00 GiB
file in this container is 0.326 s, so census-off is within 1.4× of it and
census-on is 5.3× it.

**What this says.** The census costs **1.26 s per 3.00 GiB of array-bearing
rows** — 1.80 µs per 19-column row — which is **+270%** on a scan reading from
memory, i.e. the census does nearly three times the work the rest of the scan
does on this shape. The cold reading is **+2.6%** off this SSD against a
5.73 s floor — pre-`M11`, on the superseded apparatus, and re-taken by `M14`
with the section above; the field-splitting half that dominates here is
unchanged since, so it is the right order. Both numbers are the same CPU;
which one a user sees is decided by whether the bytes are already resident.

**The pre-filter is 45 ns of that 1.80 µs** (previous section, same
apparatus and the same census-off baseline to within 15%). So splitting the
row into fields and running `observe` over all 19 of them — the work the
pre-filter exists to avoid — is **97.5% of the census's whole cost**, and the
pre-filter is what keeps the brace-free case off that path. The census is
unconditional either way (`architecture.md`, "The array shape census") — the
alternative is a query that cannot retype its array columns without a second
pass.

**The field-splitting half is the same on either libc**, which is what says it
is CPU rather than allocator: the musl leg of the same sweep read 1.29 s
census-off against 2.56 s census-on, a Δ of 1.27 s against glibc's 1.26 s,
while its *absolute* legs were 2.5× higher.

Both binaries, then the alternating runs. Census off is `pub(crate) fn
on_row`'s body in `map.rs` preceded by a bare `return;` — the pre-filter and
everything after it, and nothing else:

```sh
cd scripts && uv run generate_perf_data.py --arrays --composite \
  --size-mb 3072 --seed 42 /dev/shm/pgdq/arrays.sql
```

Then the identical loop the previous section gives, with
`/dev/shm/pgdq/arrays.sql` in place of the control.

*Rejected:* a `no-census` cargo feature, so this reproduces as a flag instead
of a source edit. Neither crate declares a `[features]` section today, and the
first one a project adds sets the precedent for what features are for — here,
a build in which `architecture.md`'s "the census is unconditional" is untrue,
serving a comparison taken about once a phase. The escape if the patch-and-
revert ever bites is to drop the comparison, not to gate it: the absolute
figures (45 ns/row rejected, 1.80 µs/row inspected) are what
[`roadmap-P7-scan-performance-inbox.md`](roadmap-P7-scan-performance-inbox.md) actually consumes, and
the census-off column exists to establish it once.

## Nested decode costs what it copies, and an element is an allocation

`benches/decoders.rs`'s `nested` group, criterion medians. **Two controls,
because there are two questions.**

- **Copy** — `String::from` over the same byte count: 21.0 ns at 49 bytes,
  25.9 ns at 601, 20.0 ns at 42. A nested value has no borrowed arm
  (`crate::batch::append_nested`), so this isolates what the *parse* costs on
  top of the copy it cannot avoid, which is the variable P7 is choosing
  over.
- **View** — one `append_view_unchecked` into a block the builder does not
  own, which is what `push_utf8view_field` does for an unescaped text field:
  **3.14 ns**, from `text_view_x1024`'s 3.216 µs ÷ 1024. Length-independent,
  which is the point of a view. This is what a user comparing a text column
  against an array column actually pays.

| Literal | Bytes | `decode` | `render` | ÷ copy | ÷ view |
|---|---|---|---|---|---|
| `integer[]`, 4 elements | 49 | 265 ns | 240 ns | **12.6×** | **84×** |
| `integer[]`, 50 elements | 601 | 3.85 µs | 1.54 µs | **149×** | **1225×** |
| two-field composite | 42 | 217 ns | 215 ns | **10.8×** | **69×** |

**Both control figures are read with a caveat.** `text_view_x1024` reports
1024 appends and must be divided — timing one append through
`iter_batched_ref` gave ~12.6 ns against a harness floor that `bool/decode`
puts at ~1.1 ns, so three quarters of it was criterion. And 3.14 ns is a
*floor* on the borrowed arm rather than the borrowed arm itself:
`push_utf8view_field` also scans the chunk deque with `find_map` and calls
`block_for`. So the `÷ view` column bounds the real ratio **from above**.

**What this says.** Cost is per *element*, not per byte: the two array lengths
differ only in element count, and the slope between them is **78 ns per
element** decoding and **28 ns per element** rendering. That is the shape of
one allocation per element, which is what `ArrayLiteral::elements` being a
`Vec<Option<String>>` buys — every element is its own `String`. A composite
sits where its field count says it should: two fields, and it costs about what
a four-element array does.

Comparing a composite against an array of the same *byte* count is therefore
meaningless; comparing them per element is the only reading these three rows
support.

```sh
cargo bench -p pgdump_query --bench decoders -- nested
```

## A typed query over nested columns costs 14 µs a row more than a string one

The end-to-end half of the figure above: what the per-element cost actually
costs a user. Two controls, on two axes. Within a file, `--schema-mode
strings` resolves every column to `Utf8View` and takes the zero-copy path, so
the `typed` run differs from it by decode plus Arrow build plus typed render
and nothing else. Across files, further 3.00 GiB dumps holding fewer of the
nested columns are what attribute the difference to a particular one — `pgdq
query` has no column projection, so there is no within-file way to ask.

Three inputs on tmpfs, output to `/dev/null`. **One interleaved sweep**: five
reps, each rep running both modes on all three files in turn, so the slow
upward drift across a long session lands on every row equally rather than on
whichever file went first. Medians of five:

| File | Rows | `strings` | `typed` | `typed` − `strings` | Ratio |
|---|---|---|---|---|---|
| control — 16 scalar columns | 814,362 | 4.43 s | 10.66 s | **7.66 µs/row** | 2.41× |
| `--composite` — the same 16 plus one composite | 803,995 | 4.43 s | 10.98 s | **8.15 µs/row** | 2.48× |
| `--arrays --composite` — the same 16 plus three nested | 699,962 | 5.48 s | 20.54 s | **21.52 µs/row** | 3.75× |

Every run completes inside the 512 MB cgroup; no max-RSS figure is quoted, for
the reason under the scan-throughput table.

**The per-row difference is the figure; the ratio is derived and does not
travel.** A ratio carries that session's `strings` leg in its denominator, and
that leg is apparatus-sensitive far beyond the ~10% session drift first blamed
for it: the same three files read 9.74 / 9.76 / 9.56 s on a page-cache-warm
SSD with a musl binary and 0.77 s of wrapper, against 4.43 / 4.43 / 5.48 s
here. That alone moved the nested ratio 3.08× → 3.75× while the per-row
difference moved 15.1 → 13.9 µs. Quote a ratio only against the sweep it came
from; the design consumes the differences.

**What this says.** Typing the 16 scalar columns costs **7.7 µs per row**;
typing those plus the three nested ones costs **21.5 µs per row**. So three
nested columns — 19% more columns — cost **13.9 µs of every row**, nearly
twice what all sixteen scalar columns together cost. **The two array columns
carry essentially all of it**: adding the composite column alone moves the
per-row figure by **+0.49 µs** (paired median over the five reps +0.59),
against a cross-file instrument whose own floor reads +0.15 and whose per-rep
readings overlap it across most of their range — so ~4% of the three columns'
cost, and a bound rather than a resolution (below).

Each per-row figure is that file's own `typed` minus its own `strings`, which
is what makes the subtraction legitimate: whatever the untyped baseline is
worth on a given file — and the three files hold different row counts at the
same byte count — it cancels out of that file's own difference, and would not
cancel out of a cross-file ratio.

**The untyped baseline is not file-independent, and the cause is the census —
measured, not inferred.** The control and the `--composite` file read within
0.03% of each other; the `--arrays --composite` file reads **24% above both**.
Its rows are the only ones carrying a `{`, so they are the only ones the
mapping pass's array-shape census splits into fields. Running the same query
with the **census-off** binary settles it:

| | control | `--arrays --composite` | gap |
|---|---|---|---|
| census on | 4.390 s | 5.505 s | **+1.115 s** |
| census off | 4.356 s | 4.281 s | **−0.075 s** |

The gap does not shrink, it **inverts** — with the census gone the arrays file
is slightly *cheaper*, which is what its 14% lower row count should give. The
census accounts for 1.190 s of a 1.115 s gap, and its cost measured this way
reproduces the `parse` figures two sections above to within 3% (+0.034 s
against +0.037 on the control, +1.224 against +1.262 on the arrays file) — a
cross-check on a different command.

So a `strings` leg is a scan plus a census whose price depends on the data's
shape, not a flat per-byte floor. Regenerate with `runs/m13-census-baseline.sh`'s
method: the same query loop below, run with both binaries. Earlier sweeps put all three baselines within 2% and
read that as evidence the untyped path was byte-driven; at 9.6 s legs a 1 s
difference was inside the spread, and it is not at 4.4 s.

The micro above covers 6.3 µs of that 13.9 µs (decode plus render for a
4-element array, a 50-element array and a two-field composite). The remaining
~7.6 µs is the Arrow build the micro does not reach: 56 per-element
`append_value` calls into the child builders, plus the list offsets. **The
literal parse is the smaller half of nested decoding**, which is the fact
P7 needs before deciding what to do about nested values always copying.

### The cross-file subtraction bottoms out at about half a microsecond a row

**One composite column sits at the edge of what this instrument can resolve.**
Two readings of the same quantity, from the same sweep, plus the control on
the instrument itself:

| Reading | Reps | Paired median | Per-rep readings |
|---|---|---|---|
| composite column's share — control against `--composite` | 5 | **+0.59 µs** | +0.29, +0.33, +0.59, +0.81, +1.03 |
| **the instrument's own floor** — control against a second control (`--seed 43`, same 16 columns) | 6 | **+0.15 µs** | −0.31, −0.14, −0.02, +0.33, +0.40, +0.92 |

The second row is the control on the *instrument*: two files that differ only
in their random seed should differ by zero, and instead they span −0.31 to
+0.92 µs per row. **The two ranges overlap across most of their width**, which
is the honest picture and the reason the ninth standing rule forbids quoting an
interval here — an earlier draft put these at +0.61 ± 0.14 against +0.20 ± 0.18
and made the separation look like a result. The composite's reading is
consistent with the micro, which puts the column at 0.43 µs of decode plus
render before any Arrow build, and is not resolved from the floor.

What the figure supports is therefore still a **bound**: the composite column
costs **around half a microsecond of every row end to end, ~4% of the 13.9 µs
the three nested columns cost together**. That is the answer to "which of the
three columns is the cost" — the arrays, by an order of magnitude.

*Retracted:* the reading that this subtraction is *biased* rather than merely
imprecise. A musl-built leg of the same sweep put the composite file
**1.16 µs/row below** the control on the same five reps — a negative cost,
which adding a decoded column cannot produce — and that was read as a
structural confound in differencing two files of different row length. The
glibc leg reverses the sign on the same inputs and the same reps, so what was
being measured was the allocator, not the instrument. The floor stands at
roughly ±0.5 µs/row; the confound does not. This pair is also the ninth
standing rule's demonstration.

*Not taken:* the instrument that would resolve it. Two files whose data
sections are **byte-identical**, one declaring `v_comp` as
`public.perf_comp` and the other as `text`, differ only in whether that one
column is decoded — same rows, same bytes, so the per-row normalization that
carries the floor above disappears. It needs a generator knob that writes a
deliberately weaker declaration, which is a new instrument rather than this
slice's, and nothing yet needs a figure that sharp.

**`typed` and `strings` agree byte for byte on all three inputs**, as they do
on what `pg_dump` writes (`architecture.md`, "CLI surface") — so `cmp` on the
two outputs is a valid smoke test here, and
`pgdump_query-cli/tests/perf_generator_fidelity.rs` asserts it on small
generated files, the control and both nested flag combinations that back a
figure, so the generator cannot drift back out of that agreement.

```sh
cargo build --release -p pgdump_query-cli          # default target: glibc
D=/dev/shm/pgdq                                     # generate from the HOST
(cd scripts &&
 uv run generate_perf_data.py --size-mb 3072 --seed 42 $D/control.sql &&
 uv run generate_perf_data.py --composite --size-mb 3072 --seed 42 \
   $D/composite.sql &&
 uv run generate_perf_data.py --arrays --composite --size-mb 3072 --seed 42 \
   $D/arrays.sql)
for i in 1 2 3 4 5; do for f in control composite arrays; do for m in strings typed; do
  echo "### $f $m rep$i"; sudo nerdctl run --rm \
    -m 512m --memory-swap 512m \
    -v "$PWD/target/release/pgdq:/pgdq:ro" \
    -v "$D/$f.sql:/dump.sql:ro" \
    postgres:16 bash -c \
    "time /pgdq query --source /dump.sql --table public.perf --dqcache none \
       --schema-mode $m >/dev/null"
done; done; done
```

Three 3.00 GiB inputs is 9 GiB of `/dev/shm`; the floor reading needs a fourth
(`--seed 43`), so drop `composite` and `arrays` before generating it. It is
otherwise the same loop, six reps, over `control` and `control43`.

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

**These are musl-build figures**, taken before the glibc rule above and under
`CLAUDE.md`'s static-binary container recipe. The scan is device-bound at
~33% of one core, so the allocator is unlikely to move them — but the next
koji run takes them on the glibc build, and until one does they are not
comparable to the warm figures above.

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

**Stop-and-resume costs nothing measurable, and reproduces the cache
byte-for-byte.** The P9 wrap run (`runs/koji-wrap.sh`, 2026-08-27) parsed
koji cold, signalled it 1200 s in, reported the partial cache, then resumed the
same command to completion:

| | |
|---|---|
| Interrupted at | byte 19,867,623,920 of 784,019,857,152 (2%) |
| Interrupted cache | 109,916 bytes, loadable, `Scan completion: 2%` |
| Resumed leg | 764,152,233,232 bytes in 3302 s, **~231 MB/s** |
| Final cache | 247,380 bytes, **byte-identical** to the 9.1 run's |
| Blocks / rows / bytes | 74 / 19,575,829,920 / 784,019,857,152 — 9.1's figures |

The resumed leg's ~231 MB/s against the 9.1 straight-through run's ~238 MB/s is
**not** a like-for-like scan comparison: it omits the file's opening 19.9 GB,
which is `public.archive_rpm_components` and row-dense rather than byte-dense.
What it does establish is that resuming carries no detectable cost — the same
device, the same order of magnitude, with a stop and a cache reload in between.

The interrupted leg's own throughput (~16.5 MB/s) measures nothing about pgdq:
a `cargo build`/`test`/`clippy` cycle ran on the same HDD throughout. The
resumed leg, uncontended over the same file, is what says so.

**The byte-identical cache is the run's real product**, and it is a property,
not a figure: a scan stopped inside a hundred-gigabyte block and resumed
produces the same structural record — span for span — as an uninterrupted one.
`architecture.md`, "CLI surface", states it. The fixture-scale version is
`pgdump_query/tests/map_file.rs`'s
`a_cancelled_map_file_reports_it_and_banks_what_it_scanned`, which asserts the
resumed *index* equals an eager scan's; koji is where the same property is
checked on the serialized cache, at a scale no fixture reaches.

The run was driven by `runs/koji-wrap.sh`, which is **gitignored** — it
hardcodes one machine's dump path and nothing in the repo consumes its output
(`CLAUDE.md`, "Long-running processes"). So the recipe is here, since a figure
whose command lives only in an ignored directory is a figure with no command:

```sh
cargo build --release --target x86_64-unknown-linux-musl -p pgdump_query-cli
R="-m 512m --memory-swap 512m \
  -v $PWD/target/x86_64-unknown-linux-musl/release/pgdq:/pgdq:ro \
  -v $PWD/runs:/out -v /path/to/koji.dump:/dump.sql:ro postgres:16-alpine"

# leg 1 — cold, interrupted. `exec` is load-bearing: it makes pgdq PID 1 so the
# signal reaches the guard rather than the wrapping shell.
sudo nerdctl run -d --name wrap1 $R sh -c \
  'exec /pgdq parse --source /dump.sql --dqcache /out/koji-wrap.dqcache \
     >> /out/koji-wrap-scan.log 2>&1'
sleep 1200 && sudo nerdctl stop -t 120 wrap1
sudo nerdctl inspect -f '{{.State.ExitCode}}' wrap1     # 130 (SIGINT), see below

# the interrupted cache must come back typed — both counts zero
sudo nerdctl run --rm $R /pgdq info --dqcache /out/koji-wrap.dqcache --verbose \
  | grep -c 'not declared\|metadata not scanned'

# leg 2 — resume the identical command, then compare
sudo nerdctl rm -f wrap1 && sudo nerdctl run -d --name wrap2 $R sh -c \
  'exec /pgdq parse --source /dump.sql --dqcache /out/koji-wrap.dqcache \
     >> /out/koji-wrap-scan.log 2>&1'
cmp runs/koji-wrap.dqcache runs/koji-9.1.dqcache
```

**The stop exercises the `SIGINT` arm, not `SIGTERM`**, and no flag on
`nerdctl run` changes that: the `postgres` images set `STOPSIGNAL SIGINT` and
`nerdctl stop` sends what the image label says, so `--stop-signal SIGTERM` is
accepted and ignored (`CLAUDE.md`). Exit **130**, not 143. Both arms reach the
same guard; the `SIGTERM` one is reached directly with `nerdctl kill -s
SIGTERM`, and at fixture scale by the CLI's own tests.

**Neither the exit codes nor the identity check are asserted by the script** —
it logs the expected value beside the observed one and a reader compares. That
is deliberate for a run whose whole point is to be read by a later session, but
it means "the log says done" is not the same as "the checks passed".

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
ranged GET that can hang, and that is a P6 decision
(`roadmap-P6-embeddable-engine-inbox.md`).

## Per-block cache saving is quadratic in block count, and so is the map

The regime koji cannot show: **block-rich and byte-poor** — a schema with
thousands of tables, or one partitioned table with a daily leaf over a decade.
Four columns, three rows per table, the schema section then the data section,
which is how `pg_dump` orders a plain dump:

```sh
cd scripts
for n in 500 1000 2000 4000; do
  uv run generate_block_count_bench.py --blocks $n --out /dev/shm/pgdq/r$n.sql
done
```

`parse` on each, cache removed first, inputs on tmpfs, both columns in one
interleaved sweep — two passes, the pair run in both orders, medians below.
"Before" is `b726f6b`, the commit preceding `SaveThrottle`; "after" is the
working tree. The cache is written to the tmpfs directory too, mounted into
the container, so no run writes to the container's own layer.

**"Before" is a whole-commit comparison, not a throttle-isolating one.** The
two builds differ in everything that landed from 9.5 onward, not only in the
save throttle, so the column says what the throttle era bought and must not be
differenced against a later change. What isolates a mechanism is the
census-off method above — one line, one rebuild — and what the P7 inbox
consumes is the map's own quadratic below, which needs no historical build at
all.

| blocks | dump | final cache | before | after | saves before → after |
|---|---|---|---|---|---|
| 500 | 248 KB | 322 KB | 0.68 s | 0.29 s | 503 → 15 |
| 1000 | 496 KB | 647 KB | 2.59 s | 1.12 s | 1003 → 27 |
| 2000 | 997 KB | 1.3 MB | 10.86 s | 4.41 s | 2003 → 51 |
| 4000 | 2.0 MB | 2.6 MB | 45.84 s | 20.10 s | 4003 → 103 |

Every run is 99% CPU at every point: the cost is *serializing* the index, not
writing it. The control is the same byte count in **one** `COPY` block —
`uv run generate_perf_data.py --size-mb 2 --seed 42 /dev/shm/pgdq/one_block.sql`,
which `parse` finishes in under 10 ms — so at 4000 blocks the overhead is three
orders of magnitude above the scan it protects.

**The throttle does exactly what it was designed to do, and the series still
quadruples per doubling.** Saves fall well under the `1/K` bound — visible in
the last column, and *self-tuning*: the throttle skips a save unless 20× the
last save's own duration has elapsed, so a faster machine or libc saves fewer
times, not the same number faster. The ~27 s of saving at 4000 blocks becomes
~1.5 s of a 20.1 s scan. What is left is a *second* quadratic with the same
shape and a different cause: every `CopyEnd` clones the whole span list
(`map::Builder::snapshot`, then `stream::splice` over the prefix), so the map
is O(blocks²) with the cache **disabled entirely**:

```sh
# maps to EOF (the table never matches) and never saves
/pgdq query --source /dump.sql --table public.nosuchtable --dqcache none
```

| blocks | 1000 | 2000 | 4000 |
|---|---|---|---|
| map only, no saving | 1.11 s | 4.32 s | 18.62 s |

So at 4000 blocks the map is **93% of what a throttled `parse` costs** —
18.6 s of 20.1 s — and the cache is the remaining 1.5 s. Against the
*unthrottled* build the split is even: 27 s of saving, 18.6 s of mapping.
Closing the second half means not rebuilding the span list per block;
it is filed in [`roadmap-P7-scan-performance-inbox.md`](roadmap-P7-scan-performance-inbox.md), because it
is a change to how the map is assembled rather than to when it is written.

Save counts come from `strace -f -e trace=open,openat` filtered to the cache
path (`std::fs::write` opens once per save; the first is the load's miss).
**Trace both calls**, not `openat` alone: glibc uses `openat` and musl uses
`open`, so tracing one of them silently reports zero saves against the other
libc. The counting stage runs on the host and is untimed, so `strace`'s
overhead reaches no figure. Reproducing the "before" column means building the
commit that precedes the throttle — `git worktree add <dir> <commit>` and a
release build there, the same two-binary method the census figure above uses.

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

`decoders.rs`'s `nested` group is the one group that is a ratio rather than a
tripwire — see "Nested decode costs what it copies". The generator's array and
composite stress columns are behind `--arrays` and `--composite`, and
`whole_file.rs` passes neither: that bench's input stays the brace-free
control, the same shape the scan-throughput and census figures were taken on.
`whole_file.rs` regenerates `runs/perf-whole-file.sql` when it is missing
**or** when `runs/perf-whole-file.stamp` disagrees with a hash of
`generate_perf_data.py` and the bench's size constant, so a change to the
generator is picked up without anyone remembering to delete the input.
