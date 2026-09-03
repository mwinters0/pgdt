# Contributing

Setting a machine up to work on `pgdump_query`. What each command *does* is
[`CLAUDE.md`](CLAUDE.md)'s command reference, which this file does not repeat;
what the system *is* is [`docs/design/architecture.md`](docs/design/architecture.md).

## Building and testing

A Rust toolchain and `cargo test --workspace` are the whole story for the
library and the CLI. The Python-side checks under `scripts/` — the fixture
generator, the oracles, the measurement harness — run under `uv`, from that
directory. [`CLAUDE.md`](CLAUDE.md) lists every one of them with its purpose.

## Profiling

**Detached debug symbols for libc are a requirement, not a nicety.** Without
them roughly half of a warm `parse` profile arrives as bare addresses inside
`libc.so.6` — and those addresses are `__memmove_avx_unaligned_erms` and
`__memset_avx2_unaligned_erms`, which are exactly the two functions the
performance work exists to look at. A profile taken without them looks
perfectly plausible and is missing its largest bucket, so this is worth
checking before reading any profile rather than after.

Most distributions ship libc stripped and put the symbols in a separate
package, often in a repository that is not enabled by default. Install that
package: symbols under `/usr/lib/debug` are found through the `.gnu_debuglink`
already present in `libc.so.6`, needing no network, no cache and no environment
variable at profile time, and the package manager keeps them in lockstep with
libc as it upgrades.

Two details generalise beyond any one distribution, because both cost an hour
to rediscover:

- **The debug repository is often not on the general mirrors.** Enabling it by
  including your usual mirror list can fail with a flat 404 from every mirror
  in it. Point the debug repository at a server you have checked actually
  serves it.
- **The package must match the installed libc exactly.** These are keyed by
  build ID, so a version skew does not warn — it silently resolves nothing, and
  you are back to bare addresses.

On Arch, where this project is developed, that is `glibc-debug` from the
`core-debug` repository; one package covers `libc`, `libm` and `ld-linux`
together. `geo.mirror.pkgbuild.com` serves `core-debug` and `extra-debug`,
while many mirrors do not and the `built.archlinux.org` host named in the wiki
may not resolve at all.

To confirm it worked, take any profile with your `perf` build-id cache
(`~/.debug`) moved aside, and check that libc frames come back named. Moving it
aside matters: that cache may already hold symbols from an earlier fetch, so a
profile taken with it in place cannot tell you whether the package is being
used.

### When no package exists

Where the distribution offers none, or for a DSO no package covers, `perf` can
fetch symbols by build ID from a `debuginfod` server:

```sh
perf buildid-cache --debuginfod=<url> -a /path/to/libc.so.6
```

The flag lives on `buildid-cache` alone — `perf` top-level and `perf report`
both reject it. This is a fallback: it needs the network, it caches per user,
and nothing keeps it in step with a libc upgrade.

### Taking a profile

`cd scripts && uv run measure.py --profile-recipe` prints the whole sequence
with every path filled in, and runs none of it. Read
[`CLAUDE.md`](CLAUDE.md)'s "A profile is not a figure" before treating anything
it produces as a measurement.

## Machine-specific setup

Paths, volumes, sample datasets and local services differ per machine and are
deliberately not in this file or in `CLAUDE.md`. On a machine already set up
for this project they are in `CLAUDE.local.md`, which is gitignored; on a new
one, that file is what you write as you go.
