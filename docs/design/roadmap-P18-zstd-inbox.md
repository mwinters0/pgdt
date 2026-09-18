# P18 inbox — facts filed for its grilling

Evidence that P18 (zstd and lz4 input) will need. **This is a queue, not a
document**: when P18 is grilled, walk every entry, fold it into the spec or
discard it as stale, and delete this file. See `docs/process.md`, "Inboxes:
facts filed by destination".

These three entries were filed for the compressed-input work when gzip and zstd
were one phase, and they bear on both halves; P15's inbox carries them in its
own terms.

---

## Relaxing `size()` is cheaper than it looks, and the `.xz` source deliberately did not spend it

**Fact.** `ByteRangeSource::size()` still means the exact addressable
length, because xz answers it exactly from its stream index. The alternative was
weighed first: every read loop in the library is already written as
`want = chunk_size.min(size - read_pos)` … `read_pos += bytes.len()`, so **all
of them already tolerate a short read**. What actually depends on the number
being exact is the loop's exit test, and the coverage denominator —
`total_size`, `scanned_through`, and `pgdq info`'s percentages.

**Why P18 cares.** This is the decision this phase cannot avoid, since zstd's
frame content size is optional and a streaming writer — which is what
`pg_dump --compress=zstd` is — omits it. So the choice is a first pass to
compute the size, or a relaxed contract, and the relaxation's cost is now known
to be a loop-termination change plus an honest answer for `info` when the total
is not yet known, rather than a rewrite of every caller. **Contingent on P15:**
it meets the same decision in gzip's form, so if it runs first the contract is
already settled and this phase inherits it rather than deciding it.

**Origin.** 2026-09-02, grilling the compressed-input work; the reasoning is
now beside the mechanism ([`decisions.md`](decisions.md), "The compressed source and the cache", which states what `size()` promises and the two relaxations refused).

---

## The seekable-xz crate is xz-only on purpose, and generalizing it was rejected

**Fact.** The external crate the `.xz` source reads through
(`/mnt/wd12t/fedora/experiments/xz-seek/docs/design/historical/initial.md`) puts
gzip and zstd explicitly out of scope. The reason is the entry above: the exact
uncompressed size from the container's own footer is load-bearing in that
crate's interface (its R3), and a crate that treats three codecs as one shape
gets the size contract wrong for all three.

**Why P18 cares.** This phase will want an addressing layer for zstd's seekable
format, and the natural instinct will be to extend the xz crate — or, by then,
whatever P15 built for gzip. The interface is reusable; the size guarantee is
not. Whether that becomes a sibling crate, a generalized trait, or something
this repo owns is this phase's to decide — but it decides it knowing the xz
crate declined the generalization deliberately rather than by oversight.

**Origin.** 2026-09-02, grilling the compressed-input work.

---

## The decisions the `.xz` source already made, which this phase inherits

**Fact.** Four decisions bind any later decompressing source, and all four are
built ([`decisions.md`](decisions.md), "The compressed source and the cache" and "The compressed source and the cache"): `stored_size()` on the trait with
`SourceIdentity` recording it, so the staleness check stays a `stat`; the seek
table living in the cache envelope as a sibling of `ContainerKind`, which stays
`Plain` because the span offsets genuinely are plain-format offsets; one
streaming decoder behind a mutex, restarted on seek, with no retained decoded
output; and a non-seekable file being warned about rather than refused.

**Why P18 cares.** A `.zst` source is the same shape of object and should not
re-decide any of them. `CompressionIndex` is an enum from its first commit with
one variant in it, so this phase adds a sibling variant of its own layout — the
seekable format's frame index — rather than reshaping the field. If P15 has
already added variants of its own, the question this phase inherits is whether
they generalize or stay per-codec, which is a naming and layout question rather
than an envelope one.

**Origin.** 2026-09-02, grilling the compressed-input work.

---

## The budget rule's two constants are both xz-derived, and one of them predicts the resident of every source that states a cost

