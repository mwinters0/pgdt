# P15 inbox — facts filed for its grilling

Evidence that P15 (gzip input) will need. **This is a queue, not a
document**: when P15 is grilled, walk every entry, fold it into the spec or
discard it as stale, and delete this file. See `docs/process.md`, "Inboxes:
facts filed by destination".

---

## Relaxing `size()` is cheaper than it looks, and the `.xz` source deliberately did not spend it

**Fact.** `ByteRangeSource::size()` still means the exact addressable
length, because xz answers it exactly from its stream index. The alternative was
weighed first: every read loop in the library is already written as
`want = chunk_size.min(size - read_pos)` … `read_pos += bytes.len()`, so **all
of them already tolerate a short read**. What actually depends on the number
being exact is the loop's exit test, and the coverage denominator —
`total_size`, `scanned_through`, and `pgdq info`'s percentages.

**Why P15 cares.** This is the decision this phase cannot avoid, since gzip
cannot answer it: `ISIZE` is the uncompressed length mod 2^32, useless above
4 GiB, and the single-member shape has nothing else to offer short of a full
decode. So the choice is a first pass to compute the size, or a relaxed
contract — and the relaxation's cost is now known to be a loop-termination
change plus an honest answer for `info` when the total is not yet known, rather
than a rewrite of every caller. Note that the decision is shared with P18,
which meets the same problem in zstd's form; whichever phase runs first settles
the contract for both.

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

**Why P15 cares.** This phase will want an equivalent addressing layer for the
multi-member shape — BGZF above all — and the natural instinct will be to
extend the xz crate. The interface is reusable; the size guarantee is not.
Whether that becomes a sibling crate, a generalized trait across both, or
something this repo owns is P15's to decide — but it decides it knowing the xz
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

**Why P15 cares.** A `.gz` source is the same shape of object and should not
re-decide any of them. The one that will feel wrong for gzip is the per-block
seek table — a single-member gzip index is a set of *checkpoints* carrying
32 KiB of dictionary each, not a list of independently decodable blocks. The
envelope field was shaped for exactly that: `CompressionIndex` is an enum from
its first commit with one variant in it, so this phase adds sibling variants of
its own layout rather than reshaping the field or fitting checkpoints into a
block list. It may need two of them rather than one, since the multi-member
shape's index genuinely is a block list. What still needs checking is naming
rather than shape — whether the accessor that hands a table to the cache reads
as xz-specific once a second codec is in.

**Origin.** 2026-09-02, grilling the compressed-input work.
