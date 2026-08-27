# Measurements

Every performance figure the design relies on, with the command that
reproduces it. A baseline nobody can re-run is a rumour with a decimal point,
so **a figure that loses its regeneration command should be deleted, not
kept**.

All figures are on the hardware `CLAUDE.local.md` describes. Synthetic inputs
are regenerable with `--seed 42` and are **never committed** — they measure
throughput, not correctness, which stays entirely fixture-based.

Six standing rules for reading anything below:

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
  below), which is taken cold on purpose. Every warm figure below predates this
  rule and was taken page-cache warm off the SSD; **`M13` re-takes the whole
  warm set on tmpfs in one session**, rather than each figure drifting onto the
  new footing whenever someone next touches it.
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
- **Long runs are detached.** A koji-scale scan is roughly an hour; see
  `CLAUDE.md`, "Long-running processes", for why waiting on one is expensive
  and what to do instead.
- **A koji figure taken while local work ran is not a figure.** Whether this
  checkout and the sample share a spindle is a machine fact — see
  `CLAUDE.local.md`'s hardware section, which records what the contention costs
  here.

## The warm set, and what `M13` re-takes

Five figures below are page-cache-warm reads off the SSD, taken before the
tmpfs rule existed: both census figures, the nested end-to-end table, the
per-block quadratic table, and the `COPY` path's warm CPU (2.92 s) that the
scan-throughput table cites as its device-bound evidence. `M13` re-takes them
together, in one session, on tmpfs — together because a figure re-taken alone
would leave the doc's warm figures disagreeing about regime, which is the
failure the standing rules were written from.

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

Max RSS is ~40–45 MB across all three.

**What this says.** The `COPY` and large-object paths are device-bound: they
spend about a fifth more wall-clock than reading the same bytes and doing
nothing, and their CPU (2.92 s warm, below) is well under the 5.73 s the read
takes. The `INSERT` path is not — it takes 8.6 s per 3 GiB *longer* than the
`COPY` path on the same device, because every line is still decoded into
`Event::Line` and pushed through the statement accumulator (see
[`architecture.md`](architecture.md), "Bulk regions"). Mapping an `--inserts`
file costs about what *decoding* a `COPY` file costs, not what *scanning* one
costs: at 1 TB that is ~45 minutes of CPU no `COPY` dump pays, against the
~15 minutes the `COPY` path spends on 1 TB in total. Correctness, tiling and
row counts are unaffected. The fix is a scanner-level `INSERT` path;
[`roadmap-phase7-inbox.md`](roadmap-phase7-inbox.md) holds it.

**The CPU ceiling under the `COPY` row is 2.92–2.96 s** — the same scan with
the file already page-cache resident, ~1.10 GB/s. That is the number the two
census figures below are differences against, and the reason they are stated
warm: at 481 MB/s the device hides everything the CPU does.

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
  /usr/bin/time -f "run$i %e s maxrss=%MkB" sudo nerdctl run --rm \
    -m 512m --memory-swap 512m \
    -v "$PWD/target/x86_64-unknown-linux-musl/release/pgdq:/pgdq:ro" \
    -v "/path/to/bench.sql:/dump.sql:ro" \
    postgres:16-alpine /pgdq parse --source /dump.sql --dqcache /tmp/x.dqcache
