# P16.19 — The CLI emits status

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md).

## What exists now

**`tracing` is a facade-only dependency of `pgdump_query`, and a rendering
dependency of `pgdump_query-cli`.** The library calls `tracing::info!` at three
call sites and decides nothing about output — no subscriber, no format, no
level filter. The CLI's `init_status_output()` (`main.rs`) installs
`tracing_subscriber::fmt` at the top of `main()`, before `Cli::parse()`, so it
runs identically for `parse`, `info` and `query`: stderr, no ANSI, no module
target (`with_target(false)` — the line names a mechanism in the library's own
vocabulary, not a source path an internal refactor would move), `UtcTime::rfc_3339`
for the timestamp, and `Level::INFO` as the one level there is.

**Two mechanisms are named, and they are not the ones the spec row's prose
might suggest at first read.** The motivating scenario — a 44-minute `parse`
indistinguishable from a hang — runs through `stream::map_forward`, the
incremental, resumable, cancellable loop behind both `pgdq parse` and
`query`'s mapping pass. It is **not** `crate::scan::scan`, which is a second,
simpler loop this crate already had: whole-file, non-resumable, with no
cancellation hook. `scan` is what `build_index`/`build_map` run — this crate's
own always-to-EOF test oracle, exercised only from `tests/*.rs` — and what
`index::scan_preamble` runs, which **is** real, CLI-visible: `pgdq parse
--preamble-only` calls it, and so does an ordinary `parse`/`query` internally,
ahead of `map_forward`, whenever the header metadata is not already cached.
So both loops earned the instrumentation: `map_forward` for the case the
phase names, `scan` because it is a second real production path with the same
problem at a smaller scale, and because `build_index`/`build_map`'s copy costs
nothing (no subscriber renders it in a test binary that never calls
`init_status_output`).

