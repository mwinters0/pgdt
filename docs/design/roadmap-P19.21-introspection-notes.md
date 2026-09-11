# `P19.21` — the introspection the compressed account needs

What `19.18` inherits. The mechanism itself is
[`architecture.md`](architecture.md), "What the binary can report about
itself"; this doc is the part that doc has no home for — how to run it, what a
first reading looked like, and the two calls that were made inside the row.

## How to build it and read it

```sh
cargo build --release -p pgdump_query-cli --features introspect \
  --target-dir /mnt/ssd/fedora/scratch/pgdump_query/alloc-builds/introspect
```

**Its own target dir, for the reason `ensure_allocator_binary` already uses
one**: a `--features` build in the default dir overwrites
`target/release/pgdq`, which is every other figure's binary. Nothing in the
harness builds this yet — `19.18` is the first sitting that wants it, and a
build step registered before a caller exists is a step nobody runs.

The report lands on **stderr**, bracketed by `# pgdq-introspect` /
`# end pgdq-introspect`, and `measure.parse_instrument` is what reads the block
out of a stream the harness's own `rss_wrapper` also writes to. A leg that
wants the numbers gets them in `raw.json`'s `reported` dict beside
`resolved_jobs`/`resolved_budget`, under the keys `live_bytes`,
`live_peak_bytes`, `mallinfo_{arena,hblkhd,uordblks,fordblks}`,
`malloc_heaps`, `malloc_system_current` and `malloc_system_max`.

## The first reading, and why it is worth `19.18`'s hour

A **probe**, not a figure — one rep, on the host, uncontained, on a machine
that was not quiet. No document may quote it as a measurement. What it
establishes is that the instrument answers the question the account has been
asking with the wrong tool:

`control_xz.xz`, `--jobs 4 --parallel-memory 512m`, `/usr/bin/time -v` beside
the report:

| | bytes |
|---|---|
| `live_peak_bytes` — what the program held, at its high-water | 209,822,124 |
| `maxrss` — the whole resident set | 382,959,616 |
| `malloc_system_max` — the arenas' own high-water, summed over 6 heaps | 353,869,824 |
| `mallinfo_fordblks` at exit — freed, held, resident | 157,826,752 |
| `mallinfo_hblkhd` at exit — mmap-backed | 0 |

Three things follow that no subtraction between whole runs produces. The live
high-water is **about half** the resident peak, so the term `19.15` could not
name is not program structure. `hblkhd` is **zero** at exit on a run that
decoded 24 MiB blocks throughout, which is the shape the dynamic-mmap-threshold
hypothesis predicts and the opposite of what a first-allocation-only reading
would show. And 158 MB sits in `fordblks` at exit — freed by the program, held
by the allocator, still resident — which is more than nine tenths of the gap
between the live high-water and the resident peak, out of the one term a
peak-RSS leg can never separate.

`19.18` reads this arrangement under the container, with reps, and its
`getrusage` legs are the check: a term the instrument names has to show up in
the sum a peak-RSS leg measures.

## Two calls made inside the row

**The report goes to stderr, and the row said stdout.** The row's ground for
stdout was that `measure.parse_reported` already reads it; the ground against
it is stronger and is in the tree rather than in an argument —
`chunk_size.rs`'s two parity tests fail under the feature, because the
instrumented build's stdout no longer matches the shipped build's. stdout is
the answer and stderr is where this binary's diagnostics already go
(`architecture.md`, "Status output"). The harness cost is one function,
`parse_instrument`.

**The block is bracketed, which the row did not ask for.** Reading `key=value`
off stderr wholesale picks up `rss_wrapper`'s own `maxrss_kib=<n>`, and that is
a *per-rep reading* landing in the dict of facts a run states about itself —
where every other entry is identical across reps and only the last rep's copy
survives. The markers carry no `=`, so they are invisible to the parse they
delimit, and `test_measure.py` holds the two constants to each other across the
two languages.

## What it deliberately does not do

- **No sampler.** The counter keeps its own high-water and `malloc_info` keeps
  each arena's. A sampler would add a thread, an arena, and a missed-peak
  failure mode to buy something already exact.
- **No library code.** The allocator is the binary's choice
  (`architecture.md`, "The allocator is the binary's choice"), which is also
  what keeps the embedding work's audience unburdened.
- **Never timed.** `pgdq --version` carries `(instrument: counting-allocator)`
  and `measure.binary_allocator` raises on it, so an instrumented binary cannot
  be published as a figure by accident. That is the mechanical form of the
  row's "not a fourth `ALLOCATOR_LEGS` member".

## The check the row owed, and where it lives

`main.rs`'s `the_instrument_build_resolves_what_the_default_build_resolves`,
beside the other resolution tests because that is what it asserts about. It
sweeps all five committed runtime roots and pins the resolved
`(jobs, memory_bytes)` pair against literals, and it is compiled into **both**
configurations — so `cargo test -p pgdump_query-cli` and `cargo test -p
pgdump_query-cli --features introspect` are the two halves of the comparison,
and neither can drift without failing.

Building a second binary from inside a `cargo test` was the alternative and is
refused: a `--features` build needs its own target dir, which means a full
dependency rebuild on every `cargo test --workspace`, and no test in this tree
shells out to `cargo` today.

**`cargo test -p pgdump_query-cli --features introspect` is not run by `cargo
test --workspace`**, and that is the standing cost of this shape. It is in
`CLAUDE.md`'s command list, and `19.18` runs it before the sitting, since a
sitting whose instrument moved the plan is an hour spent on the wrong
arrangement.

## Staleness

`pgdump_query-cli/src/introspect.rs` falls inside paths eight figures already
declare, so `--stale` names it — but every figure it appears in was already red
on `main.rs` or `alloc.rs` before this slice, and none went from green to red.
