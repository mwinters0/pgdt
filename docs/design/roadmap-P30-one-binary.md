# P30 — One binary for distribution

What this phase will do and why; how it lands is its slices'. Progress is
`STATUS.md`'s checklist, never this file. Grilled 2026-10-04.

## Premise

A release ships one binary: `pgdt`, carrying `datafusion-cli-pgdump`'s whole
CLI as a subcommand. **The composition is code reuse, not copying**: the
DataFusion CLI's arguments and entry point become a library that `pgdt`
calls, so no flag is declared twice. `#[derive(Parser)]` on a struct also
derives `clap::Args` (`clap_derive` 4.6.4, `src/derives/parser.rs`,
`gen_for_struct`), so the copied `Args` can be a `pgdt` subcommand's variant
unchanged, its help and validation parsers included. Exposing the copied
entry point as a library function is a few more `pgdump:`-marked lines in the
upstream copy, re-applied at each DataFusion major (the header of
`datafusion-cli-pgdump/src/main.rs`).

## The shipped binary is the timed binary

**The composition is unconditional.** `cargo build --release -p pgdt` builds
the binary a release ships, and every figure in `measurements.md` times it.
This reverses the rejection `datafusion-cli-pgdump/Cargo.toml` records, a
`pgdt sql` subcommand refused for putting DataFusion into the timed build: that
rejection protected the timed build, and the timed build now *is* the shipped
one, which is what D13's own rejection of "every table a figure of an
unshipped binary" asks for. The parse path's code is unchanged by linking
DataFusion; what changes is linking and feature unification of shared
dependencies (`tokio` gains `rt-multi-thread`, unused under `pgdt`'s
`current_thread` runtime), and those are then timed rather than assumed.
Rejected: a default-off feature the release turns on, under which no figure
times the `pgdt parse` a user downloads; two shipped artifacts, a slim timed
`pgdt` beside a composed one, which gives up the phase's premise; a cargo
feature at all, default-on or off, whose only use would be a slim build that
is neither shipped nor timed and that no round's check (`scripts/check.py`
builds `--workspace` at default features) would keep compiling. So `pgdt`
depends on `datafusion-cli-pgdump` outright, and the round's existing
`--workspace` steps cover the shipped build unchanged.

