# P13 inbox — facts filed for its grilling

Evidence P13 (compressed input) needs before its remaining decisions can be
grilled. **This is a queue, not a document**: walk every entry, fold it into
[`roadmap-P13-compressed-input.md`](roadmap-P13-compressed-input.md) or discard
it as stale, and delete this file. See `docs/process.md`, "Inboxes: facts filed
by destination".

**This inbox is unusual in two ways, and both are deliberate.** P13 already has
a spec, where the threshold for an inbox is normally a phase with none — the
justification is that its *remaining* decisions are ungrilled, so there is still
a grilling for an inbox to be drained by. And its first inbox was drained and
deleted when the spec was written; this is a second one, filed after the phase
was specified and while it was blocked.

**The load-bearing entry is the fourth.** This phase is now the place where the
compressed/remote integration story gets settled for four phases at once, and
three of those four are ungrilled. Read P14's, P15's and P16's inboxes as part
of this grilling — not to drain them, which belongs to their own grillings, but
because the evidence for decisions this phase is about to make is filed there.

---

## P13 is not blocked, and the spec's "Blocked" section is false

**Fact.** [`roadmap-P13-compressed-input.md`](roadmap-P13-compressed-input.md),
"Blocked: the seekable-xz layer is not a library that exists", rests on a survey
concluding that nothing answers a positioned read over an `.xz` file. `xz-seek`
now does. It ships `Reader::read_at`, held to `xz -dc`'s own output over 531,684
`(offset, len)` pairs on its fixture corpus under every backend it compiles, and
confirmed over both koji files including `xz -dc | cmp` across all 730 GiB the
40 GB file decodes to. The crate lives at the path `CLAUDE.local.md` records.

The three properties this phase's decisions need are all present: the exact
uncompressed length from the stream index (D1); a `SeekTable` that is a plain
value with public fields, handed out and taken back with no re-walk, behind a
`serde` feature (D5); and compressed bytes arriving through a caller-supplied
trait rather than a file the crate opens (D3's composition, and P14's).

**This phase needs serial decode only.** D6 is one streaming decoder restarted
on seek, and it hands parallel decode to P16 explicitly — so `xz-seek`'s own
`P3` (parallel block decode) is *not* on this phase's critical path. Its
consumer is P16.

**Why this phase cares.** The section has to be struck, and the roadmap index
row's caption with it, in whatever change first acts on this. Leaving a false
"this phase cannot proceed" in the spec is worse than a stale citation: it is
the sentence a later session would stop at.

**Origin.** 2026-09-06
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md), "P13's
blocker is discharged: a positioned read over `.xz` exists").

---

## Four answers already given to `xz-seek` bind this phase's design

**Fact.** `xz-seek`'s `P3` grilling put eleven questions to this project as its
first downstream, and they were answered as `pgdump_query`. Four of the answers
constrain *this* phase rather than a later one, and they are commitments already
made, not options still open:

- **Delivery is repeated fill** — `read(&mut buf) -> Result<usize>`,
  fill-or-EOF, caller loops. It is what the three read loops already are
  (`scan.rs:548`, `stream.rs:521`, `stream.rs:1327`). An iterator of owned
  chunks was refused because it reintroduces the per-chunk `calloc` the buffer
  pool exists to remove — 23.2% of a warm `parse`'s user time — and because the
  query path retains a delivered buffer as an Arrow `Buffer` that rows take
  string views into, so a 24–128 MiB owned block would be held behind a batch
  addressing a few kilobytes of it.
- **The boundary is blocking, wrapped here**, matching `io::LocalFileSource`,
  which is already `spawn_blocking` around `read_exact_at`. An async API inside
  `xz-seek` was refused outright: it would put an executor in the graph of a
  crate whose distinguishing claim is a four-crate unsafe-free build, and it
  would fix a runtime choice this project leaves to the binary.
- **Verify-before-return is the bulk path's default.** A scan persists the
  structure it discovers into the cache as it goes, so bytes whose check fails
  two calls later have already been recorded as fact and nothing re-examines
  them. The crate's *seeking* path keeps the weaker default, which matches
  `xz -dc`. On failure: bytes valid up to the fault, with the **uncompressed**
  offset, because that is the coordinate space this project's whole index lives
  in.
