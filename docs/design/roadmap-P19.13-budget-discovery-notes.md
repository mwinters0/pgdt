# P19.13 — the budget discovers the allocation

What the remaining slices inherit from `--parallel-memory`'s default becoming
the environment's answer instead of a constant. The spec row is
[`roadmap-P19-efficient-defaults.md`](roadmap-P19-efficient-defaults.md),
"Slices"; how the mechanism works now is
[`architecture.md`](architecture.md), "Execution model and API surface".

**The box is not ticked, and that is the spec's own ordering**: `19.15` runs
against this build before `19.13` lands, so the code is written first and the
box ticked second. Nothing is missing from the row — see "What `19.15` runs
against", below.

## What landed

- `io::discover_memory_limit()` — the `RT1`–`RT6` walk, with
  `discover_memory_limit_in(root)` beneath it so the v1 arm can be executed on
  a v2 machine.
- `io::available_memory()` — `RT8`'s `MemAvailable`, on the same root seam.
- `io::MEMORY_RESERVE` — 256 MiB, `19.12`'s constant.
- `Parallelism::discover()` and `Parallelism::discover_for(jobs, want)`, over
  `discover_in(root, …)`.
- `ByteRangeSource::default_memory_bytes()` — a defaulted `None`, overridden by
  `XzSource`.
- `ParallelArgs::resolve` asks for it whenever `--parallel-memory` is absent,
  exactly as it already asks for a count when `--jobs` is.

`MEMORY_RESERVE`, `discover_memory_limit` and `available_memory` are exported
from the crate root; `discover_for` is the entry point the CLI uses and
`discover()` is the no-source convenience the spec named.

## Three shapes to keep straight

**`discover()` is not what the CLI calls, and that is not an oversight.** The
spec names two primitives and one convenience; `discover_for` is that
convenience's general form, given recommendations a caller with an open source
already has, and `discover()` is `discover_for(available_parallelism(), None)`.
An embedder gets the sentence the spec promised — "the whole answer" — and the
CLI does not have to reimplement the reserve arithmetic to narrow it.

**The `Option` in the composition is the whole design, and it is easy to
flatten by accident.** `want.unwrap_or(DEFAULT).min(cap)` on the discovered
branch and `want.map(|w| w.min(available/2))` on the other are *different
shapes*, not two spellings of one: the second has no environment cap at all,
which is what stops an unlimited host from falling back to 64 MiB and making a
compressed scan serial. A refactor that unifies them into a single `min` over
an `Option<u64>` cap reintroduces exactly the defect the spec rejected.

**A budget of zero is a legitimate resolved value.** At or below a 256 MiB
limit `limit − MEMORY_RESERVE` is nothing, and the arrangement that produces is
one reader's worth on the streaming path, out of three floors that already
exist (`worker_count`'s `.max(1)`, `BufferPool::slots`' clamp to one,
`affordable` refusing block decode). `parse_parallel_memory` still refuses a
*stated* zero — a person who typed it meant something — and that asymmetry is
deliberate: this zero is arithmetic pgdq performed on a number the deployment
gave it.

## What `19.9` inherits

`19.9` is the test slice for this mechanism, and it is unblocked by this
landing rather than by anything else — its five deliverables all name something
that did not exist until now. Two of them changed shape here:

- **The status line's provenance is now the visible defect it was predicted to
  be.** A discovered budget prints bare, indistinguishable from a stated one,
  and `(default)` survives on exactly one arrangement: no flag, no limit found,
  and a source recommending nothing — a flagless plain scan on an unlimited
  host. `architecture.md`, "Status output", now says so, and names the
  three-way distinction the line is to gain.