**`crate::scan::scan`'s events are named `preamble scan started`/`preamble
scan complete`, not `scan started`/`scan complete` — the first cut used the
same name as `map_forward`'s, and it was wrong.** A single uninterrupted
`parse` runs `scan_preamble` and then `map_forward` in sequence, so a cold run
printed `scan started` → `scan complete (reached_eof=false)` → `scan started
(resumed_from=N)` → `scan complete (reached_eof=true)`, and the middle two
lines read as an interrupted scan resumed from byte `N` — the exact confusion
this slice exists to prevent, on a run nothing interrupted. Found by running
the binary, not by the tests: every test asserted that a line containing
`"scan started"`/`"scan complete"` existed, which the preamble pass's lines
also satisfy, so the ambiguity was invisible to the suite. Fixed by giving
`crate::scan::scan`'s pair its own name and leaving `map_forward`'s alone,
which also makes a **genuine** resume recognisable as one: `map_file`'s own
gate only runs the preamble pass when `resumed_from == 0`
(`stream::map_file`, `first_db_preamble_known`), so a real interrupted-then-
resumed scan skips it entirely and its log opens straight on `scan started`
naming the frontier the prior run reached, with no preceding preamble pair —
a shape a cold run never produces.
[`pgdump_query-cli/tests/status_output.rs`](../../pgdump_query-cli/tests/status_output.rs)'s
`an_uninterrupted_parse_never_looks_like_a_resumed_one` and
`a_genuine_resume_reports_no_preamble_pass_and_names_the_frontier` pin both
halves of that claim directly, the second against a cache built to look like
an interrupted scan's the way `partial_reporting.rs`'s own resume tests do.

**`XzSource::open` (`io.rs`) is the seek-table build.** It emits `seek table
build started` before `xz_seek::Reader::new` walks the file's stream footers,
and `seek table build complete` after, naming `streams`/`blocks` off the
resulting `SeekTable`. `XzSource::with_table` — the path a cache's persisted
table takes, skipping the walk entirely — earns no line, which is the whole
point: the walk this announces is the one a cache exists to make unnecessary,
so a resumed or re-run `parse` against an already-covered `.xz` file is
silent on this axis too.

**`scan started` names the arrangement once, in the library's own fields**:
`bytes` (the file size, or `map_forward`'s `resumed_from`/remaining split),
`chunk_size`, `jobs` (`Parallelism::jobs()`, always ≥1) and `memory_bytes`.
The last one also shipped wrong in the first cut: printed via `?
options.parallelism.memory_bytes()`, it rendered `Parallelism::Serial` as the
literal text `None` — `Option`'s debug spelling reaching a user-facing line,
saying nothing a reader could act on, again caught by running the binary
rather than by a test that only checked the field's *presence*. The fix is
`io::memory_budget_display(Parallelism) -> String`, a free function
deliberately **not** a method on `Parallelism`: that type's own
`memory_bytes()` exists to keep "the caller said nothing" apart from "the
caller said 64 MiB" for a source that must not have an already-announced
budget silently overwritten, and a status line has no such source to protect
— it wants the number actually governing reads either way, which is the
stated byte count where the caller gave one or `DEFAULT_MEMORY_BUDGET` — what
every pool falls back to — annotated `(default)` where they did not, so the
line always reads as a quantity and never implies a caller asked for exactly
64 MiB when nobody did. `scan_started_names_jobs_and_the_stated_memory_budget_as_a_quantity`,
`scan_started_names_the_default_memory_budget_when_none_was_stated` and a
blanket `no_status_line_ever_prints_nones_or_somes` pin this
(`status_output.rs`).

`scan complete` carries `reached_eof`, added to every completion site —
including the two `map_forward` sites where a query's mapping pass stops at
its settled target before EOF — after the first end-to-end run showed the
asymmetry: without the field, a targeted `query` printed a `scan started`
with no visible pairing whenever it settled early, since only the true-EOF
branch had a completion line at first. `preamble scan`'s own early exit (the
`ControlFlow::Break` a bounded `scan_preamble` takes at the first `COPY`
header) got the same field for the same reason — it is the *common* case for
`--preamble-only`, not the exception, so leaving it silent would have made
`preamble scan started` the normal output there and `preamble scan complete`
the rare one.

**The two "nothing to do" checks at the top of `map_forward`** (a cache
already covering the file; a query target the cache already settles) return
before either log line — deliberately: `pgdq parse` against a file that is
already fully cached is not a scan, and the manual already says so on stdout
("`parse`: reading the dump", "costs nothing and says so"); logging a
start/complete pair around nothing would contradict that sentence on a second
channel.

**`M72` and this slice are separate channels, landed in the order the phase
required.** `M72` (`stream::plan_partitions`'s `PlanNote`) is a per-query-plan
fact, announced by `pgdq query` via a plain `eprintln!` — it existed before
this slice and is untouched by it. `16.19` did not fold it into `tracing`,
narrow it, or duplicate its announcement: the phase's own ordering
(`roadmap-P16-parallel-scan.md`, "Slices") named `M72` as blocking precisely
so that this slice's "prints `--jobs` and the stated budget" claim would be
true of an arrangement the library had actually already planned, not a
misstatement — `M72` is what made a stated `--jobs` that the budget silently
cut down into a stated-*and-explained* one. The two remain visually distinct
in a captured log (`PlanNote`'s line carries no timestamp and no `tracing`
field syntax), which is a property worth knowing rather than one to fix.

## What was deliberately left out

**No level flags.** `-vvv` and `--quiet` are unallocated, per the spec row.
Nothing here reads an env var or a CLI flag to change the level; `Level::INFO`
is a constant in `init_status_output`.

**No elapsed-time field.** Every line is a point in time; nothing subtracts
one from another. The spec's own rejected alternative (emitting phase
transitions with no durations at all) was not taken either — the timestamps
are there and a reader may subtract them by hand, which is the middle ground
the spec settled on: a duration only where a document explicitly reads it as
one, never printed as if it already were.

**No target-module path in the rendered line** (`with_target(false)`). The
three call sites live in `io.rs`, `scan.rs` and `stream.rs` today; a reader
correlating logs should not have to know that, and a future refactor that
moves one of them should not change what a captured log says.

## Figures

**Four declared paths moved — `io.rs`, `scan.rs`, `stream.rs`, `main.rs` — all
already red on every figure declaring them, plus both crates' `Cargo.toml`,
which nothing declares.** Unlike `16.18`, this is not string-literal-only: a
`tracing_subscriber` is now installed on every invocation and each named phase
emits an event through it, so **reachability does not excuse it** — every
registered command shape runs through `main()` and reaches at least the
scan-completion site. What can be said without an oracle: the new lines are a
fixed few per invocation (one seek-table pair per `.xz` open, one scan pair per
`map_forward`/`scan` call), not one per row or per block, and the calls read or
write nothing belonging to the dump itself — only the process's own stderr.
`nested-decode-micro`, the one figure timing decoders none of this touches,
is unmoved.