**Fact.** `io::MEMORY_RESERVE` (384 MiB, the cap a discovered limit hands back)
and `io::MEMORY_UNPOOLED_BOUND` (256 MiB, what `margin_allowance` predicts a
count's resident with) were both read off `.xz` inputs alone, at 24 MiB and
128 MiB block sizes ([`decisions.md`](decisions.md), "I/O, memory and parallelism"). The second bounds what a scan holds *outside* what
`WorkerMemory::at` bills: its worst over the 400-run grid the reserve was read
off is 238.6 MiB at 24 MiB blocks and 142.0 MiB at 128, and the published
`reserve` sitting reads 4.7–179.5 MiB — one codec at two decode units, and no
reading on any other.

**Why this phase cares.** A new decompressing source states its own
`default_worker_memory`, which is the per-worker term — and nothing obliges it
to look at the constant its resident is then *predicted* against. The two are
separate numbers in separate places, so a source whose decoder retains more per
unit than `liblzma` does gets a correct charge and an optimistic prediction, and
the failure surfaces as a cgroup kill at a large limit rather than as an error.
So the phase either argues the bound carries over — the term is glibc arena
retention as far as any reading goes, which is not codec-specific — or re-derives
it, which needs no new sitting if the phase's own resident readings cover more
than one reader count. `scripts/measure.py`'s `charge_model_problem` is what
would report it: its inner fault line is exactly this constant.

**Origin.** The margin constant's derivation, 2026-09-12
([`decisions.md`](decisions.md), "I/O, memory and parallelism").

---

## A source's own file index is unbilled and grows with the input, and that was decided rather than overlooked

**Fact.** `XzSource` holds its seek table — one 80 B entry a stream, one 32 B a
block — shared with the `xz_seek::Reader` that built it, and no charge bills it:
3.33 MiB held on koji's 31,150-stream download against 4.11 KiB on the
3 GiB fixtures `MEMORY_UNPOOLED_BOUND` was read off. It is the only unbilled
term in the account that grows with the *input* rather than with the reader
count, the decode unit or the announced chunk. **Not billing it was argued from
an ordering, not from its size**: the index is built before `hint_parallelism`
states a budget, so a charge carrying it could not refuse a file whose index
does not fit — it would only subtract an already-spent allocation from the
allowance a worker count is solved against. That buys accuracy in the account
and no protection, at the cost of the count-independent third `WorkerMemory`
term `KD24` priced and left untaken. Registered as `KD26`
([`decisions.md`](decisions.md), "D4").

**Why this phase cares.** A seekable source of this phase's codec carries the
same kind of structure — the zstd seekable format's own seek table is one entry
a frame — so the phase meets this decision in its own terms and inherits both
halves of the answer: the ordering argument that says an index need not be
billed, and the warning that the argument is only sound while the index stays
small next to the bound, which is a property of the producer's frame size
rather than of the codec. It also inherits a shape to insist on — `xz_seek`
took its table **by value** and so held a second copy of it until that API
was widened to share one, so a decoder crate written or vendored for this phase
should hand out a shareable handle from the start.

**Origin.** The seek table's account, 2026-09-12
([`decisions.md`](decisions.md), "D4";
[`../status/history/2026-09-12.md`](../status/history/2026-09-12.md), "The seek
table is held twice, and the walk runs before the budget does").

---

## `xz-seek`'s publication waits for all three codecs to sit well together

**Fact.** The `.xz` addressing layer is still a frozen vendored read-only copy
under `vendor/xz-seek/` rather than a published crate
([`decisions.md`](decisions.md), "D14"). **The maintainer's condition is
explicit: publish once gzip and zstd are supported and all three codecs play
nicely in the same codebase.** A "two real consumers vetting the interface"
gate was recorded here too, and that crate carries no such condition in its own
record; the row-group statistics phase did turn out to make no call into it.

P14 turned out to need a real change rather than none: a remote source does not
compose for free, because that crate's positional trait is synchronous and ours
is async, and the seam was fixed upstream rather than bridged here
([`roadmap-P14-remote-input.md`](roadmap-P14-remote-input.md), "D13").

**Why this phase cares.** This phase is the last of the three. When its codec
lands, the condition is satisfiable for the first time, so the publication call —
publish a version and drop the vendored copy, or keep vendoring — is this phase's
to make, together with whether the crate's name, still provisional, survives.
What it has to weigh is whether one addressing layer serves three codecs or
whether each wants its own, which is a question only a tree holding all three can
answer.

**Origin.** P14's grilling, 2026-09-17, which drained the entry that had carried
the gate; the vendoring decision itself is
[`decisions.md`](decisions.md), "D14". **Contingent on**
`pgdump_query/Cargo.toml` still naming a path dependency on `vendor/xz-seek`.
