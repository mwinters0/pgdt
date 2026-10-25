# Contributing

Setting a machine up to work on `pgdump_query`. What each command *does* is
[`CLAUDE.md`](CLAUDE.md)'s command reference, which this file does not repeat;
the decisions the code cannot explain are [`docs/design/decisions.md`](docs/design/decisions.md); the code is the ground truth for the rest.

## Building and testing

A Rust toolchain and `cargo test --workspace` are the whole story for the
library and the CLI, under the Rust release `rust-toolchain.toml` pins, which
`rustup` installs on first use. `cargo nextest run --workspace`, the one `mise.toml` pins,
runs the same tests a process each and several at once, and leaves the
doctests to `cargo test --workspace --doc`. The Python-side checks under `scripts/` — the fixture
generator, the oracles, the measurement harness — run under `uv`, from that
directory. [`CLAUDE.md`](CLAUDE.md) lists every one of them with its purpose.

**`mise run check`, the round's checks, needs a container runtime as well**:
it runs its cargo checks in the release image, `release/Dockerfile`, under a
memory limit (`scripts/check.py`), so the runtime must build and run that
image — `sudo nerdctl` unless `PGDT_RELEASE_CONTAINER` names another, such as
`docker`.

**Taking a figure needs two things more**: a container runtime that can build
the release image, `release/Dockerfile`, in which the harness builds every
binary it times, and run that image's base, which every figure runs in
(`sudo nerdctl` unless `PGDT_MEASURE_CONTAINER` names another); and the Rust
target the resident-set helper `peak-rss` and the instruction counter
`count-instructions` are built static for, `rustup target add
<machine>-unknown-linux-musl`, which the toolchain file already lists for an
x86-64 host. A sweep's preflight builds the helper and starts it and `pgdt` in
the pinned image before the first reading, refusing with the command to run
where the target is missing; a sweep taking `release-profile` also counts
`/bin/true` there under `CAP_PERFMON`, refusing where the kernel or the
runtime will not let the counter open.

**One test file is opt-in**, and it is the only thing a full run leaves
unexercised: `crates/pgdt/tests/http_conformance.rs` reads a dump over HTTP from a
**real** static origin rather than from the misbehaving one the other remote
tests build in-process, which is what says whether that in-process server
encodes our own misreading of HTTP. Serve `fixtures/` from any static server
that answers **ranged GETs** — `nginx`, `caddy file-server`, an object store's
public URL — and point the run at the one object it reads:

```sh
PGDT_HTTP_CONFORMANCE_URL=http://127.0.0.1:8091/fixtures/16/edge_cases/default.sql \
  cargo test -p pgdt --test http_conformance
```

Unset, those tests pass having done nothing, which is why CI and every other
checkout need no server. Set, nothing is skipped: an unreachable URL, an object
that is not that fixture byte for byte, and a server that ignores `Range` each
fail by name. Python's `http.server` is one of the last — it answers `200` to a
`Range` request and serves the whole file.

**A second variable reaches a compressed object**, whose footer walk is a
backward chain of very small ranged GETs down one connection — the request
cadence the in-process oracle can least stand in for, since it closes every
connection it answers. There is no committed `.xz` to point at and there cannot
be one: `xz` output is not reproducible across versions, so what the tests
compare is the **plaintext** the reader hands back, and the object is one you
make. It must decompress to that same fixture and be more than one stream of
more than one block, which is what gives the walk a per-stream cost and the
reader something to seek in:

```sh
f=fixtures/16/edge_cases/default.sql
half=$(( $(wc -c < "$f") / 2 ))
{ head -c "$half" "$f" | xz --block-size=512 -c
  tail -c +$((half + 1)) "$f" | xz --block-size=512 -c
} > /srv/www/default.sql.xz     # wherever the server serves from

PGDT_HTTP_CONFORMANCE_XZ_URL=http://127.0.0.1:8091/default.sql.xz \
  cargo test -p pgdt --test http_conformance
```

The two variables are independent: either, both or neither may be set, and
each turns on the tests that need it. An object under the compressed one that
is not `.xz`, holds another dump, or has only one stream or one block fails by
name like everything else here.

## Cutting a release

`/release` (`.claude/skills/release/SKILL.md`) prepares one on the maintainer's
machine and stops before anything leaves it; the push, the `release-build`
dispatch and the annotated tag are the maintainer's own. It needs what taking a
figure does — a container runtime that can build the release image — and
network access: `scripts/debian_window.py` fetches Debian's release days to
check the image's pin is inside the window a release allows, and `gh`,
authenticated for the repository, dispatches the build and watches for the
draft. A release is verified from outside as a user does it
([installing](docs/manual/installing.md), "Verify").

## Profiling

**Every profile, recording and probe runs in the release image under a memory
limit** (`scripts/contained.py`, [`measurements.md`](docs/design/measurements.md),
"The apparatus"), and the image carries what one needs: `perf`, `heaptrack`,
`strace`, and `libc6-dbg`, which apt holds to the image's own libc. So nothing
is installed on the host. **Those symbols are a requirement, not a nicety**:
without them roughly half of a warm `parse` profile arrives as bare addresses
inside `libc.so.6`, and those are `__memmove_avx_unaligned_erms` and
`__memset_avx2_unaligned_erms`, exactly the two functions the performance work
exists to look at. A report read outside the image, against another libc,
resolves none of them and still looks plausible.

The binary profiled is built in the image too (`release.py build`), so it runs
on the libc its symbols describe; a host build may need a newer glibc than the
image's. `perf` needs no sysctl where `perf_event_paranoid` is 2 or less: that
permits user-space sampling of one's own processes, which the container's
unconfined seccomp lets through.

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
