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
now beside the mechanism ([`architecture.md`](architecture.md), "The compressed
source", which states what `size()` promises and the two relaxations refused).

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
built and filed by subject ([`architecture.md`](architecture.md), "The
compressed source" and "The cache"): `stored_size()` on the trait with
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