- **The below-reserve arrangement is reachable and observed.** A `-m 256m`
  container prints `memory_bytes=0` today, with no note saying why. That is
  `19.9`'s `PlanNote`, and the number to key it on is `budget <
  what one slot needs` rather than anything about the limit.

**The fixture tree `19.9` owns is a committed one; this slice built temporary
roots.** `io.rs`'s tests construct a `FakeRoot` per case — `proc/self/cgroup`,
`proc/self/mountinfo`, `proc/meminfo` and a cgroup mount under one `tempdir` —
because landing the reader untested was the worse option. What that leaves
`19.9` is the *resolution* layer (`ParallelArgs::resolve` over a real source),
the status line, the `PlanNote`, and whichever of the committed-tree cases it
wants under `fixtures/` rather than inline; the unit cases here are the
reader's own and should stay wherever the reader is.

## What `19.15` runs against

`19.15`'s probe is a scratch build of this rule, and this *is* that build. Its
first job is the reserve's headroom through the 1.25–1.5 GiB band. Three
readings taken here as a smoke check, not as the probe — a 25 KB fixture, one
rep, no quiet machine — say the rule fires as designed and nothing more:

| container | plain | `.xz` (4 KiB blocks) |
|---|---|---|
| `-m 256m` | `jobs=1 memory_bytes=0` | `jobs=24 memory_bytes=0` |
| `-m 512m` | `jobs=1 memory_bytes=67108864` | `jobs=24 memory_bytes=268435456` |
| `-m 3g` | `jobs=1 memory_bytes=67108864` | `jobs=24 memory_bytes=403710912` |
| no limit | `jobs=1 memory_bytes=67108864 (default)` | `jobs=24 memory_bytes=403710912` |

The `.xz` recommendation there is `24 × 16.03 MiB`, which the 3 GiB and
no-limit legs both clear — so on this machine the *source* is what binds above
512 MiB, which is the arrangement `19.15`'s unlimited arm is built to
exercise without an unbounded run.

## Calls the spec did not decide

- **`default_memory_bytes` is charged at the pool's current slot size**, which
  before any read loop has announced one is `POOL_MAX_BYTES` (8 MiB) rather
  than the 1 MiB a scan settles at — so the recommendation runs about a tenth
  high. It is the same number `block_decode_bytes` answers, deliberately: one
  statement of what a reader holds. Erring high asks for budget the worker
  count will not spend, and every byte above `jobs × per-reader` is
  structurally inert.
- **The recommendation is the source's own count, not the resolved one.** A
  stated `--jobs 4` on a 24-core host still gets a 24-reader budget. The budget
  is a bound rather than a target and the resolved count is what spends it, so
  the alternative buys nothing and would make the source's answer depend on a
  flag it is meant to be independent of.
- **v1's mount is located through `/proc/self/mountinfo`; v2's is the
  `/sys/fs/cgroup` convention.** That is what `std` does for the CPU quota, for
  the same reason — v1 mount points genuinely vary and v2's does not — and the
  v1 arm falls back to `/sys/fs/cgroup/memory` where the file lists no such
  mount, which is also what a fixture root stating only a cgroup path gets.
  `\040`-escaped mount points are not unescaped, which is `std`'s stated limit
  too.
- **`NO_LIMIT_AT_OR_ABOVE` is 4 TiB.** `RT4` says to read v1's "unlimited" as a
  threshold rather than an equality, because the value is a function of the
  page size and the word width; the *smallest* of the four shapes it takes is
  8796093018112 (32-bit), so the threshold has to sit under that. It is applied
  to the v2 files too, where only a stated limit above 4 TiB could reach it —
  and reading such a limit as unlimited falls back to the `MemAvailable` cap,
  which on such a machine is the smaller number anyway.
- **The manual moved here rather than at `19.10`.** Eight sentences went false
  the moment the default stopped being 64 MiB, and a falsified claim is
  corrected by the change that falsifies it
  ([`../process.md`](../process.md), "Where does this fact go?"). What moved:
  the flag's opening sentence, the "two ordinary ways of compressing produce
  blocks too large for the default budget" paragraph, four "at the 64 MiB
  default" phrasings that are now "under a 64 MiB budget", the koji
  transcript's `memory_bytes` and the paragraph reading it back, and the
  cgroup-sizing advice, which now says the reserve is already left for a
  flagless run. `19.10` still owns the `MALLOC_ARENA_MAX` recommendation as
  `M76` leaves it, both flags' help text as a whole, and the moved
  whole-block-decode threshold.

## The harness is unaffected, and here is why to check that again

Two registered command shapes state no `--parallel-memory`: `peak-rss`'s
`control` row and the `reserve` figure's `control` leg, both `--jobs 1` on a
plain file, both there precisely to read *the shipped default*. Under
discovery that value is now `min(64 MiB, limit − 256 MiB)`, and both run in a
container of 3 GiB or more — so both still resolve to 64 MiB and the harness's
emitted sentence ("with no budget stated runs at the library's 64 MiB default")
stays true. It stops being true below a limit of about 320 MiB. A future
change that shrinks either container is what to watch: the apparatus rule pins
a worker count on every shape, and after this slice the *budget* on those two
shapes is a property of the container as well.

## The one consequence worth a second look

A flagless `.xz` run on a host with no memory limit now asks for
`jobs × per-reader`, and *per-reader scales with the file's block size*. On a
24 MiB-block dump that is ~1.4 GiB, which is the number the spec worked
through. On a file written by `xz -9 -T0`, whose blocks are ~192 MiB, it is
~9.6 GiB — under the `MemAvailable` half-cap on this machine, so the cap does
not bind, and the block path is then *afforded* where the 64 MiB constant
declined it. That is the intended posture and the intended reversal, but no
reading covers it and `19.15`'s containers are all far below it. Filed under
STATUS's "Decisions worth another look".
