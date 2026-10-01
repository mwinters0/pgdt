# P30 inbox — facts filed for its grilling

Evidence P30 (one binary for distribution) will need. **This is a queue, not a
document**: when P30 is grilled, walk every entry, fold it into the spec or
discard it as stale, and delete this file. See `docs/process.md`, "Inboxes:
facts filed by destination".

---

## The split is a recorded rejection, made to keep DataFusion out of the timed build

**Fact.** `datafusion-cli-pgdump/Cargo.toml`'s header rejects a `pgdt sql`
subcommand "which would put DataFusion into the build every figure in
`docs/design/measurements.md` times". The timed build is `cargo build
--release -p pgdt` with default features ([`measurements.md`](measurements.md),
"The apparatus"). `scripts/check.py` builds, lints and tests `--workspace` at
default features, so a non-default feature is compiled by nothing that runs
each round.

**Why P30 cares.** A default-off `pgdt` feature answers the rejection's letter:
the timed build is unchanged, and the release build enables the feature. It
also makes the shipped `pgdt parse` a build no figure times, since it links
DataFusion and possibly another allocator. The grilling decides whether that
is acceptable, measured once, or refused. The feature combination also needs a
place in the round's checks, or it rots unbuilt.

**Origin.** Filed 2026-10-01 by the session sketching P30.

---

## A process has one global allocator; mimalloc is priced on time, not on resident

**Fact.** `pgdt` links the platform allocator. mimalloc was refused as its
default because a few percent was not worth carrying a non-default, not
because it was unsuitable ([`decisions.md`](decisions.md), "D13"). The
`allocator` figure reads it level or marginally ahead on every shape, and
builds and runs a `pgdt` leg on it at every sitting
([`measurements.md`](measurements.md), "Which allocator a figure was taken
under"). `datafusion-cli-pgdump` links mimalloc, as upstream's
`datafusion-cli` does, and every provider figure is taken on it
([`measurements.md`](measurements.md), "What DataFusion's dynamic filters buy
a query"). Three things are glibc-only:
- That figure is wall time only. The constants a flagless run carves its
  allowance with were fitted from glibc readings
  ([`decisions.md`](decisions.md), "D3"; `reserve`).
- The introspection build is refused beside mimalloc (`pgdt/src/alloc.rs`), so
  attribution sees only a platform-allocator build.
- C dependencies (`liblzma`, `aws-lc`) allocate through libc whatever Rust's
  global allocator is.

**Why P30 cares.** The composed binary gets exactly one allocator. mimalloc is
the natural choice for a binary carrying DataFusion and is open on D13's
terms. What it owes before shipping is the `reserve` figure re-taken under it,
an under-reserve being the error D3 says kills the process. The platform
allocator instead leaves `pgdt df` a DataFusion CLI nobody has measured.
Either way, `--version`'s allocator marker moves, and `measure.py` reads it.
Whichever crate becomes the library half gives up its `#[global_allocator]`
(D13), leaving it in a binary's `main.rs`.

**Origin.** Filed 2026-10-01 by the session sketching P30.

---

## The two `main`s want different runtimes and set the process up in a different order

**Fact.** `pgdt`'s `main` is `#[tokio::main(flavor = "current_thread")]`,
justified in its doc comment by D12. It installs `InitShutdown` and the tracing
subscriber *before* `Cli::parse`. The copied `main` is `#[tokio::main]`
(multi-threaded, DataFusion's), and it calls `env_logger::init`, then
`Args::parse`, then `pgdump::end_as_namespace_init(args.repl_mode())`, which
needs the parsed arguments ([`decisions.md`](decisions.md), "D26"). Tokio
refuses to start a runtime from inside another.

**Why P30 cares.** The composed `main` cannot be either attribute. It parses
first, then builds the runtime and the signal and logging setup the chosen arm
needs. That moves `pgdt`'s handler installation after argument parsing, and
the grilling should confirm D26 is indifferent to that.

**Origin.** Filed 2026-10-01 by the session sketching P30.

---

## clap composes the CLIs without restating a flag; the upstream copy grows a little

**Fact.** `#[derive(Parser)]` on a struct also derives `clap::Args`
(`clap_derive` 4.6.4, `src/derives/parser.rs`, `gen_for_struct`). So
`datafusion-cli-pgdump`'s `Args` can be a variant of `pgdt`'s `Command` as it
stands, its flags, help and validation parsers all reused. The copied
`main_inner` parses its own arguments. Exposing it as `run(args)` from a
library target, with `Args` made public, adds a few `pgdump:`-marked lines to
the copy that are re-applied at each DataFusion major (the header of
`datafusion-cli-pgdump/src/main.rs`). The copy also prints `DataFusion CLI
v<version>` unless `--quiet`, and declares `#[clap(author, version, about)]`.

**Why P30 cares.** This is what makes "no extensive duplication" true: one
enum variant and one dispatch arm in `pgdt`, plus the library split. The
subcommand's name (`df`, `sql`, …), what `pgdt --version` says about the
DataFusion inside it, and whether the banner stays are this phase's to decide.

**Origin.** Filed 2026-10-01 by the session sketching P30.

---

## The reserve an allocator would be priced against is already known short, and P23 owns it

**Fact.** `MEMORY_RESERVE` does not cover what a compressed `query` holds above
its charge and statistics account, and wide `.xz` queries were OOM-killed under
it (`KD34`). Setting it is P23's, which is sketched after P21 and P22 because
the architecture it answers to is still moving ([`roadmap.md`](roadmap.md),
"P23 — Statistics coverage and the resident reserve"). Scheduling P30 and P29
first means the public binaries carry `KD34`.

**Why P30 cares.** Re-taking `reserve` under mimalloc prices an allocator
against a constant already known wrong, so the reading cannot settle whether
mimalloc is safe to ship. The grilling chooses among: the platform allocator,
leaving `KD34` exactly as it is today for P23; mimalloc with a re-take read
only as a comparison against the platform leg's same sitting; or pulling
`KD34` forward out of P23. Whether a beta's release doc states the workaround
(a `--memory` under the container's limit) is P29's.

**Origin.** Filed 2026-10-01, when P30 and P29 were scheduled ahead of P22.

---

## DataFusion is most of the composed binary's size

**Fact.** The release `datafusion-cli-pgdump` binary is over an order of
magnitude larger than the release `pgdt`, and its `.text` is mostly
DataFusion's. Re-check with `ls -l target/release/{pgdt,datafusion-cli-pgdump}`
and `size -A` on each. Both keep their symbol tables, the release profile
setting no `strip`.

**Why P30 cares.** A user who only wants `parse`, `info` or `query` downloads
all of it. Whether a release ships the composed binary alone or a slim `pgdt`
beside it, and whether the release profile strips, are this phase's call (or
P29's, if they are grilled together).

**Origin.** Filed 2026-10-01 by the session sketching P30. Sizes read from that
day's `target/release`. A size is a measured quantity, and any figure taken
for the decision goes in `measurements.md`.