- **`SeekTable` gains `blocks_in(range)`.** D2 already commits this phase to
  announcing a non-seekable container when it is discovered rather than letting
  a user meet it as a stall; the block count over a range is that warning with a
  number in it and the same remedy attached (`xz -T0`, or `--block-size`).

**Why this phase cares.** These are decisions about this phase's own boundary,
made outside its spec, and the spec does not record them. Fold each into the
decision list rather than re-deriving it — and note that the third one, the
verify default, is the only place where this project asked another project to
change a standing decision, so it is the one to re-check has actually landed
before relying on it.

The full answers are machine-local and uncommitted, beside `xz-seek`'s
questions; `CLAUDE.local.md` names the directory. They are written to be
readable without this project's roadmap, so they restate the reasoning in
generic terms rather than citing phases.

**Origin.** 2026-09-06
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md), "What
`xz-seek` was told, and what it commits us to"). **Contingent on** `xz-seek`'s
`P3` landing as specified; the answers were given, nothing is built against
them.

---

## The dependency is a frozen in-repo copy until this project has integrated

**Fact.** Settled by the maintainer. `xz-seek` is **not published until this
repo has integrated against it** — this project is its first real-world
consumer, and the interface is vetted by being used rather than frozen into a
published version and then discovered. Publication and a proper versioned
dependency come after both this phase and P10 are complete with the shape
questions resolved. The crate's own manifest carries `publish = false` today.

**The mechanism is a frozen copy inside this repo, committed**, not a path
dependency pointing at the sibling tree. The distinction is load-bearing: a live
path dep breaks `cargo test --workspace` on any other checkout at resolve time
— verified, an optional path dependency behind an off-by-default feature still
fails with `failed to read …/Cargo.toml`, because the graph is resolved before
features are considered — and it also couples this build to the state of their
working tree. A committed copy keeps this repo self-contained and immune to
their churn, which is the whole point of the arrangement.

Five mechanics, each found by inspection rather than assumed:

- **It is a transformation, not a copy.** `xz-seek`'s manifest carries a
  `[workspace]` table and dev-dependencies that path-depend on its own workspace
  members (`fixtures-gen`, `harness`) plus a self-dependency used to force
  feature combinations into `cargo test`. None of that resolves when dropped in.
  A usable copy is `src/` (292K) plus a trimmed manifest and the licences —
  **not** `tests/`, not `fixtures/` (5.5M), not the workspace table, not
  `[dev-dependencies]`.
- **Our workspace needs `exclude`** for the vendor directory, or `cargo test
  --workspace` tries to run its suite without the fixtures it needs.
- **Make the copy a script**, so the re-sync when `P3` lands is one command
  rather than a hand-merge — the transformation above has to be repeated
  identically or the diff is unreadable.
- **Stamp it with the source commit**, on the precedent of
  `runs/pgdq-nocensus.stamp`, which names the commit its binary was built from
  and refuses to be measured against another. The snapshot point as of
  2026-09-06 is `bf26bf0`.
- **The copy is read-only.** A bug found here goes upstream and returns at the
  next sync; it is never patched in place. That is what keeps a snapshot from
  becoming a fork, and it is the answer to the objection that vendoring creates
  a second authority over that source.

**One build-story decision this forces.** `xz-seek`'s default features pull
`liblzma`, which compiles vendored C, where this workspace is pure Rust today.
The alternative is developing against `xz4rust` — pure Rust, unsafe-free,
roughly 2.2× slower. Since this phase's real figures cannot be taken until
proper integration anyway, either serves; the argument for keeping `liblzma` is
that the integration being vetted is then the one that ships.

**Why this phase cares.** It is this phase that takes the dependency, and none
of the above is in the spec. It also bounds what this phase may promise: no
published crate, so no version to pin, and a window in which the build has a
prerequisite outside the repo's own history.

**Origin.** 2026-09-06
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md),
"`xz-seek` is not published until this repo has integrated against it").

---

## Three trait and cache decisions this phase should make for four phases, not one

**Fact.** `ByteRangeSource` and `SourceIdentity` are about to be edited by four
phases in sequence, each taking one bite:

| Phase | What it wants |
|---|---|
| **P13** | dyn-compat (D3); `stored_size()` (D4); identity records `stored_size` rather than `size`; `total_size` leaves `identity.size`; `FORMAT_VERSION` bump |
| **P15** | `size()` stops being exact — neither gzip nor zstd can answer it |
| **P14** | identity is an **ETag**, not a `SystemTime`; cancellation and timeouts; `hint_read_size` on a buffer-recycling remote source |
| **P16** | buffer pool per-worker against per-source; possibly a range announcement |

`SourceIdentity` today is a closed `struct { size: u64, mtime: Option<(u64, u32)> }`
at `FORMAT_VERSION` 15 (`cache.rs:57`, `cache.rs:62`). This phase rewrites it
and bumps; P14 rewrites it again for the ETag and bumps again. That is two
cache-format breaks where one would do, and four cross-cutting rewrites of every
caller where two would.

**This phase is the only one of the four that is specified, and it is already
touching both.** So three items are cheapest here and expensive anywhere else:

1. **Make identity opaque while D4 is already rewriting it.** An ETag does not
   fit `(size, mtime)`. If identity becomes a token with variants at D4's
   moment, P14 adds one instead of breaking the format — and this phase is
   paying the version bump regardless.
2. **Re-open D1's deferral of `size()` exactness.** D1 rejected relaxing the
   contract on the ground that pre-spending buys this phase nothing but a weaker
   `info`. That was right in isolation and is weaker with P15 in the same
   conversation: D3 is already rewriting every signature, so adding an exactness
   *signal* is one pass, where P15 doing it later re-touches every caller a
   second time. It costs `info` nothing — xz still answers exactly, and the
   signal only lets `info` say "43% of at least 730 GiB" where a codec genuinely
   cannot do better. P15's inbox holds the measurement that every read loop
   already tolerates a short read.
3. **Settle D5's envelope field against gzip, not only against xz.** An xz entry
   is `(uncompressed_start, compressed_start)`, ~31,150 of them for the koji
   multistream file; a gzip checkpoint carries a **32 KiB dictionary window
   each**. Same field, three orders of magnitude apart and a different kind of
   thing — a stateful checkpoint rather than an independently decodable block.
   P15's inbox already flags that this field's naming must not assume xz's
   shape.

**Why this phase cares.** It is the integration story, and this is where it gets
settled. Note what it means procedurally: these decisions bind three phases
nobody has grilled, which is the class of call `docs/process.md`'s "Working
unattended" says escalates to the maintainer — so this grilling wants them in
the room, and it wants P14's, P15's and P16's inboxes read first.

**Origin.** 2026-09-06, reading P14's, P15's and P16's inboxes against this
phase's spec and against `cache.rs`
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md), "What
`xz-seek` was told, and what it commits us to").

---

## The scoping rule that keeps the codec × transport matrix from being eight cells

**Fact.** The obvious reading of P13 + P14 + P15 is a grid — three codecs by two
transports — and it is not one, for a reason that only appears when P15's actual
target is read rather than its title. **P15's primary case has no addressing
layer at all**: `pg_dump` writes plain-format compression *streaming*,
single-stream and non-seekable, and BGZF and `t2sz`'s zstd seekable format are
its secondary cases. So there is no family of seekable codec crates to align
interfaces across — there is one such crate, and generalizing it would be
abstracting over a single instance. `xz-seek` already declined to generalize, on
the narrower ground that the exact-size guarantee is load-bearing in its
interface.

What generalizes instead is **D2**: read every shape, warn about the degraded
one, never refuse. Remote non-seekable gzip cannot be random-accessed without
streaming the whole file once, and a checkpoint index for it at koji scale is
32 KiB × N. So the honest scope across the grid is that `parse` works
everywhere, `query` warns and decodes from zero where the container will not
seek, and nobody builds a remote gzip index until someone asks — which reduces
the whole matrix to **one `ByteRangeSource` wrapper per codec**, each warning
where it is degraded. That is what this phase's "Layering" section already says,
arrived at from the other direction.

**Why this phase cares.** D2 was written as a decision about xz's third shape.
This says it is the phase's most reusable output, and that stating it as a
general rule here is what stops P14 and P15 from re-deciding it twice — or
worse, from scoping work for cells that should be declared degraded instead of
built.

**Origin.** 2026-09-06, reading
[`roadmap.md`](roadmap.md), "P15 — gzip and zstd input" and "P14 — Remote input"
against this phase's D2.