**`datafusion-cli-pgdump` becomes a library, and development has one binary
too.** Its bin target goes; it exports the copied `Args` and the entry point,
the upstream copy staying one file whose diff against upstream is still the
`pgdump:` lines. A second binary would be the same CLI under another name, and
the provider figures would time it rather than what ships. What moves with it:
its integration tests to `pgdt/tests/`, running `pgdt sql` (Cargo gives
`CARGO_BIN_EXE_<name>` only to the owning crate's tests); `measure.py`'s
provider figures to `pgdt sql`, its `DFCLI`/`DFCLI_RELEASE_BIN` going; and
`docs/manual/datafusion-cli-pgdump.md` to the manual for `pgdt sql`. The crate
keeps its name, so an embedder wanting the CLI without `pgdt` can still depend
on it, and `datafusion-pgdump` stays free of `pgdt`-specific behaviour (D68's
layering is unchanged).

## The one binary links mimalloc

**One process has one global allocator, and it is mimalloc**, to stay on
DataFusion's tested and supported path: upstream's `datafusion-cli` links it
(`datafusion-cli/src/main.rs`, 55.1.0) and upstream's benchmarks default to it
(`benchmarks/Cargo.toml`, `default = ["mimalloc"]`), and every provider figure
is already taken on it. This reopens D13 on its stated terms — `pgdt` moves
off the platform allocator — and is the maintainer's decision, approved when
P30 was sketched; it does not need a phase of its own. What the move owes
before a release is this phase's (below).

**C dependencies keep the platform allocator.** No `override`: upstream links
`mimalloc` with `default-features = false`, and staying on its path is the
reason for the move. So `liblzma` (each `.xz` worker's decoder state) and
`aws-lc` allocate through libc, and the process has two heaps. The instrument
reports both (below), and a reading showing the second heap matters reopens
this.

## What the move owes before a release

**The instrument moves with the allocator, in an early slice.** `introspect`
counts in front of `System` and reads glibc's `mallinfo2`/`malloc_info`, and
`pgdt/src/alloc.rs` refuses it beside mimalloc, so after the move the only
attribution instrument would count an unshipped heap. The counting wrapper
moves in front of `MiMalloc`, mimalloc's own statistics (`mi_process_info`,
`mi_stats`, via `libmimalloc-sys`'s `extended`, as upstream's benchmarks read
them) report the Rust heap, and glibc's keep reporting the C dependencies'.
`--version`'s `(instrument: counting-allocator)` marker stays, so
`measure.py` still refuses it. This is `roadmap.md`, "Attribution is
introspective; only the gate is blind": where the instrument does not exist,
building it is the slice, before any sitting.

**One `introspect` feature, on `pgdt`**, turning on
`datafusion-cli-pgdump/introspect` (a dynamic filter's row-evaluation
timings) beside the counting allocator: the two already write to the file
`PGDT_INTROSPECT_OUT` names and become one process, so an instrumented `pgdt
sql` reports both sections there, each stating what it covers, under one
`--version` marker for `measure.py` to refuse.

**The allocator features flip.** mimalloc is the default build; `system` and
`jemalloc` become the opt-in features, still mutually exclusive and still
built in their own target directories. The `allocator` figure keeps three
legs, the default build being its mimalloc leg, so it remains the evidence
that would move the default again. D13 is rewritten in the slice that moves
the allocator: the principle — the allocator is the binary's, never the
library's — stands, with mimalloc the default and the reason the move was made
replacing the "few percent" rejection.

**The figure set is re-taken on the composed binary, and the gate is judged
against a `system` leg of the same sitting.** The last sweep is far behind the
code that will be measured, and the features landed since allocate more, so a
reading from that sweep cannot stand as the platform allocator's pass: a
failure judged against it could not be told apart from those commits'. The
pass/fail readings — the gate — are those `measure.py` takes against a
memory limit:

- `reserve`'s flagless legs across `RESERVE_LIMITS`, where a kill is a
  censored reading (`KILL_TOLERANT`);
- the `parallel-*` figures' resident against `budget + PARALLEL_HEADROOM`, the
  library's own contract;
- a kill anywhere else, which the harness raises as an apparatus failure.

Each of these is read twice at the same commit, in the same sitting: on the
shipped build and on the `system` build, so the legs differ by the allocator
alone, as the `allocator` figure's already do for time. **A reading failing
on both legs predates P30** — it is filed against the reserve (`KD34`, P23's)
or as a new `KD<k>`, and does not block the phase. **One failing on mimalloc
alone blocks it**, and the instrument attributes it. The phase moves the
allocator and shows it made nothing worse; it does not re-fit a constant.

**The reserve stays P23's (`KD34`).** `MEMORY_RESERVE` was chosen from glibc
readings (`runs/19.16-reserve-constant-20260911-2210/readings.json`), and the
attribution P23 inherits (`runs/20.8-reserve-attribution-20260916-1857/`) is
glibc's too. The slice that moves the allocator rewrites `KD34` to say so, and
P23 re-takes its readings under mimalloc on the rebuilt instrument. A release
carries `KD34` as it would have on glibc; whether its doc states the
workaround is P29's.

## The subcommand is `pgdt sql`

It names what the user gets, a SQL shell over the dump, beside `query`'s
native filtered scan. DataFusion's name and version stay visible in its help
and in `pgdt --version`.

**The version lives in one place.** `pgdt --version` gains a
`(datafusion: <version>)` marker after the existing ones, from
`DATAFUSION_CLI_VERSION`, so `measure.py`'s allocator parse and its refusal of
the instrument marker still read it. `pgdt sql` carries no `--version` of its
own: clap's `version` on the copied `Args` would print the crate's name and
version as a subcommand's. **The banner stays as upstream wrote it**,
`DataFusion CLI v<version>` unless `--quiet`: it is accurate, and every
upstream line left alone is one fewer to re-apply at a major.

**The two-tunables rule does not reach `pgdt sql`'s flags.** Composing the
copied `Args` brings upstream's numeric flags under `pgdt` — `--memory-limit`
(DataFusion's pool), `--batch-size`, `--disk-limit`, `--maxrows`,
`--top-memory-consumers` — and `every_numeric_flag_is_classified` walks every
subcommand. They are the DataFusion CLI's surface as upstream ships it, which
that rule never governed while it was a separate binary; the two flags we add,
`--dump` and `--strict-identity`, take no number. The rule stops *us* adding a
third knob, and within `sql` the provider is fitted by its session settings,
which already bill `--memory-limit`'s grant against the allowance
(`datafusion-pgdump/src/budget.rs`). So the walk skips that one subcommand, a
test pins the exemption to it alone, and the slice that adds `sql` amends
`roadmap.md`, "Two tunables fit pgdt to hardware: memory and parallelism"'s
check paragraph to say so. Rejected: classifying upstream's flags, which
either breaks the closed hardware pair or misclassifies `--memory-limit`; and
removing or renaming them inside `sql`, which breaks "flags included" and adds
marked lines to re-apply at each major.

## The process is set up after parsing

**The composed `main` parses first, then builds the runtime and the signal and
logging setup the chosen arm needs.** Tokio refuses to start a runtime inside
another, and the arms want different ones: `pgdt`'s commands a
`current_thread` runtime (D12), `sql` DataFusion's multi-threaded one. The
`sql` arm's namespace-init handlers depend on the parsed arguments
(`end_as_namespace_init(args.repl_mode())`, SIGINT caught in the REPL), so
they cannot be installed before parsing. That moves `pgdt`'s
`InitShutdown::install` from before `Cli::parse` to after it, and D26 is
indifferent: as init, a signal arriving before a handler is installed is
discarded (RT19), the window is clap's parse alone, and
`datafusion-cli-pgdump` already runs with exactly that window.

## Slicing

**One sitting, at the end.** The gate is pass/fail, its `system` legs taken
beside the shipped ones, and a failure on mimalloc alone is attributed by the
instrument rebuilt first; differencing a sweep after the allocator move
against one after the composition, or a sitting at HEAD before the move
against one after it, would be the subtraction `roadmap.md`, "Attribution is
introspective; only the gate is blind" refuses — two sittings differing by
more than the allocator. So the instrument lands first, the allocator move and
the composition land with no sitting between them, and the sweep is the last
slice, launched detached and read by a later session.
The library split is its own slice, reviewed against upstream's diff alone,
ahead of the rework of `pgdt`'s entry point.

## Left to P29

**Stripping and the download's size.** The composed binary is mostly
DataFusion's `.text`, so a user wanting only `parse`, `info` or `query`
downloads all of it; that is accepted above. Whether the release profile sets
`strip` changes no timing, only the artifact, and is decided with the rest of
the packaging.
