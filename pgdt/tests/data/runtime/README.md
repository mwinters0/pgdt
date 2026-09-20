# Runtime fixture roots

Filesystem trees shaped like a Linux one, for driving `pgdt`'s memory-limit
discovery against environments this machine cannot be put into. Each directory
is a **root**: `pgdump_query::discover_memory_limit_in`,
`available_memory_in` and `Parallelism::discover_in` join every path they read
onto it, so a root of `/` is the real reading and one of these is a tree
somebody built (`docs/design/runtime-invariants.md`, `RT1`–`RT8`).

They are committed rather than built in a `tempdir` because what they pin is a
*resolution* — what a flagless `pgdt` ends up running — and the arms that
matter are the ones no single machine has: this project's machines run a pure
v2 unified hierarchy, so the v1 arm cannot exist beside it (`RT6`), and a
machine with a limit cannot exercise the no-limit arm at the same time. The
reader's own unit cases stay beside the reader, in `pgdump_query/src/io.rs`.

What a tree establishes is that the reader handles the shape the register
describes — not that a kernel still produces it, which only a host of that kind
can say.

| Root | What it states |
|---|---|
| `v2-limit/` | cgroup v2, a 1 GiB `memory.max` at the leaf |
| `v1-limit/` | cgroup v1 (`RT4`), a 512 MiB `memory.limit_in_bytes`, its mount located through `mountinfo` |
| `no-limit/` | every limit file `max`, and ~19 GiB of `MemAvailable` — the roomy unlimited host |
| `cramped/` | no limit, and 512 MiB of `MemAvailable` — where the `RT8` half-cap binds |
| `below-reserve/` | a 256 MiB allocation, which `MEMORY_RESERVE` leaves nothing of |