done
```

`parse` is the only command that reads the dump
([`architecture.md`](architecture.md), "CLI surface"), and the cache it must
write goes to the container's ephemeral layer — a few hundred KB against 3 GiB
read, which is why these figures are comparable to the ones taken before that
split existed.

## The census on brace-free rows costs 39% of a warm scan

The census walks every data row of every block any mapping pass maps — a cold
query's included, since a mapped block always carries one
([`architecture.md`](architecture.md), "The array shape census"), so it is a
change to the scan hot path. Same 3.00 GiB `COPY` control as above: no `{` or
`[` in any data row, so every row is rejected by the census's own pre-filter
after one pass over its bytes and no row is ever split into fields. That is
deliberately the koji shape — koji's six array columns are entirely NULL — and
it is the case worth knowing the price of, since it is what a `pgdq parse`
over a real dump mostly does.

Census on is the working tree; census off is the same tree with one line
added, so nothing but the census differs between the two binaries (below).

| | Census off | Census on | Δ |
|---|---|---|---|
| warm (3 runs) | 2.11 / 2.12 / 2.11 s | 2.96 / 2.95 / 2.93 s | **+39%** |
| cold (2 runs) | 6.67 / 6.66 s | 6.73 / 6.77 s | **+1.2%** |

Max RSS ~40–47 MB either way. `cat` → `/dev/null` on the same file in the same
session: 5.73 s cold, 0.08 s warm.

**What this says.** The pre-filter is not free: **0.84 s per 3.00 GiB of
brace-free rows**, 1.03 µs per 16-column row of 3,943 bytes — one pass of
`raw.iter().any(…)` over the row at ~3.8 GB/s, which is about what a scalar
byte loop gives. On a page-cache-warm scan
that is +39%; on a cold read from this SSD the device floor hides all but 1.2%
of it.

So the census's cost is **two-tier, not present-or-absent**: every row pays the
pre-filter, and a row that passes it pays field splitting and `observe` on top
(next section). It is unconditional either way
([`architecture.md`](architecture.md), "The array shape census") — the
alternative is a query that cannot retype its array columns without a second
pass — but "free on brace-free data" is not what the measurement says, and the
earlier reading that it was came from taking the pair while the page cache was
still filling, where a 0.8 s difference sits inside the run-to-run spread.

**This figure is superseded by `M13`** (queued 2026-08-27; `docs/status/STATUS.md`,
"Not started"), which re-takes it after two changes it sits downstream of:
`M12` switches `scripts/generate_perf_data.py`'s `FRACTIONS` to a uniform
microsecond draw, changing the input's bytes, and `M11` replaces the
pre-filter's hand-rolled scalar loop with `memchr2`, which is expected to take
the +39% to roughly +7%.

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
cd scripts && uv run generate_perf_data.py --size-mb 3072 --seed 42 \
  /path/to/copy_control.sql
cargo build --release --target x86_64-unknown-linux-musl -p pgdump_query-cli
cp target/x86_64-unknown-linux-musl/release/pgdq runs/pgdq-census
# add `return;` as the first statement of map::Builder::on_row, rebuild,
# copy to runs/pgdq-nocensus, then revert.
cat /path/to/copy_control.sql > /dev/null          # warm, per the standing rule
for i in 1 2 3; do for w in nocensus census; do
  /usr/bin/time -f "$w run$i %e s maxrss=%MkB" sudo nerdctl run --rm \
    -m 512m --memory-swap 512m \
    -v "$PWD/runs/pgdq-$w:/pgdq:ro" \
    -v "/path/to/copy_control.sql:/dump.sql:ro" \
    postgres:16-alpine /pgdq parse --source /dump.sql --dqcache /tmp/x.dqcache
done; done
```

The cold pair takes `sudo sh -c 'sync; echo 3 > /proc/sys/vm/drop_caches'`
before *each* run, not once before the pair.

**The large-object skip is the measurement that justifies it.** Completing a
3GB region inside a 512MB limit, at the same rate the `COPY` path walks the
same kind of bytes and less than half what the `INSERT` path costs, is the
"skipped, not walked" evidence: walking it statement by statement would mean
one span *and one stored text string* per `lowrite` call, hundreds of
thousands of them.

## The census on array-bearing rows costs 81% of a warm scan

The other side of the figure above: a 3.00 GiB dump where **every** row holds
an array, so the census's pre-filter passes on all of them and every field of
every row is split out and inspected. Generated by the same script with
`--arrays --composite`, so the file differs from the control in exactly the
three stress columns — `v_int_array` (3–5 elements), `v_int_array_long` (50)
and `v_comp` (a two-field composite). 701,287 rows, 4,593 bytes each, 19
columns.

Census on is the working tree; census off is the same tree with one line
added, so nothing but the census differs between the two binaries (below).

| Run | Census off | Census on |
|---|---|---|
| warm 1 | 2.13 s | 3.87 s |
| warm 2 | 2.13 s | 3.91 s |
| warm 3 | 2.11 s | 3.84 s |
| cold | 6.66 s | 6.83 s |

Max RSS ~40–47 MB either way. `cat` → `/dev/null` on the same file in the same
session: 5.73 s cold, 0.09 s warm.

**What this says.** The census costs **1.75 s per 3.00 GiB of array-bearing
rows** — 2.50 µs per 19-column row — which is **+81%** on a page-cache-warm
scan and **+2.6%** on a cold read from this SSD, where the device floor
(5.73 s) hides most of it. Both numbers are the same CPU; which one a user
sees is decided by whether the bytes are already resident.

**The pre-filter is 1.03 µs of that 2.50 µs** (previous section, same warm
regime and the same census-off baseline to within 1%). So splitting the row
into fields and running `observe` over all 19 of them — the work the
pre-filter exists to avoid — costs the remaining **1.47 µs**, a little under
half again what refusing the row outright costs. The census is unconditional
either way (`architecture.md`, "The array shape census") — the alternative is
a query that cannot retype its array columns without a second pass.

Both binaries, then the alternating runs. Census off is `pub(crate) fn
on_row`'s body in `map.rs` preceded by a bare `return;` — the pre-filter and
everything after it, and nothing else:

```sh
cd scripts && uv run generate_perf_data.py --arrays --composite \
  --size-mb 3072 --seed 42 /path/to/arrays.sql
cargo build --release --target x86_64-unknown-linux-musl -p pgdump_query-cli
cp target/x86_64-unknown-linux-musl/release/pgdq runs/pgdq-census
# add `return;` as the first statement of map::Builder::on_row, rebuild,
# copy to runs/pgdq-nocensus, then revert.
cat /path/to/arrays.sql > /dev/null          # warm, per the standing rule
for i in 1 2 3; do for w in nocensus census; do
  /usr/bin/time -f "$w run$i %e s maxrss=%MkB" sudo nerdctl run --rm \
    -m 512m --memory-swap 512m \
    -v "$PWD/runs/pgdq-$w:/pgdq:ro" \
    -v "/path/to/arrays.sql:/dump.sql:ro" \
    postgres:16-alpine /pgdq parse --source /dump.sql --dqcache /tmp/x.dqcache
done; done
```

The cold pair takes `sudo sh -c 'sync; echo 3 > /proc/sys/vm/drop_caches'`
before each run.

*Rejected:* a `no-census` cargo feature, so this reproduces as a flag instead
of a source edit. Neither crate declares a `[features]` section today, and the
first one a project adds sets the precedent for what features are for — here,
a build in which `architecture.md`'s "the census is unconditional" is untrue,
serving a comparison taken about once a phase. The escape if the patch-and-
revert ever bites is to drop the comparison, not to gate it: the absolute
figures (1.03 µs/row rejected, 2.50 µs/row inspected) are what
[`roadmap-phase7-inbox.md`](roadmap-phase7-inbox.md) actually consumes, and
the census-off column exists to establish it once.

## Nested decode costs what it copies, and an element is an allocation

`benches/decoders.rs`'s `nested` group, criterion medians. **Two controls,
because there are two questions.**

- **Copy** — `String::from` over the same byte count: 21.0 ns at 49 bytes,
  25.9 ns at 601, 20.0 ns at 42. A nested value has no borrowed arm
  (`crate::batch::append_nested`), so this isolates what the *parse* costs on
  top of the copy it cannot avoid, which is the variable Phase 7 is choosing
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

## A typed query over nested columns costs 15 µs a row more than a string one

The end-to-end half of the figure above: what the per-element cost actually
costs a user. Two controls, on two axes. Within a file, `--schema-mode
strings` resolves every column to `Utf8View` and takes the zero-copy path, so
the `typed` run differs from it by decode plus Arrow build plus typed render
and nothing else. Across files, further 3.00 GiB dumps holding fewer of the
nested columns are what attribute the difference to a particular one — `pgdq
query` has no column projection, so there is no within-file way to ask.

Three inputs, all page-cache warm, output to `/dev/null`. **One interleaved
sweep**: five reps, each rep running both modes on all three files in turn, so
the slow upward drift across a long session lands on every row equally rather
than on whichever file went first. Medians of five:

| File | Rows | `strings` | `typed` | `typed` − `strings` | Ratio |
|---|---|---|---|---|---|
| control — 16 scalar columns | 817,024 | 9.74 s | 20.55 s | **13.23 µs/row** | 2.11× |
| `--composite` — the same 16 plus one composite | 806,322 | 9.76 s | 20.35 s | **13.13 µs/row** | 2.09× |
| `--arrays --composite` — the same 16 plus three nested | 701,287 | 9.56 s | 29.45 s | **28.36 µs/row** | 3.08× |

Max RSS ~36–45 MB throughout.

**The per-row difference is the figure; the ratio is derived and does not
travel.** A ratio carries that session's `strings` leg in its denominator, and
that leg moves ~10% between sessions on an identical binary and an identical
file — `M10` read 8.80 s and 8.04 s where this sweep reads 9.74 s and 9.56 s,
which alone moved the nested ratio 3.45× → 3.08× while the per-row difference
moved 14.6 → 15.1 µs. Quote a ratio only against the sweep it came from; the
design consumes the differences.

**What this says.** Typing the 16 scalar columns costs **13.2 µs per row**;
typing those plus the three nested ones costs **28.4 µs per row**. So three
nested columns — 19% more columns — cost **15.1 µs of every row**, slightly
more than all sixteen scalar columns together. **The two array columns carry
essentially all of it**: adding the composite column alone moves the per-row
figure by −0.10 µs, which is a *negative* cost and therefore the instrument's
floor rather than a measurement (below).

Each per-row figure is that file's own `typed` minus its own `strings`, which
is what makes the subtraction legitimate: whatever the untyped baseline is
worth on a given file — and the three files hold different row counts at the
same byte count — it cancels out of that file's own difference, and would not
cancel out of a cross-file ratio.

The three baselines sit within 2% of each other (9.56–9.76 s across a 14%
spread in row count). An earlier, **non-interleaved** session of the same
comparison put two of them 9% apart and read the gap as the untyped path being
partly per-row; that reading is retracted, since the row-count spread is
unchanged here and the gap is not. Running a multi-file comparison a file at a
time maps the session's own drift onto file identity, which is why the sweep
above interleaves and why the standing rules now require it. What the 2% does
*not* establish is that the untyped path is byte-driven — one sweep agreeing
is weaker evidence than one sweep disagreeing was.

The micro above covers 6.3 µs of that 15.1 µs (decode plus render for a
4-element array, a 50-element array and a two-field composite). The remaining
~8.8 µs is the Arrow build the micro does not reach: 56 per-element
`append_value` calls into the child builders, plus the list offsets. **The
literal parse is the smaller half of nested decoding**, which is the fact
Phase 7 needs before deciding what to do about nested values always copying.

### The cross-file subtraction bottoms out at about half a microsecond a row

**One composite column is below what this instrument can resolve**, and the
sweep above is not enough runs to see that. Three separate readings of the
same quantity:

| Reading | Reps | Composite column's per-row share |
|---|---|---|
| the sweep above | 5 (three files interleaved) | −0.10 µs (paired mean 0.00, sd 0.93) |
| control against `--composite`, interleaved | 8 | −0.49 µs (paired mean, SE 0.22) |
| **control against a second control** (`--seed 43`, same 16 columns) | 6 | **+0.22 µs** (paired mean, SE 0.16) |

The third row is the control on the *instrument*: two files that differ only
in their random seed should differ by zero, and they differ by +0.22 µs per
row with a per-rep spread of ±0.39. So a cross-file per-row difference under
roughly **±0.5 µs/row** is apparatus, not signal — and the composite column's
share, which the micro puts at 0.43 µs of decode plus render before any Arrow
build, sits inside that. The two negative readings are the proof it is not
being measured: adding a column that must be decoded and built cannot make a
row cheaper.

What the figure supports is therefore a **bound**: the composite column costs
**under ~0.5 µs of every row end to end, under 4% of the 15.1 µs the three
nested columns cost together**. That is consistent with the micro, where it is
0.43 µs of the nested group's 6.3 µs, and it is the answer to "which of the
three columns is the cost" — the arrays, by an order of magnitude.

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
cargo build --release --target x86_64-unknown-linux-musl -p pgdump_query-cli
(cd scripts &&
 uv run generate_perf_data.py --size-mb 3072 --seed 42 /path/to/control.sql &&
 uv run generate_perf_data.py --composite --size-mb 3072 --seed 42 \
   /path/to/composite.sql &&
 uv run generate_perf_data.py --arrays --composite --size-mb 3072 --seed 42 \
   /path/to/arrays.sql)
for f in control composite arrays; do cat /path/to/$f.sql > /dev/null; done
for i in 1 2 3 4 5; do for f in control composite arrays; do for m in strings typed; do
  /usr/bin/time -f "$f $m rep$i %e s maxrss=%MkB" sudo nerdctl run --rm \
    -m 512m --memory-swap 512m \
    -v "$PWD/target/x86_64-unknown-linux-musl/release/pgdq:/pgdq:ro" \
    -v "/path/to/$f.sql:/dump.sql:ro" \
    postgres:16-alpine sh -c \
    "/pgdq query --source /dump.sql --table public.perf --dqcache none \
       --schema-mode $m > /dev/null 2>/dev/null"
done; done; done
```

The floor reading swaps `composite`/`arrays` for a second control generated
with `--seed 43`, and is otherwise the same loop.

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

**Stop-and-resume costs nothing measurable, and reproduces the cache
byte-for-byte.** The Phase 9 wrap run (`runs/koji-wrap.sh`, 2026-08-27) parsed
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

`decoders.rs`'s `nested` group is the one group that is a ratio rather than a
tripwire — see "Nested decode costs what it copies". The generator's array and
composite stress columns are behind `--arrays` and `--composite`, and
`whole_file.rs` passes neither: that bench's input stays the brace-free
control, the same shape the scan-throughput and census figures were taken on.
`whole_file.rs` regenerates its input only when `runs/perf-whole-file.sql` is
missing, so a change to the generator means deleting that file before the
next `cargo bench` means anything.
